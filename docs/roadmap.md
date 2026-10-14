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

The current MSL 4.x sweep is `daedalus test-library <MSL> -m Modelica`:
**110 of 534** example models translate and simulate. The largest groups of
failures, which are the next work items:

| Failures | Missing feature |
| ---: | --- |
| ~95 | Base classes from not-yet-supported packages, mostly synchronous `Clock` models (`Modelica.Clocked`) and `ExternalObject` tables (`CombiTable*`, `CombiTimeTable`) |
| 76 | Array-valued attributes on record and matrix components (MultiBody `R_rel`, `r_0`, frame records) |
| 58 | References to disabled conditional components inside connect sets and bindings |
| ~40 | Parameter and function evaluation that meets a record component or a string where a number is expected |
| 21 | `extends` depth on deeply nested Media packages |
| 11 | Over-determined systems, most likely from overconstrained connectors (`Connections.branch`/`root`), which are not handled yet |
| 8 | External C functions |

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
