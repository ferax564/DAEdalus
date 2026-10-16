# daedalus (Python)

Python API of [DAEdalus](https://github.com/ferax564/DAEdalus), a fast Modelica
compiler and simulator written in Rust. The compiler runs in-process: no OMC server,
no Dymola licence, no files written unless you ask. It covers what scripts usually do
with OMPython or Dymola's Python interface: load models, change parameters, simulate,
read trajectories as NumPy arrays, linearize, and export FMUs.

```python
import daedalus

m = daedalus.Model("library/Daedalus", "Daedalus.Examples.TorqueConverter.StallTest")
print(m.experiment, m.states)

m.set_parameters({"engine.J": 0.3})          # dotted names: mapping form
r = m.simulate(stop=2.0, n=1000, vars=["rpm_engine", "rpm_stall_theory"])
r.time, r["rpm_engine"]                       # NumPy arrays
df = r.to_pandas()                            # if pandas is installed
r.stats["solver"]["steps"]

lin = daedalus.Model("crates/daedalus-core/tests/models/linearize.mo", "Lin.MassSpringDamper").linearize()
lin.A, lin.B, lin.C, lin.D, lin.states, lin.inputs, lin.outputs
```

## Install

From a checkout (needs a Rust toolchain):

```sh
python -m venv .venv && . .venv/bin/activate
pip install maturin
cd crates/daedalus-py
maturin develop --release          # or: maturin build --release && pip install target/wheels/*.whl
pip install pytest pandas && pytest
```

The wheel is `abi3` (one build for CPython 3.9 and later). The crate is not a member
of the cargo workspace, so plain `cargo build --workspace` does not need Python.

## API

### `daedalus.Model(paths, name, *, index_reduction=True, tearing=True)`

Loads `paths` (a path or list of paths: `.mo` files and library directories, like the
positional arguments of the `daedalus` CLI; a directory with a `package.mo` is parsed
on demand) plus the directories in `$DAEDALUS_LIB`, and translates model `name`.
Failures raise `daedalus.TranslationError` with the compiler's message.

| member | |
|---|---|
| `name`, `states`, `warnings` | model name, selected states, translation warnings |
| `experiment` | the `experiment` annotation as a dict |
| `stats` | translation statistics (as `daedalus check` prints them) |
| `variables(kind=None)` | list of `daedalus.Variable` (name, kind, type, unit, display_unit, description, state, output, protected, final, free, value); `kind` filters: `"parameter"`, `"input"`, `"state"`, `"continuous"`, `"discrete"`, `"constant"` |
| `parameters()` | `{name: value}`: evaluated defaults with overrides applied |
| `set_parameters(mapping=None, **kw)` | override parameters or top-level inputs for later runs (unknown names raise `KeyError`); `reset_parameters()` clears them |
| `simulate(start, stop, n, tol, method, vars, *, interval, step, params, store_events, timeout)` | returns a `Result`; unset options come from the experiment annotation; `method` is `bdf` (default), `radau`, `esdirk`, `dopri5`, `auto`, `rk4` or `euler`; `params` apply to this run only; the GIL is released while it runs |
| `derivatives(t, states, *, inputs, params, read)` | The right-hand side `der(x) = f(t, x, u)` at given points, as a dictionary of arrays: a way to check or plot the model's dynamics without simulating. |
| `linearize(at=None, *, start, tol, method, params, jacobian="exact")` | returns a `Linearization` (after initialization, or after simulating to `at`) |
| `steady(*, at=None, free=None, targets=None, tol=None, method=None, params=None)` | steady state (`der(x) = 0`), as the JSON object the core returns; `free` / `targets` trim |
| `linear_analysis(*, at=None, inputs=None, outputs=None, bode=False, step=False, reduce=None, lqr=None)` | linearize and analyze (poles, zeros, margins); JSON from the core. Needs a top-level `input Real` |
| `export_fmu(path=None, *, fmi_lib=None, fmi_version=2)` | writes an FMI 2.0 or 3.0 FMU (`fmi_version=3`) with Model Exchange and Co-Simulation, and returns its path |

Simulation failures raise `daedalus.SimulationError`; its `partial` attribute holds the
`Result` up to the failure, if any. Both errors derive from `daedalus.DaedalusError`.

### `daedalus.Result`

`time` (NumPy array), `result["x"]`, `names`, `"x" in result`, `len(result)` (rows),
`stats` (solver steps, rejected steps, f-evals, Jacobians, events, timings), `log`,
`final(name)`, `at(name, t)`, `unit(name)`, `description(name)`, `to_dict()`,
`to_pandas()`, `write_csv(path, names=None)`, `write_mat(path)` (Dymola-compatible).
`daedalus.read_mat(path)` reads a `.mat` result file back.

### `daedalus.Linearization`

`A`, `B`, `C`, `D` (2-D NumPy arrays of `der(x) = A x + B u`, `y = C x + D u`),
`states`, `inputs`, `outputs`, the operating point `x0`, `u0`, `y0`, `dx0`, `time`,
`eigenvalues()`, `to_modelica()` (OpenModelica-style `linearized_model`), and
`str(lin)` for the text report of `daedalus linearize`.

### FMU export

The FMU embeds the model-independent FMI runtime library built from
`crates/daedalus-fmi` (`cargo build --release -p daedalus-fmi` gives
`target/release/libdaedalus_fmi.so`). `export_fmu` finds it at `fmi_lib=`,
`$DAEDALUS_FMI_LIB`, inside the installed package, or next to a `daedalus` executable
on `PATH`. `fmi_version=2` (the default) writes an FMI 2.0 FMU; `fmi_version=3` writes FMI 3.0 (`fmiVersion="3.0"` in `modelDescription.xml`).

## Coming from OMPython / Dymola

| OMPython `ModelicaSystem` / Dymola | daedalus |
|---|---|
| `ModelicaSystem(file, "M", ["Modelica"])` / `openModel` + `translateModel` | `Model([file, msl_dir], "M")` |
| `getQuantities()` / `getParameters()` | `variables()` / `parameters()` |
| `setParameters(["a=1"])` | `set_parameters({"a": 1})` |
| `setSimulationOptions(...)`, `simulate()` / `simulateModel` | `simulate(stop=..., n=..., tol=..., method=...)` |
| `getSolutions(["time", "x"])` / `DyMat` | `r.time`, `r["x"]`, `r.to_pandas()`, `read_mat()` |
| `linearize()` / `linearizeModel` | `linearize()` |
| `convertMo2Fmu` / `translateModelFMU` | `export_fmu()` |
