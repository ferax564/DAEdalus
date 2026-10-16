"""Find the friction a DC motor model is missing, then learn a surrogate of the motor.

Uses ``daedalus.sciml`` (NumPy only) on Daedalus.Examples.SciML:

1. MotorTruth plays the test rig: its speed and current, with sensor noise, are the data.
2. MotorKnown is the physics we trust, with a top-level input ``tau_missing`` for the
   torque it cannot explain. ``missing_term`` finds that torque along the data from the
   model's right-hand side.
3. ``sindy`` picks a short formula for it from candidate friction terms; the formula is
   printed as the Modelica binding used in MotorDiscovered, which is then simulated
   and compared with the data.
4. ``fit_surrogate`` learns peak speed and RMS current from the friction and inertia,
   and writes the network as a Modelica block (MotorSurrogate.mo) that simulates in
   DAEdalus and compiles to C with ``daedalus codegen-c``.

    maturin develop --release -m crates/daedalus-py/Cargo.toml   # the daedalus package
    python python/examples/sciml_motor.py [--noise 0.01] [--out DIR]
"""

import argparse
import time
from pathlib import Path

import numpy as np

import daedalus
from daedalus import sciml

LIB = str(Path(__file__).resolve().parents[2] / "library" / "Daedalus")
PKG = "Daedalus.Examples.SciML"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--noise", type=float, default=0.01, help="speed sensor noise, rad/s (std)")
    ap.add_argument("--out", default=".", help="directory for MotorSurrogate.mo")
    a = ap.parse_args(argv)

    truth = daedalus.Model(LIB, f"{PKG}.MotorTruth")
    known = daedalus.Model(LIB, f"{PKG}.MotorKnown")

    # 1. "measurements"
    rig = truth.simulate(vars=["w", "i", "tau_f"])
    rng = np.random.default_rng(0)
    t = rig.time
    w = rig["w"] + rng.normal(0, a.noise, len(t))
    i = rig["i"] + rng.normal(0, a.noise * 1e-2, len(t))
    print(f"data: {len(t)} samples of speed and current over {t[-1]:g} s, noise {a.noise:g} rad/s")

    # 2. the torque the known physics is missing
    t0 = time.perf_counter()
    tau = sciml.missing_term(known, t, {"w": w, "i": i}, "tau_missing")
    inner = slice(10, -10)
    err = tau[inner] - rig["tau_f"][inner]
    print(
        f"missing torque recovered in {time.perf_counter() - t0:.2f} s: "
        f"RMS error {np.sqrt(np.mean(err**2)):.2e} N.m (range {np.ptp(rig['tau_f']):.2e})"
    )

    # 3. a formula for it
    theta, terms = sciml.candidate_library({"w": w}, degree=2, tanh_scales=(0.5, 2.0, 8.0))
    fit = sciml.sindy(theta[inner], tau[inner], terms, threshold=0.1)
    print(f"symbolic regression: tau_missing = {fit}")
    print(f"  as Modelica:  MotorKnown m(tau_missing={fit.expression(prefix='m.', digits=3)});")
    print("  true law:     4e-3*tanh(w/2) + 2e-7*w*abs(w)")
    disc = daedalus.Model(LIB, f"{PKG}.MotorDiscovered").simulate(vars=["m.w"])
    dev = np.max(np.abs(disc["m.w"] - np.interp(disc.time, rig.time, rig["w"])))
    nofr = known.simulate(vars=["w"])
    dev0 = np.max(np.abs(nofr["w"] - np.interp(nofr.time, rig.time, rig["w"])))
    print(f"  speed error vs the rig: {dev:.2g} rad/s with the discovered law, {dev0:.1f} rad/s without")

    # 4. a neural surrogate of the motor
    def qoi(r):
        return {"w_peak": float(np.max(r["w"])), "i_rms": float(np.sqrt(np.mean(r["i"] ** 2)))}

    t0 = time.perf_counter()
    s = sciml.fit_surrogate(
        truth, {"Tc": (0.0, 1e-2), "J": (1e-4, 4e-4)}, qoi, n=120, simulate={"vars": ["w", "i"]}
    )
    print(f"\nneural surrogate in {time.perf_counter() - t0:.1f} s")
    print(s.report())
    out = Path(a.out) / "MotorSurrogate.mo"
    out.write_text(s.to_modelica("MotorSurrogate"))
    print(f"wrote {out}: daedalus codegen-c {out} -m MotorSurrogate --step 0.01")


if __name__ == "__main__":
    main()
