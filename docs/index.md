# DAEdalus

<p class="hero">An open (Apache-2.0) Modelica modeling and simulation environment. One Rust compiler
and solver runs on the command line, on a server and in a browser tab, with a Dymola-style
workbench, the Modelica Standard Library 4.1.0, 3-D animation, sensitivity and calibration,
FMI export and CasADi optimal control.</p>

<div class="cta">
  <a class="btn primary" href="https://daedalus.apps.ezkeel.com/#Daedalus.Examples.TorqueConverter.Launch">Open the workbench</a>
  <a class="btn" href="getting-started.md">Get started</a>
  <a class="btn" href="whats-new.md">What's new</a>
</div>

![The workbench after simulating TorqueConverter.Launch: two cars launch side by side, one with a static K-factor converter and one with fluid torque build-up](img/examples/tc_launch_anim.webp)

The workbench runs entirely in the browser: the compiler and solvers are compiled to
WebAssembly and run in a Web Worker, so nothing is installed and nothing is sent to a
server. Pick a model in the package browser, press **Simulate**, then plot any variable or
watch the 3-D animation.

## Features

<div class="cards">
  <a class="card" href="linear-analysis.md"><img src="img/linear/card.webp" alt=""><b>Linear analysis <span class="new">New</span></b><span>Bode, Nyquist, step and impulse responses, poles and zeros, model reduction and LQR, from the exact linearization.</span></a>
  <a class="card" href="plotting.md"><img src="img/plotting/card.webp" alt=""><b>Live plots and dashboards <span class="new">New</span></b><span>Results stream in while the model runs; sliders change parameters mid-run; matrix layouts, expressions, FFT and statistics.</span></a>
  <a class="card" href="debugger.md"><img src="img/debugger/card.webp" alt=""><b>Equation and init debugger <span class="new">New</span></b><span>Trace how any variable is computed, from the source equation to the solved form, and see why initialization fails.</span></a>
  <a class="card" href="units.md"><img src="img/units/card.webp" alt=""><b>Unit checking <span class="new">New</span></b><span>Every equation checked for consistent units, units inferred for undeclared variables, and imperial display units.</span></a>
  <a class="card" href="editor.md"><img src="img/editor/card.webp" alt=""><b>Text editor <span class="new">New</span></b><span>Go to definition, completion, hover, find usages and rename, with the compiler's own name lookup.</span></a>
  <a class="card" href="libraries.md"><img src="img/libraries/card.webp" alt=""><b>Libraries and packages <span class="new">New</span></b><span>Install Buildings, OpenIPSL and other libraries by name, pack models as zips, diff versions, import parameter tables.</span></a>
  <a class="card" href="fmi.md"><img src="img/fmu_stalltest.png" alt=""><b>FMI and SSP <span class="new">New</span></b><span>Export FMI 2.0 and 3.0 FMUs, import FMUs from other tools as components, and run SSP systems.</span></a>
  <a class="card" href="analysis.md"><img src="img/analysis/card.webp" alt=""><b>Sensitivity, DOE and calibration <span class="new">New</span></b><span>Adjoint gradients for many parameters, designs of experiments with response surfaces, Sobol indices and fitting to data.</span></a>
  <a class="card" href="embedded.md"><img src="img/embedded/card.webp" alt=""><b>Embedded C and HIL <span class="new">New</span></b><span>Controllers as dependency-free C for Arduino and ESP32, software and processor in the loop, and PID autotuning.</span></a>
  <a class="card" href="sciml.md"><img src="img/sciml/card.webp" alt=""><b>Scientific machine learning <span class="new">New</span></b><span>Neural surrogates, sparse regression of missing physics and physics checks, on top of the Python API.</span></a>
  <a class="card" href="msl.md"><img src="img/msl/card.webp" alt=""><b>Modelica Standard Library <span class="new">New</span></b><span>MSL 4.1.0 in the browser, with native external C functions and exact lookup tables; 488 of its 534 examples match the reference.</span></a>
  <a class="card" href="state-machines.md"><img src="img/state-machines/card.webp" alt=""><b>State machines <span class="new">New</span></b><span>Modelica state machines with transitions, hierarchy, parallel machines and clocks, natively and in the browser.</span></a>
  <a class="card" href="workbench.md"><img src="img/wb/card.webp" alt=""><b>Browser workbench</b><span>Package browser, icon, diagram, text and documentation views, variable browser, plots with display units.</span></a>
  <a class="card" href="structure.md"><img src="img/structure/card.webp" alt=""><b>Structure and state selection</b><span>BLT blocks, incidence, why each state was chosen, Jacobian eigenvalues and stiffness; pick other states without editing the model.</span></a>
  <a class="card" href="solvers.md"><img src="img/solvers/card.webp" alt=""><b>Solvers</b><span>BDF, Radau IIA, ESDIRK and automatic Dopri5/BDF switching with exact Jacobians, and a hint when another method would do better.</span></a>
  <a class="card" href="experiments.md"><img src="img/experiments/card.webp" alt=""><b>Experiments and sweeps</b><span>Every run kept for overlays and differences, parallel parameter sweeps, and links that reopen a run.</span></a>
  <a class="card" href="performance.md"><img src="img/performance/card.webp" alt=""><b>Performance analyzer</b><span>Step sizes, the states that limit them, time per algebraic loop, the event log and chattering detection.</span></a>
  <a class="card" href="workbench.md#graphical-editing"><img src="img/editing/card.webp" alt=""><b>Graphical editing</b><span>Drag components onto the diagram, connect, rotate and set parameters; the source keeps its comments and layout.</span></a>
  <a class="card" href="assistant.md"><img src="img/assistant/card.webp" alt=""><b>AI assistant and MCP</b><span>Ask Claude to build, check, simulate and explain models, in the workbench or from Claude Code.</span></a>
  <a class="card" href="animation.md"><img src="img/multibody/sevenpost_rig.webp" alt=""><b>3-D animation</b><span>Replays MultiBody results and every example in 3-D, with camera presets, chase cameras and video recording.</span></a>
  <a class="card" href="multibody.md"><img src="img/multibody/kc_rig.webp" alt=""><b>Multibody vehicles</b><span>Double wishbone, pushrod, multilink and MacPherson corners, K&amp;C and seven-post rigs, a full car.</span></a>
  <a class="card" href="https://daedalus.apps.ezkeel.com/scoreboard.html"><img src="img/scoreboard_card.webp" alt=""><b>Accuracy scoreboard</b><span>Every MSL example compared with the reference results, signal by signal, by package.</span></a>
  <a class="card" href="python.md"><img src="img/python_card.webp" alt=""><b>Python API</b><span><code>import daedalus</code>: load, set parameters, simulate to NumPy, linearize and export FMUs in-process.</span></a>
  <a class="card" href="scripting.md"><img src="img/scripting_card.webp" alt=""><b>.mos scripts and trim</b><span>Run Dymola and OpenModelica regression scripts; find steady states and trim with exact Jacobians.</span></a>
  <a class="card" href="examples.md"><img src="img/examples/card.webp" alt=""><b>Example library</b><span>Torque converters, landing gear, aileron actuator, hydraulic quarter car and EV efficiency, rebuilt from engineering blog posts.</span></a>
  <a class="card" href="linearize.md"><img src="img/linearize_bode.png" alt=""><b>Linearization</b><span>Exact A, B, C, D matrices at any time, as text, JSON or an OpenModelica-style <code>linearized_model</code>.</span></a>
  <a class="card" href="optimal-control.md"><img src="img/ocp_aileron.png" alt=""><b>Optimal control</b><span>Minimum-time moves, MPC and moving horizon estimation with CasADi, straight from Modelica models.</span></a>
