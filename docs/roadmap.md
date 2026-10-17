# Roadmap

## Done

**Language and compiler**
- Modelica frontend: classes, inheritance, modifiers, redeclarations,
  conditional components, arrays and `for` loops, functions (inlined),
  connections including array connects.
- Backend: alias elimination, Pantelides with dummy derivatives, BLT, tearing,
  symbolic Jacobians, and a separate initialization system.
- Unit checking of every equation, with inferred units and display units
  ([units.md](units.md)).
- State machines (Modelica `state` and StateGraph), natively and in the browser
  ([state-machines.md](state-machines.md)).

**Simulation**
- Tape code generation.
- Solvers: BDF with dense and sparse LU, Dopri5, RK4 and Euler.
- Solver suite ([solvers.md](solvers.md)): Radau IIA (order 5) and ESDIRK4(3)6L
  with dense output, `auto` Dopri5/BDF switching with Hairer's stiffness test,
  exact colored state Jacobians from tape tangents (`--jacobian fd` for
  differences), `__Dymola_Algorithm` mapped to the closest method, and a
  grouped method select with a stiff-solver recommendation in the workbench.
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
  linearize) and an in-app assistant in the workbench with the same tools.
- `linear-analysis`: poles and zeros, Bode and Nyquist with margins, step and
  impulse responses, PID suggestions, balanced truncation and LQR
  ([linear-analysis.md](linear-analysis.md)).
- `debug`: traces how a variable is computed, and explains failed translations
  and initializations ([debugger.md](debugger.md)).
- `codegen-c`, `hil` (software and processor in the loop) and `autotune`: embedded
  C and controller tuning ([embedded.md](embedded.md)).
- `import-fmu`, `import-ssp` and `export-ssp`: FMU and SSP import and export
  ([fmi.md](fmi.md)).
- `lib`, `package`, `diff` and `params-from`: libraries, packages and parameter
  tables ([libraries.md](libraries.md)).
- `steady` (steady states and trim) and `run` (Dymola and OpenModelica `.mos`
  scripts) ([scripting.md](scripting.md)).
- `structure`: variable roles, BLT blocks with their equations, incidence,
  the dummy-derivative state sets and why each state was chosen; at a time
  point the exact state Jacobian with its condition number, eigenvalues, time
  constants, participation factors, stiffness ratio and a solver suggestion,
  the Jacobians of the algebraic loops, and a check of the static state
  choice along the trajectory. Workbench Structure view with the same.
- State selection without editing the model: `--state NAME=prefer|avoid|...`
  (globs allowed) on every command, and pin/forbid in the Structure view,
  with "copy as modifier".
