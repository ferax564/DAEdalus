//! Diff of two versions of a class: a line diff of its source text and a structural diff
//! of what it declares (extends clauses, components with their parameters, modifiers and
//! placements, connections with their routes, and equations).
//!
//! Both versions are parsed on their own, without the libraries they use, so a diff
//! works on any pair of files. Inherited members are not part of a class's diff.

use crate::frontend::inspect::{self, ComponentInfo, ConnectionInfo};
use crate::frontend::{Library, tree::ClassId, tree::ClassTree};
use anyhow::{Result, anyhow};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

/// One line of a text diff.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiffLine {
    /// ' ' unchanged, '-' only in the old text, '+' only in the new text.
    pub op: char,
    /// Line number in the old text (1-based), for ' ' and '-'.
    pub old: Option<usize>,
    /// Line number in the new text (1-based), for ' ' and '+'.
    pub new: Option<usize>,
    pub text: String,
}

/// A line diff of two texts: common prefix and suffix, and a longest common subsequence
/// of the lines in between (a plain replace when that region is very large).
pub fn text_diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf]
    {
        suf += 1;
    }
    let am = &a[pre..a.len() - suf];
    let bm = &b[pre..b.len() - suf];
    let mut out = Vec::with_capacity(a.len().max(b.len()) + 8);
    for (i, l) in a[..pre].iter().enumerate() {
        out.push(DiffLine {
            op: ' ',
            old: Some(i + 1),
            new: Some(i + 1),
            text: l.to_string(),
        });
    }
    // (op, index in am or bm)
    let mut ops: Vec<(char, usize, usize)> = Vec::new();
    let (n, m) = (am.len(), bm.len());
    if n * m <= 25_000_000 {
        // LCS table over the middle, u16 lengths would overflow beyond 65k lines; use u32.
        let mut t = vec![0u32; (n + 1) * (m + 1)];
        let w = m + 1;
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                t[i * w + j] = if am[i] == bm[j] {
                    t[(i + 1) * w + j + 1] + 1
                } else {
                    t[(i + 1) * w + j].max(t[i * w + j + 1])
                };
            }
        }
        let (mut i, mut j) = (0, 0);
        while i < n || j < m {
            if i < n && j < m && am[i] == bm[j] {
                ops.push((' ', i, j));
                i += 1;
                j += 1;
            } else if j < m && (i == n || t[i * w + j + 1] >= t[(i + 1) * w + j]) {
                ops.push(('+', i, j));
                j += 1;
            } else {
                ops.push(('-', i, j));
                i += 1;
            }
        }
    } else {
        ops.extend((0..n).map(|i| ('-', i, 0)));
        ops.extend((0..m).map(|j| ('+', n, j)));
    }
    // Within a run of changes, list removals before additions (like `diff -u`).
    let mut k = 0;
    while k < ops.len() {
        if ops[k].0 == ' ' {
            let (_, i, j) = ops[k];
            out.push(DiffLine {
                op: ' ',
                old: Some(pre + i + 1),
                new: Some(pre + j + 1),
                text: am[i].to_string(),
            });
            k += 1;
            continue;
        }
        let start = k;
        while k < ops.len() && ops[k].0 != ' ' {
            k += 1;
        }
        for &(op, i, _) in &ops[start..k] {
            if op == '-' {
                out.push(DiffLine {
                    op: '-',
                    old: Some(pre + i + 1),
                    new: None,
                    text: am[i].to_string(),
                });
            }
        }
        for &(op, _, j) in &ops[start..k] {
            if op == '+' {
                out.push(DiffLine {
                    op: '+',
                    old: None,
                    new: Some(pre + j + 1),
                    text: bm[j].to_string(),
                });
            }
        }
    }
    for s in 0..suf {
        let i = a.len() - suf + s;
        let j = b.len() - suf + s;
        out.push(DiffLine {
            op: ' ',
            old: Some(i + 1),
            new: Some(j + 1),
            text: a[i].to_string(),
        });
    }
    out
}

