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
- **Solvers.** It has variable-order BDF with dense or sparse LU, and Dopri5,
  RK4 and Euler, with zero-crossing events and `when`/`reinit`.
- **Results.** They are written as Dymola-format `.mat` files, readable by
  Dymola, OMEdit, DyMat and similar tools, or as CSV.
- **Dymola-style workbench.** It runs in the browser and has:
  - a package browser
  - icon, diagram and text views of each class
  - a variable browser
  - a plot window with display units
  - simulation setup

All compilation runs in a Web Worker.

- **Modelica Standard Library.** 441 of the 534 MSL 4.x example models
  (83%) translate and simulate, including MultiBody, the QuasiStatic machines,
  `Modelica.Media` (IF97 water, R134a, ideal-gas mixtures, moist air) and
  `Modelica.Fluid` systems.

> Status: under active development. See [docs/architecture.md](docs/architecture.md),
> [docs/examples.md](docs/examples.md) and [docs/roadmap.md](docs/roadmap.md).

## Quick start

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

```sh
# FMI 2.0 FMU (Model Exchange and Co-Simulation) for linux64
cargo build --release -p daedalus-cli -p daedalus-fmi
./target/release/daedalus fmu library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -o StallTest.fmu
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
relation. FMU state get/set and directional derivatives are not provided.

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
- ticking a variable plots it.

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
| `crates/daedalus-fmi` | FMI 2.0 runtime (`fmi2*` C functions) of exported FMUs |
| `library/Daedalus` | Component library (Blocks, Mechanics, Hydraulics, Vehicle) and the examples |
| `web` | Browser workbench (TypeScript, Vite, CodeMirror) |
| `python` | `daedalus_casadi`: CasADi optimal control, MPC and MHE on exported models |
| `tools` | Benchmark generator, coast-down fit, FMU check (FMPy) |

Modelica is a registered trademark of the Modelica Association. DAEdalus is not
affiliated with or endorsed by the Modelica Association.
