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
    /// The parse errors as short messages with their byte ranges (for editor diagnostics).
    pub error_spans: Vec<ParseDiag>,
}

/// A syntax error located in its document: byte range and a one-line message.
#[derive(Clone, Debug)]
pub struct ParseDiag {
    pub start: usize,
    pub end: usize,
    pub message: String,
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

    /// Whether a document is a file of a library registered for on-demand parsing
    /// (as opposed to a document set directly, such as the user's own models).
    pub fn is_registered_file(&self, uri: &str) -> bool {
        self.lazy_file_of(uri).is_some()
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
        Ok(f.model)
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
    let error_spans = syntax
        .parse_errors()
        .iter()
        .map(|e| {
            let sp = e.span();
            let message = match e {
                rumoca_phase_parse::ParseError::SyntaxError {
                    message,
                    expected,
                    unexpected,
                    ..
                } => {
                    let mut m = message.lines().next().unwrap_or("syntax error").to_string();
                    if let Some(u) = unexpected {
                        m = format!("{m}: unexpected `{u}`");
                    }
                    if !expected.is_empty() && expected.len() <= 6 {
                        m = format!("{m} (expected {})", expected.join(", "));
                    }
                    m
                }
                other => format!("{other:?}"),
            };
            ParseDiag {
                start: sp.start.0,
                end: sp.end.0.max(sp.start.0 + 1),
                message,
            }
        })
        .collect();
    let ast = syntax.best_effort().clone();
    Document {
        uri: uri.to_string(),
        text: Arc::from(text),
        ast,
        errors,
        error_spans,
    }
}
