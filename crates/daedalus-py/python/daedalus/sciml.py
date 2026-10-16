"""Scientific machine learning with DAEdalus models, in NumPy.

The ideas JuliaHub sells as "SciML" for Dyad, on the Python stack engineers already have
and on the Modelica models they already own:

* **Neural surrogates** (:func:`fit_surrogate`): sample parameters, simulate in parallel,
  fit a small multilayer perceptron from parameters to quantities of interest, check it
  on held-out runs, and export it as a Modelica block (:meth:`MLP.to_modelica`) that
  simulates in DAEdalus and compiles to embedded C with ``daedalus codegen-c``.
* **Missing-physics discovery** (:func:`missing_term`): the known physics is a Modelica
  model with a top-level input standing for the unknown term; from measured states the
  term's values along the data follow from the model's right-hand side
  (:meth:`daedalus.Model.derivatives`), the same "universal differential equation"
  question answered by collocation instead of backpropagation through a solver.
* **Symbolic regression** (:func:`sindy`): sparse regression over a library of candidate
  terms (SINDy, sequentially thresholded least squares) turns the recovered term into a
  short formula that goes back into the model as an equation.

Everything here needs only NumPy; :class:`MLP` is small and trains in seconds on a CPU.
"""

from __future__ import annotations

import math
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass, field
from typing import Any, Callable, Dict, List, Mapping, Optional, Sequence, Tuple

import numpy as np

__all__ = [
    "latin_hypercube",
    "MLP",
    "Surrogate",
    "fit_surrogate",
    "smooth_derivative",
    "missing_term",
    "candidate_library",
    "SparseFit",
    "sindy",
]


# -- sampling ------------------------------------------------------------------------


def latin_hypercube(
    bounds: Mapping[str, Tuple[float, float]], n: int, seed: int = 0
) -> Dict[str, np.ndarray]:
    """``n`` Latin hypercube samples of the parameters in ``bounds`` (name: (lo, hi))."""
    rng = np.random.default_rng(seed)
    out = {}
    for name, (lo, hi) in bounds.items():
        u = (rng.permutation(n) + rng.random(n)) / n
        out[name] = lo + (hi - lo) * u
    return out


# -- multilayer perceptron -----------------------------------------------------------


