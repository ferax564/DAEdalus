//! Modelica frontend: source management, class tree and flattening.
//!
//! Parsing is delegated to Rumoca's parser (`rumoca-phase-parse`, Apache-2.0);
//! instantiation, flattening and everything downstream is DAEdalus code.

pub mod inst;
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
}

impl Library {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a document. Parse errors are recorded; the recovered AST is kept
    /// so the editor and browser keep working on partially valid text.
    pub fn set_document(&mut self, uri: &str, text: &str) -> Vec<String> {
        let syntax = rumoca_phase_parse::parse_to_syntax(text, uri);
        let errors: Vec<String> = syntax
            .parse_errors()
            .iter()
            .map(|e| rumoca_phase_parse::format_parse_error(e, uri, text))
            .collect();
        let ast = syntax.best_effort().clone();
        let doc = Document { uri: uri.to_string(), text: Arc::from(text), ast, errors: errors.clone() };
        if let Some(d) = self.docs.iter_mut().find(|d| d.uri == uri) {
            *d = doc;
        } else {
            self.docs.push(doc);
        }
        errors
    }

    pub fn remove_document(&mut self, uri: &str) {
        self.docs.retain(|d| d.uri != uri);
    }

    /// Load a directory tree (package.mo / package.order layout or loose .mo files).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_dir(&mut self, dir: &std::path::Path) -> Result<usize> {
        let mut n = 0;
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let mut entries: Vec<_> = std::fs::read_dir(&d)?.filter_map(|e| e.ok()).collect();
            entries.sort_by_key(|e| e.path());
            for e in entries {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "mo") {
                    let text = std::fs::read_to_string(&p)?;
                    self.set_document(&p.to_string_lossy(), &text);
                    n += 1;
                }
            }
        }
        Ok(n)
    }

    pub fn tree(&self) -> tree::ClassTree<'_> {
        let docs: Vec<(&ast::StoredDefinition, usize)> =
            self.docs.iter().enumerate().map(|(i, d)| (&d.ast, i)).collect();
        tree::ClassTree::build(&docs)
    }

    /// Flatten `model` (full dotted path) into a [`FlatModel`].
    pub fn flatten(&self, model: &str) -> Result<FlatModel> {
        let tree = self.tree();
        let id = tree.find_path(model).ok_or_else(|| anyhow!("model '{model}' not found"))?;
        let mut f = inst::Flattener::new(&tree);
        f.flatten(id)?;
        Ok(f.model)
    }
}
