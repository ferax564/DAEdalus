//! `daedalus fmu`: write an FMI 2.0 or 3.0 FMU (Model Exchange and Co-Simulation).
//!
//! The FMU is written by [`daedalus_core::fmu::write_fmu`]; this module finds the
//! model-independent runtime `libdaedalus_fmi.so` (crate `daedalus-fmi`) it embeds.

use anyhow::{Context, bail};
use daedalus_core::codegen::Model;
use daedalus_core::fmi;
use std::path::{Path, PathBuf};

/// The prebuilt FMI runtime: `--fmi-lib`, `$DAEDALUS_FMI_LIB`, or next to the
/// `daedalus` executable (also its `deps/` folder in a cargo target directory).
fn find_runtime(explicit: Option<&Path>) -> anyhow::Result<PathBuf> {
    if let Some(p) = explicit {
        if p.is_file() {
            return Ok(p.to_path_buf());
        }
        bail!("FMI runtime library {} not found", p.display());
    }
    if let Some(p) = std::env::var_os("DAEDALUS_FMI_LIB") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        bail!("DAEDALUS_FMI_LIB={} not found", p.display());
    }
    let file = daedalus_core::fmu::runtime_file_name();
    let exe = std::env::current_exe()?;
    if let Some(dir) = exe.parent() {
        for cand in [dir.join(&file), dir.join("deps").join(&file)] {
            if cand.is_file() {
                return Ok(cand);
            }
        }
    }
    bail!(
        "cannot find the FMI runtime {file} next to {}: build it with \
         `cargo build --release -p daedalus-fmi`, or pass --fmi-lib / set DAEDALUS_FMI_LIB",
        exe.display()
    )
}

pub fn export(m: &Model, out: &Path, runtime: Option<&Path>, version: u32) -> anyhow::Result<()> {
    let lib = find_runtime(runtime)?;
    let lib_bytes = std::fs::read(&lib).with_context(|| lib.display().to_string())?;
    let desc = daedalus_core::fmu::write_fmu_version(
        m,
        out,
        &lib_bytes,
        &format!("DAEdalus {}", env!("CARGO_PKG_VERSION")),
        version,
    )?;
    eprintln!(
        "FMU {} (FMI {version}.0): {} variables, {} states, {} event indicators, {} inputs, {} outputs",
        fmi::model_identifier(&m.name),
        desc.vars.len(),
        desc.n_states,
        desc.n_event_indicators,
        desc.vars
            .iter()
            .filter(|v| v.causality == fmi::Causality::Input)
            .count(),
        desc.outputs.len()
    );
    Ok(())
}
