//! Per-chain checkpoints under `out/<asset>/runs/<run_id>/checkpoint/`.
//! Chain-level: full chain done → `transfers_<chain>.csv` + `chain_<chain>.json`.
//! Chunk-level (in-flight): each successful `eth_getLogs` chunk → `fetch_progress_<chain>.json` + append `fetch_partial_<chain>.csv`.

use alloy::primitives::U256;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs::OpenOptions;
use std::path::{Path, PathBuf};

use crate::decode::TransferEvent;

/// Serialized per completed chain (`checkpoint/chain_<name>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointChainBundle {
    pub supply: crate::rpc::transfer_audit::SupplyAuditRow,
    pub qa: crate::rpc::transfer_audit::QaChain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainSpecRecord {
    pub chain: String,
    pub from_block: u64,
    pub to_block_requested: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckpointManifest {
    pub schema: String,
    pub asset: String,
    pub run_id: String,
    pub started_at: String,
    pub chunk_size: u64,
    pub per_chain_spans: bool,
    pub chain_specs: Vec<ChainSpecRecord>,
    pub completed_chains: Vec<String>,
}

impl CheckpointManifest {
    pub const SCHEMA: &'static str = "transfer-audit-checkpoint-v1";

    pub fn new(
        asset: &str,
        run_id: &str,
        started_at: &str,
        chunk_size: u64,
        per_chain_spans: bool,
        chain_specs: Vec<ChainSpecRecord>,
    ) -> Self {
        Self {
            schema: Self::SCHEMA.to_string(),
            asset: asset.to_uppercase(),
            run_id: run_id.to_string(),
            started_at: started_at.to_string(),
            chunk_size,
            per_chain_spans,
            chain_specs,
            completed_chains: Vec::new(),
        }
    }
}

/// In-flight log fetch for one chain (`checkpoint/fetch_progress_<chain>.json`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FetchChunkProgress {
    pub schema: String,
    pub chain: String,
    pub contract_address: String,
    pub from_block: u64,
    pub to_block: u64,
    pub chunk_size: u64,
    /// Inclusive end block of the last successful `eth_getLogs` chunk.
    pub last_fetched_through: u64,
    pub chunks_done: u64,
    pub total_chunks: u64,
    pub logs_fetched: usize,
    /// Decoded rows persisted in `fetch_partial_<chain>.csv` for blocks `..=last_fetched_through`.
    /// Resume refuses a partial file whose row count differs (truncation / foreign rows).
    pub events_persisted: u64,
    /// Cumulative decode failures so far (restored on resume so the invariant gate stays honest).
    pub decode_errors: u64,
    /// Chunk whose `eth_getLogs` exhausted retries; it is NOT counted in `last_fetched_through`
    /// and is re-fetched first on resume. `None` when the last stop was not an RPC failure.
    #[serde(default)]
    pub unresolved: Option<UnresolvedInterval>,
    /// Chunk-derived QA notes (decode QA sample + first decode errors), in order, so a resumed run
    /// reports the same `qa_report.json` / manifest warnings as an uninterrupted one.
    #[serde(default)]
    pub chunk_notes: Vec<String>,
}

/// Inclusive block interval that could not be fetched; kept explicit in the checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnresolvedInterval {
    pub from_block: u64,
    pub to_block: u64,
    pub error: String,
}

impl FetchChunkProgress {
    /// v2: adds `events_persisted`, `decode_errors`, `unresolved`; partial CSV rows are validated and
    /// `value_u256` is restored from `value_raw` on load. v1 progress files are rejected (fail closed).
    pub const SCHEMA: &'static str = "transfer-audit-fetch-chunk-v2";

    pub fn resume_from_block(&self) -> u64 {
        self.last_fetched_through.saturating_add(1)
    }

    pub fn is_complete(&self) -> bool {
        self.last_fetched_through >= self.to_block
    }
}

pub fn checkpoint_root(out_dir: &Path) -> PathBuf {
    out_dir.join("checkpoint")
}

pub fn manifest_path(out_dir: &Path) -> PathBuf {
    checkpoint_root(out_dir).join("manifest.json")
}

fn transfers_path(out_dir: &Path, chain: &str) -> PathBuf {
    checkpoint_root(out_dir).join(format!("transfers_{chain}.csv"))
}

fn chain_bundle_path(out_dir: &Path, chain: &str) -> PathBuf {
    checkpoint_root(out_dir).join(format!("chain_{chain}.json"))
}

pub fn fetch_progress_path(out_dir: &Path, chain: &str) -> PathBuf {
    checkpoint_root(out_dir).join(format!("fetch_progress_{chain}.json"))
}

