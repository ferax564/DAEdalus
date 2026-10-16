//! Commands that manage sources rather than simulate them: `diff`, `params-from`,
//! `lib` (install and resolve libraries) and `package` (a model with its files and
//! resources as a zip), plus the `uses` resolution and `.zip` sources that every
//! command shares.

use anyhow::{Context, Result, anyhow, bail};
use daedalus_core::archive;
use daedalus_core::frontend::Library;
use daedalus_core::libraries::{self, Available};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub const USAGE: &str = "\
  diff        structural diff of a class between two versions:
              daedalus diff OLD NEW [-m Class] [--new-class Name] [--text]
              [--format report|unified|json]; OLD/NEW are .mo files or library
              directories; exits 1 when they differ
  params-from parameter values from a CSV (name,value[,unit]) or SSV file as a
              modifier: daedalus params-from SOURCES -m Model DATA.csv
              [-c component] [--record Name] [--apply [-o FILE]]
  lib         libraries: daedalus lib list | index | install SPEC [--dir D] |
              remove NAME[@VERSION] | resolve SOURCES...; SPEC is Name[@version]
              from the index, a GitHub URL[@ref], or a downloaded .zip
  package     write a model with the files and resources it needs as a zip:
              daedalus package SOURCES -m Model [-o Model.zip] [--include-msl];
              a .zip given as a source is opened (its model is the default -m)
";

/// The directory libraries are installed into (`--dir`, else the default).
fn lib_dir(explicit: Option<&str>) -> PathBuf {
    explicit.map(PathBuf::from).unwrap_or_else(libraries::default_dir)
}

/// Resolve the `uses` annotations of what `lib` holds against the installed libraries,
/// registering the chosen ones, until nothing new is needed. Conflicts are printed as
/// warnings, unresolved libraries as notes.
pub fn auto_uses(lib: &mut Library) {
    let installed = libraries::scan_installed(&libraries::default_dir());
    let mut added: Vec<String> = Vec::new();
    for _round in 0..8 {
        let mut reqs = libraries::requirements(lib);
        // the uses of libraries registered but not parsed (their package.mo)
        for f in &lib.lazy {
            if f.class.contains('.') || f.loaded {
                continue;
            }
            if let Some(t) = f.read() {
                for (n, v) in libraries::package_meta(&t).1.uses {
                    reqs.push(libraries::Requirement {
                        by: f.class.clone(),
                        name: n,
                        version: v,
                    });
                }
            }
        }
        let mut avail = libraries::loaded(lib);
        avail.extend(installed.iter().cloned());
        let res = libraries::resolve(&reqs, &avail);
        let mut new = false;
        for s in &res.selected {
            if s.loaded || added.contains(&s.origin) {
                continue;
            }
            let p = Path::new(&s.origin);
            let ok = if p.is_dir() {
                lib.add_dir_lazy(p).is_ok()
            } else {
                std::fs::read_to_string(p)
                    .map(|t| {
                        lib.set_document(&s.origin, &t);
                    })
                    .is_ok()
            };
            if ok {
                eprintln!(
                    "using {} {} from {}",
                    s.name,
                    s.version.as_deref().unwrap_or(""),
                    s.origin
                );
                added.push(s.origin.clone());
                new = true;
            }
        }
        if !new {
            for c in &res.conflicts {
                eprintln!("warning: version conflict: {c}");
            }
            for m in &res.missing {
                eprintln!("note: {m} (daedalus lib install, or -L DIR)");
            }
            return;
        }
    }
}

