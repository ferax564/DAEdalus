//! Zip archives: model packages and library archives.
//!
//! A **model package** is a zip holding a model, the Modelica files it needs (except
//! the libraries it only `uses`, such as the MSL), and the resources those files
//! reference with `modelica://` URIs (tables, meshes, images, ...), laid out as
//! library directories, plus `daedalus-package.json`:
//!
//! ```json
//! {"format": 1, "model": "MyLib.Examples.Test", "files": ["MyLib/package.mo", ...],
//!  "resources": ["MyLib/Resources/Data/table.txt"], "uses": {"Modelica": "4.1.0"},
//!  "missing": []}
//! ```
//!
//! A **library archive** is a source archive such as GitHub's `repo-tag.zip`; the
//! library packages in it are found by their `package.mo` and installed as
//! `Name version/` directories ([`extract_libraries`]).

use crate::frontend::Library;
use crate::frontend::lazy::class_for_path;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read, Write};
use std::sync::Arc;

/// Name of the manifest of a model package.
pub const MANIFEST: &str = "daedalus-package.json";

/// Libraries that a package refers to rather than carries.
pub const STANDARD_LIBRARIES: &[&str] = &[
    "Modelica",
    "ModelicaServices",
    "Complex",
    "ModelicaReference",
];

/// A path inside an archive, made safe: forward slashes, no `..`, no root.
fn clean(path: &str) -> Option<String> {
    let mut parts = Vec::new();
    let norm = path.replace('\\', "/");
    for p in norm.split('/') {
        match p {
            "" | "." => {}
            ".." => return None,
            p if p.contains(':') => return None,
            p => parts.push(p),
        }
    }
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// The files of a zip archive (directories are skipped, unsafe paths dropped).
pub fn read_zip(bytes: &[u8]) -> Result<Vec<(String, Vec<u8>)>> {
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).context("not a zip archive")?;
    let mut out = Vec::with_capacity(z.len());
    for i in 0..z.len() {
        let mut f = z.by_index(i)?;
        if f.is_dir() {
            continue;
        }
        let Some(name) = clean(f.name()) else {
            continue;
        };
        let mut data = Vec::with_capacity(f.size() as usize);
        f.read_to_end(&mut data)?;
        out.push((name, data));
    }
    Ok(out)
}

/// A zip archive of `files` (deflated).
pub fn write_zip(files: &[(String, Vec<u8>)]) -> Result<Vec<u8>> {
    let mut z = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    let opts = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    for (name, data) in files {
        let name = clean(name).ok_or_else(|| anyhow!("unsafe archive path {name}"))?;
        z.start_file(name, opts)?;
        z.write_all(data)?;
    }
    Ok(z.finish()?.into_inner())
}

/// `modelica://` URIs in a text, without duplicates, in order (both class links like
/// `modelica://Modelica.Blocks.Continuous.PID` and resources like
/// `modelica://Modelica/Resources/Images/x.png`).
pub fn modelica_uris(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let lower = text.to_ascii_lowercase();
    let mut from = 0;
    while let Some(k) = lower[from..].find("modelica://") {
        let s = from + k;
        let e = text[s..]
            .find(|c: char| {
                c == '"'
                    || c == '\''
                    || c == '\\'
                    || c == '<'
                    || c == '>'
                    || c == ')'
                    || c.is_whitespace()
                    || c == '#'
                    || c == '?'
            })
            .map_or(text.len(), |i| s + i);
        let uri = text[s..e].to_string();
        if uri.len() > "modelica://".len() && !out.contains(&uri) {
            out.push(uri);
        }
        from = e.max(s + 1);
    }
    out
}

