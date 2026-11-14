[stablecoin-lab](../../README.md) / value

# Value layer: stablecoin-value

Data layer for the stablecoin purchasing-power map: how much one stablecoin, held since
January 2021, buys for a holder in each country. The animated map lives on
[egpivo.dev](https://egpivo.dev) under Lab; `crates/value` fetches, pins and builds its data.

## Build

```bash
make -C crates/value fetch     # download BIS, World Bank and DefiLlama files into data/raw/, pin them
make -C crates/value build     # verify the pins, then write data/out/map_data.json and purchasing_power.csv
make test
```

`stablecoin-value pin` rewrites the manifest for raw files obtained some other way, such as a snapshot
copied from another machine. `stablecoin-value build` refuses to run when a raw file does not match
`crates/value/data/manifest/raw.sha256`.

## Layout

| Path | Contents |
|---|---|
| `crates/value/src/fill.rs` | gap rules: interior log-linear interpolation, short trailing extension |
| `crates/value/src/metrics.rs` | purchasing power by holder country and peg currency |
| `crates/value/src/sources.rs` | source URLs and parsers |
| `crates/value/src/config.rs` | pegs, euro members, dollarized countries, exclusions |
| `crates/value/data/raw/` | raw downloads (not committed) |
| `crates/value/data/manifest/raw.sha256` | pins for the raw files |
| `crates/value/data/out/` | built panel |
| `crates/value/python/` | static plots and the parity check against the first prototype |

Method and limits: [METHOD.md](METHOD.md).
