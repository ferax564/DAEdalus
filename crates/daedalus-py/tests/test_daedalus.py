"""Tests of the daedalus Python API (run after `maturin develop`)."""

import math
import os
import shutil
import threading
import zipfile
from pathlib import Path

import numpy as np
import pytest

import daedalus

ROOT = Path(__file__).resolve().parents[3]
MODELS = ROOT / "crates" / "daedalus-core" / "tests" / "models"
LIBRARY = ROOT / "library" / "Daedalus"
STALL = "Daedalus.Examples.TorqueConverter.StallTest"


@pytest.fixture(scope="module")
def ball():
    return daedalus.Model(MODELS / "events.mo", "E.Ball")


@pytest.fixture(scope="module")
def stall():
    return daedalus.Model(str(LIBRARY), STALL)


# -- translation ----------------------------------------------------------------------


def test_version():
    assert isinstance(daedalus.__version__, str) and daedalus.__version__


def test_translate_file(ball):
    assert ball.name == "E.Ball"
    assert sorted(ball.states) == ["h", "v"]
    assert ball.stats["states"] == 2
    assert "Ball" in repr(ball)


def test_translate_library_lazily(stall):
    assert stall.name == STALL
    assert stall.experiment["stop_time"] == 4.0
    assert len(stall.states) == 2


def test_translation_error_unknown_model():
    with pytest.raises(daedalus.TranslationError, match="not found"):
        daedalus.Model(MODELS / "linearize.mo", "Lin.DoesNotExist")


def test_translation_error_bad_source(tmp_path):
    bad = tmp_path / "bad.mo"
    bad.write_text("model Bad\n  Real x;\nequation\n  der(x) = y;\nend Bad;\n")
    with pytest.raises(daedalus.TranslationError) as e:
        daedalus.Model(bad, "Bad")
    assert "y" in str(e.value)
    assert isinstance(e.value, daedalus.DaedalusError)


def test_missing_path():
    with pytest.raises(daedalus.TranslationError, match="no such file"):
        daedalus.Model("/nonexistent/model.mo", "M")


# -- variables and parameters ---------------------------------------------------------


def test_variables(ball):
    vs = {v.name: v for v in ball.variables()}
    assert {"h", "v", "e", "g", "bounces"} <= set(vs)
    assert vs["e"].kind == "parameter" and vs["e"].value == 0.8 and vs["e"].free
    assert vs["h"].state and vs["h"].kind == "continuous" and vs["h"].type == "Real"
    assert vs["bounces"].type == "Integer"
    assert [v.name for v in ball.variables("parameter")] == ["e", "g"]
    assert {v.name for v in ball.variables("state")} == {"h", "v"}


def test_units_and_descriptions(stall):
    vs = {v.name: v for v in stall.variables()}
    assert vs["engine.torqueCurve[1,1]"].description.startswith("Full-load torque")
    assert any(v.unit for v in vs.values())


def test_parameters(ball):
    m = daedalus.Model(MODELS / "events.mo", "E.Ball")
    assert m.parameters() == {"e": 0.8, "g": 9.81}
    m.set_parameters(e=0.5)
    assert m.parameters()["e"] == 0.5
    with pytest.raises(KeyError):
        m.set_parameters(nope=1)
    m.reset_parameters()
    assert m.parameters()["e"] == 0.8


# -- simulation -----------------------------------------------------------------------


def test_simulate_defaults(stall):
    r = stall.simulate()
    assert isinstance(r.time, np.ndarray)
    assert r.time[0] == 0.0 and r.time[-1] == pytest.approx(4.0)
    assert "rpm_engine" in r and "time" in r
    assert r["rpm_engine"].shape == r.time.shape
    assert r.stats["solver"]["steps"] > 0
    assert len(r) == len(r.time)


def test_simulate_options(ball):
    r = ball.simulate(start=0.0, stop=0.4, n=40, tol=1e-8, method="dopri5")
    # 40 intervals: 41 output points (the first bounce is at t = 0.45)
    assert len(r.time) == 41
    t = r.time
    h = r["h"]
    assert np.allclose(h, 1 - 9.81 / 2 * t**2, atol=1e-6)
    assert r.final("v") == pytest.approx(-9.81 * 0.4, rel=1e-6)
    assert r.at("h", 0.25) == pytest.approx(1 - 9.81 / 2 * 0.25**2, abs=1e-4)


