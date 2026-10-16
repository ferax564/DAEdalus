//! Equation and initialization debugger.
//!
//! Built on the backend's [`Trace`]:
//! - [`Debugger::trace_var`] / [`Debugger::trace_eq`]: for a variable, the equation that
//!   computes it, its BLT block and tearing role, and the chain of symbolic
//!   transformations from the equation as written (with component path and file:line)
//!   through alias substitution, index-reduction differentiation, dummy-derivative
//!   selection, solving and tearing;
//! - [`init_report`]: the initialization problem's unknowns with the start values used
//!   (fixed, defaulted or iteration guess, and the modifier that set them) and, when
//!   initialization fails, the failing block's residuals at the last iterate and the
//!   variables with the largest Newton update; [`simulation_failure`] does the same for a
//!   nonlinear solve that fails during the simulation;
//! - [`diagnose_singular`]: for a structurally singular model, the local over- and
//!   underdetermined subsets of a maximum matching (which equations are extra, which
//!   unknowns are unmatched, with their components);
//! - [`unconnected_connectors`]: physical connectors left unconnected.

use crate::backend::trace::{Singular, Trace, TraceBlock, TraceEq};
use crate::backend::{self, AliasKind, Block, NO_SRC, System};
use crate::codegen::{self, Model};
use crate::expr::{ExprId, VarId};
use crate::flat::{FlatModel, SrcLoc, StartSrc, VarKind};
use crate::frontend::Library;
use crate::sim::{NewtonFailure, SimOptions, Simulation};
use crate::structure::{self, StructureReport};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

/// A position in a source file.
#[derive(Clone, Debug, Serialize, PartialEq)]
pub struct Loc {
    pub file: String,
    pub line: u32,
    pub col: u32,
}

impl std::fmt::Display for Loc {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}:{}", self.file, self.line, self.col)
    }
}

/// An equation as written, with where it comes from.
#[derive(Clone, Debug, Serialize)]
pub struct SourceRef {
    pub text: String,
    /// What the equation became after alias substitution, when different.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub now: Option<String>,
    /// Why it is listed (structural diagnosis: an equation alias elimination used).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
    /// The frontend's description (`equation in a.b`, `connection`, ...).
    pub origin: String,
    /// Component path of the instance the equation belongs to (or, for connection
    /// equations, the connectors involved).
    pub component: Option<String>,
    pub loc: Option<Loc>,
    pub initial: bool,
}

/// One transformation applied on the way from the source to the solved form.
#[derive(Clone, Debug, Serialize)]
pub struct Step {
    /// source | alias | substituted | differentiated | state-selection | assign | inner |
    /// tear | residual | newton | state | parameter | initialization | start
    pub kind: &'static str,
    pub title: String,
    /// The equation after this step.
    pub equation: Option<String>,
    pub source: Option<SourceRef>,
    pub details: Vec<String>,
}

impl Step {
    fn new(kind: &'static str, title: impl Into<String>) -> Step {
        Step {
            kind,
            title: title.into(),
            equation: None,
            source: None,
            details: Vec::new(),
        }
    }
    fn eq(mut self, e: impl Into<String>) -> Step {
        self.equation = Some(e.into());
        self
    }
}

