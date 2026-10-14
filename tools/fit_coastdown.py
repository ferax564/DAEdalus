#!/usr/bin/env python3
"""Fit dynamometer road-load coefficients F = F0 + F1*v + F2*v^2 to a coast-down result.

Usage:
    daedalus simulate library/Daedalus -m Daedalus.Examples.VehicleEfficiency.CoastDown -o coast.csv
    python3 tools/fit_coastdown.py coast.csv [speed_column] [force_column]

Reads a CSV written by `daedalus simulate` and solves the 3x3 least-squares normal equations,
so it needs nothing beyond the Python standard library.
"""
import csv
import sys


def fit(v, f):
    # normal equations for [1, v, v^2]
    s = [sum(x**k for x in v) for k in range(5)]
    b = [sum(fi * x**k for x, fi in zip(v, f)) for k in range(3)]
    a = [[s[i + j] for j in range(3)] for i in range(3)]
    # Gaussian elimination with partial pivoting
    for k in range(3):
        p = max(range(k, 3), key=lambda i: abs(a[i][k]))
        a[k], a[p] = a[p], a[k]
        b[k], b[p] = b[p], b[k]
        for i in range(k + 1, 3):
            m = a[i][k] / a[k][k]
            for j in range(k, 3):
                a[i][j] -= m * a[k][j]
            b[i] -= m * b[k]
    c = [0.0, 0.0, 0.0]
    for i in reversed(range(3)):
        c[i] = (b[i] - sum(a[i][j] * c[j] for j in range(i + 1, 3))) / a[i][i]
    return c


def main():
    path = sys.argv[1]
    vcol = sys.argv[2] if len(sys.argv) > 2 else "body.v"
    fcol = sys.argv[3] if len(sys.argv) > 3 else "F_decel"
    with open(path, newline="") as fh:
        rows = list(csv.reader(fh))
    head = rows[0]
    iv, iF = head.index(vcol), head.index(fcol)
    pts = [(float(r[iv]), float(r[iF])) for r in rows[1:]]
    pts = [(v, f) for v, f in pts if v > 1.0]
    v = [p[0] for p in pts]
    f = [p[1] for p in pts]
    f0, f1, f2 = fit(v, f)
    rms = (sum((f0 + f1 * x + f2 * x * x - y) ** 2 for x, y in zip(v, f)) / len(v)) ** 0.5
    print(f"F0 = {f0:.4g} N, F1 = {f1:.4g} N/(m/s), F2 = {f2:.4g} N/(m/s)^2  (rms residual {rms:.3g} N, {len(v)} points)")


if __name__ == "__main__":
    main()
