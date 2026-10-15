# DAEdalus

DAEdalus is an open (Apache-2.0) equation-based modeling and simulation
environment for the Modelica language. It is built for speed and scale:

- **Rust core.** It compiles natively and to WebAssembly, so the same compiler and
  solvers run on a server, on the command line and in a browser tab.
- **Symbolic backend** in the spirit of the commercial tools:
  - alias elimination
  - Pantelides index reduction with dummy derivatives
  - BLT sorting and tearing
  - exact symbolic Jacobians
  - a compiled register tape for evaluation
- **Solvers.** It has variable-order BDF with dense or sparse LU, and Dopri5,
  RK4 and Euler, with zero-crossing events and `when`/`reinit`.
- **Results.** They are written as Dymola-format `.mat` files, readable by
  Dymola, OMEdit, DyMat and similar tools, or as CSV.
- **Dymola-style workbench.** It runs in the browser and has:
  - a package browser
  - icon, diagram and text views of each class
  - a variable browser
  - a plot window with display units
  - a 3-D animation view of Modelica.Mechanics.MultiBody models
  - simulation setup

All compilation runs in a Web Worker.

- **Modelica Standard Library.** 441 of the 534 MSL 4.x example models
  (83%) translate and simulate, including MultiBody, the QuasiStatic machines,
  `Modelica.Media` (IF97 water, R134a, ideal-gas mixtures, moist air) and
  `Modelica.Fluid` systems.

**Documentation:** https://daedalus.apps.ezkeel.com/docs/ has a getting-started
guide and a page per feature, with example models, screenshots and results. The pages
are the Markdown files in [`docs/`](docs/index.md); `npm run build` in `web/` renders
them to `dist/docs/` (`npm run docs` renders only them).

> Status: under active development. See [docs/architecture.md](docs/architecture.md),
> [docs/examples.md](docs/examples.md) and [docs/roadmap.md](docs/roadmap.md).

## Install

Prebuilt binaries for Linux (x86_64), macOS (Apple Silicon and Intel) and
Windows (x86_64) are attached to each [GitHub Release](https://github.com/ferax564/DAEdalus/releases).
Each archive holds the `daedalus` CLI, the FMI runtime it bundles into FMUs, and
the `library/Daedalus` example library:

```sh
tar xzf daedalus-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
cd daedalus-v0.1.0-x86_64-unknown-linux-gnu
./daedalus simulate library/Daedalus -m Daedalus.Examples.LandingGear.SinkRates -o sink.mat
```

On macOS, clear the download quarantine first with
`xattr -dr com.apple.quarantine <folder>`, since the binaries are not notarized.
The release also carries the `daedalus-casadi` Python wheel
(`pip install daedalus_casadi-*.whl`). It drives the CLI, so put the extracted
folder on `PATH` (or set `DAEDALUS_BIN` to the executable); it then finds the
`library/Daedalus` shipped next to it, or `DAEDALUS_LIB` if set. Maintainers cut a release by pushing a
tag that matches the workspace version, for example `git tag v0.1.0 && git push origin v0.1.0`.

## Quick start

From source:

```sh
cargo build --release

# Translate and print statistics
./target/release/daedalus check library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest

# Simulate (experiment annotation defaults; override with --stop, --tol, --method, -p name=value)
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.LandingGear.SinkRates -o sink.mat

# CSV of chosen variables
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.QuarterCar.BumpComparison \
    -o bump.csv --vars clean.body.s,aerated.body.s

# Run every model with an experiment annotation, each in its own process
./target/release/daedalus test-library library/Daedalus -m Daedalus

# Export the index-reduced DAE for CasADi (optimal control, MPC, estimation)
./target/release/daedalus export-casadi library/Daedalus -m Daedalus.Examples.OptimalControl.AileronServo -o servo.json

# Linear state-space model der(x) = A x + B u, y = C x + D u around the initial
# point (or after simulating to --at T); --format text | json | mo
./target/release/daedalus linearize model.mo -m MyModel --at 1.5 --format mo -o linearized_model.mo
```

`linearize` takes `u` from the model's top-level Real inputs (held at their start
values, 0 without one, while simulating to `--at`) and `y` from its top-level Real
outputs, like Dymola's `linearizeModel` and OpenModelica's `linearize`. The
matrices are exact: a forward-mode run of the compiled tape per state and input,
with algebraic loops differentiated implicitly through their symbolic Jacobians
(`--jacobian fd` uses central differences instead). `--format mo` writes the model
`linearized_model` in the form OpenModelica writes it.

