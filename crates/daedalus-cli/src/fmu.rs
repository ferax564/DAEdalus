//! `daedalus fmu`: write an FMI 2.0 FMU (Model Exchange and Co-Simulation).
//!
//! The FMU holds `modelDescription.xml`, the compiled model serialized with bincode in
//! `resources/model.bin`, and the model-independent runtime `libdaedalus_fmi.so`
//! (crate `daedalus-fmi`) as `binaries/<platform>/<modelIdentifier>.so`.

use anyhow::{Context, bail};
use daedalus_core::codegen::Model;
use daedalus_core::fmi;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Same encoding as [`fmi::Bundle`], without cloning the model.
#[derive(serde::Serialize)]
struct BundleRef<'a> {
    guid: &'a str,
    model: &'a Model,
}

fn platform() -> (&'static str, &'static str) {
    let dir = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "x86_64") => "linux64",
        ("linux", "x86") => "linux32",
        ("macos", _) => "darwin64",
        ("windows", "x86_64") => "win64",
        ("windows", _) => "win32",
        _ => "linux64",
    };
    let ext = match std::env::consts::OS {
        "macos" => "dylib",
        "windows" => "dll",
        _ => "so",
    };
    (dir, ext)
}

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
    let file = format!(
        "{}daedalus_fmi{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    );
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

pub fn export(m: &Model, out: &Path, runtime: Option<&Path>) -> anyhow::Result<()> {
    let lib = find_runtime(runtime)?;
    let lib_bytes = std::fs::read(&lib).with_context(|| lib.display().to_string())?;
    let model_bytes = bincode::serialize(m)?;
    let guid = fmi::guid_of(&model_bytes);
    drop(model_bytes);
    let bundle = bincode::serialize(&BundleRef {
        guid: &guid,
        model: m,
    })?;
    let id = fmi::model_identifier(&m.name);
    let desc = fmi::describe(m);
    let xml = fmi::model_description_xml(
        m,
        &desc,
        &id,
        &guid,
        &format!("DAEdalus {}", env!("CARGO_PKG_VERSION")),
    );
    let (plat, ext) = platform();
    let f = std::fs::File::create(out).with_context(|| out.display().to_string())?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    z.start_file("modelDescription.xml", opts)?;
    z.write_all(xml.as_bytes())?;
    z.add_directory("binaries/", opts)?;
    z.add_directory(format!("binaries/{plat}/"), opts)?;
    z.start_file(
        format!("binaries/{plat}/{id}.{ext}"),
        opts.unix_permissions(0o755),
    )?;
    z.write_all(&lib_bytes)?;
    z.add_directory("resources/", opts)?;
    z.start_file("resources/model.bin", opts)?;
    z.write_all(&bundle)?;
    z.finish()?.flush()?;
    eprintln!(
        "FMU {id}: {} variables, {} states, {} event indicators, {} inputs, {} outputs",
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
