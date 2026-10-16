//! DAEdalus core: Modelica instantiation, symbolic backend, code generation and solvers.

pub mod adjoint;
#[cfg(feature = "archive")]
pub mod archive;
pub mod autotune;
pub mod backend;
pub mod calibrate;
pub mod casadi;
pub mod codegen;
pub mod debug;
pub mod diff;
pub mod doe;
pub mod edit;
pub mod embedded;
pub mod embedded_hil;
pub mod expr;
pub mod external;
pub mod external_component;
pub mod flat;
pub mod fmi;
#[cfg(all(feature = "fmu", not(target_arch = "wasm32")))]
pub mod fmu;
#[cfg(all(feature = "fmu", not(target_arch = "wasm32")))]
pub mod fmu_import;
pub mod frontend;
pub mod ide;
pub mod jit;
pub mod lbfgsb;
pub mod libraries;
pub mod linalg;
pub mod linearize;
pub mod lti;
pub mod modal;
pub mod optimize;
pub mod params;
pub mod preflight;
pub mod profile;
pub mod results;
pub mod sensitivity;
pub mod sim;
pub mod solver;
pub mod solver_auto;
pub mod solver_esdirk;
pub mod solver_jac;
pub mod solver_radau;
pub mod sparse;
pub mod steady;
pub mod structure;
pub mod sweep;
pub mod table1;
pub mod table2;
pub mod tape_reverse;
pub mod units;
pub mod uq;

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