class MLP:
    """A fully connected network with tanh hidden layers and a linear output layer,
    trained with Adam on standardised inputs and outputs (mean squared error)."""

    def __init__(self, n_in: int, n_out: int, hidden: Sequence[int] = (32, 32), seed: int = 0):
        rng = np.random.default_rng(seed)
        sizes = [n_in, *hidden, n_out]
        self.W = [rng.normal(0.0, 1.0 / math.sqrt(a), (b, a)) for a, b in zip(sizes[:-1], sizes[1:])]
        self.b = [np.zeros(b) for b in sizes[1:]]
        self.x_mean = np.zeros(n_in)
        self.x_std = np.ones(n_in)
        self.y_mean = np.zeros(n_out)
        self.y_std = np.ones(n_out)

    @property
    def sizes(self) -> List[int]:
        return [self.W[0].shape[1]] + [w.shape[0] for w in self.W]

    def _forward(self, Z: np.ndarray):
        acts = [Z]
        h = Z
        for k, (W, b) in enumerate(zip(self.W, self.b)):
            h = h @ W.T + b
            if k < len(self.W) - 1:
                h = np.tanh(h)
            acts.append(h)
        return acts

    def predict(self, X: np.ndarray) -> np.ndarray:
        X = np.atleast_2d(np.asarray(X, dtype=float))
        Z = (X - self.x_mean) / self.x_std
        return self._forward(Z)[-1] * self.y_std + self.y_mean

    def fit(
        self,
        X: np.ndarray,
        Y: np.ndarray,
        epochs: int = 3000,
        lr: float = 3e-3,
        batch: Optional[int] = None,
        weight_decay: float = 0.0,
        seed: int = 0,
    ) -> List[float]:
        """Train on rows of ``X`` and ``Y``; returns the loss per epoch (standardised MSE)."""
        X = np.atleast_2d(np.asarray(X, dtype=float))
        Y = np.asarray(Y, dtype=float).reshape(len(X), -1)
        self.x_mean, self.x_std = X.mean(0), X.std(0) + 1e-12
        self.y_mean, self.y_std = Y.mean(0), Y.std(0) + 1e-12
        Z = (X - self.x_mean) / self.x_std
        T = (Y - self.y_mean) / self.y_std
        params = self.W + self.b
        m = [np.zeros_like(p) for p in params]
        v = [np.zeros_like(p) for p in params]
        b1, b2, eps = 0.9, 0.999, 1e-8
        rng = np.random.default_rng(seed)
        n = len(Z)
        bs = batch or n
        hist = []
        step = 0
        for ep in range(epochs):
            order = rng.permutation(n) if bs < n else np.arange(n)
            for s in range(0, n, bs):
                idx = order[s : s + bs]
                acts = self._forward(Z[idx])
                err = acts[-1] - T[idx]
                g = 2.0 * err / err.size
                gW: List[np.ndarray] = [None] * len(self.W)  # type: ignore[list-item]
                gb: List[np.ndarray] = [None] * len(self.b)  # type: ignore[list-item]
                for k in range(len(self.W) - 1, -1, -1):
                    gW[k] = g.T @ acts[k] + weight_decay * self.W[k]
                    gb[k] = g.sum(0)
                    if k > 0:
                        g = (g @ self.W[k]) * (1.0 - acts[k] ** 2)
                grads = gW + gb
                step += 1
                cur_lr = lr * (0.5 * (1 + math.cos(math.pi * ep / epochs)) * 0.9 + 0.1)
                for i, (p, gr) in enumerate(zip(params, grads)):
                    m[i] = b1 * m[i] + (1 - b1) * gr
                    v[i] = b2 * v[i] + (1 - b2) * gr * gr
                    mh = m[i] / (1 - b1**step)
                    vh = v[i] / (1 - b2**step)
                    p -= cur_lr * mh / (np.sqrt(vh) + eps)
            hist.append(float(np.mean((self._forward(Z)[-1] - T) ** 2)))
        return hist

    def to_modelica(
        self,
        name: str,
        inputs: Sequence[str],
        outputs: Sequence[str],
        description: str = "Neural surrogate (generated by daedalus.sciml)",
    ) -> str:
        """The network as a Modelica block with one Real input per network input and one
        Real output per network output, written with scalar equations only, so that it
        simulates in any Modelica tool and compiles to C with ``daedalus codegen-c``."""

        def ident(s: str) -> str:
            t = "".join(c if c.isalnum() else "_" for c in s).strip("_")
            return t if t and not t[0].isdigit() else "v_" + t

        def num(x: float) -> str:
            return repr(float(x))

        ins = [ident(s) for s in inputs]
        outs = [ident(s) for s in outputs]
        if len(ins) != self.sizes[0] or len(outs) != self.sizes[-1]:
            raise ValueError("names do not match the network's inputs and outputs")
        lines = [f'block {ident(name)} "{description}"']
        for s, raw in zip(ins, inputs):
            lines.append(f'  input Real {s} "{raw}";')
        for s, raw in zip(outs, outputs):
            lines.append(f'  output Real {s} "{raw}";')
        def lin(coefs: np.ndarray, xs: Sequence[str], bias: float) -> str:
            # sum c*x + bias, with the signs folded in (Modelica has no "+ -x")
            out = ""
            for c, x in zip(coefs, xs):
                c = float(c)
                if not out:
                    out = f"{num(c)}*{x}"
                else:
                    out += f" {'-' if c < 0 else '+'} {num(abs(c))}*{x}"
            b = float(bias)
            if not out:
                return num(b)
            return out + f" {'-' if b < 0 else '+'} {num(abs(b))}"

        layers = len(self.W)
        lines.append("protected")
        lines.append(f"  Real z[{len(ins)}] \"normalised inputs\";")
        for k in range(layers - 1):
            lines.append(f"  Real h{k}[{self.W[k].shape[0]}] \"hidden layer {k + 1}\";")
        lines.append("equation")
        for j, (s, mu, sd) in enumerate(zip(ins, self.x_mean, self.x_std)):
            lines.append(f"  z[{j + 1}] = ({s} - ({num(mu)}))/{num(sd)};")
        prev = [f"z[{j + 1}]" for j in range(len(ins))]
        for k in range(layers):
            W, b = self.W[k], self.b[k]
            cur = []
            for i in range(W.shape[0]):
                pre = lin(W[i], prev, b[i])
                if k < layers - 1:
                    lines.append(f"  h{k}[{i + 1}] = tanh({pre});")
                    cur.append(f"h{k}[{i + 1}]")
                else:
                    lines.append(f"  {outs[i]} = ({num(self.y_mean[i])}) + ({num(self.y_std[i])})*({pre});")
            prev = cur
        lines.append(f"end {ident(name)};")
        return "\n".join(lines) + "\n"


