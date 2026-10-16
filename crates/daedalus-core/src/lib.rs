//! DAEdalus core: Modelica instantiation, symbolic backend, code generation and solvers.

#[cfg(feature = "archive")]
pub mod archive;
pub mod adjoint;
pub mod autotune;
pub mod backend;
pub mod calibrate;
pub mod casadi;
pub mod codegen;
pub mod diff;
pub mod edit;
pub mod debug;
pub mod doe;
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
pub mod libraries;
pub mod ide;
pub mod lbfgsb;
pub mod linalg;
pub mod linearize;
pub mod params;
pub mod modal;
pub mod optimize;
pub mod lti;
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
pub mod sweep;
pub mod tape_reverse;
pub mod structure;
pub mod uq;
pub mod steady;
pub mod units;
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
