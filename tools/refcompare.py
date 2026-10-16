#!/usr/bin/env python3
"""Compare DAEdalus against the Modelica Association reference results.

The reference results (https://github.com/modelica/MAP-LIB_ReferenceResults) hold,
for every MSL example, a CSV of the "comparison signals" written by Dymola and a
list of those signals. For each model this script

1. simulates it with `daedalus simulate`, writing only the comparison signals on
   the reference time grid, and
2. checks every signal with a tube test in the spirit of csv-compare (the tool the
   MSL release process uses, tolerance 2e-3): around each reference point a box
   of half-width 2e-3 * (time span) in time and 2e-3 * (signal range) in value is
   drawn, and the DAEdalus value must lie inside the union of those boxes.

Usage:
  tools/refcompare.py --daedalus target/release/daedalus --msl MSL_DIR \
      --ref REF_DIR [--filter Modelica.Blocks] [--jobs 4] [--timeout 120] \
      [--json out.json] [--markdown out.md] [--baseline baseline.json] \
      [--models FILE] [--min-pass N] \
      [--root ModelicaTest]

MSL_DIR is a checkout of ModelicaStandardLibrary (with Modelica/,
ModelicaServices/ and Complex.mo); REF_DIR a checkout of MAP-LIB_ReferenceResults
at the matching tag. --root selects the top-level package whose examples are
compared (default: the first part of --filter, else Modelica); with ModelicaTest,
MSL_DIR/ModelicaTest is loaded as well and the models under REF_DIR/ModelicaTest
are listed. With --baseline, the exit status is non-zero when a model
that passed in the baseline no longer passes. --models limits the run to
those names (one per line, or comma-separated). --min-pass N fails when
fewer than N models pass; the nightly full sweep passes the committed
summary's pass count.

The JSON file holds {meta, summary, results}. meta records where the numbers
came from (generated_at, commit, msl_version, tolerance); files written before
meta existed are still accepted as --baseline.
"""

import argparse
import concurrent.futures as cf
import csv
import datetime
import json
import os
import re
import subprocess
import sys
import tempfile
import time
from bisect import bisect_left, bisect_right

TOL = 2e-3


def read_csv(path):
    with open(path, newline="") as f:
        rows = list(csv.reader(f))
    header = [h.strip().strip('"') for h in rows[0]]
    header = ["time" if h.lower() == "time" else h for h in header]
    cols = [[] for _ in header]
    for r in rows[1:]:
        if len(r) != len(header):
            continue
        for i, v in enumerate(r):
            try:
                cols[i].append(float(v))
            except ValueError:
                cols[i].append(float("nan"))
    return {h: c for h, c in zip(header, cols)}


def interp(ts, ys, t):
    """Value at t; at a time that appears twice (an event), the left limit is
    returned for the left query and the right limit otherwise (callers sample
    with both, see tube_check)."""
    i = bisect_right(ts, t)
    if i == 0:
        return ys[0]
    if i >= len(ts):
        return ys[-1]
    t0, t1 = ts[i - 1], ts[i]
    if t1 == t0:
        return ys[i]
    return ys[i - 1] + (ys[i] - ys[i - 1]) * (t - t0) / (t1 - t0)


def tube_check(rt, ry, at, ay):
    """Fraction of DAEdalus samples outside the reference tube, and the worst
    excursion relative to the tube half-height."""
    if not rt or not at:
        return 1.0, float("inf")
    span = rt[-1] - rt[0]
    lo_y, hi_y = min(ry), max(ry)
    dy = TOL * max(hi_y - lo_y, abs(hi_y), abs(lo_y), 1e-10) if hi_y == lo_y else TOL * (hi_y - lo_y)
    dy = max(dy, 1e-10)
    dx = TOL * span if span > 0 else 0.0
    # Check DAEdalus at its own points: a point passes when some reference point
    # within dx in time is within dy in value, or the reference segment through
    # that window comes within dy (linear interpolation at the point itself).
    bad = 0
    worst = 0.0
    n = 0
    for t, y in zip(at, ay):
        if t < rt[0] - 1e-12 or t > rt[-1] + 1e-12:
            continue
        n += 1
        if y != y:  # NaN
            bad += 1
            worst = float("inf")
            continue
        j0 = bisect_left(rt, t - dx)
        j1 = bisect_right(rt, t + dx)
        cands = ry[j0:j1]
        cands.append(interp(rt, ry, t - dx))
        cands.append(interp(rt, ry, t + dx))
        cands.append(interp(rt, ry, t))
        lo = min(cands) - dy
        hi = max(cands) + dy
        if y < lo or y > hi:
            bad += 1
            exc = (lo - y if y < lo else y - hi) / dy + 1.0
            worst = max(worst, exc)
    if n == 0:
        return 1.0, float("inf")
    return bad / n, worst


