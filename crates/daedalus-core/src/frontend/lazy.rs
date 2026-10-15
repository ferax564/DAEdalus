//! On-demand loading of library directories.
//!
//! A library in the `package.mo` directory layout names every class stored in its own
//! file by the file's path, so the classes a model needs can be found before anything is
//! parsed. A light token scan splits each file into its classes and lists the dotted names
//! each class body mentions; starting from the model, every name is resolved against the
//! enclosing classes (and imports) the way Modelica lookup does, and the class it reaches
//! is needed in full: its body, its nested classes and its base classes. Enclosing
//! packages are needed for their own body only, so a package file full of examples does
//! not drag the examples in. Only the files holding needed classes are parsed.
//!
//! Anything the scan cannot see shows up as a failed translation, and the caller then
//! loads the rest with [`Library::load_all`].

// The browser build registers no directories, so the closure code is unused there.
#![cfg_attr(target_arch = "wasm32", allow(dead_code, unused_imports))]

use super::Library;
use rustc_hash::{FxHashMap, FxHashSet};
use std::path::{Path, PathBuf};

/// A library file known by path, not parsed yet.
#[derive(Clone, Debug)]
pub struct LazyFile {
    pub path: PathBuf,
    /// Dotted class path the file defines (the package for a `package.mo`).
    pub class: String,
    pub loaded: bool,
}

/// One class found by the token scan, with what its own body (not its nested classes)
/// mentions.
#[derive(Default, Debug)]
struct SClass {
    /// Path below the file's class ("" for the file's class itself).
    rel: String,
    children: Vec<usize>,
    names: Vec<String>,
    extends: Vec<String>,
    /// `import A = B.C;` and `import B.C;` (alias C).
    aliases: Vec<(String, String)>,
    /// `import B.C.*;` and `import B.C.{x, y};`
    wildcards: Vec<String>,
    /// `X = Y(...)`: the class Y it abbreviates.
    short: Option<String>,
    package: bool,
}

#[derive(PartialEq, Debug)]
enum Tok {
    Name(String),
    Eq,
    Semi,
    Star,
    Brace,
    LParen,
    RParen,
    Comma,
    Other,
}

fn tokens(text: &str) -> Vec<Tok> {
    let b = text.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut toks: Vec<Tok> = Vec::new();
    let ident = |i: &mut usize| -> String {
        let s = *i;
        if b[*i] == b'\'' {
            *i += 1;
            while *i < n && b[*i] != b'\'' {
                if b[*i] == b'\\' {
                    *i += 1;
                }
                *i += 1;
            }
            *i = (*i + 1).min(n);
        } else {
            while *i < n && (b[*i].is_ascii_alphanumeric() || b[*i] == b'_') {
                *i += 1;
            }
        }
        text[s..(*i).min(n)].to_string()
    };
    let starts_ident = |c: u8| c.is_ascii_alphabetic() || c == b'_' || c == b'\'';
    while i < n {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            while i < n && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i += 2;
        } else if c == b'"' {
            i += 1;
            while i < n && b[i] != b'"' {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i += 1;
        } else if starts_ident(c) {
            let mut name = ident(&mut i);
            // `.` followed by another identifier continues the name (not `.*`, `.{`).
            while i + 1 < n && b[i] == b'.' && starts_ident(b[i + 1]) {
                i += 1;
                name.push('.');
                name.push_str(&ident(&mut i));
            }
            toks.push(Tok::Name(name));
        } else if c.is_ascii_digit() {
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'.') {
                i += 1;
            }
            toks.push(Tok::Other);
        } else {
            toks.push(match c {
                b'=' if i + 1 < n && b[i + 1] == b'=' => {
                    i += 1;
                    Tok::Other
                }
                b'=' => Tok::Eq,
                b';' => Tok::Semi,
                b'*' => Tok::Star,
                b'{' => Tok::Brace,
                b'(' => Tok::LParen,
                b')' => Tok::RParen,
                b',' => Tok::Comma,
                _ => Tok::Other,
            });
            i += 1;
        }
    }
    toks
}

const CLASS_KW: [&str; 9] = [
    "class",
    "model",
    "block",
    "connector",
    "record",
    "type",
    "package",
    "function",
    "operator",
];
const PREFIX_KW: [&str; 6] = [
    "operator",
    "expandable",
    "pure",
    "impure",
    "partial",
    "encapsulated",
];

