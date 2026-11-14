[stablecoin-lab](../../README.md) / audit

# Audit layer: stablecoin-audit

[![CI](https://github.com/egpivo/stablecoin-audit/actions/workflows/ci.yml/badge.svg)](https://github.com/egpivo/stablecoin-audit/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/egpivo/stablecoin-audit/graph/badge.svg?token=mN0h7zLOtR)](https://codecov.io/github/egpivo/stablecoin-audit)

**0.1.0** — ERC-20 mint/burn vs `totalSupply` at pinned blocks, per chain (`--asset`, `configs/tokens/`). USDC on Ethereum / Base / Arbitrum is the first benchmark.

**Not claimed:** reserves, peg, liquidity, oracles, bridge backing, holder census, intent. [Scope →](../README.md#scope-and-interpretation)

## Architecture

Evidence is filesystem-first: CLI writes artifacts + `artifact_manifest.json`; API and UI read them only (no audit logic in the browser).

![v0 stack](../product/screenshots/architecture_pipeline.svg)

```text
Rust CLI → out/<asset>/runs/<run_id>/ → artifact_manifest.json → API (read-only) → /ui/
```

Full layers, modules, roadmap: [`docs/product/backend_architecture_v0.md`](../product/backend_architecture_v0.md). Manifest schema: [`docs/product/artifact_manifest_schema_v0.md`](../product/artifact_manifest_schema_v0.md).

## Quick start

```bash
cargo build   # RPC URLs in .env — see .env.example

cargo run -- transfer-audit --asset USDC --run-id smoke \
  --window ethereum:24000000:24001000

cargo run -- cross-chain-summary --asset USDC --run-id smoke
```

Add `--window base:…` / `arbitrum:…` for more chains. Outputs: `out/<asset>/runs/<run_id>/`; gates in `qa_report.json`. CLI details: [`docs/README.md#cli`](../README.md#cli).

## Evidence console

```bash
cargo run --features api -- serve --artifact-root out/
# http://127.0.0.1:8080/ui/
```

Public snapshot: [egpivo.github.io/stablecoin-lab/ui/](https://egpivo.github.io/stablecoin-lab/ui/) (`github_pages_demo`). [`docs/GITHUB_PAGES.md`](../GITHUB_PAGES.md)
