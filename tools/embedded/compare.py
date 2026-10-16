#!/usr/bin/env python3
"""Compare a closed-loop run of generated controller code (daedalus hil CSV) with the
model-in-the-loop simulation of the same loop (daedalus simulate CSV).

usage: compare.py MIL.csv HIL.csv --map hil_col=mil_col [--map ...] [--tol 1e-3] [--rel]

At every controller sample the MIL value is taken right after the sample (the last row
at that time: the value after the event), or interpolated between rows. The tolerance
applies to max |hil - mil| / max(1, max |mil|) with --rel, else to the absolute error.
"""
import argparse
import bisect
import csv
import sys


def read(path):
    with open(path) as f:
        r = csv.reader(f)
        head = next(r)
        cols = {h: [] for h in head}
        for row in r:
            for h, v in zip(head, row):
                cols[h].append(float(v))
    return cols


def at(times, values, t):
    """Value right after t (last row at t), else linear interpolation."""
    i = bisect.bisect_right(times, t + 1e-9) - 1
    if i >= 0 and abs(times[i] - t) <= 1e-9:
        return values[i]
    i = max(0, min(len(times) - 2, bisect.bisect_right(times, t) - 1))
    t0, t1 = times[i], times[i + 1]
    w = 0.0 if t1 == t0 else (t - t0) / (t1 - t0)
    return values[i] + w * (values[i + 1] - values[i])


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("mil")
    ap.add_argument("hil")
    ap.add_argument("--map", action="append", default=[], help="hil_column=mil_column")
    ap.add_argument("--tol", type=float, default=1e-3)
    ap.add_argument("--rel", action="store_true", help="error relative to the signal's range")
    ap.add_argument("--skip", type=float, default=0.0, help="ignore samples before this time")
    a = ap.parse_args()
    mil, hil = read(a.mil), read(a.hil)
    ok = True
    for m in a.map:
        hc, mc = m.split("=", 1)
        if hc not in hil or mc not in mil:
            print(f"missing column: {hc if hc not in hil else mc}")
            return 2
        worst, wt = 0.0, 0.0
        scale = max(1.0, max(abs(v) for v in mil[mc])) if a.rel else 1.0
        for t, v in zip(hil["time"], hil[hc]):
            if t < a.skip:
                continue
            e = abs(v - at(mil["time"], mil[mc], t)) / scale
            if e > worst:
                worst, wt = e, t
        flag = "ok" if worst <= a.tol else "FAIL"
        ok &= worst <= a.tol
        kind = "relative" if a.rel else "absolute"
        print(f"{hc} vs {mc}: max {kind} error {worst:.3e} at t = {wt:.4f} ({flag}, tolerance {a.tol:g})")
    if "cycles" in hil:
        c = [x for x in hil["cycles"][1:] if x > 0]
        if c:
            print(f"controller step: mean {sum(c)/len(c):.0f} cycles, max {max(c):.0f}")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
