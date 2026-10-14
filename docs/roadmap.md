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
**261 of 534** example models translate and simulate (up from 110). Now
covered: MultiBody, overconstrained connectors (`Connections.branch`/`root`),
`Complex` and the QuasiStatic libraries, `CombiTable1D`/`CombiTimeTable`, and
expandable connectors. The largest remaining groups of failures, which are the
next work items:

| Failures | Missing feature |
| ---: | --- |
| 73 | Synchronous `Clock` models (`Modelica.Clocked` and the clocked blocks) |
| ~40 | Media: `extends` depth on deeply nested packages, string-valued substance names in numeric context, and function evaluation over record states |
| ~25 | Array sizes and subscripts in record and matrix bindings (for example the battery `cellData` tables) |
| 14 | External C functions (`Modelica.Math.Random`, `Streams.writeRealMatrix`) |
| ~10 | Index reduction and initialization of a few FundamentalWave machines and rectifiers |
| 1 | RobotR3 `OneAxis`/`FullRobot`: bearing friction stick-slip makes events chatter; the run stops with a clear error |

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

- Graphical editing: drag components from the browser, draw connections, edit
  parameters in a dialog, and write the result back to the text.
- Animation of mechanical models, and an FMU export (FMI 3 co-simulation).
- Persisted experiments, and result comparison across runs.
