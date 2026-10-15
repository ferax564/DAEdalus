# What's new

The most recent additions, newest first. Each links to the page that covers it in full.

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

These are being built now and get their pages here when they land:

- external C libraries (`Modelica.Utilities`, `Math.Random` and friends) and more
  lookup-table support;
- from the competitor survey: a Jacobian and structure view, state selection control,
  more solvers and sensitivity analysis.

The longer-term plan is in the [Roadmap](roadmap.md).