```sh
# Steady state: x with der(x) = 0, from the initial state (or after simulating
# to --at-time T); prints states, derivatives, outputs and the eigenvalues
./target/release/daedalus steady model.mo -m MyModel --at-time 10
# Trim: also solve for inputs/parameters so that variables hit targets
./target/release/daedalus steady model.mo -m MyModel --free u1,u2 --target y1=3,y2=0.5

# Dymola / OpenModelica scripts (simulateModel, simulate, linearizeModel, loadModel, ...)
DAEDALUS_MSL=/path/to/msl ./target/release/daedalus run regression.mos
```

`steady` runs a damped Newton iteration on the state derivatives with the exact
Jacobian of the linearization (Levenberg–Marquardt steps where it is singular).
`run` executes the common scripting commands of both tools so existing
regression scripts run unchanged; see [docs/scripting.md](docs/scripting.md).

```sh
# FMI 2.0 FMU (Model Exchange and Co-Simulation) for linux64
cargo build --release -p daedalus-cli -p daedalus-fmi
./target/release/daedalus fmu library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -o StallTest.fmu
# validate with FMPy and compare its ME and CS runs with `daedalus simulate`
./target/release/daedalus simulate library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -o ref.csv --vars rpm_engine
python3 tools/fmu_check.py StallTest.fmu ref.csv rpm_engine
```

`daedalus fmu` writes `modelDescription.xml` (parameters, inputs, outputs, states,
derivatives and every other variable, with the ModelStructure), the compiled model
in `resources/model.bin`, and the model-independent FMI runtime
`libdaedalus_fmi.so` (crate `daedalus-fmi`, found next to the `daedalus` executable,
or via `--fmi-lib` / `DAEDALUS_FMI_LIB`) as `binaries/linux64/<id>.so`.
Co-Simulation steps with the model's own BDF/Dopri5 solver and event handling;
Model Exchange exposes the states, derivatives and one event indicator per
relation. FMU state get/set and directional derivatives are not provided.

Other source directories, such as the Modelica Standard Library, are loaded by
passing them as extra sources or with `-L DIR`.

## Libraries, packages, diff and parameter data

```sh
# Libraries: the index of open libraries (library/packages.json), install by name,
# GitHub URL or a downloaded zip, into $DAEDALUS_LIBRARIES (default ~/.daedalus/libraries)
./target/release/daedalus lib index
./target/release/daedalus lib install Buildings@12.1.0
./target/release/daedalus lib install https://github.com/owner/MyLib@v1.2.0
./target/release/daedalus lib install ~/Downloads/OpenIPSL-3.0.1.zip
./target/release/daedalus lib list
./target/release/daedalus lib resolve model.mo     # uses(...) -> installed versions, conflicts

# A model with the files and resources (modelica:// URIs) it needs, as a zip; the
# MSL is referred to in the manifest, not copied. A .zip source is opened again.
./target/release/daedalus package library/Daedalus -m Daedalus.Examples.TorqueConverter.Launch -o Launch.zip
./target/release/daedalus simulate Launch.zip -o launch.mat

# Structural diff of a class (components, parameters, modifiers, placements,
# connections, equations); --text adds the unified text diff, --format json
./target/release/daedalus diff old/Plant.mo new/Plant.mo -m Demo.Plant --text

# Parameter data (CSV name,value[,unit] or SSP .ssv) as a modifier; values are checked
# against the component's type and converted to its units (mm -> m, bar -> Pa, ...)
./target/release/daedalus params-from library/Daedalus -m Daedalus.Examples.TorqueConverter.StallTest -c engine engine.csv
#   engine(J = 0.25, tau_drag = 25, w_start = 104.71975511966)
# --record Name prints a record instead; --apply writes the modifier into the source
```

