"""Markdown summary of what changed between two map_data.json files (standard library only).

Usage: python3 crates/value/python/diff_summary.py OLD.json NEW.json
Lists months added or dropped, countries added or dropped, past values revised by more
than 0.01, and the latest month's USD stablecoin vs. local money for the largest moves.
"""
import json
import sys

REVISION = 0.01


def latest(series):
    return next(((i, v) for i, v in reversed(list(enumerate(series))) if v is not None), (None, None))


def main(old_path, new_path):
    old, new = (json.load(open(p)) for p in (old_path, new_path))
    out = []
    om, nm = old["months"], new["months"]
    added = [m for m in nm if m not in om]
    dropped = [m for m in om if m not in nm]
    out.append(f"Window: {om[0]}..{om[-1]} -> {nm[0]}..{nm[-1]}")
    if added:
        out.append(f"Months added: {', '.join(added)}")
    if dropped:
        out.append(f"Months dropped: {', '.join(dropped)}")
    oc, nc = set(old["countries"]), set(new["countries"])
    if nc - oc:
        out.append(f"Countries added: {', '.join(sorted(nc - oc))}")
    if oc - nc:
        out.append(f"Countries dropped: {', '.join(sorted(oc - nc))}")

    revised = []
    for area in sorted(oc & nc):
        o, n = old["countries"][area], new["countries"][area]
        worst = 0.0
        for i, month in enumerate(om):
            if month not in nm:
                continue
            j = nm.index(month)
            for peg in n["pp"]:
                a, b = o["pp"].get(peg, [None] * len(om))[i], n["pp"][peg][j]
                if a is not None and b is not None:
                    worst = max(worst, abs(a - b))
        if worst > REVISION:
            revised.append((worst, area))
    if revised:
        top = ", ".join(f"{a} ({w:.3f})" for w, a in sorted(revised, reverse=True)[:12])
        out.append(f"Past values revised by more than {REVISION} in {len(revised)} countries: {top}")
    else:
        out.append(f"No past value moved by more than {REVISION}.")

    rows = []
    for area, r in new["countries"].items():
        i, pp = latest(r["pp"]["USD"])
        if i is None or r["local"][i] is None:
            continue
        rows.append((pp / r["local"][i], area, nm[i], pp, r["local"][i]))
    rows.sort(reverse=True)
    beat = sum(1 for x in rows if x[0] > 1)
    out.append(f"USD stablecoin beat local money in {beat} of {len(rows)} countries at each one's latest month.")
    out.append("")
    out.append("| Country | Month | USD stablecoin | Local money | Ratio |")
    out.append("|---|---|---|---|---|")
    for ratio, area, month, pp, loc in rows[:5] + rows[-5:]:
        out.append(f"| {area} | {month} | {pp:.2f} | {loc:.2f} | {ratio:.2f} |")
    print("\n".join(f"- {line}" if line and not line.startswith("|") else line for line in out))


if __name__ == "__main__":
    main(*sys.argv[1:3])
