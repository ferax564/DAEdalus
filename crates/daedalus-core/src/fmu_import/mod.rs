//! FMU import (feature `fmu`, native only): FMI 2.0 and FMI 3.0 FMUs, Model Exchange
//! and Co-Simulation, as components of Modelica models and as SSP systems.
//!
//! - [`Fmu::open`] unpacks an FMU and reads its `modelDescription.xml` ([`md`]);
//!   [`binding::Instance`] loads its binary for this platform and calls the C API.
//! - [`wrapper`] generates the Modelica wrapper block of an FMU (`daedalus import-fmu`):
//!   parameters, inputs and outputs from the model description, and an annotation
//!   `__Daedalus_FMU(fileName = ..., fmiType = ...)` that marks the block as an external
//!   component (see [`crate::external`]).
//! - [`hook`] evaluates the FMU components of a model inside the simulator: Model
//!   Exchange FMUs contribute their states, derivatives and event indicators to the
//!   model's integrator; Co-Simulation FMUs are stepped at their communication step
//!   with inputs held over the step.
//! - [`ssp`] reads and writes SSP archives (`SystemStructure.ssd` with FMU components).

pub mod binding;
pub mod hook;
pub mod md;
pub mod ssp;
pub mod wrapper;

use anyhow::{Context, Result, bail};
use md::{FmiVersion, ModelDescription};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// An unpacked FMU.
pub struct Fmu {
    /// The FMU file (or directory) it was opened from.
    pub source: PathBuf,
    /// Directory the FMU was unpacked into.
    pub dir: PathBuf,
    pub md: ModelDescription,
    /// Remove `dir` when dropped.
    temporary: bool,
    libs: Mutex<HashMap<String, &'static libloading::Library>>,
}

static COUNTER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// Extract the zip archive `file` into `dir` (entries that would leave `dir` are
/// skipped).
pub fn unzip(file: &Path, dir: &Path) -> Result<()> {
    let f = std::fs::File::open(file).with_context(|| file.display().to_string())?;
    let mut z = zip::ZipArchive::new(std::io::BufReader::new(f))
        .with_context(|| format!("{}: not a zip archive", file.display()))?;
    for i in 0..z.len() {
        let mut e = z.by_index(i)?;
        let Some(rel) = e.enclosed_name() else {
            continue;
        };
        let out = dir.join(rel);
        if e.is_dir() {
            std::fs::create_dir_all(&out)?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p)?;
        }
        let mut bytes = Vec::with_capacity(e.size() as usize);
        e.read_to_end(&mut bytes)?;
        std::fs::write(&out, bytes)?;
    }
    Ok(())
}

/// A fresh temporary directory.
pub fn temp_dir(tag: &str) -> Result<PathBuf> {
    let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let clean: String = tag
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .take(40)
        .collect();
    let dir = std::env::temp_dir().join(format!("daedalus-{clean}-{}-{n}", std::process::id()));
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

impl Fmu {
    /// Open an `.fmu` archive (unpacked into a temporary directory) or an unpacked FMU
    /// directory.
    pub fn open(path: &Path) -> Result<Fmu> {
        let (dir, temporary) = if path.is_dir() {
            (path.to_path_buf(), false)
        } else {
            if !path.is_file() {
                bail!("FMU {} not found", path.display());
            }
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("fmu");
            let dir = temp_dir(&format!("fmu-{stem}"))?;
            unzip(path, &dir)?;
            (dir, true)
        };
        let xml = std::fs::read_to_string(dir.join("modelDescription.xml"))
            .with_context(|| format!("{}: no modelDescription.xml", path.display()))?;
        let md = ModelDescription::parse(&xml)
            .with_context(|| format!("{}: modelDescription.xml", path.display()))?;
        Ok(Fmu {
            source: path.to_path_buf(),
            dir,
            md,
            temporary,
            libs: Mutex::new(HashMap::new()),
        })
    }

    /// FMI platform directory of this host for the FMU's FMI version.
    pub fn platform(&self) -> &'static str {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        match self.md.version {
            FmiVersion::V2 => crate::fmu::platform().0,
            FmiVersion::V3 => match (os, arch) {
                ("linux", "x86_64") => "x86_64-linux",
                ("linux", "aarch64") => "aarch64-linux",
                ("linux", "x86") => "x86-linux",
                ("macos", "aarch64") => "aarch64-darwin",
                ("macos", _) => "x86_64-darwin",
                ("windows", "x86") => "x86-windows",
                ("windows", _) => "x86_64-windows",
                _ => "x86_64-linux",
            },
        }
    }

    /// Load (once) the binary of `model_identifier`. The library stays loaded for the
    /// life of the process: unloading FMU binaries is not reliably safe.
    pub fn library(&self, model_identifier: &str) -> Result<&'static libloading::Library, String> {
        let mut libs = self.libs.lock().map_err(|e| e.to_string())?;
        if let Some(l) = libs.get(model_identifier) {
            return Ok(l);
        }
        let plat = self.platform();
        let file = self.dir.join("binaries").join(plat).join(format!(
            "{model_identifier}{}",
            std::env::consts::DLL_SUFFIX
        ));
        if !file.is_file() {
            let have: Vec<String> = std::fs::read_dir(self.dir.join("binaries"))
                .map(|d| {
                    d.filter_map(|e| e.ok())
                        .map(|e| e.file_name().to_string_lossy().into_owned())
                        .collect()
                })
                .unwrap_or_default();
            return Err(format!(
                "{}: no binary for this platform ({plat}/{model_identifier}{}){}",
                self.source.display(),
                std::env::consts::DLL_SUFFIX,
                if have.is_empty() {
                    " - the FMU has no binaries (source-only FMUs are not supported)".to_string()
                } else {
                    format!(" - it has binaries for: {}", have.join(", "))
                }
            ));
        }
        let lib = unsafe { libloading::Library::new(&file) }
            .map_err(|e| format!("loading {}: {e}", file.display()))?;
        let lib: &'static libloading::Library = Box::leak(Box::new(lib));
        libs.insert(model_identifier.to_string(), lib);
        Ok(lib)
    }

    /// `resources` directory as a `file:` URI (FMI 2).
    pub fn resource_uri(&self) -> String {
        let p = self.dir.join("resources");
        let mut s = String::from("file://");
        for c in p.to_string_lossy().chars() {
            match c {
                ' ' => s.push_str("%20"),
                '%' => s.push_str("%25"),
                '\\' => s.push('/'),
                c => s.push(c),
            }
        }
        if !s.starts_with("file:///") {
            s = s.replacen("file://", "file:///", 1);
        }
        s
    }

    /// `resources` directory as a path with a trailing separator (FMI 3).
    pub fn resource_path(&self) -> String {
        let mut s = self.dir.join("resources").to_string_lossy().into_owned();
        s.push(std::path::MAIN_SEPARATOR);
        s
    }
}

impl Drop for Fmu {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}