/// Split a file into its classes. The first top-level class is the file's class (rel "").
fn scan(text: &str) -> Vec<SClass> {
    let toks = tokens(text);
    let mut classes: Vec<SClass> = vec![SClass::default()];
    // Stack of open long class definitions (index, name); the root entry holds whatever
    // lies outside every class (the `within` clause).
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut top_seen = false;
    // A short class definition collects names until the `;` at its own paren depth.
    let mut short: Option<(usize, i32)> = None;
    let mut depth = 0i32;
    let mut k = 0;
    let name_at = |j: usize| match toks.get(j) {
        Some(Tok::Name(s)) => Some(s.as_str()),
        _ => None,
    };
    while k < toks.len() {
        let cur = short
            .map(|(c, _)| c)
            .or_else(|| stack.last().map(|s| s.0))
            .unwrap_or(0);
        match &toks[k] {
            Tok::LParen => depth += 1,
            // A short definition ends at its `;`, or at the `,` / `)` closing the
            // modification it stands in (`redeclare package Medium = Lib.Water`).
            Tok::RParen => {
                depth -= 1;
                if let Some((_, d)) = short
                    && depth < d
                {
                    short = None;
                }
            }
            Tok::Semi | Tok::Comma => {
                if let Some((_, d)) = short
                    && d == depth
                {
                    short = None;
                }
            }
            Tok::Name(name) => {
                let w = name.as_str();
                if w == "end" {
                    if let (Some(x), Some(top)) = (name_at(k + 1), stack.last())
                        && x == top.1
                    {
                        stack.pop();
                        k += 2;
                        continue;
                    }
                } else if CLASS_KW.contains(&w) && short.is_none() {
                    // `operator record R`, `class extends C`, `model M = ...`
                    let mut j = k + 1;
                    while let Some(x) = name_at(j) {
                        if CLASS_KW.contains(&x) || PREFIX_KW.contains(&x) {
                            j += 1;
                        } else {
                            break;
                        }
                    }
                    let mut cname = name_at(j);
                    let ext = cname == Some("extends");
                    if ext {
                        j += 1;
                        cname = name_at(j);
                    }
                    // `function f(x = 1)` passes a function argument; it defines nothing.
                    let call = !ext && toks.get(j + 1) == Some(&Tok::LParen);
                    if let Some(cn) = cname
                        && !cn.contains('.')
                        && !call
                    {
                        let is_short = toks.get(j + 1) == Some(&Tok::Eq);
                        let parent = if stack.is_empty() { None } else { Some(cur) };
                        let idx = if parent.is_none() && !top_seen {
                            top_seen = true;
                            0
                        } else {
                            let rel = match parent {
                                Some(p) if classes[p].rel.is_empty() => cn.to_string(),
                                Some(p) => format!("{}.{cn}", classes[p].rel),
                                None => cn.to_string(),
                            };
                            classes.push(SClass {
                                rel,
                                ..Default::default()
                            });
                            let i = classes.len() - 1;
                            let p = parent.unwrap_or(0);
                            classes[p].children.push(i);
                            i
                        };
                        classes[idx].package = (k..j).any(|x| name_at(x) == Some("package"));
                        if is_short {
                            if let Some(t) = name_at(j + 2) {
                                classes[idx].short = Some(t.to_string());
                            }
                            short = Some((idx, depth));
                            k = j + 2;
                        } else {
                            stack.push((idx, cn.to_string()));
                            k = j + 1;
                        }
                        continue;
                    }
                } else if w == "extends" {
                    if let Some(e) = name_at(k + 1) {
                        classes[cur].extends.push(e.to_string());
                    }
                } else if w == "import" {
                    match (toks.get(k + 1), toks.get(k + 2), toks.get(k + 3)) {
                        (Some(Tok::Name(a)), Some(Tok::Eq), Some(Tok::Name(t))) => {
                            classes[cur].aliases.push((a.clone(), t.clone()));
                        }
                        (Some(Tok::Name(t)), Some(Tok::Other), Some(Tok::Star | Tok::Brace)) => {
                            classes[cur].wildcards.push(t.clone());
                        }
                        (Some(Tok::Name(t)), Some(Tok::Semi), _) => {
                            let last = t.rsplit('.').next().unwrap_or(t).to_string();
                            classes[cur].aliases.push((last, t.clone()));
                        }
                        _ => {}
                    }
                }
                classes[cur].names.push(name.clone());
            }
            _ => {}
        }
        k += 1;
    }
    classes
}

