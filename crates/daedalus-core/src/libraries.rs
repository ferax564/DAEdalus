//! Library management: the index of known open libraries, install specifications
//! (`Buildings@12.1.0`, a GitHub URL), installed library directories, and resolution
//! of `uses(...)` annotations to installed versions with conflict detection.
//!
//! Downloading is left to the caller (the CLI runs `curl`, the workbench `fetch`); this
//! module only names the archive URLs. Unpacking is [`crate::archive::extract_libraries`].

use crate::frontend::inspect::{ClassMeta, class_meta};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;

/// The bundled index of open libraries (`library/packages.json`).
pub const INDEX_JSON: &str = include_str!("../../../library/packages.json");

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Index {
    pub libraries: Vec<IndexEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexEntry {
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// GitHub `owner/repo`.
    pub repo: String,
    #[serde(default)]
    pub license: String,
    /// Top-level packages in the archive, when not just `name` (the MSL also holds
    /// ModelicaServices and Complex).
    #[serde(default)]
    pub packages: Vec<String>,
    pub versions: Vec<IndexVersion>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexVersion {
    pub version: String,
    /// Git tag or branch of the version's source archive.
    #[serde(rename = "ref")]
    pub git_ref: String,
    /// Libraries this version uses, by name and version.
    #[serde(default)]
    pub uses: BTreeMap<String, String>,
}

impl IndexEntry {
    pub fn packages(&self) -> Vec<String> {
        if self.packages.is_empty() {
            vec![self.name.clone()]
        } else {
            self.packages.clone()
        }
    }
}

/// The bundled index.
pub fn builtin_index() -> Index {
    serde_json::from_str(INDEX_JSON).expect("library/packages.json is valid")
}

impl Index {
    pub fn find(&self, name: &str) -> Option<&IndexEntry> {
        self.libraries
            .iter()
            .find(|l| l.name.eq_ignore_ascii_case(name) || l.packages.iter().any(|p| p == name))
    }
}

/// Compare dotted versions numerically (`4.10.0 > 4.9.1`), then textually.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let seg = |s: &str| -> Vec<(u64, String)> {
        s.trim_start_matches('v')
            .split(['.', '-', '+', ' '])
            .map(|p| {
                let n: String = p.chars().take_while(|c| c.is_ascii_digit()).collect();
                (n.parse().unwrap_or(0), p[n.len()..].to_string())
            })
            .collect()
    };
    let (x, y) = (seg(a), seg(b));
    for i in 0..x.len().max(y.len()) {
        let p = x.get(i).cloned().unwrap_or((0, String::new()));
        let q = y.get(i).cloned().unwrap_or((0, String::new()));
        // a pre-release suffix (`-rc.1`) sorts before the plain version
        let o =
            p.0.cmp(&q.0)
                .then_with(|| match (p.1.is_empty(), q.1.is_empty()) {
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    _ => p.1.cmp(&q.1),
                });
        if o != Ordering::Equal {
            return o;
        }
    }
    Ordering::Equal
}

/// The major version (first number) of a version string.
pub fn major(v: &str) -> Option<u64> {
    v.trim_start_matches('v')
        .split('.')
        .next()?
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// What `daedalus lib install` was asked for.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Source {
    /// Library name, when known (from the index, or the spec).
    pub name: Option<String>,
    /// Version to record for the installed copy (from the index, or the git ref).
    pub version: Option<String>,
    /// GitHub `owner/repo`.
    pub repo: String,
    /// Tag or branch.
    pub git_ref: String,
    /// Top-level packages to take from the archive (empty: every package found).
    pub packages: Vec<String>,
}