# -- surrogates ----------------------------------------------------------------------


@dataclass
class Surrogate:
    """A neural surrogate of a model: parameters in, quantities of interest out."""

    params: List[str]
    outputs: List[str]
    mlp: MLP
    X: np.ndarray
    Y: np.ndarray
    test: np.ndarray
    metrics: Dict[str, Dict[str, float]] = field(default_factory=dict)

    def predict(self, values: Optional[Mapping[str, Any]] = None, **kw: Any) -> Dict[str, np.ndarray]:
        """Predict for parameter values (scalars or equal-length arrays)."""
        v = dict(values or {})
        v.update(kw)
        cols = np.broadcast_arrays(*[np.asarray(v[p], dtype=float) for p in self.params])
        X = np.column_stack([c.ravel() for c in cols])
        Y = self.mlp.predict(X)
        return {o: Y[:, i].reshape(cols[0].shape) for i, o in enumerate(self.outputs)}

    def to_modelica(self, name: str) -> str:
        return self.mlp.to_modelica(name, self.params, self.outputs)

    def report(self) -> str:
        lines = [f"surrogate of {len(self.outputs)} outputs over {len(self.params)} parameters, "
                 f"{len(self.X)} runs ({len(self.test)} held out)"]
        for o, m in self.metrics.items():
            lines.append(f"  {o}: R2 {m['r2']:.4f}, max error {m['max_abs']:.4g} (range {m['range']:.4g})")
        return "\n".join(lines)


def _qoi_final(outputs: Sequence[str]) -> Callable[[Any], Dict[str, float]]:
    def f(res: Any) -> Dict[str, float]:
        return {o: float(res.final(o)) for o in outputs}

    return f


def fit_surrogate(
    model: Any,
    bounds: Mapping[str, Tuple[float, float]],
    qoi: Any,
    *,
    n: int = 200,
    hidden: Sequence[int] = (32, 32),
    epochs: int = 3000,
    lr: float = 3e-3,
    test_fraction: float = 0.2,
    seed: int = 0,
    workers: Optional[int] = None,
    simulate: Optional[Mapping[str, Any]] = None,
) -> Surrogate:
    """Fit a neural surrogate of ``model`` (a :class:`daedalus.Model`).

    ``bounds`` maps parameters to (low, high); ``qoi`` is a list of variable names (their
    final values are learnt) or a function ``result -> {name: value}`` (overshoot,
    settling time, energy...). ``n`` Latin hypercube runs are simulated in parallel
    threads (the simulator releases the GIL); a fraction is held out to report R² and
    the largest error per output in :attr:`Surrogate.metrics`.
    """
    f = _qoi_final(list(qoi)) if not callable(qoi) else qoi
    samples = latin_hypercube(bounds, n, seed)
    names = list(bounds)
    sim_kw = dict(simulate or {})

    def run(i: int) -> Optional[Dict[str, float]]:
        p = {k: float(samples[k][i]) for k in names}
        try:
            return f(model.simulate(params=p, **sim_kw))
        except Exception:  # a failing corner of the design space is left out
            return None

    with ThreadPoolExecutor(max_workers=workers) as ex:
        rows = list(ex.map(run, range(n)))
    ok = [i for i, r in enumerate(rows) if r is not None]
    if len(ok) < 10:
        raise RuntimeError(f"only {len(ok)} of {n} runs succeeded")
    outs = list(rows[ok[0]].keys())  # type: ignore[union-attr]
    X = np.array([[samples[k][i] for k in names] for i in ok])
    Y = np.array([[rows[i][o] for o in outs] for i in ok])  # type: ignore[index]
    rng = np.random.default_rng(seed + 1)
    perm = rng.permutation(len(X))
    n_test = max(1, int(round(test_fraction * len(X))))
    test, train = perm[:n_test], perm[n_test:]
    mlp = MLP(X.shape[1], Y.shape[1], hidden, seed)
    mlp.fit(X[train], Y[train], epochs=epochs, lr=lr, seed=seed)
    P = mlp.predict(X[test])
    metrics = {}
    for j, o in enumerate(outs):
        y = Y[test, j]
        ss = float(np.sum((y - y.mean()) ** 2)) or 1e-300
        metrics[o] = {
            "r2": 1.0 - float(np.sum((P[:, j] - y) ** 2)) / ss,
            "max_abs": float(np.max(np.abs(P[:, j] - y))),
            "range": float(np.ptp(Y[:, j])),
        }
    return Surrogate(names, outs, mlp, X, Y, test, metrics)


