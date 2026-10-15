"""Load a DAEdalus CasADi export into CasADi symbolic expressions.

The export (``daedalus export-casadi``) is the model after alias elimination,
index reduction and tearing, written as a semi-explicit index-1 DAE::

    der(x) = f(t, x, z, u, p)
         0 = g(t, x, z, u, p)

Every other model variable is an expression of ``t, x, z, u, p`` and can be
looked up by its Modelica name with :meth:`Model.var`.
"""

from __future__ import annotations

import json
import math
from collections.abc import Callable, Iterable, Sequence
from pathlib import Path

import casadi as ca
import numpy as np

FORMAT = "daedalus-casadi"
VERSION = 1


def _num(x) -> float:
    if x is None:
        return -math.inf
    if isinstance(x, str):
        return {"nan": math.nan, "inf": math.inf, "-inf": -math.inf}[x]
    return float(x)


def _trunc(a):
    return ca.sign(a) * ca.floor(ca.fabs(a))


_F1: dict[str, Callable] = {
    "sin": ca.sin,
    "cos": ca.cos,
    "tan": ca.tan,
    "asin": ca.asin,
    "acos": ca.acos,
    "atan": ca.atan,
    "sinh": ca.sinh,
    "cosh": ca.cosh,
    "tanh": ca.tanh,
    "exp": ca.exp,
    "log": ca.log,
    "log10": ca.log10,
    "sqrt": ca.sqrt,
    "abs": ca.fabs,
    "sign": ca.sign,
    "floor": ca.floor,
    "ceil": ca.ceil,
    "not": ca.logic_not,
}

_F2: dict[str, Callable] = {
    "atan2": ca.atan2,
    "min": ca.fmin,
    "max": ca.fmax,
    "div": lambda a, b: _trunc(a / b),
    "mod": lambda a, b: a - ca.floor(a / b) * b,
    "rem": lambda a, b: a - _trunc(a / b) * b,
    "and": ca.logic_and,
    "or": ca.logic_or,
}

_CMP: dict[str, Callable] = {
    "<": lambda a, b: a < b,
    "<=": lambda a, b: a <= b,
    ">": lambda a, b: a > b,
    ">=": lambda a, b: a >= b,
    "==": lambda a, b: a == b,
    "<>": lambda a, b: a != b,
}


class Variable:
    """One model variable as exported: name, kind, unit and initialized value."""

    __slots__ = ("definition", "description", "index", "kind", "max", "min", "name", "nominal", "unit", "value")

    def __init__(self, index: int, d: dict):
        self.index = index
        self.name: str = d["name"]
        self.kind: str = d["kind"]
        self.unit: str = d.get("unit", "")
        self.description: str = d.get("description", "")
        self.value: float = _num(d.get("value", 0.0))
        self.min: float = _num(d["min"]) if "min" in d else -math.inf
        self.max: float = _num(d["max"]) if "max" in d else math.inf
        self.nominal: float | None = _num(d["nominal"]) if "nominal" in d else None
        self.definition = d["def"]

    def __repr__(self) -> str:
        return f"Variable({self.name!r}, {self.kind}, value={self.value:g})"


