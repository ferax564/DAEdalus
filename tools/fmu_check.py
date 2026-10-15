#!/usr/bin/env python3
"""Validate an FMU exported by `daedalus fmu` and compare it with `daedalus simulate`.

    python3 tools/fmu_check.py Model.fmu reference.csv var1,var2 [--tol 1e-3]

(`states` instead of a variable list compares the continuous states.)

- validates modelDescription.xml with fmpy (schema, variables, ModelStructure),
- simulates the FMU as Co-Simulation (the model's own solver inside fmi2DoStep) and
  as Model Exchange (fmpy's CVode, with the FMU's event indicators),
- compares the given variables with the reference CSV written by
  `daedalus simulate ... -o reference.csv --vars var1,var2`: the largest deviation,
  relative to the range of each signal, must stay below --tol.

Needs fmpy (`python3 -m pip install fmpy`) and numpy.
"""

import argparse
import csv
import sys

import numpy as np
from fmpy import read_model_description, simulate_fmu
from fmpy.validation import validate_fmu


def read_csv(path):
    with open(path, newline="") as f:
        rows = list(csv.reader(f))
    names = [h.strip('"') for h in rows[0]]
    data = np.array([[float(x) for x in r] for r in rows[1:]])
    return {n: data[:, k] for k, n in enumerate(names)}


def compare(label, res, ref, names, tol):
    worst = 0.0
    t_ref = ref["time"]
    # Event instants appear twice in the reference: compare away from them.
    ok_rows = np.ones(len(res["time"]), dtype=bool)
    dup = t_ref[1:][np.diff(t_ref) == 0]
    for te in dup:
        ok_rows &= np.abs(res["time"] - te) > 1e-6 * max(1.0, abs(te))
    for n in names:
        y_ref = np.interp(res["time"], t_ref, ref[n])
        span = max(np.ptp(ref[n]), np.max(np.abs(ref[n])), 1e-12)
        err = np.max(np.abs(res[n] - y_ref)[ok_rows]) / span
        worst = max(worst, err)
        print(f"  {label:3s} {n}: max deviation {err:.2e} of its range "
              f"(final {res[n][-1]:.10g} vs {ref[n][-1]:.10g})")
    if worst > tol:
        print(f"{label}: deviation {worst:.2e} exceeds {tol:.1e}")
        return False
    return True


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("fmu")
    ap.add_argument("reference")
    ap.add_argument("variables")
    ap.add_argument("--tol", type=float, default=1e-3)
    ap.add_argument("--stop", type=float, default=None)
    a = ap.parse_args()

    problems = validate_fmu(a.fmu)
    if problems:
        print("fmpy validation failed:")
        for p in problems:
            print("  " + p)
        return 1
    print(f"fmpy validate {a.fmu}: no problems found")

    md = read_model_description(a.fmu)
    if a.variables == "states":
        names = [d.variable.derivative.name for d in md.derivatives]
    else:
        names = a.variables.split(",")
    ref = read_csv(a.reference)
    de = md.defaultExperiment
    stop = a.stop if a.stop is not None else (
        float(de.stopTime) if de and de.stopTime is not None else 1.0)
    de = md.defaultExperiment
    start = float(de.startTime) if de and de.startTime is not None else 0.0
    interval = (float(de.stepSize) if de and de.stepSize is not None
                else (stop - start) / 500)
    ok = True
    for fmi_type, label in [("CoSimulation", "CS"), ("ModelExchange", "ME")]:
        res = simulate_fmu(a.fmu, fmi_type=fmi_type, stop_time=stop,
                           output_interval=interval, output=names,
                           relative_tolerance=1e-8 if fmi_type == "ModelExchange" else None,
                           record_events=False, validate=True)
        print(f"{label}: {len(res)} rows to t = {res['time'][-1]}")
        ok &= compare(label, res, ref, names, a.tol)
    print("OK" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
