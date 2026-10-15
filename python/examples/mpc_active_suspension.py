"""Model predictive control of an active suspension that previews a road bump.

Plant: Daedalus.Examples.OptimalControl.ActiveSuspension, a quarter car with a
soft passive damper and a force actuator ``F_act`` between wheel and body. The
road bump is part of the model (a function of time), so the controller's
prediction sees it coming. Every 10 ms the MPC solves a 0.25 s horizon that
trades body acceleration against suspension travel and actuator force, with
|F_act| <= 2.5 kN, and applies the first move. The closed loop is compared with
the same car without actuation.

    python python/examples/mpc_active_suspension.py [--plot out.png]
"""

import argparse
import sys
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import daedalus_casadi as dc

MODEL = "Daedalus.Examples.OptimalControl.ActiveSuspension"


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--plot", help="save a figure to this file")
    ap.add_argument("--stop", type=float, default=1.0)
    ap.add_argument("--w-travel", type=float, default=10.0, help="weight on (travel / 30 mm)^2")
    ap.add_argument("--w-force", type=float, default=1.0, help="weight on (F_act / F_max)^2")
    args = ap.parse_args(argv)

    m = dc.export(MODEL)
    print(m)
    dt, N = 0.01, 25
    F_max = 2500.0

    mpc = dc.MPC(m, N=N, dt=dt, degree=2)
    ocp = mpc.ocp
    mpc.bound("F_act", -F_max, F_max)
    J = (
        ocp.integral(m["a_body"] ** 2)
        + args.w_travel * ocp.integral((m["travel"] / 0.03) ** 2)
        + args.w_force * ocp.integral((m["F_act"] / F_max) ** 2)
    )
    mpc.minimize(J)

    t0 = time.perf_counter()
    solve_times = []

    def log(k, t, x, u):
        solve_times.append(mpc.solve_time)

    closed = mpc.run(args.stop, callback=log)
    total = time.perf_counter() - t0
    # Replay both on a 1 ms grid (the input is held over each 10 ms control period).
    sub = 10
    t = np.linspace(closed.t[0], closed.t[-1], sub * (len(closed.t) - 1) + 1)
    closed = dc.simulate(m, t, u=np.repeat(closed.u[:-1], sub, axis=0))
    passive = dc.simulate(m, t, u=np.zeros((len(t) - 1, 1)))

    def rms(y):
        return float(np.sqrt(np.mean(y**2)))

    print(f"{len(solve_times)} MPC steps in {total:.1f} s (median solve {1000 * np.median(solve_times):.0f} ms)")
    for name, tr in (("passive", passive), ("MPC", closed)):
        print(
            f"{name:8s} rms body accel {rms(tr['a_body']):6.2f} m/s2, "
            f"peak {np.max(np.abs(tr['a_body'])):6.2f} m/s2, "
            f"peak travel {1000 * np.max(np.abs(tr['travel'])):5.1f} mm, "
            f"peak force {np.max(np.abs(tr['F_act'])):6.0f} N"
        )

    if args.plot:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt

        fig, ax = plt.subplots(4, 1, figsize=(7, 8), sharex=True)
        ax[0].plot(t, 1000 * closed["bump.y"], color="0.4")
        ax[0].set_ylabel("road [mm]")
        for name, tr in (("passive", passive), ("MPC", closed)):
            ax[1].plot(t, tr["a_body"], label=name)
            ax[2].plot(t, 1000 * tr["travel"], label=name)
        ax[1].set_ylabel("body accel [m/s2]")
        ax[1].legend()
        ax[2].set_ylabel("travel [mm]")
        ax[3].step(t, closed["F_act"], where="post")
        ax[3].set_ylabel("actuator [N]")
        ax[3].set_xlabel("time [s]")
        fig.suptitle("Active suspension: MPC with bump preview vs passive")
        fig.tight_layout()
        fig.savefig(args.plot, dpi=120)
        print("figure:", args.plot)
    return closed, passive


if __name__ == "__main__":
    main()