class Model:
    """A DAEdalus model as CasADi expressions.

    Attributes
    ----------
    t, x, z, u, p : casadi.SX
        Time, states, algebraic (tearing) variables, inputs and free parameters.
    ode, alg : casadi.SX
        Right-hand side ``der(x)`` and the algebraic residuals ``0 = alg``.
    x_names, z_names, u_names, p_names : list[str]
    x0, z0, u0, p0 : numpy.ndarray
        Values after DAEdalus' own initialization (consistent initial point).
    """

    def __init__(self, doc: dict, smooth: float | None = None, smooth_if: bool = False):
        if doc.get("format") != FORMAT:
            raise ValueError("not a DAEdalus CasADi export")
        if doc.get("version", 0) > VERSION:
            raise ValueError(f"export version {doc['version']} is newer than this package ({VERSION})")
        self.doc = doc
        self.smooth = smooth
        self.smooth_if = smooth_if and bool(smooth)
        self.name: str = doc["model"]
        self.experiment: dict = doc.get("experiment") or {}
        self.start_time: float = float(doc.get("start_time", 0.0))
        self.warnings: list[str] = list(doc.get("warnings", []))
        self.initialized: bool = bool(doc.get("initialized", False))
        self.variables = [Variable(i, v) for i, v in enumerate(doc["variables"])]
        self._by_name = {v.name: v for v in self.variables}
        self._nodes = doc["nodes"]
        self._tables = doc.get("tables", [])

        states, algs = doc["states"], doc["algebraics"]
        inputs, params = doc["inputs"], doc["parameters"]
        self.x_names = [self.variables[i].name for i in states]
        self.z_names = [self.variables[i].name for i in algs]
        self.u_names = [self.variables[i].name for i in inputs]
        self.p_names = [self.variables[i].name for i in params]

        def sym(name, n):
            return ca.SX.sym(name, n) if n else ca.SX(0, 1)

        self.t = ca.SX.sym("t")
        self.x = sym("x", len(states))
        self.z = sym("z", len(algs))
        self.u = sym("u", len(inputs))
        self.p = sym("p", len(params))
        self._leaf = {"x": self.x, "z": self.z, "u": self.u, "p": self.p}

        self.x0 = np.array([self.variables[i].value for i in states], dtype=float)
        self.z0 = np.array([self.variables[i].value for i in algs], dtype=float)
        self.u0 = np.array([self.variables[i].value for i in inputs], dtype=float)
        self.p0 = np.array([self.variables[i].value for i in params], dtype=float)

        self._node_val: list = [None] * len(self._nodes)
        self._var_val: list = [None] * len(self.variables)
        self._table_fns: dict = {}

        self.ode = ca.vertcat(*[self._var(i) for i in doc["derivatives"]]) if states else ca.SX(0, 1)
        self.alg = ca.vertcat(*[self._node(k) for k in doc["residuals"]]) if doc["residuals"] else ca.SX(0, 1)

    # ------------------------------------------------------------------ loading
    @classmethod
    def load(cls, path: str | Path, smooth: float | None = None, smooth_if: bool = False) -> Model:
        with open(path) as f:
            return cls(json.load(f), smooth=smooth, smooth_if=smooth_if)

    # --------------------------------------------------------------- dimensions
    @property
    def nx(self) -> int:
        return self.x.numel()

    @property
    def nz(self) -> int:
        return self.z.numel()

    @property
    def nu(self) -> int:
        return self.u.numel()

    @property
    def np(self) -> int:
        return self.p.numel()

    # ---------------------------------------------------------------- variables
    def variable(self, name: str) -> Variable:
        try:
            return self._by_name[name]
        except KeyError:
            raise KeyError(f"no variable '{name}' in {self.name}") from None

    def var(self, name: str) -> ca.SX:
        """Expression of a model variable in terms of ``t, x, z, u, p``."""
        return self._var(self.variable(name).index)

    def __getitem__(self, name: str) -> ca.SX:
        return self.var(name)

    def names(self, kinds: Iterable[str] | None = None) -> list[str]:
        """Variable names, optionally restricted to the given kinds."""
        ks = set(kinds) if kinds is not None else None
        return [v.name for v in self.variables if ks is None or v.kind in ks]

    def p_index(self, name: str) -> int:
        try:
            return self.p_names.index(name)
        except ValueError:
            raise KeyError(f"'{name}' is not a free parameter of {self.name}") from None

    def params(self, **overrides: float) -> np.ndarray:
        """Default parameter vector with some values replaced (``dots`` as ``__``)."""
        p = self.p0.copy()
        for k, v in overrides.items():
            p[self.p_index(k.replace("__", "."))] = v
        return p

    # ---------------------------------------------------------------- functions
    @property
    def args(self) -> list[ca.SX]:
        return [self.t, self.x, self.z, self.u, self.p]

    def function(self, exprs: Sequence[str | ca.SX] | str | ca.SX, name: str = "y") -> ca.Function:
        """``Function(t, x, z, u, p) -> [values]`` of variable names or expressions."""
        if isinstance(exprs, (str, ca.SX)):
            exprs = [exprs]
        out = ca.vertcat(*[self.var(e) if isinstance(e, str) else e for e in exprs])
        return ca.Function(name, self.args, [out], ["t", "x", "z", "u", "p"], [name])

    def dae_function(self) -> ca.Function:
        """``Function(t, x, z, u, p) -> (ode, alg)``."""
        return ca.Function("dae", self.args, [self.ode, self.alg], ["t", "x", "z", "u", "p"], ["ode", "alg"])

    def jacobians(self) -> ca.Function:
        """Linearization ``(t, x, z, u, p) -> (A, B)`` of the ODE after eliminating ``z``."""
        fx, fz, fu = (ca.jacobian(self.ode, a) for a in (self.x, self.z, self.u))
        if self.nz:
            gx, gz, gu = (ca.jacobian(self.alg, a) for a in (self.x, self.z, self.u))
            gzi = ca.inv(gz)
            A = fx - fz @ gzi @ gx
            B = fu - fz @ gzi @ gu
        else:
            A, B = fx, fu
        return ca.Function("linearize", self.args, [A, B], ["t", "x", "z", "u", "p"], ["A", "B"])

    # ---------------------------------------------------------------- building
    def _var(self, i: int):
        if self._var_val[i] is None:
            self._resolve_var(i)
        return self._var_val[i]

    def _node(self, k: int):
        if self._node_val[k] is None:
            self._resolve(k)
        return self._node_val[k]

    def _resolve_var(self, i: int):
        d = self.variables[i].definition
        if d[0] == "expr":
            self._var_val[i] = self._node(d[1])
        elif d[0] == "value":
            self._var_val[i] = ca.SX(self.variables[i].value)
        else:
            self._var_val[i] = self._leaf[d[0]][d[1]]

    def _var_deps(self, i: int) -> list[int]:
        d = self.variables[i].definition
        return [d[1]] if d[0] == "expr" else []

    def _resolve(self, root: int):
        nodes, val = self._nodes, self._node_val
        stack = [root]
        limit = 4 * len(nodes) + 16
        while stack:
            if len(stack) > limit:
                raise RuntimeError("cyclic definition in the CasADi export")
            k = stack[-1]
            if val[k] is not None:
                stack.pop()
                continue
            n = nodes[k]
            op = n[0]
            if op == "var":
                vi = n[1]
                if self._var_val[vi] is None:
                    deps = [d for d in self._var_deps(vi) if val[d] is None]
                    if deps:
                        stack.extend(deps)
                        continue
                    self._resolve_var(vi)
                val[k] = self._var_val[vi]
                stack.pop()
                continue
            ch = self._children(n)
            missing = [c for c in ch if val[c] is None]
            if missing:
                stack.extend(missing)
                continue
            val[k] = self._build(n)
            stack.pop()

    @staticmethod
    def _children(n) -> list[int]:
        op = n[0]
        if op in ("const", "time", "var", "pre"):
            return []
        if op in ("neg",):
            return [n[1]]
        if op in ("add", "sub", "mul", "div", "pow"):
            return [n[1], n[2]]
        if op == "f1":
            return [n[2]]
        if op in ("f2", "cmp"):
            return [n[2], n[3]]
        if op == "if":
            return [n[1], n[2], n[3]]
        if op in ("interp", "interp_der"):
            return [n[3]]
        raise ValueError(f"unknown node '{op}'")

    def _build(self, n):
        v = self._node_val
        op = n[0]
        if op == "const":
            return ca.SX(_num(n[1]))
        if op == "time":
            return self.t
        if op == "pre":
            # pre() of a discrete variable: its value after initialization (no events).
            return ca.SX(self.variables[n[1]].value)
        if op == "neg":
            return -v[n[1]]
        if op == "add":
            return v[n[1]] + v[n[2]]
        if op == "sub":
            return v[n[1]] - v[n[2]]
        if op == "mul":
            return v[n[1]] * v[n[2]]
        if op == "div":
            return v[n[1]] / v[n[2]]
        if op == "pow":
            return v[n[1]] ** v[n[2]]
        if op == "f1":
            if self.smooth and n[1] in ("abs", "sign"):
                a, e2 = v[n[2]], self.smooth**2
                return ca.sqrt(a * a + e2) if n[1] == "abs" else a / ca.sqrt(a * a + e2)
            return _F1[n[1]](v[n[2]])
        if op == "f2":
            if self.smooth and n[1] in ("min", "max"):
                a, b = v[n[2]], v[n[3]]
                r = ca.sqrt((a - b) ** 2 + self.smooth**2)
                return 0.5 * (a + b + r) if n[1] == "max" else 0.5 * (a + b - r)
            return _F2[n[1]](v[n[2]], v[n[3]])
        if op == "cmp":
            return _CMP[n[1]](v[n[2]], v[n[3]])
        if op == "if":
            c = self._nodes[n[1]]
            if self.smooth_if and c[0] == "cmp" and c[1] in ("<", "<=", ">", ">="):
                a, b = v[c[2]], v[c[3]]
                d = (b - a) if c[1] in ("<", "<=") else (a - b)
                w = 0.5 * (1 + ca.tanh(d / self.smooth))
                return w * v[n[2]] + (1 - w) * v[n[3]]
            return ca.if_else(v[n[1]], v[n[2]], v[n[3]])
        if op == "interp":
            return self._table_fn(n[1], n[2], False)(v[n[3]])
        if op == "interp_der":
            return self._table_fn(n[1], n[2], True)(v[n[3]])
        raise ValueError(f"unknown node '{op}'")

    def _table_fn(self, t: int, col: int, der: bool) -> ca.Function:
        key = (t, col, der)
        if key not in self._table_fns:
            xs = ca.SX.sym("x")
            y = table_expr(self._tables[t], col, xs)
            out = ca.jacobian(y, xs) if der else y
            self._table_fns[key] = ca.Function(f"table{t}_{col}{'_der' if der else ''}", [xs], [out])
        return self._table_fns[key]

    def __repr__(self) -> str:
        return (
            f"<daedalus_casadi.Model {self.name}: {self.nx} states, {self.nz} algebraic, "
            f"{self.nu} inputs, {self.np} parameters>"
        )