pub fn fetch_partial_path(out_dir: &Path, chain: &str) -> PathBuf {
    checkpoint_root(out_dir).join(format!("fetch_partial_{chain}.csv"))
}

/// Local checkpoint I/O failure (as opposed to an RPC failure). Carried through the fetch loop so
/// callers can tell the two apart.
#[derive(Debug)]
pub struct StorageError(pub String);

impl std::fmt::Display for StorageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for StorageError {}

impl StorageError {
    pub fn wrap(e: anyhow::Error) -> anyhow::Error {
        anyhow::Error::new(StorageError(format!("{e:#}")))
    }
}

/// Test-only fault injection, scoped by path prefix so parallel tests cannot interfere.
#[cfg(test)]
pub(crate) mod fault {
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;

    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum Op {
        /// `save_fetch_progress` returns Err before writing anything.
        SaveProgress,
        /// `append_fetch_partial_events` returns Err before writing anything.
        Append,
        /// `append_fetch_partial_events` writes half the bytes, then returns Err without cleanup (simulated crash, torn tail).
        TornAppend,
        /// `write_atomic` writes the tmp file, then fails before the rename.
        Rename,
        /// `write_atomic` fails before creating the tmp file.
        TmpCreate,
        /// `write_atomic` writes half the tmp file, then fails.
        TmpWrite,
        /// `write_atomic` fails at the tmp file fsync.
        TmpSync,
        /// `write_atomic` fails AFTER a successful rename (directory fsync error): the new file is
        /// already in place although the caller sees an error.
        DirSync,
        /// `save_completed_chain` fails before stage k (1 csv, 2 bundle, 3 manifest, 4 cleanup).
        Stage(u8),
        /// `clear_chain_fetch_progress` finds the progress file replaced by a non-empty directory,
        /// so the REAL `remove_file` call fails (a failure `.ok()` would have swallowed).
        BlockRemove,
    }

    struct Fault {
        prefix: PathBuf,
        op: Op,
        nth: i64,
    }

    static FAULTS: Mutex<Vec<Fault>> = Mutex::new(Vec::new());

    /// Fail the `nth` (0-based) matching call under `prefix`, once.
    pub fn arm(prefix: &Path, op: Op, nth: i64) {
        FAULTS.lock().unwrap().push(Fault {
            prefix: prefix.to_path_buf(),
            op,
            nth,
        });
    }

    pub fn disarm(prefix: &Path) {
        FAULTS.lock().unwrap().retain(|f| f.prefix != prefix);
    }

    pub fn hit(path: &Path, op: Op) -> bool {
        let mut v = FAULTS.lock().unwrap();
        let mut fire = false;
        let mut i = 0;
        while i < v.len() {
            if v[i].op == op && path.starts_with(&v[i].prefix) {
                if v[i].nth == 0 {
                    v.remove(i);
                    fire = true;
                    continue;
                }
                v[i].nth -= 1;
            }
            i += 1;
        }
        fire
    }
}

/// Write via temp file + rename so a crash never leaves a half-written JSON checkpoint.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write;
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    #[cfg(test)]
    if fault::hit(path, fault::Op::TmpCreate) {
        anyhow::bail!("injected tmp create failure");
    }
    {
        let mut f =
            std::fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        #[cfg(test)]
        if fault::hit(path, fault::Op::TmpWrite) {
            let _ = f.write_all(&bytes[..bytes.len() / 2]);
            anyhow::bail!("injected tmp write failure");
        }
        f.write_all(bytes)
            .with_context(|| format!("write {}", tmp.display()))?;
        #[cfg(test)]
        if fault::hit(path, fault::Op::TmpSync) {
            anyhow::bail!("injected tmp fsync failure");
        }
        f.sync_all()
            .with_context(|| format!("fsync {}", tmp.display()))?;
    }
    #[cfg(test)]
    if fault::hit(path, fault::Op::Rename) {
        anyhow::bail!(
            "injected failure between tmp write and rename of {}",
            path.display()
        );
    }
    std::fs::rename(&tmp, path).with_context(|| format!("rename to {}", path.display()))?;
    #[cfg(test)]
    if fault::hit(path, fault::Op::DirSync) {
        anyhow::bail!(
            "injected directory fsync failure after rename of {}",
            path.display()
        );
    }
    sync_parent_dir(path)
}

/// fsync the directory so a completed rename/create survives a crash.
fn sync_parent_dir(path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        let d = std::fs::File::open(dir).with_context(|| format!("open dir {}", dir.display()))?;
        d.sync_all()
            .with_context(|| format!("fsync dir {}", dir.display()))?;
    }
    Ok(())
}

pub fn clear_checkpoint_dir(out_dir: &Path) -> Result<()> {
    let root = checkpoint_root(out_dir);
    if root.exists() {
        std::fs::remove_dir_all(&root).with_context(|| format!("remove {}", root.display()))?;
    }
    Ok(())
}

