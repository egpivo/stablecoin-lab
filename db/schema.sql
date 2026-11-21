-- stablecoin-lab research database (derived; rebuilt from frozen panel CSVs).
--
-- Units are part of the contract:
--   *_base, *_base_units, issue_or_mint, redeem_or_burn, black_funds_destroyed
--       integer, token base units (6 decimals for USDT and USDC)
--   *_musd           real, million USD
--   circulating_supply   integer, whole USD (DefiLlama circulating, all chains)
--   dl_*             real, USD, DefiLlama figures for one chain
-- Column names equal the panel CSV headers. The build refuses a panel whose header differs.
-- Key columns are the declared grain; a duplicate key fails the build.
-- Dates are ISO YYYY-MM-DD (UTC); any other form fails the build.

CREATE TABLE dataset (
    table_name         TEXT NOT NULL PRIMARY KEY,
    source_file        TEXT NOT NULL,
    csv_rows           INTEGER NOT NULL,
    db_rows            INTEGER NOT NULL,
    sha256             TEXT NOT NULL,         -- of the panel CSV bytes
    sha256_in_manifest INTEGER NOT NULL CHECK (sha256_in_manifest IN (0, 1)),
    key_min            TEXT,                  -- first and last value of the date column
    key_max            TEXT,
    status             TEXT NOT NULL CHECK (status IN ('provisional', 'frozen'))
);

-- One row per UTC day and issuer, summed over all chains.
CREATE TABLE issuer_daily_supply (
    date               TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    issuer             TEXT NOT NULL,
    circulating_supply INTEGER,
    PRIMARY KEY (date, issuer)
);

-- One row per UTC day and series (spot closes and an index level).
CREATE TABLE market_daily (
    date   TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    series TEXT NOT NULL,
    value  REAL,
    PRIMARY KEY (date, series)
);

-- Tron USDT Issue/Redeem, one row per UTC day.
CREATE TABLE tron_usdt_daily_issue_redeem (
    date              TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    issuer            TEXT NOT NULL,
    chain             TEXT NOT NULL,
    issue_base_units  INTEGER,
    redeem_base_units INTEGER,
    issue_count       INTEGER,
    redeem_count      INTEGER,
    PRIMARY KEY (date, issuer, chain)
);

-- Ethereum events in long form: one row per day, issuer, chain and measure.
CREATE TABLE eth_events_daily (
    date       TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    issuer     TEXT NOT NULL,
    chain      TEXT NOT NULL,
    measure    TEXT NOT NULL,
    base_units INTEGER,
    count      INTEGER,
    PRIMARY KEY (date, issuer, chain, measure)
);

-- On-chain totalSupply against the event total, with the DefiLlama figures beside it.
-- dl_* is a different quantity from total_supply_end_base; never treat them as one series.
CREATE TABLE eth_supply_check (
    date                            TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    issuer                          TEXT NOT NULL,
    total_supply_end_base           INTEGER,
    delta_base                      INTEGER,
    net_events_base                 INTEGER,
    residual_base                   INTEGER,
    dl_eth_circulating_next_midnight REAL,
    dl_eth_minted_next_midnight      REAL,
    PRIMARY KEY (date, issuer)
);

-- supply_delta_base = issue_or_mint - redeem_or_burn - black_funds_destroyed; residual_base is what is left.
CREATE TABLE eth_daily_residual (
    date                  TEXT NOT NULL CHECK (date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    issuer                TEXT NOT NULL,
    supply_delta_base     INTEGER,
    issue_or_mint         INTEGER,
    redeem_or_burn        INTEGER,
    black_funds_destroyed INTEGER,
    residual_base         INTEGER,
    PRIMARY KEY (date, issuer)
);

-- Weekly (Monday 00:00 UTC) gross flows per series; a partial week is flagged, not dropped.
CREATE TABLE weekly_mint_burn (
    week_start_monday          TEXT NOT NULL CHECK (week_start_monday GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    series                     TEXT NOT NULL,
    gross_mint_musd            REAL,
    gross_burn_musd            REAL,
    net_musd                   REAL,
    black_funds_destroyed_musd REAL,
    btc_close_week_last_day    REAL,
    days_in_week               INTEGER CHECK (days_in_week BETWEEN 1 AND 7),
    partial_week               INTEGER CHECK (partial_week IN (0, 1)),
    partial_window             INTEGER CHECK (partial_window IN (0, 1)),
    PRIMARY KEY (week_start_monday, series)
);

-- Burn on one chain matched to a mint on another. An empty table means no pair
-- met the matching rule, not that no transfer happened.
CREATE TABLE cross_chain_candidates (
    issuer            TEXT NOT NULL,
    direction         TEXT,
    burn_chain        TEXT NOT NULL,
    burn_date         TEXT CHECK (burn_date IS NULL OR burn_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    burn_tx           TEXT NOT NULL,
    mint_chain        TEXT NOT NULL,
    mint_date         TEXT CHECK (mint_date IS NULL OR mint_date GLOB '[0-9][0-9][0-9][0-9]-[0-9][0-9]-[0-9][0-9]'),
    mint_tx           TEXT NOT NULL,
    amount_base_units INTEGER,
    ambiguous         INTEGER CHECK (ambiguous IN (0, 1)),
    PRIMARY KEY (burn_chain, burn_tx, mint_chain, mint_tx)
);