Every command resolves the `uses(...)` annotations of what it loads against the
installed libraries (exact version, else the newest of the same major version) and
warns about conflicting requirements. `lib install` downloads with `curl` from
GitHub; where that is blocked, download the release zip and install the file.

In the workbench:
- **Libraries** lists the libraries loaded with their versions, `uses` conflicts and
  missing libraries, and installs a library of the index, a GitHub URL or a picked zip.
  Browsers cannot read GitHub's archives (no CORS headers), so the files come from
  jsDelivr's GitHub CDN. Installed libraries are kept in IndexedDB for the next visit.
- **Save .zip** writes the open model as a package; **Open** takes such a zip (or a
  library zip) back.
- **Diff** compares the open class with the version as loaded or a picked file: a
  summary, the text diff, and the diagram with added, changed, moved (dashed old
  place) and removed components and connections highlighted.
- **Documentation** renders `info` and `revisions` HTML sanitized (no scripts, event
  handlers or `javascript:` links), with `modelica://` class links that navigate and
  images from library resources. Documentation that does not start with a tag is
  Markdown with `$...$`/`$$...$$` LaTeX (marked and KaTeX, bundled, so offline).
  The `__Daedalus_metadata(key="value", ...)` annotation is shown and edited there.
- **Import parameters…** in the component inspector reads a CSV or SSV file, shows the
  checked values as a modifier and a record, and applies them to the declaration.

## Python

The `daedalus` Python package (`crates/daedalus-py`, PyO3 + maturin, one abi3
wheel for CPython 3.9+) runs the compiler in-process, in place of OMPython or
Dymola's Python interface:

```sh
pip install maturin && cd crates/daedalus-py && maturin develop --release
```

```python
import daedalus

m = daedalus.Model("library/Daedalus", "Daedalus.Examples.TorqueConverter.StallTest")
m.set_parameters({"engine.J": 0.3})
r = m.simulate(stop=2.0, vars=["rpm_engine"])   # GIL released while simulating
r.time, r["rpm_engine"], r.to_pandas(), r.stats

lin = daedalus.Model("crates/daedalus-core/tests/models/linearize.mo", "Lin.MassSpringDamper").linearize()
lin.A, lin.B, lin.C, lin.D

m.export_fmu("StallTest.fmu", fmi_lib="target/release/libdaedalus_fmi.so")
```

See [crates/daedalus-py/README.md](crates/daedalus-py/README.md) for the full API.

## Workbench

```sh
cd web
npm ci
npm run wasm     # builds crates/daedalus-wasm and runs wasm-bindgen 0.2.129
npm run msl      # packs the Modelica Standard Library 4.1.0 into public/msl/
npm run dev      # http://localhost:5173
```

The page loads the bundled `Daedalus` library and the Modelica Standard Library
4.1.0 (`Modelica`, `ModelicaServices`, `Complex`). Other `.mo` files can be
opened from disk.

The MSL sources are not in this repository. `web/scripts/fetch-msl.mjs`
downloads the v4.1.0 release from GitHub and packs its `.mo` and
`package.order` files into `web/public/msl/msl-4.1.0.<hash>.json.gz`
(about 2.6 MB; 14.8 MB unpacked). It also writes `manifest.json`, which names
that bundle, and a copy of the BSD-3-Clause license. To work offline, pass a
local checkout instead:

```sh
node scripts/fetch-msl.mjs /path/to/ModelicaStandardLibrary   # or MSL_DIR=/path/to/... npm run msl
```

The script does nothing when `public/msl` already holds that version. Pass
`--force` to rebuild the bundle. Without a bundle, the workbench runs with the
Daedalus library only.

The worker downloads the bundle after the Daedalus library is loaded and
unpacks it with `DecompressionStream`. It then registers every file without
parsing it. A light token scan outlines each file, so the package browser can
list the whole library within seconds. A class is parsed when it is opened,
checked or simulated, together with the classes it needs (the same on-demand
closure the CLI uses, see `crates/daedalus-core/src/frontend/lazy.rs`).

