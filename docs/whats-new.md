# What's new

The most recent additions, newest first. Each links to the page that covers it in full.

## Structure explorer and state selection

The new **Structure** tab and `daedalus structure` show the BLT blocks, the incidence
matrix, why each state was chosen, and the state Jacobian's eigenvalues and stiffness.
States can be pinned or forbidden there, or with `--state NAME=prefer` on any command,
without editing the model, and a check along the run warns when a static choice breaks
down. See [Structure and state selection](structure.md).

![The Structure view of TorqueConverter.Launch](img/structure/launch.webp)

## Radau, ESDIRK and automatic switching

Two more stiff solvers, Radau IIA (order 5) and ESDIRK (order 4), and `auto`, which
switches between Dopri5 and BDF as the model turns stiff and back. The implicit methods
use an exact Jacobian from the compiled model, and the workbench recommends another
method when a run's statistics show it is using the wrong kind. See [Solvers](solvers.md).

## Sensitivity, uncertainty, calibration and optimization

The **Analysis** mode and `daedalus sensitivity | montecarlo | gsa | calibrate | optimize`:
exact sensitivities, Monte Carlo prediction bands, Sobol and Morris indices, fitting
parameters to measured data with standard errors, and design optimization. See
[Sensitivity and calibration](analysis.md).

<div class="pair">
<figure><img src="img/analysis/sobol.webp" alt="Sobol indices"></figure>
<figure><img src="img/analysis/calibrate.webp" alt="Calibration against measured data"></figure>
</div>

## Experiments, sweeps and share links

Every simulation is kept in the **Runs** pane to overlay or subtract, **Sweep…** runs a
parameter grid on all cores in the browser, `daedalus sweep` does the same on the command
line, and **Share link** copies an address that reruns the experiment. See
[Experiments and sweeps](experiments.md).

## Performance analyzer

**Profile** (or `--profile`) records the step sizes, the states that limit them, the time
in each algebraic loop, the event log and chattering relations, with hints in plain
language. See [Performance analyzer](performance.md).

## Graphical editing

