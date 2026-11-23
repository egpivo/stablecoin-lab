//! Deterministic checkpoint/resume tests against a scripted JSON-RPC mock (no live RPC).
//!
//! The mock serves a fixed `Transfer` log fixture, fixed `totalSupply` values and can be told to fail
//! `eth_getLogs` for chunks starting at or above a given block, simulating an interrupted scan.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use alloy::primitives::U256;
use serde_json::{json, Value};
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

use crate::rpc::transfer_audit::run_per_chain_windows_at;
use crate::rpc::transfer_checkpoint as ckpt;

pub(crate) const FROM: u64 = 24_000_000;
pub(crate) const TO: u64 = 24_000_999;
pub(crate) const CHUNK: u64 = 100;
pub(crate) const RUN_ID: &str = "resume_test";
pub(crate) const STARTED_AT: &str = "2026-01-01T00:00:00+00:00";
const ZERO: &str = "0x0000000000000000000000000000000000000000";
const A: &str = "0x00000000000000000000000000000000000000aa";
const B: &str = "0x00000000000000000000000000000000000000bb";
const CONTRACT: &str = "0xa0b86991c6218b36c1d19d4a2e9eb0ce3606eb48";
const SUPPLY_START: u128 = 5_000_000_000_000;

#[derive(Clone)]
pub(crate) struct FixtureLog {
    pub block: u64,
    pub log_index: u64,
    pub from: &'static str,
    pub to: &'static str,
    pub value: u128,
}

/// Mints/burns/transfers spread over all ten 100-block chunks, including chunk-boundary blocks and
/// values larger than `u64::MAX` so a lossy u64/zero reload is visible.
pub(crate) fn fixture_logs() -> Vec<FixtureLog> {
    let big: u128 = 1_180_591_620_717_411_303_424; // 2^70
    let spec: Vec<(u64, u64, &'static str, &'static str, u128)> = vec![
        (FROM, 0, ZERO, A, 9_000_000_000_000),
        (FROM + 50, 0, A, B, 123),
        (FROM + 99, 0, ZERO, B, big),
        (FROM + 100, 0, B, ZERO, 7_000_000),
        (FROM + 100, 1, ZERO, A, 11_000_000),
        (FROM + 250, 3, A, ZERO, 1_000_001),
        (FROM + 399, 0, ZERO, A, 2_000_000_000),
        (FROM + 400, 0, A, B, 55),
        (FROM + 499, 2, ZERO, B, 3_000_000_000_000),
        (FROM + 500, 0, B, ZERO, 4_000_000),
        (FROM + 650, 0, ZERO, A, 17_000_000),
        (FROM + 700, 0, A, ZERO, 9_999_999),
        (FROM + 799, 1, ZERO, B, big + 5),
        (FROM + 800, 0, B, A, 1),
        (FROM + 950, 0, A, ZERO, 123_456),
        (TO, 0, ZERO, A, 42_000_000),
    ];
    spec.into_iter()
        .map(|(block, log_index, from, to, value)| FixtureLog {
            block,
            log_index,
            from,
            to,
            value,
        })
        .collect()
}

fn word_addr(a: &str) -> String {
    format!("0x{:0>64}", a.trim_start_matches("0x"))
}

fn log_json(l: &FixtureLog) -> Value {
    json!({
        "address": CONTRACT,
        "topics": [
            "0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef",
            word_addr(l.from),
            word_addr(l.to),
        ],
        "data": format!("0x{:064x}", U256::from(l.value)),
        "blockNumber": format!("0x{:x}", l.block),
        "blockHash": format!("0x{:064x}", l.block),
        "transactionHash": format!("0x{:064x}", 0xa000_0000u64 + l.block * 10 + l.log_index),
        "transactionIndex": "0x0",
        "logIndex": format!("0x{:x}", l.log_index),
        "removed": false,
    })
}

pub(crate) fn expected_sums() -> (U256, U256) {
    let (mut mint, mut burn) = (U256::ZERO, U256::ZERO);
    for l in fixture_logs() {
        if l.from == ZERO {
            mint += U256::from(l.value);
        } else if l.to == ZERO {
            burn += U256::from(l.value);
        }
    }
    (mint, burn)
}

fn hex_bytes(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn abi_string(s: &str) -> String {
    let mut data = hex_bytes(s.as_bytes());
    while data.len().checked_rem(64) != Some(0) {
        data.push('0');
    }
    format!("0x{:064x}{:064x}{}", 32, s.len(), data)
}

/// Scripted RPC state shared with the responder.
pub(crate) struct MockState {
    /// `eth_getLogs` with `fromBlock >= fail_from` returns a JSON-RPC error (0 = never fail).
    pub fail_from: AtomicU64,
    /// When non-zero, an `eth_getLogs` starting exactly at this block returns ONLY one malformed
    /// log (undecodable Transfer): a chunk with zero events and one decode error.
    pub bad_only: AtomicU64,
    /// Bit i set -> chunk i (counting from FROM, CHUNK blocks each) returns ONLY a malformed log.
    pub bad_mask: AtomicU64,
    /// Every `(fromBlock, toBlock, succeeded)` requested via `eth_getLogs`, in order.
    pub log_requests: Mutex<Vec<(u64, u64, bool)>>,
}

struct Responder(Arc<MockState>);

fn hex_u64(v: &Value) -> u64 {
    u64::from_str_radix(v.as_str().unwrap().trim_start_matches("0x"), 16).unwrap()
}

impl Respond for Responder {
    fn respond(&self, req: &Request) -> ResponseTemplate {
        let body: Value = serde_json::from_slice(&req.body).unwrap();
        let id = body["id"].clone();
        let params = &body["params"];
        let ok = |result: Value| {
            ResponseTemplate::new(200)
                .set_body_json(json!({"jsonrpc":"2.0","id":id,"result":result}))
        };
        match body["method"].as_str().unwrap() {
            "eth_chainId" => ok(json!("0x1")),
            "eth_blockNumber" => ok(json!(format!("0x{TO:x}"))),
            "eth_getBlockByNumber" => ok(Value::Null),
            "eth_call" => {
                let input = params[0]
                    .get("input")
                    .or_else(|| params[0].get("data"))
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                if input.contains("06fdde03") {
                    ok(json!(abi_string("USD Coin")))
                } else if input.contains("95d89b41") {
                    ok(json!(abi_string("USDC")))
                } else if input.contains("313ce567") {
                    ok(json!(format!("0x{:064x}", 6)))
                } else {
                    // totalSupply: at FROM-1 -> start, anywhere else -> start + net mint
                    let blk = params
                        .get(1)
                        .and_then(|b| b.as_str())
                        .and_then(|s| u64::from_str_radix(s.trim_start_matches("0x"), 16).ok());
                    let (m, b) = expected_sums();
                    let v = if blk == Some(FROM - 1) {
                        U256::from(SUPPLY_START)
                    } else {
                        U256::from(SUPPLY_START) + m - b
                    };
                    ok(json!(format!("0x{v:064x}")))
                }
            }
            "eth_getLogs" => {
                let f = hex_u64(&params[0]["fromBlock"]);
                let t = hex_u64(&params[0]["toBlock"]);
                let fail_from = self.0.fail_from.load(Ordering::SeqCst);
                let fail = fail_from != 0 && f >= fail_from;
                self.0.log_requests.lock().unwrap().push((f, t, !fail));
                if fail {
                    return ResponseTemplate::new(200).set_body_json(json!({
                        "jsonrpc":"2.0","id":id,
                        "error":{"code":-32000,"message":"scripted outage"}
                    }));
                }
                let mask = self.0.bad_mask.load(Ordering::SeqCst);
                let in_mask = mask != 0 && f >= FROM && (mask >> ((f - FROM) / CHUNK)) & 1 == 1;
                if self.0.bad_only.load(Ordering::SeqCst) == f || in_mask {
                    return ok(json!([{
                        "address": CONTRACT,
                        "topics": ["0xddf252ad1be2c89b69c2b068fc378daa952ba7f163c4a11628f55a4df523b3ef"],
                        "data": "0x",
                        "blockNumber": format!("0x{f:x}"),
                        "blockHash": format!("0x{f:064x}"),
                        "transactionHash": format!("0x{:064x}", 0xbad0_0000u64 + f),
                        "transactionIndex": "0x0",
                        "logIndex": "0x0",
                        "removed": false,
                    }]));
                }
                let logs: Vec<Value> = fixture_logs()
                    .iter()
                    .filter(|l| l.block >= f && l.block <= t)
                    .map(log_json)
                    .collect();
                ok(json!(logs))
            }
            m => panic!("unscripted RPC method {m}"),
        }
    }
}

pub(crate) async fn spawn_mock() -> (MockServer, Arc<MockState>) {
    let state = Arc::new(MockState {
        fail_from: AtomicU64::new(0),
        bad_only: AtomicU64::new(0),
        bad_mask: AtomicU64::new(0),
        log_requests: Mutex::new(Vec::new()),
    });
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::method("POST"))
        .respond_with(Responder(state.clone()))
        .mount(&server)
        .await;
    (server, state)
}

