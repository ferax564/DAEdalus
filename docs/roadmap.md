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
- Browser workbench:
  - package browser;
  - icon, diagram and text views;
  - simulation setup and variable browser;
  - plot window.
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

`CombiTable1D`/`CombiTimeTable` (all smoothness kinds of the C tables, with
first and second derivatives and the C time-event intervals), `CombiTable2D`
(bilinear, constant and bivariate Akima) and tables read from text, CSV and
MATLAB v4/v6/v7 files are supported; tables supplied through a `usertab` C
function and MATLAB v7.3 (HDF5) files are not.

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

- Graphical editing: drag components from the browser, draw connections, edit
  parameters in a dialog, and write the result back to the text.
- Animation of mechanical models, and an FMU export (FMI 3 co-simulation).
- Persisted experiments, and result comparison across runs.
