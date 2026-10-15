"""Moving horizon estimation: road-load coefficients from a noisy coast-down.

Plant: Daedalus.Examples.OptimalControl.CoastDownTest, a 1650 kg car coasting
from 130 km/h in neutral. DAEdalus simulates the "real" car, whose drag and
rolling-resistance coefficients differ from the model's (Cd 0.33 instead of
0.29, crr 0.012 instead of 0.009), and the speed is read once a second with
0.3 km/h of noise for 170 s, until it is down to about 10 km/h. A moving horizon estimator over a 30 s window recovers the speed
and both coefficients as the coast-down goes on.

    python python/examples/mhe_coastdown.py [--plot out.png]
"""

import argparse
import sys
import time
from pathlib import Path

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import daedalus_casadi as dc

MODEL = "Daedalus.Examples.OptimalControl.CoastDownTest"
TRUE = {"body.Cd": 0.33, "body.crr": 0.012}


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    ap.add_argument("--plot", help="save a figure to this file")
    ap.add_argument("--noise", type=float, default=0.3, help="speed noise std [km/h]")
    ap.add_argument("--seed", type=int, default=1)
    args = ap.parse_args(argv)

    # The "real" car: simulated by DAEdalus with the true coefficients.
    t, true = dc.simulate_csv(MODEL, ["v_kmh", "body.v"], params=TRUE, stop=170, interval=1.0)
    rng = np.random.default_rng(args.seed)
    y = true["v_kmh"] + args.noise * rng.standard_normal(len(t))

    m = dc.export(MODEL)
    print(m)
    names = list(TRUE)
    print("model values:", {n: float(m.p0[m.p_index(n)]) for n in names})

    mhe = dc.MHE(
        m,
        N=30,
        dt=1.0,
        measurements=["v_kmh"],
        meas_std=[args.noise],
        process_std=[1e-3, 0.05],  # body.s [m], body.v [m/s] per interval
        estimate_params=names,
        prior_std=[1.0, 1.0],
        param_prior_std=[0.01, 0.001],
    )
    mhe.bound_param("body.Cd", 0.1, 1.0)
    mhe.bound_param("body.crr", 0.0, 0.05)

    t0 = time.perf_counter()
    t_est, x_est, p_est = mhe.run(t, y)
    dt = time.perf_counter() - t0
    v_est = x_est[:, m.x_names.index("body.v")] * 3.6
    v_true = np.interp(t_est, t, true["v_kmh"])
    print(f"{len(t_est)} windows in {dt:.1f} s")
    print(f"speed error: rms {np.sqrt(np.mean((v_est - v_true) ** 2)):.3f} km/h (measurement noise {args.noise} km/h)")
    for i, n in enumerate(names):
        print(f"{n}: estimate {p_est[-1, i]:.4f}, true {TRUE[n]}, model {m.p0[m.p_index(n)]}")

    if args.plot:
        import matplotlib

        matplotlib.use("Agg")
        import matplotlib.pyplot as plt

        fig, ax = plt.subplots(3, 1, figsize=(7, 8), sharex=True)
        ax[0].plot(t, y, ".", ms=3, color="0.6", label="measured")
        ax[0].plot(t, true["v_kmh"], label="true")
        ax[0].plot(t_est, v_est, "--", label="MHE")
        ax[0].set_ylabel("speed [km/h]")
        ax[0].legend()
        for i, (n, a) in enumerate(zip(names, ax[1:])):
            a.plot(t_est, p_est[:, i], label="MHE estimate")
            a.axhline(TRUE[n], color="k", ls=":", label="true")
            a.axhline(m.p0[m.p_index(n)], color="r", ls=":", label="model default")
            a.set_ylabel(n)
            a.legend()
        ax[-1].set_xlabel("time [s]")
        fig.suptitle("Coast-down: moving horizon estimation of speed and road load")
        fig.tight_layout()
        fig.savefig(args.plot, dpi=120)
        print("figure:", args.plot)
    return t_est, x_est, p_est


if __name__ == "__main__":
    main()