pub(crate) fn tmp_root(label: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("resume_tests_{}_{label}", std::process::id()));
    let _ = std::fs::remove_dir_all(&p);
    std::fs::create_dir_all(&p).unwrap();
    p
}

pub(crate) fn run_dir(root: &Path) -> PathBuf {
    crate::report::ensure_run_out_dir_at(root, "USDC", RUN_ID).unwrap()
}

/// Pin `started_at` (otherwise `Utc::now()`), so uninterrupted and resumed runs stamp identical times.
pub(crate) fn seed_manifest(root: &Path, chunk: u64) {
    let specs = vec![ckpt::ChainSpecRecord {
        chain: "ethereum".into(),
        from_block: FROM,
        to_block_requested: TO.to_string(),
    }];
    let m = ckpt::CheckpointManifest::new("USDC", RUN_ID, STARTED_AT, chunk, true, specs);
    ckpt::save_manifest(&run_dir(root), &m).unwrap();
}

pub(crate) async fn run_scan(root: &Path, chunk: u64) -> anyhow::Result<()> {
    run_per_chain_windows_at(
        root,
        "USDC",
        vec![("ethereum".into(), FROM, TO)],
        Some(chunk),
        Some(RUN_ID.into()),
        false,
        None,
    )
    .await
}

pub(crate) fn set_rpc_env(uri: &str) -> Option<String> {
    let saved = std::env::var("ALCHEMY_ETHEREUM_URL").ok();
    std::env::set_var("ALCHEMY_ETHEREUM_URL", uri);
    saved
}

pub(crate) fn restore_rpc_env(saved: Option<String>) {
    match saved {
        Some(v) => std::env::set_var("ALCHEMY_ETHEREUM_URL", v),
        None => std::env::remove_var("ALCHEMY_ETHEREUM_URL"),
    }
}

pub(crate) fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| panic!("read {}: {e}", p.display()))
}

// ---------------------------------------------------------------------------------------------
// helpers for scenario tests
// ---------------------------------------------------------------------------------------------

fn files_in(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(base: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).unwrap().to_string_lossy().to_string();
                out.push((rel, std::fs::read(&p).unwrap()));
            }
        }
    }
    walk(dir, dir, &mut out);
    out.sort();
    out
}

fn assert_same_outputs(a: &Path, b: &Path) {
    let (fa, fb) = (files_in(a), files_in(b));
    let names = |f: &[(String, Vec<u8>)]| f.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
    assert_eq!(names(&fa), names(&fb), "output file sets differ");
    assert!(fa.iter().any(|f| f.0 == "artifact_manifest.json"));
    for (x, y) in fa.iter().zip(fb.iter()) {
        assert!(x.1 == y.1, "file {} differs between runs", x.0);
    }
}

struct Env {
    _lock: tokio::sync::MutexGuard<'static, ()>,
    server: MockServer,
    state: Arc<MockState>,
    saved: Option<String>,
}

impl Env {
    async fn new() -> Env {
        let lock = crate::rpc::RPC_ENV_LOCK.lock().await;
        let (server, state) = spawn_mock().await;
        let saved = set_rpc_env(&server.uri());
        Env {
            _lock: lock,
            server,
            state,
            saved,
        }
    }
    fn fail_from(&self, b: u64) {
        self.state.fail_from.store(b, Ordering::SeqCst);
    }
    fn successful_intervals(&self) -> Vec<(u64, u64)> {
        let mut v: Vec<(u64, u64)> = self
            .state
            .log_requests
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.2)
            .map(|r| (r.0, r.1))
            .collect();
        v.sort();
        v
    }
    fn bad_mask(&self, m: u64) {
        self.state.bad_mask.store(m, Ordering::SeqCst);
    }
    fn bad_only(&self, b: u64) {
        self.state.bad_only.store(b, Ordering::SeqCst);
    }
    fn clear_requests(&self) {
        self.state.log_requests.lock().unwrap().clear();
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        let _ = &self.server;
        restore_rpc_env(self.saved.take());
    }
}

fn expected_chunks() -> Vec<(u64, u64)> {
    (0..10)
        .map(|i| (FROM + i * CHUNK, FROM + (i + 1) * CHUNK - 1))
        .collect()
}

/// Fresh, uninterrupted run into a new root with the pinned manifest.
async fn uninterrupted(label: &str) -> PathBuf {
    let root = tmp_root(label);
    seed_manifest(&root, CHUNK);
    run_scan(&root, CHUNK).await.expect("uninterrupted run");
    root
}

/// Run that fails at `fail_from`, leaving a checkpoint behind.
async fn interrupt_at(env: &Env, root: &Path, fail_from: u64) {
    seed_manifest(root, CHUNK);
    env.fail_from(fail_from);
    assert!(
        run_scan(root, CHUNK).await.is_err(),
        "outage must fail the run"
    );
}

fn progress_json(root: &Path) -> Value {
    let p = ckpt::fetch_progress_path(&run_dir(root), "ethereum");
    serde_json::from_str(&read(&p)).unwrap()
}

fn partial_rows(root: &Path) -> usize {
    let p = ckpt::fetch_partial_path(&run_dir(root), "ethereum");
    if !p.exists() {
        return 0;
    }
    read(&p).lines().count().saturating_sub(1)
}

// ---------------------------------------------------------------------------------------------
// (0) regression for the reported bug
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn resumed_scan_keeps_gross_mint_burn_sums() {
    let env = Env::new().await;
    let a = uninterrupted("sums_a").await;
    let b = tmp_root("sums_b");
    interrupt_at(&env, &b, FROM + 500).await;
    env.fail_from(0);
    run_scan(&b, CHUNK).await.expect("resumed run");
    let audit = read(&run_dir(&b).join("supply_audit.csv"));
    let (m, bu) = expected_sums();
    assert!(
        audit.contains(&m.to_string()),
        "sum_mints_raw lost on resume:\n{audit}"
    );
    assert!(audit.contains(&bu.to_string()));
    assert!(
        audit.contains(",0,true,true,true,true,true,0,0"),
        "discrepancy must be 0:\n{audit}"
    );
    let _ = (a, b);
}

// ---------------------------------------------------------------------------------------------
// (1) uninterrupted == interrupted + resumed, for several interruption points
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn interrupted_and_resumed_matches_uninterrupted_outputs() {
    let env = Env::new().await;
    let a = uninterrupted("eq_a").await;
    for (i, fail_from) in [FROM, FROM + 100, FROM + 500, FROM + 900]
        .iter()
        .enumerate()
    {
        let b = tmp_root(&format!("eq_b{i}"));
        interrupt_at(&env, &b, *fail_from).await;
        env.fail_from(0);
        run_scan(&b, CHUNK).await.expect("resumed run");
        assert_same_outputs(&run_dir(&a), &run_dir(&b));
        let _ = std::fs::remove_dir_all(&b);
    }
    let _ = std::fs::remove_dir_all(&a);
}

// ---------------------------------------------------------------------------------------------
// (2) no skipped or duplicated block intervals
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn resume_covers_every_block_interval_exactly_once() {
    let env = Env::new().await;
    let b = tmp_root("cover");
    interrupt_at(&env, &b, FROM + 500).await;
    // 5 attempts of the same failing chunk, nothing past it
    let failed: Vec<_> = env
        .state
        .log_requests
        .lock()
        .unwrap()
        .iter()
        .filter(|r| !r.2)
        .cloned()
        .collect();
    assert_eq!(failed.len(), 5);
    assert!(failed
        .iter()
        .all(|r| (r.0, r.1) == (FROM + 500, FROM + 599)));
    env.fail_from(0);
    run_scan(&b, CHUNK).await.expect("resumed run");
    assert_eq!(
        env.successful_intervals(),
        expected_chunks(),
        "successful eth_getLogs intervals must tile the window exactly once"
    );
    let _ = std::fs::remove_dir_all(&b);
}