pub fn clear_chain_fetch_progress(out_dir: &Path, chain: &str) -> Result<()> {
    for p in [
        fetch_progress_path(out_dir, chain),
        fetch_partial_path(out_dir, chain),
    ] {
        #[cfg(test)]
        if p == fetch_progress_path(out_dir, chain) && fault::hit(&p, fault::Op::BlockRemove) {
            let _ = std::fs::remove_file(&p);
            std::fs::create_dir_all(&p)?;
            std::fs::write(p.join("blocker"), b"x")?;
        }
        match std::fs::remove_file(&p) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(anyhow::Error::new(e).context(format!("remove {}", p.display()))),
        }
    }
    sync_parent_dir(&fetch_progress_path(out_dir, chain))
}

pub fn load_manifest(out_dir: &Path) -> Result<Option<CheckpointManifest>> {
    let path = manifest_path(out_dir);
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(&path).with_context(|| path.display().to_string())?;
    let m: CheckpointManifest = serde_json::from_str(&raw).with_context(|| {
        format!(
            "checkpoint manifest {} is truncated, corrupt or missing fields; use --fresh to restart",
            path.display()
        )
    })?;
    if m.schema != CheckpointManifest::SCHEMA {
        anyhow::bail!(
            "unsupported checkpoint schema {:?} in {}; use --fresh to restart",
            m.schema,
            path.display()
        );
    }
    if chrono::DateTime::parse_from_rfc3339(&m.started_at).is_err() {
        anyhow::bail!(
            "checkpoint manifest {} has unparseable started_at {:?}; use --fresh",
            path.display(),
            m.started_at
        );
    }
    let spec_chains: HashSet<&str> = m.chain_specs.iter().map(|c| c.chain.as_str()).collect();
    for c in &m.completed_chains {
        if !spec_chains.contains(c.as_str()) {
            anyhow::bail!(
                "checkpoint manifest {} lists completed chain {:?} that is not in its chain_specs; use --fresh",
                path.display(),
                c
            );
        }
    }
    Ok(Some(m))
}

pub fn save_manifest(out_dir: &Path, manifest: &CheckpointManifest) -> Result<()> {
    let root = checkpoint_root(out_dir);
    std::fs::create_dir_all(&root)?;
    let path = manifest_path(out_dir);
    write_atomic(&path, serde_json::to_string_pretty(manifest)?.as_bytes())
}

pub fn validate_manifest_matches(
    manifest: &CheckpointManifest,
    asset: &str,
    run_id: &str,
    chunk_size: u64,
    per_chain_spans: bool,
    chain_specs: &[ChainSpecRecord],
) -> Result<()> {
    if manifest.asset.to_uppercase() != asset.to_uppercase() {
        anyhow::bail!(
            "checkpoint asset {:?} != --asset {:?}; use --fresh or a different --run-id",
            manifest.asset,
            asset
        );
    }
    if manifest.run_id != run_id {
        anyhow::bail!(
            "checkpoint run_id {:?} != --run-id {:?}; use --fresh or matching --run-id",
            manifest.run_id,
            run_id
        );
    }
    if manifest.chunk_size != chunk_size {
        anyhow::bail!(
            "checkpoint chunk_size {} != current {} ; use --fresh",
            manifest.chunk_size,
            chunk_size
        );
    }
    if manifest.per_chain_spans != per_chain_spans {
        anyhow::bail!("checkpoint per_chain_spans mismatch; use --fresh");
    }
    let mut want: Vec<_> = chain_specs.to_vec();
    let mut have = manifest.chain_specs.clone();
    want.sort_by(|a, b| a.chain.cmp(&b.chain));
    have.sort_by(|a, b| a.chain.cmp(&b.chain));
    if want != have {
        anyhow::bail!(
            "checkpoint window specs differ from current CLI args; use --fresh.\n  checkpoint: {:?}\n  current:    {:?}",
            have,
            want
        );
    }
    Ok(())
}

pub fn completed_set(manifest: &CheckpointManifest) -> HashSet<String> {
    manifest.completed_chains.iter().cloned().collect()
}

fn addr_norm(a: &str) -> String {
    a.trim_start_matches("0x").to_lowercase()
}