/// Open a model package or library zip given as a source: extract it to a temporary
/// directory and return the directories and files to load, and the package's model.
pub fn open_zip(path: &str) -> Result<(Vec<String>, Option<String>)> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {path}"))?;
    let pkg = archive::read_package(&bytes)?;
    // content-addressed, so opening the same zip again reuses the extraction
    let mut h: u64 = 0xcbf29ce484222325;
    for b in &bytes {
        h = (h ^ *b as u64).wrapping_mul(0x100000001b3);
    }
    let dir = std::env::temp_dir().join(format!("daedalus-pkg-{h:016x}"));
    for (name, data) in pkg
        .mo
        .iter()
        .map(|(n, t)| (n, t.as_bytes()))
        .chain(pkg.orders.iter().map(|(n, t)| (n, t.as_bytes())))
        .chain(pkg.resources.iter().map(|(n, d)| (n, d.as_slice())))
    {
        let p = dir.join(name);
        if let Some(par) = p.parent() {
            std::fs::create_dir_all(par)?;
        }
        std::fs::write(&p, data)?;
    }
    let mut out = Vec::new();
    let mut tops: Vec<String> = pkg
        .mo
        .iter()
        .map(|(n, _)| n.split('/').next().unwrap_or("").to_string())
        .collect();
    tops.sort();
    tops.dedup();
    for t in tops {
        out.push(dir.join(t).to_string_lossy().to_string());
    }
    if let Some(m) = &pkg.manifest {
        if !m.uses.is_empty() {
            eprintln!(
                "{path}: uses {}",
                m.uses
                    .iter()
                    .map(|(k, v)| format!("{k} {v}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        for miss in &m.missing {
            eprintln!("note: {path}: resource {miss} was not found when it was packaged");
        }
    }
    Ok((out, pkg.manifest.map(|m| m.model)))
}

/// Load sources (files, directories, zips) and `-L` libraries the way `run` does.
fn load(sources: &[String], libs: &[String], model: Option<&str>) -> Result<(Library, Option<String>)> {
    let mut lib = Library::new();
    let mut model = model.map(String::from);
    let mut all: Vec<String> = libs.to_vec();
    if let Ok(dir) = std::env::var("DAEDALUS_LIB") {
        all.push(dir);
    }
    for s in sources {
        if s.ends_with(".zip") {
            let (paths, m) = open_zip(s)?;
            all.extend(paths);
            if model.is_none() {
                model = m;
            }
        } else {
            all.push(s.clone());
        }
    }
    for s in &all {
        let p = Path::new(s);
        if p.is_dir() {
            lib.add_dir_lazy(p)?;
        } else {
            let text = std::fs::read_to_string(p).map_err(|e| anyhow!("{s}: {e}"))?;
            for e in lib.set_document(s, &text) {
                eprintln!("{e}");
            }
        }
    }
    auto_uses(&mut lib);
    if let Some(m) = &model
        && lib.has_pending()
    {
        lib.load_for(m);
    }
    Ok((lib, model))
}

struct Args {
    pos: Vec<String>,
    opts: Vec<(String, Option<String>)>,
}

impl Args {
    fn parse(args: &[String], flags: &[&str]) -> Result<Args> {
        let mut pos = Vec::new();
        let mut opts = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = &args[i];
            if a.starts_with('-') && a.len() > 1 {
                if flags.contains(&a.as_str()) {
                    opts.push((a.clone(), None));
                } else {
                    let v = args
                        .get(i + 1)
                        .cloned()
                        .ok_or_else(|| anyhow!("missing value for {a}"))?;
                    opts.push((a.clone(), Some(v)));
                    i += 1;
                }
            } else {
                pos.push(a.clone());
            }
            i += 1;
        }
        Ok(Args { pos, opts })
    }
    fn get(&self, names: &[&str]) -> Option<&str> {
        self.opts
            .iter()
            .rev()
            .find(|(k, _)| names.contains(&k.as_str()))
            .and_then(|(_, v)| v.as_deref())
    }
    fn all(&self, names: &[&str]) -> Vec<String> {
        self.opts
            .iter()
            .filter(|(k, _)| names.contains(&k.as_str()))
            .filter_map(|(_, v)| v.clone())
            .collect()
    }
    fn flag(&self, name: &str) -> bool {
        self.opts.iter().any(|(k, _)| k == name)
    }
}

/// `daedalus diff OLD NEW [-m Class]`
pub fn diff(args: &[String]) -> Result<ExitCode> {
    let a = Args::parse(args, &["--text"])?;
    if a.pos.len() != 2 {
        bail!("usage: daedalus diff OLD NEW [-m Class] [--new-class Name] [--text] [--format report|unified|json]");
    }
    let class = a.get(&["-m", "--model"]);
    let new_class = a.get(&["--new-class"]);
    let side = |p: &str| -> Result<Library> {
        let path = Path::new(p);
        let mut lib = Library::new();
        if path.is_dir() {
            lib.add_dir_lazy(path)?;
            let c = class.ok_or_else(|| anyhow!("{p} is a directory: name the class with -m"))?;
            lib.load_for(c);
            if new_class.is_some() {
                lib.load_for(new_class.unwrap());
            }
        } else {
            let text = std::fs::read_to_string(path).with_context(|| format!("reading {p}"))?;
            lib.set_document(p, &text);
        }
        Ok(lib)
    };
    let (la, lb) = (side(&a.pos[0])?, side(&a.pos[1])?);
    let d = daedalus_core::diff::diff_libraries(&la, &lb, class, new_class.or(class))?;
    match a.get(&["--format"]).unwrap_or("report") {
        "json" => println!("{}", serde_json::to_string_pretty(&d)?),
        "unified" => print!(
            "{}",
            daedalus_core::diff::unified(&d.text, &a.pos[0], &a.pos[1], 3)
        ),
        "report" | "text" => {
            print!("{}", d.report());
            if a.flag("--text") {
                let u = daedalus_core::diff::unified(&d.text, &a.pos[0], &a.pos[1], 3);
                if !u.is_empty() {
                    println!();
                    print!("{u}");
                }
            }
        }
        f => bail!("unknown format '{f}' (report | unified | json)"),
    }
    Ok(if d.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

/// `daedalus params-from SOURCES -m Model DATA [-c comp] [--record Name] [--apply]`
pub fn params_from(args: &[String]) -> Result<ExitCode> {
    let a = Args::parse(args, &["--apply", "--no-check"])?;
    let is_data = |p: &str| {
        let l = p.to_ascii_lowercase();
        [".csv", ".ssv", ".tsv", ".txt", ".xml"].iter().any(|e| l.ends_with(e))
    };
    let data: Vec<&String> = a.pos.iter().filter(|p| is_data(p)).collect();
    let sources: Vec<String> = a.pos.iter().filter(|p| !is_data(p)).cloned().collect();
    let [data] = data.as_slice() else {
        bail!("give one parameter file (.csv or .ssv)");
    };
    let text = std::fs::read_to_string(data).with_context(|| format!("reading {data}"))?;
    let mut params = daedalus_core::params::parse(&text, data)?;
    if params.is_empty() {
        bail!("{data}: no parameter values");
    }
    let model = a.get(&["-m", "--model"]);
    let comp = a.get(&["-c", "--component"]);
    let libs = a.all(&["-L", "--lib"]);
    let mut lib_model: Option<(Library, String)> = None;
    if let Some(m) = model {
        let (lib, _) = load(&sources, &libs, Some(m))?;
        if !a.flag("--no-check") {
            let checked = daedalus_core::params::check(&lib, m, comp, &params)?;
            for msg in &checked.messages {
                eprintln!("{msg}");
            }
            params = checked.params;
        }
        lib_model = Some((lib, m.to_string()));
    }
    if let Some(r) = a.get(&["--record"]) {
        print!("{}", daedalus_core::params::record(r, &params));
        return Ok(ExitCode::SUCCESS);
    }
    let target = comp.or(model.map(|m| m.rsplit('.').next().unwrap_or(m))).unwrap_or("data");
    if a.flag("--apply") {
        let (lib, m) = lib_model.ok_or_else(|| anyhow!("--apply needs the model (-m)"))?;
        let c = comp.ok_or_else(|| anyhow!("--apply needs the component (-c)"))?;
        let tree = daedalus_core::params::ModTree::from_params(&params);
        let (uri, new_text) = daedalus_core::edit::apply_modifier(&lib, &m, c, &tree)?;
        let out = a.get(&["-o", "--output"]).unwrap_or(&uri);
        std::fs::write(out, new_text).with_context(|| format!("writing {out}"))?;
        eprintln!("modifier of {c} written to {out}");
        return Ok(ExitCode::SUCCESS);
    }
    println!("{}", daedalus_core::params::modifier(target, &params));
    Ok(ExitCode::SUCCESS)
}

fn download(url: &str, to: &Path) -> Result<()> {
    let st = std::process::Command::new("curl")
        .args(["-fsSL", "--retry", "2", "-o"])
        .arg(to)
        .arg(url)
        .status()
        .context("running curl (needed to download libraries)")?;
    if !st.success() {
        bail!("download failed: {url}");
    }
    Ok(())
}

/// `daedalus lib ...`
pub fn lib(args: &[String]) -> Result<ExitCode> {
    let a = Args::parse(args, &[])?;
    let dir = lib_dir(a.get(&["--dir"]));
    let index = libraries::builtin_index();
    let sub = a.pos.first().map(String::as_str).unwrap_or("list");
    match sub {
        "index" | "search" | "available" => {
            let q = a.pos.get(1).map(|s| s.to_ascii_lowercase());
            for l in &index.libraries {
                if let Some(q) = &q
                    && !l.name.to_ascii_lowercase().contains(q)
                    && !l.description.to_ascii_lowercase().contains(q)
                {
                    continue;
                }
                let vs: Vec<&str> = l.versions.iter().map(|v| v.version.as_str()).collect();
                println!("{:<24} {:<20} {}", l.name, vs.join(", "), l.description);
                println!("{:<24} https://github.com/{}", "", l.repo);
            }
        }
        "list" => {
            let inst = libraries::scan_installed(&dir);
            println!("installed in {}:", dir.display());
            if inst.is_empty() {
                println!("  (none)");
            }
            for l in inst {
                println!(
                    "  {:<24} {:<10} {}",
                    l.name,
                    l.version.as_deref().unwrap_or("?"),
                    l.origin
                );
            }
        }
        "install" | "add" => {
            let spec = a
                .pos
                .get(1)
                .ok_or_else(|| anyhow!("usage: daedalus lib install NAME[@VERSION] | URL[@REF] | FILE.zip"))?;
            let (bytes, packages, version) = if spec.ends_with(".zip") && Path::new(spec).is_file() {
                (
                    std::fs::read(spec)?,
                    a.all(&["--package"]),
                    a.get(&["--version"]).map(String::from),
                )
            } else {
                let src = libraries::parse_spec(spec, &index)?;
                let tmp = std::env::temp_dir().join(format!(
                    "daedalus-dl-{}-{}.zip",
                    std::process::id(),
                    src.git_ref.replace('/', "_")
                ));
                let mut last = None;
                for url in src.archive_urls() {
                    eprintln!("downloading {url}");
                    match download(&url, &tmp) {
                        Ok(()) => {
                            last = None;
                            break;
                        }
                        Err(e) => last = Some(e),
                    }
                }
                if let Some(e) = last {
                    return Err(e.context(format!(
                        "could not download {}@{}; download the archive yourself and run \
                         `daedalus lib install FILE.zip`",
                        src.repo, src.git_ref
                    )));
                }
                let b = std::fs::read(&tmp)?;
                let _ = std::fs::remove_file(&tmp);
                (b, src.packages.clone(), src.version.clone())
            };
            let got = archive::extract_libraries(&bytes, &dir, &packages, version.as_deref())?;
            for l in &got {
                println!(
                    "installed {} {} in {}",
                    l.name,
                    l.version.as_deref().unwrap_or(""),
                    l.origin
                );
            }
            // what the new libraries use, against what is installed
            let mut reqs = Vec::new();
            for l in &got {
                let main = if l.origin.ends_with(".mo") {
                    PathBuf::from(&l.origin)
                } else {
                    Path::new(&l.origin).join("package.mo")
                };
                if let Ok(t) = std::fs::read_to_string(main) {
                    for (n, v) in libraries::package_meta(&t).1.uses {
                        reqs.push(libraries::Requirement {
                            by: l.name.clone(),
                            name: n,
                            version: v,
                        });
                    }
                }
            }
            let res = libraries::resolve(&reqs, &libraries::scan_installed(&dir));
            for s in &res.selected {
                println!("  uses {} {} (installed)", s.name, s.version.as_deref().unwrap_or(""));
            }
            for m in &res.missing {
                println!("  note: {m}");
            }
            for c in &res.conflicts {
                println!("  warning: {c}");
            }
        }
        "remove" | "uninstall" => {
            let spec = a.pos.get(1).ok_or_else(|| anyhow!("usage: daedalus lib remove NAME[@VERSION]"))?;
            let (name, ver) = match spec.split_once('@') {
                Some((n, v)) => (n, Some(v)),
                None => (spec.as_str(), None),
            };
            let mut n = 0;
            for l in libraries::scan_installed(&dir) {
                if l.name == name && (ver.is_none() || l.version.as_deref() == ver) {
                    let p = Path::new(&l.origin);
                    if p.is_dir() {
                        std::fs::remove_dir_all(p)?;
                    } else {
                        std::fs::remove_file(p)?;
                    }
                    println!("removed {}", l.origin);
                    n += 1;
                }
            }
            if n == 0 {
                bail!("{spec} is not installed in {}", dir.display());
            }
        }
        "resolve" | "check" => {
            let sources: Vec<String> = a.pos[1..].to_vec();
            let mut lib = Library::new();
            for s in &sources {
                let p = Path::new(s);
                if p.is_dir() {
                    lib.add_dir_lazy(p)?;
                } else {
                    lib.set_document(s, &std::fs::read_to_string(p)?);
                }
            }
            let mut reqs = libraries::requirements(&lib);
            for f in &lib.lazy {
                if !f.class.contains('.')
                    && let Some(t) = f.read()
                {
                    for (n, v) in libraries::package_meta(&t).1.uses {
                        reqs.push(libraries::Requirement {
                            by: f.class.clone(),
                            name: n,
                            version: v,
                        });
                    }
                }
            }
            let mut avail: Vec<Available> = libraries::loaded(&lib);
            avail.extend(libraries::scan_installed(&dir));
            let res = libraries::resolve(&reqs, &avail);
            for r in &reqs {
                println!("{} uses {} {}", r.by, r.name, r.version);
            }
            for s in &res.selected {
                println!(
                    "  -> {} {} ({})",
                    s.name,
                    s.version.as_deref().unwrap_or("?"),
                    if s.loaded { "given" } else { &s.origin }
                );
            }
            for n in &res.notes {
                println!("note: {n}");
            }
            for m in &res.missing {
                println!("missing: {m}");
            }
            for c in &res.conflicts {
                println!("conflict: {c}");
            }
            if !res.conflicts.is_empty() || !res.missing.is_empty() {
                return Ok(ExitCode::from(1));
            }
        }
        s => bail!("unknown lib command '{s}' (list | index | install | remove | resolve)"),
    }
    Ok(ExitCode::SUCCESS)
}

/// `daedalus package SOURCES -m Model [-o Model.zip] [--include-msl]`
pub fn package(args: &[String]) -> Result<ExitCode> {
    let a = Args::parse(args, &["--include-msl"])?;
    let model = a
        .get(&["-m", "--model"])
        .ok_or_else(|| anyhow!("no model given (-m)"))?;
    let libs = a.all(&["-L", "--lib"]);
    let (lib, _) = load(&a.pos, &libs, Some(model))?;
    let exclude: Vec<&str> = if a.flag("--include-msl") {
        Vec::new()
    } else {
        archive::STANDARD_LIBRARIES.to_vec()
    };
    let list = |key: &str| -> Vec<String> {
        let p = Path::new(key);
        if p.is_file() {
            return vec![key.to_string()];
        }
        let mut out = Vec::new();
        let mut stack = vec![p.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = std::fs::read_dir(&d) else { continue };
            for e in rd.flatten() {
                let q = e.path();
                if q.is_dir() {
                    stack.push(q);
                } else {
                    out.push(q.to_string_lossy().to_string());
                }
            }
        }
        out.sort();
        out
    };
    let plan = archive::plan_package(&lib, model, &exclude, &list)?;
    let docs: std::collections::HashMap<&str, &str> =
        lib.docs.iter().map(|d| (d.uri.as_str(), &*d.text)).collect();
    let text = |uri: &str| -> Option<String> {
        docs.get(uri)
            .map(|t| t.to_string())
            .or_else(|| lib.lazy_text(uri).map(|t| t.to_string()))
            .or_else(|| std::fs::read_to_string(uri).ok())
    };
    let read = |key: &str| std::fs::read(key).ok();
    let zip = archive::write_package(&plan, &text, &read)?;
    let out = a
        .get(&["-o", "--output"])
        .map(String::from)
        .unwrap_or_else(|| format!("{}.zip", model.rsplit('.').next().unwrap_or(model)));
    std::fs::write(&out, &zip).with_context(|| format!("writing {out}"))?;
    println!(
        "{out}: {} Modelica files, {} resources ({} kB)",
        plan.files.len(),
        plan.resources.len(),
        zip.len() / 1000
    );
    for (k, v) in &plan.uses {
        println!("  uses {k} {v} (not included)");
    }
    for m in &plan.missing {
        println!("  warning: resource not found: {m}");
    }
    Ok(ExitCode::SUCCESS)
}
