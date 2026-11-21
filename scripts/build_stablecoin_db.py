#!/usr/bin/env python3
"""Build db/stablecoin_lab.sqlite from frozen panel CSVs.

The SQLite file is derived: the CSVs and their manifests stay the source of truth,
and the file is rebuilt, never edited. See db/README.md.
"""
from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import os
import re
import sqlite3
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SCHEMA = ROOT / "db" / "schema.sql"
DEFAULT_OUT = ROOT / "db" / "stablecoin_lab.sqlite"

# table -> glob of its panel under --panels-dir; exactly one file must match
PANELS = {
    "issuer_daily_supply": "issuer_daily_supply_*.csv",
    "market_daily": "market_daily_*.csv",
    "tron_usdt_daily_issue_redeem": "tron_usdt_daily_issue_redeem_*.csv",
    "eth_events_daily": "eth_events_daily_*.csv",
    "eth_supply_check": "eth_supply_check_*.csv",
    "eth_daily_residual": "eth_daily_residual_*.csv",
    "weekly_mint_burn": "weekly_mint_burn_*.csv",
    "cross_chain_candidates": "cross_chain_candidates_*.csv",
}
DATE_COLUMNS = ("date", "week_start_monday")
# a complete 64-digit hex token, either case; a longer hex run does not count
SHA256 = re.compile(r"(?<![0-9a-fA-F])[0-9a-fA-F]{64}(?![0-9a-fA-F])")


class BuildError(Exception):
    """The panels do not satisfy the schema or the freeze rules."""


def manifest_hashes(manifests: Path) -> set[str]:
    """Every 64-hex string found in any key or value of any manifest JSON."""
    found: set[str] = set()

    def walk(node: object) -> None:
        if isinstance(node, dict):
            for key, value in node.items():
                walk(key)
                walk(value)
        elif isinstance(node, list):
            for value in node:
                walk(value)
        elif isinstance(node, str):
            found.update(h.lower() for h in SHA256.findall(node))

    for path in sorted(manifests.glob("*.json")):
        try:
            walk(json.loads(path.read_text(encoding="utf-8")))
        except ValueError as exc:
            raise BuildError(f"manifest {path.name} is not valid JSON: {exc}") from exc
    return found


def check_types(db: sqlite3.Connection, table: str) -> None:
    """An INTEGER column must hold integers (no float, no text, no int64 overflow); REAL must hold numbers."""
    for _cid, name, declared, *_rest in db.execute(f'PRAGMA table_info("{table}")').fetchall():
        allowed = {"INTEGER": ("integer",), "REAL": ("real", "integer")}.get(declared)
        if allowed is None:
            continue
        marks = ",".join("?" * len(allowed))
        bad = db.execute(
            f'SELECT COUNT(*) FROM "{table}" WHERE "{name}" IS NOT NULL AND typeof("{name}") NOT IN ({marks})',
            allowed,
        ).fetchone()[0]
        if bad:
            raise BuildError(f"{table}.{name}: {bad} value(s) are not {declared} (check units and int64 range)")


def load_panel(db: sqlite3.Connection, table: str, path: Path, hashes: set[str]) -> tuple:
    raw = path.read_bytes()
    sha = hashlib.sha256(raw).hexdigest()
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError as exc:
        raise BuildError(f"{table}: {path.name} is not valid UTF-8: {exc}") from exc
    rows = list(csv.reader(io.StringIO(text, newline="")))
    if not rows:
        raise BuildError(f"{table}: {path.name} is empty")
    header, body = rows[0], rows[1:]
    expected = [r[1] for r in db.execute(f'PRAGMA table_info("{table}")')]
    if header != expected:
        raise BuildError(f"{table}: CSV header {header} does not match schema {expected}")
    marks = ",".join("?" * len(header))
    try:
        db.executemany(
            f'INSERT INTO "{table}" VALUES ({marks})',
            [[None if v == "" else v for v in r] for r in body],
        )
    except (sqlite3.IntegrityError, sqlite3.ProgrammingError) as exc:
        raise BuildError(f"{table}: {exc}") from exc
    check_types(db, table)
    count = db.execute(f'SELECT COUNT(*) FROM "{table}"').fetchone()[0]
    if count != len(body):
        raise BuildError(f"{table}: loaded {count} rows from {len(body)} CSV rows")
    lo = hi = None
    date_col = next((c for c in DATE_COLUMNS if c in header), None)
    if date_col and count:
        lo, hi = db.execute(f'SELECT MIN("{date_col}"), MAX("{date_col}") FROM "{table}"').fetchone()
    return path.name, len(body), count, sha, int(sha in hashes), lo, hi


def build(panels: Path, manifests: Path, out: Path, status: str) -> list[tuple]:
    if status not in ("provisional", "frozen"):
        raise BuildError(f"status must be provisional or frozen, got {status!r}")
    hashes = manifest_hashes(manifests)
    paths: dict[str, Path] = {}
    for table, pattern in PANELS.items():
        matches = sorted(panels.glob(pattern))
        if len(matches) != 1:
            raise BuildError(f"{table}: expected one file matching {pattern}, found {len(matches)}")
        paths[table] = matches[0]
    inputs = {SCHEMA.resolve(), *(p.resolve() for p in paths.values()), *(p.resolve() for p in manifests.glob("*.json"))}
    if out.resolve() in inputs:
        raise BuildError(f"--out {out} is an input file; refusing to overwrite it")
    out.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp_name = tempfile.mkstemp(prefix=out.name + ".", suffix=".tmp", dir=out.parent)
    os.close(fd)
    tmp = Path(tmp_name)
    db = sqlite3.connect(tmp)
    report: list[tuple] = []
    try:
        db.executescript(SCHEMA.read_text(encoding="utf-8"))
        for table in PANELS:
            row = load_panel(db, table, paths[table], hashes)
            db.execute("INSERT INTO dataset VALUES (?,?,?,?,?,?,?,?,?)", (table, *row[:5], row[5], row[6], status))
            report.append((table, *row))
        if status == "frozen":
            missing = [r[0] for r in report if not r[5]]
            if missing:
                raise BuildError(f"status frozen needs every panel hash in a manifest; missing: {', '.join(missing)}")
        db.commit()
        db.close()
        os.replace(tmp, out)
    except BaseException:
        db.close()
        tmp.unlink(missing_ok=True)
        raise
    return report


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--panels-dir", type=Path, required=True, help="directory with the frozen panel CSVs")
    parser.add_argument("--manifests-dir", type=Path, required=True, help="directory with the manifest JSON files")
    parser.add_argument("--out", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--status", default="provisional", choices=["provisional", "frozen"])
    args = parser.parse_args()
    try:
        report = build(args.panels_dir, args.manifests_dir, args.out, args.status)
    except (BuildError, OSError, UnicodeError, sqlite3.Error) as exc:
        print(f"build failed: {exc}", file=sys.stderr)
        return 1
    print(f"{'table':32} {'rows':>7} {'hash in manifest':>17}  window")
    for table, _file, _csv_rows, rows, _sha, in_manifest, lo, hi in report:
        print(f"{table:32} {rows:>7} {'yes' if in_manifest else 'no':>17}  {lo or '-'} -> {hi or '-'}")
    print(f"wrote {args.out} (status {args.status})")
    return 0


if __name__ == "__main__":
    sys.exit(main())
