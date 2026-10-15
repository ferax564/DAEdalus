//! DAEdalus core: Modelica instantiation, symbolic backend, code generation and solvers.

pub mod backend;
pub mod casadi;
pub mod codegen;
pub mod expr;
pub mod flat;
pub mod fmi;
pub mod frontend;
pub mod linalg;
pub mod linearize;
pub mod results;
pub mod sim;
pub mod solver;
pub mod solver_auto;
pub mod solver_esdirk;
pub mod solver_jac;
pub mod solver_radau;
pub mod sparse;

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
