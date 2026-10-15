//! DAEdalus core: Modelica instantiation, symbolic backend, code generation and solvers.

pub mod backend;
pub mod calibrate;
pub mod casadi;
pub mod codegen;
pub mod expr;
pub mod external;
pub mod flat;
pub mod fmi;
#[cfg(all(feature = "fmu", not(target_arch = "wasm32")))]
pub mod fmu;
pub mod frontend;
pub mod linalg;
pub mod linearize;
pub mod modal;
pub mod profile;
pub mod optimize;
pub mod results;
pub mod sensitivity;
pub mod sim;
pub mod solver;
pub mod solver_auto;
pub mod solver_esdirk;
pub mod solver_jac;
pub mod solver_radau;
pub mod sparse;
pub mod sweep;
pub mod structure;
pub mod uq;
pub mod steady;
pub mod table1;
pub mod table2;

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
