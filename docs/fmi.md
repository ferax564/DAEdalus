# FMI and SSP

`daedalus fmu` exports a model as a Functional Mock-up Unit for FMI 2.0 or 3.0, so it runs in any
FMI importer: Simulink, Dymola, OpenModelica, FMPy, Silver, or your own co-simulation
master. One FMU carries both interfaces:

- **Co-Simulation.** The FMU steps itself with the model's own BDF or Dopri5 solver and
  event handling, inside `fmi2DoStep`.
- **Model Exchange.** The importer integrates; the FMU exposes the states, their
  derivatives and one event indicator per relation.

## Exporting

```sh
cargo build --release -p daedalus-cli -p daedalus-fmi
./target/release/daedalus fmu library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -o StallTest.fmu
```

```
FMU Daedalus_Examples_TorqueConverter_StallTest: 552 variables, 2 states, 2 event indicators, 0 inputs, 0 outputs
wrote StallTest.fmu
```

The archive holds:

| File | What |
| --- | --- |
| `modelDescription.xml` | Parameters, inputs, outputs, states, derivatives and every other variable, with the `ModelStructure` |
| `resources/model.bin` | The compiled model: the tape and its metadata |
| `binaries/linux64/<id>.so` | The model-independent FMI runtime (`daedalus-fmi`) |

The runtime is the same library for every model; only `model.bin` changes. `daedalus fmu`
finds it next to the `daedalus` executable, or through `--fmi-lib` or
`DAEDALUS_FMI_LIB`. The [release archives](getting-started.md#2-release-binaries) ship it
next to the CLI.

## Validated with FMPy

CI exports `StallTest.fmu` on every pull request, validates `modelDescription.xml` with
FMPy, runs the FMU both as Co-Simulation and as Model Exchange (with FMPy's CVode), and
compares both runs with `daedalus simulate`:

```sh
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest \
    -o ref.csv --vars rpm_engine,rpm_stall_theory
python3 tools/fmu_check.py StallTest.fmu ref.csv rpm_engine,rpm_stall_theory
```

```
fmpy validate StallTest.fmu: no problems found
CS: 1001 rows to t = 4.0
  CS  rpm_engine: max deviation 4.97e-06 of its range (final 2160.538718 vs 2160.538639)
  CS  rpm_stall_theory: max deviation 8.78e-07 of its range (final 2160.538718 vs 2160.538705)
ME: 1001 rows to t = 4.0
  ME  rpm_engine: max deviation 3.28e-06 of its range (final 2160.538718 vs 2160.538639)
  ME  rpm_stall_theory: max deviation 8.90e-07 of its range (final 2160.538718 vs 2160.538705)
OK
```

![The FMU in FMPy, as Co-Simulation and as Model Exchange, on top of the native run](img/fmu_stalltest.png)

## FMI 3.0

`--fmi-version 3` writes the same model as an FMI 3.0 FMU (`x86_64-linux`):

```sh
daedalus fmu library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest --fmi-version 3 -o StallTest3.fmu
```

```
FMU Daedalus_Examples_TorqueConverter_StallTest (FMI 3.0): 552 variables, 2 states, 2 event indicators, 0 inputs, 0 outputs
wrote StallTest3.fmu
```

The same runtime implements the `fmi3*` API: Float64, Int32 and Boolean variables,
`fmi3UpdateDiscreteStates`, and exact directional derivatives from the forward tangents
of the compiled equations, available after initialization.

## Using the FMU from Python

```python
from fmpy import simulate_fmu

res = simulate_fmu("StallTest.fmu", fmi_type="CoSimulation",
                   stop_time=4, output=["rpm_engine"])
print(res["rpm_engine"][-1])   # 2160.5
```

Parameters are set the usual way, with `start_values={"engine.J": 0.3}`.

## Importing FMUs

DAEdalus also runs FMUs from other tools, FMI 2.0 or 3.0, on their own or as components
of a Modelica model:

```sh
# simulate an FMU on its own: Model Exchange if it has it
daedalus simulate StallTest.fmu -o fmu.csv --vars rpm_engine
daedalus simulate StallTest.fmu --fmi-type cs --comm-step 0.004 -o cs.csv

# a wrapper block to use as a component in any model
daedalus import-fmu Plant.fmu -o Plant.mo [--fmi-type me|cs] [--comm-step H] [--class NAME] [--within PKG]
```

The wrapper declares the FMU's parameters with their units and descriptions, and its
inputs and outputs as connectors:

```modelica
block StallTest "Daedalus.Examples.TorqueConverter.StallTest (DAEdalus) (FMU, ModelExchange)"
  parameter Real engine_tau_drag(unit = "N.m") = 20.0 "Friction (motoring) torque at 3000 rpm (engine.tau_drag)";
  parameter Real engine_J(unit = "kg.m2") = 0.2 "Engine (crank + flywheel) inertia (engine.J)";
  ...
```

and an annotation `__Daedalus_FMU(fileName = "Plant.fmu", fmiType = "ModelExchange")`
that names the file relative to the `.mo`.

- **Model Exchange.** The FMU's states are integrated together with the model's, and its
  event indicators are located like the model's own zero crossings. Its time events and
  discrete state updates take part in the event iteration, and outputs that depend
  directly on inputs are iterated to a fixed point. Every integrator works; the implicit
  ones use a difference Jacobian, since DAEdalus cannot see inside an FMU.
- **Co-Simulation.** The FMU is stepped over the communication grid, with its inputs held
  over each step. Its outputs change at the grid points, so a signal through a chain of
  Co-Simulation FMUs is delayed one step per FMU.

FMU components need the native tools, the command line or the Python package. The browser
workbench reports an error for them, and `linearize` and `steady` refuse them.
`crates/daedalus-cli/tests/fmu_import.rs` imports exported FMUs back and compares them
with the native simulation.

## SSP

[SSP](https://ssp-standard.org) packages describe systems of connected FMUs:

```sh
daedalus simulate System.ssp --comm-step 0.01 -o sys.csv   # run as a co-simulation master
daedalus import-ssp System.ssp -o System.mo                 # one model, a wrapper per FMU
daedalus export-ssp . -m Loop -o Loop.ssp                   # a model made of FMU components
```

`import-ssp` reads `SystemStructure.ssd`: the FMU components, parameter bindings inline
or in `.ssv` files, connections, system connectors and the default experiment. Nested
systems are not supported. `export-ssp` packs a model's FMU components with their
parameter values, placements and connections.

## Limits

- The binaries are built for `linux64`; Windows and macOS FMUs need `daedalus-fmi` built
  on those platforms.
- FMU state get/set is not provided; FMI 2.0 FMUs have no directional derivatives (the
  FMI 3.0 ones do).

