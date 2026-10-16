# Scripts and steady states

## `daedalus run script.mos`

A script is a list of commands, the same as typing them one at a time. Many teams keep scripts that
simulate a model and check the results against expected values; these are called regression scripts.

`daedalus run` runs Dymola and OpenModelica scripts (`.mos`). It covers the commands
that regression scripts use, so most scripts run without changes.

```sh
daedalus run regression.mos                 # library roots from the environment
daedalus run regression.mos --msl ~/msl     # plus this library root (repeatable)
daedalus run regression.mos -L mylib/MyLib  # load a library before the script starts
```

Paths in the script are relative to the directory `daedalus run` was started in,
until the script calls `cd(...)`.

The exit code is:
- the value given to `exit(n)`;
- otherwise 1 if any command failed (a command returned `false`, or an unknown
  function was called), and 0 if none did.

Errors stop the script immediately. These are syntax errors, unknown variables,
and wrong argument types; each is reported with its line number.

### Example

A Dymola-style regression script, with a parameter sweep:

```modelica
openModel("Plant.mo");
ok := simulateModel("Plant.FirstOrder(k=4)", stopTime=5, numberOfIntervals=100,
  method="Dassl", tolerance=1e-8, resultFile="FirstOrder_k4");
if not ok then
  print("simulation failed");
  exit(2);
end if;
for T in {0.5, 1.0} loop
  (ok, finals) := simulateExtendedModel("Plant.FirstOrder", stopTime=20,
    initialNames={"T"}, initialValues={T}, finalNames={"y", "x"}, resultFile="sweep");
  print("T = " + String(T) + ": y(20) = " + String(finals[1]));
end for;
linearizeModel("Plant.Pendulum", resultFile="pendulum_lin");
s := steadyState("Plant.FirstOrder(k=4)");
s.x
```

```
= true
T = 0.5: y(20) = 5.000000014410608
T = 1: y(20) = 4.999999692229807
wrote pendulum_lin.mat
= true
= 8.0
```

The full script and `Plant.mo` are in `crates/daedalus-cli/tests/mos/`, which CI runs.

### Output

Every statement that is not an assignment prints its value in Dymola's style,
whether or not it ends with `;`:

```
= true
= 4.98
= {1.0, 2.0}
```

Records print in OpenModelica's layout (`record SimulationResult ... end SimulationResult;`).
Errors and warnings go to stderr. `getErrorString()` returns them (and clears them)
as OpenModelica does.

### Language

The interpreter supports a subset of the Modelica scripting language:
- statements: expressions, assignments (`:=` or `=`), `(a, b) := f(...)`, and
  declarations (`Real x;`, `Boolean ok = ...;`, `String s[2];`, where the type is
  not checked);
- control flow: `for i in 1:n loop ... end for;` (also over arrays `{..}`),
  `while ... loop ... end while;`, and `if / elseif / else`;
- expressions:
  - Real, Integer, Boolean and String literals;
  - arrays `{..}`, `[..]` and ranges `a:b`, `a:s:b`;
  - `+ - * / ^`, the relational operators, `and`, `or` and `not`;
  - `if`-expressions and indexing `x[i]`;
  - record fields `r.name`;
  - string concatenation with `+`;
- functions: `String`, `integer`, `abs`, `sqrt`, `sin`, `cos`, `tan`, `exp`,
  `log`, `log10`, `floor`, `ceil`, `min`, `max`, `sum`, `size` and `mod`;
- comments: `//` and `/* */`.

### Commands