# -- derivatives and missing physics -------------------------------------------------


def smooth_derivative(t: np.ndarray, y: np.ndarray, window: int = 11, order: int = 3) -> np.ndarray:
    """Derivative of noisy samples by local polynomial fits (Savitzky–Golay on any grid)."""
    t = np.asarray(t, dtype=float)
    y = np.asarray(y, dtype=float)
    n = len(t)
    h = max(order + 1, window) // 2
    d = np.empty(n)
    for i in range(n):
        lo, hi = max(0, i - h), min(n, i + h + 1)
        if hi - lo < order + 1:
            lo, hi = max(0, hi - order - 1), min(n, lo + order + 1)
        tt = t[lo:hi] - t[i]
        scale = np.max(np.abs(tt)) or 1.0
        c = np.polyfit(tt / scale, y[lo:hi], min(order, hi - lo - 1))
        d[i] = c[-2] / scale if len(c) >= 2 else 0.0
    return d


def missing_term(
    model: Any,
    t: np.ndarray,
    data: Mapping[str, np.ndarray],
    term: str,
    *,
    inputs: Optional[Mapping[str, Any]] = None,
    params: Optional[Mapping[str, float]] = None,
    derivatives: Optional[Mapping[str, np.ndarray]] = None,
    window: int = 11,
    iterations: int = 3,
) -> np.ndarray:
    """Values of an unknown term along measured data.

    ``model`` holds the known physics, with the unknown term as the top-level input
    ``term``; ``data`` maps every state of the model to its measured trajectory at
    times ``t`` (``derivatives`` optionally gives their time derivatives, otherwise they
    are estimated with :func:`smooth_derivative`). At each point the term is the value
    for which the model's derivatives best match the measured ones (least squares over
    the states, a few Gauss–Newton steps), so the result can be fed to :func:`sindy`
    or used to train a network.
    """
    t = np.asarray(t, dtype=float)
    states = model.states
    missing = [s for s in states if s not in data]
    if missing:
        raise ValueError(f"no data for the states {missing}")
    X = np.column_stack([np.asarray(data[s], dtype=float) for s in states])
    if derivatives is None:
        dX = np.column_stack([smooth_derivative(t, X[:, i], window) for i in range(X.shape[1])])
    else:
        dX = np.column_stack([np.asarray(derivatives[s], dtype=float) for s in states])
    base_in = dict(inputs or {})
    d = np.zeros(len(t))

    def f(dv: np.ndarray) -> np.ndarray:
        r = model.derivatives(t, X, inputs={**base_in, term: dv}, params=params)
        return np.column_stack([r[f"der({s})"] for s in states])

    for _ in range(iterations):
        f0 = f(d)
        step = 1e-6 * (1.0 + np.abs(d)) + 1e-6 * (np.max(np.abs(d)) + 1.0)
        J = (f(d + step) - f0) / step[:, None]
        num = np.sum(J * (dX - f0), axis=1)
        den = np.sum(J * J, axis=1)
        delta = np.where(den > 0, num / np.where(den > 0, den, 1.0), 0.0)
        d = d + delta
        if np.max(np.abs(delta)) <= 1e-10 * (1.0 + np.max(np.abs(d))):
            break
    return d


# -- symbolic regression -------------------------------------------------------------