#[tokio::test]
async fn resume_with_unaligned_final_chunk_tiles_exactly() {
    // chunk 300 over 1000 blocks -> last chunk is short (900..999)
    let env = Env::new().await;
    let b = tmp_root("cover_unaligned");
    seed_manifest(&b, 300);
    env.fail_from(FROM + 600);
    assert!(run_scan(&b, 300).await.is_err());
    env.fail_from(0);
    run_scan(&b, 300).await.expect("resumed");
    assert_eq!(
        env.successful_intervals(),
        vec![
            (FROM, FROM + 299),
            (FROM + 300, FROM + 599),
            (FROM + 600, FROM + 899),
            (FROM + 900, TO)
        ]
    );
    let _ = std::fs::remove_dir_all(&b);
}

// ---------------------------------------------------------------------------------------------
// (3) unresolved intervals are retried or explicitly kept
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn unresolved_interval_is_recorded_kept_and_retried() {
    let env = Env::new().await;
    let b = tmp_root("unresolved");
    interrupt_at(&env, &b, FROM + 500).await;

    let p = progress_json(&b);
    assert_eq!(p["last_fetched_through"], FROM + 499);
    assert_eq!(p["unresolved"]["from_block"], FROM + 500);
    assert_eq!(p["unresolved"]["to_block"], FROM + 599);
    assert!(p["unresolved"]["error"]
        .as_str()
        .unwrap()
        .contains("scripted outage"));

    // still down: it stays unresolved (and is retried, not skipped)
    env.clear_requests();
    assert!(run_scan(&b, CHUNK).await.is_err());
    let p = progress_json(&b);
    assert_eq!(p["unresolved"]["from_block"], FROM + 500);
    assert_eq!(p["last_fetched_through"], FROM + 499);
    assert!(env
        .state
        .log_requests
        .lock()
        .unwrap()
        .iter()
        .all(|r| r.0 == FROM + 500 && !r.2));

    // recovered: retried first, checkpoint removed, run completes
    env.fail_from(0);
    run_scan(&b, CHUNK).await.expect("resumed run");
    assert!(!ckpt::checkpoint_root(&run_dir(&b)).exists());
    assert!(run_dir(&b).join("artifact_manifest.json").is_file());
    let _ = std::fs::remove_dir_all(&b);
}

#[tokio::test]
async fn first_chunk_failure_is_recorded_as_unresolved() {
    let env = Env::new().await;
    let b = tmp_root("unresolved_first");
    interrupt_at(&env, &b, FROM).await;
    let p = progress_json(&b);
    assert_eq!(p["last_fetched_through"], FROM - 1);
    assert_eq!(p["chunks_done"], 0);
    assert_eq!(p["unresolved"]["from_block"], FROM);
    env.fail_from(0);
    run_scan(&b, CHUNK).await.expect("resumed run");
    assert_eq!(env.successful_intervals(), expected_chunks());
    let _ = std::fs::remove_dir_all(&b);
}

// ---------------------------------------------------------------------------------------------
// (4) repeated resume does not duplicate events
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn repeated_resume_does_not_duplicate_events() {
    let env = Env::new().await;
    let a = uninterrupted("rep_a").await;
    let b = tmp_root("rep_b");
    interrupt_at(&env, &b, FROM + 500).await;
    let rows_after_first = partial_rows(&b);
    // resume #2 advances further then dies again; resume #3 dies at the same place
    env.fail_from(FROM + 800);
    assert!(run_scan(&b, CHUNK).await.is_err());
    let rows_after_second = partial_rows(&b);
    assert!(rows_after_second > rows_after_first);
    assert!(run_scan(&b, CHUNK).await.is_err());
    assert_eq!(
        partial_rows(&b),
        rows_after_second,
        "no-progress resume must not append"
    );
    env.fail_from(0);
    run_scan(&b, CHUNK).await.expect("final resume");
    assert_same_outputs(&run_dir(&a), &run_dir(&b));
    let audit = read(&run_dir(&b).join("supply_audit.csv"));
    assert!(
        audit.contains(",0,0\n"),
        "duplicate_count / decode errors must be 0:\n{audit}"
    );
    let _ = (a, b);
}

#[tokio::test]
async fn crash_between_partial_append_and_progress_save_does_not_duplicate() {
    let env = Env::new().await;
    let a = uninterrupted("crash_a").await;
    let b = tmp_root("crash_b");
    interrupt_at(&env, &b, FROM + 500).await;
    // Simulate: chunk 500..599 rows were appended to the partial CSV but progress was never saved.
    let dir = run_dir(&b);
    let before = partial_rows(&b);
    let orphan = crate::decode::TransferEvent {
        chain: "ethereum".into(),
        contract_address: "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48".into(),
        block_number: FROM + 500,
        tx_hash: format!("0x{:064x}", 0xa000_0000u64 + (FROM + 500) * 10),
        log_index: 0,
        from: B.into(),
        to: ZERO.into(),
        value_raw: "4000000".into(),
        value_decimal: "4.000000".into(),
        kind: "burn".into(),
        value_u256: U256::from(4_000_000u64),
    };
    ckpt::append_fetch_partial_events(&dir, "ethereum", &[orphan]).unwrap();
    assert_eq!(partial_rows(&b), before + 1);
    env.fail_from(0);
    run_scan(&b, CHUNK)
        .await
        .expect("resume after simulated crash");
    assert_same_outputs(&run_dir(&a), &run_dir(&b));
    let _ = (a, b);
}

// ---------------------------------------------------------------------------------------------
// (5) corrupt / incompatible checkpoints fail closed with a clear error
// ---------------------------------------------------------------------------------------------

const PROGRESS_ARGS: (&str, &str) = ("ethereum", "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48");

fn load_progress(root: &Path) -> anyhow::Result<Option<ckpt::FetchChunkProgress>> {
    ckpt::load_fetch_progress(
        &run_dir(root),
        PROGRESS_ARGS.0,
        PROGRESS_ARGS.1,
        FROM,
        TO,
        CHUNK,
    )
}

fn load_partial(root: &Path) -> anyhow::Result<Vec<crate::decode::TransferEvent>> {
    let p = load_progress(root)?.expect("progress");
    ckpt::load_fetch_partial_events(&run_dir(root), &p)
}

fn err_text<T>(r: anyhow::Result<T>) -> String {
    match r {
        Ok(_) => panic!("expected a fail-closed error, got Ok"),
        Err(e) => format!("{e:#}"),
    }
}

async fn checkpointed(label: &str) -> (Env, PathBuf) {
    let env = Env::new().await;
    let root = tmp_root(label);
    interrupt_at(&env, &root, FROM + 500).await;
    (env, root)
}

fn edit_progress(root: &Path, f: impl FnOnce(&mut Value)) {
    let p = ckpt::fetch_progress_path(&run_dir(root), "ethereum");
    let mut v: Value = serde_json::from_str(&read(&p)).unwrap();
    f(&mut v);
    std::fs::write(&p, serde_json::to_string_pretty(&v).unwrap()).unwrap();
}

#[tokio::test]
async fn bad_schema_version_fails_closed() {
    let (_env, root) = checkpointed("bad_schema").await;
    edit_progress(&root, |v| {
        v["schema"] = json!("transfer-audit-fetch-chunk-v1")
    });
    let e = err_text(load_progress(&root));
    assert!(
        e.contains("unsupported fetch progress schema") && e.contains("--fresh"),
        "{e}"
    );

    let mp = ckpt::manifest_path(&run_dir(&root));
    let mut m: Value = serde_json::from_str(&read(&mp)).unwrap();
    m["schema"] = json!("transfer-audit-checkpoint-v0");
    std::fs::write(&mp, m.to_string()).unwrap();
    let e = err_text(ckpt::load_manifest(&run_dir(&root)));
    assert!(
        e.contains("unsupported checkpoint schema") && e.contains("--fresh"),
        "{e}"
    );
}