def test_simulate_events_and_parameters(ball):
    t_hit = math.sqrt(2 / 9.81)
    r = ball.simulate(stop=1.0, method="bdf")
    assert r.final("bounces") == 1
    assert r.stats["events"] >= 1
    # The bounce speed scales with e: compare two coefficients.
    v1 = r.at("v", t_hit + 0.01)
    r2 = ball.simulate(stop=1.0, params={"e": 0.4})
    v2 = r2.at("v", t_hit + 0.01)
    assert v2 < v1  # slower rebound
    assert r2["e"][0] == 0.4  # parameters are constant columns


def test_set_parameters_affects_simulation():
    m = daedalus.Model(MODELS / "events.mo", "E.Ball")
    m.set_parameters(g=1.0)
    r = m.simulate(stop=0.5)
    assert r.final("v") == pytest.approx(-0.5, rel=1e-6)


def test_vars_selection(stall):
    r = stall.simulate(stop=1.0, vars=["rpm_engine", "rpm_stall_theory"])
    assert r.names == ["rpm_engine", "rpm_stall_theory"]
    with pytest.raises(KeyError):
        stall.simulate(stop=1.0, vars=["nope"])
    with pytest.raises(KeyError):
        r["nope"]


def test_bad_method(ball):
    with pytest.raises(ValueError, match="unknown method"):
        ball.simulate(method="magic")


def test_unknown_parameter_override(ball):
    with pytest.raises(daedalus.SimulationError, match="unknown parameter"):
        ball.simulate(params={"nope": 1.0})


def test_simulation_error_partial(tmp_path):
    src = tmp_path / "t.mo"
    src.write_text(
        "model T\n  Real x(start = 0, fixed = true);\nequation\n  der(x) = 1;\n"
        '  assert(x < 0.5, "x too large");\nend T;\n'
    )
    m = daedalus.Model(src, "T")
    with pytest.raises(daedalus.SimulationError, match="x too large") as e:
        m.simulate(stop=1.0)
    p = e.value.partial
    assert p is None or (len(p.time) > 0 and p.time[-1] <= 0.6)


def test_to_pandas(stall):
    pd = pytest.importorskip("pandas")
    r = stall.simulate(stop=1.0, n=10, vars=["rpm_engine"])
    df = r.to_pandas()
    assert isinstance(df, pd.DataFrame)
    assert list(df.columns) == ["rpm_engine"]
    assert df.index.name == "time"
    assert len(df) == len(r.time)


def test_csv_and_mat_roundtrip(ball, tmp_path):
    r = ball.simulate(stop=1.0)
    csv = tmp_path / "r.csv"
    r.write_csv(csv, ["h", "v"])
    head = csv.read_text().splitlines()[0]
    assert head == 'time,"h","v"'
    mat = tmp_path / "r.mat"
    r.write_mat(mat)
    back = daedalus.read_mat(mat)
    assert np.allclose(back.time, r.time)
    assert np.allclose(back["h"], r["h"])


def test_concurrent_simulations(stall):
    # The GIL is released while simulating: threads run concurrently and agree.
    out = {}

    def run(i):
        out[i] = stall.simulate(stop=2.0).final("rpm_engine")

    ts = [threading.Thread(target=run, args=(i,)) for i in range(4)]
    for t in ts:
        t.start()
    for t in ts:
        t.join()
    assert len(set(out.values())) == 1


# -- linearization --------------------------------------------------------------------


def test_linearize_mass_spring_damper():
    lin = daedalus.Model(MODELS / "linearize.mo", "Lin.MassSpringDamper").linearize()
    assert lin.states == ["x", "v"]
    assert lin.inputs == ["f"]
    assert lin.outputs == ["pos"]
    m, c, k = 2.0, 0.5, 8.0
    assert np.allclose(lin.A, [[0, 1], [-k / m, -c / m]])
    assert np.allclose(lin.B, [[0], [1 / m]])
    assert np.allclose(lin.C, [[1, 0]])
    assert np.allclose(lin.D, [[0]])
    ev = lin.eigenvalues()
    assert np.all(ev.real < 0)
    assert "linearized_model" in lin.to_modelica()
    assert "A (2 x 2)" in str(lin)