def candidate_library(
    variables: Mapping[str, np.ndarray],
    degree: int = 3,
    *,
    sign: bool = True,
    abs_products: bool = True,
    tanh_scales: Sequence[float] = (),
    constant: bool = True,
) -> Tuple[np.ndarray, List[str]]:
    """Candidate terms for :func:`sindy`: monomials up to ``degree`` in the variables, and
    optionally ``sign(x)``, ``x*abs(x)`` and ``tanh(x/s)`` (Coulomb friction, quadratic
    drag and smooth switching). Returns the matrix (one column per term) and the terms
    as Modelica expressions."""
    names = list(variables)
    cols: List[np.ndarray] = []
    terms: List[str] = []
    n = len(next(iter(variables.values())))
    if constant:
        cols.append(np.ones(n))
        terms.append("1")
    from itertools import combinations_with_replacement

    for deg in range(1, degree + 1):
        for combo in combinations_with_replacement(range(len(names)), deg):
            c = np.ones(n)
            for k in combo:
                c = c * np.asarray(variables[names[k]], dtype=float)
            cols.append(c)
            terms.append("*".join(names[k] for k in combo))
    for v in names:
        x = np.asarray(variables[v], dtype=float)
        if sign:
            cols.append(np.sign(x))
            terms.append(f"sign({v})")
        if abs_products:
            cols.append(x * np.abs(x))
            terms.append(f"{v}*abs({v})")
        for s in tanh_scales:
            cols.append(np.tanh(x / s))
            terms.append(f"tanh({v}/{s!r})")
    return np.column_stack(cols), terms


@dataclass
class SparseFit:
    """A sparse linear combination of candidate terms."""

    terms: List[str]
    coefficients: np.ndarray
    r2: float

    @property
    def active(self) -> Dict[str, float]:
        return {t: float(c) for t, c in zip(self.terms, self.coefficients) if c != 0.0}

    def expression(self, prefix: str = "", digits: int = 6) -> str:
        """The formula as Modelica text; ``prefix`` is put before variable names
        (``"m."`` to use it in a modifier of component ``m``)."""
        import re

        parts = []
        for t, c in self.active.items():
            cs = f"{c:.{digits}g}"
            if t == "1":
                parts.append(cs)
                continue
            tt = re.sub(r"\b([A-Za-z_][A-Za-z_0-9.]*)\b(?!\()", lambda m: prefix + m.group(1), t) if prefix else t
            parts.append(f"{cs}*{tt}")
        s = " + ".join(parts) if parts else "0"
        return s.replace("+ -", "- ")

    def predict(self, theta: np.ndarray) -> np.ndarray:
        return theta @ self.coefficients

    def __str__(self) -> str:
        return f"{self.expression()}   (R2 {self.r2:.5f}, {len(self.active)} of {len(self.terms)} terms)"


def sindy(
    theta: np.ndarray,
    y: np.ndarray,
    terms: Sequence[str],
    *,
    threshold: float = 0.05,
    ridge: float = 1e-8,
    max_iter: int = 25,
) -> SparseFit:
    """Sparse regression ``y ≈ theta @ c`` by sequentially thresholded least squares.

    Columns are normalised first, so ``threshold`` is relative: a term whose contribution
    is below ``threshold`` times that of the largest is dropped and the rest refitted,
    until the set of terms settles."""
    theta = np.asarray(theta, dtype=float)
    y = np.asarray(y, dtype=float)
    scale = np.linalg.norm(theta, axis=0)
    scale[scale == 0] = 1.0
    A = theta / scale
    active = np.ones(A.shape[1], dtype=bool)

    def solve(mask: np.ndarray) -> np.ndarray:
        c = np.zeros(A.shape[1])
        if mask.any():
            Am = A[:, mask]
            c[mask] = np.linalg.solve(Am.T @ Am + ridge * np.eye(mask.sum()), Am.T @ y)
        return c

    c = solve(active)
    for _ in range(max_iter):
        big = np.abs(c) >= threshold * np.max(np.abs(c)) if np.any(c) else active
        new = active & big
        if np.array_equal(new, active):
            break
        active = new
        c = solve(active)
    coef = c / scale
    ss = float(np.sum((y - y.mean()) ** 2)) or 1e-300
    r2 = 1.0 - float(np.sum((theta @ coef - y) ** 2)) / ss
    return SparseFit(list(terms), coef, r2)
