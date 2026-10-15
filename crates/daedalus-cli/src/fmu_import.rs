//! FMU and SSP import: `daedalus import-fmu`, `import-ssp`, `export-ssp`, and `.fmu` /
//! `.ssp` files as sources of `simulate` (see `daedalus_core::fmu_import`).

use anyhow::{Context, bail};
use daedalus_core::external::FmiKind;
use daedalus_core::fmu_import::Fmu;
use daedalus_core::fmu_import::ssp::{self, SspImportOptions};
use daedalus_core::fmu_import::wrapper::{self, WrapperOptions};
use daedalus_core::frontend::Library;
use std::path::{Component, Path, PathBuf};

/// Options shared by the import commands.
#[derive(Default)]
pub struct ImportOptions {
    pub kind: Option<FmiKind>,
    pub class: Option<String>,
    pub within: Option<String>,
    pub step: Option<f64>,
    /// Expose the FMU's local variables as outputs.
    pub locals: bool,
}

fn absolute(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| {
        std::env::current_dir()
            .map(|d| d.join(p))
            .unwrap_or_else(|_| p.to_path_buf())
    })
}

/// `to` relative to the directory `from` (both absolute), with `..` where needed.
fn relative(to: &Path, from: &Path) -> PathBuf {
    let a: Vec<Component> = to.components().collect();
    let b: Vec<Component> = from.components().collect();
    let common = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    if common == 0 {
        return to.to_path_buf();
    }
    let mut r = PathBuf::new();
    for _ in common..b.len() {
        r.push("..");
    }
    for c in &a[common..] {
        r.push(c.as_os_str());
    }
    r
}

fn pick_kind(fmu: &Fmu, want: Option<FmiKind>) -> anyhow::Result<FmiKind> {
    let md = &fmu.md;
    Ok(match want {
        Some(FmiKind::ModelExchange) if md.me.is_none() => {
            bail!("{} does not support Model Exchange", fmu.source.display())
        }
        Some(FmiKind::CoSimulation) if md.cs.is_none() => {
            bail!("{} does not support Co-Simulation", fmu.source.display())
        }
        Some(k) => k,
        None if md.me.is_some() => FmiKind::ModelExchange,
        None => FmiKind::CoSimulation,
    })
}

fn class_name_for(path: &Path, opts: &ImportOptions) -> String {
    opts.class.clone().unwrap_or_else(|| {
        wrapper::ident(path.file_stem().and_then(|s| s.to_str()).unwrap_or("FMU"))
    })
}

/// `daedalus import-fmu X.fmu -o X.mo`: write the wrapper block of an FMU.
pub fn import_fmu(fmu_path: &Path, out: Option<&str>, opts: &ImportOptions) -> anyhow::Result<()> {
    let fmu = Fmu::open(fmu_path)?;
    let kind = pick_kind(&fmu, opts.kind)?;
    let class = class_name_for(fmu_path, opts);
    let out = out
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{class}.mo")));
    let out_dir = absolute(
        out.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    );
    let file = relative(&absolute(fmu_path), &out_dir);
    let text = wrapper::wrapper_text(
        &fmu.md,
        &class,
        &file.to_string_lossy().replace('\\', "/"),
        kind,
        &WrapperOptions {
            within: opts.within.clone(),
            experiment: false,
            step: opts.step,
            locals: opts.locals,
        },
    );
    std::fs::write(&out, text).with_context(|| out.display().to_string())?;
    let itf = wrapper::interface(&fmu.md);
    eprintln!(
        "wrote {} (block {class}: FMI {} {}, {} parameters, {} inputs, {} outputs, {} states)",
        out.display(),
        fmu.md.version_str(),
        kind.as_str(),
        itf.params.len(),
        itf.inputs.len(),
        itf.outputs.len(),
        if kind == FmiKind::ModelExchange {
            fmu.md.states.len()
        } else {
            0
        }
    );
    if !itf.skipped.is_empty() {
        eprintln!(
            "warning: not exposed (strings, arrays or unsupported types): {}",
            itf.skipped.join(", ")
        );
    }
    Ok(())
}

/// What a `.fmu` or `.ssp` source turned into: Modelica text with its top class, and
/// whatever must live until the simulation is done.
pub struct Generated {
    pub uri: String,
    pub text: String,
    pub model: String,
    _ssp: Option<ssp::ImportedSsp>,
}

/// A `.fmu` source: its wrapper as a runnable top-level model (inputs at their start
/// values, the FMU's default experiment).
pub fn fmu_source(path: &Path, opts: &ImportOptions) -> anyhow::Result<Generated> {
    let fmu = Fmu::open(path)?;
    let kind = pick_kind(&fmu, opts.kind)?;
    let class = class_name_for(path, opts);
    let abs = absolute(path);
    let text = wrapper::wrapper_text(
        &fmu.md,
        &class,
        &abs.to_string_lossy(),
        kind,
        &WrapperOptions {
            within: None,
            experiment: true,
            step: opts.step,
            locals: true,
        },
    );
    Ok(Generated {
        uri: format!("{}.mo", abs.display()),
        text,
        model: class,
        _ssp: None,
    })
}

