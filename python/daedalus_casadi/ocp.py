"""Direct collocation of a DAEdalus model: optimal control problems on ``casadi.Opti``.

The horizon is split into ``N`` intervals. On each interval the states are a
polynomial through Radau IIA collocation points (stiffly accurate, the usual
choice for DAEs); algebraic variables are decision variables at every
collocation point and mesh point; inputs are constant over each interval.
Decision variables are scaled by nominal values so IPOPT sees numbers of
order one even for pressures in pascal.

Costs and constraints refer to model variables by name, or to expressions
built from them with ``model["name"]``::

    ocp = OCP(model, N=40, T=0.3)
    ocp.bound("u", -1, 1)                          # every interval
    ocp.subject_to(ocp.final("delta_deg") == 10)
    ocp.minimize(ocp.integral((model["delta_deg"] - 10) ** 2))
    sol = ocp.solve()
    sol["delta_deg"]                               # values on the mesh
"""

from __future__ import annotations

from collections.abc import Sequence

import casadi as ca
import numpy as np

from .model import Model
from .simulate import Trajectory


def radau_points(degree: int) -> np.ndarray:
    """Radau IIA collocation points on (0, 1] (the last point is 1)."""
    return np.append(0.0, np.array(ca.collocation_points(degree, "radau")))


def collocation_matrices(degree: int):
    """``C[r, j]``: derivative at point ``j`` of the Lagrange basis ``r``; ``B[j]``: quadrature weights."""
    tau = radau_points(degree)
    d = degree
    C = np.zeros((d + 1, d + 1))
    B = np.zeros(d + 1)
    for r in range(d + 1):
        p = np.poly1d([1.0])
        for s in range(d + 1):
            if s != r:
                p *= np.poly1d([1.0, -tau[s]]) / (tau[r] - tau[s])
        dp = np.polyder(p)
        for j in range(d + 1):
            C[r, j] = dp(tau[j])
        B[r] = np.polyint(p)(1.0)
    return tau, C, B


def _nominal(v, value: float) -> float:
    """Scale of a variable: its ``nominal`` attribute or its initial magnitude, whichever is larger."""
    a = abs(value) if np.isfinite(value) else 0.0
    if v is not None and v.nominal is not None and v.nominal > 0:
        return max(float(v.nominal), a)
    return a if a > 1e-6 else 1.0