#[tokio::test]
async fn missing_fields_fail_closed() {
    let (_env, root) = checkpointed("missing_fields").await;
    edit_progress(&root, |v| {
        v.as_object_mut().unwrap().remove("events_persisted");
    });
    let e = err_text(load_progress(&root));
    assert!(e.contains("missing fields") && e.contains("--fresh"), "{e}");

    let mp = ckpt::manifest_path(&run_dir(&root));
    let mut m: Value = serde_json::from_str(&read(&mp)).unwrap();
    m.as_object_mut().unwrap().remove("chain_specs");
    std::fs::write(&mp, m.to_string()).unwrap();
    let e = err_text(ckpt::load_manifest(&run_dir(&root)));
    assert!(e.contains("missing fields") && e.contains("--fresh"), "{e}");
}

#[tokio::test]
async fn truncated_files_fail_closed() {
    let (_env, root) = checkpointed("truncated").await;
    let dir = run_dir(&root);

    // truncated partial CSV: cut inside the last row
    let partial = ckpt::fetch_partial_path(&dir, "ethereum");
    let raw = std::fs::read(&partial).unwrap();
    std::fs::write(&partial, &raw[..raw.len() - 40]).unwrap();
    let e = err_text(load_partial(&root));
    assert!(e.contains("--fresh"), "{e}");
    std::fs::write(&partial, &raw).unwrap();
    assert!(load_partial(&root).is_ok(), "restored file loads again");

    // whole trailing row missing -> count mismatch against progress
    let text = String::from_utf8(raw.clone()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    std::fs::write(&partial, lines[..lines.len() - 1].join("\n") + "\n").unwrap();
    let e = err_text(load_partial(&root));
    assert!(
        e.contains("truncated or modified") && e.contains("--fresh"),
        "{e}"
    );
    std::fs::write(&partial, &raw).unwrap();

    // deleted partial CSV while progress says rows exist
    std::fs::remove_file(&partial).unwrap();
    let e = err_text(load_partial(&root));
    assert!(e.contains("is missing") && e.contains("--fresh"), "{e}");

    // truncated progress JSON
    let pp = ckpt::fetch_progress_path(&dir, "ethereum");
    let raw = std::fs::read(&pp).unwrap();
    std::fs::write(&pp, &raw[..raw.len() / 2]).unwrap();
    let e = err_text(load_progress(&root));
    assert!(e.contains("truncated") && e.contains("--fresh"), "{e}");

    // truncated manifest JSON
    let mp = ckpt::manifest_path(&dir);
    let raw = std::fs::read(&mp).unwrap();
    std::fs::write(&mp, &raw[..raw.len() / 2]).unwrap();
    let e = err_text(ckpt::load_manifest(&dir));
    assert!(e.contains("truncated") && e.contains("--fresh"), "{e}");
}

#[tokio::test]
async fn corrupt_rows_and_inconsistent_progress_fail_closed() {
    let (_env, root) = checkpointed("corrupt_rows").await;
    let dir = run_dir(&root);
    let partial = ckpt::fetch_partial_path(&dir, "ethereum");
    let good = read(&partial);

    // garbage value_raw
    let header = good.lines().next().unwrap().to_string();
    let first = good.lines().nth(1).unwrap().to_string();
    let cols: Vec<&str> = header.split(',').collect();
    let vi = cols.iter().position(|c| *c == "value_raw").unwrap();
    let mut f: Vec<String> = first.split(',').map(String::from).collect();
    f[vi] = "12x".into();
    let bad = good.replacen(&first, &f.join(","), 1);
    std::fs::write(&partial, bad).unwrap();
    let e = err_text(load_partial(&root));
    assert!(
        e.contains("not a decimal integer") && e.contains("--fresh"),
        "{e}"
    );
    std::fs::write(&partial, &good).unwrap();

    // row outside the window
    let ki = cols.iter().position(|c| *c == "block_number").unwrap();
    let mut f: Vec<String> = first.split(',').map(String::from).collect();
    f[ki] = (TO + 5).to_string();
    std::fs::write(&partial, good.replacen(&first, &f.join(","), 1)).unwrap();
    let e = err_text(load_partial(&root));
    assert!(e.contains("outside window"), "{e}");
    std::fs::write(&partial, &good).unwrap();

    // progress claims a position that is not on a chunk boundary / wrong chunk counters
    edit_progress(&root, |v| v["last_fetched_through"] = json!(FROM + 450));
    let e = err_text(load_progress(&root));
    assert!(e.contains("not on a chunk boundary"), "{e}");
    edit_progress(&root, |v| {
        v["last_fetched_through"] = json!(FROM + 499);
        v["chunks_done"] = json!(7);
    });
    let e = err_text(load_progress(&root));
    assert!(e.contains("chunks_done"), "{e}");
    edit_progress(&root, |v| {
        v["chunks_done"] = json!(5);
        v["unresolved"]["from_block"] = json!(FROM + 700);
    });
    let e = err_text(load_progress(&root));
    assert!(e.contains("unresolved interval"), "{e}");
}

#[tokio::test]
async fn mismatched_config_or_range_fails_closed() {
    let (_env, root) = checkpointed("mismatch").await;
    let dir = run_dir(&root);
    let a = PROGRESS_ARGS;
    for (what, r) in [
        (
            "chunk",
            ckpt::load_fetch_progress(&dir, a.0, a.1, FROM, TO, 50),
        ),
        (
            "to_block",
            ckpt::load_fetch_progress(&dir, a.0, a.1, FROM, TO + 1, CHUNK),
        ),
        (
            "from_block",
            ckpt::load_fetch_progress(&dir, a.0, a.1, FROM + 1, TO, CHUNK),
        ),
        (
            "contract",
            ckpt::load_fetch_progress(&dir, a.0, "0xdead", FROM, TO, CHUNK),
        ),
        (
            "chain",
            ckpt::load_fetch_progress(&dir, "base", a.1, FROM, TO, CHUNK),
        ),
    ] {
        if what == "chain" {
            // a different chain has its own (absent) progress file, never this one
            assert!(r.unwrap().is_none());
            continue;
        }
        let e = err_text(r);
        assert!(
            e.contains("does not match current window") && e.contains("--fresh"),
            "{what}: {e}"
        );
    }

    // end-to-end: re-running with a different window is refused and leaves the checkpoint untouched
    let before = files_in(&ckpt::checkpoint_root(&dir));
    let e = err_text(
        run_per_chain_windows_at(
            &root,
            "USDC",
            vec![("ethereum".into(), FROM, TO - 1)],
            Some(CHUNK),
            Some(RUN_ID.into()),
            false,
            None,
        )
        .await,
    );
    assert!(
        e.contains("window specs differ") && e.contains("--fresh"),
        "{e}"
    );
    assert!(files_in(&ckpt::checkpoint_root(&dir)) == before);
    let e = err_text(run_scan(&root, 50).await);
    assert!(e.contains("chunk_size"), "{e}");
}

#[tokio::test]
async fn corrupt_partial_checkpoint_blocks_end_to_end_resume() {
    let (env, root) = checkpointed("e2e_corrupt").await;
    let dir = run_dir(&root);
    let partial = ckpt::fetch_partial_path(&dir, "ethereum");
    let raw = std::fs::read(&partial).unwrap();
    std::fs::write(&partial, &raw[..raw.len() - 40]).unwrap();
    env.fail_from(0);
    env.clear_requests();
    assert!(run_scan(&root, CHUNK).await.is_err());
    assert!(
        env.state.log_requests.lock().unwrap().is_empty(),
        "no RPC log fetch after a bad checkpoint"
    );
    assert!(!dir.join("artifact_manifest.json").exists());
    let qa = read(&dir.join("qa_report.json"));
    assert!(
        qa.contains("load partial transfers") && qa.contains("--fresh"),
        "{qa}"
    );
    // checkpoint is preserved for inspection
    assert!(ckpt::fetch_progress_path(&dir, "ethereum").is_file());
}

#[tokio::test]
async fn tampered_completed_chain_checkpoint_fails_closed() {
    let env = Env::new().await;
    let root = tmp_root("completed_tamper2");
    let specs = vec![
        ckpt::ChainSpecRecord {
            chain: "ethereum".into(),
            from_block: FROM,
            to_block_requested: TO.to_string(),
        },
        ckpt::ChainSpecRecord {
            chain: "nochain_zz".into(),
            from_block: 1,
            to_block_requested: "2".into(),
        },
    ];
    let m = ckpt::CheckpointManifest::new("USDC", RUN_ID, STARTED_AT, CHUNK, true, specs);
    ckpt::save_manifest(&run_dir(&root), &m).unwrap();
    let windows = vec![
        ("ethereum".to_string(), FROM, TO),
        ("nochain_zz".to_string(), 1, 2),
    ];
    let first = run_per_chain_windows_at(
        &root,
        "USDC",
        windows.clone(),
        Some(CHUNK),
        Some(RUN_ID.into()),
        false,
        None,
    )
    .await;
    assert!(first.is_err(), "second chain has no config -> hard error");
    let dir = run_dir(&root);
    let csv_path = dir.join("checkpoint/transfers_ethereum.csv");
    assert!(csv_path.is_file(), "ethereum chain checkpoint saved");

    // resume reuses it and still reports correct gross sums (value_u256 restored from value_raw)
    let (events, bundle) = ckpt::load_completed_chain(&dir, "ethereum").unwrap();
    let (m_sum, b_sum) = expected_sums();
    assert_eq!(
        events
            .iter()
            .filter(|e| e.kind == "mint")
            .fold(U256::ZERO, |a, e| a + e.value_u256),
        m_sum
    );
    assert_eq!(bundle.supply.sum_burns_raw, b_sum.to_string());

    // tamper: drop a row -> row count / sums no longer match the bundle
    let text = read(&csv_path);
    let lines: Vec<&str> = text.lines().collect();
    std::fs::write(&csv_path, lines[..lines.len() - 1].join("\n") + "\n").unwrap();
    let e = err_text(ckpt::load_completed_chain(&dir, "ethereum"));
    assert!(
        e.contains("chain bundle records") && e.contains("--fresh"),
        "{e}"
    );
    let e = err_text(
        run_per_chain_windows_at(
            &root,
            "USDC",
            windows,
            Some(CHUNK),
            Some(RUN_ID.into()),
            false,
            None,
        )
        .await,
    );
    assert!(e.contains("chain bundle records"), "{e}");
    let _ = env;
}

// ---------------------------------------------------------------------------------------------
// (6) manifests and SHA256 values are reproducible across runs
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn manifests_and_sha256_are_reproducible() {
    let env = Env::new().await;
    let a = uninterrupted("sha_a").await;
    let b = uninterrupted("sha_b").await;
    let c = tmp_root("sha_c");
    interrupt_at(&env, &c, FROM + 500).await;
    env.fail_from(0);
    run_scan(&c, CHUNK).await.expect("resumed");

    let manifest = |r: &Path| read(&run_dir(r).join("artifact_manifest.json"));
    assert_eq!(manifest(&a), manifest(&b));
    assert_eq!(manifest(&a), manifest(&c));

    let v: Value = serde_json::from_str(&manifest(&c)).unwrap();
    let arts = v["artifacts"].as_array().expect("artifacts array");
    assert!(arts.len() >= 10);
    for art in arts {
        let rel = art["path"].as_str().unwrap();
        let want = crate::artifact::checksum::sha256_file_hex(&run_dir(&c).join(rel)).unwrap();
        assert_eq!(art["checksum_sha256"].as_str().unwrap(), want, "{rel}");
        let other = crate::artifact::checksum::sha256_file_hex(&run_dir(&a).join(rel)).unwrap();
        assert_eq!(
            want, other,
            "{rel} sha256 differs between uninterrupted and resumed run"
        );
    }
    assert_same_outputs(&run_dir(&a), &run_dir(&b));
    assert_same_outputs(&run_dir(&a), &run_dir(&c));
}

// ---------------------------------------------------------------------------------------------
// storage-fault tests (review follow-up): progress write / CSV append / rename failures
// ---------------------------------------------------------------------------------------------

use ckpt::fault::{self, Op};

const BAD_CHUNK: u64 = FROM + 500;

fn counters_ok(root: &Path) {
    let p = progress_json(root);
    let n = |k: &str| p[k].as_u64().unwrap();
    assert_eq!(
        n("events_persisted") + n("decode_errors"),
        n("logs_fetched"),
        "progress counters must describe one committed state: {p}"
    );
}

fn audit_row(root: &Path) -> String {
    read(&run_dir(root).join("supply_audit.csv"))
        .lines()
        .nth(1)
        .unwrap()
        .to_string()
}

/// (1) returned progress-write error -> fallback save -> resume (incl. zero-event decode-only chunk)
#[tokio::test]
async fn progress_write_failure_then_resume_matches_uninterrupted() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let a = uninterrupted("pw_a").await;
    assert!(
        audit_row(&a).ends_with(",1"),
        "fixture: one decode error: {}",
        audit_row(&a)
    );
    for nth in [0i64, 3, 5, 9] {
        let b = tmp_root(&format!("pw_b{nth}"));
        seed_manifest(&b, CHUNK);
        fault::arm(&run_dir(&b), Op::SaveProgress, nth);
        let first = run_scan(&b, CHUNK).await;
        fault::disarm(&run_dir(&b));
        assert!(
            first.is_err(),
            "nth={nth}: injected save failure must fail the run"
        );
        if ckpt::fetch_progress_path(&run_dir(&b), "ethereum").exists() {
            counters_ok(&b);
        }
        run_scan(&b, CHUNK)
            .await
            .unwrap_or_else(|e| panic!("nth={nth}: resume must succeed: {e:#}"));
        assert_same_outputs(&run_dir(&a), &run_dir(&b));
        assert_eq!(audit_row(&a), audit_row(&b), "nth={nth}");
        let _ = std::fs::remove_dir_all(&b);
    }
    let _ = std::fs::remove_dir_all(&a);
}

