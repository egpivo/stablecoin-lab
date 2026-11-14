//! Raw source files: where they come from, and how each is parsed into monthly series.
//!
//! All files land under `data/raw/` and are pinned by `data/manifest/raw.sha256`.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::{Context, bail};
use serde_json::Value;

use crate::config;
use crate::fill::Series;
use crate::month::Month;

pub struct Source {
    pub path: &'static str,
    pub url: &'static str,
    pub note: &'static str,
}

/// `{start}` is replaced by the first month fetched (one month before the base month).
pub const SOURCES: &[Source] = &[
    Source {
        path: "bis/cpi_index.csv",
        url: "https://stats.bis.org/api/v2/data/dataflow/BIS/WS_LONG_CPI/1.0/M..628?startPeriod={start}&format=csv",
        note: "BIS consumer prices, monthly index (unit 628)",
    },
    Source {
        path: "bis/fx_monthly.csv",
        url: "https://stats.bis.org/api/v2/data/dataflow/BIS/WS_XRU/1.0/M....A?startPeriod={start}&format=csv",
        note: "BIS US dollar exchange rates, monthly average (local currency per USD)",
    },
    Source {
        path: "worldbank/gem_cpi.json",
        url: "https://api.worldbank.org/v2/country/all/indicator/CPTOTNSXN?source=15&format=json&per_page=30000&date={start_wb}:{end_wb}",
        note: "World Bank Global Economic Monitor CPI, not seasonally adjusted",
    },
    Source {
        path: "worldbank/countries.json",
        url: "https://api.worldbank.org/v2/country?format=json&per_page=400",
        note: "World Bank country list (ISO3 to ISO2)",
    },
    Source {
        path: "defillama/stablecoins.json",
        url: "https://stablecoins.llama.fi/stablecoins?includePrices=true",
        note: "DefiLlama stablecoin list with circulating supply by peg type",
    },
];

pub type Panel = BTreeMap<String, Series>;

/// BIS SDMX CSV: one row per observation. `collection` filters WS_XRU to averages ("A").
pub fn parse_bis_csv(path: &Path, collection: Option<&str>) -> anyhow::Result<Panel> {
    let mut reader =
        csv::Reader::from_path(path).with_context(|| format!("open {}", path.display()))?;
    let headers = reader.headers()?.clone();
    let col = |name: &str| headers.iter().position(|h| h == name);
    let (area, period, value) = match (col("REF_AREA"), col("TIME_PERIOD"), col("OBS_VALUE")) {
        (Some(a), Some(p), Some(v)) => (a, p, v),
        _ => bail!("{}: missing REF_AREA/TIME_PERIOD/OBS_VALUE", path.display()),
    };
    let coll = col("COLLECTION");
    let mut panel = Panel::new();
    for row in reader.records() {
        let row = row?;
        if let (Some(want), Some(i)) = (collection, coll)
            && &row[i] != want
        {
            continue;
        }
        let Ok(v) = row[value].parse::<f64>() else {
            continue;
        };
        let m: Month = row[period].parse()?;
        let prev = panel.entry(row[area].to_string()).or_default().insert(m, v);
        if let Some(p) = prev
            && (p - v).abs() > 1e-9
        {
            bail!(
                "{}: conflicting values for {} {}",
                path.display(),
                &row[area],
                m
            );
        }
    }
    Ok(panel)
}

/// World Bank country list → ISO3 to ISO2, countries only (no aggregates).
pub fn parse_wb_countries(path: &Path) -> anyhow::Result<HashMap<String, String>> {
    let doc: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let rows = doc
        .get(1)
        .and_then(Value::as_array)
        .context("World Bank country list: no rows")?;
    let mut map: HashMap<String, String> = rows
        .iter()
        .filter(|r| r["region"]["value"].as_str() != Some("Aggregates"))
        .filter_map(|r| {
            Some((
                r["id"].as_str()?.to_string(),
                r["iso2Code"].as_str()?.to_string(),
            ))
        })
        .collect();
    for (iso3, iso2) in config::EXTRA_ISO3 {
        map.insert(iso3.to_string(), iso2.to_string());
    }
    Ok(map)
}