pub fn load_fetch_progress(
    out_dir: &Path,
    chain: &str,
    contract_address: &str,
    from_block: u64,
    to_block: u64,
    chunk_size: u64,
) -> Result<Option<FetchChunkProgress>> {
    let path = fetch_progress_path(out_dir, chain);
    if !path.exists() {
        return Ok(None);
    }
    let p: FetchChunkProgress = serde_json::from_str(
        &std::fs::read_to_string(&path).with_context(|| path.display().to_string())?,
    )
    .with_context(|| {
        format!(
            "fetch checkpoint {} is truncated, corrupt, missing fields or has an old schema; use --fresh",
            path.display()
        )
    })?;
    if p.schema != FetchChunkProgress::SCHEMA {
        anyhow::bail!(
            "unsupported fetch progress schema {:?} in {} (expected {:?}); use --fresh",
            p.schema,
            path.display(),
            FetchChunkProgress::SCHEMA
        );
    }
    if p.chain != chain
        || p.from_block != from_block
        || p.to_block != to_block
        || p.chunk_size != chunk_size
        || addr_norm(&p.contract_address) != addr_norm(contract_address)
    {
        anyhow::bail!(
            "fetch checkpoint for {:?} does not match current window; use --fresh or delete {}",
            chain,
            path.display()
        );
    }
    validate_progress_consistency(&p, &path)?;
    Ok(Some(p))
}

/// Internal-consistency checks so a hand-edited or damaged progress file cannot skip block intervals.
fn validate_progress_consistency(p: &FetchChunkProgress, path: &Path) -> Result<()> {
    let bad = |why: String| -> Result<()> {
        anyhow::bail!(
            "inconsistent fetch checkpoint {}: {why}; use --fresh",
            path.display()
        )
    };
    if p.chunk_size == 0 || p.from_block == 0 || p.to_block < p.from_block {
        return bad("invalid window".into());
    }
    // `from_block - 1` means "no chunk finished yet" (written when the first chunk is unresolved).
    if p.last_fetched_through < p.from_block - 1 || p.last_fetched_through > p.to_block {
        return bad(format!(
            "last_fetched_through {} outside [{}, {}]",
            p.last_fetched_through,
            p.from_block - 1,
            p.to_block
        ));
    }
    let done_span = p.last_fetched_through + 1 - p.from_block;
    if p.last_fetched_through != p.to_block && done_span.checked_rem(p.chunk_size) != Some(0) {
        return bad(format!(
            "last_fetched_through {} is not on a chunk boundary",
            p.last_fetched_through
        ));
    }
    let want_done = count_chunks(p.from_block, p.last_fetched_through, p.chunk_size);
    let want_total = count_chunks(p.from_block, p.to_block, p.chunk_size);
    if p.chunks_done != want_done || p.total_chunks != want_total {
        return bad(format!(
            "chunks_done/total_chunks {}/{} != expected {}/{}",
            p.chunks_done, p.total_chunks, want_done, want_total
        ));
    }
    if p.decode_errors > p.logs_fetched as u64 {
        return bad(format!(
            "decode_errors {} > logs_fetched {}",
            p.decode_errors, p.logs_fetched
        ));
    }
    if p.events_persisted > p.logs_fetched as u64 {
        return bad(format!(
            "events_persisted {} > logs_fetched {}",
            p.events_persisted, p.logs_fetched
        ));
    }
    // Every fetched log yields exactly one decoded event or one decode error.
    if p.events_persisted + p.decode_errors != p.logs_fetched as u64 {
        return bad(format!(
            "events_persisted {} + decode_errors {} != logs_fetched {}",
            p.events_persisted, p.decode_errors, p.logs_fetched
        ));
    }
    if let Some(u) = &p.unresolved {
        if p.is_complete() {
            return bad("window marked complete but has an unresolved interval".into());
        }
        if u.from_block != p.resume_from_block()
            || u.to_block < u.from_block
            || u.to_block > p.to_block
        {
            return bad(format!(
                "unresolved interval {}..{} does not start at resume block {}",
                u.from_block,
                u.to_block,
                p.resume_from_block()
            ));
        }
    }
    Ok(())
}

pub fn save_fetch_progress(out_dir: &Path, progress: &FetchChunkProgress) -> Result<()> {
    #[cfg(test)]
    if fault::hit(
        &fetch_progress_path(out_dir, &progress.chain),
        fault::Op::SaveProgress,
    ) {
        anyhow::bail!("injected progress write failure");
    }
    std::fs::create_dir_all(checkpoint_root(out_dir))?;
    let path = fetch_progress_path(out_dir, &progress.chain);
    write_atomic(&path, serde_json::to_string_pretty(progress)?.as_bytes())
}

const ZERO_ADDR: &str = "0x0000000000000000000000000000000000000000";

