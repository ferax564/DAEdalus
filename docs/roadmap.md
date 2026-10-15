# Roadmap

## Done

**Language and compiler**
- Modelica frontend: classes, inheritance, modifiers, redeclarations,
  conditional components, arrays and `for` loops, functions (inlined),
  connections including array connects.
- Backend: alias elimination, Pantelides with dummy derivatives, BLT, tearing,
  symbolic Jacobians, and a separate initialization system.

**Simulation**
- Tape code generation.
- Solvers: BDF with dense and sparse LU, Dopri5, RK4 and Euler.
- Events, `when`, `reinit` and `terminate`.
- Results as Dymola `.mat` or CSV.

**Library and examples**
- Bundled library with these packages:
  - Blocks
  - Rotational mechanics, including a torque converter and clutches
  - Translational mechanics, including an elasto-gap
  - Hydraulics, including a servo valve, cylinder, accumulator and aerated oil
  - Vehicle, including an EV machine, battery, driver and drive cycles
- Six blog-post examples with regression tests (see [examples.md](examples.md)).

**Tooling**
- CLI: `check`, `flatten`, `dump`, `simulate`, and `test-library` (parallel,
  one process per model, with a time limit).
- `linearize`: exact state-space matrices A, B, C, D (forward-mode tangents of
  the tape, implicit differentiation of algebraic loops) at the initial point or
  after simulating to a time; text, JSON or an OpenModelica-style
  `linearized_model`.
- Performance analyzer and event log (`simulate --profile`, `--events`, the
  workbench Performance tab): step sizes, rejected steps and the states that
  limit the step; time and Newton iterations per block; events with their
  relations, components and changed values; chattering detection; hints.
- `fmu`: FMI 2.0 export, Model Exchange and Co-Simulation in one FMU (linux64),
  with a model-independent runtime library and the compiled model as a resource;
  validated with FMPy in CI.
- `mcp`: a Model Context Protocol server (browse, edit, check, simulate,
  linearize) and an in-app Claude assistant in the workbench with the same tools.
- `structure`: variable roles, BLT blocks with their equations, incidence,
  the dummy-derivative state sets and why each state was chosen; at a time
  point the exact state Jacobian with its condition number, eigenvalues, time
  constants, participation factors, stiffness ratio and a solver suggestion,
  the Jacobians of the algebraic loops, and a check of the static state
  choice along the trajectory. Workbench Structure view with the same.
- State selection without editing the model: `--state NAME=prefer|avoid|...`
  (globs allowed) on every command, and pin/forbid in the Structure view,
  with "copy as modifier".
- Browser workbench:
  - package browser;
  - icon, diagram and text views;
  - simulation setup and variable browser;
  - plot window;
  - graphical editing: drag components from the browser, draw connections with
    compatible-connector highlighting and orthogonal routing, move, rotate,
    flip, delete, a parameter dialog, undo and redo, all written back to the
    text without disturbing its formatting.
- Experiments: `daedalus sweep` (grid or grouped parameter sweeps on all cores,
  structural parameters translated again per value, summary table); in the
  workbench, stored runs (IndexedDB) with overlay and diff, sweeps over a pool
  of Web Workers with a summary table and a scalar-vs-parameter plot, and
  shareable links that reopen and run a model with its overrides.
- CI: Rust tests, a simulation of every example, a 2000-mass scalability smoke
  test, and the WebAssembly and workbench build.

## Next

### Modelica Standard Library coverage

The MSL 4.x sweep is `daedalus test-library <MSL> -m Modelica`:
**441 of 534** example models translate and simulate (up from 261, and 110
before that). Recently covered: `Modelica.Media` (IF97 water, R134a, ideal-gas
mixtures, moist air, reference air) and most `Modelica.Fluid` examples, through
compiled run-time functions with exact forward-mode partials, Dymola-style
preferred-state selection and record constants from modifiers. The remaining
failures, which are the next work items:

| Failures | Cause |
| ---: | --- |
| ~30 | Stiff or discontinuous runs that stop with "step size too small" or reach the wall-clock limit (Spice3 transistors, digital circuits, some Fluid start-ups such as `HeatingSystem`) |
| 12 | External C functions (`Modelica.Math.Random`, `Streams`, FFT, `Utilities`) |
| ~10 | Function bodies: `return` under a time-varying condition, missing defaulted inputs in nested calls, vector size checks |
| ~10 | Subscripts on non-array components and non-constant subscripts in bindings |
| 6 | Friction and thyristor events that chatter (clutches, RobotR3 bearing friction) |
| ~5 | Initialization of nonlinear systems (R134a two-volume system, op-amp circuits, rolling wheel) |
| 3 | Synchronous `Clock` partitions |

`CombiTable2D` is not supported yet.

### Performance

- **Flattener allocation.** At 10k components, flattening (1.4 s) is the
  largest translation cost. The plan:
  - intern instance names;
  - share modifier environments instead of cloning them;
  - cache instantiated class skeletons per (class, modifier) pair.
- **Parallelism.**
  - Parallel instantiation of independent component subtrees.
  - Parallel Jacobian coloring and evaluation for >100k states.
- **JIT.** Optionally lower the tape to Cranelift natively and to a generated
  WebAssembly module in the browser.
- **Solver.** A sparse-Jacobian reuse heuristic across steps, and IDA-style
  DAE mode for very large index-1 systems without state-space conversion.

### Workbench

- Graphical editing, next steps: editing inherited components and connections,
  array connectors (`u[1]`), connector names in the diagram, align and space.
- Animation of mechanical models.

### FMI

- FMU export for other platforms (Windows, macOS: build `daedalus-fmi` there),
  `fmi2GetDirectionalDerivative` from the tape's forward-mode tangents, FMU state
  get/set, and time events for table breakpoints in Model Exchange (a jump in a
  time table without an event can stall an importer's integrator).
- FMI 3.0 and FMU import.
- Persisted experiments, and result comparison across runs.