/// World Bank indicator JSON → panel keyed by ISO2. Aggregates are dropped.
pub fn parse_wb_indicator(
    path: &Path,
    iso3_to_2: &HashMap<String, String>,
) -> anyhow::Result<Panel> {
    let doc: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let meta = doc.get(0).context("World Bank indicator: no header")?;
    if meta["pages"].as_u64().unwrap_or(1) > 1 {
        bail!(
            "{}: more than one page; raise per_page and fetch again",
            path.display()
        );
    }
    let rows = doc
        .get(1)
        .and_then(Value::as_array)
        .context("World Bank indicator: no rows")?;
    let mut panel = Panel::new();
    for r in rows {
        let (Some(id), Some(date), Some(v)) = (
            r["country"]["id"].as_str(),
            r["date"].as_str(),
            r["value"].as_f64(),
        ) else {
            continue;
        };
        let Some(iso2) = iso3_to_2.get(id) else {
            continue;
        };
        panel
            .entry(iso2.clone())
            .or_default()
            .insert(date.parse()?, v);
    }
    Ok(panel)
}

/// DefiLlama stablecoin list → circulating USD value per peg type.
pub fn parse_defillama_supply(path: &Path) -> anyhow::Result<BTreeMap<String, f64>> {
    let doc: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let assets = doc["peggedAssets"]
        .as_array()
        .context("DefiLlama: no peggedAssets")?;
    let mut supply = BTreeMap::new();
    for a in assets {
        let Some(peg) = a["pegType"].as_str() else {
            continue;
        };
        let v = a["circulating"][peg].as_f64().unwrap_or(0.0);
        *supply.entry(peg.to_string()).or_insert(0.0) += v;
    }
    Ok(supply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn bis_csv_filters_collection_and_rejects_conflicts() {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        writeln!(f, "FREQ,REF_AREA,CURRENCY,COLLECTION,TIME_PERIOD,OBS_VALUE").unwrap();
        writeln!(f, "M,JP,JPY,A,2021-01,103.8").unwrap();
        writeln!(f, "M,JP,JPY,E,2021-01,104.6").unwrap();
        writeln!(f, "M,JP,JPY,A,2021-02,105.4").unwrap();
        let p = parse_bis_csv(f.path(), Some("A")).unwrap();
        assert_eq!(p["JP"].len(), 2);
        assert_eq!(p["JP"][&Month::new(2021, 1)], 103.8);
        assert!(parse_bis_csv(f.path(), None).is_err()); // A and E disagree
    }

    #[test]
    fn world_bank_rows_map_to_iso2_and_skip_aggregates() {
        let mut c = tempfile::NamedTempFile::new().unwrap();
        write!(c, r#"[{{"pages":1}},[{{"id":"NGA","iso2Code":"NG","region":{{"value":"Sub-Saharan Africa"}}}},{{"id":"WLD","iso2Code":"1W","region":{{"value":"Aggregates"}}}}]]"#).unwrap();
        let mut i = tempfile::NamedTempFile::new().unwrap();
        write!(i, r#"[{{"pages":1}},[{{"country":{{"id":"NGA"}},"date":"2021M01","value":300.5}},{{"country":{{"id":"WLD"}},"date":"2021M01","value":120.0}},{{"country":{{"id":"TWN"}},"date":"2021M01","value":101.0}},{{"country":{{"id":"NGA"}},"date":"2021M02","value":null}}]]"#).unwrap();
        let map = parse_wb_countries(c.path()).unwrap();
        let p = parse_wb_indicator(i.path(), &map).unwrap();
        assert_eq!(p.keys().cloned().collect::<Vec<_>>(), vec!["NG", "TW"]);
        assert_eq!(p["NG"].len(), 1);
    }
}