/// A text diff in unified format with `context` lines around each change.
pub fn unified(lines: &[DiffLine], old_name: &str, new_name: &str, context: usize) -> String {
    let changed: Vec<usize> = (0..lines.len()).filter(|&i| lines[i].op != ' ').collect();
    if changed.is_empty() {
        return String::new();
    }
    let mut out = format!("--- {old_name}\n+++ {new_name}\n");
    let mut k = 0;
    while k < changed.len() {
        let lo = changed[k].saturating_sub(context);
        let mut hi = (changed[k] + context + 1).min(lines.len());
        k += 1;
        while k < changed.len() && changed[k] <= hi + context {
            hi = (changed[k] + context + 1).min(lines.len());
            k += 1;
        }
        let hunk = &lines[lo..hi];
        let old_start = hunk
            .iter()
            .find_map(|l| l.old)
            .or_else(|| lines[..lo].iter().rev().find_map(|l| l.old))
            .unwrap_or(0);
        let new_start = hunk
            .iter()
            .find_map(|l| l.new)
            .or_else(|| lines[..lo].iter().rev().find_map(|l| l.new))
            .unwrap_or(0);
        let old_n = hunk.iter().filter(|l| l.op != '+').count();
        let new_n = hunk.iter().filter(|l| l.op != '-').count();
        out.push_str(&format!(
            "@@ -{old_start},{old_n} +{new_start},{new_n} @@\n"
        ));
        for l in hunk {
            out.push(l.op);
            out.push_str(&l.text);
            out.push('\n');
        }
    }
    out
}

/// A component that differs between the versions.
#[derive(Debug, Clone, Serialize)]
pub struct ComponentChange {
    pub name: String,
    /// "added" | "removed" | "modified"
    pub change: &'static str,
    pub old_type: Option<String>,
    pub new_type: Option<String>,
    /// Declared `parameter` or `constant` (in either version).
    pub parameter: bool,
    /// Its placement changed (moved, resized or rotated in the diagram).
    pub moved: bool,
    pub old_placement: Option<Value>,
    pub new_placement: Option<Value>,
    /// What changed, one line each: `type A -> B`, `= 1 -> = 2`, `J: 0.5 -> 0.7`, ...
    pub details: Vec<String>,
}

/// A connection that differs between the versions (compared regardless of the order of
/// its two ends).
#[derive(Debug, Clone, Serialize)]
pub struct ConnectionChange {
    pub from: String,
    pub to: String,
    /// "added" | "removed" | "rerouted"
    pub change: &'static str,
    pub old_line: Option<Value>,
    pub new_line: Option<Value>,
}

/// An equation present in only one version (connections are listed separately).
#[derive(Debug, Clone, Serialize)]
pub struct EquationChange {
    /// "added" | "removed"
    pub change: &'static str,
    pub initial: bool,
    pub text: String,
}

/// The structural and text diff of one class.
#[derive(Debug, Clone, Serialize)]
pub struct ClassDiff {
    pub old_class: String,
    pub new_class: String,
    /// `(change, base class)`
    pub extends: Vec<(&'static str, String)>,
    pub components: Vec<ComponentChange>,
    pub connections: Vec<ConnectionChange>,
    pub equations: Vec<EquationChange>,
    /// Other differences: the class kind, description, documentation, icon and diagram
    /// graphics, the experiment annotation, metadata.
    pub other: Vec<String>,
    /// Line diff of the class's source text.
    pub text: Vec<DiffLine>,
}

impl ClassDiff {
    /// No structural difference and the same text.
    pub fn is_empty(&self) -> bool {
        self.extends.is_empty()
            && self.components.is_empty()
            && self.connections.is_empty()
            && self.equations.is_empty()
            && self.other.is_empty()
            && self.text.iter().all(|l| l.op == ' ')
    }