The Diagram view is now an editor for your own models: drag components in, connect them,
rotate, flip and set parameters, with undo. Edits change only the text they must. See
[Workbench](workbench.md#graphical-editing).

## AI assistant and MCP server

**✦ Assistant** in the workbench is a chat with Claude that can read, write, check and
simulate models and read the structure and profile reports, with your own API key.
`daedalus mcp` gives Claude Code and other MCP clients the same tools. See
[AI assistant and MCP](assistant.md).

## MSL accuracy scoreboard

The [scoreboard](https://daedalus.apps.ezkeel.com/scoreboard.html) compares every MSL
4.1.0 example with the Modelica Association's reference results, signal by signal:
**424 of 534 match**, 49 run but differ somewhere, 56 fail and 5 hit the time limit.
It breaks the results down by package, shows the signals that differ and the error
text for each model, and links each example to the workbench.

![The MSL accuracy scoreboard](img/scoreboard.webp)

See [Modelica Standard Library](msl.md#checked-against-the-reference-results).

## Python API

The `daedalus` Python package (`crates/daedalus-py`, built with maturin) runs the compiler
in-process, with no server and no licence: load a model, change parameters, simulate to
NumPy arrays or pandas, linearize and export FMUs.

```python
import daedalus

m = daedalus.Model("library/Daedalus", "Daedalus.Examples.TorqueConverter.StallTest")
m.set_parameters({"engine.J": 0.3})
r = m.simulate(stop=2.0, vars=["rpm_engine"])
r.time, r["rpm_engine"]
```

See [Python API](python.md), which also maps OMPython and Dymola calls onto it.

## `.mos` scripts, steady states and trim

`daedalus run script.mos` runs Dymola and OpenModelica regression scripts:
`loadModel`, `simulateModel`, `simulate`, `simulateExtendedModel`, `linearizeModel`,
`val`, loops and conditionals. `daedalus steady` finds equilibria with an exact Jacobian
and reports their stability, and with `--free` and `--target` it trims, for example the
input that holds an output at a set value. See [Scripts and steady states](scripting.md).

## 3-D animation for every example

After **Simulate**, models with shapes open in the new **Animation** view: a WebGL scene
with studio lighting, camera presets, follow and chase cameras, X-ray selection and WebM
recording. It draws every `Modelica.Mechanics.MultiBody` shape in a result, including STL
and OBJ meshes, and the bundled examples got their own visuals through
`Daedalus.Visualizers`: cars that drive the cycles side by side, the landing gear dropping
onto the runway, the aileron turning on its hinge.

<div class="pair">
<figure><img src="img/examples/landing_gear_anim.webp" alt="Landing gear drop tests at three sink rates"></figure>
<figure><img src="img/examples/nedc_anim.webp" alt="Four electric cars driving the NEDC cycle"></figure>
</div>

See [3-D animation](animation.md).

## Multibody suspensions, rigs and a full car

`library/MultiBodyVehicle` builds vehicle models on `Modelica.Mechanics.MultiBody`:
double-wishbone, pushrod, multilink and MacPherson corners, an anti-roll bar, a tyre with
lift-off, wheel pads and aero loaders, and a 6-DOF car. Nine examples cover suspension
kinematics, a K&C rig, a seven-post rig, step steer, lane change and a one-sided bump, and
all nine match OpenModelica 1.26.1 to within 0.04 % of each signal's range while running
several times faster.

<video src="media/sevenpost_rig.mp4" poster="img/multibody/sevenpost_rig.webp" controls muted loop playsinline preload="none"></video>

See [Multibody vehicles](multibody.md).

## Back and forward between classes

Every class you open is a browser history entry with its own address
(`/#Package.Model`), so Back and Forward, Alt+← and Alt+→, mouse side buttons and the
arrows next to the class name walk through them, and a link opens a class directly. The
workbench colours now follow the light or dark system theme everywhere, diagrams included.
See [Workbench](workbench.md).

## Release binaries

Pushing a version tag builds and publishes a
[GitHub Release](https://github.com/ferax564/DAEdalus/releases) with the CLI, the FMI
runtime and the example library for Linux, macOS (Apple Silicon and Intel) and Windows,
plus the `daedalus-casadi` Python wheel. See [Getting started](getting-started.md#2-release-binaries).

## Optimal control, MPC and estimation with CasADi

`daedalus export-casadi` hands the index-reduced DAE to CasADi, and the `daedalus_casadi`
Python package solves optimal control problems by direct collocation, runs model
predictive control and estimates states and parameters with moving horizon estimation,
all referring to model variables by their Modelica names.

![Minimum-time aileron move: full-open valve, then a reverse braking pulse, 41.7 ms](img/ocp_aileron.png)

See [Optimal control](optimal-control.md).

## Modelica Standard Library 4.1.0 in the browser

The workbench loads MSL 4.1.0 (`Modelica`, `ModelicaServices`, `Complex`) out of the box.
The package browser lists all of it within seconds, and a class is parsed only when it is
opened or simulated. A nightly job compares every MSL example with the Modelica
Association's reference results. See [Modelica Standard Library](msl.md).

## Linearization

`daedalus linearize` gives the exact state-space matrices A, B, C and D around the initial
point or after simulating to a time, as text, JSON or an OpenModelica-style
`linearized_model`. See [Linearization](linearize.md).

## FMI 2.0 export

`daedalus fmu` writes one FMU with both Model Exchange and Co-Simulation, validated with
FMPy in CI. See [FMI 2.0 export](fmi.md).

## Multibody closed loops

Planar kinematic loops on a free body, driven loops and steady-state initialization now
work, which is what the suspension models above are built on.

## In progress

This is being built now and gets its page here when it lands:

- external C libraries (`Modelica.Utilities`, `Math.Random` and friends) and more
  lookup-table support.

The longer-term plan is in the [Roadmap](roadmap.md).