/// (2) CSV append failure and torn tail, then resume
#[tokio::test]
async fn partial_csv_append_failure_or_torn_tail_then_resume() {
    let env = Env::new().await;
    let a = uninterrupted("ap_a").await;
    for (i, op) in [Op::Append, Op::TornAppend].into_iter().enumerate() {
        let b = tmp_root(&format!("ap_b{i}"));
        seed_manifest(&b, CHUNK);
        fault::arm(&run_dir(&b), op, 4);
        let first = run_scan(&b, CHUNK).await;
        fault::disarm(&run_dir(&b));
        assert!(first.is_err(), "{op:?}");
        run_scan(&b, CHUNK)
            .await
            .unwrap_or_else(|e| panic!("{op:?}: resume must recover: {e:#}"));
        assert_same_outputs(&run_dir(&a), &run_dir(&b));
        let _ = std::fs::remove_dir_all(&b);
    }
    drop(env);
}

/// (3) failure between tmp write and rename
#[tokio::test]
async fn failure_between_tmp_write_and_rename_then_resume() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let a = uninterrupted("rn_a").await;
    for nth in [1i64, 3, 5] {
        let b = tmp_root(&format!("rn_b{nth}"));
        seed_manifest(&b, CHUNK);
        fault::arm(&run_dir(&b), Op::Rename, nth);
        let first = run_scan(&b, CHUNK).await;
        fault::disarm(&run_dir(&b));
        assert!(first.is_err(), "nth={nth}");
        run_scan(&b, CHUNK)
            .await
            .unwrap_or_else(|e| panic!("nth={nth}: resume: {e:#}"));
        assert_same_outputs(&run_dir(&a), &run_dir(&b));
        let _ = std::fs::remove_dir_all(&b);
    }
    drop(env);
}

/// (4) repeated resume after repeated storage errors
#[tokio::test]
async fn repeated_resume_after_repeated_storage_errors() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let a = uninterrupted("rs_a").await;
    let b = tmp_root("rs_b");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    let plan = [
        (Op::SaveProgress, 2i64),
        (Op::SaveProgress, 2),
        (Op::TornAppend, 1),
        (Op::SaveProgress, 0),
        (Op::Rename, 1),
        (Op::SaveProgress, 1),
    ];
    for (op, nth) in plan {
        fault::arm(&dir, op, nth);
        let r = run_scan(&b, CHUNK).await;
        fault::disarm(&dir);
        assert!(r.is_err(), "{op:?}/{nth} should fail the run");
        if ckpt::fetch_progress_path(&dir, "ethereum").exists() {
            counters_ok(&b);
        }
    }
    run_scan(&b, CHUNK).await.expect("final clean resume");
    assert_same_outputs(&run_dir(&a), &run_dir(&b));
    assert_eq!(audit_row(&a), audit_row(&b));
    let _ = (a, b);
}

