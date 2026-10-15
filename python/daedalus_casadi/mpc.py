"""Model predictive control on a DAEdalus model.

An :class:`MPC` is an :class:`~daedalus_casadi.ocp.OCP` over a fixed receding
horizon whose initial state and start time are parameters. Build the cost and
constraints once on ``mpc.ocp``; each :meth:`MPC.step` then solves from the
measured state, warm-started from the previous solution shifted by one interval,
and returns the first input.
"""

from __future__ import annotations

import time
from collections.abc import Callable, Sequence

import numpy as np

from .model import Model
from .ocp import OCP, Solution
from .simulate import Trajectory, consistent_z, simulate


class MPC:
    """Receding-horizon controller.

    Parameters
    ----------
    model : Model
    N : int
        Intervals in the prediction horizon.
    dt : float
        Interval length (the control period).
    degree : int
        Collocation points per interval.
    """

    def __init__(self, model: Model, N: int, dt: float, degree: int = 3, **ocp_options):
        self.model = model
        self.N, self.dt = N, dt
        self.ocp = OCP(model, N=N, T=N * dt, degree=degree, **ocp_options)
        self.solution: Solution | None = None
        self.solve_time = 0.0
        self.solver_options: dict = {"ipopt.max_iter": 200, "ipopt.warm_start_init_point": "yes"}
        self._built = False

    # Convenience pass-throughs to the underlying OCP.
    def minimize(self, J) -> None:
        self.ocp.minimize(J)

    def subject_to(self, *c) -> None:
        self.ocp.subject_to(*c)

    def bound(self, name: str, lb: float | None = None, ub: float | None = None) -> None:
        self.ocp.bound(name, lb, ub)

    def step(self, x: Sequence[float], t: float) -> np.ndarray:
        """Solve from state ``x`` at time ``t``; returns the input for the next interval."""
        ocp, opti = self.ocp, self.ocp.opti
        ocp.set_initial_state(x)
        opti.set_value(ocp.t0, t)
        if self.solution is not None:
            self._shift_guess()
        t_wall = time.perf_counter()
        sol = ocp.solve(self.solver_options)
        self.solve_time = time.perf_counter() - t_wall
        self.solution = sol
        return sol.u[0].copy()

    def _shift_guess(self) -> None:
        """Warm start: previous solution moved one interval forward (last interval repeated)."""
        ocp, opti, s = self.ocp, self.ocp.opti, self.solution.sol
        N = ocp.N
        groups = [ocp.Xs, ocp.Zs]
        for g in groups:
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
        if ocp.Us and ocp.Us[0].numel():
            vals = [s.value(v) for v in ocp.Us]
            for k in range(N):
                opti.set_initial(ocp.Us[k], vals[min(k + 1, N - 1)])
        lam = s.value(opti.lam_g)
        opti.set_initial(opti.lam_g, lam)

    def run(
        self,
        t_end: float,
        x0: Sequence[float] | None = None,
        t0: float | None = None,
        plant: Callable[[np.ndarray, np.ndarray, float], np.ndarray] | None = None,
        p_plant: Sequence[float] | None = None,
        callback: Callable[[int, float, np.ndarray, np.ndarray], None] | None = None,
    ) -> Trajectory:
        """Closed loop from ``t0`` to ``t_end``.

        The plant is the model itself simulated by CasADi's integrator (optionally
        with other parameter values ``p_plant``, to test robustness), unless a
        ``plant(x, u, t) -> x_next`` function is given.
        """
        m = self.model
        t = m.start_time if t0 is None else t0
        x = m.x0.copy() if x0 is None else np.asarray(x0, dtype=float)
        n = round((t_end - t) / self.dt)
        ts = t + self.dt * np.arange(n + 1)
        xs, us = [x.copy()], []
        for k in range(n):
            u = self.step(x, ts[k])
            us.append(u)
            if plant is not None:
                x = np.asarray(plant(x, u, ts[k]), dtype=float)
            else:
                x = simulate(m, ts[k : k + 2], u=u[None, :], p=p_plant, x0=x).x[-1]
            xs.append(x.copy())
            if callback is not None:
                callback(k, ts[k], x, u)
        # The closed loop as it ran: the states the plant produced, with the inputs
        # held over each interval (the last one repeated at the final time).
        us = np.array(us).reshape(n, m.nu)
        u_mesh = np.vstack([us, us[-1:]]) if n else np.tile(m.u0, (1, 1))
        p = m.p0 if p_plant is None else np.asarray(p_plant, dtype=float)
        zs = consistent_z(m, ts, np.array(xs), u_mesh, p)
        return Trajectory(m, ts, np.array(xs), zs, u_mesh, p)