def test_linearize_at_time_and_fd():
    m = daedalus.Model(MODELS / "linearize.mo", "Lin.Pendulum")
    exact = m.linearize(at=1.0)
    fd = m.linearize(at=1.0, jacobian="fd")
    assert exact.time == 1.0
    assert exact.A.shape == (2, 2) and exact.C.shape == (2, 2)
    assert np.allclose(exact.A, fd.A, rtol=1e-5, atol=1e-6)
    # d(der(w))/d(phi) = -g/L cos(phi)
    phi = exact.x0[exact.states.index("phi")]
    i = exact.states.index("w")
    j = exact.states.index("phi")
    assert exact.A[i, j] == pytest.approx(-9.81 * math.cos(phi), rel=1e-8)


def test_linearize_parameters():
    m = daedalus.Model(MODELS / "linearize.mo", "Lin.MassSpringDamper")
    m.set_parameters(k=18.0)
    assert m.linearize().A[1, 0] == pytest.approx(-9.0)


# -- FMU ------------------------------------------------------------------------------


def _fmi_runtime():
    env = os.environ.get("DAEDALUS_FMI_LIB")
    if env and Path(env).is_file():
        return Path(env)
    for d in ("release", "debug"):
        p = ROOT / "target" / d / daedalus._runtime_name()
        if p.is_file():
            return p
    return None


def test_export_fmu(tmp_path):
    rt = _fmi_runtime()
    if rt is None:
        pytest.skip("FMI runtime not built (cargo build --release -p daedalus-fmi)")
    m = daedalus.Model(MODELS / "events.mo", "E.Ball")
    out = m.export_fmu(tmp_path / "Ball.fmu", fmi_lib=rt)
    assert out.is_file()
    with zipfile.ZipFile(out) as z:
        names = z.namelist()
        assert "modelDescription.xml" in names
        assert "resources/model.bin" in names
        assert any(n.startswith("binaries/") and "E_Ball" in n for n in names)
        xml = z.read("modelDescription.xml").decode()
        assert "<ModelExchange" in xml and "<CoSimulation" in xml
    fmpy = pytest.importorskip("fmpy")
    res = fmpy.simulate_fmu(str(out), stop_time=0.3, output=["h"])
    assert res["h"][-1] == pytest.approx(1 - 9.81 / 2 * 0.3**2, abs=1e-3)


def test_export_fmu_missing_runtime(tmp_path, monkeypatch):
    monkeypatch.delenv("DAEDALUS_FMI_LIB", raising=False)
    m = daedalus.Model(MODELS / "events.mo", "E.Ball")
    with pytest.raises(daedalus.DaedalusError, match="not found"):
        m.export_fmu(tmp_path / "x.fmu", fmi_lib=tmp_path / "nope.so")


def test_daedalus_lib_env(monkeypatch, tmp_path):
    src = tmp_path / "Use.mo"
    src.write_text(
        "model Use\n  extends Daedalus.Examples.TorqueConverter.StallTest;\nend Use;\n"
    )
    monkeypatch.setenv("DAEDALUS_LIB", str(LIBRARY))
    m = daedalus.Model(src, "Use")
    assert len(m.states) == 2


@pytest.mark.skipif(shutil.which("daedalus") is None, reason="no daedalus CLI on PATH")
def test_matches_cli(tmp_path, stall):
    import subprocess

    out = tmp_path / "cli.csv"
    subprocess.run(
        ["daedalus", "simulate", str(LIBRARY), "-m", STALL, "--stop", "1", "-o", str(out),
         "--vars", "rpm_engine"],
        check=True,
    )
    rows = [l.split(",") for l in out.read_text().splitlines()[1:]]
    cli = np.array([float(r[1]) for r in rows])
    py = stall.simulate(stop=1.0)["rpm_engine"]
    assert np.allclose(cli, py, rtol=1e-12)
