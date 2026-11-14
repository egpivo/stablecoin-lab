//! Download the raw sources. The World Bank API can take minutes, so timeouts are long and
//! each request is retried.

use std::fs;
use std::path::Path;
use std::thread::sleep;
use std::time::Duration;

use anyhow::{Context, bail};

use crate::month::Month;
use crate::sources::SOURCES;

pub fn url_for(template: &str, start: Month, end: Month) -> String {
    let first = start.plus(-1);
    template
        .replace("{start}", &first.to_string())
        .replace(
            "{start_wb}",
            &format!("{}M{:02}", first.year(), first.month()),
        )
        .replace("{end_wb}", &format!("{}M{:02}", end.year(), end.month()))
}

pub fn fetch_all(raw_dir: &Path, start: Month, end: Month) -> anyhow::Result<Vec<&'static str>> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(600))
        .user_agent(concat!("stablecoin-geo-lab/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let mut done = Vec::new();
    for src in SOURCES {
        let url = url_for(src.url, start, end);
        let dest = raw_dir.join(src.path);
        fs::create_dir_all(dest.parent().unwrap())?;
        let mut last_err = None;
        for attempt in 1..=3 {
            eprintln!("fetch {} (attempt {attempt}): {}", src.path, src.note);
            match client
                .get(&url)
                .send()
                .and_then(|r| r.error_for_status())
                .and_then(|r| r.bytes())
            {
                Ok(body) if !body.is_empty() => {
                    fs::write(&dest, &body).with_context(|| format!("write {}", dest.display()))?;
                    last_err = None;
                    break;
                }
                Ok(_) => last_err = Some(anyhow::anyhow!("empty body")),
                Err(e) => last_err = Some(e.into()),
            }
            sleep(Duration::from_secs(5 * attempt));
        }
        if let Some(e) = last_err {
            bail!("{}: {e}", src.path);
        }
        done.push(src.path);
    }
    Ok(done)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn url_templates_use_the_month_before_the_base() {
        let u = url_for(
            "a?s={start}&d={start_wb}:{end_wb}",
            Month::new(2021, 1),
            Month::new(2026, 9),
        );
        assert_eq!(u, "a?s=2020-12&d=2020M12:2026M09");
    }
}
