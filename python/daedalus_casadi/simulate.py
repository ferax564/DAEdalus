"""Simulation of an exported model with CasADi's integrators (IDAS / CVODES).

DAEdalus itself is the reference simulator; this is for closed loops (MPC),
for generating initial guesses and for checking an export against DAEdalus.
"""

from __future__ import annotations

from collections.abc import Callable, Sequence

import casadi as ca
import numpy as np

from .model import Model


class Trajectory:
    """Values of a model on a time grid; ``traj["name"]`` gives a variable's samples."""

    def __init__(self, model: Model, t, x, z, u, p):
        self.model = model
        self.t = np.asarray(t, dtype=float)
        self.x = np.asarray(x, dtype=float).reshape(len(self.t), model.nx)
        self.z = np.asarray(z, dtype=float).reshape(len(self.t), model.nz)
        self.u = np.asarray(u, dtype=float).reshape(len(self.t), model.nu)
        self.p = np.asarray(p, dtype=float).reshape(model.np)
        self._cache: dict[str, np.ndarray] = {}

    def __getitem__(self, name: str) -> np.ndarray:
        return self.value(name)

    def value(self, name: str | ca.SX) -> np.ndarray:
        key = name if isinstance(name, str) else None
        if key is not None and key in self._cache:
            return self._cache[key]
        f = self.model.function([name]).map(len(self.t))
        y = np.array(f(self.t, self.x.T, self.z.T, self.u.T, np.tile(self.p[:, None], len(self.t)))).ravel()
        if key is not None:
            self._cache[key] = y
        return y


def _make_integrator(model: Model, dt: float, method: str, opts: dict) -> ca.Function:
    t0 = ca.SX.sym("t0")
    tau = ca.SX.sym("tau")
    subs = ca.substitute([model.ode, model.alg], [model.t], [t0 + tau])
    dae = {
        "x": model.x,
        "z": model.z,
        "p": ca.vertcat(model.u, model.p, t0),
        "t": tau,
        "ode": subs[0],
        "alg": subs[1],
    }
    if model.nz == 0:
        del dae["z"], dae["alg"]
    plugin = method or ("idas" if model.nz else "cvodes")
    o = {"abstol": 1e-8, "reltol": 1e-6}
    if plugin == "collocation":
        o = {"number_of_finite_elements": 4, "interpolation_order": 3}
    o.update(opts)
    return ca.integrator("F", plugin, dae, 0.0, dt, o)


def simulate(
    model: Model,
    t_grid: Sequence[float],
    u: Callable[[float, np.ndarray], Sequence[float]] | np.ndarray | None = None,
    p: Sequence[float] | None = None,
    x0: Sequence[float] | None = None,
    z0: Sequence[float] | None = None,
    method: str | None = None,
    options: dict | None = None,
) -> Trajectory:
    """Simulate on ``t_grid`` with inputs held constant over each interval.

    ``u`` is ``None`` (the inputs' start values), an array with one row per
    interval, or a function ``u(t, x) -> inputs`` evaluated at the start of each
    interval (a feedback law).
    """
    t_grid = np.asarray(t_grid, dtype=float)
    p = model.p0 if p is None else np.asarray(p, dtype=float)
    x = model.x0.copy() if x0 is None else np.asarray(x0, dtype=float)
    z = model.z0.copy() if z0 is None else np.asarray(z0, dtype=float)
    dts = np.diff(t_grid)
    integrators: dict[float, ca.Function] = {}
    zf = _alg_solver(model)
    X, Z, U = [], [], []
    for k, t in enumerate(t_grid):
        if callable(u):
            uk = np.asarray(u(t, x), dtype=float).reshape(model.nu)
        elif u is None:
            uk = model.u0
        else:
            uk = np.asarray(u, dtype=float).reshape(-1, model.nu)[min(k, len(t_grid) - 2)] if model.nu else model.u0
        if model.nz:
            z = np.array(zf(z, ca.vertcat(t, x, uk, p))).ravel()
        X.append(x.copy())
        Z.append(z.copy())
        U.append(uk)
        if k == len(t_grid) - 1:
            break
        dt = round(float(dts[k]), 12)
        if dt not in integrators:
            integrators[dt] = _make_integrator(model, dt, method or "", options or {})
        args = {"x0": x, "p": ca.vertcat(uk, p, t)}
        if model.nz:
            args["z0"] = z
        r = integrators[dt](**args)
        x = np.array(r["xf"]).ravel()
        if model.nz:
            z = np.array(r["zf"]).ravel()
    return Trajectory(model, t_grid, np.array(X), np.array(Z), np.array(U), p)


def _alg_solver(model: Model):
    """Newton solver for ``alg(t, x, z, u, p) = 0`` in ``z``."""
    if model.nz == 0:
        return None
    par = ca.vertcat(model.t, model.x, model.u, model.p)
    g = ca.Function("g", [model.z, par], [model.alg])
    return ca.rootfinder("zsolve", "newton", g, {"abstol": 1e-10, "max_iter": 50, "error_on_fail": False})
