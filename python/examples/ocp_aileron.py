"""Minimum-time aileron deflection: optimal control of a DAEdalus hydraulic actuator.

Plant: Daedalus.Examples.OptimalControl.AileronServo (servo valve, cylinder,
3000 psi supply, aileron with an aerodynamic hinge moment); the valve command
``u`` in [-1, 1] is the control. Find the fastest move from 0 to 10 degrees
that ends at rest, then check the optimal command on the unsmoothed model with
CasADi's IDAS integrator.

    python python/examples/ocp_aileron.py [--plot out.png]
"""

import argparse
import sys
import time
from pathlib import Path

import casadi as ca
import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import daedalus_casadi as dc

MODEL = "Daedalus.Examples.OptimalControl.AileronServo"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--plot", help="save a figure to this file")
    ap.add_argument("-N", type=int, default=40, help="collocation intervals")
    ap.add_argument("--target", type=float, default=10.0, help="target deflection [deg]")
    args = ap.parse_args(argv)

    # The valve has kinks (spool saturation, metering edges at zero opening) right where
    # the optimum holds it: smooth them over 1e-3 so IPOPT converges.
    m = dc.export(MODEL, smooth=1e-3)
    exact = dc.export(MODEL)
    print(m)

    # Initial guess: the proportional position loop of AileronActuator.PositionControl.
    i_s = m.x_names.index("act.cylinder.s")
    s_ref = m.x0[i_s] + m.p0[m.p_index("r_horn")] * np.deg2rad(args.target)
    guess = dc.simulate(m, np.linspace(0, 0.06, args.N + 1), u=lambda t, x: [np.clip(40 * (s_ref - x[i_s]), -1, 1)])

    ocp = dc.OCP(m, N=args.N, free_time=True, T_guess=0.06)
    ocp.bound("u", -1, 1)
    ocp.subject_to(ocp.final("delta_deg") == args.target, ocp.final("aileron.w") == 0)
    u = ocp.input("u")
    ocp.minimize(ocp.T + 1e-4 * ca.sumsqr(u[:, 1:] - u[:, :-1]))
    ocp.guess(guess)

    t0 = time.perf_counter()
    sol = ocp.solve()
    dt = time.perf_counter() - t0
    print(f"minimum time {sol.T * 1000:.1f} ms ({sol.stats['iter_count']} IPOPT iterations, {dt:.2f} s)")
    print("valve command per interval:", np.round(sol.u.ravel(), 2))

    # Replay the optimal command on the exact (unsmoothed) model.
    check = dc.simulate(exact, sol.t, u=sol.u)
    err = np.max(np.abs(check["delta_deg"] - sol["delta_deg"]))
    print(f"exact model, same command: final deflection {check['delta_deg'][-1]:.2f} deg (max deviation {err:.3f} deg)")

    if args.plot:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt

        fig, ax = plt.subplots(3, 1, figsize=(7, 7), sharex=True)
        tm = sol.t * 1000
        ax[0].plot(tm, sol["delta_deg"], label="optimal (collocation)")
        ax[0].plot(tm, check["delta_deg"], "--", label="exact model, IDAS")
        ax[0].set_ylabel("aileron [deg]")
        ax[0].legend()
        ax[1].step(tm[:-1], sol.u.ravel(), where="post")
        ax[1].set_ylabel("valve command u")
        ax[2].plot(tm, sol["act.cylinder.pA"] / 1e5, label="pA")
        ax[2].plot(tm, sol["act.cylinder.pB"] / 1e5, label="pB")
        ax[2].set_ylabel("chamber pressure [bar]")
        ax[2].set_xlabel("time [ms]")
        ax[2].legend()
        fig.suptitle(f"Minimum-time {args.target:g} deg aileron move: {sol.T * 1000:.1f} ms")
        fig.tight_layout()
        fig.savefig(args.plot, dpi=120)
        print("figure:", args.plot)
    return sol


if __name__ == "__main__":
    main()
