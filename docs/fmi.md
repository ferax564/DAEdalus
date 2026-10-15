# FMI 2.0 export

`daedalus fmu` exports a model as a Functional Mock-up Unit for FMI 2.0, so it runs in any
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

## Using the FMU from Python

```python
from fmpy import simulate_fmu

res = simulate_fmu("StallTest.fmu", fmi_type="CoSimulation",
                   stop_time=4, output=["rpm_engine"])
print(res["rpm_engine"][-1])   # 2160.5
```

Parameters are set the usual way, with `start_values={"engine.J": 0.3}`.

## Limits

- The binaries are built for `linux64`; Windows and macOS FMUs need `daedalus-fmi` built
  on those platforms.
- FMU state get/set (`fmi2GetFMUstate`) and directional derivatives are not provided yet.
- In Model Exchange, a jump in a time table without an event can stall an importer's
  integrator; time events for table breakpoints are planned.

FMI 3.0 and FMU import are on the [Roadmap](roadmap.md).
