"""Tests for the CasADi integration. They need a built ``daedalus`` binary."""

import sys
from pathlib import Path

import numpy as np
import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
import daedalus_casadi as dc
from daedalus_casadi.model import table_expr

try:
    dc.daedalus_binary()
except FileNotFoundError:  # pragma: no cover
    pytest.skip("daedalus binary not built", allow_module_level=True)

OC = "Daedalus.Examples.OptimalControl."


@pytest.fixture(scope="module")
def aileron():
    return dc.export(OC + "AileronServo")


def test_initial_point_is_consistent(aileron):
    m = aileron
    ode, alg = m.dae_function()(m.start_time, m.x0, m.z0, m.u0, m.p0)
    derivs = np.array([m.variables[i].value for i in m.doc["derivatives"]])
    np.testing.assert_allclose(np.array(ode).ravel(), derivs, rtol=1e-6, atol=1e-6)
    assert np.max(np.abs(np.array(alg))) < 1e-3
    # Every variable evaluates to the value DAEdalus initialized it to.
    names = [v.name for v in m.variables]
    vals = np.array(m.function(names)(m.start_time, m.x0, m.z0, m.u0, m.p0)).ravel()
    ref = np.array([v.value for v in m.variables])
    np.testing.assert_allclose(vals, ref, rtol=1e-6, atol=1e-6)


@pytest.mark.parametrize(
    "model,variables,stop",
    [
        ("Daedalus.Examples.AileronActuator.PositionControl", ["delta_deg", "act.valve.x"], 2.5),
        (OC + "ActiveSuspension", ["body.s", "a_body", "travel"], 1.0),
        ("Daedalus.Examples.TorqueConverter.StallTest", ["engine.w"], 3.0),
    ],
)
def test_casadi_simulation_matches_daedalus(model, variables, stop):
    t, ref = dc.simulate_csv(model, variables, stop=stop, interval=stop / 100)
    m = dc.export(model)
    tr = dc.simulate(m, t)
    for v in variables:
        scale = max(np.max(np.abs(ref[v])), 1e-9)
        assert np.max(np.abs(tr[v] - ref[v])) / scale < 2e-3, v


def test_tables():
    import casadi as ca

    tab = {
        "rows": 4,
        "cols": 2,
        "data": [0, 0, 1, 1, 1, 3, 2, 2],
        "extrapolation": 0,
        "smoothness": 0,
        "spline": [],
        "shift": 0.0,
        "start": None,
    }
    x = ca.SX.sym("x")
    f = ca.Function("f", [x], [table_expr(tab, 1, x)])
    # linear, a jump at the repeated abscissa (right value holds), hold outside
    for xi, yi in [(-1, 0), (0.5, 0.5), (0.999, 0.999), (1, 3), (1.5, 2.5), (2, 2), (5, 2)]:
        assert float(f(xi)) == pytest.approx(yi)
    tab["extrapolation"] = 1
    f = ca.Function("f", [x], [table_expr(tab, 1, x)])
    assert float(f(-1)) == pytest.approx(-1)
    assert float(f(3)) == pytest.approx(1)
    # constant segments extrapolate linearly from the edge segments
    tab["smoothness"] = 1
    f = ca.Function("f", [x], [table_expr(tab, 1, x)])
    assert float(f(0.5)) == pytest.approx(0)
    assert float(f(-1)) == pytest.approx(-1)
    assert float(f(3)) == pytest.approx(1)