/// Parse an install specification against `index`:
/// - `Buildings` or `Buildings@12.1.0` (a library of the index; the latest version
///   when none is given);
/// - `https://github.com/owner/repo@ref`, `https://github.com/owner/repo/tree/ref`,
///   `https://github.com/owner/repo/archive/refs/tags/ref.zip`, or `owner/repo@ref`.
pub fn parse_spec(spec: &str, index: &Index) -> Result<Source> {
    let spec = spec.trim();
    let (base, at) = match spec.rsplit_once('@') {
        // `git@github.com:...` is not a version separator
        Some((b, v)) if !v.contains('/') && !v.contains(':') => (b, Some(v.to_string())),
        _ => (spec, None),
    };
    let url = base
        .trim_start_matches("git+")
        .trim_start_matches("https://")
        .trim_start_matches("http://")
        .trim_start_matches("www.");
    let gh = url
        .strip_prefix("github.com/")
        .or_else(|| url.strip_prefix("codeload.github.com/"));
    let looks_repo =
        gh.is_some() || (url.split('/').count() == 2 && !url.contains('.') && url.contains('/'));
    if looks_repo {
        let path = gh
            .unwrap_or(url)
            .trim_end_matches('/')
            .trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        if parts.len() < 2 {
            bail!("'{spec}': expected github.com/OWNER/REPO");
        }
        let repo = format!("{}/{}", parts[0], parts[1]);
        // ref from the URL path: /tree/REF, /archive/refs/tags/REF.zip, /zip/refs/heads/REF
        let rest = &parts[2..];
        let mut git_ref = at.clone();
        if git_ref.is_none() && !rest.is_empty() {
            let r = match rest {
                ["tree", r @ ..] | ["releases", "tag", r @ ..] => r.join("/"),
                ["archive", "refs", _, r @ ..] | ["zip", "refs", _, r @ ..] => r.join("/"),
                ["archive", r @ ..] | ["zip", r @ ..] => r.join("/"),
                _ => String::new(),
            };
            let r = r
                .trim_end_matches(".zip")
                .trim_end_matches(".tar.gz")
                .to_string();
            if !r.is_empty() {
                git_ref = Some(r);
            }
        }
        let known = index
            .libraries
            .iter()
            .find(|l| l.repo.eq_ignore_ascii_case(&repo));
        let git_ref = match (git_ref, known) {
            (Some(r), Some(k)) => {
                // a version number of the index maps to its tag
                k.versions
                    .iter()
                    .find(|v| v.version == r || v.git_ref == r)
                    .map(|v| v.git_ref.clone())
                    .unwrap_or(r)
            }
            (Some(r), None) => r,
            (None, Some(k)) => k
                .versions
                .iter()
                .max_by(|a, b| compare_versions(&a.version, &b.version))
                .map(|v| v.git_ref.clone())
                .ok_or_else(|| anyhow!("{} has no versions in the index", k.name))?,
            (None, None) => "main".to_string(),
        };
        let version = known
            .and_then(|k| k.versions.iter().find(|v| v.git_ref == git_ref))
            .map(|v| v.version.clone())
            .or_else(|| {
                let v = git_ref.trim_start_matches('v');
                v.chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
                    .then(|| v.to_string())
            });
        return Ok(Source {
            name: known.map(|k| k.name.clone()),
            version,
            repo,
            git_ref,
            packages: known.map(|k| k.packages()).unwrap_or_default(),
        });
    }
    let entry = index.find(base).ok_or_else(|| {
        anyhow!(
            "'{base}' is not in the library index (known: {}); give a GitHub URL instead",
            index
                .libraries
                .iter()
                .map(|l| l.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    })?;
    let v = match &at {
        Some(want) => entry
            .versions
            .iter()
            .find(|v| v.version == *want || v.git_ref == *want)
            .cloned()
            .unwrap_or(IndexVersion {
                // not listed: try the conventional tag
                version: want.trim_start_matches('v').to_string(),
                git_ref: if want.starts_with('v') {
                    want.clone()
                } else {
                    format!("v{want}")
                },
                uses: BTreeMap::new(),
            }),
        None => entry
            .versions
            .iter()
            .max_by(|a, b| compare_versions(&a.version, &b.version))
            .cloned()
            .ok_or_else(|| anyhow!("{} has no versions in the index", entry.name))?,
    };
    Ok(Source {
        name: Some(entry.name.clone()),
        version: Some(v.version),
        repo: entry.repo.clone(),
        git_ref: v.git_ref,
        packages: entry.packages(),
    })
}

impl Source {
    /// Zip archive URLs of the source, most likely first (a tag, then a branch).
    pub fn archive_urls(&self) -> Vec<String> {
        vec![
            format!(
                "https://codeload.github.com/{}/zip/refs/tags/{}",
                self.repo, self.git_ref
            ),
            format!(
                "https://codeload.github.com/{}/zip/refs/heads/{}",
                self.repo, self.git_ref
            ),
            format!(
                "https://github.com/{}/archive/{}.zip",
                self.repo, self.git_ref
            ),
        ]
    }

    /// jsDelivr's GitHub CDN (sends CORS headers, so a browser can read it): the file
    /// listing, and the base URL of the files.
    pub fn cdn_urls(&self) -> (String, String) {
        (
            format!(
                "https://data.jsdelivr.com/v1/packages/gh/{}@{}?structure=flat",
                self.repo, self.git_ref
            ),
            format!("https://cdn.jsdelivr.net/gh/{}@{}", self.repo, self.git_ref),
        )
    }
}

/// `"Modelica 4.1.0"` → `("Modelica", Some("4.1.0"))`; also `Name-1.0` is not split.
pub fn split_versioned(dir_name: &str) -> (String, Option<String>) {
    let s = dir_name.trim_end_matches(".mo");
    match s.split_once(' ') {
        Some((n, v)) if !v.trim().is_empty() => (n.to_string(), Some(v.trim().to_string())),
        _ => (s.to_string(), None),
    }
}

/// The class name, `version` and `uses` of a library's top-level `package.mo` (or
/// single-file library).
pub fn package_meta(text: &str) -> (Option<String>, ClassMeta) {
    let doc = crate::frontend::parse_document("package.mo", text);
    match doc.ast.classes.iter().next() {
        Some((name, def)) => (Some(name.clone()), class_meta(def)),
        None => (None, ClassMeta::default()),
    }
}

/// A library available to resolve `uses` against.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Available {
    pub name: String,
    pub version: Option<String>,
    /// Where it is: a directory, or `loaded` for one given explicitly (`-L`, sources, or
    /// already in the workbench).
    pub origin: String,
    /// Given explicitly: it is used whatever version is asked for.
    pub loaded: bool,
}

/// A `uses(Name(version="..."))` requirement.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Requirement {
    /// The library (or model) that states it.
    pub by: String,
    pub name: String,
    pub version: String,
}