def table_expr(tab: dict, col: int, x):
    """Interpolate column ``col`` of an exported table at ``x`` (CasADi expression).

    Follows the DAEdalus runtime: linear, constant or cubic Hermite segments,
    hold / linear / periodic extrapolation, ``shift`` and ``start``.
    """
    rows, cols = tab["rows"], tab["cols"]
    data = np.asarray(tab["data"], dtype=float).reshape(rows, cols)
    xs, ys = data[:, 0], data[:, col]
    smooth = tab.get("smoothness", 0)
    extrap = tab.get("extrapolation", 0)
    start = _num(tab.get("start"))
    slopes = np.asarray(tab.get("slopes") or [], dtype=float)
    slopes = slopes.reshape(rows, cols)[:, col] if slopes.size else None
    if rows == 0:
        return ca.SX(0.0) * x
    if rows == 1:
        y = ca.SX(ys[0]) + 0 * x
        return y if start == -math.inf else ca.if_else(x < start, 0, y)
    xi = x - tab.get("shift", 0.0)
    x0, x1 = xs[0], xs[-1]
    if extrap == 2 and x1 > x0:
        xi = x0 + ca.fmod(ca.fmod(xi - x0, x1 - x0) + (x1 - x0), x1 - x0)
    if extrap == 0:
        xi = ca.fmin(ca.fmax(xi, x0), x1)
    n = rows
    if smooth == 1:
        y = ca.SX(ys[0])
        for k in range(1, n):
            y = y + (ys[k] - ys[k - 1]) * (xi >= xs[k])
    elif smooth == 2 and slopes is not None:
        y = ca.SX(0.0)
        for k in range(n - 1):
            a, b = xs[k], xs[k + 1]
            h = b - a
            if h <= 0:
                continue
            w = (xi - a) / h
            seg = (
                (2 * w**3 - 3 * w**2 + 1) * ys[k]
                + (w**3 - 2 * w**2 + w) * h * slopes[k]
                + (-2 * w**3 + 3 * w**2) * ys[k + 1]
                + (w**3 - w**2) * h * slopes[k + 1]
            )
            lo = (xi >= a) if k > 0 else ca.SX(1)
            hi = (xi < b) if k < n - 2 else ca.SX(1)
            y = y + ca.if_else(ca.logic_and(lo, hi), seg, 0)
        if extrap == 1:
            y = ca.if_else(xi < x0, ys[0] + slopes[0] * (xi - x0), y)
            y = ca.if_else(xi > x1, ys[-1] + slopes[-1] * (xi - x1), y)
    else:
        # Sum of clamped ramps: exact for linear segments, with jumps at repeated abscissae.
        y = ca.SX(ys[0])
        for k in range(n - 1):
            a, b = xs[k], xs[k + 1]
            if b > a:
                s = (ys[k + 1] - ys[k]) / (b - a)
                lo = xi if k == 0 and extrap == 1 else ca.fmax(xi, a)
                hi = b if not (k == n - 2 and extrap == 1) else None
                r = (lo if hi is None else ca.fmin(lo, hi)) - a
                y = y + s * r
            else:
                y = y + (ys[k + 1] - ys[k]) * (xi >= a)
    if start != -math.inf:
        y = ca.if_else(x < start, 0, y)
    return y


def load(path: str | Path, smooth: float | None = None, smooth_if: bool = False) -> Model:
    """Load an export written by ``daedalus export-casadi``.

    ``smooth`` replaces the kinks of ``min``, ``max``, ``abs`` and ``sign`` by smooth
    approximations of width ``smooth`` (e.g. ``max(a, b) ~ (a + b + sqrt((a - b)^2 + smooth^2)) / 2``).
    Saturations and dead zones then no longer stall gradient-based optimizers at an
    optimum that sits on a kink. With ``smooth_if`` the branches of ``if a < b`` are
    also blended with ``tanh((b - a) / smooth)``; both branches are then evaluated,
    so use it only where both are defined everywhere.
    """
    return Model.load(path, smooth=smooth, smooth_if=smooth_if)
