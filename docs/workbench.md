# Workbench

The workbench is a Dymola-style modeling and simulation environment that runs in the
browser. The compiler and solvers are the same Rust code as the command line, compiled to
WebAssembly and run in a Web Worker, so the page stays responsive while a model
translates or simulates, and models never leave your machine.

[Open the workbench](https://daedalus.apps.ezkeel.com/#Daedalus.Examples.TorqueConverter.StallTest)

## Layout

![The workbench in Modeling mode: package browser, diagram, component inspector and messages](img/wb/component_selected.webp)

- **Toolbar.** **Modeling** and **Simulation** switch modes. **Open .mo** and **Library**
  load your own files or folders. **Check** translates the model and reports its
  statistics. **Stop**, **Intervals**, **Tol** and **Method** override the model's
  `experiment` annotation for the next run, and **▶ Simulate** (Ctrl+Enter) runs it.
- **Packages** (left). Every loaded library: `Daedalus`, the MSL (`Modelica`,
  `ModelicaServices`, `Complex`) and whatever you opened. Models with a ▶ can be simulated.
  The filter box narrows the tree to matching classes.
- **Class views** (centre). The open class in one of five views, with Back and Forward
  arrows next to its name.
- **Component** (right). Click a component in the diagram to see its class, description
  and parameter values; **Open** jumps to that class.
- **Messages** (bottom). Translation statistics, timings, warnings and errors.

## Class views

<div class="pair">
<figure><img src="img/wb/icon.webp" alt="Icon view"><figcaption>Icon: the class's icon layer.</figcaption></figure>
<figure><img src="img/wb/text.webp" alt="Text view"><figcaption>Text: the Modelica source in an editor with syntax highlighting, search and undo. Edits are re-parsed as you type, syntax errors show in Messages, and the next Check or Simulate uses them.</figcaption></figure>
<figure><img src="img/wb/doc.webp" alt="Documentation view"><figcaption>Documentation: the class's HTML documentation annotation.</figcaption></figure>
<figure><img src="img/wb/flat.webp" alt="Flat model view"><figcaption>Flat model: every scalar variable and equation after flattening.</figcaption></figure>
</div>

The **Diagram** view draws components, connections and graphics from their annotations,
with zoom and pan (wheel and drag). It works for the bundled library and for the MSL:

![ChuaCircuit from the MSL in the diagram view](img/msl/chua_diagram.webp)

## Filtering the package browser

![The package browser filtered to Examples](img/wb/filter.webp)

## Simulation and the variable browser

**▶ Simulate** compiles the model in the worker and switches to Simulation mode. The left
pane becomes the variable browser: the result's component tree, with each variable's
value at the end of the run and its display unit. Typing in the filter keeps the matching
variables. Ticking a variable plots it.

![The variable browser filtered to engine, next to the StallTest animation](img/wb/variables.webp)

Parameters are editable in the variable browser: change a value and simulate again, and
the run uses the new value without touching the source. Overrides belong to the model they
were made on and are cleared when you open another class.

## Plots

The plot window draws thousands of points per series smoothly and has:

- **Display units**, as in Dymola: `rad/s` shows as `rpm`, `rad` as `deg`, `Pa` as
  `bar`, `K` as `degC`, `m3/s` as `l/min` and `J` as `kJ`;
- **subplots** (**New subplot**) that share the time axis and zoom together;
- **parametric plots**: choose a variable as the **x-axis** of a subplot to plot one
  variable against another, such as a damper's force against its velocity;
- a **crosshair readout** of every series under the mouse, and drag-to-zoom;
- **Export CSV** and **Export .mat** of the plotted variables.

![QuarterCar.BumpComparison: body travel over the bump, and the damper's force-velocity loop as a parametric plot](img/examples/quarter_car_plot.webp)

## Animation

Models that contain MultiBody shapes, or the visualizers of the bundled examples, open in
the **Animation** view after a simulation. See [3-D animation](animation.md).

![TorqueConverter.StallTest animated: engine, converter, held turbine and brake](img/examples/tc_stall_anim.webp)

## Dark mode

The workbench follows the system's light or dark theme. Diagrams keep their hues with
inverted lightness, so MSL icons stay readable.

<div class="pair">
<figure><img src="img/wb/dark_diagram.webp" alt="Dark diagram view"></figure>
<figure><img src="img/wb/dark_plot.webp" alt="Dark plot view"></figure>
</div>

## Navigation and links

Each class you open is a browser history entry with its own address, `/#` followed by the
class name. Back and Forward, Alt+← and Alt+→, the mouse side buttons and the arrows next
to the class name walk through them. A link to `/#Some.Class` opens that class, including
MSL classes, which open once the library has loaded. Every example on these pages links to
its model this way.

## How loading works

The page loads the bundled `Daedalus` library, then downloads the MSL bundle (about
2.6 MB, 14.8 MB unpacked) and unpacks it with `DecompressionStream`. A light token scan
outlines every file so the package browser can list the whole library within seconds; a
class is parsed only when it is opened, checked or simulated, together with the classes
it needs.
