//! Purchasing power of a stablecoin for a holder who spends in local currency.
//!
//! For holder country *i* keeping a stablecoin pegged to currency *c*, base month 0:
//!
//! ```text
//! PP[i,c,t] = (LCU_i per c)[t] / (LCU_i per c)[0] × CPI_i[0] / CPI_i[t]
//! LCU_i per c = (LCU_i per USD) / (c per USD)
//! ```
//!
//! Holding local money (cash, or a local-currency stablecoin at par) is `CPI_i[0] / CPI_i[t]`.
//! Peg deviations are ignored.

use std::collections::{BTreeMap, HashMap};

use serde::Serialize;

use crate::config;
use crate::fill::{Filled, fill};
use crate::month::Month;
use crate::sources::Panel;

#[derive(Serialize, Debug)]
pub struct PegInfo {
    pub label: String,
    pub supply_usd: f64,
}

#[derive(Serialize, Debug)]
pub struct CountryRow {
    pub iso3: String,
    pub source: String,
    pub caveat: String,
    pub local: Vec<Option<f64>>,
    pub estimated: Vec<bool>,
    pub pp: BTreeMap<String, Vec<Option<f64>>>,
}

#[derive(Serialize, Debug)]
pub struct AssetInfo {
    pub label: String,
    /// USD price index, base month = 1.00. Purchasing power for holder country i is
    /// `index × pp["USD"]` of that country: the crypto move times the stablecoin layer.
    pub index: Vec<Option<f64>>,
}

#[derive(Serialize, Debug)]
pub struct MapData {
    pub months: Vec<String>,
    pub pegs: BTreeMap<String, PegInfo>,
    pub assets: BTreeMap<String, AssetInfo>,
    pub countries: BTreeMap<String, CountryRow>,
    pub excluded: BTreeMap<String, String>,
}

pub struct Inputs<'a> {
    pub bis_cpi: &'a Panel,
    pub wb_cpi: &'a Panel,
    pub fx: &'a Panel,
    pub supply: &'a BTreeMap<String, f64>,
    /// Asset code → monthly USD price.
    pub assets: &'a BTreeMap<String, crate::fill::Series>,
    pub iso2_to_3: &'a HashMap<String, String>,
}

/// Which CPI series a country uses: BIS when it covers the window (to within a few months),
/// otherwise the World Bank.
pub fn choose_cpi<'a>(
    area: &str,
    inputs: &'a Inputs,
    start: Month,
    end: Month,
) -> Option<(&'a crate::fill::Series, &'static str)> {
    if let Some(s) = inputs.bis_cpi.get(area)
        && let (Some((first, _)), Some((last, _))) = (s.first_key_value(), s.last_key_value())
        && *first <= start
        && last.until(end) <= config::BIS_MAX_LAG_MONTHS
    {
        return Some((s, "BIS"));
    }
    inputs
        .wb_cpi
        .get(area)
        .filter(|s| !s.is_empty())
        .map(|s| (s, "World Bank"))
}

fn ratio_index(num: &[Option<f64>], den: &[Option<f64>]) -> Vec<Option<f64>> {
    // num/den re-based to the first month.
    let base = match (num[0], den[0]) {
        (Some(a), Some(b)) => a / b,
        _ => return vec![None; num.len()],
    };
    num.iter()
        .zip(den)
        .map(|(a, b)| Some(a.as_ref()? / b.as_ref()? / base))
        .collect()
}

fn round4(v: Vec<Option<f64>>) -> Vec<Option<f64>> {
    v.into_iter()
        .map(|x| x.map(|y| (y * 1e4).round() / 1e4))
        .collect()
}