/// Where a start value comes from.
#[derive(Clone, Debug, Serialize, Default)]
pub struct StartInfo {
    pub expr: Option<String>,
    pub fixed: Option<bool>,
    /// Instance whose modifier or declaration set it (`""`: the model itself).
    pub written_in: Option<String>,
    pub loc: Option<Loc>,
    /// The alias-class member whose start value the variable took over.
    pub from_alias: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct VarTrace {
    pub name: String,
    pub role: String,
    pub unit: String,
    pub description: String,
    /// Simulation equation that computes it (an id of the structure report).
    pub equation: Option<usize>,
    pub block: Option<usize>,
    /// assign | linear | nonlinear
    pub block_kind: Option<&'static str>,
    /// assigned | inner | tear
    pub tearing_role: Option<&'static str>,
    pub alias_of: Option<String>,
    pub start: StartInfo,
    pub steps: Vec<Step>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EqTrace {
    pub id: usize,
    pub text: String,
    pub block: Option<usize>,
    /// The unknown it is solved for (none for a residual of a loop).
    pub solved_for: Option<String>,
    pub steps: Vec<Step>,
}

/// A physical connector whose flow variables are not connected.
#[derive(Clone, Debug, Serialize)]
pub struct Unconnected {
    pub connector: String,
    pub component: String,
    pub flows: Vec<String>,
    /// warning | note
    pub level: &'static str,
    pub reason: String,
}

/// An over- or underdetermined part of a structurally singular system.
#[derive(Clone, Debug, Serialize)]
pub struct Subset {
    /// Underdetermined: the unknowns no equation is left for. Overdetermined: the
    /// equations no unknown is left for.
    pub unmatched: Vec<String>,
    /// The equations of the subset (all of them involve only `variables`).
    pub equations: Vec<SourceRef>,
    pub variables: Vec<String>,
    /// Components the subset touches, most involved first.
    pub components: Vec<String>,
    /// Equations minus unknowns of the subset (negative: underdetermined).
    pub excess: i64,
    pub truncated: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct StructuralDiag {
    /// model | index reduction | simulation | initialization
    pub problem: String,
    pub equations: usize,
    pub unknowns: usize,
    pub underdetermined: Vec<Subset>,
    pub overdetermined: Vec<Subset>,
    pub hints: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ResidualInfo {
    pub equation: String,
    pub value: f64,
    pub source: Option<SourceRef>,
}

#[derive(Clone, Debug, Serialize)]
pub struct UpdateInfo {
    pub var: String,
    /// Value at the last iterate.
    pub value: f64,
    /// Last Newton update (after damping).
    pub step: f64,
    /// `|step| / (1 + |value|)`.
    pub relative: f64,
    pub start: Option<f64>,
}

/// A nonlinear block that did not converge, re-examined at the point where it failed.
#[derive(Clone, Debug, Serialize)]
pub struct SolverFailure {
    /// initialization | simulation | event
    pub phase: String,
    pub time: f64,
    /// Index of the block in the initialization (phase `initialization`) or simulation
    /// block list.
    pub block: Option<usize>,
    pub linear: bool,
    pub tears: Vec<String>,
    pub inner: Vec<String>,
    /// Largest absolute residual the solver reported.
    pub residual: f64,
    /// Damped Newton iterations of the diagonstic re-run from the same entry point.
    pub iterations: usize,
    /// Largest absolute residual after each iteration of the re-run.
    pub history: Vec<f64>,
    pub residual_norm: f64,
    /// Residuals at the last iterate, largest first.
    pub residuals: Vec<ResidualInfo>,
    /// Tear variables by size of their last Newton update, largest first.
    pub updates: Vec<UpdateInfo>,
    pub jacobian_singular: bool,
    pub hints: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InitUnknown {
    pub name: String,
    /// state | derivative | algebraic | discrete | parameter
    pub kind: &'static str,
    /// fixed (start value is an initial equation) | defaulted (start value added as an
    /// equation to make the system square) | guess (iteration start of a nonlinear
    /// block) | computed (explicit; the start value does not matter)
    pub start_use: &'static str,
    pub start_value: Option<f64>,
    pub start: StartInfo,
    /// Value after initialization (or at the failure).
    pub value: Option<f64>,
    pub block: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InitBlockInfo {
    pub index: usize,
    /// linear | nonlinear
    pub kind: &'static str,
    pub tears: Vec<String>,
    pub inner: Vec<String>,
    pub residuals: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct InitReport {
    pub model: String,
    pub ok: bool,
    /// flatten | translation | initialization | ok
    pub stage: &'static str,
    pub error: Option<String>,
    pub structural: Option<StructuralDiag>,
    pub unknowns: Vec<InitUnknown>,
    /// Unknowns whose start value matters (fixed, defaulted or guess).
    pub relevant: usize,
    pub assignments: usize,
    pub loops: Vec<InitBlockInfo>,
    pub failure: Option<SolverFailure>,
    /// Variables that are not finite after initialization.
    pub non_finite: Vec<String>,
    pub redundant: Vec<String>,
    pub unconnected: Vec<Unconnected>,
    pub warnings: Vec<String>,
}

// ---------------------------------------------------------------- source positions

/// Source id -> document URI of the loaded documents.
pub fn file_map(lib: &Library) -> FxHashMap<u64, String> {
    lib.docs
        .iter()
        .map(|d| (rumoca_core::source_id_for_name(&d.uri).0, d.uri.clone()))
        .collect()
}

fn loc_of(files: &FxHashMap<u64, String>, l: &SrcLoc) -> Option<Loc> {
    files.get(&l.source).map(|f| Loc {
        file: f.clone(),
        line: l.line,
        col: l.col,
    })
}

/// `a.b[2].c` -> `a.b[2]` (none for a top-level name).
pub fn parent_path(name: &str) -> Option<&str> {
    let mut depth = 0i32;
    for (i, c) in name.char_indices().rev() {
        match c {
            ']' | ')' => depth += 1,
            '[' | '(' => depth -= 1,
            '.' if depth == 0 => return Some(&name[..i]),
            _ => {}
        }
    }
    None
}

/// Component path named by an origin text.
fn component_of(origin: &str) -> Option<String> {
    for p in ["declaration of ", "start value of ", "binding of "] {
        if let Some(v) = origin.strip_prefix(p) {
            return parent_path(v).map(str::to_string);
        }
    }
    origin.rsplit_once(" in ").map(|(_, c)| c.to_string())
}

/// Shared context of source references: positions and texts.
struct Sources<'a> {
    files: &'a FxHashMap<u64, String>,
    origin_locs: &'a FxHashMap<u32, SrcLoc>,
    origins: &'a [String],
}

impl Sources<'_> {
    fn origin(&self, o: u32) -> String {
        self.origins.get(o as usize).cloned().unwrap_or_default()
    }
    fn loc(&self, o: u32) -> Option<Loc> {
        self.origin_locs.get(&o).and_then(|l| loc_of(self.files, l))
    }
    /// A source reference for an equation `text` with origin `o`; `names` are the
    /// variables in it (for connection equations, whose origin names no component).
    fn reference(&self, text: String, o: u32, names: &[String], initial: bool) -> SourceRef {
        let origin = self.origin(o);
        let component = component_of(&origin).or_else(|| {
            if origin.contains("connection") || origin.contains("flow") {
                let mut cs: Vec<&str> = names.iter().filter_map(|n| parent_path(n)).collect();
                cs.sort_unstable();
                cs.dedup();
                (!cs.is_empty()).then(|| cs.join(", "))
            } else {
                None
            }
        });
        SourceRef {
            text,
            now: None,
            note: None,
            origin,
            component,
            loc: self.loc(o),
            initial,
        }
    }
}

fn clip(mut s: String, n: usize) -> String {
    if s.len() > n {
        let mut k = n;
        while !s.is_char_boundary(k) {
            k -= 1;
        }
        s.truncate(k);
        s.push('…');
    }
    s
}

// ---------------------------------------------------------------- debugger

/// A model translated with the backend trace on, ready to explain its variables and
/// equations.
pub struct Debugger {
    pub sys: System,
    pub trace: Trace,
    pub report: StructureReport,
    files: FxHashMap<u64, String>,
    origin_locs: FxHashMap<u32, SrcLoc>,
    start_src: FxHashMap<VarId, StartSrc>,
    by_name: FxHashMap<String, VarId>,
    /// var -> (block, equation or none for a tear, role)
    computed_by: FxHashMap<VarId, (usize, Option<usize>, &'static str)>,
    init_by: FxHashMap<VarId, (usize, Option<usize>, &'static str)>,
    eq_block: Vec<Option<(usize, Option<VarId>)>>,
    pub unconnected: Vec<Unconnected>,
}

/// Why a model could not be prepared for debugging.
pub struct Failure {
    pub stage: &'static str,
    pub error: String,
    pub structural: Option<StructuralDiag>,
    pub unconnected: Vec<Unconnected>,
}

fn block_index(blocks: &[TraceBlock]) -> FxHashMap<VarId, (usize, Option<usize>, &'static str)> {
    let mut m = FxHashMap::default();
    for (i, b) in blocks.iter().enumerate() {
        let lp = !b.tears.is_empty() || !b.residual_eqs.is_empty();
        for &(e, v) in &b.solved {
            m.insert(v, (i, Some(e), if lp { "inner" } else { "assigned" }));
        }
        for &t in &b.tears {
            m.insert(t, (i, None, "tear"));
        }
    }
    m
}

impl Debugger {
    /// Flatten and translate `model` with the trace on.
    pub fn new(
        lib: &Library,
        model: &str,
        bopts: &backend::Options,
    ) -> Result<Debugger, Box<Failure>> {
        let flat = lib.flatten(model).map_err(|e| {
            Box::new(Failure {
                stage: "flatten",
                error: format!("{e:#}"),
                structural: None,
                unconnected: Vec::new(),
            })
        })?;
        Self::from_flat(lib, flat, bopts)
    }

    pub fn from_flat(
        lib: &Library,
        flat: FlatModel,
        bopts: &backend::Options,
    ) -> Result<Debugger, Box<Failure>> {
        let files = file_map(lib);
        let unconnected = unconnected_connectors(&flat);
        let origin_locs = flat.origin_locs.clone();
        let start_src = flat.start_src.clone();
        let origins = flat.origins.clone();
        let mut o = bopts.clone();
        o.trace = true;
        let mut sys = match backend::compile_traced(flat, &o) {
            Ok(s) => s,
            Err((e, trace)) => {
                let src = Sources {
                    files: &files,
                    origin_locs: &origin_locs,
                    origins: &origins,
                };
                let mut structural = trace
                    .and_then(|t| t.singular)
                    .map(|s| diagnose_singular(&s, &src));
                if let Some(d) = &mut structural {
                    let mut named = FxHashSet::default();
                    for sub in &d.underdetermined {
                        for v in &sub.unmatched {
                            for u in &unconnected {
                                if v.starts_with(&format!("{}.", u.connector))
                                    || v.starts_with(&format!("{}.", u.component))
                                {
                                    if named.insert(u.component.clone()) {
                                        d.hints.push(format!(
                                            "{v} belongs to {}, which has unconnected connectors: {}",
                                            u.component,
                                            u.reason
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
                return Err(Box::new(Failure {
                    stage: "translation",
                    error: format!("{e:#}"),
                    structural,
                    unconnected,
                }));
            }
        };
        let trace = sys.trace.take().unwrap_or_default();
        let report = structure::report(&mut sys, &trace);
        let by_name = sys
            .vars
            .iter()
            .enumerate()
            .map(|(i, v)| (v.name.clone(), i as VarId))
            .collect();
        let computed_by = block_index(&trace.blocks);
        let init_by = block_index(&trace.init_blocks);
        let mut eq_block = vec![None; trace.eqs.len()];
        for (i, b) in trace.blocks.iter().enumerate() {
            for &(e, v) in &b.solved {
                eq_block[e] = Some((i, Some(v)));
            }
            for &e in &b.residual_eqs {
                eq_block[e] = Some((i, None));
            }
        }
        Ok(Debugger {
            sys,
            trace,
            report,
            files,
            origin_locs,
            start_src,
            by_name,
            computed_by,
            init_by,
            eq_block,
            unconnected,
        })
    }

    fn sources(&self) -> Sources<'_> {
        Sources {
            files: &self.files,
            origin_locs: &self.origin_locs,
            origins: &self.sys.origins,
        }
    }

    fn name(&self, v: VarId) -> &str {
        self.sys.name_of(v)
    }

    fn text(&self, l: ExprId, r: ExprId) -> String {
        clip(format!("{} = {}", self.sys.show(l), self.sys.show(r)), 600)
    }

    fn names_in(&self, l: ExprId, r: ExprId) -> Vec<String> {
        let mut v: Vec<String> = self
            .sys
            .arena
            .vars_in(&[l, r])
            .into_iter()
            .map(|v| self.name(v).to_string())
            .collect();
        v.sort();
        v
    }

    /// Source reference of flat equation `src`.
    fn source_ref(&self, src: u32) -> Option<SourceRef> {
        let s = self.trace.sources.get(src as usize)?;
        let names = self.names_in(s.lhs, s.rhs);
        Some(
            self.sources()
                .reference(self.text(s.lhs, s.rhs), s.origin, &names, s.initial),
        )
    }

    /// Look a variable up by name; `der(x)` names the derivative of `x`.
    pub fn var_id(&self, name: &str) -> Option<VarId> {
        let name = name.trim();
        if let Some(&v) = self.by_name.get(name) {
            return Some(v);
        }
        let inner = name.strip_prefix("der(")?.strip_suffix(')')?;
        let x = *self.by_name.get(inner.trim())?;
        self.sys.der_of[x as usize]
    }

    fn alias_of(&self, v: VarId) -> Option<&backend::Alias> {
        self.sys.aliases.iter().find(|a| a.var == v)
    }

    /// The transformations from the source equation to equation `e` of the simulation
    /// system (without the block step).
    fn eq_chain(&self, e: usize, eqs: &[TraceEq], steps: &mut Vec<Step>) {
        let mut chain = vec![e];
        while let Some(p) = eqs[*chain.last().unwrap()].parent {
            chain.push(p);
        }
        chain.reverse();
        let root = &eqs[chain[0]];
        if root.src != NO_SRC
            && let Some(sr) = self.source_ref(root.src)
        {
            let s = &self.trace.sources[root.src as usize];
            let src_text = sr.text.clone();
            let mut st = Step::new(
                "source",
                format!(
                    "{} as written{}",
                    if sr.initial {
                        "initial equation"
                    } else {
                        "equation"
                    },
                    sr.component
                        .as_ref()
                        .map(|c| format!(" in {c}"))
                        .unwrap_or_default()
                ),
            )
            .eq(src_text.clone());
            st.source = Some(sr);
            steps.push(st);
            // Alias substitution: the variables of the source equation that alias
            // elimination replaced.
            let mut vars: Vec<VarId> = self
                .sys
                .arena
                .vars_in(&[s.lhs, s.rhs])
                .into_iter()
                .collect();
            vars.sort_by(|a, b| self.name(*a).cmp(self.name(*b)));
            let mut details = Vec::new();
            for v in vars {
                if let Some(a) = self.alias_of(v) {
                    details.push(format!("{} → {}", self.name(v), self.sys.show(a.expr)));
                }
            }
            let now = self.text(root.lhs, root.rhs);
            if !details.is_empty() || now != src_text {
                let mut st = Step::new(
                    "alias",
                    if details.is_empty() {
                        "simplified (parameter folding, substitution)".to_string()
                    } else {
                        format!(
                            "alias substitution: {} variable{} replaced by {}",
                            details.len(),
                            if details.len() == 1 { "" } else { "s" },
                            if details.len() == 1 {
                                "its representative"
                            } else {
                                "their representatives"
                            }
                        )
                    },
                )
                .eq(now);
                st.details = details;
                steps.push(st);
            }
        } else if root.src == NO_SRC {
            let o = self.sources().origin(root.origin);
            steps.push(
                Step::new("source", format!("generated: {o}")).eq(self.text(root.lhs, root.rhs)),
            );
        }
        for (k, &d) in chain.iter().enumerate().skip(1) {
            let q = &eqs[d];
            let mut st = Step::new(
                "differentiated",
                format!(
                    "differentiated with respect to time ({}{} derivative) by index reduction",
                    k,
                    match k {
                        1 => "st",
                        2 => "nd",
                        3 => "rd",
                        _ => "th",
                    }
                ),
            )
            .eq(self.text(q.lhs, q.rhs));
            if k == 1 && chain.len() > 1 && self.trace.state_change_eqs > 0 {
                st.details
                    .push("(Pantelides, or a state change for stateSelect = prefer/always)".into());
            }
            steps.push(st);
        }
        if std::ptr::eq(eqs, self.trace.eqs.as_slice()) {
            let mut done = FxHashSet::default();
            for &c in &chain {
                self.set_steps(c, steps, &mut done);
            }
        }
    }

    /// The state set (dummy-derivative selection) that equation `e` constrains.
    fn set_steps(&self, e: usize, steps: &mut Vec<Step>, done: &mut FxHashSet<usize>) {
        for s in &self.report.state_sets {
            if s.eqs.contains(&e) && done.insert(s.index) {
                let mut st = Step::new(
                    "state-selection",
                    format!(
                        "constraint of state set {} (dummy derivatives, level {})",
                        s.index, s.level
                    ),
                );
                st.details.push(s.reason.clone());
                st.details.push(format!(
                    "states: {}",
                    s.candidates
                        .iter()
                        .filter(|c| c.chosen == "state")
                        .map(|c| c.var.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                st.details.push(format!(
                    "dummy derivatives: {}",
                    s.candidates
                        .iter()
                        .filter(|c| c.chosen == "dummy")
                        .map(|c| c.derivative.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                steps.push(st);
            }
        }
    }

    /// How block `bi` of `blocks` solves variable `v` (or, with `v = None`, uses equation
    /// `e` as a residual).
    fn block_step(&self, init: bool, bi: usize, v: Option<VarId>, e: Option<usize>) -> Step {
        let blocks = if init {
            &self.sys.init_blocks
        } else {
            &self.sys.blocks
        };
        let tb = if init {
            &self.trace.init_blocks[bi]
        } else {
            &self.trace.blocks[bi]
        };
        let which = if init {
            "initialization block"
        } else {
            "block"
        };
        match &blocks[bi] {
            Block::Assign { var, expr } => Step::new(
                "assign",
                format!("solved symbolically for {} ({which} {bi})", self.name(*var)),
            )
            .eq(clip(
                format!("{} := {}", self.name(*var), self.sys.show(*expr)),
                600,
            )),
            Block::Solve {
                tears,
                inner,
                residuals,
                linear,
            } => {
                let kind = if *linear { "linear" } else { "nonlinear" };
                let tn: Vec<&str> = tears.iter().map(|&t| self.name(t)).collect();
                let size = tears.len() + inner.len();
                if let Some(v) = v
                    && let Some((_, x)) = inner.iter().find(|(w, _)| *w == v)
                {
                    let mut st = Step::new(
                        "inner",
                        format!(
                            "inner equation of a torn {kind} loop ({which} {bi}, {size} unknowns): solved explicitly for {} from the tear variables",
                            self.name(v)
                        ),
                    )
                    .eq(clip(format!("{} := {}", self.name(v), self.sys.show(*x)), 600));
                    st.details
                        .push(format!("tear variables: {}", tn.join(", ")));
                    return st;
                }
                if size == 1 {
                    let mut st = Step::new(
                        "newton",
                        format!(
                            "no explicit solution for {}: solved numerically ({kind}, {which} {bi})",
                            tn.join(", ")
                        ),
                    )
                    .eq(format!("0 = {}", clip(self.sys.show(residuals[0]), 600)));
                    st.details.push(if *linear {
                        "linear in the unknown, but with a coefficient that may vanish or a branch on it".into()
                    } else {
                        "the unknown appears nonlinearly (or inside a branch); Newton iteration".into()
                    });
                    return st;
                }
                let title = match v {
                    Some(v) if tears.contains(&v) => format!(
                        "tear variable of a {kind} loop ({which} {bi}, {size} unknowns, {} tear{}): Newton iterates on it until the residual equations vanish",
                        tears.len(),
                        if tears.len() == 1 { "" } else { "s" }
                    ),
                    _ => format!(
                        "residual equation of a torn {kind} loop ({which} {bi}, {size} unknowns): must vanish at the solution"
                    ),
                };
                let mut st = Step::new(if v.is_some() { "tear" } else { "residual" }, title);
                if let Some(e) = e {
                    let eqs = if init {
                        &self.trace.init_eqs
                    } else {
                        &self.trace.eqs
                    };
                    st.equation = Some(self.text(eqs[e].lhs, eqs[e].rhs));
                }
                st.details
                    .push(format!("tear variables: {}", tn.join(", ")));
                if !inner.is_empty() {
                    st.details.push(format!(
                        "inner (explicit) variables: {}",
                        inner
                            .iter()
                            .map(|(w, _)| self.name(*w))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
                let eqs = if init {
                    &self.trace.init_eqs
                } else {
                    &self.trace.eqs
                };
                for &r in &tb.residual_eqs {
                    st.details.push(format!(
                        "residual: {}",
                        clip(self.text(eqs[r].lhs, eqs[r].rhs), 200)
                    ));
                }
                st
            }
        }
    }

    pub fn start_info(&self, v: VarId) -> StartInfo {
        let var = &self.sys.vars[v as usize];
        let member = self
            .trace
            .start_from
            .iter()
            .rev()
            .find(|(r, _)| *r == v)
            .map(|&(_, m)| m);
        let src = self.start_src.get(&member.unwrap_or(v));
        StartInfo {
            expr: var.start.map(|s| clip(self.sys.show(s), 200)),
            fixed: var.fixed,
            written_in: src.map(|s| s.scope.clone()),
            loc: src
                .and_then(|s| s.loc.as_ref())
                .and_then(|l| loc_of(&self.files, l)),
            from_alias: member.map(|m| self.name(m).to_string()),
        }
    }

    /// The alias equations linking `v` to `to` (breadth-first over the eliminated
    /// equations).
    fn alias_path(&self, v: VarId, to: Option<VarId>) -> Vec<usize> {
        let mut adj: FxHashMap<VarId, Vec<usize>> = FxHashMap::default();
        for (i, a) in self.trace.alias_eqs.iter().enumerate() {
            adj.entry(a.a).or_default().push(i);
            if let Some(b) = a.b {
                adj.entry(b).or_default().push(i);
            }
        }
        let mut prev: FxHashMap<VarId, (VarId, usize)> = FxHashMap::default();
        let mut seen: FxHashSet<VarId> = [v].into_iter().collect();
        let mut queue = std::collections::VecDeque::from([v]);
        let mut end = None;
        while let Some(x) = queue.pop_front() {
            if Some(x) == to && x != v {
                end = Some(x);
                break;
            }
            for &i in adj.get(&x).map(|v| v.as_slice()).unwrap_or(&[]) {
                let a = &self.trace.alias_eqs[i];
                let Some(y) = (if a.a == x { a.b } else { Some(a.a) }) else {
                    // `x = f(parameters)`: the end when no target is given.
                    if to.is_none() {
                        let mut path = vec![i];
                        let mut c = x;
                        while let Some(&(p, k)) = prev.get(&c) {
                            path.push(k);
                            c = p;
                        }
                        path.reverse();
                        return path;
                    }
                    continue;
                };
                if seen.insert(y) {
                    prev.insert(y, (x, i));
                    queue.push_back(y);
                }
            }
        }
        let Some(mut c) = end else { return Vec::new() };
        let mut path = Vec::new();
        while let Some(&(p, k)) = prev.get(&c) {
            path.push(k);
            c = p;
        }
        path.reverse();
        path
    }

    /// The transformation chain of variable `name`.
    pub fn trace_var(&self, name: &str) -> Result<VarTrace, String> {
        let v = self
            .var_id(name)
            .ok_or_else(|| format!("no variable '{name}' in {}", self.sys.name))?;
        let mut out = self.trace_var_id(v, 0);
        out.name = self.name(v).to_string();
        Ok(out)
    }

    fn trace_var_id(&self, v: VarId, depth: usize) -> VarTrace {
        let var = &self.sys.vars[v as usize];
        let info = self.report.variables.iter().find(|i| i.name == var.name);
        let role = info.map(|i| i.role.to_string()).unwrap_or_else(|| {
            if self.sys.der_of.iter().any(|d| *d == Some(v)) {
                "derivative".into()
            } else {
                "unused".into()
            }
        });
        let mut t = VarTrace {
            name: var.name.clone(),
            role: role.clone(),
            unit: var.unit.clone(),
            description: var.description.clone(),
            equation: None,
            block: None,
            block_kind: None,
            tearing_role: None,
            alias_of: None,
            start: self.start_info(v),
            steps: Vec::new(),
        };
        let steps = &mut t.steps;
        match role.as_str() {
            "parameter" | "input" => {
                let mut st = Step::new(
                    "parameter",
                    if role == "input" {
                        "top-level input: its value is given (start value until set)"
                    } else {
                        "parameter or constant: fixed before the simulation"
                    },
                );
                if let Some(b) = var.binding.or(var.start) {
                    st.equation = Some(clip(format!("{} = {}", var.name, self.sys.show(b)), 600));
                }
                steps.push(st);
                return t;
            }
            "alias" => {
                let a = self.alias_of(v).cloned();
                let rep = match a.as_ref().map(|a| &a.kind) {
                    Some(AliasKind::Var { of, sign }) => {
                        t.alias_of = Some(format!(
                            "{}{}",
                            if *sign < 0.0 { "-" } else { "" },
                            self.name(*of)
                        ));
                        Some(*of)
                    }
                    _ => None,
                };
                let path = self.alias_path(v, rep);
                for i in &path {
                    let ae = &self.trace.alias_eqs[*i];
                    let mut st = Step::new(
                        "alias",
                        match ae.b {
                            Some(b) => format!(
                                "eliminated by alias elimination: relates {} and {}",
                                self.name(ae.a),
                                self.name(b)
                            ),
                            None => format!(
                                "eliminated by alias elimination: {} is fixed by parameters",
                                self.name(ae.a)
                            ),
                        },
                    )
                    .eq(self.text(ae.lhs, ae.rhs));
                    st.source = self.source_ref(ae.src);
                    steps.push(st);
                }
                if let Some(a) = &a {
                    steps.push(
                        Step::new(
                            "substituted",
                            "replaced everywhere by its representative; its value is recovered from it in the results",
                        )
                        .eq(clip(format!("{} = {}", var.name, self.sys.show(a.expr)), 600)),
                    );
                }
                if let Some(r) = rep
                    && depth < 2
                {
                    let rt = self.trace_var_id(r, depth + 1);
                    steps.push(Step::new(
                        "representative",
                        format!("{} ({}) is computed as follows", rt.name, rt.role),
                    ));
                    t.equation = rt.equation;
                    t.block = rt.block;
                    t.block_kind = rt.block_kind;
                    t.tearing_role = rt.tearing_role;
                    t.steps.extend(rt.steps);
                }
                return t;
            }
            "state" => {
                let reason = self
                    .report
                    .state_reasons
                    .iter()
                    .find(|r| r.state == var.name)
                    .map(|r| r.reason.clone())
                    .unwrap_or_default();
                let mut st = Step::new(
                    "state",
                    "state: integrated by the solver from its derivative",
                );
                st.details.push(reason);
                steps.push(st);
                if let Some(d) = self.sys.der_of[v as usize]
                    && depth < 2
                {
                    let dt = self.trace_var_id(d, depth + 1);
                    steps.push(Step::new(
                        "representative",
                        format!("its derivative {} is computed as follows", dt.name),
                    ));
                    t.equation = dt.equation;
                    t.block = dt.block;
                    t.block_kind = dt.block_kind;
                    t.tearing_role = dt.tearing_role;
                    t.steps.extend(dt.steps);
                }
                self.init_steps(v, &mut t.steps);
                return t;
            }
            _ => {}
        }
        if let Some(&(bi, e, r)) = self.computed_by.get(&v) {
            t.block = Some(bi);
            t.tearing_role = Some(r);
            t.block_kind = self.report.blocks.get(bi).map(|b| b.kind);
            let eq = e.or_else(|| self.trace.blocks[bi].residual_eqs.first().copied());
            t.equation = eq;
            if let Some(e) = e {
                self.eq_chain(e, &self.trace.eqs, steps);
            } else {
                // A tear: show the residual equations' chains.
                for &e in &self.trace.blocks[bi].residual_eqs {
                    self.eq_chain(e, &self.trace.eqs, steps);
                }
            }
            if role == "dummy derivative" {
                let set = self
                    .report
                    .state_sets
                    .iter()
                    .find(|s| s.candidates.iter().any(|c| c.derivative == var.name));
                let mut st = Step::new(
                    "state-selection",
                    "dummy derivative: index reduction made this derivative an algebraic unknown",
                );
                if let Some(s) = set {
                    st.details
                        .push(format!("state set {}: {}", s.index, s.reason));
                }
                steps.push(st);
            }
            steps.push(self.block_step(false, bi, Some(v), e));
        } else if self.sys.discretes.contains(&v) {
            steps.push(Step::new(
                "discrete",
                "discrete variable: assigned in when-clauses (or held between events)",
            ));
        } else {
            steps.push(Step::new(
                "unused",
                "not part of the simulation system (not used by any equation)",
            ));
        }
        self.init_steps(v, &mut t.steps);
        t
    }

    fn init_steps(&self, v: VarId, steps: &mut Vec<Step>) {
        let Some(&(bi, e, _)) = self.init_by.get(&v) else {
            return;
        };
        let fixed = self.trace.init_fixed.contains(&v);
        let defaulted = self.trace.init_defaulted.contains(&v);
        let mut st = self.block_step(true, bi, Some(v), e);
        st.kind = "initialization";
        st.title = format!(
            "at initialization: {}{}",
            st.title,
            if fixed {
                " — from its fixed start value"
            } else if defaulted {
                " — from its start value (added to make the initial system square)"
            } else {
                ""
            }
        );
        if let Some(e) = e {
            let q = &self.trace.init_eqs[e];
            if q.src != NO_SRC
                && let Some(sr) = self.source_ref(q.src)
                && sr.initial
            {
                st.source = Some(sr);
            }
        }
        steps.push(st);
    }

    /// The transformation chain of equation `id` of the simulation system (the ids of
    /// the structure report).
    pub fn trace_eq(&self, id: usize) -> Result<EqTrace, String> {
        let q = self
            .trace
            .eqs
            .get(id)
            .ok_or_else(|| format!("no equation {id}"))?;
        let mut steps = Vec::new();
        self.eq_chain(id, &self.trace.eqs, &mut steps);
        let (block, solved_for) = match self.eq_block[id] {
            Some((bi, v)) => {
                steps.push(self.block_step(false, bi, v, Some(id)));
                (Some(bi), v.map(|v| self.name(v).to_string()))
            }
            None => (None, None),
        };
        Ok(EqTrace {
            id,
            text: self.text(q.lhs, q.rhs),
            block,
            solved_for,
            steps,
        })
    }
}

// ---------------------------------------------------------------- structural diagnosis

/// Locate the over- and underdetermined parts of a system whose matching failed.
fn diagnose_singular(s: &Singular, src: &Sources) -> StructuralDiag {
    use crate::backend::graph::{Bipartite, NONE, hopcroft_karp};
    let ne = s.eqs.len();
    let nv = s.vars.len();
    let adj: Vec<Vec<u32>> = s.eqs.iter().map(|e| e.unknowns.clone()).collect();
    let (eq_match, var_match) = hopcroft_karp(&Bipartite {
        n_eq: ne,
        n_var: nv,
        adj: &adj,
    });
    let mut var_eqs: Vec<Vec<u32>> = vec![Vec::new(); nv];
    for (e, row) in adj.iter().enumerate() {
        for &v in row {
            var_eqs[v as usize].push(e as u32);
        }
    }
    const MAX_EQS: usize = 60;
    const MAX_SUBSETS: usize = 12;
    let eq_refs = |e: usize| {
        let q = &s.eqs[e];
        let names: Vec<String> = q
            .unknowns
            .iter()
            .map(|&v| s.vars[v as usize].clone())
            .collect();
        let (text, now) = match &q.source_text {
            Some(t) => (clip(t.clone(), 400), Some(clip(q.text.clone(), 400))),
            None => (clip(q.text.clone(), 400), None),
        };
        let mut r = src.reference(text, q.origin_id, &names, false);
        r.now = now;
        r.origin = q.origin.clone();
        if r.component.is_none() {
            r.component = component_of(&q.origin);
        }
        let mut out = vec![r];
        for (t, o) in &q.related {
            let mut r = src.reference(clip(t.clone(), 400), *o, &names, false);
            r.note =
                Some("used by alias elimination to remove a variable of the equation above".into());
            out.push(r);
        }
        out
    };
    let components = |eqs: &[usize], vars: &[usize]| -> Vec<String> {
        let mut count: FxHashMap<String, usize> = FxHashMap::default();
        for &v in vars {
            if let Some(p) = parent_path(&s.vars[v]) {
                *count.entry(p.to_string()).or_default() += 1;
            }
        }
        for &e in eqs {
            if let Some(c) = component_of(&s.eqs[e].origin) {
                *count.entry(c).or_default() += 1;
            }
        }
        let mut v: Vec<(String, usize)> = count.into_iter().collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().take(12).map(|(c, _)| c).collect()
    };
    // Underdetermined: from each unmatched unknown, the equations it is in, their matched
    // unknowns, their equations, ... (more unknowns than equations).
    let mut under = Vec::new();
    let mut seen_v = vec![false; nv];
    for v0 in 0..nv {
        if var_match[v0] != NONE || seen_v[v0] {
            continue;
        }
        if under.len() >= MAX_SUBSETS {
            break;
        }
        let mut vars = vec![v0];
        let mut eqs: Vec<usize> = Vec::new();
        let mut seen_e = FxHashSet::default();
        seen_v[v0] = true;
        let mut k = 0;
        let mut truncated = false;
        while k < vars.len() {
            let v = vars[k];
            k += 1;
            for &e in &var_eqs[v] {
                if !seen_e.insert(e) {
                    continue;
                }
                if eqs.len() >= MAX_EQS {
                    truncated = true;
                    break;
                }
                eqs.push(e as usize);
                let m = eq_match[e as usize];
                if m != NONE && !seen_v[m as usize] {
                    seen_v[m as usize] = true;
                    vars.push(m as usize);
                }
            }
        }
        // Other unmatched unknowns reached through the same equations belong here too.
        let mut unmatched: Vec<String> = vec![s.vars[v0].clone()];
        for v in 0..nv {
            if v != v0
                && var_match[v] == NONE
                && !seen_v[v]
                && var_eqs[v].iter().any(|e| seen_e.contains(e))
            {
                seen_v[v] = true;
                vars.push(v);
                unmatched.push(s.vars[v].clone());
            }
        }
        under.push(Subset {
            excess: eqs.len() as i64 - vars.len() as i64,
            components: components(&eqs, &vars),
            unmatched,
            equations: eqs.iter().flat_map(|&e| eq_refs(e)).collect(),
            variables: vars.iter().map(|&v| s.vars[v].clone()).collect(),
            truncated,
        });
    }
    // Overdetermined: from each unmatched equation, its unknowns, their matched
    // equations, their unknowns, ... (more equations than unknowns).
    let mut over = Vec::new();
    let mut seen_e = vec![false; ne];
    for e0 in 0..ne {
        if eq_match[e0] != NONE || seen_e[e0] {
            continue;
        }
        if over.len() >= MAX_SUBSETS {
            break;
        }
        let mut eqs = vec![e0];
        seen_e[e0] = true;
        let mut vars: Vec<usize> = Vec::new();
        let mut seen_v = FxHashSet::default();
        let mut k = 0;
        let mut truncated = false;
        while k < eqs.len() {
            let e = eqs[k];
            k += 1;
            for &v in &adj[e] {
                if !seen_v.insert(v) {
                    continue;
                }
                vars.push(v as usize);
                let m = var_match[v as usize];
                if m != NONE && !seen_e[m as usize] {
                    if eqs.len() >= MAX_EQS {
                        truncated = true;
                        continue;
                    }
                    seen_e[m as usize] = true;
                    eqs.push(m as usize);
                }
            }
        }
        let mut extra = vec![e0];
        for e in 0..ne {
            if e != e0
                && eq_match[e] == NONE
                && !seen_e[e]
                && adj[e].iter().any(|v| seen_v.contains(v))
            {
                seen_e[e] = true;
                eqs.push(e);
                extra.push(e);
            }
        }
        over.push(Subset {
            excess: eqs.len() as i64 - vars.len() as i64,
            components: components(&eqs, &vars),
            unmatched: extra
                .iter()
                .map(|&e| {
                    let q = &s.eqs[e];
                    clip(
                        match &q.source_text {
                            Some(t) => format!("{t} (after substitution: {})", q.text),
                            None => q.text.clone(),
                        },
                        300,
                    )
                })
                .collect(),
            equations: eqs.iter().flat_map(|&e| eq_refs(e)).collect(),
            variables: vars.iter().map(|&v| s.vars[v].clone()).collect(),
            truncated,
        });
    }
    let mut hints = Vec::new();
    for sub in &under {
        if sub.equations.is_empty() {
            hints.push(format!(
                "{} appear{} in no equation: add an equation (or a binding) for {}",
                sub.unmatched.join(", "),
                if sub.unmatched.len() == 1 { "s" } else { "" },
                if sub.unmatched.len() == 1 {
                    "it"
                } else {
                    "them"
                }
            ));
        }
    }
    StructuralDiag {
        problem: s.problem.clone(),
        equations: ne,
        unknowns: nv,
        underdetermined: under,
        overdetermined: over,
        hints,
    }
}

// ---------------------------------------------------------------- unconnected connectors

/// Physical connectors (with flow variables) of components that nothing connects.
///
/// An unconnected flow variable is set to zero (MLS 9.2), which is often intended (a free
/// flange, an adiabatic heat port, an open output terminal, a lone ground), so only
/// components placed directly in the model are judged, and a connector is a warning only
/// when its component has two or more physical connectors and none is connected (it
/// then does nothing), or when it is the open end of a two-pin electrical component (no
/// current flows through it). Everything else is a note: open connectors of nested
/// components are the library's business (disabled thermal ports of machines, unused
/// subcircuit nodes). The model's own (top-level) connectors are never reported.
pub fn unconnected_connectors(flat: &FlatModel) -> Vec<Unconnected> {
    let marks: FxHashSet<u32> = flat
        .origins
        .iter()
        .enumerate()
        .filter(|(_, o)| o.as_str() == "unconnected flow variable")
        .map(|(i, _)| i as u32)
        .collect();
    if marks.is_empty() {
        return Vec::new();
    }
    // Flows in a connection equation: a connector of the model itself (a connector
    // instance placed in it, connected inside) is connected from the outside only.
    let connected: FxHashSet<VarId> = {
        let roots: Vec<ExprId> = flat
            .equations
            .iter()
            .filter(|e| {
                flat.origins
                    .get(e.origin as usize)
                    .is_some_and(|o| o == "connection")
            })
            .flat_map(|e| [e.lhs, e.rhs])
            .collect();
        flat.arena.vars_in(&roots)
    };
    let mut open: FxHashMap<&str, Vec<&str>> = FxHashMap::default();
    for e in &flat.equations {
        if !marks.contains(&e.origin) {
            continue;
        }
        if let crate::expr::Node::Var(v) = flat.arena.node(e.lhs) {
            if connected.contains(&v) {
                continue;
            }
            let name = flat.vars[v as usize].name.as_str();
            if let Some(c) = parent_path(name)
                && parent_path(c).is_some()
            {
                open.entry(c).or_default().push(name);
            }
        }
    }
    // Physical connectors of the components placed in the model: their first-level
    // children holding flow variables (a multi-phase plug counts once).
    let first = |name: &str| -> Option<(String, String)> {
        let mut parts = Vec::new();
        let mut rest = name;
        while let Some(p) = parent_path(rest) {
            parts.push(&rest[p.len() + 1..]);
            rest = p;
        }
        parts.push(rest);
        parts.reverse();
        (parts.len() >= 3).then(|| (parts[0].to_string(), format!("{}.{}", parts[0], parts[1])))
    };
    let mut conns: FxHashMap<String, FxHashSet<String>> = FxHashMap::default();
    let mut members: FxHashMap<&str, Vec<&str>> = FxHashMap::default();
    for v in &flat.vars {
        if let Some(c) = parent_path(&v.name) {
            members.entry(c).or_default().push(&v.name);
            if v.is_flow
                && let Some((comp, conn)) = first(&v.name)
            {
                conns.entry(comp).or_default().insert(conn);
            }
        }
    }
    let open_first: FxHashSet<String> = open
        .values()
        .flatten()
        .filter_map(|n| first(n).map(|f| f.1))
        .collect();
    let mut out = Vec::new();
    let mut keys: Vec<&&str> = open.keys().collect();
    keys.sort();
    for c in keys {
        let comp = parent_path(c).unwrap_or_default();
        let top = first(open[c][0]).map(|f| f.0).unwrap_or_default();
        let all = conns.get(&top).map(|s| s.len()).unwrap_or(1);
        let open_here = conns
            .get(&top)
            .map(|s| s.iter().filter(|k| open_first.contains(*k)).count())
            .unwrap_or(1);
        let flows: Vec<String> = open[c].iter().map(|s| s.to_string()).collect();
        // An electrical pin: a connector with `v` and `i`.
        let pin = members.get(c).is_some_and(|m| {
            m.iter().any(|n| n.ends_with(".i")) && m.iter().any(|n| n.ends_with(".v"))
        });
        let nested = parent_path(comp).is_some();
        let (level, reason) = if nested {
            (
                "note",
                format!(
                    "not connected inside {}: its flow is set to zero",
                    comp.split('.').next().unwrap_or(comp)
                ),
            )
        } else if open_here == all && all >= 2 {
            (
                "warning",
                format!(
                    "none of the {all} physical connectors of {comp} is connected: the component has no effect"
                ),
            )
        } else if open_here == all {
            (
                "note",
                format!("{comp} is not connected to anything: it has no effect"),
            )
        } else if pin && all == 2 {
            (
                "warning",
                format!(
                    "open end of {comp}: no current can flow through it (its current is set to zero)"
                ),
            )
        } else if pin {
            (
                "note",
                "open electrical pin: its current is zero (an open terminal)".to_string(),
            )
        } else {
            (
                "note",
                "not connected: its flow is set to zero (a free end)".to_string(),
            )
        };
        out.push(Unconnected {
            connector: c.to_string(),
            component: comp.to_string(),
            flows,
            level,
            reason,
        });
    }
    out
}

// ---------------------------------------------------------------- numeric diagnosis

/// Re-run a damped Newton iteration on block `bi` from the values in `sim` (the solver
/// leaves the tears at its entry point after a failure) and report the residuals at the
/// last iterate and the largest updates.
fn diagnose_newton(
    sim: &mut Simulation,
    nf: &NewtonFailure,
    starts: Option<&[f64]>,
    describe: &dyn Fn(usize, &[usize]) -> Vec<Option<SourceRef>>,
    block_of: &dyn Fn(&[VarId]) -> Option<usize>,
) -> SolverFailure {
    let m: &Model = sim.rt.m;
    let b = &m.newton[nf.block];
    let n = b.tears.len();
    let tears: Vec<VarId> = b.tears.iter().map(|&s| s as VarId).collect();
    let block = block_of(&tears);
    let residual_srcs = describe(nf.block, &[]);
    let norm_of = |rt: &crate::codegen::Runtime| {
        b.residuals
            .iter()
            .map(|&s| rt.r[s as usize].abs())
            .fold(
                0.0f64,
                |a, x| if x.is_nan() { f64::INFINITY } else { a.max(x) },
            )
    };
    let mut history = Vec::new();
    let mut last_step = vec![0.0; n];
    let mut singular = false;
    let mut iterations = 0;
    let mut jm = vec![0.0; n * n];
    let mut piv = vec![0usize; n];
    for _ in 0..50 {
        sim.rt.run(&b.body);
        let norm = norm_of(&sim.rt);
        let scale = tears
            .iter()
            .map(|&t| sim.rt.r[t as usize].abs())
            .fold(1.0, f64::max);
        if history.is_empty() {
            history.push(norm);
        }
        if norm <= 1e-10 * scale || !norm.is_finite() {
            break;
        }
        sim.rt.run(&b.jac);
        for (k, &s) in b.jac_slots.iter().enumerate() {
            jm[k] = sim.rt.r[s as usize];
        }
        let mut dx: Vec<f64> = b.residuals.iter().map(|&s| -sim.rt.r[s as usize]).collect();
        if jm.iter().any(|x| !x.is_finite()) || !crate::linalg::lu_factor(&mut jm, n, &mut piv) {
            singular = true;
            break;
        }
        crate::linalg::lu_solve(&jm, n, &piv, &mut dx);
        iterations += 1;
        let x0: Vec<f64> = tears.iter().map(|&t| sim.rt.r[t as usize]).collect();
        let mut lambda = 1.0;
        let mut new_norm;
        loop {
            for (i, &t) in tears.iter().enumerate() {
                sim.rt.r[t as usize] = x0[i] + lambda * dx[i];
            }
            sim.rt.run(&b.body);
            new_norm = norm_of(&sim.rt);
            if (new_norm.is_finite() && new_norm < norm) || lambda < 1e-4 {
                break;
            }
            lambda *= 0.5;
        }
        for i in 0..n {
            last_step[i] = lambda * dx[i];
        }
        history.push(new_norm);
        let step = (0..n)
            .map(|i| last_step[i].abs() / (1.0 + x0[i].abs()))
            .fold(0.0, f64::max);
        if step < 1e-14 {
            break;
        }
    }
    sim.rt.run(&b.body);
    let norm = norm_of(&sim.rt);
    let mut residuals: Vec<ResidualInfo> = b
        .residuals
        .iter()
        .enumerate()
        .map(|(i, &s)| ResidualInfo {
            equation: b.eqs.get(i).cloned().unwrap_or_default(),
            value: sim.rt.r[s as usize],
            source: residual_srcs.get(i).cloned().flatten(),
        })
        .collect();
    residuals.sort_by(|a, b| b.value.abs().total_cmp(&a.value.abs()));
    let mut updates: Vec<UpdateInfo> = tears
        .iter()
        .enumerate()
        .map(|(i, &t)| {
            let x = sim.rt.r[t as usize];
            UpdateInfo {
                var: m.vars[t as usize].name.clone(),
                value: x,
                step: last_step[i],
                relative: last_step[i].abs() / (1.0 + x.abs()),
                start: starts.map(|s| s[t as usize]),
            }
        })
        .collect();
    updates.sort_by(|a, b| b.relative.total_cmp(&a.relative));
    let mut hints = Vec::new();
    if singular {
        hints.push("the Jacobian of the residuals with respect to the tear variables is singular at the last iterate: the equations do not determine the unknowns there (a wrong branch, a vanishing coefficient, or redundant equations)".into());
    }
    if residuals.iter().any(|r| !r.value.is_finite()) {
        hints.push("a residual is not finite (NaN or infinite): an expression is evaluated outside its domain (sqrt or log of a negative number, division by zero); check the start values of the tear variables".into());
    }
    let unset: Vec<&str> = tears
        .iter()
        .filter(|&&t| m.vars[t as usize].start.is_none())
        .map(|&t| m.vars[t as usize].name.as_str())
        .collect();
    if !unset.is_empty() && nf.phase == "initialization" {
        hints.push(format!(
            "no start value for: {} (iteration starts at 0); a better guess with `start = ...` often helps",
            unset.join(", ")
        ));
    }
    let stalled = history.len() > 5 && {
        let k = history.len();
        history[k - 1] > 0.9 * history[k - 5] && history[k - 1] > 0.0
    };
    if stalled && !singular && norm > 1e-6 {
        hints.push("the residual stops decreasing at a nonzero value: the iteration sits at a minimum of the residual norm, so the equations may have no solution near these values (check signs, parameter values, and the start values of the tear variables)".into());
    }
    if norm.is_finite()
        && norm <= 1e-10 * (1.0 + updates.iter().map(|u| u.value.abs()).fold(0.0, f64::max))
    {
        hints.push("a plain damped Newton iteration from the same point converges: the failure depends on the solver's safeguards or the step taken; tighten tolerances or change start values".into());
    }
    let linear = b.linear;
    SolverFailure {
        phase: nf.phase.clone(),
        time: nf.time,
        block,
        linear,
        tears: b.names[..n].to_vec(),
        inner: b.names[n..].to_vec(),
        residual: nf.residual,
        iterations,
        history,
        residual_norm: norm,
        residuals,
        updates,
        jacobian_singular: singular,
        hints,
    }
}

/// Everything about a model's initialization; never fails (the error is in the report).
pub fn init_report(
    lib: &Library,
    model: &str,
    bopts: &backend::Options,
    so: Option<SimOptions>,
    params: &[(String, f64)],
) -> InitReport {
    let mut rep = InitReport {
        model: model.to_string(),
        ok: false,
        stage: "flatten",
        error: None,
        structural: None,
        unknowns: Vec::new(),
        relevant: 0,
        assignments: 0,
        loops: Vec::new(),
        failure: None,
        non_finite: Vec::new(),
        redundant: Vec::new(),
        unconnected: Vec::new(),
        warnings: Vec::new(),
    };
    let dbg = match Debugger::new(lib, model, bopts) {
        Ok(d) => d,
        Err(f) => {
            rep.stage = f.stage;
            rep.error = Some(f.error);
            rep.structural = f.structural;
            rep.unconnected = f.unconnected;
            return rep;
        }
    };
    rep.unconnected = dbg.unconnected.clone();
    rep.redundant = dbg.trace.init_redundant.clone();
    rep.warnings = dbg.sys.warnings.clone();
    // The runtime model: compile a copy of the system (the debugger keeps its own).
    let m = codegen::compile(clone_system(&dbg.sys));
    let mut o = so.unwrap_or_else(|| SimOptions::from_model(&m));
    if !params.is_empty() {
        o.params = params.to_vec();
    }
    let mut sim = Simulation::new(&m, o);
    rep.stage = "initialization";
    let starts = match sim.start_values() {
        Ok(s) => Some(s),
        Err(e) => {
            rep.error = Some(e);
            None
        }
    };
    let result = if starts.is_some() {
        sim.initialize()
    } else {
        Ok(())
    };
    let nv = m.layout.nv;
    if let Err(e) = &result {
        rep.error = Some(e.clone());
    }
    if let Some(nf) = sim.newton_failure.clone() {
        let describe = |bi: usize, _: &[usize]| dbg.residual_sources(&m, bi, &nf.phase);
        let block_of = |tears: &[VarId]| dbg.block_with_tears(tears, nf.phase == "initialization");
        rep.failure = Some(diagnose_newton(
            &mut sim,
            &nf,
            starts.as_deref(),
            &describe,
            &block_of,
        ));
    }
    let values: Vec<f64> = sim.rt.r[..nv].to_vec();
    rep.ok = result.is_ok() && starts.is_some();
    if rep.ok {
        rep.stage = "ok";
        rep.non_finite = (0..nv)
            .filter(|&i| !values[i].is_finite() && !m.vars[i].name.starts_with('$'))
            .map(|i| m.vars[i].name.clone())
            .take(50)
            .collect();
    }
    // Unknowns of the initial system.
    let states: FxHashSet<VarId> = dbg.sys.states.iter().copied().collect();
    let derivs: FxHashSet<VarId> = dbg.sys.state_derivs.iter().copied().collect();
    let fixed: FxHashSet<VarId> = dbg.trace.init_fixed.iter().copied().collect();
    let defaulted: FxHashSet<VarId> = dbg.trace.init_defaulted.iter().copied().collect();
    let mut seen = FxHashSet::default();
    for (bi, (b, tb)) in dbg
        .sys
        .init_blocks
        .iter()
        .zip(&dbg.trace.init_blocks)
        .enumerate()
    {
        let is_loop = matches!(b, Block::Solve { .. });
        if let Block::Solve {
            tears,
            inner,
            residuals,
            linear,
        } = b
        {
            rep.loops.push(InitBlockInfo {
                index: bi,
                kind: if *linear { "linear" } else { "nonlinear" },
                tears: tears.iter().map(|&t| dbg.name(t).to_string()).collect(),
                inner: inner
                    .iter()
                    .map(|(v, _)| dbg.name(*v).to_string())
                    .collect(),
                residuals: residuals
                    .iter()
                    .map(|&r| clip(format!("0 = {}", dbg.sys.show(r)), 300))
                    .collect(),
            });
        } else {
            rep.assignments += 1;
        }
        let nonlinear = matches!(b, Block::Solve { linear: false, .. });
        let vars = tb
            .solved
            .iter()
            .map(|&(_, v)| (v, false))
            .chain(tb.tears.iter().map(|&t| (t, true)));
        for (v, tear) in vars {
            if !seen.insert(v) || dbg.name(v).starts_with('$') {
                continue;
            }
            let var = &dbg.sys.vars[v as usize];
            let kind = if states.contains(&v) {
                "state"
            } else if derivs.contains(&v) || dbg.sys.der_of.contains(&Some(v)) {
                "derivative"
            } else if var.kind == VarKind::Discrete {
                if dbg.sys.vars[v as usize].fixed.is_some() || var.ty != crate::flat::VarType::Real
                {
                    "discrete"
                } else {
                    "parameter"
                }
            } else {
                "algebraic"
            };
            let start_use = if fixed.contains(&v) {
                "fixed"
            } else if defaulted.contains(&v) {
                "defaulted"
            } else if tear && (nonlinear || !is_loop) && is_loop {
                "guess"
            } else if tear && !nonlinear {
                "computed"
            } else {
                "computed"
            };
            rep.unknowns.push(InitUnknown {
                name: var.name.clone(),
                kind,
                start_use,
                start_value: starts.as_ref().map(|s| s[v as usize]),
                start: dbg.start_info(v),
                value: Some(values[v as usize]),
                block: Some(bi),
            });
        }
    }
    let rank = |u: &InitUnknown| match u.start_use {
        "fixed" => 0,
        "guess" => 1,
        "defaulted" => 2,
        _ => 3,
    };
    rep.unknowns
        .sort_by(|a, b| rank(a).cmp(&rank(b)).then(a.name.cmp(&b.name)));
    rep.relevant = rep
        .unknowns
        .iter()
        .filter(|u| u.start_use != "computed")
        .count();
    rep
}

impl Debugger {
    /// Source references of the residual equations of runtime Newton block `bi`.
    fn residual_sources(&self, m: &Model, bi: usize, phase: &str) -> Vec<Option<SourceRef>> {
        let tears: Vec<VarId> = m.newton[bi].tears.iter().map(|&s| s as VarId).collect();
        let init = phase == "initialization";
        let Some(k) = self.block_with_tears(&tears, init) else {
            return Vec::new();
        };
        let (tb, eqs) = if init {
            (&self.trace.init_blocks[k], &self.trace.init_eqs)
        } else {
            (&self.trace.blocks[k], &self.trace.eqs)
        };
        tb.residual_eqs
            .iter()
            .map(|&e| {
                let q = &eqs[e];
                if q.src != NO_SRC {
                    self.source_ref(q.src)
                } else {
                    let names = self.names_in(q.lhs, q.rhs);
                    Some(
                        self.sources()
                            .reference(self.text(q.lhs, q.rhs), q.origin, &names, true),
                    )
                }
            })
            .collect()
    }

    /// The block (initialization or simulation) whose tear variables are `tears`.
    fn block_with_tears(&self, tears: &[VarId], init: bool) -> Option<usize> {
        let blocks = if init {
            &self.sys.init_blocks
        } else {
            &self.sys.blocks
        };
        blocks
            .iter()
            .position(|b| matches!(b, Block::Solve { tears: t, .. } if t == tears))
    }
}

/// A copy of the parts of `sys` code generation needs.
fn clone_system(s: &System) -> System {
    System {
        name: s.name.clone(),
        arena: s.arena.clone(),
        vars: s.vars.clone(),
        der_of: s.der_of.clone(),
        states: s.states.clone(),
        state_derivs: s.state_derivs.clone(),
        params: s.params.clone(),
        inputs: s.inputs.clone(),
        discretes: s.discretes.clone(),
        blocks: s.blocks.clone(),
        init_blocks: s.init_blocks.clone(),
        pre_init: s.pre_init.clone(),
        whens: s.whens.clone(),
        asserts: s.asserts.clone(),
        aliases: s.aliases.clone(),
        experiment: s.experiment.clone(),
        stats: s.stats.clone(),
        warnings: s.warnings.clone(),
        origins: s.origins.clone(),
        externals: s.externals.clone(),
        trace: None,
    }
}

/// Re-run a simulation that failed in a nonlinear solve and examine the failing block at
/// the point of failure. `None` when the run does not fail in a nonlinear solve.
pub fn simulation_failure(
    lib: &Library,
    model: &str,
    bopts: &backend::Options,
    so: SimOptions,
) -> Option<SolverFailure> {
    let dbg = Debugger::new(lib, model, bopts).ok()?;
    let m = codegen::compile(clone_system(&dbg.sys));
    let stop = so.stop_time;
    let mut sim = Simulation::new(&m, so);
    let starts = sim.start_values().ok();
    let r = sim.initialize().and_then(|_| {
        let mut st = sim.stepper();
        sim.advance_to(&mut st, stop)
    });
    r.err()?;
    let nf = match sim.newton_failure.clone() {
        Some(nf) => nf,
        None => {
            // The run stopped for another reason (step size too small, often from
            // rejected steps whose Newton solves failed): evaluate the model where it
            // stopped and see whether a nonlinear block fails there.
            sim.rt.error = None;
            sim.rt.run(&m.ode);
            let e = sim.rt.error.take()?;
            NewtonFailure {
                block: e.block,
                phase: "simulation".into(),
                time: sim.rt.time(),
                residual: e.residual,
            }
        }
    };
    let describe = |bi: usize, _: &[usize]| dbg.residual_sources(&m, bi, &nf.phase);
    let block_of = |tears: &[VarId]| dbg.block_with_tears(tears, nf.phase == "initialization");
    Some(diagnose_newton(
        &mut sim,
        &nf,
        starts.as_deref(),
        &describe,
        &block_of,
    ))
}

// ---------------------------------------------------------------- text output

fn src_line(s: &SourceRef) -> String {
    let mut out = s.origin.clone();
    if let Some(c) = &s.component
        && !s.origin.contains(c.as_str())
    {
        out.push_str(&format!(" [{c}]"));
    }
    if let Some(l) = &s.loc {
        out.push_str(&format!(" at {l}"));
    }
    out
}

fn steps_text(steps: &[Step], s: &mut String) {
    use std::fmt::Write;
    for (i, st) in steps.iter().enumerate() {
        let _ = writeln!(s, "  {}. [{}] {}", i + 1, st.kind, st.title);
        if let Some(e) = &st.equation {
            let _ = writeln!(s, "       {e}");
        }
        if let Some(src) = &st.source {
            let _ = writeln!(s, "       from {}", src_line(src));
        }
        for d in &st.details {
            let _ = writeln!(s, "       - {d}");
        }
    }
}

impl VarTrace {
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} ({}{})",
            self.name,
            self.role,
            if self.unit.is_empty() {
                String::new()
            } else {
                format!(", {}", self.unit)
            }
        );
        if !self.description.is_empty() {
            let _ = writeln!(s, "  \"{}\"", self.description);
        }
        if let Some(a) = &self.alias_of {
            let _ = writeln!(s, "  alias of {a}");
        }
        if let Some(b) = self.block {
            let _ = writeln!(
                s,
                "  block {b} ({}), {}",
                self.block_kind.unwrap_or("?"),
                self.tearing_role.unwrap_or("?")
            );
        }
        let st = &self.start;
        if st.expr.is_some() || st.fixed.is_some() {
            let _ = writeln!(
                s,
                "  start = {}{}{}{}",
                st.expr.as_deref().unwrap_or("(none: 0)"),
                st.fixed
                    .map(|f| format!(", fixed = {f}"))
                    .unwrap_or_default(),
                st.written_in
                    .as_ref()
                    .map(|w| format!(", set in {}", if w.is_empty() { "the model" } else { w }))
                    .unwrap_or_default(),
                st.loc
                    .as_ref()
                    .map(|l| format!(" at {l}"))
                    .unwrap_or_default()
            );
            if let Some(a) = &st.from_alias {
                let _ = writeln!(s, "  (start value taken over from alias {a})");
            }
        }
        let _ = writeln!(s, "transformations:");
        steps_text(&self.steps, &mut s);
        s
    }
}

impl EqTrace {
    pub fn to_text(&self) -> String {
        let mut s = format!("equation {}: {}\n", self.id, self.text);
        if let Some(b) = self.block {
            s.push_str(&format!(
                "  block {b}{}\n",
                self.solved_for
                    .as_ref()
                    .map(|v| format!(", solved for {v}"))
                    .unwrap_or_else(|| ", residual".into())
            ));
        }
        s.push_str("transformations:\n");
        steps_text(&self.steps, &mut s);
        s
    }
}

impl StructuralDiag {
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "structural analysis of the {} problem ({} equations, {} unknowns):",
            self.problem, self.equations, self.unknowns
        );
        for (label, list) in [
            ("underdetermined", &self.underdetermined),
            ("overdetermined", &self.overdetermined),
        ] {
            for (k, sub) in list.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "  {label} subset {} ({} equations, {} unknowns){}:",
                    k + 1,
                    sub.equations.len(),
                    sub.variables.len(),
                    if sub.truncated { ", truncated" } else { "" }
                );
                if label == "underdetermined" {
                    let _ = writeln!(s, "    no equation left for: {}", sub.unmatched.join(", "));
                } else {
                    for u in &sub.unmatched {
                        let _ = writeln!(s, "    extra equation: {u}");
                    }
                }
                if !sub.components.is_empty() {
                    let _ = writeln!(s, "    components: {}", sub.components.join(", "));
                }
                for e in sub.equations.iter().take(12) {
                    let _ = writeln!(s, "      {}   ({})", e.text, src_line(e));
                    if let Some(n) = &e.now {
                        let _ = writeln!(s, "        after alias substitution: {n}");
                    }
                    if let Some(n) = &e.note {
                        let _ = writeln!(s, "        ({n})");
                    }
                }
                if sub.equations.len() > 12 {
                    let _ = writeln!(s, "      ... {} more", sub.equations.len() - 12);
                }
                if !sub.variables.is_empty() {
                    let _ = writeln!(s, "    unknowns: {}", clip(sub.variables.join(", "), 600));
                }
            }
        }
        for h in &self.hints {
            let _ = writeln!(s, "  hint: {h}");
        }
        s
    }
}

impl SolverFailure {
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} nonlinear solve failed at t = {}{}: {} {} tear variable(s) [{}]{}",
            self.phase,
            self.time,
            self.block
                .map(|b| format!(" (block {b})"))
                .unwrap_or_default(),
            if self.linear { "linear" } else { "nonlinear" },
            self.tears.len(),
            self.tears.join(", "),
            if self.inner.is_empty() {
                String::new()
            } else {
                format!(", {} inner", self.inner.len())
            }
        );
        let _ = writeln!(
            s,
            "  solver residual {:.3e}; diagnostic damped Newton re-run: {} iteration(s), residual {:.3e}{}",
            self.residual,
            self.iterations,
            self.residual_norm,
            if self.jacobian_singular {
                ", singular Jacobian"
            } else {
                ""
            }
        );
        if self.history.len() > 1 {
            let h: Vec<String> = self
                .history
                .iter()
                .take(12)
                .map(|x| format!("{x:.2e}"))
                .collect();
            let _ = writeln!(s, "  residual history: {}", h.join(" "));
        }
        let _ = writeln!(s, "  residuals at the last iterate (largest first):");
        for r in self.residuals.iter().take(10) {
            let _ = writeln!(s, "    {:>12.4e}  {}", r.value, r.equation);
            if let Some(src) = &r.source {
                let _ = writeln!(s, "                  from {}", src_line(src));
            }
        }
        let _ = writeln!(s, "  largest Newton updates (tear variables):");
        for u in self.updates.iter().take(10) {
            let _ = writeln!(
                s,
                "    {:<32} value {:.6e}, step {:.3e} (relative {:.2e}){}",
                u.var,
                u.value,
                u.step,
                u.relative,
                u.start.map(|x| format!(", start {x}")).unwrap_or_default()
            );
        }
        for h in &self.hints {
            let _ = writeln!(s, "  hint: {h}");
        }
        s
    }
}

impl InitReport {
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "initialization of {}: {}",
            self.model,
            if self.ok {
                "ok".to_string()
            } else {
                format!("FAILED ({})", self.stage)
            }
        );
        if let Some(e) = &self.error {
            let _ = writeln!(s, "  error: {}", e.lines().next().unwrap_or_default());
        }
        if let Some(d) = &self.structural {
            s.push_str(&d.to_text());
        }
        if self.stage != "flatten" && self.stage != "translation" {
            let _ = writeln!(
                s,
                "  initial system: {} unknowns ({} whose start value matters), {} explicit assignments, {} loops",
                self.unknowns.len(),
                self.relevant,
                self.assignments,
                self.loops.len()
            );
            let _ = writeln!(s, "start values used:");
            for u in self
                .unknowns
                .iter()
                .filter(|u| u.start_use != "computed")
                .take(200)
            {
                let st = &u.start;
                let _ = writeln!(
                    s,
                    "  {:<9} {:<10} {:<36} start {}{}{}{}{}",
                    u.start_use,
                    u.kind,
                    u.name,
                    u.start_value
                        .map(|x| format!("{x}"))
                        .unwrap_or_else(|| "?".into()),
                    st.expr
                        .as_ref()
                        .filter(|e| e.parse::<f64>().is_err())
                        .map(|e| format!(" (= {e})"))
                        .unwrap_or_default(),
                    st.written_in
                        .as_ref()
                        .map(|w| format!(" set in {}", if w.is_empty() { "the model" } else { w }))
                        .unwrap_or_else(|| if st.expr.is_none() {
                            " (default)".into()
                        } else {
                            String::new()
                        }),
                    st.loc
                        .as_ref()
                        .map(|l| format!(" at {l}"))
                        .unwrap_or_default(),
                    match u.value {
                        Some(v) if self.ok => format!(" -> {v}"),
                        _ => String::new(),
                    }
                );
            }
            for l in &self.loops {
                let _ = writeln!(
                    s,
                    "  loop (initialization block {}): {} with tears [{}]",
                    l.index,
                    l.kind,
                    l.tears.join(", ")
                );
            }
        }
        if let Some(f) = &self.failure {
            s.push_str(&f.to_text());
        }
        if !self.non_finite.is_empty() {
            let _ = writeln!(
                s,
                "  not finite after initialization: {}",
                self.non_finite.join(", ")
            );
        }
        for r in &self.redundant {
            let _ = writeln!(s, "  redundant initial equation ignored: {r}");
        }
        for u in &self.unconnected {
            let _ = writeln!(
                s,
                "  {}: {} ({}): {}",
                u.level,
                u.connector,
                u.flows.join(", "),
                u.reason
            );
        }
        s
    }
}
