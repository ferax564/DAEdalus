"""Moving horizon estimation of states and parameters of a DAEdalus model.

Over a window of ``N`` intervals the estimator finds the initial state, the
process disturbances ``w_k`` and the chosen parameters that best explain the
measurements, by weighted least squares::

    min  |x_0 - x_prior|^2_P + |p - p_prior|^2_Pp
         + sum_k |y_k - h(x_k, z_k, u_k, p)|^2_R  + sum_k |w_k|^2_Q
    s.t. the model DAE (collocation) with x_{k+1} = x(t_{k+1}^-) + w_k

Weights are given as standard deviations in the variables' own units. After
each window the prior moves to the estimate one interval later (the arrival
cost keeps its weight).
"""

from __future__ import annotations

from collections.abc import Sequence

import casadi as ca
import numpy as np

from .model import Model
from .ocp import OCP, Solution


def _std(values, n, default):
    if values is None:
        return np.full(n, float(default))
    a = np.asarray(values, dtype=float).ravel()
    return np.full(n, float(a[0])) if a.size == 1 else a


class MHE:
    """Moving horizon estimator.

    Parameters
    ----------
    model : Model
    N : int
        Intervals in the estimation window (``N + 1`` measurement times).
    dt : float
        Sampling period.
    measurements : sequence of str
        Measured variables (any model variable or expression).
    meas_std : sequence of float
        Measurement noise standard deviations.
    process_std : sequence of float, optional
        Standard deviation of the per-interval state disturbance, per state
        (default: 1e-3 of each state's nominal value).
    estimate_params : sequence of str
        Free parameters estimated along with the states.
    prior_std, param_prior_std : sequence of float, optional
        Standard deviations of the arrival cost on the initial state and parameters.
    """

    def __init__(
        self,
        model: Model,
        N: int,
        dt: float,
        measurements: Sequence[str | ca.SX],
        meas_std: Sequence[float],
        process_std: Sequence[float] | None = None,
        estimate_params: Sequence[str] = (),
        prior_std: Sequence[float] | None = None,
        param_prior_std: Sequence[float] | None = None,
        degree: int = 3,
        **ocp_options,
    ):
        self.model = m = model
        self.N, self.dt = N, dt
        self.measurements = list(measurements)
        self.estimate_params = list(estimate_params)
        self.ocp = ocp = OCP(
            m,
            N=N,
            T=N * dt,
            degree=degree,
            free_params=self.estimate_params,
            process_noise=True,
            fix_initial_state=False,
            **ocp_options,
        )
        opti = ocp.opti
        ny, nx, npe = len(self.measurements), m.nx, len(self.estimate_params)
        self.y = opti.parameter(ny, N + 1)
        self.u_known = opti.parameter(m.nu, N) if m.nu else None
        self.x_prior = opti.parameter(nx)
        self.p_prior = opti.parameter(npe) if npe else None
        opti.set_value(self.x_prior, m.x0)
        self.p_est0 = np.array([m.p0[m.p_index(n)] for n in self.estimate_params])
        if npe:
            opti.set_value(self.p_prior, self.p_est0)

        r = _std(meas_std, ny, 1.0)
        q = _std(process_std, nx, 0.0)
        q = np.where(q > 0, q, 1e-3 * ocp.x_nom)
        s0 = _std(prior_std, nx, 0.0)
        s0 = np.where(s0 > 0, s0, ocp.x_nom)
        sp = _std(param_prior_std, npe, 0.0) if npe else np.zeros(0)
        sp = np.where(sp > 0, sp, np.abs(self.p_est0) + 1e-9) if npe else sp

        J = 0
        for k in range(N + 1):
            for i, name in enumerate(self.measurements):
                J += ((ocp.at(name, k) - self.y[i, k]) / r[i]) ** 2
        for w in ocp.Ws:
            J += ca.sumsqr(w * ca.DM(ocp.x_nom / q))
        J += ca.sumsqr((ocp.X[0] - self.x_prior) / ca.DM(s0))
        if npe:
            pe = ca.vertcat(*[ocp.param(n) for n in self.estimate_params])
            J += ca.sumsqr((pe - self.p_prior) / ca.DM(sp))
            self._pe = pe
        else:
            self._pe = None
        if m.nu:
            for k in range(N):
                opti.subject_to(ocp.U[k] == self.u_known[:, k])
        ocp.minimize(J)
        self.solution: Solution | None = None
        self.solver_options: dict = {"ipopt.max_iter": 300}

    def bound_param(self, name: str, lb: float | None = None, ub: float | None = None) -> None:
        self.ocp.bound(name, lb, ub)

    def step(
        self,
        y: np.ndarray,
        t0: float,
        u: np.ndarray | None = None,
    ) -> tuple[np.ndarray, np.ndarray]:
        """Estimate from a window of measurements ``y`` (shape ``(N + 1, ny)``) starting at ``t0``.

        Returns the state estimate at the end of the window and the parameter estimates.
        """
        ocp, opti, m = self.ocp, self.ocp.opti, self.model
        y = np.asarray(y, dtype=float).reshape(self.N + 1, len(self.measurements))
        opti.set_value(self.y, y.T)
        opti.set_value(ocp.t0, t0)
        if m.nu:
            ua = np.asarray(u if u is not None else np.tile(m.u0, (self.N, 1)), dtype=float)
            opti.set_value(self.u_known, ua.reshape(self.N, m.nu).T)
        sol = ocp.solve(self.solver_options)
        self.solution = sol
        # Arrival cost for the next window: the estimate one interval later.
        opti.set_value(self.x_prior, sol.x[1])
        p_est = np.array([sol.param(n) for n in self.estimate_params])
        if self.p_prior is not None:
            opti.set_value(self.p_prior, p_est)
        self._warm_start(sol)
        return sol.x[-1].copy(), p_est

    def _warm_start(self, sol: Solution) -> None:
        ocp, opti, s = self.ocp, self.ocp.opti, sol.sol
        N = ocp.N
        for g in (ocp.Xs, ocp.Zs, ocp.Ws):
            if not g or g[0].numel() == 0:
                continue
            vals = [s.value(v) for v in g]
            for k in range(len(g)):
                opti.set_initial(g[k], vals[min(k + 1, len(g) - 1)])
        for g2 in (ocp.Xc, ocp.Zc):
            if not g2 or not g2[0] or g2[0][0].numel() == 0:
                continue
            vals = [[s.value(v) for v in row] for row in g2]
            for k in range(N):
                for j in range(ocp.degree):
                    opti.set_initial(g2[k][j], vals[min(k + 1, N - 1)][j])
        if ocp.P_free.numel():
            opti.set_initial(ocp.P_free, s.value(ocp.P_free))

    def run(self, t: np.ndarray, y: np.ndarray, u: np.ndarray | None = None):
        """Estimate over a whole record, one window per new sample.

        Returns ``(t_est, x_est, p_est)``: the estimates at the end of each window.
        """
        t = np.asarray(t, dtype=float)
        y = np.asarray(y, dtype=float).reshape(len(t), len(self.measurements))
        ts, xs, ps = [], [], []
        for k in range(self.N, len(t)):
            uk = None if u is None else np.asarray(u)[k - self.N : k]
            x_est, p_est = self.step(y[k - self.N : k + 1], t[k - self.N], uk)
            ts.append(t[k])
            xs.append(x_est)
            ps.append(p_est)
        return np.array(ts), np.array(xs), np.array(ps)
