//! Writing an FMI 2.0 FMU (Model Exchange and Co-Simulation) for a compiled model
//! (feature `fmu`; used by `daedalus fmu` and the Python package).
//!
//! The FMU holds `modelDescription.xml`, the compiled model serialized with bincode in
//! `resources/model.bin` ([`fmi::Bundle`]), and the model-independent runtime
//! `libdaedalus_fmi.so` (crate `daedalus-fmi`) as `binaries/<platform>/<modelIdentifier>.so`.
//! Finding that prebuilt runtime is up to the caller.

use crate::codegen::Model;
use crate::fmi;
use anyhow::Context;
use std::io::Write;
use std::path::Path;

/// Same encoding as [`fmi::Bundle`], without cloning the model.
#[derive(serde::Serialize)]
struct BundleRef<'a> {
    guid: &'a str,
    model: &'a Model,
}

/// The FMI platform folder and shared-library extension of the host.
pub fn platform() -> (&'static str, &'static str) {
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

/// File name of the FMI runtime library on this host (`libdaedalus_fmi.so`, ...).
pub fn runtime_file_name() -> String {
    format!(
        "{}daedalus_fmi{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

/// Write the FMU of `m` to `out`, with `runtime` (the bytes of the prebuilt
/// `daedalus-fmi` library) as its binary. `generator` goes into the model description's
/// `generationTool`. Returns the model description.
pub fn write_fmu(
    m: &Model,
    out: &Path,
    runtime: &[u8],
    generator: &str,
) -> anyhow::Result<fmi::Description> {
    write_fmu_version(m, out, runtime, generator, 2)
}

/// The FMI 3.0 platform tuple of the host (`x86_64-linux`, ...).
pub fn platform3() -> &'static str {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("linux", "aarch64") => "aarch64-linux",
        ("linux", "x86") => "x86-linux",
        ("macos", "aarch64") => "aarch64-darwin",
        ("macos", _) => "x86_64-darwin",
        ("windows", "x86") => "x86-windows",
        ("windows", _) => "x86_64-windows",
        _ => "x86_64-linux",
    }
}

/// [`write_fmu`] for FMI version `version` (2 or 3).
pub fn write_fmu_version(
    m: &Model,
    out: &Path,
    runtime: &[u8],
    generator: &str,
    version: u32,
) -> anyhow::Result<fmi::Description> {
    if !m.externals.is_empty() {
        anyhow::bail!(
            "the model contains FMU components ({}): exporting it as an FMU is not supported",
            m.externals
                .iter()
                .map(|e| e.file.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if version != 2 && version != 3 {
        anyhow::bail!("FMI version {version} is not supported (2 or 3)");
    }
    let model_bytes = bincode::serialize(m)?;
    let guid = fmi::guid_of(&model_bytes);
    drop(model_bytes);
    let bundle = bincode::serialize(&BundleRef {
        guid: &guid,
        model: m,
    })?;
    let id = fmi::model_identifier(&m.name);
    let desc = fmi::describe(m);
    let xml = if version == 3 {
        fmi::model_description_xml3(m, &desc, &id, &guid, generator)
    } else {
        fmi::model_description_xml(m, &desc, &id, &guid, generator)
    };
    let (plat, ext) = platform();
    let plat = if version == 3 { platform3() } else { plat };
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
    z.write_all(runtime)?;
    z.add_directory("resources/", opts)?;
    z.start_file("resources/model.bin", opts)?;
    z.write_all(&bundle)?;
    z.finish()?.flush()?;
    Ok(desc)
}
