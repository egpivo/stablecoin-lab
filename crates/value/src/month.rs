//! Calendar month arithmetic. Accepts `2021-01` (BIS) and `2021M01` (World Bank).

use std::fmt;
use std::str::FromStr;

use anyhow::{Context, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Month {
    index: i32, // months since year 0
}

impl Month {
    pub fn new(year: i32, month: u32) -> Self {
        assert!((1..=12).contains(&month), "month out of range: {month}");
        Month {
            index: year * 12 + month as i32 - 1,
        }
    }

    pub fn year(self) -> i32 {
        self.index.div_euclid(12)
    }

    pub fn month(self) -> u32 {
        (self.index.rem_euclid(12) + 1) as u32
    }

    pub fn plus(self, months: i32) -> Self {
        Month {
            index: self.index + months,
        }
    }

    /// Whole months from `self` to `later` (negative if `later` is earlier).
    pub fn until(self, later: Month) -> i32 {
        later.index - self.index
    }

    /// The current calendar month in UTC, from the system clock.
    pub fn current_utc() -> Self {
        let secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        let (y, m) = civil_from_days(secs.div_euclid(86_400));
        Month::new(y, m)
    }

    /// Inclusive range.
    pub fn range(start: Month, end: Month) -> Vec<Month> {
        (start.index..=end.index)
            .map(|index| Month { index })
            .collect()
    }
}

/// Year and month of a day count since 1970-01-01 (H. Hinnant's civil-from-days).
fn civil_from_days(z: i64) -> (i32, u32) {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y as i32, m as u32)
}

impl fmt::Display for Month {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:04}-{:02}", self.year(), self.month())
    }
}

impl FromStr for Month {
    type Err = anyhow::Error;

    fn from_str(text: &str) -> anyhow::Result<Self> {
        let text = text.trim();
        let (year, month) = text
            .split_once('-')
            .or_else(|| text.split_once('M'))
            .with_context(|| format!("not a month: {text:?}"))?;
        let year: i32 = year
            .parse()
            .with_context(|| format!("bad year in {text:?}"))?;
        let month: u32 = month
            .parse()
            .with_context(|| format!("bad month in {text:?}"))?;
        if !(1..=12).contains(&month) {
            bail!("month out of range in {text:?}");
        }
        Ok(Month::new(year, month))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_both_formats_and_counts() {
        let a: Month = "2021-01".parse().unwrap();
        let b: Month = "2026M09".parse().unwrap();
        assert_eq!(a.until(b), 68);
        assert_eq!(a.plus(-1).to_string(), "2020-12");
        assert_eq!(Month::range(a, a.plus(2)).len(), 3);
        assert!("2021-13".parse::<Month>().is_err());
        assert!("2021Q1".parse::<Month>().is_err());
        assert_eq!(civil_from_days(0), (1970, 1));
        assert_eq!(civil_from_days(20_406), (2025, 11)); // 2025-11-14
        assert_eq!(civil_from_days(11_016), (2000, 2)); // 2000-02-29
    }
}
