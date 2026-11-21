"""Tests for the stablecoin research database build (stdlib unittest, no network)."""
from __future__ import annotations

import hashlib
import json
import sqlite3
import sys
import tempfile
import unittest
from pathlib import Path
from unittest import mock

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "scripts"))

import build_stablecoin_db as b  # noqa: E402

# table -> (CSV header, one valid row). The headers double as a schema-drift guard.
SAMPLE = {
    "issuer_daily_supply": ("date,issuer,circulating_supply", ["2025-01-01,USDT,1000"]),
    "market_daily": ("date,series,value", ["2025-01-01,BTCUSDT_CLOSE,100.5"]),
    "tron_usdt_daily_issue_redeem": (
        "date,issuer,chain,issue_base_units,redeem_base_units,issue_count,redeem_count",
        ["2025-01-01,USDT,Tron,0,0,0,0"],
    ),
    "eth_events_daily": ("date,issuer,chain,measure,base_units,count", ["2025-01-01,USDT,Ethereum,issue,0,0"]),
    "eth_supply_check": (
        "date,issuer,total_supply_end_base,delta_base,net_events_base,residual_base,"
        "dl_eth_circulating_next_midnight,dl_eth_minted_next_midnight",
        ["2025-01-01,USDT,5,0,0,0,64.5,5.25"],
    ),
    "eth_daily_residual": (
        "date,issuer,supply_delta_base,issue_or_mint,redeem_or_burn,black_funds_destroyed,residual_base",
        ["2025-01-01,USDT,0,0,0,0,0"],
    ),
    "weekly_mint_burn": (
        "week_start_monday,series,gross_mint_musd,gross_burn_musd,net_musd,black_funds_destroyed_musd,"
        "btc_close_week_last_day,days_in_week,partial_week,partial_window",
        ["2025-01-06,USDT Ethereum,0.0,0.0,0.0,0.0,94545.06,7,0,1"],
    ),
    "cross_chain_candidates": (
        "issuer,direction,burn_chain,burn_date,burn_tx,mint_chain,mint_date,mint_tx,amount_base_units,ambiguous",
        [],
    ),
}


def write_panel(directory: Path, table: str, rows: list[str] | None = None, header: str | None = None) -> Path:
    sample_header, sample_rows = SAMPLE[table]
    path = directory / f"{table}_2025-01-01_2025-01-07.csv"
    lines = [header or sample_header] + (sample_rows if rows is None else rows)
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")
    return path