/// (5) a storage error is reported as storage, never as an unresolved RPC interval
#[tokio::test]
async fn storage_error_is_not_classified_as_rpc_failure() {
    let env = Env::new().await;
    let b = tmp_root("cls");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    fault::arm(&dir, Op::SaveProgress, 3);
    let first = run_scan(&b, CHUNK).await;
    fault::disarm(&dir);
    assert!(first.is_err());
    let qa = read(&dir.join("qa_report.json"));
    assert!(
        qa.contains("storage error"),
        "must be labelled a storage error:\n{qa}"
    );
    assert!(
        !qa.contains("eth_getLogs failed"),
        "storage failure mislabelled as RPC:\n{qa}"
    );
    let p = progress_json(&b);
    assert!(
        p["unresolved"].is_null(),
        "storage failure must not create an unresolved RPC interval: {p}"
    );
    counters_ok(&b);
    // no RPC retries were burned on it
    assert!(env.state.log_requests.lock().unwrap().iter().all(|r| r.2));
    let _ = b;
}

/// (6) value_raw beyond u128, and 2^256 overflow
#[tokio::test]
async fn value_raw_u256_boundaries() {
    let root = tmp_root("u256");
    let dir = run_dir(&root);
    let mk = |idx: u64, raw: &str| crate::decode::TransferEvent {
        chain: "ethereum".into(),
        contract_address: PROGRESS_ARGS.1.into(),
        block_number: FROM + 1,
        tx_hash: format!("0x{idx:064x}"),
        log_index: idx,
        from: ZERO.into(),
        to: A.into(),
        value_raw: raw.into(),
        value_decimal: "0".into(),
        kind: "mint".into(),
        value_u256: U256::ZERO,
    };
    let progress = |n: u64| ckpt::FetchChunkProgress {
        schema: ckpt::FetchChunkProgress::SCHEMA.into(),
        chain: "ethereum".into(),
        contract_address: PROGRESS_ARGS.1.into(),
        from_block: FROM,
        to_block: TO,
        chunk_size: CHUNK,
        last_fetched_through: FROM + 99,
        chunks_done: 1,
        total_chunks: 10,
        logs_fetched: n as usize,
        events_persisted: n,
        decode_errors: 0,
        unresolved: None,
        chunk_notes: Vec::new(),
    };
    let two128 = "340282366920938463463374607431768211456";
    let max = U256::MAX.to_string();
    ckpt::append_fetch_partial_events(&dir, "ethereum", &[mk(0, two128), mk(1, &max)]).unwrap();
    let rows = ckpt::load_fetch_partial_events(&dir, &progress(2)).unwrap();
    assert_eq!(
        rows[0].value_u256,
        U256::from(1u128 << 127) * U256::from(2u8)
    );
    assert_eq!(rows[1].value_u256, U256::MAX);
    let _ = std::fs::remove_dir_all(&dir);

    let overflow = "115792089237316195423570985008687907853269984665640564039457584007913129639936";
    ckpt::append_fetch_partial_events(&dir, "ethereum", &[mk(0, overflow)]).unwrap();
    let e = err_text(ckpt::load_fetch_partial_events(&dir, &progress(1)));
    assert!(
        e.contains("does not fit in uint256") && e.contains("--fresh"),
        "{e}"
    );
    let _ = std::fs::remove_dir_all(&root);
}

/// (7) decode_errors > logs_fetched (and other counter contradictions) are rejected
#[tokio::test]
async fn counter_contradictions_are_rejected() {
    let (_env, root) = checkpointed("counters").await;
    edit_progress(&root, |v| v["decode_errors"] = json!(99));
    let e = err_text(load_progress(&root));
    assert!(e.contains("decode_errors"), "{e}");
    edit_progress(&root, |v| {
        v["decode_errors"] = json!(0);
        v["events_persisted"] = json!(v["logs_fetched"].as_u64().unwrap() + 1);
    });
    let e = err_text(load_progress(&root));
    assert!(e.contains("events_persisted"), "{e}");
}

/// A failed fallback (unresolved-interval) save is loud in qa_report, and the checkpoint stays resumable.
#[tokio::test]
async fn failed_fallback_save_is_reported_loudly_and_stays_resumable() {
    let env = Env::new().await;
    let a = uninterrupted("fb_a").await;
    let b = tmp_root("fb_b");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    env.fail_from(FROM + 300);
    // saves 0..=2 succeed (chunks 0..2); the fallback unresolved-interval save is the 4th
    fault::arm(&dir, Op::SaveProgress, 3);
    assert!(run_scan(&b, CHUNK).await.is_err());
    fault::disarm(&dir);
    let qa = read(&dir.join("qa_report.json"));
    assert!(qa.contains("CHECKPOINT WARNING"), "{qa}");
    counters_ok(&b);
    env.fail_from(0);
    run_scan(&b, CHUNK).await.expect("resume");
    assert_same_outputs(&run_dir(&a), &run_dir(&b));
}

// ---------------------------------------------------------------------------------------------
// round-2 review: rename-then-error recovery, completed-chain stage order, QA sample on resume
// ---------------------------------------------------------------------------------------------

/// DirSync fails AFTER the progress file was renamed into place: the on-disk progress is the NEW
/// one while the caller sees an error. Recovery must work from either old or new progress.
#[tokio::test]
async fn progress_error_after_rename_recovers_from_new_progress() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let a = uninterrupted("ar_a").await;
    // DirSync nth counts write_atomic calls: nth k (k<10) is the progress save of chunk k.
    for nth in 0i64..10 {
        let b = tmp_root(&format!("ar_b{nth}"));
        seed_manifest(&b, CHUNK);
        let dir = run_dir(&b);
        fault::arm(&dir, Op::DirSync, nth);
        assert!(run_scan(&b, CHUNK).await.is_err(), "nth={nth}");
        fault::disarm(&dir);
        // new progress is on disk, so the CSV must still hold that chunk's rows
        let p = progress_json(&b);
        assert_eq!(
            p["last_fetched_through"],
            FROM + (nth as u64 + 1) * CHUNK - 1,
            "nth={nth}"
        );
        counters_ok(&b);
        load_partial(&b)
            .unwrap_or_else(|e| panic!("nth={nth}: new progress + CSV must be consistent: {e:#}"));
        run_scan(&b, CHUNK)
            .await
            .unwrap_or_else(|e| panic!("nth={nth}: resume from NEW progress: {e:#}"));
        assert_same_outputs(&run_dir(&a), &run_dir(&b));
        let _ = std::fs::remove_dir_all(&b);
    }
}

/// Failure before the rename leaves the OLD progress; the appended rows are orphans and are dropped.
#[tokio::test]
async fn progress_error_before_rename_recovers_from_old_progress() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let a = uninterrupted("br_a").await;
    for op in [Op::TmpCreate, Op::TmpWrite, Op::TmpSync, Op::Rename] {
        for nth in 0i64..10 {
            let b = tmp_root(&format!("br_b{op:?}{nth}"));
            seed_manifest(&b, CHUNK);
            let dir = run_dir(&b);
            fault::arm(&dir, op, nth);
            assert!(run_scan(&b, CHUNK).await.is_err(), "{op:?} nth={nth}");
            fault::disarm(&dir);
            if nth > 0 {
                let p = progress_json(&b);
                assert_eq!(
                    p["last_fetched_through"],
                    FROM + nth as u64 * CHUNK - 1,
                    "{op:?} nth={nth}: old progress"
                );
            }
            run_scan(&b, CHUNK)
                .await
                .unwrap_or_else(|e| panic!("{op:?} nth={nth}: resume from OLD progress: {e:#}"));
            assert_same_outputs(&run_dir(&a), &run_dir(&b));
            let _ = std::fs::remove_dir_all(&b);
        }
    }
}

/// Message must not claim a specific commit outcome for a storage error.
#[tokio::test]
async fn storage_error_message_does_not_claim_old_state() {
    let env = Env::new().await;
    let b = tmp_root("msg");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    fault::arm(&dir, Op::DirSync, 2);
    assert!(run_scan(&b, CHUNK).await.is_err());
    fault::disarm(&dir);
    let qa = read(&dir.join("qa_report.json"));
    assert!(qa.contains("storage error"), "{qa}");
    assert!(!qa.contains("left at last committed state"), "{qa}");
    assert!(qa.contains("previous or the new"), "{qa}");
    drop(env);
}