/// `value_u256` is `#[serde(skip)]`, so a CSV round trip yields 0. Rebuild it from the persisted
/// `value_raw` (decimal) and check the row is internally consistent. Fails closed on garbage.
fn restore_event(mut ev: TransferEvent, row: usize, path: &Path) -> Result<TransferEvent> {
    let ctx = |why: &str| {
        anyhow::anyhow!(
            "{}: row {row}: {why} (tx {} log_index {}); checkpoint is corrupt, use --fresh",
            path.display(),
            ev.tx_hash,
            ev.log_index
        )
    };
    if ev.value_raw.is_empty() || !ev.value_raw.bytes().all(|b| b.is_ascii_digit()) {
        return Err(ctx(&format!(
            "value_raw {:?} is not a decimal integer",
            ev.value_raw
        )));
    }
    let value = U256::from_str_radix(&ev.value_raw, 10).map_err(|_| {
        ctx(&format!(
            "value_raw {:?} does not fit in uint256",
            ev.value_raw
        ))
    })?;
    let kind = if ev.from == ZERO_ADDR {
        "mint"
    } else if ev.to == ZERO_ADDR {
        "burn"
    } else {
        "transfer"
    };
    if ev.kind != kind {
        return Err(ctx(&format!(
            "kind {:?} contradicts from/to (expected {kind:?})",
            ev.kind
        )));
    }
    ev.value_u256 = value;
    Ok(ev)
}

fn read_events_csv(path: &Path) -> Result<Vec<TransferEvent>> {
    let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
    parse_events_csv(&bytes, path)
}

fn parse_events_csv(bytes: &[u8], path: &Path) -> Result<Vec<TransferEvent>> {
    let mut events = Vec::new();
    let mut rdr = csv::Reader::from_reader(bytes);
    for (i, row) in rdr.deserialize::<TransferEvent>().enumerate() {
        let ev = row.with_context(|| {
            format!(
                "{}: row {} is truncated or malformed; use --fresh",
                path.display(),
                i + 1
            )
        })?;
        events.push(restore_event(ev, i + 1, path)?);
    }
    Ok(events)
}

fn events_to_csv_bytes(events: &[TransferEvent], header: bool) -> Result<Vec<u8>> {
    let mut wtr = csv::WriterBuilder::new()
        .has_headers(header)
        .from_writer(Vec::new());
    for ev in events {
        wtr.serialize(ev)?;
    }
    wtr.into_inner()
        .map_err(|e| anyhow::anyhow!("csv buffer: {e}"))
}

/// Load and validate the in-flight partial events for `progress`.
///
/// * restores `value_u256` from `value_raw`;
/// * rows from a chunk that was appended but whose progress was never saved (crash between the two
///   writes; blocks beyond `last_fetched_through`) are dropped and the file rewritten, because that
///   chunk is fetched again;
/// * rows must belong to this chain/contract/window and match `events_persisted`.
pub fn load_fetch_partial_events(
    out_dir: &Path,
    progress: &FetchChunkProgress,
) -> Result<Vec<TransferEvent>> {
    let path = fetch_partial_path(out_dir, &progress.chain);
    if !path.exists() {
        if progress.events_persisted == 0 {
            return Ok(Vec::new());
        }
        anyhow::bail!(
            "{} is missing but fetch progress records {} persisted events; use --fresh",
            path.display(),
            progress.events_persisted
        );
    }
    // A chunk's rows are written with a single append that ends in a newline, and the progress file
    // is saved only after that append is fsynced. A final line without a newline is therefore a torn
    // append of an UNcommitted chunk: drop it (it is fetched again). Anything else wrong still fails.
    let bytes = std::fs::read(&path).with_context(|| path.display().to_string())?;
    let torn = !bytes.is_empty() && !bytes.ends_with(b"\n");
    let usable: &[u8] = if torn {
        match bytes.iter().rposition(|b| *b == b'\n') {
            Some(i) => &bytes[..=i],
            None => &[],
        }
    } else {
        &bytes
    };
    let all = parse_events_csv(usable, &path)?;
    let total = all.len();
    let mut kept = Vec::with_capacity(total);
    for (i, ev) in all.into_iter().enumerate() {
        if ev.chain != progress.chain
            || addr_norm(&ev.contract_address) != addr_norm(&progress.contract_address)
        {
            anyhow::bail!(
                "{}: row {} belongs to {}/{}, expected {}/{}; use --fresh",
                path.display(),
                i + 1,
                ev.chain,
                ev.contract_address,
                progress.chain,
                progress.contract_address
            );
        }
        if ev.block_number < progress.from_block || ev.block_number > progress.to_block {
            anyhow::bail!(
                "{}: row {} block {} outside window {}..{}; use --fresh",
                path.display(),
                i + 1,
                ev.block_number,
                progress.from_block,
                progress.to_block
            );
        }
        if ev.block_number > progress.last_fetched_through {
            continue; // orphan of an un-checkpointed chunk; will be re-fetched
        }
        kept.push(ev);
    }
    if kept.len() as u64 != progress.events_persisted {
        anyhow::bail!(
            "{}: {} usable rows but fetch progress records {}; file truncated or modified; use --fresh",
            path.display(),
            kept.len(),
            progress.events_persisted
        );
    }
    if kept.len() != total || torn {
        rewrite_partial(&path, &kept)?;
    }
    Ok(kept)
}

