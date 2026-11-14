"""Static scatter: holding local currency vs holding a stablecoin, one month, one peg.

Usage: python3 python/plot_scatter.py data/out/purchasing_power.csv --month 2026-09 [--peg USD] --out scatter.png
Needs pandas and matplotlib.
"""
import argparse

import matplotlib

matplotlib.use("Agg")
import matplotlib.pyplot as plt
import numpy as np
import pandas as pd

BG, INK, MUTED, GRID, REF = "#F7F6F3", "#2B2B2B", "#5F6368", "#D9D7D1", "#7A7A7A"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("csv")
    ap.add_argument("--month", default="2026-09")
    ap.add_argument("--peg", default="USD")
    ap.add_argument("--label", nargs="*", default=["TR", "AR", "EG", "NG", "JP", "IN", "US", "BR", "MX", "ID", "KR", "CN"])
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    d = pd.read_csv(a.csv, keep_default_na=False, na_values=[""])
    d = d[(d.month == a.month) & (d.peg == a.peg)].dropna(subset=["pp", "local"])
    plt.rcParams.update({"font.family": ["Inter", "Source Sans 3", "Arial", "DejaVu Sans"], "figure.facecolor": BG,
                         "axes.facecolor": BG, "axes.edgecolor": MUTED, "axes.spines.top": False, "axes.spines.right": False})
    fig, ax = plt.subplots(figsize=(10.67, 7.2), dpi=150)
    ax.set_xscale("log"), ax.set_yscale("log")
    lim = (min(d.local.min(), d.pp.min()) * 0.9, max(d.local.max(), d.pp.max()) * 1.1)
    ax.plot(lim, lim, color=REF, lw=1, ls="--")
    ax.scatter(d.local, d.pp, s=36, color=INK, alpha=0.75)
    for _, r in d[d.area.isin(a.label)].iterrows():
        ax.annotate(r.area, (r.local, r.pp), xytext=(5, 3), textcoords="offset points", fontsize=12, color=INK)
    ax.set_xlim(lim), ax.set_ylim(lim)
    ticks = [t for t in [0.03, 0.1, 0.2, 0.5, 1, 1.5, 2] if lim[0] <= t <= lim[1]]
    for axis in (ax.xaxis, ax.yaxis):
        axis.set_major_locator(matplotlib.ticker.FixedLocator(ticks))
        axis.set_major_formatter(matplotlib.ticker.FormatStrFormatter("%g"))
        axis.set_minor_locator(matplotlib.ticker.NullLocator())
    ax.grid(color=GRID, lw=0.8)
    ax.tick_params(labelsize=13, colors=MUTED)
    ax.set_xlabel(f"Holding local currency: purchasing power, Jan 2021 = 1 (log)", fontsize=15, color=INK)
    ax.set_ylabel(f"Holding a {a.peg} stablecoin (log)", fontsize=15, color=INK)
    above = int((d.pp > d.local).sum())
    fig.suptitle(f"{a.peg} stablecoin kept more value than local currency in {above} of {len(d)} countries",
                 fontsize=20, x=0.06, ha="left", color=INK)
    fig.tight_layout()
    fig.savefig(a.out)
    print(f"{a.out}: {len(d)} countries, {above} above the line")


if __name__ == "__main__":
    main()
