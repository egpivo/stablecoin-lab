//! Gap rules for a monthly series on a fixed window.
//!
//! * Interior gaps: log-linear interpolation between the nearest observed months.
//! * Trailing gap of at most `max_trailing` months: extend the last value at the mean of the
//!   last `drift_window` monthly log changes (observed or interpolated).
//! * Leading gaps and longer trailing gaps stay empty.
//!
//! Filled months are flagged so the viewer can mark them as estimated.

use std::collections::BTreeMap;

use crate::month::Month;

pub type Series = BTreeMap<Month, f64>;

#[derive(Debug, Clone, PartialEq)]
pub struct Filled {
    pub values: Vec<Option<f64>>,
    pub estimated: Vec<bool>,
}

pub fn fill(series: &Series, months: &[Month], max_trailing: i32, drift_window: usize) -> Filled {
    let observed: Vec<Option<f64>> = months
        .iter()
        .map(|m| series.get(m).copied().filter(|v| v.is_finite() && *v > 0.0))
        .collect();
    let mut logs: Vec<Option<f64>> = observed.iter().map(|v| v.map(f64::ln)).collect();

    // Interior: log-linear between consecutive observed points.
    let known: Vec<usize> = (0..logs.len()).filter(|&i| logs[i].is_some()).collect();
    for pair in known.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let (la, lb) = (logs[a].unwrap(), logs[b].unwrap());
        for (k, slot) in logs[a + 1..b].iter_mut().enumerate() {
            let w = (k + 1) as f64 / (b - a) as f64;
            *slot = Some(la + w * (lb - la));
        }
    }

    // Trailing: short extension at recent drift.
    if let Some(&last) = known.last() {
        let gap = (logs.len() - 1 - last) as i32;
        if gap > 0 && gap <= max_trailing {
            let diffs: Vec<f64> = (1..=last)
                .filter_map(|i| Some(logs[i]? - logs[i - 1]?))
                .collect();
            let tail = &diffs[diffs.len().saturating_sub(drift_window)..];
            if !tail.is_empty() {
                let drift = tail.iter().sum::<f64>() / tail.len() as f64;
                let base = logs[last].unwrap();
                for k in 1..=gap as usize {
                    logs[last + k] = Some(base + drift * k as f64);
                }
            }
        }
    }

    let values: Vec<Option<f64>> = logs.iter().map(|v| v.map(f64::exp)).collect();
    let estimated = observed
        .iter()
        .zip(&values)
        .map(|(o, v)| o.is_none() && v.is_some())
        .collect();
    Filled { values, estimated }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn months(n: i32) -> Vec<Month> {
        Month::range(Month::new(2021, 1), Month::new(2021, 1).plus(n - 1))
    }

    fn series(points: &[(i32, f64)]) -> Series {
        points
            .iter()
            .map(|&(i, v)| (Month::new(2021, 1).plus(i), v))
            .collect()
    }

    #[test]
    fn interior_gap_is_log_linear_and_flagged() {
        let f = fill(&series(&[(0, 100.0), (2, 121.0)]), &months(3), 3, 6);
        assert!((f.values[1].unwrap() - 110.0).abs() < 1e-9);
        assert_eq!(f.estimated, vec![false, true, false]);
    }

    #[test]
    fn short_trailing_gap_extends_at_recent_drift() {
        // 1% a month for 7 months, then 2 missing months.
        let pts: Vec<(i32, f64)> = (0..7).map(|i| (i, 100.0 * 1.01f64.powi(i))).collect();
        let f = fill(&series(&pts), &months(9), 3, 6);
        assert!((f.values[8].unwrap() - 100.0 * 1.01f64.powi(8)).abs() < 1e-9);
        assert_eq!(&f.estimated[6..], &[false, true, true]);
    }

    #[test]
    fn long_trailing_and_leading_gaps_stay_empty() {
        let f = fill(&series(&[(1, 100.0), (2, 101.0)]), &months(7), 3, 6);
        assert_eq!(f.values[0], None);
        assert_eq!(f.values[6], None); // 4 months after the last point
        assert!(f.values[5].is_none());
    }

    #[test]
    fn non_positive_values_are_treated_as_missing() {
        let f = fill(
            &series(&[(0, 100.0), (1, 0.0), (2, 100.0)]),
            &months(3),
            3,
            6,
        );
        assert!((f.values[1].unwrap() - 100.0).abs() < 1e-9);
        assert!(f.estimated[1]);
    }
}
