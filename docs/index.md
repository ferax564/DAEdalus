# DAEdalus

<p class="hero">DAEdalus is a free, open-source tool for simulating physical systems: engines, circuits,
vehicles, hydraulics and control loops. You describe a system in Modelica, a language that builds a model
from parts and the connections between them. DAEdalus runs the model on the command line, on a server or in a
browser tab, with a Dymola-style workbench, the Modelica Standard Library 4.1.0, 3-D animation,
sensitivity and calibration, FMI export and CasADi optimal control.</p>

<div class="cta">
  <a class="btn primary" href="https://daedalus.apps.ezkeel.com/#Daedalus.Examples.TorqueConverter.Launch">Open the workbench</a>
  <a class="btn" href="getting-started.md">Get started</a>
  <a class="btn" href="whats-new.md">What's new</a>
</div>

![The workbench after simulating TorqueConverter.Launch: two cars launch side by side, one with a static K-factor converter and one with fluid torque build-up](img/examples/tc_launch_anim.webp)

The workbench runs entirely in your browser. The compiler and solvers are compiled to
WebAssembly and run in a Web Worker, so there is nothing to install and nothing is sent to a
server. Pick a model in the package browser, press **Simulate**, then plot any variable or
watch the 3-D animation.

## Features

<div class="cards">
  <a class="card" href="linear-analysis.md"><img src="img/linear/card.webp" alt=""><b>Linear analysis <span class="new">New</span></b><span>Bode and Nyquist plots, step and impulse responses, poles and zeros, simpler model versions and an LQR controller, all from the model's exact linear approximation.</span></a>
  <a class="card" href="plotting.md"><img src="img/plotting/card.webp" alt=""><b>Live plots and dashboards <span class="new">New</span></b><span>Results appear while the model runs. Sliders change parameters mid-run. Also: grids of plots, expressions, FFT and statistics.</span></a>
  <a class="card" href="debugger.md"><img src="img/debugger/card.webp" alt=""><b>Equation and init debugger <span class="new">New</span></b><span>See how any variable is computed, from the original equation to the form the solver uses, and why initialization fails.</span></a>
  <a class="card" href="units.md"><img src="img/units/card.webp" alt=""><b>Unit checking <span class="new">New</span></b><span>Every equation is checked for consistent units. Undeclared variables get units inferred for them, and imperial units can be shown.</span></a>
  <a class="card" href="editor.md"><img src="img/editor/card.webp" alt=""><b>Text editor <span class="new">New</span></b><span>Jump to a definition, autocomplete names, see hover information, find every use of a name and rename it safely.</span></a>
  <a class="card" href="libraries.md"><img src="img/libraries/card.webp" alt=""><b>Libraries and packages <span class="new">New</span></b><span>Install libraries such as Buildings and OpenIPSL by name, pack models into a zip, compare versions and import parameter tables.</span></a>
  <a class="card" href="fmi.md"><img src="img/fmu_stalltest.png" alt=""><b>FMI and SSP <span class="new">New</span></b><span>Export FMUs (FMI 2.0 and 3.0) to use in other tools, import FMUs made elsewhere as components, and run SSP systems.</span></a>
  <a class="card" href="analysis.md"><img src="img/analysis/card.webp" alt=""><b>Sensitivity, DOE and calibration <span class="new">New</span></b><span>Fast sensitivities for many parameters at once, experiment designs with response-surface fits, Sobol indices and fitting a model to measured data.</span></a>
  <a class="card" href="embedded.md"><img src="img/embedded/card.webp" alt=""><b>Embedded C and HIL <span class="new">New</span></b><span>Controllers as plain C with no dependencies, for Arduino and ESP32. Test them in software or on hardware, and tune PID gains automatically.</span></a>
  <a class="card" href="sciml.md"><img src="img/sciml/card.webp" alt=""><b>Scientific machine learning <span class="new">New</span></b><span>Neural networks that stand in for parts of a model, and finding missing equations in data with sparse regression (SINDy). Built on the Python API.</span></a>
  <a class="card" href="msl.md"><img src="img/msl/card.webp" alt=""><b>Modelica Standard Library <span class="new">New</span></b><span>MSL 4.1.0 runs in the browser, including its external C functions and exact lookup tables. 488 of its 534 examples match the reference.</span></a>
  <a class="card" href="state-machines.md"><img src="img/state-machines/card.webp" alt=""><b>State machines <span class="new">New</span></b><span>Modelica state machines with transitions, nested states, parallel machines and clocks, running natively and in the browser.</span></a>
  <a class="card" href="workbench.md"><img src="img/wb/card.webp" alt=""><b>Browser workbench</b><span>Package browser, icon, diagram, text and documentation views, a variable browser, and plots in the units you choose.</span></a>
  <a class="card" href="structure.md"><img src="img/structure/card.webp" alt=""><b>Structure and state selection</b><span>See how the equations are grouped, why each state was picked and how stiff the model is. You can choose different states without editing the model.</span></a>
  <a class="card" href="solvers.md"><img src="img/solvers/card.webp" alt=""><b>Solvers</b><span>Several time-stepping methods, including one that switches automatically between a fast method and a stiff one. You also get a hint when another method would work better.</span></a>
  <a class="card" href="experiments.md"><img src="img/experiments/card.webp" alt=""><b>Experiments and sweeps</b><span>Every run is kept for overlays and comparisons. Parameter sweeps run in parallel, and each link reopens its run.</span></a>
  <a class="card" href="performance.md"><img src="img/performance/card.webp" alt=""><b>Performance analyzer</b><span>Shows the step sizes, the states that slow the run down, the time spent in each algebraic loop, the event log, and any chattering.</span></a>
  <a class="card" href="workbench.md#graphical-editing"><img src="img/editing/card.webp" alt=""><b>Graphical editing</b><span>Drag components onto the diagram, connect them, rotate them and set their parameters. The source keeps its comments and layout.</span></a>
  <a class="card" href="assistant.md"><img src="img/assistant/card.webp" alt=""><b>AI assistant and MCP</b><span>Ask Claude to build, check, simulate and explain models, from the workbench or from Claude Code.</span></a>
  <a class="card" href="animation.md"><img src="img/multibody/sevenpost_rig.webp" alt=""><b>3-D animation</b><span>Replays MultiBody results and every example in 3-D, with camera presets, chase cameras and video recording.</span></a>
  <a class="card" href="multibody.md"><img src="img/multibody/kc_rig.webp" alt=""><b>Multibody vehicles</b><span>Double-wishbone, pushrod, multilink and MacPherson suspensions, K&amp;C and seven-post rigs, and a full car.</span></a>
  <a class="card" href="https://daedalus.apps.ezkeel.com/scoreboard.html"><img src="img/scoreboard_card.webp" alt=""><b>Accuracy scoreboard</b><span>Each MSL example compared with the reference results, signal by signal and package by package.</span></a>
  <a class="card" href="python.md"><img src="img/python_card.webp" alt=""><b>Python API</b><span><code>import daedalus</code> loads a model, sets parameters, simulates into NumPy arrays, linearizes it and exports FMUs, all inside your Python program.</span></a>
  <a class="card" href="scripting.md"><img src="img/scripting_card.webp" alt=""><b>.mos scripts and trim</b><span>Run the regression scripts from Dymola and OpenModelica, and find steady states with exact derivatives.</span></a>
  <a class="card" href="examples.md"><img src="img/examples/card.webp" alt=""><b>Example library</b><span>Torque converters, landing gear, an aileron actuator, a hydraulic quarter car and EV efficiency, rebuilt from engineering blog posts.</span></a>
  <a class="card" href="linearize.md"><img src="img/linearize_bode.png" alt=""><b>Linearization</b><span>The exact A, B, C and D matrices of a model at any time, as text, JSON or an OpenModelica-style <code>linearized_model</code>.</span></a>
  <a class="card" href="optimal-control.md"><img src="img/ocp_aileron.png" alt=""><b>Optimal control</b><span>Fastest moves, model predictive control (MPC) and moving horizon estimation, built with CasADi straight from Modelica models.</span></a>
