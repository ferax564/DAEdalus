//! Source edits made from the workbench and the command line: a component's modifier
//! (from imported parameter data) and a class's `__Daedalus_metadata` annotation.
//!
//! Each edit returns the new text of the document that holds the class, leaving every
//! other byte of it as it was.

use crate::frontend::Library;
use crate::params::{ModTree, matching, split_top};
use anyhow::{Result, anyhow, bail};

/// The document and class definition of `class`.
fn class_source<'a>(
    lib: &'a Library,
    class: &str,
) -> Result<(&'a crate::frontend::Document, &'a rumoca_ir_ast::ClassDef)> {
    let tree = lib.tree();
    let id = tree
        .find_path(class)
        .ok_or_else(|| anyhow!("class {class} not found"))?;
    let node = tree.node(id);
    let def = node.def.ok_or_else(|| anyhow!("class {class} has no definition"))?;
    let doc = lib
        .docs
        .get(node.source)
        .ok_or_else(|| anyhow!("no source document for {class}"))?;
    Ok((doc, def))
}

/// Merge `args` into the modifier of component `component` declared in `class`
/// (`J = 0.5` becomes `inertia(J = 0.5)`; existing arguments of the same name are
/// replaced, others kept). Returns `(document uri, new text)`.
pub fn apply_modifier(
    lib: &Library,
    class: &str,
    component: &str,
    args: &ModTree,
) -> Result<(String, String)> {
    let (doc, def) = class_source(lib, class)?;
    let Some(comp) = def.components.get(component) else {
        bail!(
            "{component} is not declared in {class} itself (an inherited component is \
             modified on its extends clause)"
        );
    };
    let text = &*doc.text;
    let mut pos = comp.name_token.location.end as usize;
    if pos > text.len() || text.get(comp.name_token.location.start as usize..pos) != Some(component)
    {
        bail!("cannot locate the declaration of {component} in {}", doc.uri);
    }
    let skip_ws = |mut p: usize| {
        while p < text.len() && text.as_bytes()[p].is_ascii_whitespace() {
            p += 1;
        }
        p
    };
    let mut after = skip_ws(pos);
    if text.as_bytes().get(after) == Some(&b'[') {
        let close = matching(text, after).ok_or_else(|| anyhow!("unbalanced subscript"))?;
        pos = close + 1;
        after = skip_ws(pos);
    }
    let mut out = String::with_capacity(text.len() + 64);
    if text.as_bytes().get(after) == Some(&b'(') {
        let close = matching(text, after).ok_or_else(|| anyhow!("unbalanced modifier"))?;
        let mut tree = ModTree::parse_args(&text[after + 1..close]);
        tree.merge(args);
        out.push_str(&text[..after + 1]);
        out.push_str(&tree.args());
        out.push_str(&text[close..]);
    } else {
        out.push_str(&text[..pos]);
        out.push('(');
        out.push_str(&args.args());
        out.push(')');
        out.push_str(&text[pos..]);
    }
    Ok((doc.uri.clone(), out))
}

fn quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\").replace('"', "\\\"").replace('\n', "\\n")
    )
}

fn valid_key(k: &str) -> bool {
    !k.is_empty()
        && k.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
        && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Set the `__Daedalus_metadata(key="value", ...)` annotation of `class` to `entries`
/// (an empty list removes it). Returns `(document uri, new text)`.
pub fn set_metadata(
    lib: &Library,
    class: &str,
    entries: &[(String, String)],
) -> Result<(String, String)> {
    for (k, _) in entries {
        if !valid_key(k) {
            bail!("metadata key '{k}' is not an identifier");
        }
    }
    let (doc, def) = class_source(lib, class)?;
    let text = &*doc.text;
    let item = if entries.is_empty() {
        None
    } else {
        Some(format!(
            "__Daedalus_metadata({})",
            entries
                .iter()
                .map(|(k, v)| format!("{k}={}", quote(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    };
    if let Some(first) = def.annotation.first() {
        // The class annotation exists: edit its argument list.
        let start = first.span().start.0;
        let open = text[..start]
            .rfind('(')
            .filter(|&o| text[o + 1..start].trim().is_empty())
            .ok_or_else(|| anyhow!("cannot locate the annotation of {class}"))?;
        let close = matching(text, open).ok_or_else(|| anyhow!("unbalanced annotation"))?;
        let inner = &text[open + 1..close];
        let mut parts = split_top(inner, ',');
        let is_meta = |p: &str| p.trim_start().starts_with("__Daedalus_metadata");
        let at = parts.iter().position(|p| is_meta(p));
        match (at, &item) {
            (Some(i), Some(it)) => {
                // keep the whitespace that led the old item
                let lead: String = parts[i].chars().take_while(|c| c.is_whitespace()).collect();
                parts[i] = format!("{lead}{it}");
            }
            (Some(i), None) => {
                parts.remove(i);
                if let Some(p) = parts.first_mut() {
                    *p = p.trim_start().to_string();
                }
            }
            (None, Some(it)) => {
                if inner.trim().is_empty() {
                    parts = vec![it.clone()];
                } else {
                    parts.push(format!("\n    {it}"));
                }
            }
            (None, None) => return Ok((doc.uri.clone(), text.to_string())),
        }
        let joined = parts.join(",");
        let new_inner = if joined.trim().is_empty() { String::new() } else { joined };
        return Ok((
            doc.uri.clone(),
            format!("{}{}{}", &text[..open + 1], new_inner, &text[close..]),
        ));
    }
    let Some(item) = item else {
        return Ok((doc.uri.clone(), text.to_string()));
    };
    // No class annotation: add one before `end Name;`.
    let end_tok = def
        .end_name_token
        .as_ref()
        .ok_or_else(|| anyhow!("{class} is a short class definition: no place for an annotation"))?;
    let name_at = end_tok.location.start as usize;
    let before = text[..name_at].trim_end();
    if !before.ends_with("end") {
        bail!("cannot locate the end of {class}");
    }
    let end_kw = before.len() - 3;
    let line_start = text[..end_kw].rfind('\n').map_or(0, |i| i + 1);
    let indent: String = text[line_start..end_kw]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let insert_at = if text[line_start..end_kw].trim().is_empty() {
        line_start
    } else {
        end_kw
    };
    let ann = format!("{indent}  annotation({item});\n");
    let mut out = String::with_capacity(text.len() + ann.len());
    out.push_str(&text[..insert_at]);
    if insert_at == end_kw {
        out.push('\n');
    }
    out.push_str(&ann);
    out.push_str(&text[insert_at..]);
    Ok((doc.uri.clone(), out))
}