fn rewrite_partial(path: &Path, events: &[TransferEvent]) -> Result<()> {
    if events.is_empty() {
        std::fs::remove_file(path)?; // append recreates it with a header
        return sync_parent_dir(path);
    }
    write_atomic(path, &events_to_csv_bytes(events, true)?)
}

/// Append one chunk's rows with a single write + fsync. On a real write error the file is cut back
/// to its previous length so a short write cannot poison the checkpoint.
pub fn append_fetch_partial_events(
    out_dir: &Path,
    chain: &str,
    new_rows: &[TransferEvent],
) -> Result<()> {
    use std::io::Write;
    if new_rows.is_empty() {
        return Ok(());
    }
    std::fs::create_dir_all(checkpoint_root(out_dir))?;
    let path = fetch_partial_path(out_dir, chain);
    let pre_len = path.metadata().map(|m| m.len()).unwrap_or(0);
    let bytes = events_to_csv_bytes(new_rows, pre_len == 0)?;
    #[cfg(test)]
    if fault::hit(&path, fault::Op::Append) {
        anyhow::bail!("injected partial CSV append failure");
    }
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    #[cfg(test)]
    if fault::hit(&path, fault::Op::TornAppend) {
        // Simulated crash mid-write: half the bytes land, no cleanup runs.
        file.write_all(&bytes[..bytes.len() / 2])?;
        file.sync_all()?;
        anyhow::bail!("injected torn partial CSV append");
    }
    let res = file.write_all(&bytes).and_then(|_| file.sync_all());
    if let Err(e) = res {
        let _ = file.set_len(pre_len);
        return Err(anyhow::Error::new(e).context(format!("append {}", path.display())));
    }
    if pre_len == 0 {
        sync_parent_dir(&path)?;
    }
    Ok(())
}

pub fn load_completed_chain(
    out_dir: &Path,
    chain: &str,
) -> Result<(Vec<TransferEvent>, CheckpointChainBundle)> {
    let csv_path = transfers_path(out_dir, chain);
    let json_path = chain_bundle_path(out_dir, chain);
    if !csv_path.exists() || !json_path.exists() {
        anyhow::bail!(
            "checkpoint for chain {:?} incomplete (missing transfers or chain bundle); use --fresh",
            chain
        );
    }
    let events = read_events_csv(&csv_path)?;
    let bundle: CheckpointChainBundle = serde_json::from_str(
        &std::fs::read_to_string(&json_path).with_context(|| json_path.display().to_string())?,
    )
    .with_context(|| {
        format!(
            "chain bundle {} is truncated, corrupt or missing fields; use --fresh",
            json_path.display()
        )
    })?;
    // Cross-check CSV against the bundle that summarised it.
    if events.len() != bundle.supply.transfer_event_count {
        anyhow::bail!(
            "{}: {} rows but chain bundle records {} transfers; use --fresh",
            csv_path.display(),
            events.len(),
            bundle.supply.transfer_event_count
        );
    }
    let sum = |kind: &str| {
        events
            .iter()
            .filter(|e| e.kind == kind)
            .fold(U256::ZERO, |a, e| a + e.value_u256)
            .to_string()
    };
    if sum("mint") != bundle.supply.sum_mints_raw || sum("burn") != bundle.supply.sum_burns_raw {
        anyhow::bail!(
            "{}: mint/burn sums recomputed from rows ({}/{}) differ from chain bundle ({}/{}); use --fresh",
            csv_path.display(),
            sum("mint"),
            sum("burn"),
            bundle.supply.sum_mints_raw,
            bundle.supply.sum_burns_raw
        );
    }
    Ok((events, bundle))
}

