"""Run the ``daedalus`` command line tool from Python."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path

from .model import Model

_REPO = Path(__file__).resolve().parents[2]


def daedalus_binary() -> str:
    """The ``daedalus`` executable: ``$DAEDALUS_BIN``, then ``PATH``, then this checkout's build."""
    env = os.environ.get("DAEDALUS_BIN")
    if env:
        return env
    found = shutil.which("daedalus")
    if found:
        return found
    for build in ("release", "debug"):
        p = _REPO / "target" / build / "daedalus"
        if p.exists():
            return str(p)
    raise FileNotFoundError("daedalus executable not found: build it with `cargo build --release` or set DAEDALUS_BIN")


def bundled_library() -> str:
    """The bundled Modelica library ``Daedalus`` of this checkout (or ``$DAEDALUS_LIB``)."""
    env = os.environ.get("DAEDALUS_LIB")
    if env:
        return env
    return str(_REPO / "library" / "Daedalus")


def _sources(sources: Sequence[str | Path] | None) -> list[str]:
    return [str(s) for s in sources] if sources else [bundled_library()]


def _params(params: Mapping[str, float] | None) -> list[str]:
    out: list[str] = []
    for k, v in (params or {}).items():
        out += ["-p", f"{k}={v!r}"]
    return out


def export(
    model: str,
    sources: Sequence[str | Path] | None = None,
    output: str | Path | None = None,
    params: Mapping[str, float] | None = None,
    smooth: float | None = None,
    smooth_if: bool = False,
) -> Model:
    """Translate ``model`` with DAEdalus and load it for CasADi.

    ``sources`` are ``.mo`` files or library directories (default: the bundled
    ``Daedalus`` library). ``params`` override parameter values before the export,
    which also changes the initialization the export carries. The JSON document is
    kept at ``output`` when given.
    """
    if output:
        out = Path(output)
    else:
        fd, name = tempfile.mkstemp(suffix=".json")
        os.close(fd)
        out = Path(name)
    cmd = [daedalus_binary(), "export-casadi", *_sources(sources), "-m", model, "-o", str(out), *_params(params)]
    r = subprocess.run(cmd, capture_output=True, text=True, check=False)
    if r.returncode != 0:
        raise RuntimeError(f"daedalus export-casadi failed:\n{r.stderr.strip()}")
    try:
        return Model.load(out, smooth=smooth, smooth_if=smooth_if)
    finally:
        if output is None:
            out.unlink(missing_ok=True)


def simulate_csv(
    model: str,
    variables: Sequence[str],
    sources: Sequence[str | Path] | None = None,
    params: Mapping[str, float] | None = None,
    stop: float | None = None,
    interval: float | None = None,
):
    """Simulate with DAEdalus itself; returns ``(t, {name: values})``."""
    import numpy as np

    with tempfile.TemporaryDirectory() as d:
        csv = Path(d) / "result.csv"
        cmd = [daedalus_binary(), "simulate", *_sources(sources), "-m", model, "-o", str(csv)]
        cmd += ["--vars", ",".join(variables), "--store-events", "none", *_params(params)]
        if stop is not None:
            cmd += ["--stop", repr(stop)]
        if interval is not None:
            cmd += ["--interval", repr(interval)]
        r = subprocess.run(cmd, capture_output=True, text=True, check=False)
        if r.returncode != 0:
            raise RuntimeError(f"daedalus simulate failed:\n{r.stderr.strip()}")
        lines = csv.read_text().splitlines()
    header = [h.strip().strip('"') for h in lines[0].split(",")]
    data = np.array([[float(x) for x in line.split(",")] for line in lines[1:]])
    return data[:, 0], {h: data[:, i] for i, h in enumerate(header) if i > 0}