/// A library chosen for a requirement.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Selected {
    pub name: String,
    pub version: Option<String>,
    pub origin: String,
    pub loaded: bool,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct Resolution {
    pub selected: Vec<Selected>,
    /// Requirements no available library satisfies.
    pub missing: Vec<String>,
    /// Incompatible requirements (different major versions asked for, or a loaded
    /// library of another major version than required).
    pub conflicts: Vec<String>,
    /// Compatible differences (another minor version used than asked for).
    pub notes: Vec<String>,
}

/// Resolve requirements against the available libraries. A library given explicitly
/// is always used; otherwise the exact version asked for is chosen, else the newest of
/// the same major version (a compatible one), else the requirement is missing.
pub fn resolve(reqs: &[Requirement], avail: &[Available]) -> Resolution {
    let mut out = Resolution::default();
    let mut names: Vec<&str> = Vec::new();
    for r in reqs {
        if !names.contains(&r.name.as_str()) {
            names.push(&r.name);
        }
    }
    for name in names {
        let rs: Vec<&Requirement> = reqs.iter().filter(|r| r.name == name).collect();
        let mut wanted: Vec<&str> = rs
            .iter()
            .map(|r| r.version.as_str())
            .filter(|v| !v.is_empty())
            .collect();
        wanted.sort_by(|a, b| compare_versions(b, a));
        wanted.dedup();
        let who = |v: &str| {
            rs.iter()
                .filter(|r| r.version == v)
                .map(|r| r.by.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let majors: Vec<Option<u64>> = {
            let mut m: Vec<Option<u64>> = wanted.iter().map(|v| major(v)).collect();
            m.dedup();
            m
        };
        if majors.len() > 1 {
            out.conflicts.push(format!(
                "{name}: incompatible versions required: {}",
                wanted
                    .iter()
                    .map(|v| format!("{v} (by {})", who(v)))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        } else if wanted.len() > 1 {
            out.notes.push(format!(
                "{name}: versions {} required; using one {} release for all",
                wanted.join(", "),
                majors[0].map_or("".into(), |m| m.to_string())
            ));
        }
        let cands: Vec<&Available> = avail.iter().filter(|a| a.name == name).collect();
        let pick = |a: &Available| Selected {
            name: a.name.clone(),
            version: a.version.clone(),
            origin: a.origin.clone(),
            loaded: a.loaded,
        };
        if let Some(l) = cands.iter().find(|a| a.loaded) {
            for v in &wanted {
                match &l.version {
                    Some(have) if have == v => {}
                    Some(have) if major(have) == major(v) => out.notes.push(format!(
                        "{name} {v} required by {}; {have} is loaded (compatible)",
                        who(v)
                    )),
                    Some(have) => out.conflicts.push(format!(
                        "{name} {v} required by {}, but {name} {have} is loaded",
                        who(v)
                    )),
                    None => {}
                }
            }
            out.selected.push(pick(l));
            continue;
        }
        if cands.is_empty() {
            out.missing.push(format!(
                "{name}{} (required by {}) is not installed",
                wanted.first().map(|v| format!(" {v}")).unwrap_or_default(),
                rs.iter()
                    .map(|r| r.by.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
            continue;
        }
        // exact match of the highest wanted version, then same major, newest first
        let exact = wanted
            .iter()
            .find_map(|v| cands.iter().find(|a| a.version.as_deref() == Some(*v)));
        let chosen = exact.copied().or_else(|| {
            let m = wanted.first().and_then(|v| major(v));
            cands
                .iter()
                .filter(|a| m.is_none() || a.version.as_deref().and_then(major) == m)
                .max_by(|a, b| {
                    compare_versions(
                        a.version.as_deref().unwrap_or(""),
                        b.version.as_deref().unwrap_or(""),
                    )
                })
                .copied()
        });
        match chosen {
            Some(a) => {
                if let (Some(v), Some(have)) = (wanted.first(), &a.version)
                    && have != v
                {
                    out.notes.push(format!(
                        "{name} {v} required by {}; using installed {have} (compatible)",
                        who(v)
                    ));
                }
                out.selected.push(pick(a));
            }
            None => {
                let have: Vec<String> = cands
                    .iter()
                    .map(|a| a.version.clone().unwrap_or_else(|| "?".into()))
                    .collect();
                out.conflicts.push(format!(
                    "{name} {} required by {}, but only {} installed",
                    wanted.first().copied().unwrap_or(""),
                    rs.iter()
                        .map(|r| r.by.as_str())
                        .collect::<Vec<_>>()
                        .join(", "),
                    have.join(", ")
                ));
            }
        }
    }
    out
}

/// Installed libraries in a library directory: `Name version/package.mo` directories,
/// `Name/package.mo` directories and `Name version.mo` single files.
#[cfg(not(target_arch = "wasm32"))]
pub fn scan_installed(dir: &std::path::Path) -> Vec<Available> {
    let mut out = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let fname = p
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let file = if p.is_dir() {
            p.join("package.mo")
        } else if fname.ends_with(".mo") {
            p.clone()
        } else {
            continue;
        };
        if !file.is_file() {
            continue;
        }
        let (name, mut version) = split_versioned(&fname);
        if version.is_none() {
            // the version annotation of the top-level package
            if let Ok(t) = std::fs::read_to_string(&file) {
                version = package_meta(&t).1.version;
            }
        }
        out.push(Available {
            name,
            version,
            origin: p.to_string_lossy().to_string(),
            loaded: false,
        });
    }
    out
}

/// The directory libraries are installed into: `$DAEDALUS_LIBRARIES`, else
/// `~/.daedalus/libraries`.
#[cfg(not(target_arch = "wasm32"))]
pub fn default_dir() -> std::path::PathBuf {
    if let Some(d) = std::env::var_os("DAEDALUS_LIBRARIES") {
        return d.into();
    }
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .unwrap_or_else(|| ".".into());
    std::path::PathBuf::from(home)
        .join(".daedalus")
        .join("libraries")
}

/// The `uses` requirements stated by the top-level classes of `lib`'s parsed documents.
pub fn requirements(lib: &crate::frontend::Library) -> Vec<Requirement> {
    let mut out = Vec::new();
    for d in &lib.docs {
        // only a library's top-level package (or a model without `within`) states `uses`
        if d.ast.within.as_ref().is_some_and(|w| !w.name.is_empty()) {
            continue;
        }
        for (name, def) in d.ast.classes.iter() {
            for (lib_name, version) in class_meta(def).uses {
                if !out
                    .iter()
                    .any(|r: &Requirement| r.by == *name && r.name == lib_name)
                {
                    out.push(Requirement {
                        by: name.clone(),
                        name: lib_name,
                        version,
                    });
                }
            }
        }
    }
    out
}

/// The libraries loaded in `lib` (top-level packages of parsed documents and registered
/// files) with their `version` annotation when it is parsed.
pub fn loaded(lib: &crate::frontend::Library) -> Vec<Available> {
    let mut out: Vec<Available> = Vec::new();
    for d in &lib.docs {
        if d.ast.within.as_ref().is_some_and(|w| !w.name.is_empty()) {
            continue;
        }
        for (name, def) in d.ast.classes.iter() {
            if out.iter().any(|a| a.name == *name) {
                continue;
            }
            out.push(Available {
                name: name.clone(),
                version: class_meta(def).version,
                origin: d.uri.clone(),
                loaded: true,
            });
        }
    }
    for f in &lib.lazy {
        if f.class.contains('.') || out.iter().any(|a| a.name == f.class) {
            continue;
        }
        let version = f
            .text
            .as_deref()
            .map(String::from)
            .or_else(|| std::fs::read_to_string(&f.path).ok())
            .and_then(|t| package_meta(&t).1.version);
        out.push(Available {
            name: f.class.clone(),
            version,
            origin: f.path.to_string_lossy().to_string(),
            loaded: true,
        });
    }
    out
}