The `Dockerfile` at the repository root builds the workbench (Rust to
WebAssembly, then Vite) and serves the static bundle with nginx on port 80:

```sh
docker build -t daedalus-web .
docker run --rm -p 8080:80 daedalus-web   # http://localhost:8080
```

The live workbench at https://daedalus.apps.ezkeel.com is built from this
Dockerfile, and ezkeel redeploys it on every push to `main`.

The page is laid out like Dymola:
- the package browser is on the left;
- the class you open shows in Icon, Diagram or Text view;
- **Simulate** compiles the class in the worker and fills the variable browser;
- ticking a variable plots it;
- a model with MultiBody shapes also gets an **Animation** view: a 3-D
  replay with STL/OBJ meshes, studio lighting, camera presets, follow and
  chase cameras, part selection with X-ray, and WebM recording.

## Examples

The `Daedalus.Examples` package rebuilds six technical blog posts from Modelon
and Claytex: torque converters, landing-gear drop tests, an aileron actuator,
a quarter car with a hydraulic damper, vehicle-efficiency sensitivity, and
coast-down road-load fitting. Each model cites its source post. Results and
assumptions are in [docs/examples.md](docs/examples.md).

## Optimal control, MPC and estimation

`daedalus export-casadi` hands a translated model to [CasADi](https://web.casadi.org):
the system after alias elimination, index reduction and tearing, as a
semi-explicit index-1 DAE. The `daedalus_casadi` Python package in `python/`
builds on it with:
- direct-collocation optimal control;
- model predictive control;
- moving horizon estimation of states and parameters.

Costs and constraints refer to model variables by their Modelica names.

```sh
pip install casadi numpy matplotlib
python python/examples/ocp_aileron.py            # minimum-time aileron move: 41.7 ms
python python/examples/mpc_active_suspension.py  # MPC with bump preview: -35 % body acceleration
python python/examples/mhe_coastdown.py          # Cd and crr from a noisy coast-down
```

See [docs/optimal-control.md](docs/optimal-control.md).

## Performance

The benchmark is `tools/gen_chain.py`: a chain of N masses and spring-dampers
from the library, driven by a sine force. It is a stiff system with 2N states,
simulated for 1 s with BDF at tolerance 1e-6, in a release build.

| N | Unknowns after alias elimination | Translate | Simulate |
| ---: | ---: | ---: | ---: |
| 1,000 | 13,000 | 0.25 s | 0.42 s |
| 10,000 | 130,000 | 2.5 s | 3.9 s |

The nine example models simulate in 2.6 s in total. That includes 11 NEDC laps
of the EV model, which take 109k steps.

## Layout

| Path | What |
| --- | --- |
| `crates/daedalus-core` | Frontend (Rumoca parser, instantiation, flattening, class inspection), backend, codegen, solvers, result files |
| `crates/daedalus-cli` | `daedalus` command line tool |
| `crates/daedalus-wasm` | WebAssembly bindings used by the workbench |
| `crates/daedalus-py` | Python package `daedalus` (PyO3, built with maturin; not a cargo workspace member) |
| `crates/daedalus-fmi` | FMI 2.0 runtime (`fmi2*` C functions) of exported FMUs |
| `library/Daedalus` | Component library (Blocks, Mechanics, Hydraulics, Vehicle) and the examples |
| `library/MultiBodyVehicle` | 3-D suspensions (double wishbone with external or pushrod springs, MacPherson, multilink), test rigs (K&C, seven-post) and full-car manoeuvres on MSL `Modelica.Mechanics.MultiBody` (needs MSL 4.1.0) |
| `web` | Browser workbench (TypeScript, Vite, CodeMirror) |
| `python` | `daedalus_casadi`: CasADi optimal control, MPC and MHE on exported models |
| `tools` | Benchmark generator, coast-down fit, FMU check (FMPy) |

Modelica is a registered trademark of the Modelica Association. DAEdalus is not
affiliated with or endorsed by the Modelica Association.
