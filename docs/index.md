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
  <a class="card" href="workbench.md"><img src="img/wb/card.webp" alt=""><b>Browser workbench</b><span>Package browser, icon, diagram, text and documentation views, variable browser, plots with display units.</span></a>
  <a class="card" href="structure.md"><img src="img/structure/card.webp" alt=""><b>Structure and state selection <span class="new">New</span></b><span>BLT blocks, incidence, why each state was chosen, Jacobian eigenvalues and stiffness; pick other states without editing the model.</span></a>
  <a class="card" href="solvers.md"><img src="img/solvers/card.webp" alt=""><b>Solvers <span class="new">New</span></b><span>BDF, Radau IIA, ESDIRK and automatic Dopri5/BDF switching with exact Jacobians, and a hint when another method would do better.</span></a>
  <a class="card" href="analysis.md"><img src="img/analysis/card.webp" alt=""><b>Sensitivity and calibration <span class="new">New</span></b><span>Exact sensitivities, Monte Carlo bands, Sobol and Morris indices, fitting to measured data and design optimization.</span></a>
  <a class="card" href="experiments.md"><img src="img/experiments/card.webp" alt=""><b>Experiments and sweeps <span class="new">New</span></b><span>Every run kept for overlays and differences, parallel parameter sweeps, and links that reopen a run.</span></a>
  <a class="card" href="performance.md"><img src="img/performance/card.webp" alt=""><b>Performance analyzer <span class="new">New</span></b><span>Step sizes, the states that limit them, time per algebraic loop, the event log and chattering detection.</span></a>
  <a class="card" href="workbench.md#graphical-editing"><img src="img/editing/card.webp" alt=""><b>Graphical editing <span class="new">New</span></b><span>Drag components onto the diagram, connect, rotate and set parameters; the source keeps its comments and layout.</span></a>
  <a class="card" href="assistant.md"><img src="img/assistant/card.webp" alt=""><b>AI assistant and MCP <span class="new">New</span></b><span>Ask Claude to build, check, simulate and explain models, in the workbench or from Claude Code.</span></a>
  <a class="card" href="animation.md"><img src="img/multibody/sevenpost_rig.webp" alt=""><b>3-D animation</b><span>Replays MultiBody results and every example in 3-D, with camera presets, chase cameras and video recording.</span></a>
  <a class="card" href="multibody.md"><img src="img/multibody/kc_rig.webp" alt=""><b>Multibody vehicles</b><span>Double wishbone, pushrod, multilink and MacPherson corners, K&amp;C and seven-post rigs, a full car.</span></a>
  <a class="card" href="msl.md"><img src="img/msl/card.webp" alt=""><b>Modelica Standard Library</b><span>MSL 4.1.0 loads in the browser out of the box; 424 of its 534 examples match the reference results.</span></a>
  <a class="card" href="https://daedalus.apps.ezkeel.com/scoreboard.html"><img src="img/scoreboard_card.webp" alt=""><b>Accuracy scoreboard <span class="new">New</span></b><span>Every MSL example compared with the reference results, signal by signal, by package.</span></a>
  <a class="card" href="python.md"><img src="img/python_card.webp" alt=""><b>Python API <span class="new">New</span></b><span><code>import daedalus</code>: load, set parameters, simulate to NumPy, linearize and export FMUs in-process.</span></a>
  <a class="card" href="scripting.md"><img src="img/scripting_card.webp" alt=""><b>.mos scripts and trim <span class="new">New</span></b><span>Run Dymola and OpenModelica regression scripts; find steady states and trim with exact Jacobians.</span></a>
  <a class="card" href="examples.md"><img src="img/examples/card.webp" alt=""><b>Example library</b><span>Torque converters, landing gear, aileron actuator, hydraulic quarter car and EV efficiency, rebuilt from engineering blog posts.</span></a>
  <a class="card" href="linearize.md"><img src="img/linearize_bode.png" alt=""><b>Linearization</b><span>Exact A, B, C, D matrices at any time, as text, JSON or an OpenModelica-style <code>linearized_model</code>.</span></a>
  <a class="card" href="fmi.md"><img src="img/fmu_stalltest.png" alt=""><b>FMI 2.0 export</b><span>Model Exchange and Co-Simulation in one FMU, validated with FMPy in CI.</span></a>
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