- Analysis (`sensitivity`, `montecarlo` / `gsa`, `calibrate`, `optimize`, and
  the workbench's Analysis mode):
  - exact forward sensitivities (tape tangents) or central differences;
  - Monte Carlo bands, Sobol and Morris indices;
  - Levenberg–Marquardt calibration against CSV data;
  - Nelder–Mead design optimization (see the README).
  - adjoint sensitivities for many parameters, and designs of experiments with
    response-surface fits ([analysis.md](analysis.md)).
- Browser workbench:
  - package browser, with the Modelica Standard Library 4.1.0 loaded on demand;
  - icon, diagram, text, documentation and flat-model views;
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
  - 3-D animation of MultiBody results and of every bundled example;
  - back and forward navigation between classes.
- Live plots and dashboards that update while the model runs
  ([plotting.md](plotting.md)).
- Python API (`import daedalus`) and `daedalus.sciml`: missing physics, sparse
  regression (SINDy) and neural surrogates ([python.md](python.md),
  [sciml.md](sciml.md)).
- `export-casadi` and the `daedalus_casadi` package: optimal control, MPC and
  moving horizon estimation.
- `MultiBodyVehicle`: suspension corners, K&C and seven-post rigs and a full car.
- Text editor with go to definition, completion, hover, find usages and rename
  ([editor.md](editor.md)).
- Release binaries for Linux, macOS and Windows.
- CI: Rust tests, a simulation of every example, a 2000-mass scalability smoke
  test, and the WebAssembly and workbench build.

## Next

### Modelica Standard Library coverage

The MSL 4.x sweep is `daedalus test-library <MSL> -m Modelica`:
**512 of 534** example models run to the end and **488** match the reference results (up from
445 matches at the start of the last round). Recently covered: `Modelica.Media` (IF97 water, R134a, ideal-gas
mixtures, moist air, reference air) and most `Modelica.Fluid` examples, through
compiled run-time functions with exact forward-mode partials, Dymola-style
preferred-state selection and record constants from modifiers.

<!-- conformance-failures:start -->

**512 of 534** example models run to the end and **488** match the reference
results. The 46 that do not are listed from
`docs/conformance/msl-4.1.0.json` (regenerated by `tools/roadmap_failures.py`).

### Does not finish (22)

Translation or simulation stopped, or the run hit the wall-clock limit.

| Model | Status | Detail |
| --- | --- | --- |
| `Modelica.Clocked.Examples.Systems.ControlledMixingUnit` | error | clocked partition is singular at 'filter.x[3] = pre(filter.x[3]) + periodicClock1.period*pre(der(filter.x[3]))' |
| `Modelica.Electrical.Analog.Examples.OpAmps.DifferentialAmplifier` | error | nonlinear system for [resistorGround.v, opAmp.v_out, resistor1.i, opAmp.vps, opAmp.vns, resistorGround.i, resistanceInstrument.v, resistanceInstrument.i, opAmp… |
| `Modelica.Electrical.Machines.Examples.InductionMachines.IMC_InverterDrive` | error | step size too small at t = 0.000000001001116802189966 |
| `Modelica.Electrical.Machines.Examples.InductionMachines.IMC_YDarc` | error | events chatter at t = 0.10001139191079551: `abs(switchYDwithArc.idealCloser.i[1]) Le abs(switchYDwithArc.idealCloser.closerWithArc[1].v)*switchYDwithArc.idealC… |
| `Modelica.Electrical.PowerConverters.Examples.ACDC.RectifierBridge2Pulse.ThyristorBridge2Pulse_DC_Drive` | error | events chatter at t = 14.652213328082592: `rectifier.thyristor_p2.s Lt 0`, `rectifier.thyristor_n1.s Lt 0` keep switching back and forth |
| `Modelica.Electrical.Spice3.Examples.CascodeCircuit` | error | initialization: nonlinear system for [J1.irs, der(J1.S.v), J1.icGS, J2.icGD, der(J2.vGD), der(J1.vGS)] did not converge at t = 0 (residual 1.844e5) |
| `Modelica.Electrical.Spice3.Examples.Inverter` | error | step size too small at t = 0.000000000000006324553933709278 |
| `Modelica.Electrical.Spice3.Examples.Nand` | error | initialization: nonlinear system for [der(mp1.Sinternal), mn1.icGD, mn1.icBD, mn1.ird, mp1.icGS, mp2.icGS, mp2.icBS, mp2.irs, mp1.irs, mp1.icBS] did not conver… |
| `Modelica.Electrical.Spice3.Examples.Spice3BenchmarkDifferentialPair` | error | step size too small at t = 0 |
| `Modelica.Electrical.Spice3.Examples.Spice3BenchmarkFourBitBinaryAdder` | error | initialization: nonlinear system for [der(X1.X1.X1.X1.Q1.vcs), der(X1.X1.X1.X2.Q1.vcs), der(X1.X1.X1.X3.Q1.vcs), der(X1.X1.X1.X4.Q1.vcs), der(X1.X1.X1.X5.Q1.vc… |
| `Modelica.Electrical.Spice3.Examples.Spice3BenchmarkRtlInverter` | error | step size too small at t = 0 |
| `Modelica.Fluid.Examples.AST_BatchPlant.BatchPlant_StandardWater` | error | events chatter at t = 1.0000000034031273: relations keep switching back and forth |
| `Modelica.Magnetic.FundamentalWave.Examples.BasicMachines.InductionMachines.ComparisonPolyphase.IMC_DOL_CommonLeakage` | error | step size too small at t = 0.0000023573410401794283 |
| `Modelica.Magnetic.FundamentalWave.Examples.BasicMachines.InductionMachines.ComparisonPolyphase.IMC_DOL_Polyphase` | error | step size too small at t = 0.0000023570081389449344 |
| `Modelica.Magnetic.FundamentalWave.Examples.BasicMachines.InductionMachines.ComparisonPolyphase.IMS_Start_Polyphase` | error | step size too small at t = 0.000002357014169741107 |
| `Modelica.Magnetic.QuasiStatic.FundamentalWave.Examples.BasicMachines.SynchronousMachines.SMPM_FieldWeakening` | error | nonlinear system for [smpm.is[2].re, smpm.is[2].im, smpm.is[3].re, smpm.is[3].im, dqToThreePhase.id_rms, smpm.stator.Phi.re, smpm.stator.resistor.resistor[2].v… |
| `Modelica.Mechanics.MultiBody.Examples.Constraints.UniversalConstraint` | error | step size too small at t = 4.216816664143639 |
| `Modelica.Mechanics.MultiBody.Examples.Elementary.PointGravityWithPointMasses2` | error | step size too small at t = 1.6193585149900147 |
| `Modelica.Mechanics.Rotational.Examples.HeatLosses` | error | events chatter at t = 0.037699937178018034: `brake.sa Gt brake.tau0_max`, `brake.sa Gt brake.tau0` keep switching back and forth |
| `Modelica.Electrical.Analog.Examples.SimpleTriacCircuit` | timeout |  |
| `Modelica.Electrical.PowerConverters.Examples.ACAC.SoftStarter` | timeout |  |
| `Modelica.Mechanics.MultiBody.Examples.Rotational3DEffects.GyroscopicEffects` | timeout |  |

### Finishes but differs (24)

The run completed and at least one comparison signal left the reference tube.
Several of these are a single signal.

| Model | Signals outside the tube |
| --- | --- |
| `Modelica.Blocks.Examples.DemoSignalCharacteristic` | 2/5 signals: harmonic.mean1.y, harmonic.mean2.y |
| `Modelica.Blocks.Examples.Noise.ActuatorWithNoise` | 1/11 signals: controller.busdelay.y |
| `Modelica.Clocked.Examples.Elementary.ClockSignals.RotationalSample` | 1/4 signals: sample_angle.y |
| `Modelica.Clocked.Examples.Systems.EngineThrottleControl` | 1/8 signals: engine.torque.tau |
| `Modelica.Electrical.Analog.Examples.Lines.CompareLineTrunks` | 1/9 signals: tLine4.p2.v |
| `Modelica.Electrical.Analog.Examples.OpAmps.Multivibrator` | 3/3 signals: c.v, opAmp.out.v, opAmp.out.i |
| `Modelica.Electrical.Machines.Examples.InductionMachines.IMC_DCBraking` | 5/11 signals: imc.idq_rr[1], imc.idq_sr[1], imc.idq_sr[2], imc.is[1] … |
| `Modelica.Electrical.PowerConverters.Examples.ACAC.Dimmer_R` | 2/11 signals: vH01.y_arg, iH01.y_arg |
| `Modelica.Electrical.PowerConverters.Examples.ACAC.Dimmer_RL` | 3/11 signals: multiSensor.v, vH01.y_arg, iH01.y_arg |
| `Modelica.Electrical.PowerConverters.Examples.DCAC.PolyphaseTwoLevel.PolyphaseTwoLevel_RL` | 2/47 signals: voltageSensor.v[1], voltageSensor.v[2] |
| `Modelica.Fluid.Examples.AST_BatchPlant.Test.TankWithEmptyingPipe1` | 1/4 signals: tank1.medium.T |
| `Modelica.Fluid.Examples.AST_BatchPlant.Test.TanksWithEmptyingPipe1` | 1/8 signals: tank2.medium.T |
| `Modelica.Fluid.Examples.AST_BatchPlant.Test.TwoTanks` | 1/6 signals: tank1.medium.T |
| `Modelica.Fluid.Examples.BranchingDynamicPipes` | 2/82 signals: pipe3.flowModel.m_flows[5], pipe3.flowModel.m_flows[6] |
| `Modelica.Fluid.Examples.ControlledTankSystem.ControlledTanks` | 1/23 signals: tank2.medium.T |
| `Modelica.Fluid.Examples.TraceSubstances.RoomCO2` | 2/3 signals: volume.medium.T, volume.medium.p |
| `Modelica.Fluid.Examples.TraceSubstances.RoomCO2WithControls` | 2/16 signals: PID.I.y, ductIn.mediums[1].T |
| `Modelica.Magnetic.FluxTubes.Examples.Hysteresis.HysteresisModelComparison` | 1/12 signals: preisachEverett.x |
| `Modelica.Magnetic.FluxTubes.Examples.MovingCoilActuator.ForceCurrentBehaviour` | 1/4 signals: pmActuator.coil.L_stat |
| `Modelica.Magnetic.FundamentalWave.Examples.BasicMachines.SynchronousMachines.SMEE_LoadDump` | 1/12 signals: voltageController.I.y |
| `Modelica.Magnetic.QuasiStatic.FundamentalWave.Examples.BasicMachines.InductionMachines.IMC_Transformer` | 1/12 signals: imc.stator.zeroInductor.i0 |
| `Modelica.Magnetic.QuasiStatic.FundamentalWave.Examples.BasicMachines.InductionMachines.IMC_YD` | 1/10 signals: imc.stator.zeroInductor.i0 |
| `Modelica.Mechanics.MultiBody.Examples.Constraints.RevoluteConstraint` | 1/5 signals: bodyOfConstraint.body.w_a[2] |
| `Modelica.Mechanics.Rotational.Examples.LossyGearDemo2` | 3/6 signals: bearingFriction.mode, bearingFriction.startForward, gear.mode |

<!-- conformance-failures:end -->
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

- Graphical editing, next steps: editing inherited components and connections,
  array connectors (`u[1]`), connector names in the diagram, align and space.

### FMI

- FMU export for other platforms (Windows, macOS: build `daedalus-fmi` there),
  `fmi2GetDirectionalDerivative` from the tape's forward-mode tangents, FMU state
  get/set, and time events for table breakpoints in Model Exchange (a jump in a
  time table without an event can stall an importer's integrator).