class OCP:
    """An optimal control problem over a DAEdalus model, transcribed by direct collocation.

    Parameters
    ----------
    model : Model
    N : int
        Number of intervals (the inputs are piecewise constant on them).
    T : float, optional
        Horizon length. Omit it (or pass ``free_time=True``) for a free final time;
        ``ocp.T`` is then a decision variable.
    degree : int
        Collocation points per interval.
    t0 : float
        Start time (models with time-varying sources use it). It is a parameter:
        ``ocp.opti.set_value(ocp.t0, ...)`` changes it.
    free_params : sequence of str
        Parameters that become decision variables (parameter estimation, design).
    process_noise : bool
        Add a disturbance ``w_k`` to the state at each mesh point (for estimation):
        ``x_{k+1} = x(t_{k+1}^-) + w_k``. ``ocp.W`` holds the scaled disturbances.
    fix_initial_state : bool
        Constrain the initial state to the parameter ``ocp.x_init`` (default value: ``model.x0``).
    """

    def __init__(
        self,
        model: Model,
        N: int,
        T: float | None = None,
        degree: int = 3,
        t0: float | None = None,
        free_time: bool = False,
        T_guess: float | None = None,
        free_params: Sequence[str] = (),
        process_noise: bool = False,
        fix_initial_state: bool = True,
        nominal: dict[str, float] | None = None,
    ):
        self.model = m = model
        self.N, self.degree = N, degree
        self.opti = opti = ca.Opti()
        nominal = dict(nominal or {})

        def nom(name, value):
            if name in nominal:
                return float(nominal[name])
            return _nominal(m.variable(name), value)

        self.x_nom = np.array([nom(n, v) for n, v in zip(m.x_names, m.x0)])
        self.z_nom = np.array([nom(n, v) for n, v in zip(m.z_names, m.z0)])
        self.u_nom = np.array([nom(n, v) for n, v in zip(m.u_names, m.u0)])
        self.p_nom = np.array([nom(n, v) for n, v in zip(m.p_names, m.p0)])

        free_time = free_time or T is None
        if free_time:
            self.T = opti.variable()
            opti.subject_to(self.T > 0)
            opti.set_initial(self.T, T_guess if T_guess is not None else (T or 1.0))
        else:
            self.T = float(T)
        self.free_time = free_time
        self.t0 = opti.parameter()
        opti.set_value(self.t0, m.start_time if t0 is None else t0)

        # Parameters: fixed ones are Opti parameters, free ones decision variables.
        self.free_params = list(free_params)
        free_idx = [m.p_index(n) for n in self.free_params]
        self.p_fixed = opti.parameter(m.np) if m.np else ca.MX(0, 1)
        if m.np:
            opti.set_value(self.p_fixed, m.p0)
        self.P_free = opti.variable(len(free_idx)) if free_idx else ca.MX(0, 1)
        prow = []
        for i in range(m.np):
            if i in free_idx:
                k = free_idx.index(i)
                prow.append(self.P_free[k] * self.p_nom[i])
                opti.set_initial(self.P_free[k], m.p0[i] / self.p_nom[i])
            else:
                prow.append(self.p_fixed[i])
        self.P = ca.vertcat(*prow) if prow else ca.MX(0, 1)

        tau, C, B = collocation_matrices(degree)
        self.tau, self._C, self._B = tau, C, B
        h = self.T / N
        self.h = h
        f = m.dae_function()
        nx, nz, nu = m.nx, m.nz, m.nu
        Xn, Zn, Un = (ca.DM(a) if len(a) else ca.DM(0, 1) for a in (self.x_nom, self.z_nom, self.u_nom))

        def var(n, scale, init):
            if n == 0:
                return ca.MX(0, 1)
            v = opti.variable(n)
            opti.set_initial(v, np.asarray(init) / np.asarray(scale))
            return v

        self.Xs = [var(nx, self.x_nom, m.x0) for _ in range(N + 1)]
        self.Zs = [var(nz, self.z_nom, m.z0) for _ in range(N + 1)]
        self.Us = [var(nu, self.u_nom, m.u0) for _ in range(N)]
        self.Xc = [[var(nx, self.x_nom, m.x0) for _ in range(degree)] for _ in range(N)]
        self.Zc = [[var(nz, self.z_nom, m.z0) for _ in range(degree)] for _ in range(N)]
        self.Ws = [var(nx, self.x_nom, np.zeros(nx)) for _ in range(N)] if process_noise else []

        X = [Xn * xs for xs in self.Xs]
        Z = [Zn * zs for zs in self.Zs]
        U = [Un * us for us in self.Us]
        self.X, self.Z, self.U = X, Z, U
        self.t_mesh = [self.t0 + k * h for k in range(N + 1)]

        self.x_init = opti.parameter(nx) if nx else ca.MX(0, 1)
        if nx:
            opti.set_value(self.x_init, m.x0)
        if fix_initial_state and nx:
            opti.subject_to(self.Xs[0] == self.x_init / Xn)

        # Algebraic residual scale: |dg/dz| * z_nominal at the initial point.
        if nz:
            gz = np.abs(
                np.array(ca.Function("gz", m.args, [ca.jacobian(m.alg, m.z)])(m.start_time, m.x0, m.z0, m.u0, m.p0))
            )
            g_nom = np.max(gz * self.z_nom[None, :], axis=1)
            g_nom[~np.isfinite(g_nom) | (g_nom <= 0)] = 1.0
        else:
            g_nom = np.ones(0)
        Gn = ca.DM(g_nom) if nz else ca.DM(0, 1)
        self.g_nom = g_nom

        self._colloc = []  # (t, x, z, u) at collocation points, for quadratures
        for k in range(N):
            uk = U[min(k, N - 1)]
            # Algebraic equations at mesh points (the end mesh point uses the last input).
            if nz:
                _, gk = f(self.t_mesh[k], X[k], Z[k], uk, self.P)
                opti.subject_to(gk / Gn == 0)
            pts = [X[k]] + [Xn * xc for xc in self.Xc[k]]
            row = []
            for j in range(1, degree + 1):
                tj = self.t_mesh[k] + tau[j] * h
                xj = pts[j]
                zj = Zn * self.Zc[k][j - 1] if nz else ca.MX(0, 1)
                dx = sum(C[r, j] * pts[r] for r in range(degree + 1))
                fj, gj = f(tj, xj, zj, uk, self.P)
                opti.subject_to((dx - h * fj) / Xn == 0)
                if nz:
                    opti.subject_to(gj / Gn == 0)
                row.append((tj, xj, zj, uk))
            self._colloc.append(row)
            x_end = pts[degree]
            if process_noise:
                opti.subject_to(self.Xs[k + 1] == x_end / Xn + self.Ws[k])
            else:
                opti.subject_to(self.Xs[k + 1] == x_end / Xn)
        if nz:
            _, gN = f(self.t_mesh[N], X[N], Z[N], U[N - 1], self.P)
            opti.subject_to(gN / Gn == 0)
        self._funcs: dict = {}
        self._objective = 0
        self.solver_options: dict = {}

    # ----------------------------------------------------------- expressions
    def _fn(self, e) -> ca.Function:
        key = e if isinstance(e, str) else id(e)
        if key not in self._funcs:
            self._funcs[key] = (self.model.function([e]), e)
        return self._funcs[key][0]

    def at(self, e: str | ca.SX, k: int) -> ca.MX:
        """Value of a variable name or model expression at mesh point ``k`` (0..N)."""
        k = k % (self.N + 1)
        u = self.U[min(k, self.N - 1)]
        return self._fn(e)(self.t_mesh[k], self.X[k], self.Z[k], u, self.P)

    def trajectory(self, e: str | ca.SX) -> list[ca.MX]:
        """Values on the mesh points 0..N."""
        return [self.at(e, k) for k in range(self.N + 1)]

    def initial(self, e: str | ca.SX) -> ca.MX:
        return self.at(e, 0)

    def final(self, e: str | ca.SX) -> ca.MX:
        return self.at(e, self.N)

    def integral(self, e: str | ca.SX) -> ca.MX:
        """Integral over the horizon by the collocation quadrature (exact for the polynomials)."""
        f = self._fn(e)
        tot = 0
        for row in self._colloc:
            for j, (tj, xj, zj, uj) in enumerate(row, start=1):
                tot = tot + self._B[j] * f(tj, xj, zj, uj, self.P)
        return self.h * tot

    def input(self, name: str, k: int | None = None):
        """An input on interval ``k`` (all intervals as a row when ``k`` is None)."""
        i = self.model.u_names.index(name)
        if k is None:
            return ca.horzcat(*[u[i] for u in self.U])
        return self.U[k][i]

    def param(self, name: str) -> ca.MX:
        return self.P[self.model.p_index(name)]

    # ------------------------------------------------------------ problem
    def minimize(self, J) -> None:
        self._objective = J
        self.opti.minimize(J)

    def subject_to(self, *constraints) -> None:
        for c in constraints:
            self.opti.subject_to(c)

    def bound(self, name: str, lb: float | None = None, ub: float | None = None, path: bool = True) -> None:
        """Bounds on an input (every interval) or on any variable (every mesh point)."""
        m = self.model
        if name in m.u_names:
            i = m.u_names.index(name)
            for u in self.Us:
                if lb is not None:
                    self.opti.subject_to(u[i] >= lb / self.u_nom[i])
                if ub is not None:
                    self.opti.subject_to(u[i] <= ub / self.u_nom[i])
            return
        if name in self.free_params:
            v = self.param(name)
            if lb is not None:
                self.opti.subject_to(v >= lb)
            if ub is not None:
                self.opti.subject_to(v <= ub)
            return
        ks = range(self.N + 1) if path else [self.N]
        for k in ks:
            y = self.at(name, k)
            if lb is not None:
                self.opti.subject_to(y >= lb)
            if ub is not None:
                self.opti.subject_to(y <= ub)

    def set_initial_state(self, x0: Sequence[float]) -> None:
        self.opti.set_value(self.x_init, np.asarray(x0, dtype=float))

    def set_param(self, name: str, value: float) -> None:
        m = self.model
        p = np.array(self.opti.value(self.p_fixed)).ravel() if m.np else np.zeros(0)
        p[m.p_index(name)] = value
        self.opti.set_value(self.p_fixed, p)

    # --------------------------------------------------------- initial guess
    def guess(self, traj: Trajectory | None = None, u: Sequence[float] | np.ndarray | None = None) -> None:
        """Initial guess from a simulation (on any grid) and/or input values."""
        opti = self.opti
        if u is not None:
            ua = np.asarray(u, dtype=float).reshape(-1, self.model.nu)
            for k, us in enumerate(self.Us):
                opti.set_initial(us, ua[min(k, len(ua) - 1)] / self.u_nom)
        if traj is None:
            return
        T = self.T if not self.free_time else float(opti.debug.value(self.T, opti.initial()))
        t0 = traj.t[0]

        def interp(t, arr):
            return np.array([np.interp(t, traj.t, arr[:, i]) for i in range(arr.shape[1])])

        for k in range(self.N + 1):
            t = t0 + k * T / self.N
            if self.model.nx:
                opti.set_initial(self.Xs[k], interp(t, traj.x) / self.x_nom)
            if self.model.nz:
                opti.set_initial(self.Zs[k], interp(t, traj.z) / self.z_nom)
            if k < self.N:
                if self.model.nu and u is None:
                    opti.set_initial(self.Us[k], interp(t, traj.u) / self.u_nom)
                for j in range(self.degree):
                    tj = t + self.tau[j + 1] * T / self.N
                    if self.model.nx:
                        opti.set_initial(self.Xc[k][j], interp(tj, traj.x) / self.x_nom)
                    if self.model.nz:
                        opti.set_initial(self.Zc[k][j], interp(tj, traj.z) / self.z_nom)

    # --------------------------------------------------------------- solve
    def solve(self, options: dict | None = None, verbose: bool = False) -> Solution:
        o = {"ipopt.print_level": 5 if verbose else 0, "print_time": verbose, "ipopt.sb": "yes"}
        o.update(self.solver_options)
        o.update(options or {})
        key = repr(sorted(o.items()))
        if key != getattr(self, "_solver_key", None):
            self.opti.solver("ipopt", o)
            self._solver_key = key
        return Solution(self, self.opti.solve())