def test_spline_tables():
    import casadi as ca

    # y = x^2 as a cubic spline: on segment k, y = y_k + (v + 2 x_k) v, v = x - x_k
    xs = [0.0, 1.0, 2.0, 3.0]
    tab = {
        "rows": 4,
        "cols": 2,
        "data": [v for xk in xs for v in (xk, xk * xk)],
        "extrapolation": 1,
        "smoothness": 2,
        "spline": [v for xk in xs[:-1] for v in (0.0, 0.0, 0.0, 0.0, 1.0, 2 * xk)],
        "shift": 0.0,
        "start": None,
    }
    x = ca.SX.sym("x")
    y = table_expr(tab, 1, x)
    f = ca.Function("f", [x], [y, ca.jacobian(y, x), ca.jacobian(ca.jacobian(y, x), x)])
    for xi in [0.3, 1.5, 2.9]:
        v, d, d2 = (float(o) for o in f(xi))
        assert (v, d, d2) == pytest.approx((xi * xi, 2 * xi, 2.0))
    # linear extrapolation with the end slopes
    assert float(f(-1)[0]) == pytest.approx(0.0)
    assert float(f(4)[0]) == pytest.approx(9.0 + 6.0)


def test_minimum_time_ocp():
    m = dc.export(OC + "AileronServo", smooth=1e-3)
    ocp = dc.OCP(m, N=20, free_time=True, T_guess=0.06)
    ocp.bound("u", -1, 1)
    ocp.subject_to(ocp.final("delta_deg") == 10, ocp.final("aileron.w") == 0)
    import casadi as ca

    u = ocp.input("u")
    ocp.minimize(ocp.T + 1e-4 * ca.sumsqr(u[:, 1:] - u[:, :-1]))
    sol = ocp.solve()
    assert 0.035 < sol.T < 0.05
    assert sol["delta_deg"][-1] == pytest.approx(10, abs=1e-6)
    assert np.all(np.abs(sol.u) <= 1 + 1e-6)
    # the command drives the exact model to the target too
    exact = dc.export(OC + "AileronServo")
    check = dc.simulate(exact, sol.t, u=sol.u)
    assert check["delta_deg"][-1] == pytest.approx(10, abs=0.1)


def test_mpc_reduces_body_acceleration():
    m = dc.export(OC + "ActiveSuspension")
    mpc = dc.MPC(m, N=20, dt=0.01, degree=2)
    o = mpc.ocp
    mpc.bound("F_act", -2500, 2500)
    mpc.minimize(
        o.integral(m["a_body"] ** 2) + 10 * o.integral((m["travel"] / 0.03) ** 2) + o.integral((m["F_act"] / 2500) ** 2)
    )
    seen = []

    def plant(x, u, t):  # the model with a heavier body: the result must be this loop
        x = dc.simulate(m, [t, t + 0.01], u=u[None, :], p=m.params(m_body=440), x0=x).x[-1]
        seen.append(x)
        return x

    closed = mpc.run(0.5, plant=plant)
    np.testing.assert_allclose(closed.x[1:], np.array(seen))
    passive = dc.simulate(m, closed.t, u=np.zeros((len(closed.t) - 1, 1)))
    rms = lambda y: np.sqrt(np.mean(y**2))
    assert rms(closed["a_body"]) < 0.8 * rms(passive["a_body"])
    assert np.max(np.abs(closed["F_act"])) <= 2500 + 1e-6


def test_mhe_recovers_parameters():
    model = OC + "CoastDownTest"
    truth = {"body.Cd": 0.33, "body.crr": 0.012}
    t, true = dc.simulate_csv(model, ["v_kmh"], params=truth, stop=60, interval=1.0)
    y = true["v_kmh"] + 0.1 * np.random.default_rng(0).standard_normal(len(t))
    m = dc.export(model)
    mhe = dc.MHE(
        m,
        N=40,
        dt=1.0,
        measurements=["v_kmh"],
        meas_std=[0.1],
        process_std=[1e-3, 0.05],
        estimate_params=list(truth),
        prior_std=[1, 1],
        param_prior_std=[0.05, 0.005],
    )
    _, x, p = mhe.run(t, y)
    assert p[-1, 0] == pytest.approx(0.33, abs=0.01)
    assert p[-1, 1] == pytest.approx(0.012, abs=0.002)
    assert x[-1, m.x_names.index("body.v")] * 3.6 == pytest.approx(true["v_kmh"][-1], abs=0.3)