    /// Human-readable report, one change per line, as `daedalus diff` prints it.
    pub fn report(&self) -> String {
        let mut s = String::new();
        if self.old_class == self.new_class {
            s.push_str(&format!("class {}\n", self.new_class));
        } else {
            s.push_str(&format!("class {} -> {}\n", self.old_class, self.new_class));
        }
        if self.is_empty() {
            s.push_str("  no differences\n");
            return s;
        }
        let sign = |c: &str| match c {
            "added" => '+',
            "removed" => '-',
            _ => '~',
        };
        for (c, b) in &self.extends {
            s.push_str(&format!("  {} extends {b}\n", sign(c)));
        }
        if !self.components.is_empty() {
            let (params, comps): (Vec<_>, Vec<_>) =
                self.components.iter().partition(|c| c.parameter);
            for (title, list) in [("components", comps), ("parameters", params)] {
                if list.is_empty() {
                    continue;
                }
                s.push_str(&format!("  {title}:\n"));
                for c in list {
                    let ty = c.new_type.as_ref().or(c.old_type.as_ref()).cloned();
                    s.push_str(&format!(
                        "    {} {}{}\n",
                        sign(c.change),
                        c.name,
                        ty.map(|t| format!(" : {t}")).unwrap_or_default()
                    ));
                    for d in &c.details {
                        s.push_str(&format!("        {d}\n"));
                    }
                }
            }
        }
        if !self.connections.is_empty() {
            s.push_str("  connections:\n");
            for c in &self.connections {
                s.push_str(&format!(
                    "    {} connect({}, {}){}\n",
                    sign(c.change),
                    c.from,
                    c.to,
                    if c.change == "rerouted" {
                        " (line changed)"
                    } else {
                        ""
                    }
                ));
            }
        }
        if !self.equations.is_empty() {
            s.push_str("  equations:\n");
            for e in &self.equations {
                s.push_str(&format!(
                    "    {} {}{}\n",
                    sign(e.change),
                    if e.initial { "initial: " } else { "" },
                    e.text
                ));
            }
        }
        for o in &self.other {
            s.push_str(&format!("  ~ {o}\n"));
        }
        s
    }
}

/// Find `want` in a tree: its full path, or else a class with that last name (a file
/// with another `within` clause). Without `want`, the first class of document 0.
pub fn locate(tree: &ClassTree, want: Option<&str>) -> Option<ClassId> {
    if let Some(w) = want {
        if let Some(id) = tree.find_path(w) {
            return Some(id);
        }
        let last = w.rsplit('.').next().unwrap_or(w);
        let mut hit = None;
        tree.walk(&mut |id| {
            if hit.is_none() && tree.node(id).name == last && tree.node(id).def.is_some() {
                hit = Some(id);
            }
        });
        return hit;
    }
    let mut first: Option<ClassId> = None;
    tree.walk(&mut |id| {
        let n = tree.node(id);
        if first.is_none() && n.def.is_some() {
            first = Some(id);
        }
    });
    first
}

fn one_doc(uri: &str, text: &str) -> Library {
    let mut lib = Library::new();
    lib.set_document(uri, text);
    lib
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The extent, origin and rotation of a placement, which decide where it is drawn.
fn placement_key(p: &Option<Value>) -> String {
    let Some(p) = p else { return String::new() };
    let t = p.get("transformation");
    t.map(|t| t.to_string()).unwrap_or_default()
}

fn equations(eqs: &[rumoca_ir_ast::Equation]) -> Vec<String> {
    eqs.iter()
        .filter(|e| !matches!(e, rumoca_ir_ast::Equation::Connect { .. }))
        .filter(|e| !matches!(e, rumoca_ir_ast::Equation::Empty))
        .map(|e| squash(&e.to_string()))
        .collect()
}

/// Lines added and removed between two lists, as multisets in first-seen order.
fn multiset_diff(old: &[String], new: &[String]) -> (Vec<String>, Vec<String>) {
    let mut count: BTreeMap<&str, i64> = BTreeMap::new();
    for e in old {
        *count.entry(e).or_default() -= 1;
    }
    for e in new {
        *count.entry(e).or_default() += 1;
    }
    let mut added = Vec::new();
    let mut removed = Vec::new();
    let mut left = count.clone();
    for e in new {
        let c = left.get_mut(e.as_str()).unwrap();
        if *c > 0 {
            added.push(e.clone());
            *c -= 1;
        }
    }
    for e in old {
        let c = left.get_mut(e.as_str()).unwrap();
        if *c < 0 {
            removed.push(e.clone());
            *c += 1;
        }
    }
    (added, removed)
}

fn is_param(c: &ComponentInfo) -> bool {
    c.variability == "parameter" || c.variability == "constant"
}

fn compare_component(o: &ComponentInfo, n: &ComponentInfo) -> Option<ComponentChange> {
    let mut details = Vec::new();
    if o.type_name != n.type_name {
        details.push(format!("type {} -> {}", o.type_name, n.type_name));
    }
    if o.variability != n.variability || o.causality != n.causality {
        let pre = |c: &ComponentInfo| {
            format!("{} {}", c.variability, c.causality)
                .trim()
                .to_string()
        };
        details.push(format!("prefix '{}' -> '{}'", pre(o), pre(n)));
    }
    if o.binding != n.binding {
        let b = |x: &Option<String>| x.clone().map_or("(none)".to_string(), |s| format!("= {s}"));
        details.push(format!("binding {} -> {}", b(&o.binding), b(&n.binding)));
    }
    let om: BTreeMap<&str, &str> = o
        .modifiers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    let nm: BTreeMap<&str, &str> = n
        .modifiers
        .iter()
        .map(|(k, v)| (k.as_str(), v.as_str()))
        .collect();
    for (k, v) in &om {
        match nm.get(k) {
            None => details.push(format!("modifier {k} = {v} removed")),
            Some(w) if squash(w) != squash(v) => details.push(format!("modifier {k}: {v} -> {w}")),
            _ => {}
        }
    }
    for (k, v) in &nm {
        if !om.contains_key(k) {
            details.push(format!("modifier {k} = {v} added"));
        }
    }
    if o.condition != n.condition {
        details.push(format!(
            "condition {} -> {}",
            o.condition.as_deref().unwrap_or("(none)"),
            n.condition.as_deref().unwrap_or("(none)")
        ));
    }
    if o.description != n.description {
        details.push(format!(
            "description \"{}\" -> \"{}\"",
            o.description, n.description
        ));
    }
    if o.protected != n.protected {
        details.push(if n.protected {
            "now protected".into()
        } else {
            "now public".into()
        });
    }
    let moved = placement_key(&o.placement) != placement_key(&n.placement);
    if moved {
        details.push("placement changed (moved in the diagram)".into());
    }
    if details.is_empty() {
        return None;
    }
    Some(ComponentChange {
        name: n.name.clone(),
        change: "modified",
        old_type: Some(o.type_name.clone()),
        new_type: Some(n.type_name.clone()),
        parameter: is_param(o) || is_param(n),
        moved,
        old_placement: o.placement.clone(),
        new_placement: n.placement.clone(),
        details,
    })
}

fn conn_key(c: &ConnectionInfo) -> (String, String) {
    let (a, b) = (squash(&c.from), squash(&c.to));
    if a <= b { (a, b) } else { (b, a) }
}

/// Diff class `class` (or, without it, the first class of each text) between two
/// source texts. `new_class` names the class in the new text when it was renamed.
pub fn diff_sources(
    old_text: &str,
    new_text: &str,
    class: Option<&str>,
    new_class: Option<&str>,
) -> Result<ClassDiff> {
    let la = one_doc("old.mo", old_text);
    let lb = one_doc("new.mo", new_text);
    diff_libraries(&la, &lb, class, new_class.or(class))
}

/// Diff a class between two libraries (each holding one version).
pub fn diff_libraries(
    la: &Library,
    lb: &Library,
    old_class: Option<&str>,
    new_class: Option<&str>,
) -> Result<ClassDiff> {
    let ta = la.tree();
    let tb = lb.tree();
    let ia = locate(&ta, old_class).ok_or_else(|| {
        anyhow!(
            "class {} not found in the old version",
            old_class.unwrap_or("")
        )
    })?;
    let ib = locate(&tb, new_class.or(old_class).or(Some(&ta.node(ia).path))).ok_or_else(|| {
        anyhow!(
            "class {} not found in the new version",
            new_class.or(old_class).unwrap_or("")
        )
    })?;
    let pa = ta.node(ia).path.clone();
    let pb = tb.node(ib).path.clone();
    let ca = inspect::class_info(la, &pa).ok_or_else(|| anyhow!("class {pa} not found"))?;
    let cb = inspect::class_info(lb, &pb).ok_or_else(|| anyhow!("class {pb} not found"))?;
    let da = ta.node(ia).def.unwrap();
    let db = tb.node(ib).def.unwrap();

    // extends, as written
    let ext = |d: &rumoca_ir_ast::ClassDef| -> Vec<String> {
        d.extends
            .iter()
            .map(|e| {
                e.base_name
                    .name
                    .iter()
                    .map(|t| t.text.to_string())
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .collect()
    };
    let (ea, eb) = (ext(da), ext(db));
    let mut extends = Vec::new();
    for e in &ea {
        if !eb.contains(e) {
            extends.push(("removed", e.clone()));
        }
    }
    for e in &eb {
        if !ea.contains(e) {
            extends.push(("added", e.clone()));
        }
    }

    // own components only
    let own = |c: &inspect::ClassInfo| -> Vec<ComponentInfo> {
        c.components
            .iter()
            .filter(|k| k.declared_in == c.path)
            .cloned()
            .collect()
    };
    let (oa, ob) = (own(&ca), own(&cb));
    let mut components = Vec::new();
    for o in &oa {
        match ob.iter().find(|n| n.name == o.name) {
            None => components.push(ComponentChange {
                name: o.name.clone(),
                change: "removed",
                old_type: Some(o.type_name.clone()),
                new_type: None,
                parameter: is_param(o),
                moved: false,
                old_placement: o.placement.clone(),
                new_placement: None,
                details: o
                    .binding
                    .as_ref()
                    .map(|b| vec![format!("was = {b}")])
                    .unwrap_or_default(),
            }),
            Some(n) => components.extend(compare_component(o, n)),
        }
    }
    for n in &ob {
        if !oa.iter().any(|o| o.name == n.name) {
            components.push(ComponentChange {
                name: n.name.clone(),
                change: "added",
                old_type: None,
                new_type: Some(n.type_name.clone()),
                parameter: is_param(n),
                moved: false,
                old_placement: None,
                new_placement: n.placement.clone(),
                details: n
                    .binding
                    .as_ref()
                    .map(|b| vec![format!("= {b}")])
                    .unwrap_or_default(),
            });
        }
    }

    // own connections
    let conns = |c: &inspect::ClassInfo| -> Vec<ConnectionInfo> {
        c.connections
            .iter()
            .filter(|k| k.declared_in == c.path)
            .cloned()
            .collect()
    };
    let (na, nb) = (conns(&ca), conns(&cb));
    let mut connections = Vec::new();
    for o in &na {
        match nb.iter().find(|n| conn_key(n) == conn_key(o)) {
            None => connections.push(ConnectionChange {
                from: o.from.clone(),
                to: o.to.clone(),
                change: "removed",
                old_line: o.line.clone(),
                new_line: None,
            }),
            Some(n) => {
                let pts = |l: &Option<Value>| l.as_ref().and_then(|v| v.get("points")).cloned();
                if pts(&o.line) != pts(&n.line) {
                    connections.push(ConnectionChange {
                        from: n.from.clone(),
                        to: n.to.clone(),
                        change: "rerouted",
                        old_line: o.line.clone(),
                        new_line: n.line.clone(),
                    });
                }
            }
        }
    }
    for n in &nb {
        if !na.iter().any(|o| conn_key(o) == conn_key(n)) {
            connections.push(ConnectionChange {
                from: n.from.clone(),
                to: n.to.clone(),
                change: "added",
                old_line: None,
                new_line: n.line.clone(),
            });
        }
    }

    let mut eqs = Vec::new();
    for (initial, a, b) in [
        (false, &da.equations, &db.equations),
        (true, &da.initial_equations, &db.initial_equations),
    ] {
        let (added, removed) = multiset_diff(&equations(a), &equations(b));
        for t in removed {
            eqs.push(EquationChange {
                change: "removed",
                initial,
                text: t,
            });
        }
        for t in added {
            eqs.push(EquationChange {
                change: "added",
                initial,
                text: t,
            });
        }
    }

    let mut other = Vec::new();
    if ca.kind != cb.kind || ca.partial != cb.partial {
        other.push(format!("kind {} -> {}", ca.kind, cb.kind));
    }
    if ca.description != cb.description {
        other.push(format!(
            "description \"{}\" -> \"{}\"",
            ca.description, cb.description
        ));
    }
    if ca.documentation != cb.documentation || ca.revisions != cb.revisions {
        other.push("documentation changed".into());
    }
    let gr = |l: &inspect::Layer| serde_json::to_string(&l.graphics).unwrap_or_default();
    if gr(&ca.icon) != gr(&cb.icon) {
        other.push("icon graphics changed".into());
    }
    if gr(&ca.diagram) != gr(&cb.diagram) {
        other.push("diagram graphics changed".into());
    }
    if ca.experiment != cb.experiment {
        other.push(format!(
            "experiment {} -> {}",
            ca.experiment
                .as_ref()
                .map_or("(none)".into(), |v| v.to_string()),
            cb.experiment
                .as_ref()
                .map_or("(none)".into(), |v| v.to_string())
        ));
    }
    if ca.metadata != cb.metadata {
        other.push("metadata changed".into());
    }
    let algs = |d: &rumoca_ir_ast::ClassDef| d.algorithms.len() + d.initial_algorithms.len();
    if algs(da) != algs(db) {
        other.push("algorithm sections changed".into());
    }

    let src = |lib: &Library, ci: &inspect::ClassInfo| -> String {
        ci.source
            .as_ref()
            .and_then(|s| {
                let d = lib.docs.iter().find(|d| d.uri == s.uri)?;
                // Start at the beginning of the class's first line, so indentation is kept.
                let line_start = d.text[..s.start.min(d.text.len())]
                    .rfind('\n')
                    .map_or(0, |i| i + 1);
                d.text
                    .get(line_start..s.end.min(d.text.len()))
                    .map(String::from)
            })
            .unwrap_or_default()
    };
    let mut text = text_diff(&src(la, &ca), &src(lb, &cb));
    // number the lines as in the files
    let (oa, ob) = (
        ca.source
            .as_ref()
            .map_or(0, |s| s.line.saturating_sub(1) as usize),
        cb.source
            .as_ref()
            .map_or(0, |s| s.line.saturating_sub(1) as usize),
    );
    for l in &mut text {
        l.old = l.old.map(|n| n + oa);
        l.new = l.new.map(|n| n + ob);
    }
    Ok(ClassDiff {
        old_class: pa,
        new_class: pb,
        extends,
        components,
        connections,
        equations: eqs,
        other,
        text,
    })
}