</div>

## Under the hood

DAEdalus does the symbolic work the commercial tools do before any numerics:

- alias elimination, Pantelides index reduction with dummy derivatives, BLT sorting and
  tearing, and exact symbolic Jacobians;
- a compiled register tape that evaluates the model, natively or in WebAssembly;
- variable-order BDF with dense or sparse LU, plus Dopri5, RK4 and Euler, with
  zero-crossing events, `when` and `reinit`;
- results as Dymola-format `.mat` files (readable by Dymola, OMEdit and DyMat) or CSV.

It scales close to linearly: a chain of 10,000 masses (130,000 unknowns after alias
elimination) translates in 2.5 s and simulates for one second in 3.9 s. See
[Architecture](architecture.md) for how each stage works.

## Where to go next

- [Getting started](getting-started.md): use the live workbench, install a release, or
  build from source.
- [What's new](whats-new.md): the features added most recently.
- [Example library](examples.md): every bundled example, with its results.
- [Command line](cli.md): every `daedalus` command and option.
- [Python API](python.md): scripting DAEdalus from Python and Jupyter, and
  [scientific machine learning](sciml.md) on top of it.
- [Embedded C and processor in the loop](embedded.md): controllers on Arduino and ESP32.
- [Structure and state selection](structure.md) and [Sensitivity and calibration](analysis.md):
  understanding a model and its parameters.

> DAEdalus is under active development. Modelica is a registered trademark of the
> Modelica Association; DAEdalus is not affiliated with or endorsed by it.
