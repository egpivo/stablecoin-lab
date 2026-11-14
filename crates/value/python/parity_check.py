"""Compare two map_data.json files value by value (standard library only).

Usage: python3 python/parity_check.py NEW.json REFERENCE.json [--tol 1e-3]
Exit 0 when every shared country, peg and month agrees within the tolerance and
both files list the same countries.
"""
import argparse
import json
import sys


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("new")
    ap.add_argument("ref")
    ap.add_argument("--tol", type=float, default=1e-3)
    a = ap.parse_args()
    new, ref = (json.load(open(p)) for p in (a.new, a.ref))
    ok = new["months"] == ref["months"]
    if not ok:
        print("months differ")
    only_new = sorted(set(new["countries"]) - set(ref["countries"]))
    only_ref = sorted(set(ref["countries"]) - set(new["countries"]))
    if only_new or only_ref:
        ok = False
        print("only in new:", only_new, "only in reference:", only_ref)
    worst, cells = (0.0, None), 0
    for area in sorted(set(new["countries"]) & set(ref["countries"])):
        n, r = new["countries"][area], ref["countries"][area]
        pairs = [("local", n["local"], r["local"])] + [(p, n["pp"][p], r["pp"][p]) for p in r["pp"]]
        for name, xs, ys in pairs:
            for i, (x, y) in enumerate(zip(xs, ys)):
                cells += 1
                if (x is None) != (y is None):
                    ok = False
                    print(f"{area} {name} {new['months'][i]}: {x} vs {y}")
                elif x is not None and abs(x - y) > worst[0]:
                    worst = (abs(x - y), f"{area} {name} {new['months'][i]}")
        if n["estimated"] != r["estimated"]:
            ok = False
            print(f"{area}: estimated flags differ")
    print(f"{cells} cells compared; largest difference {worst[0]:.2e} at {worst[1]}")
    if worst[0] > a.tol:
        ok = False
    print("PARITY OK" if ok else "PARITY FAILED")
    sys.exit(0 if ok else 1)


if __name__ == "__main__":
    main()