class BuildTests(unittest.TestCase):
    def setUp(self) -> None:
        tmp = tempfile.TemporaryDirectory()
        self.addCleanup(tmp.cleanup)
        self.root = Path(tmp.name)
        self.panels = self.root / "processed"
        self.manifests = self.root / "manifests"
        self.panels.mkdir()
        self.manifests.mkdir()
        self.out = self.root / "out.sqlite"
        for table in SAMPLE:
            write_panel(self.panels, table)

    def build(self, status: str = "provisional") -> list:
        return b.build(self.panels, self.manifests, self.out, status)

    def write_manifest(self, tables: list[str]) -> None:
        hashes = {t: hashlib.sha256(next(self.panels.glob(f"{t}_*.csv")).read_bytes()).hexdigest() for t in tables}
        (self.manifests / "run.json").write_text(json.dumps({"panels": {"nested": hashes}}), encoding="utf-8")

    def test_loads_every_panel_with_matching_counts(self) -> None:
        self.build()
        db = sqlite3.connect(self.out)
        rows = db.execute("SELECT table_name, csv_rows, db_rows, status FROM dataset").fetchall()
        self.assertEqual(len(rows), len(SAMPLE))
        for _name, csv_rows, db_rows, status in rows:
            self.assertEqual(csv_rows, db_rows)
            self.assertEqual(status, "provisional")
        self.assertEqual(db.execute("SELECT dl_eth_circulating_next_midnight FROM eth_supply_check").fetchone()[0], 64.5)

    def test_header_drift_fails_and_names_the_table(self) -> None:
        write_panel(self.panels, "market_daily", header="date,series,value,extra", rows=["2025-01-01,BTCUSDT_CLOSE,1,x"])
        with self.assertRaisesRegex(b.BuildError, "market_daily"):
            self.build()

    def test_duplicate_key_fails(self) -> None:
        write_panel(self.panels, "issuer_daily_supply", rows=["2025-01-01,USDT,1", "2025-01-01,USDT,2"])
        with self.assertRaisesRegex(b.BuildError, "issuer_daily_supply"):
            self.build()

    def test_missing_key_value_fails(self) -> None:
        write_panel(self.panels, "issuer_daily_supply", rows=[",USDT,1"])
        with self.assertRaises(b.BuildError):
            self.build()

    def test_fractional_value_in_integer_column_fails(self) -> None:
        write_panel(self.panels, "eth_events_daily", rows=["2025-01-01,USDT,Ethereum,issue,1.5,0"])
        with self.assertRaisesRegex(b.BuildError, "eth_events_daily.base_units"):
            self.build()

    def test_value_beyond_int64_fails(self) -> None:
        write_panel(self.panels, "eth_events_daily", rows=["2025-01-01,USDT,Ethereum,issue,10000000000000000000,0"])
        with self.assertRaisesRegex(b.BuildError, "base_units"):
            self.build()

    def test_non_numeric_value_in_real_column_fails(self) -> None:
        write_panel(self.panels, "market_daily", rows=["2025-01-01,BTCUSDT_CLOSE,abc"])
        with self.assertRaisesRegex(b.BuildError, "market_daily.value"):
            self.build()

    def test_missing_panel_fails(self) -> None:
        next(self.panels.glob("weekly_mint_burn_*.csv")).unlink()
        with self.assertRaisesRegex(b.BuildError, "weekly_mint_burn"):
            self.build()

    def test_two_files_for_one_panel_fail(self) -> None:
        (self.panels / "market_daily_other.csv").write_text("date,series,value\n", encoding="utf-8")
        with self.assertRaisesRegex(b.BuildError, "market_daily"):
            self.build()

    def test_manifest_hash_is_recorded_per_panel(self) -> None:
        self.write_manifest(["issuer_daily_supply"])
        self.build()
        db = sqlite3.connect(self.out)
        found = dict(db.execute("SELECT table_name, sha256_in_manifest FROM dataset"))
        self.assertEqual(found["issuer_daily_supply"], 1)
        self.assertEqual(found["market_daily"], 0)

    def test_frozen_needs_every_hash_in_a_manifest(self) -> None:
        self.write_manifest(["issuer_daily_supply"])
        with self.assertRaisesRegex(b.BuildError, "market_daily"):
            self.build("frozen")
        self.assertFalse(self.out.exists())
        self.write_manifest(list(SAMPLE))
        self.build("frozen")
        db = sqlite3.connect(self.out)
        self.assertEqual({s for (s,) in db.execute("SELECT status FROM dataset")}, {"frozen"})

    def test_failed_build_keeps_the_previous_file_and_leaves_no_temp(self) -> None:
        self.out.write_bytes(b"previous")
        write_panel(self.panels, "market_daily", rows=["2025-01-01,BTCUSDT_CLOSE,abc"])
        with self.assertRaises(b.BuildError):
            self.build()
        self.assertEqual(self.out.read_bytes(), b"previous")
        self.assertEqual(list(self.root.glob("*.tmp")), [])

    def test_unknown_status_is_rejected(self) -> None:
        with self.assertRaises(b.BuildError):
            self.build("final")

    def test_out_that_is_a_panel_is_refused_and_left_intact(self) -> None:
        panel = next(self.panels.glob("market_daily_*.csv"))
        before = panel.read_bytes()
        with self.assertRaisesRegex(b.BuildError, "input file"):
            b.build(self.panels, self.manifests, panel, "provisional")
        self.assertEqual(panel.read_bytes(), before)

    def test_out_that_is_the_schema_is_refused_and_left_intact(self) -> None:
        schema = self.root / "schema.sql"
        schema.write_text(b.SCHEMA.read_text(encoding="utf-8"), encoding="utf-8")
        before = schema.read_bytes()
        with mock.patch.object(b, "SCHEMA", schema):
            with self.assertRaisesRegex(b.BuildError, "input file"):
                b.build(self.panels, self.manifests, schema, "provisional")
        self.assertEqual(schema.read_bytes(), before)

    def test_uppercase_manifest_hash_counts(self) -> None:
        digest = hashlib.sha256(next(self.panels.glob("issuer_daily_supply_*.csv")).read_bytes()).hexdigest()
        (self.manifests / "run.json").write_text(json.dumps({"sha": digest.upper()}), encoding="utf-8")
        self.build()
        db = sqlite3.connect(self.out)
        self.assertEqual(db.execute("SELECT sha256_in_manifest FROM dataset WHERE table_name='issuer_daily_supply'").fetchone()[0], 1)

    def test_hash_inside_a_longer_hex_run_does_not_count(self) -> None:
        digest = hashlib.sha256(next(self.panels.glob("issuer_daily_supply_*.csv")).read_bytes()).hexdigest()
        (self.manifests / "run.json").write_text(json.dumps({"sha": "a" + digest}), encoding="utf-8")
        self.build()
        db = sqlite3.connect(self.out)
        self.assertEqual(db.execute("SELECT sha256_in_manifest FROM dataset WHERE table_name='issuer_daily_supply'").fetchone()[0], 0)

    def test_non_utf8_panel_fails_with_a_build_error(self) -> None:
        next(self.panels.glob("market_daily_*.csv")).write_bytes(b"date,series,value\n\xff\xfe,x,1\n")
        with self.assertRaisesRegex(b.BuildError, "UTF-8"):
            self.build()

    def test_date_must_be_iso(self) -> None:
        write_panel(self.panels, "market_daily", rows=["2025/01/01,BTCUSDT_CLOSE,1"])
        with self.assertRaisesRegex(b.BuildError, "market_daily"):
            self.build()

    def test_week_flag_and_day_count_are_constrained(self) -> None:
        for bad in ("2025-01-06,USDT Ethereum,0,0,0,0,1,7,2,1", "2025-01-06,USDT Ethereum,0,0,0,0,1,9,0,1"):
            write_panel(self.panels, "weekly_mint_burn", rows=[bad])
            with self.assertRaisesRegex(b.BuildError, "weekly_mint_burn"):
                self.build()

    def test_same_tx_hash_on_two_chains_is_two_rows(self) -> None:
        rows = [
            "USDC,burn,Ethereum,2025-01-01,0xabc,Base,2025-01-01,0xdef,5,0",
            "USDC,burn,Arbitrum,2025-01-01,0xabc,Base,2025-01-01,0xdef,5,0",
        ]
        write_panel(self.panels, "cross_chain_candidates", rows=rows)
        self.build()
        db = sqlite3.connect(self.out)
        self.assertEqual(db.execute("SELECT COUNT(*) FROM cross_chain_candidates").fetchone()[0], 2)

    def test_temp_file_does_not_replace_an_unrelated_file(self) -> None:
        user_file = self.root / "out.sqlite.tmp"
        user_file.write_bytes(b"mine")
        self.build()
        self.assertEqual(user_file.read_bytes(), b"mine")


if __name__ == "__main__":
    unittest.main()
