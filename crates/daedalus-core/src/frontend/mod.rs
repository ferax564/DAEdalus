//! Modelica frontend: source management, class tree and flattening.
//!
//! Parsing is delegated to Rumoca's parser (`rumoca-phase-parse`, Apache-2.0);
//! instantiation, flattening and everything downstream is DAEdalus code.

pub mod inspect;
pub mod inst;
pub mod lazy;
pub mod overconstrained;
pub mod tree;

use crate::flat::FlatModel;
use anyhow::{Result, anyhow};
use rumoca_ir_ast as ast;
use std::sync::Arc;

/// A parsed source document.
pub struct Document {
    pub uri: String,
    pub text: Arc<str>,
    pub ast: ast::StoredDefinition,
    pub errors: Vec<String>,
}

/// A set of Modelica sources (libraries + user models) that compile together.
#[derive(Default)]
pub struct Library {
    pub docs: Vec<Document>,
    /// Library files registered for on-demand parsing, by path and by class.
    pub lazy: Vec<lazy::LazyFile>,
    lazy_index: rustc_hash::FxHashMap<String, usize>,
    lazy_cache: lazy::ScanCache,
    /// `package.order` of registered packages: member names in browsing order.
    pub package_orders: rustc_hash::FxHashMap<String, Vec<String>>,
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a document. Parse errors are recorded; the recovered AST is kept
    /// so the editor and browser keep working on partially valid text.
    pub fn set_document(&mut self, uri: &str, text: &str) -> Vec<String> {
        let doc = parse_document(uri, text);
        let errors = doc.errors.clone();
        self.lazy_edited(uri, text);
        self.insert(doc);
        errors
    }

    fn insert(&mut self, doc: Document) {
        let uri = doc.uri.clone();
        if let Some(d) = self.docs.iter_mut().find(|d| d.uri == uri) {
            *d = doc;
        } else {
            self.docs.push(doc);
        }
    }

    pub fn remove_document(&mut self, uri: &str) {
        self.docs.retain(|d| d.uri != uri);
        if let Some(i) = self.lazy_file_of(uri) {
            self.lazy[i].loaded = false;
        }
    }