/// Completed-chain save: fail before every stage; resume must reproduce the uninterrupted outputs
/// for failures up to (and excluding) the manifest commit.
#[tokio::test]
async fn completed_chain_failure_before_each_stage_then_resume() {
    let env = Env::new().await;
    let a = uninterrupted("st_a").await;
    // stage numbering is logical: 1 csv, 2 bundle, 3 manifest, 4 cleanup (after the manifest commit)
    for stage in [1u8, 2, 3] {
        let b = tmp_root(&format!("st_b{stage}"));
        seed_manifest(&b, CHUNK);
        let dir = run_dir(&b);
        fault::arm(&ckpt::checkpoint_root(&dir), Op::Stage(stage), 0);
        assert!(run_scan(&b, CHUNK).await.is_err(), "stage {stage}");
        fault::disarm(&ckpt::checkpoint_root(&dir));
        // the fetch progress must still be there until the chain is committed in the manifest
        assert!(
            ckpt::fetch_progress_path(&dir, "ethereum").exists(),
            "stage {stage}: fetch progress must survive until the completed-chain manifest commit"
        );
        env.clear_requests();
        run_scan(&b, CHUNK)
            .await
            .unwrap_or_else(|e| panic!("stage {stage}: resume: {e:#}"));
        assert_same_outputs(&run_dir(&a), &run_dir(&b));
        let _ = std::fs::remove_dir_all(&b);
    }
}

/// Cleanup failure AFTER the manifest commit: the chain is complete, resume must not refetch and
/// must finish. (Byte-equality of ALL outputs on the completed-chain load path is a separate,
/// pre-existing problem tracked in the follow-up issue; see `completed_chain_load_path_known_divergence`.)
#[tokio::test]
async fn completed_chain_cleanup_failure_after_manifest_commit() {
    let env = Env::new().await;
    let b = tmp_root("st_cleanup");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    let root = ckpt::checkpoint_root(&dir);
    fault::arm(&root, Op::Stage(4), 0);
    assert!(run_scan(&b, CHUNK).await.is_err());
    fault::disarm(&root);
    let m = ckpt::load_manifest(&dir).unwrap().unwrap();
    assert_eq!(
        m.completed_chains,
        vec!["ethereum".to_string()],
        "manifest commit happened first"
    );
    assert!(root.join("chain_ethereum.json").is_file());
    env.clear_requests();
    run_scan(&b, CHUNK)
        .await
        .expect("resume treats the chain as complete");
    assert!(
        env.state.log_requests.lock().unwrap().is_empty(),
        "no refetch of a committed chain"
    );
    assert!(dir.join("artifact_manifest.json").is_file());
    assert!(
        !root.exists(),
        "checkpoint dir cleared after a successful run"
    );
}

/// Mid-write failures inside each completed-chain write (tmp / rename / dir fsync) then resume.
#[tokio::test]
async fn completed_chain_atomic_write_failures_then_resume() {
    let env = Env::new().await;
    let a = uninterrupted("cw_a").await;
    // writes 0..9 are progress saves, 10 csv, 11 bundle, 12 manifest (final manifest update)
    for op in [Op::TmpCreate, Op::TmpWrite, Op::TmpSync, Op::Rename] {
        for nth in [10i64, 11, 12] {
            let b = tmp_root(&format!("cw_b{op:?}{nth}"));
            seed_manifest(&b, CHUNK);
            let dir = run_dir(&b);
            fault::arm(&dir, op, nth);
            assert!(run_scan(&b, CHUNK).await.is_err(), "{op:?} nth={nth}");
            fault::disarm(&dir);
            run_scan(&b, CHUNK)
                .await
                .unwrap_or_else(|e| panic!("{op:?} nth={nth}: resume: {e:#}"));
            assert_same_outputs(&run_dir(&a), &run_dir(&b));
            let _ = std::fs::remove_dir_all(&b);
        }
    }
    drop(env);
}

/// Medium-2: decode-QA sample note must survive a first-chunk RPC outage followed by resume.
#[tokio::test]
async fn qa_sample_note_survives_first_chunk_outage() {
    let env = Env::new().await;
    for (label, mask) in [
        ("allbad", 1023u64),
        ("mixed", 0b11_1010_1010),
        ("lastbad", 1 << 9),
    ] {
        env.bad_mask(mask);
        let a = uninterrupted(&format!("qs_{label}_a")).await;
        for fail in [FROM, FROM + 100, FROM + 300, FROM + 900] {
            let b = tmp_root(&format!("qs_{label}_b{fail}"));
            seed_manifest(&b, CHUNK);
            env.fail_from(fail);
            assert!(run_scan(&b, CHUNK).await.is_err());
            env.fail_from(0);
            run_scan(&b, CHUNK)
                .await
                .unwrap_or_else(|e| panic!("{label}@{fail}: {e:#}"));
            assert_same_outputs(&run_dir(&a), &run_dir(&b));
            let _ = std::fs::remove_dir_all(&b);
        }
    }
}

/// Fault sweep with EXACT accounting. For every (fault kind, call index) the fault must actually fire
/// (first run errors), the retry must succeed, and every output file (incl. artifact_manifest.json
/// and its SHA256 values) must equal the uninterrupted reference byte for byte. One index past the
/// last call of each kind must NOT fire, which pins the call counts.
///
/// Totals: SaveProgress 10, Append 9, TornAppend 9, TmpCreate 13, TmpWrite 13, TmpSync 13,
/// Rename 13, DirSync 12 = 92. DirSync index 12 (the manifest renamed into place, then an error)
/// resumes through the completed-chain load path and is in `fault_matrix_dirsync_manifest_commit_case`
/// (ignored, issue #14), not in these 92.
#[tokio::test]
async fn fault_matrix_sweep_recovers_identically() {
    let env = Env::new().await;
    env.bad_mask(0b00_0010_0000);
    let a = uninterrupted("mx_a").await;
    let plan: [(Op, i64); 8] = [
        (Op::SaveProgress, 10),
        (Op::Append, 9),
        (Op::TornAppend, 9),
        (Op::TmpCreate, 13),
        (Op::TmpWrite, 13),
        (Op::TmpSync, 13),
        (Op::Rename, 13),
        (Op::DirSync, 12),
    ];
    let mut total = 0;
    for (op, calls) in plan {
        let mut fired = 0;
        for nth in 0..calls {
            let b = tmp_root(&format!("mx_{op:?}{nth}"));
            seed_manifest(&b, CHUNK);
            let dir = run_dir(&b);
            fault::arm(&dir, op, nth);
            let first = run_scan(&b, CHUNK).await;
            fault::disarm(&dir);
            assert!(
                first.is_err(),
                "{op:?} nth={nth}: fault must fire and fail the run"
            );
            run_scan(&b, CHUNK)
                .await
                .unwrap_or_else(|e| panic!("{op:?} nth={nth}: resume: {e:#}"));
            assert_same_outputs(&run_dir(&a), &run_dir(&b));
            fired += 1;
            let _ = std::fs::remove_dir_all(&b);
        }
        // one past the last call of this kind never fires (a DirSync "call 12" is the ignored case)
        if op != Op::DirSync {
            let b = tmp_root(&format!("mx_{op:?}_past"));
            seed_manifest(&b, CHUNK);
            let dir = run_dir(&b);
            fault::arm(&dir, op, calls);
            let r = run_scan(&b, CHUNK).await;
            fault::disarm(&dir);
            assert!(
                r.is_ok(),
                "{op:?}: index {calls} must be past the last call"
            );
            let _ = std::fs::remove_dir_all(&b);
        }
        assert_eq!(fired, calls, "{op:?}");
        total += fired;
    }
    println!("fault matrix: {total} faulted runs recovered byte-identically");
    assert_eq!(total, 92);
}

/// DirSync at call index 12: the manifest is renamed into place (chain committed), then the call
/// errors. Resume goes through the completed-chain load path, whose outputs differ from an
/// uninterrupted run until egpivo/stablecoin-lab#14 (PR #15) is fixed.
#[tokio::test]
#[ignore = "completed-chain load path diverges; fixed by PR #15 / issue #14"]
async fn fault_matrix_dirsync_manifest_commit_case() {
    let env = Env::new().await;
    env.bad_mask(0b00_0010_0000);
    let a = uninterrupted("mx12_a").await;
    let b = tmp_root("mx12_b");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    fault::arm(&dir, Op::DirSync, 12);
    assert!(run_scan(&b, CHUNK).await.is_err(), "fault must fire");
    fault::disarm(&dir);
    run_scan(&b, CHUNK).await.expect("resume");
    assert_same_outputs(&run_dir(&a), &run_dir(&b));
}

