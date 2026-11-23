# Checkpoint resume (transfer-audit)

`transfer-audit` checkpoints under `out/<asset>/runs/<run_id>/checkpoint/`. Re-running with the same
`--run-id`, `--window` and `--chunk-size` resumes; `--fresh` discards the checkpoint.

## Guarantees

- Resume restores `value_u256` from the persisted `value_raw`, so gross mint/burn sums of a resumed
  scan equal those of an uninterrupted scan (before this fix a resume counted every pre-interruption
  event as zero value).
- The progress file is the commit point. Rows of a chunk are appended to `fetch_partial_<chain>.csv`
  (single write + fsync) and counters advance only after the progress file is durably saved
  (tmp file fsync, rename, directory fsync). A failed progress write rolls the append back.
- A torn final line in the partial CSV (crash mid-append) belongs to an uncommitted chunk and is
  dropped on resume; rows beyond `last_fetched_through` are dropped likewise. Every other
  inconsistency (bad schema, missing fields, truncated JSON, row count or window mismatch,
  undecodable `value_raw`, counter contradictions) fails closed with a `--fresh` hint.
- A chunk whose `eth_getLogs` exhausted retries is recorded as `unresolved` and fetched first on
  resume. Local storage failures are reported as `checkpoint storage error` and are never recorded as
  RPC failures.

## Compatibility: fetch progress schema v1 -> v2

`transfer-audit-fetch-chunk-v1` progress files are **not** readable by this version. Resuming an
in-flight v1 checkpoint fails with `unsupported fetch progress schema ... use --fresh`. Finish such a
run with the old binary, or restart it with `--fresh` (re-fetches that chain). Completed-chain
checkpoints (`transfers_<chain>.csv`, `chain_<chain>.json`, `manifest.json`) keep their schema, but
completed-chain CSVs are now cross-checked against their bundle (row count, mint/burn sums).
v2 adds `events_persisted`, `decode_errors`, `unresolved` and `chunk_notes`.
Runs finished with the old resume code are not repaired; re-run or flag any whose supply invariant
failed after a resume.
