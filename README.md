# DAEdalus

DAEdalus is an open-core (Apache-2.0) equation-based modeling and simulation
environment for the Modelica language, built for speed and scale:

- **Rust core**, compiled natively and to WebAssembly, so the same compiler and
  solvers run in a browser tab and on a server.
- **Symbolic backend** in the spirit of the commercial tools: alias elimination,
  Pantelides index reduction with dummy derivatives, BLT sorting, tearing,
  exact Jacobians, then a compiled evaluation tape.
- **Dymola-style workbench** in the browser: package browser, diagram / icon /
  text views, plot window and variable browser.

> Status: under active development. See `docs/` for the architecture and roadmap.

## Layout

| Path | What |
| --- | --- |
| `crates/daedalus-core` | Frontend (parsing via Rumoca, instantiation, flattening), backend, codegen, solvers |
| `crates/daedalus-cli` | `daedalus` command line tool |
| `crates/daedalus-wasm` | WebAssembly bindings used by the web UI |

## Build

```sh
cargo build --release
./target/release/daedalus model.mo Package.Model
```

Modelica is a registered trademark of the Modelica Association. DAEdalus is not
affiliated with or endorsed by the Modelica Association.
