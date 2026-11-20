# Research database

One SQLite file that holds the frozen stablecoin supply and mint/burn panels, so they can be queried together.

The file is **derived**. The panel CSVs and their manifests are the source of truth; the database is rebuilt from them and never edited.

## What is committed

| Committed | Not committed |
|---|---|
| `schema.sql`: tables, keys, units | `stablecoin_lab.sqlite` (ignored by git) |
| `scripts/build_stablecoin_db.py`: the loader and its checks | raw RPC and API responses |
| This README | panel CSVs, until they have been reviewed |

## Rebuild

```bash
python3 scripts/build_stablecoin_db.py \
  --panels-dir <dir with the panel CSVs> \
  --manifests-dir <dir with the manifest JSON files> \
  --status provisional
```

The build stops, and leaves any previous file untouched, when:

- a panel is missing, or two files match one panel;
- a CSV header differs from `schema.sql`;
- a key repeats (the key columns are the declared grain);
- an `INTEGER` column holds a fraction, text, or a value outside int64;
- `--status frozen` is used and a panel's SHA-256 is not found in any manifest.

## Status

Every table is recorded in the `dataset` table with a status.

- `provisional`: the panel may still change (window extended, a definition fixed). Do not cite its numbers.
- `frozen`: the panel passed review and its hash is in a manifest. Only then may a number from it be cited. The build enforces only the hash condition; review is a human decision, so do not pass `--status frozen` before it.

`sha256_in_manifest` records whether the hash of the CSV bytes appears anywhere in a manifest JSON. It shows the panel is the one the manifest describes; it does not show the panel is correct.

## Units

Units are part of the column contract; `schema.sql` lists them.

| Columns | Unit |
|---|---|
| `*_base`, `*_base_units`, `issue_or_mint`, `redeem_or_burn`, `black_funds_destroyed` | integer, token base units (6 decimals) |
| `*_musd` | real, million USD |
| `circulating_supply` | integer, whole USD |
| `dl_*` | real, USD |

The `dl_*` columns come from DefiLlama and are a different quantity from the on-chain `total_supply_end_base`. They sit side by side so the gap stays visible; do not merge them into one series.

## Boundary

Rows here are daily or weekly aggregates and published panels. Event-level chain data (one row per log) belongs in a dataset store with its own coverage record, not in this file.