fn diff_names(a: &Path, b: &Path) -> Vec<String> {
    let (fa, fb) = (files_in(a), files_in(b));
    let mut out = vec![];
    for (n, x) in &fa {
        match fb.iter().find(|y| &y.0 == n) {
            Some(y) if y.1 == *x => {}
            Some(y) => out.push(format!("DIFF {n} ({} vs {} bytes)", x.len(), y.1.len())),
            None => out.push(format!("MISSING {n}")),
        }
    }
    out
}

/// KNOWN ISSUE, not fixed in this PR: when a run resumes through the completed-chain LOAD path
/// (chain already committed in the manifest), outputs derived from `#[serde(skip)]` fields of
/// `SupplyAuditRow` (e.g. supply_snapshots.csv) differ from an uninterrupted run. Run explicitly with
/// `cargo test -- --ignored completed_chain_load_path_known_divergence`.
#[tokio::test]
#[ignore = "known pre-existing issue in the completed-chain load path; tracked in egpivo/stablecoin-lab#14"]
async fn completed_chain_load_path_known_divergence() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let a = uninterrupted("kd_a").await;
    let b = tmp_root("kd_b");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    let root = ckpt::checkpoint_root(&dir);
    fault::arm(&root, Op::Stage(4), 0);
    assert!(run_scan(&b, CHUNK).await.is_err());
    fault::disarm(&root);
    run_scan(&b, CHUNK).await.expect("resume");
    let d = diff_names(&run_dir(&a), &dir);
    println!("KNOWN-DIVERGENCE diffs: {d:?}");
    println!(
        "uninterrupted supply_snapshots.csv:\n{}",
        read(&run_dir(&a).join("supply_snapshots.csv"))
    );
    println!(
        "resumed supply_snapshots.csv ({} bytes):\n{}",
        std::fs::metadata(dir.join("supply_snapshots.csv"))
            .unwrap()
            .len(),
        read(&dir.join("supply_snapshots.csv"))
    );
    assert!(
        d.is_empty(),
        "outputs diverge on the completed-chain load path: {d:?}"
    );
}

/// Cleanup failure AFTER the manifest commit with a REAL `remove_file` error (the progress path is a
/// non-empty directory, which `remove_file` rejects). The earlier `Stage(4)` test fails before the
/// removal call and so cannot tell "cleanup failed" from "cleanup skipped"; this one can: with the old
/// `remove_file(..).ok()` the first run would succeed and the `is_err` assertion would fail.
#[tokio::test]
async fn completed_chain_real_remove_failure_is_surfaced_and_harmless() {
    let env = Env::new().await;
    let b = tmp_root("rm_real");
    seed_manifest(&b, CHUNK);
    let dir = run_dir(&b);
    let root = ckpt::checkpoint_root(&dir);
    let prog = ckpt::fetch_progress_path(&dir, "ethereum");
    fault::arm(&prog, Op::BlockRemove, 0);
    let first = run_scan(&b, CHUNK).await;
    fault::disarm(&prog);
    let e = err_text(first);
    assert!(
        e.contains("remove") && e.contains("fetch_progress_ethereum.json"),
        "{e}"
    );
    assert!(e.contains("manifest ALREADY COMMITTED"), "{e}");
    assert!(
        e.contains("cleanup of the in-flight fetch-progress files failed"),
        "{e}"
    );
    assert!(
        e.contains("recognizes the chain as complete (no refetch)"),
        "{e}"
    );
    // (a) the leftover is there and the chain is committed
    assert!(prog.exists(), "stale progress path left behind");
    let m = ckpt::load_manifest(&dir).unwrap().unwrap();
    assert_eq!(m.completed_chains, vec!["ethereum".to_string()]);
    // (b) resume: chain complete, no refetch; (c) stale leftovers never read, dir cleaned at the end
    env.clear_requests();
    run_scan(&b, CHUNK).await.expect("resume");
    assert!(env.state.log_requests.lock().unwrap().is_empty());
    assert!(
        !root.exists(),
        "stale checkpoint removed with the checkpoint dir"
    );
    assert!(dir.join("artifact_manifest.json").is_file());
}

// ---------------------------------------------------------------------------------------------
// schema v3: chunk_notes is required and consistent with decode_errors
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn schema_v3_round_trips_with_decode_notes() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let root = tmp_root("v3_rt");
    interrupt_at(&env, &root, FROM + 800).await;
    let p = load_progress(&root).unwrap().unwrap();
    assert_eq!(p.schema, "transfer-audit-fetch-chunk-v3");
    assert_eq!(p.decode_errors, 1);
    assert_eq!(
        p.chunk_notes
            .iter()
            .filter(|n| n.starts_with("decode: "))
            .count(),
        1
    );
    ckpt::save_fetch_progress(&run_dir(&root), &p).unwrap();
    let again = load_progress(&root).unwrap().unwrap();
    assert_eq!(
        serde_json::to_value(&p).unwrap(),
        serde_json::to_value(&again).unwrap()
    );
}

#[tokio::test]
async fn schema_v3_rejects_missing_chunk_notes() {
    let (_env, root) = checkpointed("v3_missing").await;
    edit_progress(&root, |v| {
        v.as_object_mut().unwrap().remove("chunk_notes");
    });
    let e = err_text(load_progress(&root));
    assert!(
        e.contains("chunk_notes") || e.contains("missing field"),
        "{e}"
    );
    assert!(e.contains("--fresh"), "{e}");
}

#[tokio::test]
async fn legacy_v2_progress_is_rejected() {
    let (_env, root) = checkpointed("v3_legacy").await;
    edit_progress(&root, |v| {
        v["schema"] = json!("transfer-audit-fetch-chunk-v2")
    });
    let e = err_text(load_progress(&root));
    assert!(
        e.contains("unsupported fetch progress schema") && e.contains("--fresh"),
        "{e}"
    );
    assert!(e.contains("transfer-audit-fetch-chunk-v3"), "{e}");
}

#[tokio::test]
async fn decode_errors_without_decode_notes_are_rejected() {
    let env = Env::new().await;
    env.bad_only(BAD_CHUNK);
    let root = tmp_root("v3_notes_missing");
    interrupt_at(&env, &root, FROM + 800).await;
    edit_progress(&root, |v| v["chunk_notes"] = json!([]));
    let e = err_text(load_progress(&root));
    assert!(
        e.contains("decode_errors") && e.contains("chunk_notes"),
        "{e}"
    );
    // and the reverse: fabricated decode notes with no decode errors
    edit_progress(&root, |v| {
        v["decode_errors"] = json!(0);
        v["events_persisted"] = json!(v["logs_fetched"].as_u64().unwrap());
        v["chunk_notes"] = json!(["decode: invented"]);
    });
    let e = err_text(load_progress(&root));
    assert!(
        e.contains("decode_errors") && e.contains("chunk_notes"),
        "{e}"
    );
}

// ---------------------------------------------------------------------------------------------
// L1: stale partial data without progress must be cleared or the run must stop
// ---------------------------------------------------------------------------------------------

#[tokio::test]
async fn stale_partial_that_cannot_be_cleared_stops_the_run() {
    let env = Env::new().await;
    let a = uninterrupted("l1_a").await;
    let b = tmp_root("l1_b");
    interrupt_at(&env, &b, FROM + 500).await;
    env.fail_from(0);
    let dir = run_dir(&b);
    let partial = ckpt::fetch_partial_path(&dir, "ethereum");
    // stale partial data with NO progress file
    std::fs::remove_file(ckpt::fetch_progress_path(&dir, "ethereum")).unwrap();
    assert!(partial.exists());
    fault::arm(&partial, Op::BlockRemove, 0);
    env.clear_requests();
    let first = run_scan(&b, CHUNK).await;
    fault::disarm(&partial);
    assert!(first.is_err());
    let qa = read(&dir.join("qa_report.json"));
    assert!(
        qa.contains("fetch_partial_ethereum.csv") && qa.contains("stale"),
        "{qa}"
    );
    assert!(
        env.state.log_requests.lock().unwrap().is_empty(),
        "must not start a new fetch on top of stale partial data"
    );
    // operator clears the blocker; the retry cleans normally and matches an uninterrupted run
    std::fs::remove_dir_all(&partial).unwrap();
    run_scan(&b, CHUNK).await.expect("retry");
    assert_same_outputs(&run_dir(&a), &run_dir(&b));
}