    /// Load a directory tree (package.mo / package.order layout or loose .mo files).
    /// Files are parsed in parallel.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_dir(&mut self, dir: &std::path::Path) -> Result<usize> {
        let mut files = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&d)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.path());
            for e in entries {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "mo") {
                    files.push(p);
                }
            }
        }
        let n = files.len();
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(16);
        let next = std::sync::atomic::AtomicUsize::new(0);
        let cache_dir = lazy::cache::dir();
        let cache_dir = cache_dir.as_deref();
        let mut docs: Vec<(usize, Document)> = std::thread::scope(|sc| {
            let handles: Vec<_> = (0..threads)
                .map(|_| {
                    sc.spawn(|| {
                        let mut out = Vec::new();
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if i >= files.len() {
                                break;
                            }
                            let p = &files[i];
                            if let Ok(text) = std::fs::read_to_string(p) {
                                out.push((
                                    i,
                                    lazy::cache::parse(cache_dir, &p.to_string_lossy(), &text),
                                ));
                            }
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
        for (_, d) in docs {
            self.insert(d);
        }
        Ok(n)
    }

    pub fn tree(&self) -> tree::ClassTree<'_> {
        let docs: Vec<(&ast::StoredDefinition, usize)> = self
            .docs
            .iter()
            .enumerate()
            .map(|(i, d)| (&d.ast, i))
            .collect();
        tree::ClassTree::build(&docs)
    }

    /// Flatten `model` (full dotted path) into a [`FlatModel`].
    pub fn flatten(&self, model: &str) -> Result<FlatModel> {
        let tree = self.tree();
        let id = tree
            .find_path(model)
            .ok_or_else(|| anyhow!("model '{model}' not found"))?;
        let mut f = inst::Flattener::new(&tree);
        let first = f.flatten(id);
        if !f.oc_log.used {
            first?;
            self.mark_externals(&tree, &mut f)?;
            return Ok(f.model);
        }
        // Overconstrained connectors: flatten again with the spanning-tree answers, so that
        // `Connections.isRoot/rooted` are known and loop-closing connections use
        // `equalityConstraint` instead of potential equality.
        let answers = std::rc::Rc::new(overconstrained::OcAnswers::compute(&f.oc_log));
        if std::env::var_os("DAEDALUS_OCDEBUG").is_some() {
            eprintln!(
                "{:#?}\nroots {:?}\nbroken {:?}",
                f.oc_log, answers.roots, answers.broken
            );
        }
        drop(f);
        let mut f = inst::Flattener::new(&tree);
        f.oc_answers = Some(answers);
        f.flatten(id)?;
        self.mark_externals(&tree, &mut f)?;
        Ok(f.model)
    }

    /// Record the instances of FMU wrapper blocks (classes annotated
    /// `__Daedalus_FMU(fileName = ..., fmiType = ...)`) and make the values they supply
    /// (their outputs and the derivatives of their states) externally set variables;
    /// see [`crate::external`].
    fn mark_externals(&self, tree: &tree::ClassTree, f: &mut inst::Flattener) -> Result<()> {
        use crate::external::{DERIVATIVES, ExternalComponent, FMU_ANNOTATION, FmiKind};
        let mut found: Vec<ExternalComponent> = Vec::new();
        // Per component: names of the wrapper's outputs (`output` components, or of a
        // `...Output` connector type).
        let mut outputs: Vec<Vec<String>> = Vec::new();
        for i in &f.insts {
            let Some(def) = tree.def(i.class) else {
                continue;
            };
            let Some(a) = inspect::find_annotation(&def.annotation, FMU_ANNOTATION) else {
                continue;
            };
            if found.iter().any(|e| e.prefix == i.prefix) {
                continue;
            }
            let node = tree.node(i.class);
            let v = inspect::expr_json(a, None);
            let file = v["fileName"].as_str().ok_or_else(|| {
                anyhow!(
                    "{FMU_ANNOTATION} annotation of '{}' has no fileName",
                    node.path
                )
            })?;
            let kind_s = v["fmiType"].as_str().unwrap_or("ModelExchange");
            let kind = FmiKind::parse(kind_s).ok_or_else(|| {
                anyhow!(
                    "{FMU_ANNOTATION} of '{}': unknown fmiType '{kind_s}'",
                    node.path
                )
            })?;
            let src = self.docs.get(node.source).map_or("", |d| d.uri.as_str());
            outputs.push(
                def.components
                    .iter()
                    .filter(|(_, c)| {
                        matches!(c.causality, rumoca_core::Causality::Output(_))
                            || c.type_name
                                .name
                                .last()
                                .is_some_and(|t| t.text.ends_with("Output"))
                    })
                    .map(|(n, _)| n.clone())
                    .collect(),
            );
            found.push(ExternalComponent {
                prefix: i.prefix.clone(),
                file: resolve_file(file, src),
                kind,
                class: node.path.clone(),
            });
        }
        for (e, outs) in found.iter().zip(&outputs) {
            let pre = if e.prefix.is_empty() {
                String::new()
            } else {
                format!("{}.", e.prefix)
            };
            for v in &mut f.model.vars {
                let Some(rest) = v.name.strip_prefix(&pre) else {
                    continue;
                };
                if rest.contains('.') {
                    continue;
                }
                let base = rest.split('[').next().unwrap_or(rest);
                if base == DERIVATIVES || outs.iter().any(|o| o == base) {
                    v.kind = crate::flat::VarKind::Input;
                }
            }
        }
        f.model.externals = found;
        Ok(())
    }
}

/// An FMU file named in a wrapper annotation: absolute paths and `file://` URIs as they
/// are, other paths relative to the directory of the wrapper's source file.
fn resolve_file(file: &str, source_uri: &str) -> String {
    let file = file.strip_prefix("file://").unwrap_or(file);
    let absolute = file.starts_with('/') || file.as_bytes().get(1) == Some(&b':');
    if absolute || file.contains("://") {
        return file.to_string();
    }
    let src = source_uri.strip_prefix("file://").unwrap_or(source_uri);
    match src.rfind(['/', '\\']) {
        Some(k) => format!("{}/{}", &src[..k], file),
        None => file.to_string(),
    }
}

/// Parse one document, keeping the recovered AST on syntax errors.
pub fn parse_document(uri: &str, text: &str) -> Document {
    let syntax = rumoca_phase_parse::parse_to_syntax(text, uri);
    let errors: Vec<String> = syntax
        .parse_errors()
        .iter()
        .map(|e| rumoca_phase_parse::format_parse_error(e, uri, text))
        .collect();
    let ast = syntax.best_effort().clone();
    Document {
        uri: uri.to_string(),
        text: Arc::from(text),
        ast,
        errors,
    }
}