pub fn build(inputs: &Inputs, start: Month, end: Month) -> MapData {
    let months = Month::range(start, end);
    let fill_ =
        |s: &crate::fill::Series| fill(s, &months, config::MAX_TRAILING_FILL, config::DRIFT_WINDOW);
    let ones = Filled {
        values: vec![Some(1.0); months.len()],
        estimated: vec![false; months.len()],
    };

    let mut pegs = BTreeMap::new();
    let mut peg_fx: BTreeMap<&str, Vec<Option<f64>>> = BTreeMap::new();
    for &(code, label, peg_type, area) in config::PEGS {
        pegs.insert(
            code.to_string(),
            PegInfo {
                label: label.into(),
                supply_usd: *inputs.supply.get(peg_type).unwrap_or(&0.0),
            },
        );
        let fx = if code == "USD" {
            ones.values.clone()
        } else {
            inputs
                .fx
                .get(area)
                .map(|s| fill_(s).values)
                .unwrap_or_else(|| vec![None; months.len()])
        };
        peg_fx.insert(code, fx);
    }

    let mut areas: Vec<&String> = inputs.bis_cpi.keys().chain(inputs.wb_cpi.keys()).collect();
    areas.sort();
    areas.dedup();

    let mut countries = BTreeMap::new();
    for area in areas {
        if config::BIS_AGGREGATES.contains(&area.as_str())
            || config::lookup(config::EXCLUDED, area).is_some()
        {
            continue;
        }
        let Some(iso3) = inputs.iso2_to_3.get(area.as_str()) else {
            continue;
        };
        let Some((cpi_series, source)) = choose_cpi(area, inputs, start, end) else {
            continue;
        };
        let cpi = fill_(cpi_series);
        let fx_area = if config::EURO_MEMBERS.contains(&area.as_str()) {
            "XM"
        } else {
            area.as_str()
        };
        let lcu = if config::DOLLARIZED.contains(&area.as_str()) {
            ones.clone()
        } else if let Some(s) = inputs.fx.get(fx_area) {
            fill_(s)
        } else {
            continue;
        };
        if cpi.values[0].is_none() || lcu.values[0].is_none() {
            continue; // no base month: cannot index
        }
        let ones_vec = vec![Some(1.0); months.len()];
        let local = ratio_index(&ones_vec, &cpi.values); // CPI0 / CPI_t
        let mut pp = BTreeMap::new();
        for &(code, ..) in config::PEGS {
            let per_c = ratio_index(&lcu.values, &peg_fx[code]);
            let v: Vec<Option<f64>> = per_c
                .iter()
                .zip(&local)
                .map(|(a, b)| Some(a.as_ref()? * b.as_ref()?))
                .collect();
            pp.insert(code.to_string(), round4(v));
        }
        let estimated = cpi
            .estimated
            .iter()
            .zip(&lcu.estimated)
            .map(|(a, b)| *a || *b)
            .collect();
        countries.insert(
            area.clone(),
            CountryRow {
                iso3: iso3.clone(),
                source: source.into(),
                caveat: config::lookup(config::MULTI_RATE, area)
                    .unwrap_or("")
                    .into(),
                local: round4(local),
                estimated,
                pp,
            },
        );
    }
    let mut assets = BTreeMap::new();
    for &(code, label, _) in config::ASSETS {
        let Some(series) = inputs.assets.get(code) else {
            continue;
        };
        let prices = fill_(series).values;
        let ones_vec = vec![Some(1.0); months.len()];
        let index = ratio_index(&prices, &ones_vec);
        assets.insert(
            code.to_string(),
            AssetInfo {
                label: label.into(),
                index: round4(index),
            },
        );
    }
    MapData {
        months: months.iter().map(Month::to_string).collect(),
        pegs,
        assets,
        countries,
        excluded: config::EXCLUDED
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fill::Series;

    fn flat(start: Month, values: &[f64]) -> Series {
        values
            .iter()
            .enumerate()
            .map(|(i, v)| (start.plus(i as i32), *v))
            .collect()
    }

    #[test]
    fn purchasing_power_matches_the_formula() {
        let s = Month::new(2021, 1);
        let mut bis = Panel::new();
        bis.insert("US".into(), flat(s, &[100.0, 110.0]));
        bis.insert("TR".into(), flat(s, &[100.0, 200.0])); // prices double
        let mut fx = Panel::new();
        fx.insert("TR".into(), flat(s, &[8.0, 24.0])); // lira per USD triples
        fx.insert("XM".into(), flat(s, &[0.8, 0.8]));
        let iso: HashMap<String, String> = [("US", "USA"), ("TR", "TUR")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let supply = BTreeMap::new();
        let mut assets = BTreeMap::new();
        assets.insert("BTC".to_string(), flat(s, &[100.0, 200.0]));
        let wb = Panel::new();
        let inputs = Inputs {
            bis_cpi: &bis,
            wb_cpi: &wb,
            fx: &fx,
            supply: &supply,
            assets: &assets,
            iso2_to_3: &iso,
        };
        let d = build(&inputs, s, s.plus(1));
        let tr = &d.countries["TR"];
        assert_eq!(d.assets["BTC"].index, vec![Some(1.0), Some(2.0)]);
        assert_eq!(tr.local[1], Some(0.5));
        assert_eq!(tr.pp["USD"][1], Some(1.5)); // 3x FX / 2x prices
        assert_eq!(
            d.countries["US"].pp["USD"][1],
            Some((1.0f64 / 1.1 * 1e4).round() / 1e4)
        );
        // EUR stablecoin for a Turkish holder: lira per euro triples too (euro flat vs USD).
        assert_eq!(tr.pp["EUR"][1], Some(1.5));
    }

    #[test]
    fn excluded_and_aggregate_areas_are_left_out() {
        let s = Month::new(2021, 1);
        let mut bis = Panel::new();
        for a in ["LB", "XM", "US"] {
            bis.insert(a.into(), flat(s, &[100.0, 101.0]));
        }
        let fx = Panel::new();
        let iso: HashMap<String, String> = [("LB", "LBN"), ("XM", "EMU"), ("US", "USA")]
            .iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect();
        let supply = BTreeMap::new();
        let assets = BTreeMap::new();
        let wb = Panel::new();
        let d = build(
            &Inputs {
                bis_cpi: &bis,
                wb_cpi: &wb,
                fx: &fx,
                supply: &supply,
                assets: &assets,
                iso2_to_3: &iso,
            },
            s,
            s.plus(1),
        );
        assert_eq!(d.countries.keys().cloned().collect::<Vec<_>>(), vec!["US"]);
        assert!(d.excluded.contains_key("LB"));
    }
}