| Command | Notes |
| --- | --- |
| `loadFile(file)`, `openModel(file)` | A `.mo` file. A `package.mo` file or a directory loads the whole package; its files are parsed when a model needs them. |
| `loadModel(Modelica)`, `loadModel(Modelica, {"4.1.0"})` | Searches the library roots, in order: `--msl DIR`, `$DAEDALUS_MSL`, `$MODELICAPATH` (path list), the bundled library's parent directory (`$DAEDALUS_LIB`), and `~/.openmodelica/libraries`. A root may hold `Modelica 4.1.0/`, `Modelica/`, `Complex.mo`, ..., or be the library directory itself. The version comes from the directory name, else from `annotation(version = ...)`. `ModelicaServices` and `Complex` are loaded with `Modelica` when found. |
| `cd(dir)`, `cd()` | Changes the script's directory, creating it if missing. Returns the directory. |
| `translateModel(problem)`, `checkModel(problem)`, `buildModel(problem)` | Translate. Return `true` or `false`. |
| `instantiateModel(class)` | The flat model as a string. |
| `simulateModel(problem, startTime, stopTime, numberOfIntervals, outputInterval, method, tolerance, fixedstepsize, resultFile)` | Dymola. Writes `resultFile.mat` (default `dsres.mat`) in Dymola's format, or `.csv` when the name ends in `.csv`. Returns `true` or `false`. |
| `simulateExtendedModel(..., initialNames, initialValues, finalNames)` | Dymola. `initialNames` must be parameters (or inputs). Returns `{ok, finalValues}`, for `(ok, v) := ...`. |
| `simulate(className, startTime, stopTime, numberOfIntervals, tolerance, method, fileNamePrefix, ..., outputFormat)` | OpenModelica. Writes `<fileNamePrefix>_res.mat` (or `.csv`) and returns a `SimulationResult` record with `resultFile`. `options`, `variableFilter`, `cflags` and `simflags` are ignored, with a warning. |
| `linearizeModel(problem, startTime, stopTime, ..., resultFile)` | Dymola. Writes `dslin.mat` (`Aclass`, `nx`, `xuyName`, `ABCD = [A B; C D]`). A `resultFile` ending in `.json` writes the JSON of `daedalus linearize --format json` instead. Linearizes at `stopTime` if one is given (after simulating to it), else after initialization. |
| `linearize(className, stopTime = ...)` | OpenModelica. Writes `linearized_model.mo`. |
| `steadyState(problem, atTime, tolerance)` | See [Steady states and trim](#steady-states-and-trim). Returns a `SteadyState` record with fields `converged`, `time`, `iterations`, `residual`, `stable`, and one field per state, input and output (for example `s.x`). |
| `trim(problem, free, targetNames, targetValues, atTime, tolerance)` | Trim, for example `trim("M", {"u"}, {"y"}, {3.0})`. Returns a `SteadyState` record that also holds the free variables. |
| `val(var, time[, file])` | OpenModelica. The value at `time` from the last result, or from `file`. |
| `getErrorString()`, `getVersion()`, `print(s)`, `exit(n)`, `RunScript(file)`, `removeFile(f)`, `regularFileExists(f)` | |
| `plot(...)`, `createPlot(...)`, `plotExpression(...)` and other plot commands | No effect. A note says that no plot was made. |
| `Advanced.* = ...`, `Hidden.* = ...`, `OutputCPUtime = ...`, `experimentSetupOutput(...)`, `setCommandLineOptions(...)`, ... | Tool settings. Ignored, with a warning. |

Problems take modifiers in the class name: `"P.M(k = 4, sub(T = 0.1))"`. The
modifiers become parameter overrides, so changing them does not need a new
translation. They must be numbers or Booleans.

Arguments that are not given fall back to the model's `experiment` annotation,
then to the defaults below:
- 0 to 1 s;
- 500 intervals;
- BDF.

Method names map onto DAEdalus' solvers:

| Script method | Solver |
| --- | --- |
| `dassl`, `ida`, `cvode`, `radau`, ... | BDF |
| `dopri5`, `dopri45`, `rkfix` | Dopri5 |
| `rk4` | RK4 |
| `euler` | Euler |

Unknown methods fall back to BDF with a warning.

### Not supported

Some parts of the scripting language are not implemented:
- user-defined script functions;
- `Modelica.Utilities` beyond `print`;
- modifiers that are not numbers (redeclarations, strings, arrays);
- `simulateMultiExtendedModel`;
- FMU import;
- plotting.

## Steady states and trim

```sh
daedalus steady model.mo -m M                       # from the initial state
daedalus steady model.mo -m M --at-time 10          # from the state at t = 10
daedalus steady model.mo -m M --free u1,u2 --target y1=3 --target y2=0.5
daedalus steady model.mo -m M --format json -o steady.json
```

`steady` finds the states `x` with `der(x) = 0` at a fixed time. It prints:
- the states and their derivatives;
- the inputs and outputs;
- the eigenvalues of `d der(x) / dx` at the solution, with a stability verdict.

With `--free` and `--target` it trims. The listed inputs or parameters become
unknowns as well, and the targets become extra equations, so there must be as many
targets as free variables. A target can be any variable of the model.

Parameters computed from a free parameter are re-evaluated with it. For example,
`cEff = 2 * c` follows `c`.

The iteration starts from the initial state, or from the state reached by
simulating to `--at-time`. It is a damped Newton iteration:
- The Jacobian is exact: a forward-mode run of the compiled tape per unknown, as in
  `linearize`. Algebraic loops are differentiated implicitly.
- Each step is backtracked on the residual norm. The norm is weighted by the size of
  the terms in each equation.
- When the Jacobian is singular, for example for the position of a free mass, the
  iteration takes Levenberg–Marquardt steps instead. The undetermined states stay
  where they were.

The iteration has converged when every residual, scaled by the size of the terms in
its equation, is below 1e-10.

The library function is `daedalus_core::steady::steady_state(&model, sim_options,
&SteadyOptions { at_time, free, targets, .. })`.

Limitations:
- Relations (`if` conditions) are held at their values while Newton iterates. They
  are re-evaluated at the solution, and the solve is repeated when one of them
  changed.
- When-clauses and other discrete changes are not executed.
- Starting from a point where the Jacobian vanishes can stall, for example
  `der(x) = 1 - x^3` at `x = 0`. Simulate first (`--at-time`) to start closer to
  the solution.
- A trim whose free variables cannot affect its targets at an equilibrium is
  ill-posed. It ends in an error, or in a degenerate point that only satisfies the
  scaled residual test, such as a time constant driven toward infinity. Check the
  free values the solve reports.