/// Where a class lives: a file and one of the classes its scan found.
type Loc = (usize, usize);

/// State of one closure computation.
struct Closure<'a> {
    lib: &'a Library,
    scans: FxHashMap<usize, Vec<SClass>>,
    texts: FxHashMap<usize, String>,
    /// Every class of the scanned files, by full path.
    classes: FxHashMap<String, Loc>,
    /// (class, full) already queued; a class needed in full is also marked for its body.
    done: FxHashSet<(Loc, bool)>,
    queue: Vec<(Loc, bool)>,
}

impl<'a> Closure<'a> {
    fn path(&self, (f, c): Loc) -> String {
        let rel = &self.scans[&f][c].rel;
        if rel.is_empty() {
            self.lib.lazy[f].class.clone()
        } else {
            format!("{}.{rel}", self.lib.lazy[f].class)
        }
    }

    fn scan_file(&mut self, f: usize) -> bool {
        if self.scans.contains_key(&f) {
            return true;
        }
        let Ok(text) = std::fs::read_to_string(&self.lib.lazy[f].path) else {
            return false;
        };
        let sc = scan(&text);
        let base = self.lib.lazy[f].class.clone();
        for (i, c) in sc.iter().enumerate() {
            let p = if c.rel.is_empty() {
                base.clone()
            } else {
                format!("{base}.{}", c.rel)
            };
            self.classes.entry(p).or_insert((f, i));
        }
        self.scans.insert(f, sc);
        self.texts.insert(f, text);
        true
    }

    /// The class at a full path, scanning the file that would hold it.
    fn class_at(&mut self, p: &str) -> Option<Loc> {
        if let Some(&l) = self.classes.get(p) {
            return Some(l);
        }
        let &f = self.lib.lazy_index.get(p)?;
        if !self.scan_file(f) {
            return None;
        }
        Some((f, 0))
    }

    fn need(&mut self, l: Loc, full: bool) {
        if self.done.insert((l, full)) {
            self.queue.push((l, full));
        }
    }

    /// Resolve a dotted name seen in class `at` to the deepest class it reaches.
    fn resolve(&mut self, at: Loc, name: &str, hops: u32) -> Option<Loc> {
        let name = name.strip_prefix('.').unwrap_or(name);
        let (first, rest) = match name.find('.') {
            Some(k) => (&name[..k], &name[k + 1..]),
            None => (name, ""),
        };
        let mut head: Option<Loc> = None;
        // Enclosing classes, innermost first; each one's imports apply too.
        let mut scope = Some(self.path(at));
        let mut wild: Vec<String> = Vec::new();
        while let Some(s) = scope {
            let cand = if s.is_empty() {
                first.to_string()
            } else {
                format!("{s}.{first}")
            };
            if let Some(l) = self.class_at(&cand) {
                head = Some(l);
                break;
            }
            if let Some(sl) = self.class_at(&s) {
                let (f, c) = sl;
                let cls = &self.scans[&f][c];
                if let Some((_, t)) = cls.aliases.iter().find(|(a, _)| a == first) {
                    let t = t.clone();
                    head = self.class_at(&t);
                    if head.is_none() {
                        // An alias of a class nested inside a file: walk it.
                        return self.walk_from_root(&t, rest, hops);
                    }
                    break;
                }
                wild.extend(cls.wildcards.iter().cloned());
            }
            scope = if s.is_empty() {
                None
            } else {
                Some(s.rfind('.').map_or(String::new(), |k| s[..k].to_string()))
            };
        }
        if head.is_none() {
            for w in &wild {
                if let Some(l) = self.class_at(&format!("{w}.{first}")) {
                    head = Some(l);
                    break;
                }
            }
        }
        let mut cur = head?;
        self.walk(&mut cur, rest, hops);
        Some(cur)
    }