pub fn save_completed_chain(
    out_dir: &Path,
    manifest: &mut CheckpointManifest,
    chain: &str,
    events: &[TransferEvent],
    bundle: &CheckpointChainBundle,
) -> Result<()> {
    let root = checkpoint_root(out_dir);
    std::fs::create_dir_all(&root)?;
    // Order matters for crash safety: (1) final transfers CSV, (2) chain bundle, (3) the manifest
    // entry that declares the chain complete (the commit point), (4) only then drop the in-flight
    // fetch progress. A failure before (3) leaves the fetch progress intact, so resume redoes the
    // cheap tail; a failure at (4) leaves stale files that the completed manifest makes irrelevant.
    #[cfg(test)]
    if fault::hit(&root, fault::Op::Stage(1)) {
        anyhow::bail!("injected failure at completed-chain stage 1 (transfers csv)");
    }
    let csv_path = transfers_path(out_dir, chain);
    write_atomic(&csv_path, &events_to_csv_bytes(events, true)?)?;
    #[cfg(test)]
    if fault::hit(&root, fault::Op::Stage(2)) {
        anyhow::bail!("injected failure at completed-chain stage 2 (chain bundle)");
    }
    let json_path = chain_bundle_path(out_dir, chain);
    write_atomic(&json_path, serde_json::to_string_pretty(bundle)?.as_bytes())?;
    #[cfg(test)]
    if fault::hit(&root, fault::Op::Stage(3)) {
        anyhow::bail!("injected failure at completed-chain stage 3 (manifest)");
    }
    if !manifest.completed_chains.iter().any(|c| c == chain) {
        manifest.completed_chains.push(chain.to_string());
        manifest.completed_chains.sort();
    }
    save_manifest(out_dir, manifest)?;
    #[cfg(test)]
    if fault::hit(&root, fault::Op::Stage(4)) {
        anyhow::bail!("injected failure at completed-chain stage 4 (cleanup)");
    }
    clear_chain_fetch_progress(out_dir, chain).with_context(|| {
        format!(
            "chain {chain:?}: completed-chain manifest ALREADY COMMITTED, but cleanup of the in-flight \
fetch-progress files failed; retrying the same run recognizes the chain as complete (no refetch) and \
clears the stale checkpoint files"
        )
    })?;
    println!(
        "[{}] chain checkpoint saved ({} transfer rows) — safe to stop; re-run same --run-id to resume",
        chain.to_uppercase(),
        events.len()
    );
    Ok(())
}

pub fn remove_checkpoint_dir(out_dir: &Path) -> Result<()> {
    clear_checkpoint_dir(out_dir)
}

