# Checkpoint resume (transfer-audit)

`transfer-audit` checkpoints under `out/<asset>/runs/<run_id>/checkpoint/`. Re-running with the same
`--run-id`, `--window` and `--chunk-size` resumes; `--fresh` discards the checkpoint.

## Protocol

- Resume restores `value_u256` from the persisted `value_raw`, so gross mint/burn sums of a resumed
  scan equal those of an uninterrupted scan (before this fix a resume counted every pre-interruption
  event as zero value).
- **The progress file (`fetch_progress_<chain>.json`) is the single commit point of a chunk.** Rows of
  a chunk are appended to `fetch_partial_<chain>.csv` (single write + fsync; a short write inside that
  append is cut back by the append itself), then the progress file is written (tmp file fsync, rename,
  directory fsync). Counters in memory advance only after the save returns success.
- **There is no rollback after a failed progress write.** If the save returns an error it is
  ambiguous: the error can come before the rename (old progress on disk) or after it (new progress on
  disk). The on-disk progress is therefore either the previous or the new committed state, and both are
  resumable. Recovery uses the persisted `last_fetched_through` boundary: rows in the partial CSV for
  blocks beyond it belong to an uncommitted chunk and are dropped on load (that chunk is fetched
  again); rows up to it must match `events_persisted`.
- A torn final line in the partial CSV (crash mid-append, no trailing newline) belongs to an
  uncommitted chunk and is dropped on resume. Every other inconsistency (bad schema, missing fields,
  truncated JSON, row count or window mismatch, undecodable `value_raw`, counter contradictions) fails
  closed with a `--fresh` hint.
- A chunk whose `eth_getLogs` exhausted retries is recorded as `unresolved` and fetched first on
  resume. Local storage failures are reported as `checkpoint storage error` and are never recorded as
  RPC failures.

## Completed-chain order and cleanup failure

A finished chain is saved in this order: (1) `transfers_<chain>.csv`, (2) `chain_<chain>.json`,
(3) the manifest entry that lists the chain in `completed_chains` (the commit point), (4) removal of
the in-flight fetch-progress files. A failure before (3) leaves the fetch progress intact, so resume
redoes only the tail. A failure at (4) is still a hard error, but its message states that the manifest
is already committed and that retrying recognizes the chain as complete without refetching; the stale
progress files are never read and are removed with the checkpoint directory when the run succeeds.

## Compatibility: fetch progress schema v1 -> v2

`transfer-audit-fetch-chunk-v1` progress files are **not** readable by this version. Resuming an
in-flight v1 checkpoint fails with `unsupported fetch progress schema ... use --fresh`. Finish such a
run with the old binary, or restart it with `--fresh` (re-fetches that chain). Completed-chain
checkpoints (`transfers_<chain>.csv`, `chain_<chain>.json`, `manifest.json`) keep their schema, but
completed-chain CSVs are now cross-checked against their bundle (row count, mint/burn sums).
v2 adds `events_persisted`, `decode_errors`, `unresolved` and `chunk_notes`.
Runs finished with the old resume code are not repaired; re-run or flag any whose supply invariant
failed after a resume.

## Known issue

Resuming through the completed-chain load path yields an empty `supply_snapshots.csv` and a different
`artifact_manifest.json` (tracked in egpivo/stablecoin-lab#14, not fixed here).
