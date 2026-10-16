"""Tests of daedalus.sciml: derivatives, missing physics, symbolic regression, surrogates."""

from pathlib import Path

import numpy as np
import pytest

import daedalus
from daedalus import sciml

ROOT = Path(__file__).resolve().parents[3]
LIBRARY = ROOT / "library" / "Daedalus"
PKG = "Daedalus.Examples.SciML"


@pytest.fixture(scope="module")
def truth():
    return daedalus.Model(str(LIBRARY), f"{PKG}.MotorTruth")


@pytest.fixture(scope="module")
def known():
    return daedalus.Model(str(LIBRARY), f"{PKG}.MotorKnown")


@pytest.fixture(scope="module")
def data(truth):
    return truth.simulate(vars=["w", "i", "tau_f", "u"])


def test_derivatives_match_the_equations(known):
    w = np.array([0.0, 10.0, -50.0])
    i = np.array([0.5, -1.0, 2.0])
    r = known.derivatives(0.3, {"w": w, "i": i}, inputs={"tau_missing": [0.0, 1e-3, -2e-3]}, read=["u"])
    p = known.parameters()
    u = r["u"]
    assert u.shape == (3,)
    assert np.allclose(u, 6 * np.sin(2 * np.pi * 0.4 * 0.3) + 3 * np.sin(2 * np.pi * 1.3 * 0.3))
    assert np.allclose(r["der(i)"], (u - p["R"] * i - p["k"] * w) / p["L"])
    tau = np.array([0.0, 1e-3, -2e-3])
    assert np.allclose(r["der(w)"], (p["k"] * i - p["b"] * w - tau) / p["J"])


def test_derivatives_array_form_and_params(known):
    x = np.array([[0.1, 2.0], [0.2, 3.0]])  # columns in known.states order
    r = known.derivatives([0.0, 0.0], x, params={"J": 1e-3})
    k = dict(zip(known.states, x.T))
    assert np.allclose(r["der(w)"], (0.05 * k["i"] - 1e-5 * k["w"]) / 1e-3)


def test_smooth_derivative():
    t = np.linspace(0, 2, 401)
    d = sciml.smooth_derivative(t, np.sin(3 * t))
    assert np.max(np.abs(d - 3 * np.cos(3 * t))) < 1e-3


def test_missing_physics_and_sindy(known, data):
    t = data.time
    tau = sciml.missing_term(known, t, {"w": data["w"], "i": data["i"]}, "tau_missing")
    inner = slice(10, -10)
    assert np.sqrt(np.mean((tau - data["tau_f"])[inner] ** 2)) < 1e-4
    theta, terms = sciml.candidate_library({"w": data["w"]}, degree=2, tanh_scales=(0.5, 2.0, 8.0))
    fit = sciml.sindy(theta[inner], tau[inner], terms, threshold=0.1)
    assert set(fit.active) == {"tanh(w/2.0)", "w*abs(w)"}
    assert fit.active["tanh(w/2.0)"] == pytest.approx(4e-3, rel=1e-2)
    assert fit.active["w*abs(w)"] == pytest.approx(2e-7, rel=1e-2)
    assert fit.r2 > 0.999
    expr = fit.expression(prefix="m.", digits=3)
    assert "tanh(m.w/2.0)" in expr and "m.w*abs(m.w)" in expr


def test_discovered_model_reproduces_the_data(data):
    m = daedalus.Model(str(LIBRARY), f"{PKG}.MotorDiscovered")
    r = m.simulate(vars=["m.w"])
    assert np.max(np.abs(r["m.w"] - np.interp(r.time, data.time, data["w"]))) < 0.05 * np.ptp(data["w"])


def test_sindy_exact_polynomial():
    rng = np.random.default_rng(1)
    x = rng.uniform(-2, 2, 300)
    y = rng.uniform(-2, 2, 300)
    theta, terms = sciml.candidate_library({"x": x, "y": y}, degree=3, sign=False, abs_products=False)
    fit = sciml.sindy(theta, 1.5 * x * y - 0.5 * y**3 + 2.0, terms)
    assert fit.active == pytest.approx({"x*y": 1.5, "y*y*y": -0.5, "1": 2.0})


def test_latin_hypercube_strata():
    s = sciml.latin_hypercube({"a": (0, 1), "b": (10, 20)}, 50, seed=3)
    for k, (lo, hi) in {"a": (0, 1), "b": (10, 20)}.items():
        cells = np.floor((s[k] - lo) / (hi - lo) * 50).astype(int)
        assert sorted(cells) == list(range(50))


def test_mlp_fits_a_smooth_function():
    rng = np.random.default_rng(0)
    X = rng.uniform(-1, 1, (400, 2))
    Y = np.column_stack([np.sin(2 * X[:, 0]) * X[:, 1], X[:, 0] ** 2])
    net = sciml.MLP(2, 2, (24, 24))
    net.fit(X, Y, epochs=2000)
    assert np.max(np.abs(net.predict(X) - Y)) < 0.05


def test_surrogate_and_modelica_export(truth, tmp_path):
    def qoi(r):
        return {"w_peak": float(np.max(r["w"])), "i_rms": float(np.sqrt(np.mean(r["i"] ** 2)))}

    s = sciml.fit_surrogate(
        truth, {"Tc": (0.0, 1e-2), "J": (1e-4, 4e-4)}, qoi, n=60, epochs=1500, simulate={"vars": ["w", "i"]}
    )
    assert s.metrics["w_peak"]["r2"] > 0.99 and s.metrics["i_rms"]["r2"] > 0.99
    ref = qoi(truth.simulate(params={"Tc": 4e-3, "J": 2e-4}, vars=["w", "i"]))
    pred = s.predict(Tc=4e-3, J=2e-4)
    assert float(pred["w_peak"]) == pytest.approx(ref["w_peak"], rel=1e-2)
    mo = tmp_path / "Surr.mo"
    mo.write_text(s.to_modelica("Surr"))
    m = daedalus.Model(mo, "Surr")
    r = m.simulate(stop=0.01, params={"Tc": 4e-3, "J": 2e-4})
    assert r.final("w_peak") == pytest.approx(float(pred["w_peak"]), rel=1e-9)
    assert r.final("i_rms") == pytest.approx(float(pred["i_rms"]), rel=1e-9)
