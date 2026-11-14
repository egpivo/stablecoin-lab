# Method

## Question

How much does one stablecoin, held since the base month, buy today for a holder who
spends in their own country's currency?

## Measure

For holder country *i* and a stablecoin pegged to currency *c*, base month 0:

```
PP[i,c,t] = (LCU_i per c)[t] / (LCU_i per c)[0] × CPI_i[0] / CPI_i[t]
LCU_i per c = (LCU_i per USD) / (c per USD)
```

Holding local money, meaning local cash or a local-currency stablecoin at par, is
`CPI_i[0] / CPI_i[t]`. The map's second view is the ratio `PP / local`.

Peg deviations are ignored. USDT and USDC monthly means stayed within 0.4% of $1 over
2021-2026; the March 2023 USDC dip is inside one month.

## Crypto assets

BTC, ETH and SOL are quoted in USD stablecoins, so they inherit the stablecoin layer. For a
holder in country *i*:

```
PP_asset[i,t] = index_asset[t] × PP[i,USD,t]
index_asset[t] = VWAP_asset[t] / VWAP_asset[0]
```

`index_asset` is the asset's USD move. `PP[i,USD,t]` is the stablecoin layer, which depends
on where the holder spends. VWAP is the monthly volume-weighted price from Binance spot
monthly klines (quote volume / base volume), to match the monthly-average exchange rates.
The output stores `index_asset` once, in `assets`. Viewers multiply it by each country's
USD stablecoin series.

## Sources

| File | Source | Use |
|---|---|---|
| `bis/cpi_index.csv` | BIS WS_LONG_CPI, unit 628 | CPI, 61 economies |
| `bis/fx_monthly.csv` | BIS WS_XRU, collection A | local currency per USD, monthly average |
| `worldbank/gem_cpi.json` | World Bank GEM `CPTOTNSXN` | CPI for countries BIS does not cover |
| `worldbank/countries.json` | World Bank country list | ISO3 to ISO2 |
| `defillama/stablecoins.json` | DefiLlama | circulating supply by peg currency |
| `binance/{btc,eth,sol}usdt_1M.json` | Binance spot monthly klines | USD price index of each asset |

A country uses BIS CPI when the BIS series starts by the base month and ends within
3 months of the window end. Otherwise it uses World Bank CPI.

## Window end

`build` uses the latest month that at least 80% of countries have published, for both BIS CPI
and BIS FX. A country counts if it reported in the 12 months before that month. Prices and
exchange rates arrive one to two months late, and not all at once. This rule keeps a few
early reporters from setting the end month (`crates/value/src/window.rs`). Pass `--end YYYY-MM`
to fix it.

## Gap rules

- Interior gaps: log-linear interpolation.
- Trailing gap of at most 3 months: extended at the mean of the last 6 monthly log changes.
- Leading gaps and longer trailing gaps stay empty, and the map shows those months in grey.
- No value is borrowed from a neighbouring country.
- Every filled month is flagged `estimated`.

## Country rules

- Euro members use the euro area's exchange rate (BIS `XM`).
- Ecuador, El Salvador, Panama and the US use the US dollar: local currency per USD is 1.
- Lebanon is excluded. Its official rate in January 2021 (1,507 LBP) was far from the
  market rate, so the base would not be what a holder paid.
- Argentina and Nigeria are kept with a caveat. A parallel rate traded above the official
  rate for part of the window, so these values overstate the stablecoin's gain.

## Limits

- Chains and holders are not mapped to countries. The map answers "if you hold here",
  not "who holds".
- A holder who spends on dollar-priced goods has a purchasing power between the US CPI
  and the local CPI measures.
- Official exchange rates only.