    fn walk_from_root(&mut self, full: &str, rest: &str, hops: u32) -> Option<Loc> {
        // Longest prefix of `full` that is a class.
        let mut p = full.to_string();
        let mut tail: Vec<String> = Vec::new();
        let mut cur = loop {
            if let Some(l) = self.class_at(&p) {
                break l;
            }
            let k = p.rfind('.')?;
            tail.push(p[k + 1..].to_string());
            p.truncate(k);
        };
        tail.reverse();
        let mut path = tail.join(".");
        if !rest.is_empty() {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(rest);
        }
        self.walk(&mut cur, &path, hops);
        Some(cur)
    }

    /// Follow `rest` (dotted) down from class `cur`, through abbreviations.
    fn walk(&mut self, cur: &mut Loc, rest: &str, hops: u32) {
        if rest.is_empty() {
            return;
        }
        let mut parts = rest.split('.').peekable();
        while let Some(comp) = parts.next() {
            let p = format!("{}.{comp}", self.path(*cur));
            if let Some(l) = self.class_at(&p) {
                *cur = l;
                continue;
            }
            // `Medium.X` with `package Medium = Lib.Water`: continue in Lib.Water.
            let (f, c) = *cur;
            let short = self.scans[&f][c].short.clone();
            if let Some(t) = short
                && hops < 8
                && let Some(mut l) = self.resolve(*cur, &t, hops + 1)
            {
                self.need(l, true);
                let left: Vec<&str> = std::iter::once(comp).chain(parts).collect();
                self.walk(&mut l, &left.join("."), hops + 1);
                *cur = l;
            }
            return;
        }
    }

    fn run(&mut self) {
        while let Some((l, full)) = self.queue.pop() {
            let (f, c) = l;
            // Enclosing classes: their own body only.
            let p = self.path(l);
            let mut q = p.as_str();
            while let Some(k) = q.rfind('.') {
                q = &q[..k];
                if let Some(a) = self.class_at(q) {
                    self.need(a, false);
                }
            }
            if !full && self.done.contains(&(l, true)) {
                continue;
            }
            let cls = &self.scans[&f][c];
            // Base classes and abbreviated classes are needed whole; a package named
            // otherwise (an import, a `uses` annotation) only for its own body.
            let mut whole: Vec<String> = cls.extends.clone();
            whole.extend(cls.short.iter().cloned());
            let mut names: Vec<String> = cls.names.clone();
            names.extend(cls.aliases.iter().map(|(_, t)| t.clone()));
            names.extend(cls.wildcards.iter().cloned());
            let children = cls.children.clone();
            for n in &whole {
                if let Some(r) = self.resolve(l, n, 0) {
                    self.need(r, true);
                }
            }
            for n in &names {
                if let Some(r) = self.resolve(l, n, 0) {
                    let pkg = self.scans[&r.0][r.1].package;
                    self.need(r, !pkg);
                }
            }
            if full {
                for ch in children {
                    self.need((f, ch), true);
                }
                // A package stored as a directory: the classes in its own files.
                if c == 0 && self.lib.lazy[f].path.ends_with("package.mo") {
                    let pre = format!("{p}.");
                    let subs: Vec<usize> = (0..self.lib.lazy.len())
                        .filter(|&i| {
                            let s = &self.lib.lazy[i].class;
                            s.starts_with(&pre) && !s[pre.len()..].contains('.')
                        })
                        .collect();
                    for i in subs {
                        if self.scan_file(i) {
                            self.need((i, 0), true);
                        }
                    }
                }
            }
        }
    }
}

