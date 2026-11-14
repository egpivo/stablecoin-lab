//! stablecoin-value: the value layer of stablecoin-lab. Purchasing power of stablecoins by holder country.
//!
//! `fetch` downloads public sources into `data/raw/` and pins them with a SHA-256 manifest.
//! `build` turns them into a monthly panel and writes `data/out/map_data.json` (viewer schema)
//! and `data/out/purchasing_power.csv` (long format). See docs/METHOD.md.

pub mod config;
pub mod export;
pub mod fetch;
pub mod fill;
pub mod manifest;
pub mod metrics;
pub mod month;
pub mod sources;
pub mod window;
