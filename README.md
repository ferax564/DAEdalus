# DAEdalus

DAEdalus is an open (Apache-2.0) equation-based modeling and simulation
environment for the Modelica language. It is built for speed and scale:

- **Rust core.** It compiles natively and to WebAssembly, so the same compiler and
  solvers run on a server, on the command line and in a browser tab.
- **Symbolic backend** in the spirit of the commercial tools:
  - alias elimination
  - Pantelides index reduction with dummy derivatives
  - BLT sorting and tearing
  - exact symbolic Jacobians
  - a compiled register tape for evaluation
- **Solvers.** It has variable-order BDF, Radau IIA and ESDIRK with exact
  colored Jacobians and dense or sparse LU, Dopri5 with automatic switching to
  BDF when the model turns stiff, and RK4 and Euler, with zero-crossing events
  and `when`/`reinit`. See [Solvers](#solvers).
- **Results.** They are written as Dymola-format `.mat` files, readable by
  Dymola, OMEdit, DyMat and similar tools, or as CSV.
- **Dymola-style workbench.** It runs in the browser and has:
  - a package browser
  - icon, diagram and text views of each class
  - a variable browser
  - a plot window with display units
  - a 3-D animation view of Modelica.Mechanics.MultiBody models
  - simulation setup

All compilation runs in a Web Worker.

- **Modelica Standard Library.** 489 of the 534 MSL 4.1.0 example models
  (92%) translate and simulate, including MultiBody, the QuasiStatic machines,
  `Modelica.Media` (IF97 water, R134a, ideal-gas mixtures, moist air) and
  `Modelica.Fluid` systems.

**Documentation:** https://daedalus.apps.ezkeel.com/docs/ has a getting-started
guide and a page per feature, with example models, screenshots and results. The pages
are the Markdown files in [`docs/`](docs/index.md); `npm run build` in `web/` renders
them to `dist/docs/` (`npm run docs` renders only them).

> Status: under active development. See [docs/architecture.md](docs/architecture.md),
> [docs/examples.md](docs/examples.md) and [docs/roadmap.md](docs/roadmap.md).

## Install

Prebuilt binaries for Linux (x86_64), macOS (Apple Silicon and Intel) and
Windows (x86_64) are attached to each [GitHub Release](https://github.com/ferax564/DAEdalus/releases).
Each archive holds the `daedalus` CLI, the FMI runtime it bundles into FMUs, and
the `library/Daedalus` example library:

```sh
tar xzf daedalus-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd daedalus-v0.1.0-x86_64-unknown-linux-gnu
./daedalus simulate library/Daedalus -m Daedalus.Examples.LandingGear.SinkRates -o sink.mat
```

On macOS, clear the download quarantine first with
`xattr -dr com.apple.quarantine <folder>`, since the binaries are not notarized.
The release also carries the `daedalus-casadi` Python wheel
(`pip install daedalus_casadi-*.whl`). It drives the CLI, so put the extracted
folder on `PATH` (or set `DAEDALUS_BIN` to the executable); it then finds the
`library/Daedalus` shipped next to it, or `DAEDALUS_LIB` if set. Maintainers cut a release by pushing a
tag that matches the workspace version, for example `git tag v0.1.0 && git push origin v0.1.0`.

## Quick start

From source:

```sh
cargo build --release

# Translate and print statistics
./target/release/daedalus check library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest

# Simulate (experiment annotation defaults; override with --stop, --tol, --method, -p name=value)
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.LandingGear.SinkRates -o sink.mat

# CSV of chosen variables
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.QuarterCar.BumpComparison \
    -o bump.csv --vars clean.body.s,aerated.body.s

# Run every model with an experiment annotation, each in its own process
./target/release/daedalus test-library library/Daedalus -m Daedalus

# Export the index-reduced DAE for CasADi (optimal control, MPC, estimation)
./target/release/daedalus export-casadi library/Daedalus -m Daedalus.Examples.OptimalControl.AileronServo -o servo.json

# Linear state-space model der(x) = A x + B u, y = C x + D u around the initial
# point (or after simulating to --at T); --format text | json | mo
./target/release/daedalus linearize model.mo -m MyModel --at 1.5 --format mo -o linearized_model.mo
```

`linearize` takes `u` from the model's top-level Real inputs (held at their start
values, 0 without one, while simulating to `--at`) and `y` from its top-level Real
outputs, like Dymola's `linearizeModel` and OpenModelica's `linearize`. The
matrices are exact: a forward-mode run of the compiled tape per state and input,
with algebraic loops differentiated implicitly through their symbolic Jacobians
(`--jacobian fd` uses central differences instead). `--format mo` writes the model
`linearized_model` in the form OpenModelica writes it.

### Linear analysis

`daedalus linear-analysis` linearizes the model like `linearize` and analyzes the
chosen input → output pairs: poles, transmission zeros, DC gain and relative degree,
Bode and Nyquist data with gain and phase margins, step and impulse responses with
rise time, settling time and overshoot, and PID suggestions (SIMC PI and
Ziegler–Nichols PID from a first-order-plus-dead-time fit of the step response).
`--reduce N` adds balanced truncation (reduced A, B, C, D, Hankel singular values,
the H∞ error bound, and the reduced model's responses for comparison), `--lqr Q,R`
the LQR gain for `Q = q·I`, `R = r·I` with its closed-loop poles. Outputs may be any
continuous variable or state, not only top-level outputs.

```sh
./target/release/daedalus linear-analysis library/Daedalus \
    -m Daedalus.Examples.OptimalControl.ActiveSuspension \
    --inputs F_act --outputs a_body,travel --reduce 2 --lqr 1,1e-6 -o suspension.json
# CSV: Bode data to bode.csv and step responses to bode_step.csv
./target/release/daedalus linear-analysis model.mo -m MyModel --at 2 --bode --step -o bode.csv
```

A text summary goes to stdout; `-o file.json` holds everything, `-o file.csv` the
Bode (`--bode`) and step (`--step`) data. The numerics are in
`crates/daedalus-core/src/lti.rs`, with no extra dependencies: Bode points are `O(n²)`
each after a Householder Hessenberg reduction; step responses are exact for a step
(zero-order hold with the Padé matrix exponential); Lyapunov and Riccati equations are
solved with the matrix sign function; transmission zeros are the zero-dynamics
eigenvalues of a Krylov minimal realization of each pair. Balanced truncation needs a
stable model, and the dense algorithms are meant for up to a few hundred states. In
the workbench, the **Linear** tab of the model view shows the same analysis with input
and output pickers, Bode or Nyquist plot, pole-zero map, step or impulse response and
the summary.

```sh
# Steady state: x with der(x) = 0, from the initial state (or after simulating
# to --at-time T); prints states, derivatives, outputs and the eigenvalues
./target/release/daedalus steady model.mo -m MyModel --at-time 10
# Trim: also solve for inputs/parameters so that variables hit targets
./target/release/daedalus steady model.mo -m MyModel --free u1,u2 --target y1=3,y2=0.5

# Dymola / OpenModelica scripts (simulateModel, simulate, linearizeModel, loadModel, ...)
DAEDALUS_MSL=/path/to/msl ./target/release/daedalus run regression.mos
```

`steady` runs a damped Newton iteration on the state derivatives with the exact
Jacobian of the linearization (Levenberg–Marquardt steps where it is singular).
`run` executes the common scripting commands of both tools so existing
regression scripts run unchanged; see [docs/scripting.md](docs/scripting.md).

### Units

```sh
# every variable's declared or inferred unit, then the unit issues (--json, --all)
./target/release/daedalus units model.mo -m MyModel
# translate as usual and print unit issues as warnings
./target/release/daedalus check model.mo -m MyModel --units
```

The unit checker (`daedalus_core::units`) is a separate pass over the flat model.
It parses `unit` strings with the grammar of MLS 19.1 (`"kg.m2"`, `"rad/s"`,
`"W/(m2.K)"`, `"s-1"`, SI prefixes, plus `degC`, `bar`, `rpm`, `l`, `min`, `h` and the
Imperial `ft`, `in`, `lbf`, `psi`, `mph`, `hp`, `degF`, ...) into the seven SI base
dimensions and a scale. Terms of sums, the sides of equations, `if` branches, relation
operands and `min`/`max` must agree; products combine, `der()` divides by seconds,
`sqrt` halves, and the arguments of `sin`, `exp`, ... must be dimensionless. Functions are
checked through their inlined bodies with the units of the actual arguments. Variables
without a unit get one inferred, and the workbench plots them with it.

Findings are warnings, never errors, and the rules are tuned so that MSL's own idioms
stay quiet: numeric literals are unit-polymorphic (`x + 1`, `1.5*R`), parameters and
variables declared `unit="1"` (gains, ideal-switch and friction path parameters) are
polymorphic, each branch of an if-equation is its own equation, `log` takes any unit, a
clash that involves an inferred unit drops that inference instead of reporting it, and a
clash by exactly the unit of a folded `constant ... unitTime = 1` is ignored. On the 509
MSL 4.1.0 examples that flatten and the 19 `Daedalus` / `MultiBodyVehicle` examples it
reports no issue, and it costs about 1 % of the flattening time. Rotations count as
dimensionless when checking (`rad/s` agrees with `Hz`) but not when converting for display.

In the workbench, **Check** lists the unit issues in the messages panel. Plots show each
variable in its `displayUnit` (else a Dymola-style default: rpm, deg, bar, degC, l/min,
kJ); the plot toolbar picks the unit of the x and y axes from the units that convert, and
**Imperial** switches the defaults to ft, lbf, lbf.ft, psi, mph, degF, hp, gal/min, ...

```sh
# FMI 2.0 FMU (Model Exchange and Co-Simulation) for linux64
cargo build --release -p daedalus-cli -p daedalus-fmi
./target/release/daedalus fmu library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -o StallTest.fmu
# the same as an FMI 3.0 FMU (x86_64-linux)
./target/release/daedalus fmu library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest --fmi-version 3 -o StallTest3.fmu
# validate with FMPy and compare its ME and CS runs with `daedalus simulate`
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -o ref.csv --vars rpm_engine
python3 tools/fmu_check.py StallTest.fmu ref.csv rpm_engine
```

`daedalus fmu` writes `modelDescription.xml` (parameters, inputs, outputs, states,
derivatives and every other variable, with the ModelStructure), the compiled model
in `resources/model.bin`, and the model-independent FMI runtime
`libdaedalus_fmi.so` (crate `daedalus-fmi`, found next to the `daedalus` executable,
or via `--fmi-lib` / `DAEDALUS_FMI_LIB`) as `binaries/linux64/<id>.so`.
Co-Simulation steps with the model's own BDF/Dopri5 solver and event handling;
Model Exchange exposes the states, derivatives and one event indicator per
relation. With `--fmi-version 3` the same runtime implements the `fmi3*` API
(Float64, Int32 and Boolean variables, `fmi3UpdateDiscreteStates`, and exact
directional derivatives from the forward tangents of the compiled equations, after
initialization). FMU state get/set is not provided.

### FMU and SSP import

```sh
# simulate an FMU (FMI 2.0 or 3.0) on its own: Model Exchange if it has it, its
# outputs and local variables in the result
./target/release/daedalus simulate StallTest.fmu -o fmu.csv --vars rpm_engine
./target/release/daedalus simulate StallTest.fmu --fmi-type cs --comm-step 0.004 -o cs.csv
# wrapper block, usable as a component in any model (connect its inputs and outputs)
./target/release/daedalus import-fmu Plant.fmu -o Plant.mo [--fmi-type me|cs] [--comm-step H] [--class NAME] [--within PKG]
# SSP: simulate a system of FMUs as a co-simulation master, import it as a model,
# or export a model made of FMU components
./target/release/daedalus simulate System.ssp --comm-step 0.01 -o sys.csv
./target/release/daedalus import-ssp System.ssp -o System.mo
./target/release/daedalus export-ssp . -m Loop -o Loop.ssp
```

The wrapper declares the FMU's parameters, its inputs and outputs as connectors, and
an annotation `__Daedalus_FMU(fileName = "Plant.fmu", fmiType = "ModelExchange")`
(the file relative to the `.mo`). The FMU is unpacked and its binary loaded when the
simulation starts. A Model Exchange FMU's states (`fmu_x`) are integrated together
with the model's, its event indicators are located like the model's own zero
crossings, and its time events and discrete state updates take part in the event
iteration; outputs that depend directly on inputs are iterated to a fixed point.
Every integrator works with FMU components; the implicit ones (BDF, Radau, ESDIRK,
auto) then use a dense difference Jacobian, since the tape does not see inside an FMU.
A Co-Simulation FMU is stepped over the communication grid `t0 + k*communicationStepSize`
with its inputs held over each step; its outputs change at the grid points (a
signal through a chain of Co-Simulation FMUs is delayed one step per FMU, so the
error is first order in the step). `import-ssp` reads `SystemStructure.ssd` (FMU
components, parameter bindings inline or in `.ssv` files, connections, system
connectors, default experiment) and writes one model with a wrapper class per FMU;
nested systems are not supported. `export-ssp` packs the FMU components of a model,
their parameter values, placements and connections. FMU components need the native
tool (CLI or Python package): the browser workbench reports an error for them, and
`linearize`/`steady` refuse them. Tests: `crates/daedalus-cli/tests/fmu_import.rs`
imports exported FMUs back and compares them with the native simulation.

Other source directories, such as the Modelica Standard Library, are loaded by
passing them as extra sources or with `-L DIR`.

## Model structure, Jacobian and state selection

```sh
# Variable roles, BLT blocks, incidence, state selection and why; plus the state
# Jacobian at the start (or after simulating to --at T)
./target/release/daedalus structure model.mo -m MyModel --at 1 [--format json]

# Choose other states without editing the model (any command; NAME may be a glob)
./target/release/daedalus simulate model.mo -m MyModel --state body.v=prefer --state 'arm.*.phi=avoid'
```

`structure` lists every variable with its role (state, derivative, dummy
derivative, algebraic, alias, parameter, discrete) and the BLT blocks in
evaluation order: which equation computes which unknown, and for linear and
nonlinear loops the tear variables, inner equations and residuals. For each
candidate set of the dummy-derivative state selection it shows which variables
stayed states and which became dummy derivatives, their `stateSelect` tiers
(and overrides), and the reason. DAEdalus selects states once, statically;
when a set's constraint Jacobian depends on the solution, `structure` checks
the choice along the simulated trajectory and warns when it degenerates (a
pendulum in cartesian coordinates with `y` as state, at the bottom of the
swing), naming the states that would work there.

The Jacobian part gives the exact state Jacobian `df/dx` (the `linearize`
machinery) with its sparsity and 1-norm condition number, the eigenvalues
with time constants, frequencies, damping and participation factors (from left
and right eigenvectors), the stiffness ratio with a stiff or non-stiff solver
suggestion, and the condition number of each algebraic loop's Jacobian with
respect to its tear variables.

`--state NAME=always|prefer|default|avoid|never` sets the `stateSelect` of the
matching variables before alias elimination and index reduction; later
overrides win, and one that matches nothing is reported. In the workbench the
**Structure** tab shows the blocks (click one for its equations), the
incidence matrix in BLT order, a heatmap of `df/dx` and the eigenvalues in
the complex plane (hover for time constant and participating states). Each
state candidate can be pinned or forbidden there; the model is translated
again and the new states, eigenvalues and stiffness are shown next to the old
ones before simulating. The overrides are kept per model for the session, used
by **Simulate**, and **Copy as modifier** gives the equivalent
`x(stateSelect = StateSelect.prefer)` modifications.

## Libraries, packages, diff and parameter data

```sh
# Libraries: the index of open libraries (library/packages.json), install by name,
# GitHub URL or a downloaded zip, into $DAEDALUS_LIBRARIES (default ~/.daedalus/libraries)
./target/release/daedalus lib index
./target/release/daedalus lib install Buildings@12.1.0
./target/release/daedalus lib install https://github.com/owner/MyLib@v1.2.0
./target/release/daedalus lib install ~/Downloads/OpenIPSL-3.0.1.zip
./target/release/daedalus lib list
./target/release/daedalus lib resolve model.mo     # uses(...) -> installed versions, conflicts

# A model with the files and resources (modelica:// URIs) it needs, as a zip; the
# MSL is referred to in the manifest, not copied. A .zip source is opened again.
./target/release/daedalus package library/Daedalus -m Daedalus.Examples.TorqueConverter.Launch -o Launch.zip
./target/release/daedalus simulate Launch.zip -o launch.mat

# Structural diff of a class (components, parameters, modifiers, placements,
# connections, equations); --text adds the unified text diff, --format json
./target/release/daedalus diff old/Plant.mo new/Plant.mo -m Demo.Plant --text

# Parameter data (CSV name,value[,unit] or SSP .ssv) as a modifier; values are checked
# against the component's type and converted to its units (mm -> m, bar -> Pa, ...)
./target/release/daedalus params-from library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -c engine engine.csv
#   engine(J = 0.25, tau_drag = 25, w_start = 104.71975511966)
# --record Name prints a record instead; --apply writes the modifier into the source
```

Every command resolves the `uses(...)` annotations of what it loads against the
installed libraries (exact version, else the newest of the same major version) and
warns about conflicting requirements. `lib install` downloads with `curl` from
GitHub; where that is blocked, download the release zip and install the file.

In the workbench:
- **Libraries** lists the libraries loaded with their versions, `uses` conflicts and
  missing libraries, and installs a library of the index, a GitHub URL or a picked zip.
  Browsers cannot read GitHub's archives (no CORS headers), so the files come from
  jsDelivr's GitHub CDN. Installed libraries are kept in IndexedDB for the next visit.
- **Save .zip** writes the open model as a package; **Open** takes such a zip (or a
  library zip) back.
- **Diff** compares the open class with the version as loaded or a picked file: a
  summary, the text diff, and the diagram with added, changed, moved (dashed old
  place) and removed components and connections highlighted.
- **Documentation** renders `info` and `revisions` HTML sanitized (no scripts, event
  handlers or `javascript:` links), with `modelica://` class links that navigate and
  images from library resources. Documentation that does not start with a tag is
  Markdown with `$...$`/`$$...$$` LaTeX (marked and KaTeX, bundled, so offline).
  The `__Daedalus_metadata(key="value", ...)` annotation is shown and edited there.
- **Import parameters…** in the component inspector reads a CSV or SSV file, shows the
  checked values as a modifier and a record, and applies them to the declaration
  with the diagram editor's modifier edit (undo and redo work as for diagram edits).

## Equation and initialization debugger

```sh
# How is x computed? Equation, BLT block, tearing role and every symbolic
# transformation (alias substitution, differentiation for index reduction,
# dummy derivative, solve-for, tearing), each with component and file:line
./target/release/daedalus debug model.mo -m MyModel --var body.v --var 'der(x)'
./target/release/daedalus debug model.mo -m MyModel --equation 12

# Initialization: unknowns, start values used (fixed / guess, set where),
# blocks, and for a failing block the residuals at the last iterate and the
# variables with the largest Newton update
./target/release/daedalus debug model.mo -m MyModel --init [--format json]
```

Each transformation step names the equation as written, the component
instance it came from and its source position, so a chain reads like
"`r.v = r.p.v - r.n.v` (in r, Resistor.mo:12) → substituted alias
`r.p.v = c.n.v` (connection) → solved for `r.i` in block 7". The start value
of every iteration variable says whether it is fixed or only a guess, which
instance set it (`b(x0 = 2)` at the modification site) and whether it was
inherited from an alias.

When translation fails, `debug` explains the structural problem locally
instead of only counting equations: a matching-based search finds the
smallest under- or over-determined subset and lists its equations and
unknowns with component paths (for example "`leak.w` is not determined by
any equation; component `leak`"). It also warns about physical connectors
that are left unconnected: a component placed in the model none of whose
(two or more) physical connectors is connected, or the open end of a two-pin
electrical component, through which no current can flow. Connectors whose
unconnected flow is simply zero by the language rules (an open output
terminal, a free flange, a lone ground, the disabled thermal ports inside a
machine) are reported as notes only. Over the 521 example models of the
Modelica Standard Library 4.1.0 and the Daedalus library that flatten, this
gives 2 warnings, both real (the `earth` resistor of the transformer examples
whose `p` pin is left open), and no false positive.

`simulate` prints the same initialization report when the initial solve
fails, and when a nonlinear block fails during the run it names the block,
the time, the iteration count and residual norm and the residuals of its
equations. In the workbench the **Structure** tab has a **Transformation
chain** card (click any variable or equation in the blocks or states to see
its chain, positions open the Text view at that line) and an
**Initialization** card; failed checks and simulations in the messages panel
have a link that opens it on the failing model.

## Python

The `daedalus` Python package (`crates/daedalus-py`, PyO3 + maturin, one abi3
wheel for CPython 3.9+) runs the compiler in-process, in place of OMPython or
Dymola's Python interface:

```sh
pip install maturin && cd crates/daedalus-py && maturin develop --release
```

```python
import daedalus

m = daedalus.Model("library/Daedalus", "Daedalus.Examples.TorqueConverter.StallTest")
m.set_parameters({"engine.J": 0.3})
r = m.simulate(stop=2.0, vars=["rpm_engine"])   # GIL released while simulating
r.time, r["rpm_engine"], r.to_pandas(), r.stats

lin = daedalus.Model("crates/daedalus-core/tests/models/linearize.mo", "Lin.MassSpringDamper").linearize()
lin.A, lin.B, lin.C, lin.D

m.export_fmu("StallTest.fmu", fmi_lib="target/release/libdaedalus_fmi.so")
```

See [crates/daedalus-py/README.md](crates/daedalus-py/README.md) for the full API.

## Workbench

```sh
cd web
npm ci
npm run wasm     # builds crates/daedalus-wasm and runs wasm-bindgen 0.2.129
npm run msl      # packs the Modelica Standard Library 4.1.0 into public/msl/
npm run dev      # http://localhost:5173
```

The page loads the bundled `Daedalus` library and the Modelica Standard Library
4.1.0 (`Modelica`, `ModelicaServices`, `Complex`). Other `.mo` files can be
opened from disk.

The MSL sources are not in this repository. `web/scripts/fetch-msl.mjs`
downloads the v4.1.0 release from GitHub and packs its `.mo` and
`package.order` files into `web/public/msl/msl-4.1.0.<hash>.json.gz`
(about 2.6 MB; 14.8 MB unpacked). It also writes `manifest.json`, which names
that bundle, and a copy of the BSD-3-Clause license. To work offline, pass a
local checkout instead:

```sh
node scripts/fetch-msl.mjs /path/to/ModelicaStandardLibrary   # or MSL_DIR=/path/to/... npm run msl
```

The script does nothing when `public/msl` already holds that version. Pass
`--force` to rebuild the bundle. Without a bundle, the workbench runs with the
Daedalus library only.

The worker downloads the bundle after the Daedalus library is loaded and
unpacks it with `DecompressionStream`. It then registers every file without
parsing it. A light token scan outlines each file, so the package browser can
list the whole library within seconds. A class is parsed when it is opened,
checked or simulated, together with the classes it needs (the same on-demand
closure the CLI uses, see `crates/daedalus-core/src/frontend/lazy.rs`).

The `Dockerfile` at the repository root builds the workbench (Rust to
WebAssembly, then Vite) and serves the static bundle with nginx on port 80:

```sh
docker build -t daedalus-web .
docker run --rm -p 8080:80 daedalus-web   # http://localhost:8080
```

The live workbench at https://daedalus.apps.ezkeel.com is built from this
Dockerfile, and ezkeel redeploys it on every push to `main`.

The page is laid out like Dymola:
- the package browser is on the left;
- the class you open shows in Icon, Diagram or Text view;
- **Simulate** compiles the class in the worker and fills the variable browser;
- ticking a variable plots it;
- a model with MultiBody shapes also gets an **Animation** view: a 3-D
  replay with STL/OBJ meshes, studio lighting, camera presets, follow and
  chase cameras, part selection with X-ray, and WebM recording.

### Plotting and dashboards

- **Live plotting.** With **Live** ticked (the default) a simulation streams its
  rows into the plot while it runs, on a fixed time axis from start to stop time;
  **■ Stop** ends it and keeps the rows so far. The worker advances the run in
  slices of about 120 ms of wall-clock time (`sim::Incremental`): the solver takes
  exactly the steps of a run in one go, so the result is identical; only the
  plotted variables' new rows cross to the page after each slice. A profiled run
  (and Live unticked) uses the one-shot path.
- **Plots in the Modeling page.** Once a model has results, a plot strip below the
  diagram shows the same plot window as the Simulation page (curves added in one
  appear in the other). It can be resized by its top edge and collapsed; both are
  remembered per browser. Simulating from the Modeling page with the strip open
  stays there.
- **Click to plot.** Clicking a component in the diagram (Modeling page with
  results, or the **Diagram** pane next to the plot in Simulation mode) opens a
  menu of its variables, its own first and then its connectors'; clicking a
  connection lists the across and flow variables of both connectors.
- **Matrix layout.** **Layout** arranges the subplots in rows × columns (1 × 2 up
  to 3 × 3). All time subplots share one time axis: drag to zoom, wheel to zoom
  around the cursor, Shift+drag to pan, double-click to reset; the cursor readout
  shows the same time in every subplot.
- **Expressions.** The expression field plots a formula over result variables,
  re-evaluated after every run: `inertia.w*60/(2*pi)`, `abs(a - b)`, `der(x)`
  (the solver's derivative when `der(x)` is a result variable, else differences),
  `integral(x)`, `movavg(x, T)`, `rms(x, T)`, `min`, `max`, `pow`, `atan2`, the
  usual elementary functions, `pi`, `e` and `time`. `fft(x)` plots the amplitude
  spectrum of the visible time window (uniform resampling to a power of two, Hann
  window) in a frequency subplot. **Signal…** applies one of these operators to
  every variable of the active subplot.
- **Stats** shows min, max, time-weighted mean, RMS and peak-to-peak of every
  curve over the visible window, updated as you zoom.
- **Dashboard.** A panel of widgets bound to the open model: sliders on free Real
  parameters, gauges, numeric displays and mini plots of variables. **Run live**
  paces the simulation against wall time (0.1× to 200×) in the worker; **Pause**,
  **Stop** and the speed apply during the run. Moving a slider during the run
  changes the parameter at the time reached without restarting: parameters bound to
  it are evaluated again, an event iteration settles the discrete variables and
  the integrator restarts from the current states (start values and
  `fixed = false` parameters keep their initial values). Only free Real
  parameters can change this way; others need a new run. A slider moved while
  nothing runs sets the override for the next run. Layouts are saved per model in
  local storage, and **Share** copies a link (`&dash=…`) that opens the model
  with the dashboard.

`sim::Incremental` (`advance(t, wall_budget)`, `set_params`, `results`,
`finish`) is the core of both; `crates/daedalus-core/tests/incremental.rs` checks
that budget slices reproduce a full run bit for bit, that time slices see every
event, and that mid-run parameter changes propagate. In the WebAssembly workspace
it is `liveStart`, `liveAdvance`, `liveValues`, `liveSetParams` and `liveFinish`.
`web/scripts/e2e-plotting.mjs` exercises all of the above in Chromium.

### Editor intelligence

The Text view is a Modelica editor backed by the compiler's own name lookup
(`crates/daedalus-core/src/ide.rs`, run in the worker):

| Key | Does |
| --- | --- |
| F12, or Ctrl/Cmd-click | Go to the definition: opens the class that declares the name and selects it |
| Shift+F12 | Find usages across the loaded documents, in a panel; click a row to go there |
| F2 | Rename a class, component or loop index everywhere it is named |
| Ctrl+Space (or just type) | Complete: classes and components in scope, members after `a.b.`, modifier names inside `x(...)`, keywords |
| hover | Declaration, description, declaring class, type and unit |
| Ctrl+Shift+O | Outline of the document's classes and components |

Syntax errors are underlined as you type, with unknown classes (errors) and
unknown names, members and modifiers (warnings). Enter and `end`/`else`/
`equation` indent the Modelica way.

Names are resolved with the Modelica lookup rules: enclosing classes, imports
(qualified, renamed, unqualified, selective), inherited elements, component
types for dotted references in equations (`r1.p.v`), modifier and named
argument names against the type they modify, enumeration literals and `for`
indices. Definition on an `outer` component also lists the matching `inner`
declarations. Rename refuses names already taken, keywords, classes stored in
their own file, and any edit in the read-only library bundle (the MSL).

Only the edited document is re-parsed per keystroke, and while it does not
parse the last version that did stands in for it, so completion keeps working
mid-edit. With the whole MSL parsed (2,554 files), a natively compiled
update plus completion takes about 6 ms and finding the 38 uses of
`Modelica.Blocks.Sources.Step` about 85 ms; `cargo test -p daedalus-core
--test ide` checks that every MSL file (with `DAEDALUS_MSL=/path/to/msl`) and
the Daedalus library resolve without a single diagnostic.

## Experiments: sweeps, runs and comparison

`daedalus sweep` translates a model once and simulates it for every combination of
parameter values, one run per thread:

```sh
# 3 x 3 grid over spring rate and bump height, on all cores; peak body travel and acceleration
./target/release/daedalus sweep library/Daedalus -m Daedalus.Examples.QuarterCar.BumpComparison \
    -s linear.c_spring=lin:20e3:40e3:3 -s bump.h=0.03,0.05,0.07 \
    --vars linear.body.s,linear.a_body --stats max,min -o runs/   # runs/run_001.csv ... runs/summary.csv
```

- Values: a list `1,2,3`, `lin:START:STOP:N` or `log:START:STOP:N`.
- `--grouped` runs the n-th values of all parameters together (Dymola's grouped
  sweep) instead of the full grid.
- `-o FILE.csv` writes all runs into one CSV (`run, parameters, time, vars`);
  `-o DIR` writes one file per run (`--format csv|mat`) and `summary.csv`.
- `--stats` picks the summary columns per kept variable: `initial`, `final`,
  `min`, `max`, `mean` (time average) or `at=T`.
- Free Real parameters change at run time on the shared compiled model. Other
  parameters (Integer, Boolean, computed ones; possibly structural, such as array
  sizes) are applied as a modifier on a wrapper class and translated once per
  distinct value, which is slower; `--retranslate` forces that for all.

In the workbench, every simulation is stored as a run (model, overrides,
settings, time and the plotted variables) in the **Runs** pane, kept in the
browser's IndexedDB across reloads. Runs can be renamed, pinned and deleted.

- **Overlay** plots the variables of the active subplot for every selected run;
  the legend shows the run and the parameter values that differ.
- **Diff B − A** plots the newer of two selected runs minus the older one,
  interpolated on the union of their time grids.
- **Sweep…** picks one or more editable parameters with list, linear or log
  values, grid or grouped, and the variables to keep. The runs fan out over a pool
  of `hardwareConcurrency - 1` Web Workers (at most 8), each with its own
  WebAssembly workspace and the same sources; a worker translates once and reuses
  the translation for every run. Progress, cancel, a summary table and a plot of a
  scalar result (final value, max, min, mean) against a parameter follow; the
  runs are overlaid in the plot.
- **Share link** copies a URL such as
  `#Daedalus.Examples.QuarterCar.BumpComparison?p.bump.h=0.07&p.linear.c_spring=40000&stop=1.5&plot=linear.body.s`
  that reopens the model, applies the overrides and settings, simulates it and
  plots the variables. A plain `#Model.Name` still just opens the model.

The pool (`web/src/pool.ts`, `pool().run(model, options, vars)`) is generic, so
other analyses can use it. `web/scripts/e2e-experiments.mjs` checks all of this in
Chromium with Playwright against a running dev server.
### Graphical editing

Classes in your own documents can be edited in the Diagram view. The bundled
`Daedalus` library and the MSL are read-only: **Duplicate to my models** copies
the open class into `MyModels.mo`, with every class name it uses written out in
full, and opens the copy for editing. **New model** adds an empty model there.
`MyModels.mo` is kept in the browser's local storage and **Save .mo**
downloads the edited file.

With **✎ Edit** on:
- drag a class from the package browser onto the diagram to add a component
  (20×20 at the drop point, named `resistor1`, `resistor2`, ...);
- click or rubber-band to select, Shift adds to the selection, drag to move
  (2-unit grid), arrow keys nudge;
- **R** rotates by 90°, **H** and **V** flip, **Del** deletes components with
  their connections, the corner handle resizes;
- drag from a connector to another to connect. Compatible connectors (same
  class, same members, or one family such as `RealOutput`/`RealInput` or
  `PositivePin`/`NegativePin`) light up, and the line is routed orthogonally.
  Click a line to select it and drag its corners;
- double-click a component for its parameter dialog: name, value, unit,
  description and the class default of every parameter. Values are written
  as modifiers, and an emptied field removes the modifier;
- **Ctrl+Z** and **Ctrl+Y** undo and redo, over whole document versions.

Every gesture is a list of edit operations applied to the source text in the
worker (`crates/daedalus-core/src/frontend/edit.rs`). An edit replaces only the
tokens it has to, so comments, layout and other annotations stay as written,
and the result must parse before it is accepted. The Text view, **Check** and
**Simulate** always use the edited text.

## Solvers

`--method` (or the workbench's Method select, or the experiment's
`__Dymola_Algorithm`) picks the integrator:

- stiff: `bdf` (default), `radau` (Radau IIA, order 5) and `esdirk`
  (ESDIRK4(3)6L, order 4);
- switching: `auto` starts with Dopri5 and moves to BDF when the step size is
  limited by stability, and back;
- non-stiff: `dopri5`; fixed step: `rk4`, `euler`.

The implicit methods get the state Jacobian exactly from forward-mode passes of
the tape, one per color group (`--jacobian fd` uses finite differences
instead). After a run the workbench recommends a stiff method when the
statistics show a stiff signature. Details, the `__Dymola_Algorithm` mapping
and a comparison on the examples are in [docs/solvers.md](docs/solvers.md).

## Examples

The `Daedalus.Examples` package rebuilds six technical blog posts from Modelon
and Claytex: torque converters, landing-gear drop tests, an aileron actuator,
a quarter car with a hydraulic damper, vehicle-efficiency sensitivity, and
coast-down road-load fitting. Each model cites its source post. Results and
assumptions are in [docs/examples.md](docs/examples.md).

## Optimal control, MPC and estimation

`daedalus export-casadi` hands a translated model to [CasADi](https://web.casadi.org):
the system after alias elimination, index reduction and tearing, as a
semi-explicit index-1 DAE. The `daedalus_casadi` Python package in `python/`
builds on it with:
- direct-collocation optimal control;
- model predictive control;
- moving horizon estimation of states and parameters.

Costs and constraints refer to model variables by their Modelica names.

```sh
pip install casadi numpy matplotlib
python python/examples/ocp_aileron.py            # minimum-time aileron move: 41.7 ms
python python/examples/mpc_active_suspension.py  # MPC with bump preview: -35 % body acceleration
python python/examples/mhe_coastdown.py          # Cd and crr from a noisy coast-down
```

See [docs/optimal-control.md](docs/optimal-control.md).

## AI assistant and MCP server

`daedalus mcp` serves the loaded libraries to an AI assistant over the Model Context
Protocol (newline-delimited JSON-RPC on stdio). Its tools are `list_classes`,
`get_class`, `set_source`, `check_model`, `simulate_model`, `linearize_model`,
`model_structure` (roles, BLT, state choice, Jacobian eigenvalues and stiffness, with
`stateSelect` overrides), `profile_simulation`, `equations` and `flatten_model`. With `--workspace DIR`, `set_source` also writes the
file to `DIR` and the `.mo` files already there are loaded at start.

```sh
# Claude Code
claude mcp add daedalus -- /path/to/daedalus mcp /path/to/library/Daedalus --workspace ./models
```

The workbench has the same tools built in: **✦ Assistant** in the top bar opens a chat
with Claude (`claude-opus-5-5`) that can browse and read classes, write models to
`user/*.mo` files, check, simulate (the result shows in the variable browser), read the
structure report and the performance profile, and open classes. Library files stay read-only. It uses your own Claude API key, which is kept
only in that browser; requests go straight from the page to the Claude API. Other
workbench features add their tools with `registerAssistantTool` (`web/src/assistant.ts`).

## Sensitivity, uncertainty and calibration

The CLI and the workbench's Analysis mode share the analyses in
`daedalus-core`:
- local sensitivities `dy/dp`, either exact (forward sensitivity equations with
  tape tangents) or by central differences, with a parameter ranking;
- Monte Carlo prediction bands, Sobol indices and Morris screening over
  parameter distributions;
- calibration against measured CSV data (projected Levenberg–Marquardt with
  standard errors and a Modelica modifier of the fit);
- design optimization of scalar quantities.

```sh
./target/release/daedalus sensitivity model.mo -m M --params k,c --outputs x
./target/release/daedalus gsa model.mo -m M --dist k=normal:6:0.5 --dist c=0.2:0.6 --qoi 'max(x)'
./target/release/daedalus calibrate model.mo -m M --data meas.csv --tune k=1:20 --tune c=0.05:2
./target/release/daedalus optimize model.mo -m M --tune b=0:5 --target 'final(x)=2'
```

Only free parameters can be varied: public parameters with a literal value
(the editable ones in the variable browser). Parameters computed from others,
protected ones, and those evaluated at translation give an error that lists the
free ones.

- **sensitivity**: `--sens-method auto|exact|fd`.
  - `exact` integrates `ds/dt = df/dx s + df/dp` next to the state (staggered,
    an L-stable Radau IIA step per half output interval). `s(t0)` comes from a
    tangent run of the initialization, and every derivative is an exact
    forward-mode tangent of the tape, including algebraic loops. It needs a model
    without when-clauses and `delay()`, and a run without events; it does not
    apply jump conditions.
  - `fd` uses two runs per parameter with a relative step of 1e-4 and the
    tolerance tightened to at most 1e-8.
  - `auto` uses `exact` where it applies and otherwise says why it used `fd`.
  - The output holds the time series of `dy/dp` and `p/y dy/dp` (with |y|
    floored at 1 % of its peak), plus a ranking by the time average of
    |normalized sensitivity|.
- **montecarlo / gsa**: `--dist p=lo:hi | normal:mean:std | lognormal:mean:std`
  sets the distributions; sampling is Latin hypercube or random, seeded.
  - Quantities: `final(x)`, `max(x)`, `min(x)`, `mean(x)`, `integral(x)` and
    `at(x,t)`, with an optional window `max(x,t0,t1)`.
  - `--sobol` gives first-order indices (Saltelli) and total indices (Jansen)
    with bootstrap intervals, in N (k+2) runs.
  - `--morris` gives mu*, mu and sigma, in r (k+1) runs.
  - Runs are independent: natively they use all cores, and in the browser one
    worker runs them in slices.
- **calibrate**: the CSV holds a time column and measured columns, which are
  fitted by name or mapped with `--map col=var`.
  - Weights default to 1/std of each column, and `--window T0:T1` restricts the
    fit in time.
  - It reports the fitted values, the RMS before and after, standard errors and
    correlations from J^T J, and the fit as a modifier such as
    `(c = 0.55, body(m = 1.2))`.
- **optimize**: Nelder–Mead in the bounds box. The objective sums `--minimize q`,
  `--maximize q` and `--target q=v` (a squared error).
- **Output**: a text summary, or JSON / CSV with `-o`.

In the workbench, simulate a model, open **Analysis** and tick parameters. The
tabs are:
- Sensitivity: curves over time and a ranking bar chart;
- Uncertainty: the mean with its 5–95 % band, and Sobol and Morris bar charts;
- Calibrate: load a CSV, map its columns and set bounds; it plots measured data
  against the model before and after the fit, with "Copy as modifier";
- Optimize.

A progress bar shows while an analysis runs, and **Cancel** stops it.

Heavy dynamic optimization and estimation (optimal control, MHE, constraints)
stay in the CasADi export below. The tests in
`crates/daedalus-core/tests/analysis.rs` compare these analyses with analytic
results: exponential decay, a mass-spring-damper, Ishigami Sobol indices,
Monte Carlo moments, and parameter recovery from synthetic data.

### Adjoint gradients

`--adjoint --objective QOI` computes the gradient of one scalar with respect to
many parameters at the cost of about one forward and one backward solve,
independent of the number of parameters:

```sh
./target/release/daedalus sensitivity model.mo -m M --adjoint \
    --objective 'integral(x)' --params k1,k2,k3,k4,k5,k6
```

- Objective: one of `final(x)`, `at(x,T)`, `integral(x)`, `mean(x)`, with an
  optional window (`integral(x,T0,T1)`). `max`/`min` are differentiated at the
  argmax on the output grid.
- The forward run stores the states on a mesh (`--substeps`, default 2 per
  output interval) and at the Radau stage times, from the solver's dense output.
- The backward pass solves `-dλ/dt = (df/dx)^T λ + (dg/dx)^T` with a 2-stage
  Radau IIA step. Each `f_p^T λ` is one reverse pass over the tape
  (`tape_reverse.rs`), including algebraic loops (implicit-function rule).
  Point terms add jumps to `λ`, and `λ(t0)` is chained through the initialization.
- `--grad-method auto|adjoint|forward|fd`: `auto` uses the adjoint and falls back
  to forward sensitivities (or differences) for models with events, saying why.
- Accuracy against forward sensitivities and central differences is about
  1e-7 relative on the test models (`crates/daedalus-core/tests/adjoint.rs`).

The same gradients drive bound-constrained optimization:
- `optimize --algorithm lbfgsb` (projected L-BFGS with an active set and Armijo
  backtracking);
- `calibrate --optimizer lbfgsb`, which minimizes the weighted least-squares
  cost with an adjoint gradient (one point term per measurement) and then
  reports standard errors from the Jacobian at the solution.

On a 6-state chain with 9 parameters, L-BFGS-B converges in about 5 runs plus
5 gradients, where Nelder–Mead needs several hundred runs.

### Design of experiments

```sh
./target/release/daedalus doe model.mo -m M --factor k=1:10 --factor c=0.1:1 \
    --design ccd --qoi 'max(x)' --qoi 'integral(v)' --pareto min,min -o doe.csv
```

- Designs:
  - `full`: full factorial with `p=lo:hi:levels` levels (default 2);
  - `frac`: 2^(k-p) fractions with minimum-aberration generators, or your own
    with `--generators D=ABC,E=AB` or `--resolution R`;
  - `ccd` (default): central composite, with `--alpha face|rotatable|orthogonal|A`
    and a resolution-V fraction for 5 or more factors;
  - `bbd`: Box–Behnken;
  - `lhs`: Latin hypercube with `-N` samples and `--seed`.
- Runs go through the sweep machinery: all cores natively. In the browser they
  are spread over the worker pool, and each worker evaluates the quantities.
- Per quantity it fits a quadratic response surface in coded units (−1..1) by
  least squares and prints R², adjusted R², RMSE and a coefficient table with
  standard errors and t-values. Terms the design cannot estimate (e.g.
  quadratics in a 2-level design) are dropped and listed.
- It also reports main effects (mean response per level), two-factor
  interaction tables and, with `--pareto` for two quantities, the
  non-dominated runs.
- Factors must be free parameters, like for the other analyses.

The workbench's Analysis mode has the method **adjoint** in the Sensitivity tab
(an objective field and a gradient bar chart). A **DOE** tab picks the design
and quantities and shows main effects, interactions, the fitted coefficients
and the Pareto front, with "Use" to apply a Pareto point's parameters. The
Calibrate and Optimize tabs can choose L-BFGS-B.

## Performance

The benchmark is `tools/gen_chain.py`: a chain of N masses and spring-dampers
from the library, driven by a sine force. It is a stiff system with 2N states,
simulated for 1 s with BDF at tolerance 1e-6, in a release build.

| N | Unknowns after alias elimination | Translate | Simulate |
| ---: | ---: | ---: | ---: |
| 1,000 | 13,000 | 0.25 s | 0.42 s |
| 10,000 | 130,000 | 2.5 s | 3.9 s |

The nine example models simulate in 2.6 s in total. That includes 11 NEDC laps
of the EV model, which take 109k steps.

### Profiling a simulation

`--profile` answers "why is my model slow" and "why does it chatter", like
Dymola's Simulation Analysis and Amesim's Performance Analyzer:

```sh
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.TorqueConverter.Launch \
    --profile launch-profile.json   # summary on stdout, the full profile as JSON
./target/release/daedalus simulate model.mo -m M --events   # the event log
```

A profile records:
- every accepted step (time, size, order) and the scaled local error estimate
  of each state, and every rejected step with its reason (error test or Newton
  failure). From these it finds the states that limit the step size: the
  state with the largest error term, per step and as a share of all steps;
- the time, calls, iterations and failures of each algebraic loop (Newton
  block) and function loop, the explicit equations and the integrator itself,
  with the equations of each block;
- each event: its time, whether a state or a time event triggered it, the
  relations that switched (source text and component), the `when` clauses
  that fired (`reinit`, `terminate`) and the values that changed;
- per relation, its switches, first and last time and shortest interval. A
  relation that switches at least 20 times in a row, each less than 0.05 % of
  the simulated interval after the previous switch, is reported as chattering;
- the time split (initialization, integration, events, output) and
  plain-language hints, e.g. "relation `v > 0` fired 501 times between
  t=1.019367991845 and 1.019367991895: likely chattering; consider ...".

In the workbench, tick **Profile** next to **Simulate**. A **Performance** tab
then appears beside the plot. It shows the step size over time (log scale,
rejected steps in red), a strip of the dominant state per step, the limiting
states, the block cost table (click a block for its equations), the
relations, and the event log with a filter. Chattering is highlighted, and
clicking an event marks its time in both plots.

Profiling is off by default, and then nothing is recorded. Results with and
without profiling are bit-identical. With profiling on, the run is about 10-35 %
slower (TorqueConverter.Launch: 4.5 ms to 6.5 ms; the 1000-mass chain: 344 ms
to 453 ms).

## Layout

| Path | What |
| --- | --- |
| `crates/daedalus-core` | Frontend (Rumoca parser, instantiation, flattening, class inspection), backend, codegen, solvers, result files |
| `crates/daedalus-cli` | `daedalus` command line tool |
| `crates/daedalus-wasm` | WebAssembly bindings used by the workbench |
| `crates/daedalus-py` | Python package `daedalus` (PyO3, built with maturin; not a cargo workspace member) |
| `crates/daedalus-fmi` | FMI 2.0 and 3.0 runtime (`fmi2*`, `fmi3*` C functions) of exported FMUs |
| `library/Daedalus` | Component library (Blocks, Mechanics, Hydraulics, Vehicle) and the examples |
| `library/MultiBodyVehicle` | 3-D suspensions (double wishbone with external or pushrod springs, MacPherson, multilink), test rigs (K&C, seven-post) and full-car manoeuvres on MSL `Modelica.Mechanics.MultiBody` (needs MSL 4.1.0) |
| `web` | Browser workbench (TypeScript, Vite, CodeMirror) |
| `python` | `daedalus_casadi`: CasADi optimal control, MPC and MHE on exported models |
| `tools` | Benchmark generator, coast-down fit, FMU check (FMPy) |

Modelica is a registered trademark of the Modelica Association. DAEdalus is not
affiliated with or endorsed by the Modelica Association.