</div>

## Under the hood

Before any numerical solving, DAEdalus does the same symbolic work as the commercial tools:

- It removes duplicate variables (alias elimination), handles equations that constrain
  each other (Pantelides index reduction with dummy derivatives), sorts the equations into the
  order they must be solved (BLT sorting and tearing), and derives exact Jacobians
  symbolically. A Jacobian holds how each equation changes with each variable.
- It compiles the model into a short register program that runs the same way natively and
  in WebAssembly.
- It offers several time-stepping methods: variable-order BDF, Radau IIA and ESDIRK for stiff
  models, Dopri5 with automatic switching to BDF when the model turns stiff, and RK4 and Euler
  for simple cases. It also handles events, such as a zero crossing, a `when` clause and `reinit`.
- Results are saved as Dymola-format `.mat` files, which Dymola, OMEdit and DyMat can read, or as CSV.

It scales close to linearly. A chain of 10,000 masses (130,000 unknowns after alias
elimination) translates in 2.5 s and simulates one second of time in 3.9 s. See
[Architecture](architecture.md) for how each stage works.

## Where to go next

- [Getting started](getting-started.md): use the live workbench, install a release, or
  build from source.
- [What's new](whats-new.md): the features added most recently.
- [Example library](examples.md): every bundled example, with its results.
- [Command line](cli.md): every `daedalus` command and option.
- [Python API](python.md): using DAEdalus from Python and Jupyter, and
  [scientific machine learning](sciml.md) built on top of it.
- [Embedded C and processor in the loop](embedded.md): controllers on Arduino and ESP32.
- [Structure and state selection](structure.md) and [Sensitivity and calibration](analysis.md):
  understanding a model and its parameters.

> DAEdalus is under active development. Modelica is a registered trademark of the
> Modelica Association; DAEdalus is not affiliated with or endorsed by it.
