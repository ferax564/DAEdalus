"""DAEdalus: translate, simulate, linearize and export Modelica models from Python.

    >>> import daedalus
    >>> m = daedalus.Model("library/Daedalus", "Daedalus.Examples.TorqueConverter.StallTest")
    >>> r = m.simulate(stop=2.0)
    >>> r.time, r["rpm_engine"]

The compiler runs in-process (a Rust extension module); nothing is written to disk
unless asked for.
"""

from __future__ import annotations

import os
import shutil
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Dict, Iterable, Iterator, List, Mapping, Optional, Sequence, Union

import numpy as np

from . import _daedalus
from ._daedalus import DaedalusError, SimulationError, TranslationError

__version__ = _daedalus.__version__

__all__ = [
    "Model",
    "Result",
    "Linearization",
    "Variable",
    "DaedalusError",
    "TranslationError",
    "SimulationError",
    "read_mat",
    "__version__",
]

PathLike = Union[str, "os.PathLike[str]"]


@dataclass(frozen=True)
class Variable:
    """A model variable as the compiler sees it after translation."""

    name: str
    #: "parameter", "constant", "input", "discrete" or "continuous"
    kind: str
    #: "Real", "Integer", "Boolean" or "enumeration"
    type: str
    unit: str
    display_unit: str
    description: str
    #: a selected state of the integrator
    state: bool
    #: a top-level `output`
    output: bool
    protected: bool
    final: bool
    #: a public parameter with a literal value: overriding it changes the result
    free: bool
    #: value of parameters, constants and inputs (their start value); None otherwise
    value: Optional[float] = None


class Result:
    """A simulation result: the time grid and one trajectory per variable.

    ``result["x"]`` is the trajectory of ``x`` as a NumPy array on ``result.time``
    (parameters are expanded to a constant array).
    """

    def __init__(
        self,
        native: "_daedalus.SimResult",
        stats: Optional[Dict[str, Any]] = None,
        vars: Optional[Sequence[str]] = None,
    ) -> None:
        self._r = native
        #: solver and event statistics (None for a partial result or one read from a file)
        self.stats: Optional[Dict[str, Any]] = stats
        self._info = {n: (k, u, d) for n, k, u, d in native.info()}
        if vars is not None:
            missing = [v for v in vars if v not in self._info]
            if missing:
                raise KeyError(f"no variable(s) {', '.join(missing)} in the results")
            self._names = list(vars)
        else:
            self._names = native.names()
        self._time: Optional[np.ndarray] = None
        self._cache: Dict[str, np.ndarray] = {}

    @property
    def model(self) -> str:
        return self._r.model

    @property
    def time(self) -> np.ndarray:
        if self._time is None:
            self._time = self._r.time()
        return self._time

    @property
    def names(self) -> List[str]:
        """Variable names (the selection given by ``vars=``, if any)."""
        return list(self._names)

    @property
    def log(self) -> List[str]:
        """Messages from the model (``assert`` warnings, ``terminate``, ...)."""
        return self._r.log

    def __getitem__(self, name: str) -> np.ndarray:
        if name in ("time", "Time"):
            return self.time
        a = self._cache.get(name)
        if a is None:
            a = self._r.values(name)
            self._cache[name] = a
        return a

    def __contains__(self, name: object) -> bool:
        return isinstance(name, str) and (name in self._info or name in ("time", "Time"))

    def __iter__(self) -> Iterator[str]:
        return iter(self._names)

    def __len__(self) -> int:
        """Number of rows (output points plus stored event rows)."""
        return len(self._r)

    def __repr__(self) -> str:
        t = self.time
        span = f"t = {t[0]:g} .. {t[-1]:g}" if len(t) else "empty"
        return f"<daedalus.Result {self.model}: {len(self._names)} variables, {len(t)} rows, {span}>"

    def unit(self, name: str) -> str:
        return self._info[name][1]

    def description(self, name: str) -> str:
        return self._info[name][2]

    def final(self, name: str) -> float:
        """Value at the end of the simulation."""
        return self._r.final_value(name)

    def at(self, name: str, t: float) -> float:
        """Linear interpolation at time ``t`` (right value at events)."""
        return self._r.at(name, t)

    def to_dict(self, names: Optional[Iterable[str]] = None) -> Dict[str, np.ndarray]:
        """``{"time": ..., name: ...}`` for ``names`` (default: :attr:`names`)."""
        out = {"time": self.time}
        for n in self._names if names is None else names:
            out[n] = self[n]
        return out

    def to_pandas(self, names: Optional[Iterable[str]] = None):
        """A :class:`pandas.DataFrame` indexed by time (needs pandas)."""
        try:
            import pandas as pd
        except ImportError as e:  # pragma: no cover - depends on the environment
            raise ImportError("Result.to_pandas() needs pandas: pip install pandas") from e
        d = self.to_dict(names)
        t = d.pop("time")
        return pd.DataFrame(d, index=pd.Index(t, name="time"))

    def write_csv(self, path: PathLike, names: Optional[Iterable[str]] = None) -> None:
        """Write a CSV file (``time`` first; default columns: :attr:`names`)."""
        self._r.write_csv(os.fspath(path), list(self._names if names is None else names))

    def write_mat(self, path: PathLike) -> None:
        """Write a Dymola-compatible ``dsres``-style ``.mat`` file (all variables)."""
        self._r.write_mat(os.fspath(path))


