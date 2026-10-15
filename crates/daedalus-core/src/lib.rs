//! DAEdalus core: Modelica instantiation, symbolic backend, code generation and solvers.

pub mod backend;
pub mod casadi;
pub mod codegen;
pub mod expr;
pub mod flat;
pub mod fmi;
#[cfg(all(feature = "fmu", not(target_arch = "wasm32")))]
pub mod fmu;
pub mod frontend;
pub mod linalg;
pub mod linearize;
pub mod results;
pub mod sim;
pub mod solver;
pub mod sparse;
pub mod steady;
pub mod units;

/// Flatten, analyse and compile `model` from `lib` into an executable model.
pub fn translate(
    lib: &frontend::Library,
    model: &str,
    opts: &backend::Options,
) -> anyhow::Result<codegen::Model> {
    let flat = lib.flatten(model)?;
    let sys = backend::compile(flat, opts)?;
    Ok(codegen::compile(sys))
}
