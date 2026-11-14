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

    /// Inclusive range.
    pub fn range(start: Month, end: Month) -> Vec<Month> {
        (start.index..=end.index)
            .map(|index| Month { index })
            .collect()
    }
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
    }
}