class Solution:
    """Result of :meth:`OCP.solve`: ``sol["name"]`` is a variable on the mesh."""

    def __init__(self, ocp: OCP, sol):
        self.ocp, self.sol = ocp, sol
        m = ocp.model
        v = sol.value
        self.T = float(v(ocp.T)) if ocp.free_time else ocp.T
        t0 = float(v(ocp.t0))
        self.t = t0 + np.arange(ocp.N + 1) * self.T / ocp.N
        self.x = np.array([np.atleast_1d(v(x)) for x in ocp.X]) if m.nx else np.zeros((ocp.N + 1, 0))
        self.z = np.array([np.atleast_1d(v(z)) for z in ocp.Z]) if m.nz else np.zeros((ocp.N + 1, 0))
        self.u = np.array([np.atleast_1d(v(u)) for u in ocp.U]) if m.nu else np.zeros((ocp.N, 0))
        self.p = np.atleast_1d(np.array(v(ocp.P)).ravel()) if m.np else np.zeros(0)
        self.objective = float(v(ocp.opti.f))
        self.stats = sol.stats()
        u_mesh = np.vstack([self.u, self.u[-1:]]) if m.nu else np.zeros((ocp.N + 1, 0))
        self.trajectory = Trajectory(m, self.t, self.x, self.z, u_mesh, self.p)

    def __getitem__(self, name: str) -> np.ndarray:
        return self.trajectory[name]

    def value(self, e) -> np.ndarray:
        """Any expression on the Opti stack, or a model variable name / expression."""
        if isinstance(e, (str, ca.SX)):
            return self.trajectory.value(e)
        return np.array(self.sol.value(e))

    def param(self, name: str) -> float:
        return float(self.p[self.ocp.model.p_index(name)])