def read_mat(path: PathLike) -> Result:
    """Read a Dymola / OpenModelica / DAEdalus ``.mat`` result file."""
    return Result(_daedalus.read_mat(os.fspath(path)))


@dataclass
class Linearization:
    """The linear model ``der(x) = A x + B u``, ``y = C x + D u`` at a solution point."""

    model: str
    time: float
    states: List[str]
    inputs: List[str]
    outputs: List[str]
    A: np.ndarray
    B: np.ndarray
    C: np.ndarray
    D: np.ndarray
    x0: np.ndarray
    u0: np.ndarray
    y0: np.ndarray
    #: state derivatives at the point (zero at an equilibrium)
    dx0: np.ndarray
    #: "exact" (forward-mode tangents) or "fd" (central differences)
    jacobian: str
    _report: str = field(repr=False, default="")
    _modelica: str = field(repr=False, default="")

    def eigenvalues(self) -> np.ndarray:
        """Eigenvalues of ``A``."""
        if self.A.size == 0:
            return np.zeros(0, dtype=complex)
        return np.linalg.eigvals(self.A)

    def to_modelica(self) -> str:
        """The linear model as Modelica text (model ``linearized_model``, OpenModelica style)."""
        return self._modelica

    def __str__(self) -> str:
        return self._report


