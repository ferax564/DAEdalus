# Command line

`daedalus` is one executable with a command per task. Every command takes the model
sources (`.mo` files or directories, loaded recursively) and the model's full name:

```sh
daedalus <command> [sources...] -m <Model> [options]
```

Only the classes a model needs are parsed, so pointing at a large library costs little;
parsed library files are cached on disk between runs (`~/.cache/daedalus`; `DAEDALUS_CACHE`
moves it and `DAEDALUS_NO_CACHE` turns it off).

## Commands

| Command | What it does |
| --- | --- |
| `check` | Translate the model and print statistics: variables, aliases removed, states, algebraic loops, events, tape size. |
| `flatten` | Print the flat model. |
| `dump` | Print the sorted, causalized equation system (BLT blocks, tearing). |
| `simulate` | Simulate and write a Dymola `.mat` file or a CSV file. |
| `linearize` | State-space matrices A, B, C, D. See [Linearization](linearize.md). |
| `steady` | Steady state (`der(x) = 0`) with its stability, or a trim with `--free` and `--target`. See [Scripts and steady states](scripting.md#steady-states-and-trim). |
| `fmu` | Export an FMI 2.0 FMU. See [FMI 2.0 export](fmi.md). |
| `export-casadi` | Write the index-reduced DAE as JSON for CasADi. See [Optimal control](optimal-control.md). |
| `run` | Run a Dymola or OpenModelica script (`.mos`). See [Scripts and steady states](scripting.md). |
| `test-library` | Translate and simulate every model with an `experiment` annotation in a package, each in its own process with a time limit, and print a report. |

The same compiler is also a Python package, `import daedalus`, for notebooks and
scripts: see [Python API](python.md).

## Simulating

```sh
# Experiment annotation defaults
daedalus simulate library/Daedalus -m Daedalus.Examples.LandingGear.SinkRates -o sink.mat

# Override the interval, tolerance, method and parameters
daedalus simulate library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest \
    --stop 6 --tol 1e-8 --method dopri5 -p engine.J=0.3 -o stall.mat

# CSV of selected variables
daedalus simulate library/Daedalus -m Daedalus.Examples.QuarterCar.BumpComparison \
    -o bump.csv --vars clean.body.s,aerated.body.s
```

```
translated in 11.7 ms; simulated in 7.2 ms (158 steps, 10 rejected (0 Newton), 378 f-evals, 3 jacobians, 2 events)
```

`.mat` results use Dymola's `dsres` layout, so Dymola, OMEdit, DyMat, ModelicaRes and
existing post-processing scripts read them.

## Libraries

The bundled `Daedalus` library is a source directory like any other, and so is the
Modelica Standard Library:

```sh
git clone -b v4.1.0 --depth 1 https://github.com/modelica/ModelicaStandardLibrary msl
daedalus simulate msl/Modelica msl/ModelicaServices msl/Complex.mo \
    -m Modelica.Mechanics.Rotational.Examples.CoupledClutches -o clutches.mat
```

`-L DIR` adds a library directory the same way, and `DAEDALUS_LIB` (or `--lib`) points at
the bundled library.

## Steady states and trim

```sh
daedalus steady crates/daedalus-core/tests/models/steady.mo -m Steady.Pendulum \
    --free tau --target angle=0.5
```

```
trim of Steady.Pendulum at t = 0 (2 Newton iterations, residual 0.000e0)

states:
  phi = 0.5
  w   = 0

free variables:
  tau = 3.7625316269657856

targets:
  angle = 0.5 (target 0.5)

eigenvalues of the state Jacobian (stable):
  -2.343750e-1 - 3.272067e0i
  -2.343750e-1 + 3.272067e0i
```

The torque that holds the pendulum at 0.5 rad is `m·g·L·sin(0.5)` = 3.76 N·m, and the
equilibrium is stable. Without `--free` and `--target`, `steady` finds the equilibrium
for the inputs as they are. See [Scripts and steady states](scripting.md#steady-states-and-trim).

## Scripts

```sh
daedalus run regression.mos --msl ~/ModelicaStandardLibrary
```

runs a Dymola or OpenModelica `.mos` script: loading files and libraries, translating,
`simulateModel` and `simulate`, parameter sweeps with `simulateExtendedModel`,
`linearizeModel`, `steadyState` and `trim`, with loops and conditionals. See
[Scripts and steady states](scripting.md).

## Testing a whole library

```sh
daedalus test-library library/Daedalus -m Daedalus
```

Each model runs in its own process, in parallel (`--jobs N`, default: all cores), with a
time limit (`--timeout S`, default 60). CI runs this on every pull request, and a nightly
job runs the MSL examples against their reference results (see
[Modelica Standard Library](msl.md)).

## Options

| Option | Meaning |
| --- | --- |
| `-m, --model NAME` | Model to translate (full dotted name) |
| `-L, --lib DIR` | Additional library directory (repeatable) |
| `--start T`, `--stop T` | Simulation interval |
| `--interval DT` | Output interval |
| `-n, --intervals N` | Number of output intervals (default 500) |
| `--tol TOL` | Relative tolerance (default 1e-6) |
| `--method M` | `bdf`, `dopri5`, `rk4` or `euler` |
| `--step H` | Fixed step size |
| `--store-events E` | Rows written at events: `all`, `none` or a number |
| `-p, --param NAME=VALUE` | Parameter override (repeatable) |
| `-o, --output FILE` | Result file: `.mat` (Dymola format) or `.csv` |
| `--vars a,b,c` | Variables written to CSV, or printed |
| `--no-index-reduction` | Disable Pantelides index reduction |
| `--no-tearing` | Disable tearing |
| `--jobs N` | Parallel processes for `test-library` |
| `--timeout S` | Per-model time limit for `test-library` |
| `--at T` | `linearize`: simulate to time T first |
| `--format F` | `linearize`: `text`, `json` or `mo` |
| `--jacobian J` | `linearize`: `exact` or `fd` (central differences) |
| `--fmi-lib FILE` | `fmu`: the FMI runtime library to bundle |
| `--at-time T` | `steady`: start from the state at time T |
| `--free a,b`, `--target y=v` | `steady`: trim these inputs or parameters to hit these targets |
| `--msl DIR` | `run`: an extra library root for `loadModel` |

## Performance

The benchmark is `tools/gen_chain.py`: a chain of N masses and spring-dampers driven by a
sine force, a stiff system with 2N states, simulated for 1 s with BDF at tolerance 1e-6.

| N | Unknowns after alias elimination | Translate | Simulate |
| ---: | ---: | ---: | ---: |
| 1,000 | 13,000 | 0.25 s | 0.42 s |
| 10,000 | 130,000 | 2.5 s | 3.9 s |

```sh
python3 tools/gen_chain.py 2000 > chain.mo
daedalus simulate library/Daedalus chain.mo -m Chain --stop 0.2 -o chain.mat
```
