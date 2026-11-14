# stablecoin-geo-lab

Data layer for the stablecoin purchasing-power map: how much one stablecoin, held since
January 2021, buys for a holder in each country. The animated map lives on
[egpivo.dev](https://egpivo.dev) under Lab; this repository fetches, pins and builds its data.

## Build

```bash
make fetch     # download BIS, World Bank and DefiLlama files into data/raw/, pin them
make build     # verify the pins, then write data/out/map_data.json and purchasing_power.csv
make test
```

`sgl pin` rewrites the manifest for raw files obtained some other way, such as a snapshot
copied from another machine. `sgl build` refuses to run when a raw file does not match
`data/manifest/raw.sha256`.

## Layout

| Path | Contents |
|---|---|
| `src/fill.rs` | gap rules: interior log-linear interpolation, short trailing extension |
| `src/metrics.rs` | purchasing power by holder country and peg currency |
| `src/sources.rs` | source URLs and parsers |
| `src/config.rs` | pegs, euro members, dollarized countries, exclusions |
| `data/raw/` | raw downloads (not committed) |
| `data/manifest/raw.sha256` | pins for the raw files |
| `data/out/` | built panel |
| `python/` | static plots and the parity check against the first prototype |

Method and limits: [docs/METHOD.md](docs/METHOD.md).
