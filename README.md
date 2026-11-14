# stablecoin-lab

[![CI](https://github.com/egpivo/stablecoin-audit/actions/workflows/ci.yml/badge.svg)](https://github.com/egpivo/stablecoin-audit/actions/workflows/ci.yml)
[![codecov](https://codecov.io/github/egpivo/stablecoin-audit/graph/badge.svg?token=mN0h7zLOtR)](https://codecov.io/github/egpivo/stablecoin-audit)

Tools for looking at stablecoins one layer at a time. Each layer answers one question
and makes no claim about the others.

| Layer | Question | Code | CLI | Docs |
|---|---|---|---|---|
| **audit** | Does onchain supply reconcile? ERC-20 mint/burn vs `totalSupply` at pinned blocks, per chain | `src/` | `stablecoin-audit` | [docs/audit](docs/audit/README.md) |
| **audit · control** (experimental) | Which issuer control events (freeze, blacklist, pause) appear in a window? | `src/control_events`, `src/rpc/control_*` | `stablecoin-audit` built with `--features experimental` | [docs/README.md](docs/README.md#cli) |
| **map** | Where do stablecoins sit: footprint by peg, transfer activity, DEX counterparts | `src/stablecoin_map.rs` | `stablecoin-audit stablecoin-map-package` | [docs/README.md](docs/README.md) |
| **value** | What does one stablecoin, held since 2021, buy for a holder in each country? | `crates/value/` | `stablecoin-value` | [docs/value](docs/value/README.md) |

Issuer geography (where a coin is issued and regulated) is context for the audit layer:
`data/external/stablecoin_geo_context.csv`. Holder geography (where the coin is spent) is
the value layer.

## Quick start

```bash
cargo build --workspace

# audit: RPC URLs in .env, see .env.example
cargo run -- transfer-audit --asset USDC --run-id smoke --window ethereum:24000000:24001000

# value: pinned public sources -> crates/value/data/out/map_data.json
cargo run -p stablecoin-value -- fetch
cargo run -p stablecoin-value -- build
```

Evidence console: [egpivo.github.io/stablecoin-lab/ui/](https://egpivo.github.io/stablecoin-lab/ui/).
Benchmarks, panels and the blog evidence map: [docs/README.md](docs/README.md).

## Related writing

This repo grew out of a short stablecoin evidence series. The posts are not required to run the toolkit, but they explain the motivation behind the artifact-first design.

| Post | Focus | How it connects to this repo |
|------|-------|------------------------------|
| [USDC Shows Why Stablecoin Risk Analysis Is Not One Signal](https://medium.com/coinmonks/usdc-shows-why-stablecoin-risk-analysis-is-not-one-signal-bb9c333ed169) | Per-chain USDC transfer and supply signals | Motivation for treating each asset × chain deployment as a separate audit unit |
| [Local Pegs, Dollar Rails: Auditing XSGD and EURC Liquidity](https://medium.com/towards-finance/local-pegs-dollar-rails-auditing-xsgd-and-eurc-liquidity-9ed540b84cfe) | Local-currency stablecoins and observed USDC-paired liquidity | Motivation for keeping liquidity observations separate from reserve, peg, and routing claims |
| [The Stablecoin Map: What Crypto’s Cash Rails Depend On](https://medium.com/thecapital/the-stablecoin-map-what-cryptos-cash-rails-depend-on-e50e429d0fbe) | Stablecoin footprint, transfer activity, DEX counterparts, and claim boundaries | Motivation for turning article evidence into reproducible artifacts, manifests, and an Evidence Console |

MIT — [LICENSE](LICENSE).
