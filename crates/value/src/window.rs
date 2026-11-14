//! Window end: the latest month most countries have already published.
//!
//! CPI and exchange rates arrive with a lag of one to two months, and not every
//! country publishes at once. `auto_end` walks back from the latest observed
//! month to the first month where at least `MIN_COVERAGE` of the areas that
//! reported in the previous 12 months have reported, for both BIS CPI and BIS FX.

use anyhow::bail;

use crate::month::Month;
use crate::sources::Panel;

pub const MIN_COVERAGE: f64 = 0.8;
const LOOKBACK: i32 = 12;
const MAX_STEPS_BACK: i32 = 24;

/// Share of areas with an observation at `m`, among areas that reported in the
/// `LOOKBACK` months before it. Zero when no area reported in that span.
pub fn coverage(panel: &Panel, m: Month) -> f64 {
    let mut base = 0usize;
    let mut hits = 0usize;
    for series in panel.values() {
        let recent = (1..=LOOKBACK).any(|k| series.contains_key(&m.plus(-k)));
        if recent {
            base += 1;
            if series.contains_key(&m) {
                hits += 1;
            }
        }
    }
    if base == 0 {
        0.0
    } else {
        hits as f64 / base as f64
    }
}

fn latest(panel: &Panel) -> Option<Month> {
    panel
        .values()
        .filter_map(|s| s.keys().next_back().copied())
        .max()
}

pub fn auto_end(cpi: &Panel, fx: &Panel) -> anyhow::Result<Month> {
    let Some(top) = latest(cpi) else {
        bail!("no CPI observations: cannot choose a window end")
    };
    for k in 0..=MAX_STEPS_BACK {
        let m = top.plus(-k);
        if coverage(cpi, m) >= MIN_COVERAGE && coverage(fx, m) >= MIN_COVERAGE {
            return Ok(m);
        }
    }
    bail!("no month within {MAX_STEPS_BACK} months of {top} has {MIN_COVERAGE} coverage")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill::Series;

    fn series(start: Month, n: i32) -> Series {
        (0..n).map(|i| (start.plus(i), 100.0)).collect()
    }

    #[test]
    fn stops_at_the_last_month_most_areas_reported() {
        let s = Month::new(2025, 1);
        let mut cpi = Panel::new();
        // Ten areas through 2025-12; two of them also report 2026-01.
        for i in 0..10 {
            let n = if i < 2 { 13 } else { 12 };
            cpi.insert(format!("A{i}"), series(s, n));
        }
        let fx = cpi.clone();
        assert_eq!(coverage(&cpi, Month::new(2026, 1)), 0.2);
        assert_eq!(auto_end(&cpi, &fx).unwrap(), Month::new(2025, 12));
    }

    #[test]
    fn fx_lag_also_holds_the_end_back() {
        let s = Month::new(2025, 1);
        let mut cpi = Panel::new();
        let mut fx = Panel::new();
        for i in 0..5 {
            cpi.insert(format!("A{i}"), series(s, 12));
            fx.insert(format!("A{i}"), series(s, 11));
        }
        assert_eq!(auto_end(&cpi, &fx).unwrap(), Month::new(2025, 11));
        assert!(auto_end(&Panel::new(), &fx).is_err());
    }
}
