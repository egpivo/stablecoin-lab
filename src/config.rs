//! Fixed choices of the panel. Each one is documented in docs/METHOD.md.

/// Stablecoin peg currencies on the map: (code, label, DefiLlama pegType, BIS area for its FX rate).
pub const PEGS: &[(&str, &str, &str, &str)] = &[
    ("USD", "US dollar", "peggedUSD", "US"),
    ("EUR", "Euro", "peggedEUR", "XM"),
    ("RUB", "Russian ruble", "peggedRUB", "RU"),
    ("JPY", "Japanese yen", "peggedJPY", "JP"),
    ("BRL", "Brazilian real", "peggedREAL", "BR"),
    ("CHF", "Swiss franc", "peggedCHF", "CH"),
    ("GBP", "British pound", "peggedGBP", "GB"),
    ("SGD", "Singapore dollar", "peggedSGD", "SG"),
];

/// Euro members priced in euros for the whole window; their FX is the euro area's (BIS `XM`).
pub const EURO_MEMBERS: &[&str] = &[
    "AT", "BE", "CY", "DE", "EE", "ES", "FI", "FR", "GR", "HR", "IE", "IT", "LT", "LU", "LV", "MT",
    "NL", "PT", "SI", "SK",
];

/// The US dollar is legal tender: local currency per USD is 1.
pub const DOLLARIZED: &[&str] = &["US", "EC", "SV", "PA"];

/// Left out: the official rate in the base month was far from what a holder paid.
pub const EXCLUDED: &[(&str, &str)] = &[(
    "LB",
    "official LBP rate (1,507) was far below the market rate",
)];

/// Kept, with a note: a parallel rate traded above the official one for part of the window.
pub const MULTI_RATE: &[(&str, &str)] = &[
    (
        "AR",
        "official rate; a parallel rate traded higher in 2021-2024",
    ),
    (
        "NG",
        "official rate; a parallel rate traded higher before 2023",
    ),
];

/// ISO3 codes the World Bank country list does not carry.
pub const EXTRA_ISO3: &[(&str, &str)] = &[("TWN", "TW")];

/// BIS areas that are aggregates, not countries.
pub const BIS_AGGREGATES: &[&str] = &["XM", "XW", "4T", "5R", "1X"];

/// A BIS CPI series is preferred when it starts by the base month and ends within this many
/// months of the window end; otherwise the World Bank series is used.
pub const BIS_MAX_LAG_MONTHS: i32 = 3;

/// Trailing gaps up to this many months are extended at recent inflation; longer gaps stay empty.
pub const MAX_TRAILING_FILL: i32 = 3;

/// Months of log changes averaged for a trailing extension.
pub const DRIFT_WINDOW: usize = 6;

pub fn lookup<'a>(table: &'a [(&str, &str)], key: &str) -> Option<&'a str> {
    table.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
}