impl Library {
    /// Register a directory in the `package.mo` layout without parsing it; files are
    /// parsed when [`Library::load_for`] finds that a model needs them. Other directories
    /// are loaded at once.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn add_dir_lazy(&mut self, dir: &Path) -> anyhow::Result<usize> {
        if !dir.join("package.mo").is_file() {
            return self.load_dir(dir);
        }
        let root = dir
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        let mut stack = vec![(dir.to_path_buf(), root)];
        let mut count = 0;
        while let Some((d, class)) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&d)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.path());
            for e in entries {
                let p = e.path();
                let stem = p
                    .file_stem()
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_default();
                if p.is_dir() {
                    if p.join("package.mo").is_file() {
                        stack.push((p, format!("{class}.{stem}")));
                    }
                } else if p.extension().is_some_and(|x| x == "mo") {
                    let c = if stem == "package" {
                        class.clone()
                    } else {
                        format!("{class}.{stem}")
                    };
                    self.lazy_index.insert(c.clone(), self.lazy.len());
                    self.lazy.push(LazyFile {
                        path: p,
                        class: c,
                        loaded: false,
                    });
                    count += 1;
                }
            }
        }
        Ok(count)
    }

    /// Whether registered files are still unparsed.
    pub fn has_pending(&self) -> bool {
        self.lazy.iter().any(|f| !f.loaded)
    }

    /// Parse the registered files `model` can need (see the module documentation).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_for(&mut self, model: &str) {
        let mut cl = Closure {
            lib: self,
            scans: FxHashMap::default(),
            texts: FxHashMap::default(),
            classes: FxHashMap::default(),
            done: FxHashSet::default(),
            queue: Vec::new(),
        };
        if let Some(l) = cl.walk_from_root(model, "", 0) {
            cl.need(l, true);
        }
        // Names in documents loaded directly (the user's own models) are roots too.
        for d in &self.docs {
            for sc in scan(&d.text) {
                for n in sc
                    .names
                    .iter()
                    .chain(&sc.extends)
                    .chain(sc.short.iter())
                    .chain(sc.aliases.iter().map(|(_, t)| t))
                {
                    if let Some(l) = cl.walk_from_root(n, "", 0) {
                        let pkg = cl.scans[&l.0][l.1].package;
                        cl.need(l, !pkg);
                    }
                }
            }
        }
        cl.run();
        let debug = std::env::var_os("DAEDALUS_DEBUG_LAZY").is_some();
        let mut used: FxHashSet<usize> = FxHashSet::default();
        for &((f, _), _) in &cl.done {
            used.insert(f);
        }
        if debug {
            let mut v: Vec<String> = cl
                .done
                .iter()
                .filter(|(_, full)| *full)
                .map(|&(l, _)| cl.path(l))
                .collect();
            v.sort();
            for p in v {
                eprintln!("  need {p}");
            }
        }
        let mut texts = std::mem::take(&mut cl.texts);
        let files: Vec<(usize, String)> = used
            .into_iter()
            .filter_map(|f| Some((f, texts.remove(&f)?)))
            .collect();
        self.parse_lazy(files);
    }

    /// Parse every registered file not parsed yet.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_all(&mut self) {
        let files: Vec<(usize, String)> = (0..self.lazy.len())
            .filter(|&i| !self.lazy[i].loaded)
            .filter_map(|i| Some((i, std::fs::read_to_string(&self.lazy[i].path).ok()?)))
            .collect();
        self.parse_lazy(files);
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn parse_lazy(&mut self, mut files: Vec<(usize, String)>) {
        files.retain(|(i, _)| !self.lazy[*i].loaded);
        files.sort_by_key(|(i, _)| *i);
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(16);
        // Largest files first, so one big file does not finish last on its own.
        let mut order: Vec<usize> = (0..files.len()).collect();
        order.sort_by_key(|&k| std::cmp::Reverse(files[k].1.len()));
        let next = std::sync::atomic::AtomicUsize::new(0);
        let lazy = &self.lazy;
        let cache_dir = cache::dir();
        let cache_dir = cache_dir.as_deref();
        let mut docs: Vec<(usize, super::Document)> = std::thread::scope(|sc| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    sc.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            let Some(&o) = order.get(k) else {
                                break;
                            };
                            let (i, text) = &files[o];
                            let uri = lazy[*i].path.to_string_lossy();
                            out.push((*i, cache::parse(cache_dir, &uri, text)));
                        }
                        out
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|h| h.join().unwrap_or_default())
                .collect()
        });
        docs.sort_by_key(|(i, _)| *i);
        for (i, d) in docs {
            self.lazy[i].loaded = true;
            self.insert(d);
        }
    }
}

