//! Outputs: viewer JSON and a long CSV for plotting scripts.

use std::fs;
use std::path::Path;

use crate::metrics::MapData;

pub fn write_json(data: &MapData, path: &Path) -> anyhow::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    // Trailing newline keeps the file stable under the end-of-file-fixer hook.
    fs::write(path, serde_json::to_string(data)? + "\n")?;
    Ok(())
}

/// One row per country × peg × month: area, iso3, month, peg, pp, local, estimated, source.
pub fn write_csv(data: &MapData, path: &Path) -> anyhow::Result<()> {
    if let Some(p) = path.parent() {
        fs::create_dir_all(p)?;
    }
    let mut w = csv::Writer::from_path(path)?;
    w.write_record([
        "area",
        "iso3",
        "month",
        "peg",
        "pp",
        "local",
        "estimated",
        "source",
    ])?;
    let cell = |v: Option<f64>| v.map(|x| format!("{x:.4}")).unwrap_or_default();
    for (area, row) in &data.countries {
        for (peg, series) in &row.pp {
            for (i, month) in data.months.iter().enumerate() {
                w.write_record([
                    area.as_str(),
                    &row.iso3,
                    month,
                    peg,
                    &cell(series[i]),
                    &cell(row.local[i]),
                    if row.estimated[i] { "1" } else { "0" },
                    &row.source,
                ])?;
            }
        }
    }
    w.flush()?;
    Ok(())
}