/// Split a `modelica://` URI into its class and path: `modelica://Modelica/Resources/a.png`
/// is `("Modelica", "Resources/a.png")`, `modelica://Modelica.Blocks` is
/// `("Modelica.Blocks", "")`. Percent-escapes in the path are decoded.
pub fn split_uri(uri: &str) -> Option<(String, String)> {
    if uri.len() < 11 || !uri[..11].eq_ignore_ascii_case("modelica://") {
        return None;
    }
    let rest = &uri[11..];
    let (class, path) = rest.split_once('/').unwrap_or((rest, ""));
    let b = path.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len() + 1
            && i + 3 <= b.len()
            && let Ok(v) = u8::from_str_radix(&path[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    Some((
        class.trim_end_matches('.').to_string(),
        String::from_utf8_lossy(&out).into_owned(),
    ))
}

/// The manifest of a model package.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub format: u32,
    pub model: String,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub resources: Vec<String>,
    /// Libraries used but not included, with their version when known.
    #[serde(default)]
    pub uses: BTreeMap<String, String>,
    /// Resources referenced but not found.
    #[serde(default)]
    pub missing: Vec<String>,
    #[serde(default)]
    pub created_by: String,
}

/// What goes into a model package: document uris and resource keys, with their paths
/// in the archive.
#[derive(Debug, Clone, Default, Serialize)]
pub struct PackagePlan {
    pub model: String,
    /// (document uri in the library, path in the archive)
    pub files: Vec<(String, String)>,
    /// (resource key as `resource` resolves it, path in the archive)
    pub resources: Vec<(String, String)>,
    pub uses: BTreeMap<String, String>,
    pub missing: Vec<String>,
}

/// The top-level class a document defines (`within` + class name).
fn doc_class(text: &str, ast: Option<&rumoca_ir_ast::StoredDefinition>) -> Option<String> {
    let own;
    let ast = match ast {
        Some(a) => a,
        None => {
            own = crate::frontend::parse_document("x.mo", text).ast;
            &own
        }
    };
    let name = ast.classes.keys().next()?.clone();
    let within = ast
        .within
        .as_ref()
        .map(|w| {
            w.name
                .iter()
                .map(|t| t.text.to_string())
                .collect::<Vec<_>>()
                .join(".")
        })
        .unwrap_or_default();
    Some(if within.is_empty() {
        name
    } else {
        format!("{within}.{name}")
    })
}

fn dir_of(uri: &str) -> &str {
    uri.rfind('/').map_or("", |i| &uri[..i])
}

/// Plan the package of `model`: the documents of `lib` it needs (found like the
/// on-demand loader finds them), except those of the `exclude` libraries, and the
/// resources they reference. `resources(key)` lists the resource files at a key (one
/// file, or every file below a directory); keys are the directory of a library file
/// joined with the resource path.
pub fn plan_package(
    lib: &Library,
    model: &str,
    exclude: &[&str],
    resources: &dyn Fn(&str) -> Vec<String>,
) -> Result<PackagePlan> {
    // Every document, parsed or registered, as a registered file of a scratch library,
    // so the closure covers the user's own documents too.
    let mut scratch = Library::new();
    let mut class_of: BTreeMap<String, String> = BTreeMap::new();
    let mut texts: BTreeMap<String, Arc<str>> = BTreeMap::new();
    for d in &lib.docs {
        if let Some(c) = doc_class(&d.text, Some(&d.ast)) {
            scratch.add_lazy_shared(&d.uri, &c, d.text.clone());
            class_of.insert(d.uri.clone(), c);
            texts.insert(d.uri.clone(), d.text.clone());
        }
    }
    for f in &lib.lazy {
        let uri = f.path.to_string_lossy().to_string();
        if texts.contains_key(&uri) {
            continue;
        }
        if let Some(t) = f.read() {
            scratch.add_lazy_shared(&uri, &f.class, t.clone());
            class_of.insert(uri.clone(), f.class.clone());
            texts.insert(uri, t);
        }
    }
    let used = scratch.closure_uris(model);
    if used.is_empty() {
        bail!("model {model} not found");
    }
    // directory of each package (from its package.mo)
    let mut pkg_dir: BTreeMap<String, String> = BTreeMap::new();
    for (uri, c) in &class_of {
        if uri.ends_with("package.mo") {
            pkg_dir.insert(c.clone(), dir_of(uri).to_string());
        }
    }
    let root_of = |c: &str| c.split('.').next().unwrap_or(c).to_string();
    // A file's path in the archive: below the directory that holds its library.
    let rel = |uri: &str, class: &str| -> String {
        let root = root_of(class);
        if let Some(d) = pkg_dir.get(&root) {
            let parent = dir_of(d);
            let base = if parent.is_empty() {
                uri.to_string()
            } else {
                uri.strip_prefix(&format!("{parent}/"))
                    .unwrap_or(uri)
                    .to_string()
            };
            // a versioned directory ("Buildings 12.1.0") is stored under its name
            let mut parts: Vec<&str> = base.split('/').collect();
            if let Some(first) = parts.first_mut()
                && first.split(' ').next() == Some(root.as_str())
            {
                *first = root.as_str();
                return parts.join("/");
            }
            base
        } else {
            uri.rsplit('/').next().unwrap_or(uri).to_string()
        }
    };
    let mut plan = PackagePlan {
        model: model.to_string(),
        ..Default::default()
    };
    let mut reqs: Vec<crate::libraries::Requirement> = Vec::new();
    let mut seen_files = BTreeSet::new();
    for uri in &used {
        let class = &class_of[uri];
        let root = root_of(class);
        if exclude.contains(&root.as_str()) {
            let ver = pkg_dir
                .get(&root)
                .and_then(|d| texts.get(&format!("{d}/package.mo")))
                .and_then(|t| crate::libraries::package_meta(t).1.version)
                .unwrap_or_default();
            plan.uses.entry(root).or_insert(ver);
            continue;
        }
        let r = rel(uri, class);
        if seen_files.insert(r.clone()) {
            plan.files.push((uri.clone(), r));
        }
        if !class.contains('.') {
            let (_, meta) = crate::libraries::package_meta(&texts[uri]);
            for (n, v) in meta.uses {
                reqs.push(crate::libraries::Requirement {
                    by: class.clone(),
                    name: n,
                    version: v,
                });
            }
        }
    }
    let included: BTreeSet<String> = plan
        .files
        .iter()
        .map(|(u, _)| root_of(&class_of[u]))
        .collect();
    for r in reqs {
        if !included.contains(&r.name) {
            let e = plan.uses.entry(r.name).or_default();
            if e.is_empty() {
                *e = r.version;
            }
        }
    }
    // resources referenced by the included files
    let mut seen = BTreeSet::new();
    for (uri, _) in plan.files.clone() {
        for u in modelica_uris(&texts[&uri]) {
            let Some((class, path)) = split_uri(&u) else {
                continue;
            };
            if path.is_empty() || !seen.insert(u.clone()) {
                continue;
            }
            let root = root_of(&class);
            if exclude.contains(&root.as_str()) {
                continue;
            }
            let Some(mut dir) = pkg_dir.get(&root).cloned() else {
                plan.missing.push(u.clone());
                continue;
            };
            // descend through the packages stored as directories
            let segs: Vec<&str> = class.split('.').collect();
            for k in 1..segs.len() {
                match pkg_dir.get(&segs[..=k].join(".")) {
                    Some(d) => dir = d.clone(),
                    None => break,
                }
            }
            let key = format!("{dir}/{}", path.trim_end_matches('/'));
            let found = resources(&key);
            if found.is_empty() {
                plan.missing.push(u.clone());
                continue;
            }
            let parent = pkg_dir
                .get(&root)
                .map(|d| dir_of(d).to_string())
                .unwrap_or_default();
            for f in found {
                let base = if parent.is_empty() {
                    f.clone()
                } else {
                    f.strip_prefix(&format!("{parent}/"))
                        .unwrap_or(&f)
                        .to_string()
                };
                let mut parts: Vec<&str> = base.split('/').collect();
                if let Some(first) = parts.first_mut()
                    && first.split(' ').next() == Some(root.as_str())
                {
                    *first = root.as_str();
                }
                let r = parts.join("/");
                if !plan.resources.iter().any(|(_, x)| *x == r) {
                    plan.resources.push((f, r));
                }
            }
        }
    }
    Ok(plan)
}

/// Write a model package: the planned files (texts from `text`) and resources (bytes
/// from `read`), with the manifest.
pub fn write_package(
    plan: &PackagePlan,
    text: &dyn Fn(&str) -> Option<String>,
    read: &dyn Fn(&str) -> Option<Vec<u8>>,
) -> Result<Vec<u8>> {
    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    let mut missing = plan.missing.clone();
    for (uri, rel) in &plan.files {
        let t = text(uri).ok_or_else(|| anyhow!("cannot read {uri}"))?;
        files.push((rel.clone(), t.into_bytes()));
    }
    let mut res = Vec::new();
    for (key, rel) in &plan.resources {
        match read(key) {
            Some(b) => {
                files.push((rel.clone(), b));
                res.push(rel.clone());
            }
            None => missing.push(key.clone()),
        }
    }
    let manifest = Manifest {
        format: 1,
        model: plan.model.clone(),
        files: plan.files.iter().map(|(_, r)| r.clone()).collect(),
        resources: res,
        uses: plan.uses.clone(),
        missing,
        created_by: format!("DAEdalus {}", env!("CARGO_PKG_VERSION")),
    };
    files.insert(
        0,
        (MANIFEST.to_string(), serde_json::to_vec_pretty(&manifest)?),
    );
    write_zip(&files)
}

/// A model package (or any zip of Modelica files) read back.
#[derive(Debug, Clone, Default)]
pub struct Package {
    pub manifest: Option<Manifest>,
    /// Modelica files: (path, text)
    pub mo: Vec<(String, String)>,
    /// `package.order` files: (path, text)
    pub orders: Vec<(String, String)>,
    /// Everything else: (path, bytes)
    pub resources: Vec<(String, Vec<u8>)>,
}

/// Read a model package. A zip without a manifest is read the same way (a library
/// archive); a single top folder holding everything (`repo-tag/`) is stripped.
pub fn read_package(bytes: &[u8]) -> Result<Package> {
    let mut files = read_zip(bytes)?;
    let mut pkg = Package::default();
    if let Some(i) = files.iter().position(|(n, _)| n == MANIFEST) {
        let (_, m) = files.remove(i);
        pkg.manifest = Some(serde_json::from_slice(&m).context("daedalus-package.json")?);
    } else {
        // strip a common top folder that is not itself a package
        let top: BTreeSet<&str> = files
            .iter()
            .map(|(n, _)| n.split('/').next().unwrap_or(""))
            .collect();
        if top.len() == 1 && files.iter().all(|(n, _)| n.contains('/')) {
            let t = top.into_iter().next().unwrap().to_string();
            let is_pkg = files.iter().any(|(n, _)| *n == format!("{t}/package.mo"));
            if !is_pkg {
                for (n, _) in &mut files {
                    *n = n[t.len() + 1..].to_string();
                }
            }
        }
    }
    for (name, data) in files {
        if name.ends_with(".mo") {
            pkg.mo
                .push((name, String::from_utf8_lossy(&data).into_owned()));
        } else if name.ends_with("package.order") {
            pkg.orders
                .push((name, String::from_utf8_lossy(&data).into_owned()));
        } else {
            pkg.resources.push((name, data));
        }
    }
    Ok(pkg)
}

/// The library packages of an archive: `(name, prefix)` for each outermost
/// `.../Name/package.mo` (prefix `.../Name/`), and `(name, path)` of single-file
/// libraries `Name.mo` lying next to those directories (MSL's `Complex.mo`). A
/// `package.mo` at the archive root gives an empty name and prefix.
pub fn find_libraries(paths: &[&str]) -> Vec<(String, String)> {
    let dirs: BTreeSet<&str> = paths
        .iter()
        .filter_map(|p| {
            p.strip_suffix("/package.mo")
                .or_else(|| (*p == "package.mo").then_some(""))
        })
        .collect();
    let inside = |d: &str, a: &str| a.is_empty() || d.starts_with(&format!("{a}/"));
    let outer: Vec<&str> = dirs
        .iter()
        .copied()
        .filter(|d| !dirs.iter().any(|a| a != d && inside(d, a)))
        .collect();
    let mut found: Vec<(String, String)> = Vec::new();
    for d in &outer {
        let name = d.rsplit('/').next().unwrap_or(d);
        let name = name.split(' ').next().unwrap_or(name).to_string();
        let prefix = if d.is_empty() {
            String::new()
        } else {
            format!("{d}/")
        };
        found.push((name, prefix));
    }
    let parents: BTreeSet<&str> = outer.iter().map(|d| dir_of(d)).collect();
    for p in paths {
        if !p.ends_with(".mo") || p.ends_with("package.mo") {
            continue;
        }
        if outer.iter().any(|d| inside(p, d)) {
            continue;
        }
        if !parents.is_empty() && !parents.contains(dir_of(p)) {
            continue;
        }
        let file = p.rsplit('/').next().unwrap_or(p);
        let name = file
            .trim_end_matches(".mo")
            .split(' ')
            .next()
            .unwrap_or("")
            .to_string();
        found.push((name, p.to_string()));
    }
    found.sort();
    found
}

/// Install the library packages of a library archive into `dest` as `Name version/`
/// directories (or `Name version.mo` files). `packages` restricts which are taken
/// (empty: all found); `version` names the version when the package does not (its
/// `version` annotation wins). Returns what was installed.
#[cfg(not(target_arch = "wasm32"))]
pub fn extract_libraries(
    bytes: &[u8],
    dest: &std::path::Path,
    packages: &[String],
    version: Option<&str>,
) -> Result<Vec<crate::libraries::Available>> {
    let files = read_zip(bytes)?;
    let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
    let libs: Vec<(String, String)> = find_libraries(&names)
        .into_iter()
        .filter(|(n, _)| packages.is_empty() || packages.iter().any(|p| p == n))
        .collect();
    if libs.is_empty() {
        bail!(
            "no Modelica library (a directory with package.mo{}) in the archive",
            if packages.is_empty() {
                String::new()
            } else {
                format!(" named {}", packages.join(" or "))
            }
        );
    }
    std::fs::create_dir_all(dest)?;
    let mut out = Vec::new();
    for (name, prefix) in libs {
        let single = prefix.ends_with(".mo");
        let main = if single {
            prefix.clone()
        } else {
            format!("{prefix}package.mo")
        };
        let text = files
            .iter()
            .find(|(n, _)| *n == main)
            .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
            .unwrap_or_default();
        let (top, meta) = crate::libraries::package_meta(&text);
        let name = if name.is_empty() {
            top.ok_or_else(|| anyhow!("{main}: no class in the top-level package.mo"))?
        } else {
            name
        };
        let ver = meta
            .version
            .clone()
            .or_else(|| version.map(String::from))
            .filter(|v| !v.is_empty());
        let dir_name = match &ver {
            Some(v) => format!("{name} {v}"),
            None => name.clone(),
        };
        let target = if single {
            dest.join(format!("{dir_name}.mo"))
        } else {
            dest.join(&dir_name)
        };
        if target.exists() {
            if target.is_dir() {
                std::fs::remove_dir_all(&target)?;
            } else {
                std::fs::remove_file(&target)?;
            }
        }
        if single {
            std::fs::write(&target, text.as_bytes())?;
        } else {
            for (n, d) in &files {
                if let Some(rel) = n.strip_prefix(prefix.as_str()) {
                    let p = target.join(rel);
                    if let Some(par) = p.parent() {
                        std::fs::create_dir_all(par)?;
                    }
                    std::fs::write(&p, d).with_context(|| format!("writing {}", p.display()))?;
                }
            }
        }
        out.push(crate::libraries::Available {
            name,
            version: ver,
            origin: target.to_string_lossy().to_string(),
            loaded: false,
        });
    }
    Ok(out)
}

/// Register the Modelica files of a package in `lib` under `prefix`: files of
/// package directories as registered (on-demand parsed) library files, loose files
/// parsed directly. Libraries already in `lib` are skipped. Returns
/// `(registered files, skipped top-level packages)`.
pub fn register_package(lib: &mut Library, pkg: &Package, prefix: &str) -> (usize, Vec<String>) {
    let present: BTreeSet<String> = crate::frontend::inspect::class_tree(lib)
        .into_iter()
        .map(|e| e.name)
        .collect();
    let mut skipped = BTreeSet::new();
    let mut n = 0;
    for (path, text) in &pkg.mo {
        let root = path
            .split('/')
            .next()
            .unwrap_or("")
            .trim_end_matches(".mo")
            .to_string();
        if path.contains('/') {
            if present.contains(&root) {
                skipped.insert(root);
                continue;
            }
            lib.add_lazy_text(&format!("{prefix}{path}"), &class_for_path(path), text);
        } else {
            let class = doc_class(text, None).unwrap_or(root.clone());
            let top = class.split('.').next().unwrap_or("").to_string();
            if present.contains(&top) && !class.contains('.') {
                skipped.insert(top);
                continue;
            }
            lib.set_document(&format!("{prefix}{path}"), text);
        }
        n += 1;
    }
    for (path, text) in &pkg.orders {
        if let Some(dir) = path.strip_suffix("package.order") {
            lib.add_package_order(&class_for_path(&format!("{dir}package.mo")), text);
        }
    }
    (n, skipped.into_iter().collect())
}
