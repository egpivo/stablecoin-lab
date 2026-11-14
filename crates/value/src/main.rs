use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Context;
use clap::{Parser, Subcommand};

use stablecoin_value::month::Month;
use stablecoin_value::{export, fetch, manifest, metrics, sources};

#[derive(Parser)]
#[command(
    name = "stablecoin-value",
    about = "Stablecoin purchasing power by holder country"
)]
struct Cli {
    /// Data root holding raw/, manifest/ and out/.
    #[arg(long, default_value = concat!(env!("CARGO_MANIFEST_DIR"), "/data"), global = true)]
    data: PathBuf,
    /// Base month (purchasing power = 1.00).
    #[arg(long, default_value = "2021-01", global = true)]
    start: Month,
    /// Last month of the panel.
    #[arg(long, default_value = "2026-09", global = true)]
    end: Month,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Download every source into raw/ and rewrite manifest/raw.sha256.
    Fetch,
    /// Pin raw files obtained some other way: rewrite manifest/raw.sha256 from what is in raw/.
    Pin,
    /// Check raw/ against manifest/raw.sha256.
    Verify,
    /// Verify, then build out/map_data.json and out/purchasing_power.csv.
    Build,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let raw = cli.data.join("raw");
    let man = cli.data.join("manifest/raw.sha256");
    match cli.cmd {
        Cmd::Fetch => {
            let files = fetch::fetch_all(&raw, cli.start, cli.end)?;
            manifest::write(&raw, &files, &man)?;
            println!("fetched {} files; manifest {}", files.len(), man.display());
        }
        Cmd::Pin => {
            let files: Vec<&str> = sources::SOURCES.iter().map(|s| s.path).collect();
            manifest::write(&raw, &files, &man)?;
            println!("pinned {} files in {}", files.len(), man.display());
        }
        Cmd::Verify => println!(
            "{} files match {}",
            manifest::verify(&raw, &man)?,
            man.display()
        ),
        Cmd::Build => {
            let n = manifest::verify(&raw, &man).context(
                "raw files do not match the manifest; run `stablecoin-value fetch` or restore them",
            )?;
            let bis_cpi = sources::parse_bis_csv(&raw.join("bis/cpi_index.csv"), None)?;
            let fx = sources::parse_bis_csv(&raw.join("bis/fx_monthly.csv"), Some("A"))?;
            let iso3_to_2 = sources::parse_wb_countries(&raw.join("worldbank/countries.json"))?;
            let wb_cpi =
                sources::parse_wb_indicator(&raw.join("worldbank/gem_cpi.json"), &iso3_to_2)?;
            let supply = sources::parse_defillama_supply(&raw.join("defillama/stablecoins.json"))?;
            let iso2_to_3: HashMap<String, String> = iso3_to_2
                .iter()
                .map(|(a, b)| (b.clone(), a.clone()))
                .collect();
            let inputs = metrics::Inputs {
                bis_cpi: &bis_cpi,
                wb_cpi: &wb_cpi,
                fx: &fx,
                supply: &supply,
                iso2_to_3: &iso2_to_3,
            };
            let data = metrics::build(&inputs, cli.start, cli.end);
            let out = cli.data.join("out");
            export::write_json(&data, &out.join("map_data.json"))?;
            export::write_csv(&data, &out.join("purchasing_power.csv"))?;
            let reach = data
                .countries
                .values()
                .filter(|r| r.pp["USD"].last().copied().flatten().is_some())
                .count();
            let est = data
                .countries
                .values()
                .filter(|r| r.estimated.iter().any(|e| *e))
                .count();
            println!(
                "{n} raw files verified; {} countries ({reach} reach {}), {est} with estimated months; excluded: {:?}",
                data.countries.len(),
                cli.end,
                data.excluded.keys().collect::<Vec<_>>()
            );
        }
    }
    Ok(())
}