/// Inclusive chunk count for `from_block..=to_block` with given `chunk_size`.
pub fn count_chunks(from_block: u64, to_block: u64, chunk_size: u64) -> u64 {
    if to_block < from_block || chunk_size == 0 {
        return 0;
    }
    let span = to_block - from_block + 1;
    span.div_ceil(chunk_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::TransferEvent;
    use alloy::primitives::U256;
    use std::path::PathBuf;

    fn tmp_out(suffix: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "stablecoin_audit_cp_{}_{suffix}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&p);
        p
    }

    fn sample_event(chain: &str) -> TransferEvent {
        TransferEvent {
            chain: chain.into(),
            contract_address: "0xabc".into(),
            block_number: 10,
            tx_hash: "0xtx".into(),
            log_index: 0,
            from: "0x0".into(),
            to: "0x1".into(),
            value_raw: "1".into(),
            value_decimal: "0.000001".into(),
            kind: "transfer".into(),
            value_u256: U256::from(1u64),
        }
    }

    fn sample_bundle(chain: &str) -> CheckpointChainBundle {
        let json = format!(
            r#"{{
              "supply": {{
                "chain": "{chain}",
                "chain_id": 1,
                "contract_address": "0xabc",
                "from_block": 100,
                "resolved_to_block": 200,
                "to_block_requested": "200",
                "chunk_size": 500,
                "transfer_event_count": 1,
                "active_senders": 1,
                "active_recipients": 1,
                "mint_count": 0,
                "burn_count": 0,
                "plain_transfer_count": 1,
                "sum_mints_raw": "0",
                "sum_burns_raw": "0",
                "total_supply_at_start_minus_1_provenance": "on-chain",
                "metadata_call_pass": true,
                "historical_supply_pass": true,
                "duplicate_count": 0,
                "full_decode_error_count": 0
              }},
              "qa": {{
                "chain": "{chain}",
                "chain_id": 1,
                "contract_address": "0xabc",
                "from_block": 100,
                "resolved_to_block": 200,
                "gates": {{
                  "metadata_call_pass": "PASS",
                  "historical_supply_pass": "PASS",
                  "no_duplicate_logs_pass": "PASS",
                  "transfer_decode_pass": "PASS",
                  "supply_invariant_pass": "PASS",
                  "provenance_stamped": "PASS"
                }},
                "duplicate_count": 0,
                "full_decode_error_count": 0,
                "errors": []
              }}
            }}"#
        );
        serde_json::from_str(&json).expect("test bundle json")
    }

    #[test]
    fn count_chunks_single_and_multi() {
        assert_eq!(count_chunks(100, 100, 500), 1);
        assert_eq!(count_chunks(100, 599, 500), 1);
        assert_eq!(count_chunks(100, 600, 500), 2);
        assert_eq!(count_chunks(200, 100, 500), 0);
        assert_eq!(count_chunks(100, 200, 0), 0);
    }

    #[test]
    fn fetch_progress_resume_and_complete() {
        let p = FetchChunkProgress {
            schema: FetchChunkProgress::SCHEMA.into(),
            chain: "ethereum".into(),
            contract_address: "0xAbC".into(),
            from_block: 1,
            to_block: 1000,
            chunk_size: 500,
            last_fetched_through: 499,
            chunks_done: 1,
            total_chunks: 2,
            logs_fetched: 10,
            events_persisted: 10,
            decode_errors: 0,
            unresolved: None,
            chunk_notes: Vec::new(),
        };
        assert_eq!(p.resume_from_block(), 500);
        assert!(!p.is_complete());
        let done = FetchChunkProgress {
            last_fetched_through: 1000,
            ..p
        };
        assert!(done.is_complete());
    }

    #[test]
    fn manifest_roundtrip_and_validate() {
        let out = tmp_out("manifest");
        let specs = vec![
            ChainSpecRecord {
                chain: "base".into(),
                from_block: 10,
                to_block_requested: "20".into(),
            },
            ChainSpecRecord {
                chain: "ethereum".into(),
                from_block: 100,
                to_block_requested: "200".into(),
            },
        ];
        let mut manifest = CheckpointManifest::new(
            "usdc",
            "run_a",
            "2026-01-01T00:00:00Z",
            500,
            true,
            specs.clone(),
        );
        save_manifest(&out, &manifest).unwrap();
        let loaded = load_manifest(&out).unwrap().expect("manifest exists");
        validate_manifest_matches(&loaded, "USDC", "run_a", 500, true, &specs).unwrap();
        assert!(validate_manifest_matches(&loaded, "USDC", "other", 500, true, &specs).is_err());
        manifest.completed_chains.push("ethereum".into());
        assert!(completed_set(&manifest).contains("ethereum"));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn fetch_progress_addr_case_insensitive() {
        let out = tmp_out("fetch_prog");
        let progress = FetchChunkProgress {
            schema: FetchChunkProgress::SCHEMA.into(),
            chain: "ethereum".into(),
            contract_address: "0xABCDEF".into(),
            from_block: 1,
            to_block: 10,
            chunk_size: 5,
            last_fetched_through: 5,
            chunks_done: 1,
            total_chunks: 2,
            logs_fetched: 3,
            events_persisted: 3,
            decode_errors: 0,
            unresolved: None,
            chunk_notes: Vec::new(),
        };
        save_fetch_progress(&out, &progress).unwrap();
        let loaded = load_fetch_progress(&out, "ethereum", "0xabcdef", 1, 10, 5)
            .unwrap()
            .expect("progress");
        assert_eq!(loaded.last_fetched_through, 5);
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn partial_events_append_and_load_restores_value_u256() {
        let out = tmp_out("partial");
        let mut ev = sample_event("base");
        ev.value_raw = "1180591620717411303424".into(); // 2^70 > u64::MAX
        ev.value_u256 = U256::from(1u128 << 70);
        let mut ev2 = ev.clone();
        ev2.log_index = 1;
        append_fetch_partial_events(&out, "base", &[ev.clone()]).unwrap();
        append_fetch_partial_events(&out, "base", &[ev2]).unwrap();
        let progress = FetchChunkProgress {
            schema: FetchChunkProgress::SCHEMA.into(),
            chain: "base".into(),
            contract_address: "0xabc".into(),
            from_block: 1,
            to_block: 20,
            chunk_size: 10,
            last_fetched_through: 10,
            chunks_done: 1,
            total_chunks: 2,
            logs_fetched: 2,
            events_persisted: 2,
            decode_errors: 0,
            unresolved: None,
            chunk_notes: Vec::new(),
        };
        let rows = load_fetch_partial_events(&out, &progress).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].value_u256, U256::from(1u128 << 70));
        let _ = std::fs::remove_dir_all(&out);
    }

    #[test]
    fn completed_chain_roundtrip() {
        let out = tmp_out("completed");
        let mut manifest = CheckpointManifest::new("usdc", "run_b", "t0", 500, false, vec![]);
        let events = vec![sample_event("ethereum")];
        let bundle = sample_bundle("ethereum");
        save_completed_chain(&out, &mut manifest, "ethereum", &events, &bundle).unwrap();
        let (loaded_events, _bundle) = load_completed_chain(&out, "ethereum").unwrap();
        assert_eq!(loaded_events.len(), 1);
        assert_eq!(loaded_events[0].chain, "ethereum");
        assert!(manifest.completed_chains.contains(&"ethereum".to_string()));
        let _ = std::fs::remove_dir_all(&out);
    }
}