def grid_intervals(uniq):
    """Number of output intervals of the reference's equidistant grid. The result
    file also holds rows at events, often a hair off a grid point, so counting the
    distinct times would shift every output point off the reference grid; the
    typical spacing gives the grid itself."""
    span = uniq[-1] - uniq[0]
    steps = sorted(b - a for a, b in zip(uniq, uniq[1:]) if b - a > 1e-9 * span)
    if not steps:
        return len(uniq) - 1
    dt = steps[len(steps) // 2]
    n = round(span / dt)
    if n < 1 or abs(n * dt - span) > 1e-6 * span:
        return len(uniq) - 1
    return n


def model_dir(ref, model):
    return os.path.join(ref, *model.split("."))


def list_models(ref, prefix, top="Modelica"):
    out = []
    root = os.path.join(ref, top)
    for d, _, files in os.walk(root):
        if "comparisonSignals.txt" in files:
            rel = os.path.relpath(d, ref).replace(os.sep, ".")
            if rel.startswith(prefix):
                out.append(rel)
    return sorted(out)


def creation_tolerance(d):
    """The relative tolerance the reference result was generated with (creation.txt:
    the model's experiment annotation multiplied by 0.1), or None if not recorded."""
    try:
        with open(os.path.join(d, "creation.txt")) as f:
            for line in f:
                m = re.match(r"\s*Tolerance\s*=\s*([0-9.eE+-]+)", line)
                if m:
                    return float(m.group(1))
    except OSError:
        pass
    return None


def run_one(args, model):
    d = model_dir(args.ref, model)
    with open(os.path.join(d, "comparisonSignals.txt")) as f:
        signals = [s.strip() for s in f if s.strip() and s.strip() != "time"]
    name = model.split(".")[-1]
    ref = read_csv(os.path.join(d, name + ".csv"))
    rt = ref["time"]
    uniq = sorted(set(rt))
    res = {"model": model, "signals": len(signals)}
    with tempfile.TemporaryDirectory() as tmp:
        out = os.path.join(tmp, "r.csv")
        cmd = [
            args.daedalus, "simulate",
            os.path.join(args.msl, "Modelica"),
            os.path.join(args.msl, "ModelicaServices"),
            os.path.join(args.msl, "Complex.mo"),
        ]
        if args.root != "Modelica":
            cmd.append(os.path.join(args.msl, args.root))
        cmd += [
            "-m", model, "-o", out, "--vars", ",".join(signals),
            "--start", repr(uniq[0]), "--stop", repr(uniq[-1]),
        ]
        if len(uniq) > 1:
            cmd += ["-n", str(grid_intervals(uniq))]
        tol = creation_tolerance(d)
        if tol is not None:
            cmd += ["--tol", repr(tol)]
        t0 = time.time()
        try:
            p = subprocess.run(cmd, capture_output=True, text=True, timeout=args.timeout)
        except subprocess.TimeoutExpired:
            res.update(status="timeout", time=args.timeout)
            return res
        res["time"] = round(time.time() - t0, 3)
        if p.returncode != 0 or not os.path.exists(out):
            err = (p.stderr or p.stdout).strip().splitlines()
            res.update(status="error", error=(err[-1] if err else "failed")[:300])
            return res
        ours = read_csv(out)
    at = ours.get("time", [])
    failed = []
    missing = []
    for s in signals:
        if s not in ref:
            continue
        if s not in ours:
            missing.append(s)
            continue
        frac, worst = tube_check(rt, ref[s], at, ours[s])
        if frac > 0:
            failed.append({"signal": s, "outside": round(frac, 4), "worst": worst if worst != float("inf") else "inf"})
    res["missing"] = missing
    res["failed"] = failed
    res["status"] = "pass" if not failed and not missing else "differ"
    return res


def source_commit():
    """The DAEdalus commit being measured: $GITHUB_SHA in CI, else HEAD of the
    checkout this script lives in; None when neither is available."""
    sha = os.environ.get("GITHUB_SHA")
    if sha:
        return sha
    try:
        p = subprocess.run(
            ["git", "rev-parse", "HEAD"], capture_output=True, text=True, timeout=10,
            cwd=os.path.dirname(os.path.abspath(__file__)),
        )
    except (OSError, subprocess.SubprocessError):
        return None
    return (p.stdout.strip() or None) if p.returncode == 0 else None


def msl_version(msl):
    """The version annotation of MSL_DIR/Modelica/package.mo, if readable."""
    try:
        with open(os.path.join(msl, "Modelica", "package.mo"), encoding="utf-8", errors="replace") as f:
            m = re.search(r'\bversion\s*=\s*"([^"]+)"', f.read())
    except OSError:
        return None
    return m.group(1) if m else None


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--daedalus", required=True)
    ap.add_argument("--msl", required=True)
    ap.add_argument("--ref", required=True)
    ap.add_argument("--filter", default=None)
    ap.add_argument("--root", default=None, help="top-level package to compare (Modelica or ModelicaTest; default: the first part of --filter)")
    ap.add_argument("--jobs", type=int, default=os.cpu_count() or 1)
    ap.add_argument("--timeout", type=float, default=120)
    ap.add_argument("--json")
    ap.add_argument("--markdown")
    ap.add_argument("--baseline")
    ap.add_argument("--models", help="file of model names to run (one per line, or commas) instead of every example")
    ap.add_argument("--min-pass", type=int, default=None, help="fail if fewer than this many models pass")
    ap.add_argument("--msl-version", help="recorded in the JSON (default: read from MSL_DIR/Modelica/package.mo)")
    args = ap.parse_args()
    meta = {
        "generated_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        "commit": source_commit(),
        "msl_version": args.msl_version or msl_version(args.msl),
        "tolerance": TOL,
        "solver_tolerance": "from each reference's creation.txt (the experiment tolerance x 0.1)",
        "method": "csv-compare style tube: half-width tolerance*(time span) in time, tolerance*(signal range) in value",
        "reference": "MAP-LIB_ReferenceResults (Dymola-generated)",
        "filter": args.filter,
        "timeout": args.timeout,
    }

    if args.root is None:
        args.root = (args.filter or "Modelica").split(".")[0]
    if args.filter is None:
        args.filter = args.root
    models = list_models(args.ref, args.filter, args.root)
    if args.models:
        text = open(args.models, encoding="utf-8").read()
        wanted = [p.strip() for p in text.replace(",", "\n").splitlines() if p.strip() and not p.strip().startswith("#")]
        known = set(models)
        missing = [m for m in wanted if m not in known]
        if missing:
            print(f"{len(missing)} --models entries are not in the reference set", file=sys.stderr)
            for m in missing[:20]:
                print(f"  {m}", file=sys.stderr)
        models = [m for m in wanted if m in known]
    results = []
    with cf.ThreadPoolExecutor(args.jobs) as ex:
        futs = {ex.submit(run_one, args, m): m for m in models}
        for i, f in enumerate(cf.as_completed(futs), 1):
            try:
                r = f.result()
            except Exception as e:  # a crash in the comparison itself
                r = {"model": futs[f], "status": "error", "error": f"compare: {e}"}
            results.append(r)
            print(f"[{i}/{len(models)}] {r['status']:7} {r['model']}", file=sys.stderr, flush=True)
    results.sort(key=lambda r: r["model"])
    counts = {}
    for r in results:
        counts[r["status"]] = counts.get(r["status"], 0) + 1
    summary = {"total": len(results), **counts}
    print(json.dumps(summary))
    if args.json:
        with open(args.json, "w") as f:
            json.dump({"meta": meta, "summary": summary, "results": results}, f, indent=1)
    if args.markdown:
        write_markdown(args.markdown, summary, results)
    failed = False
    if args.baseline:
        with open(args.baseline) as f:
            # Only "results" is read, so baselines written before "meta" still work.
            base = {r["model"]: r.get("status") for r in json.load(f).get("results", [])}
        now = {r["model"]: r["status"] for r in results}
        lost = sorted(m for m, s in base.items() if s == "pass" and now.get(m) not in (None, "pass"))
        if lost:
            print("Models that passed in the baseline and no longer do:", file=sys.stderr)
            for m in lost:
                print(f"  {m}: {now[m]}", file=sys.stderr)
            failed = True
    if args.min_pass is not None and summary.get("pass", 0) < args.min_pass:
        print(
            f"pass count {summary.get('pass', 0)} is below the committed summary ({args.min_pass})",
            file=sys.stderr,
        )
        failed = True
    if failed:
        sys.exit(1)


def write_markdown(path, summary, results):
    tot = summary["total"]
    p = summary.get("pass", 0)
    lines = [
        "# MSL reference-result comparison",
        "",
        f"{p} of {tot} examples match the MSL reference results (Dymola-generated, tube tolerance 2e-3).",
        "",
        "| Status | Models |",
        "| --- | ---: |",
    ]
    for k in ("pass", "differ", "error", "timeout"):
        lines.append(f"| {k} | {summary.get(k, 0)} |")
    lines += ["", "| Model | Status | Time (s) | Detail |", "| --- | --- | ---: | --- |"]
    for r in results:
        if r["status"] == "pass":
            detail = ""
        elif r["status"] == "differ":
            bad = [f["signal"] for f in r.get("failed", [])] + [s + " (missing)" for s in r.get("missing", [])]
            detail = f"{len(bad)}/{r['signals']} signals: " + ", ".join(bad[:4]) + (" …" if len(bad) > 4 else "")
        else:
            detail = r.get("error", "")
        detail = detail.replace("|", "\\|")
        lines.append(f"| {r['model']} | {r['status']} | {r.get('time', '')} | {detail} |")
    with open(path, "w") as f:
        f.write("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