/// A `.ssp` source: its system as a Modelica model (unpacked for the run).
pub fn ssp_source(path: &Path, opts: &ImportOptions) -> anyhow::Result<Generated> {
    let imp = ssp::import(
        path,
        &SspImportOptions {
            kind: opts.kind,
            step: opts.step,
            relative: false,
        },
    )?;
    Ok(Generated {
        uri: format!("{}.mo", absolute(path).display()),
        text: imp.text.clone(),
        model: imp.model.clone(),
        _ssp: Some(imp),
    })
}

/// `daedalus import-ssp X.ssp -o X.mo`: the system as a Modelica model, with the FMUs
/// unpacked next to it (paths as in the archive).
pub fn import_ssp(path: &Path, out: Option<&str>, opts: &ImportOptions) -> anyhow::Result<()> {
    let imp = ssp::import(
        path,
        &SspImportOptions {
            kind: opts.kind,
            step: opts.step,
            relative: true,
        },
    )?;
    let out = out
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(format!("{}.mo", imp.model)));
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."))
        .to_path_buf();
    for f in &imp.fmus {
        let dst = dir.join(f);
        if let Some(p) = dst.parent() {
            std::fs::create_dir_all(p)?;
        }
        std::fs::copy(imp.dir.join(f), &dst)
            .with_context(|| format!("copying {f} to {}", dst.display()))?;
    }
    std::fs::write(&out, &imp.text).with_context(|| out.display().to_string())?;
    eprintln!(
        "wrote {} (model {}, {} FMUs next to it)",
        out.display(),
        imp.model,
        imp.fmus.len()
    );
    Ok(())
}

fn placement_extent(v: &serde_json::Value) -> Option<[f64; 4]> {
    let e = v.get("transformation")?.get("extent")?.as_array()?;
    let p = |k: usize, j: usize| e.get(k)?.as_array()?.get(j)?.as_f64();
    Some([p(0, 0)?, p(0, 1)?, p(1, 0)?, p(1, 1)?])
}

/// `daedalus export-ssp ... -m System -o System.ssp`: a model whose components are FMU
/// wrapper blocks, as an SSP archive.
pub fn export_ssp(lib: &Library, model: &str, out: &Path) -> anyhow::Result<()> {
    let m = daedalus_core::translate(lib, model, &Default::default())?;
    if m.externals.is_empty() {
        bail!("{model} has no FMU components (instances of blocks made by `daedalus import-fmu`)");
    }
    let info = daedalus_core::frontend::inspect::class_info(lib, model)
        .ok_or_else(|| anyhow::anyhow!("model '{model}' not found"))?;
    let values = daedalus_core::fmi::parameter_values(&m);
    let index: std::collections::HashMap<&str, usize> = m
        .vars
        .iter()
        .enumerate()
        .map(|(i, v)| (v.name.as_str(), i))
        .collect();
    let mut comps = Vec::new();
    for e in &m.externals {
        if e.prefix.is_empty() || e.prefix.contains('.') || e.prefix.contains('[') {
            bail!(
                "FMU component '{}': only FMU components declared directly in {model} can be \
                 exported to SSP",
                e.prefix
            );
        }
        let fmu = Fmu::open(Path::new(&e.file))?;
        let mut params = std::collections::HashMap::new();
        for id in ssp::parameter_idents(&fmu.md) {
            if let Some(&k) = index.get(format!("{}.{id}", e.prefix).as_str()) {
                params.insert(id, values[k]);
            }
        }
        let placement = info
            .components
            .iter()
            .find(|c| c.name == e.prefix)
            .and_then(|c| c.placement.as_ref())
            .and_then(placement_extent);
        comps.push(ssp::ExportComponent {
            name: e.prefix.clone(),
            fmu: PathBuf::from(&e.file),
            params,
            placement,
        });
    }
    let is_fmu = |port: &str| {
        port.split_once('.')
            .is_some_and(|(c, _)| comps.iter().any(|x| x.name == c))
    };
    let mut conns = Vec::new();
    for c in &info.connections {
        if is_fmu(&c.from) && is_fmu(&c.to) {
            conns.push(ssp::ExportConnection {
                from: c.from.clone(),
                to: c.to.clone(),
            });
        } else {
            eprintln!(
                "warning: connect({}, {}) is not between FMU components: not exported",
                c.from, c.to
            );
        }
    }
    let others: Vec<&str> = info
        .components
        .iter()
        .filter(|c| c.variability != "parameter" && c.variability != "constant")
        .filter(|c| !comps.iter().any(|x| x.name == c.name))
        .map(|c| c.name.as_str())
        .collect();
    if !others.is_empty() {
        eprintln!(
            "warning: components that are not FMUs are not exported: {}",
            others.join(", ")
        );
    }
    ssp::export(
        model.rsplit('.').next().unwrap_or(model),
        &comps,
        &conns,
        m.experiment.start_time,
        m.experiment.stop_time,
        out,
    )?;
    eprintln!(
        "wrote {} ({} FMU components, {} connections)",
        out.display(),
        comps.len(),
        conns.len()
    );
    Ok(())
}