/// Parsed files kept on disk, keyed by path and content: reading one back is far faster
/// than parsing it again.
#[cfg(not(target_arch = "wasm32"))]
pub(super) mod cache {
    use super::super::{Document, parse_document};
    use bincode::Options;
    use rumoca_ir_ast as ast;
    use std::hash::Hasher;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    /// Changes whenever the AST layout can change (the parser revision or this crate).
    const FORMAT: &str = concat!("rumoca-ad00214/", env!("CARGO_PKG_VERSION"));

    /// `$DAEDALUS_CACHE`, else `$XDG_CACHE_HOME/daedalus/ast` or `~/.cache/daedalus/ast`;
    /// none with `DAEDALUS_NO_CACHE` set.
    pub fn dir() -> Option<PathBuf> {
        if std::env::var_os("DAEDALUS_NO_CACHE").is_some() {
            return None;
        }
        if let Some(d) = std::env::var_os("DAEDALUS_CACHE") {
            return Some(PathBuf::from(d));
        }
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))?;
        Some(base.join("daedalus").join("ast"))
    }

    fn key(uri: &str, text: &str) -> String {
        let mut h = rustc_hash::FxHasher::default();
        h.write(FORMAT.as_bytes());
        h.write(uri.as_bytes());
        h.write(text.as_bytes());
        format!("{:016x}-{}.bin", h.finish(), text.len())
    }

    pub fn parse(dir: Option<&Path>, uri: &str, text: &str) -> Document {
        let file = dir.map(|d| d.join(key(uri, text)));
        let opts = bincode::DefaultOptions::new();
        if let Some(f) = &file
            && let Ok(bytes) = std::fs::read(f)
            && let Ok(ast) = opts.deserialize::<ast::StoredDefinition>(&bytes)
        {
            return Document {
                uri: uri.to_string(),
                text: Arc::from(text),
                ast,
                errors: Vec::new(),
            };
        }
        let d = parse_document(uri, text);
        // Only clean parses are kept, so errors are always reported afresh.
        if d.errors.is_empty()
            && let Some(f) = &file
            && let Ok(bytes) = opts.serialize(&d.ast)
            && let Some(parent) = f.parent()
            && std::fs::create_dir_all(parent).is_ok()
        {
            let tmp = f.with_extension(format!("tmp{}", std::process::id()));
            if std::fs::write(&tmp, &bytes).is_ok() {
                let _ = std::fs::rename(&tmp, f);
            }
        }
        d
    }
}

#[cfg(test)]
mod tests {
    use super::scan;

    #[test]
    fn scan_splits_classes_and_finds_names() {
        let s = scan(
            "within Modelica.Blocks; package P \"doc A.B\" // C.D\n\
             import SI = Modelica.Units.SI; import Modelica.Math.*; import Modelica.Constants;\n\
             extends Modelica.Icons.Package; /* E.F */\n\
             model M extends Interfaces.SISO(k=2.5e-3); SI.Time t = 'q.r'.x;\n\
             replaceable package Medium = Lib.Water constrainedby Lib.Partial; end M;\n\
             type T = Real(unit=\"s\"); end P;",
        );
        let top = &s[0];
        assert_eq!(top.rel, "");
        assert!(
            top.aliases
                .contains(&("SI".into(), "Modelica.Units.SI".into()))
        );
        assert!(
            top.aliases
                .contains(&("Constants".into(), "Modelica.Constants".into()))
        );
        assert_eq!(top.wildcards, vec!["Modelica.Math".to_string()]);
        assert_eq!(top.extends, vec!["Modelica.Icons.Package".to_string()]);
        assert!(!top.names.iter().any(|n| n == "SI.Time"));
        let m = s.iter().find(|c| c.rel == "M").unwrap();
        assert_eq!(m.extends, vec!["Interfaces.SISO".to_string()]);
        assert!(m.names.contains(&"SI.Time".to_string()));
        assert!(
            !m.names
                .iter()
                .any(|n| n == "A.B" || n == "C.D" || n == "E.F")
        );
        let med = s.iter().find(|c| c.rel == "M.Medium").unwrap();
        assert_eq!(med.short.as_deref(), Some("Lib.Water"));
        assert!(med.names.contains(&"Lib.Partial".to_string()));
        let t = s.iter().find(|c| c.rel == "T").unwrap();
        assert_eq!(t.short.as_deref(), Some("Real"));
        assert_eq!(top.children.len(), 2);
    }
}