class Model:
    """A Modelica model loaded from ``paths`` and translated.

    ``paths`` are ``.mo`` files and library directories (a directory holding a
    ``package.mo`` is loaded on demand, like the ``daedalus`` command line). The
    directories in ``$DAEDALUS_LIB`` (``os.pathsep``-separated) are added too.
    Translation errors raise :class:`TranslationError` with the compiler's message.
    """

    def __init__(
        self,
        paths: Union[PathLike, Sequence[PathLike]],
        name: str,
        *,
        index_reduction: bool = True,
        tearing: bool = True,
    ) -> None:
        if isinstance(paths, (str, os.PathLike)):
            paths = [paths]
        srcs = [os.fspath(p) for p in paths]
        for p in srcs:
            if not os.path.exists(p):
                raise TranslationError(f"{p}: no such file or directory")
        env = os.environ.get("DAEDALUS_LIB")
        libs = [d for d in env.split(os.pathsep) if d] if env else []
        self._m = _daedalus.CompiledModel(libs + srcs, name, index_reduction, tearing)
        self._overrides: Dict[str, float] = {}
        self._vars: Optional[List[Variable]] = None

    # -- metadata ---------------------------------------------------------------------

    @property
    def name(self) -> str:
        return self._m.name

    @property
    def warnings(self) -> List[str]:
        """Translation warnings."""
        return self._m.warnings

    @property
    def experiment(self) -> Dict[str, Any]:
        """The ``experiment`` annotation (start_time, stop_time, tolerance, interval, algorithm)."""
        return self._m.experiment

    @property
    def stats(self) -> Dict[str, Any]:
        """Translation statistics (as ``daedalus check`` prints them)."""
        return self._m.stats

    @property
    def states(self) -> List[str]:
        """The states selected by the compiler."""
        return self._m.states

    def __repr__(self) -> str:
        return f"<daedalus.Model {self.name}: {len(self.states)} states>"

    def variables(self, kind: Optional[str] = None) -> List[Variable]:
        """All model variables, or those of one ``kind`` ("parameter", "input", "state",
        "continuous", "discrete", "constant")."""
        if self._vars is None:
            self._vars = [Variable(**d) for d in self._m.variables()]
        if kind is None:
            return list(self._vars)
        if kind == "state":
            return [v for v in self._vars if v.state]
        return [v for v in self._vars if v.kind == kind]

    # -- parameters -------------------------------------------------------------------

    def parameters(self) -> Dict[str, float]:
        """Parameter values: the model's defaults with :meth:`set_parameters` applied."""
        p = self._m.parameter_values()
        p.update({k: v for k, v in self._overrides.items() if k in p})
        return p

    def set_parameters(self, values: Optional[Mapping[str, float]] = None, **kw: float) -> None:
        """Override parameters (or top-level inputs) for later simulations.

        Dotted names need the mapping form: ``m.set_parameters({"spring.c": 1e4}, d=2)``.
        """
        new = dict(values or {}, **kw)
        known = {v.name for v in self.variables() if v.kind in ("parameter", "input")}
        unknown = [k for k in new if k not in known]
        if unknown:
            raise KeyError(f"not a parameter of {self.name}: {', '.join(unknown)}")
        for k, v in new.items():
            self._overrides[k] = float(v)

    def reset_parameters(self) -> None:
        """Forget all :meth:`set_parameters` overrides."""
        self._overrides.clear()

    def _params(self, params: Optional[Mapping[str, float]]) -> List[tuple]:
        p = dict(self._overrides)
        if params:
            p.update({k: float(v) for k, v in params.items()})
        return list(p.items())

    # -- simulation -------------------------------------------------------------------

    def simulate(
        self,
        start: Optional[float] = None,
        stop: Optional[float] = None,
        n: Optional[int] = None,
        tol: Optional[float] = None,
        method: Optional[str] = None,
        vars: Optional[Sequence[str]] = None,
        *,
        interval: Optional[float] = None,
        step: Optional[float] = None,
        params: Optional[Mapping[str, float]] = None,
        store_events: Optional[bool] = None,
        timeout: Optional[float] = None,
    ) -> Result:
        """Simulate; unset options come from the ``experiment`` annotation.

        ``n`` is the number of output intervals (default 500) and ``interval`` the
        output step (overrides ``n``); ``method`` is one of METHODS (``step`` sets
        the fixed step of rk4 and euler); ``vars`` restricts
        :attr:`Result.names`; ``params`` are overrides for this run only, on top of
        :meth:`set_parameters`; ``timeout`` aborts after that many seconds of wall time.
        Failures raise :class:`SimulationError`; its ``partial`` attribute holds the
        result up to the failure (or None). The GIL is released while integrating.
        """
        native, stats, error = self._m.simulate(
            start=start,
            stop=stop,
            n=n,
            interval=interval,
            tol=tol,
            method=method,
            step=step,
            params=self._params(params),
            store_events=store_events,
            max_wall_time=timeout,
        )
        if error is not None:
            exc = SimulationError(error)
            exc.partial = Result(native) if native is not None else None
            raise exc
        return Result(native, stats, vars)

    def linearize(
        self,
        at: Optional[float] = None,
        *,
        start: Optional[float] = None,
        tol: Optional[float] = None,
        method: Optional[str] = None,
        params: Optional[Mapping[str, float]] = None,
        jacobian: str = "exact",
    ) -> Linearization:
        """Linearize right after initialization, or after simulating to time ``at``.

        ``x`` are the states, ``u`` the top-level Real inputs (held at their start
        values) and ``y`` the top-level Real outputs. ``jacobian="fd"`` uses central
        differences instead of exact tangents.
        """
        d = self._m.linearize(
            at=at,
            start=start,
            tol=tol,
            method=method,
            params=self._params(params),
            jacobian=jacobian,
        )
        report = d.pop("report")
        modelica = d.pop("modelica")
        return Linearization(**d, _report=report, _modelica=modelica)

    def derivatives(
        self,
        t: Union[float, Sequence[float], np.ndarray],
        states: Union[Mapping[str, Any], np.ndarray, Sequence[Sequence[float]]],
        *,
        inputs: Optional[Mapping[str, Any]] = None,
        params: Optional[Mapping[str, float]] = None,
        read: Optional[Sequence[str]] = None,
    ) -> Dict[str, np.ndarray]:
        """The model's right-hand side ``der(x) = f(t, x, u)`` at given points.

        ``states`` is an array with one row per point and one column per state (in
        :attr:`states` order) or a mapping from state name to values; ``inputs`` maps
        top-level inputs (or parameters) to a value or one value per point. Discrete
        variables keep their start values. Returns a dict with ``"der(x)"`` for each
        state and the variables in ``read``, each an array over the points: the known
        physics at measured states, for hybrid and data-driven modelling
        (see :mod:`daedalus.sciml`).
        """
        names = self.states
        # The number of points is the longest of t, the states and the inputs.
        sizes = [np.size(t)]
        if isinstance(states, Mapping):
            sizes += [np.size(states[s]) for s in names if s in states]
        else:
            sizes.append(np.asarray(states, dtype=float).size // max(1, len(names)))
        sizes += [np.size(v) for v in (inputs or {}).values()]
        npts = max(sizes)
        tt = np.broadcast_to(np.asarray(t, dtype=float), (npts,))
        if isinstance(states, Mapping):
            missing = [s for s in names if s not in states]
            if missing:
                raise KeyError(f"no value for the states {missing}")
            cols = [np.broadcast_to(np.asarray(states[s], dtype=float), tt.shape) for s in names]
            x = np.column_stack(cols) if cols else np.zeros((npts, 0))
        else:
            x = np.asarray(states, dtype=float).reshape(npts, len(names))
        cols_in = []
        for k, v in (inputs or {}).items():
            cols_in.append((k, [float(a) for a in np.broadcast_to(np.asarray(v, dtype=float), tt.shape)]))
        read = list(read or [])
        der, vals = self._m.rhs(
            [float(a) for a in tt],
            [list(map(float, row)) for row in x],
            inputs=cols_in,
            params=self._params(params),
            read=read,
        )
        out: Dict[str, np.ndarray] = {f"der({s})": der[:, i] for i, s in enumerate(names)}
        for i, r in enumerate(read):
            out[r] = vals[:, i]
        return out

    # -- FMU --------------------------------------------------------------------------

    def steady(
        self,
        *,
        at: Optional[float] = None,
        free: Optional[Sequence[str]] = None,
        targets: Optional[Mapping[str, float]] = None,
        tol: Optional[float] = None,
        method: Optional[str] = None,
        params: Optional[Mapping[str, float]] = None,
    ) -> Dict[str, Any]:
        """Steady state (``der(x) = 0``) as the JSON object the core returns.

        ``at`` simulates to that time first. ``free`` names inputs or parameters to
        solve for, and ``targets`` maps a variable to the value those must achieve
        (one target per free name).
        """
        pairs = list((targets or {}).items())
        return self._m.steady(
            at=at,
            free=list(free or []),
            targets=[(k, float(v)) for k, v in pairs],
            tol=tol,
            method=method,
            params=self._params(params),
        )

    def linear_analysis(
        self,
        *,
        at: Optional[float] = None,
        inputs: Optional[Sequence[str]] = None,
        outputs: Optional[Sequence[str]] = None,
        tol: Optional[float] = None,
        method: Optional[str] = None,
        params: Optional[Mapping[str, float]] = None,
        bode: bool = False,
        step: bool = False,
        reduce: Optional[int] = None,
        lqr: Optional[tuple] = None,
    ) -> Dict[str, Any]:
        """Linearize and analyze; returns the core's JSON object (poles, zeros, margins).

        The model needs a top-level ``input Real``. ``bode`` and ``step`` keep frequency
        and time responses; ``reduce`` is a balanced-truncation order; ``lqr`` is ``(q, r)``.
        """
        lqr_pair = (float(lqr[0]), float(lqr[1])) if lqr is not None else None
        return self._m.linear_analysis(
            at=at,
            inputs=list(inputs or []),
            outputs=list(outputs or []),
            tol=tol,
            method=method,
            params=self._params(params),
            bode=bode,
            step=step,
            reduce=reduce,
            lqr=lqr_pair,
        )

    def export_fmu(
        self,
        path: Optional[PathLike] = None,
        *,
        fmi_lib: Optional[PathLike] = None,
        fmi_version: int = 2,
    ) -> Path:
        """Write an FMI FMU supporting both Model Exchange and Co-Simulation.

        ``fmi_version`` is 2 (default) or 3. The FMU embeds the prebuilt DAEdalus FMI
        runtime (``libdaedalus_fmi.so`` / ``.dylib`` / ``daedalus_fmi.dll``), found at
        ``fmi_lib``, ``$DAEDALUS_FMI_LIB``, inside this package, or next to a
        ``daedalus`` executable on ``PATH``. Build it with
        ``cargo build --release -p daedalus-fmi``. Returns the FMU path.
        """
        out = Path(path) if path is not None else Path(self.name.replace(".", "_") + ".fmu")
        runtime = _find_fmi_runtime(fmi_lib)
        self._m.export_fmu(os.fspath(out), os.fspath(runtime), int(fmi_version))
        return out


# The method list is Method::name() in the extension, so the docstring cannot drift.
Model.simulate.__doc__ = (Model.simulate.__doc__ or "").replace("METHODS", _daedalus.method_names())


def _runtime_name() -> str:
    import sys

    if sys.platform == "win32":
        return "daedalus_fmi.dll"
    if sys.platform == "darwin":
        return "libdaedalus_fmi.dylib"
    return "libdaedalus_fmi.so"


def _find_fmi_runtime(explicit: Optional[PathLike]) -> Path:
    if explicit is not None:
        p = Path(explicit)
        if p.is_file():
            return p
        raise DaedalusError(f"FMI runtime library {p} not found")
    env = os.environ.get("DAEDALUS_FMI_LIB")
    if env:
        p = Path(env)
        if p.is_file():
            return p
        raise DaedalusError(f"DAEDALUS_FMI_LIB={env} not found")
    name = _runtime_name()
    cands = [Path(__file__).with_name(name)]
    exe = shutil.which("daedalus")
    if exe:
        d = Path(exe).resolve().parent
        cands += [d / name, d / "deps" / name]
    for c in cands:
        if c.is_file():
            return c
    raise DaedalusError(
        f"cannot find the FMI runtime {name}: build it with "
        "`cargo build --release -p daedalus-fmi` and pass fmi_lib= or set DAEDALUS_FMI_LIB"
    )
