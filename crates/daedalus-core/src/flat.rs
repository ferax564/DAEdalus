//! Flat (scalarised) model: the contract between the frontend and the symbolic backend.
//!
//! This plays the role Base Modelica plays in the research brief: a fully
//! instantiated, scalar equation system with variable attributes, independent
//! of how it was produced.

use crate::expr::{Arena, ExprId, VarId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VarKind {
    Constant,
    Parameter,
    /// Top-level input: value supplied externally (defaults to its start value).
    Input,
    /// Discrete-time variable (Integer, Boolean, `discrete Real`, or assigned in `when`).
    Discrete,
    Continuous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum VarType {
    Real,
    Integer,
    Boolean,
    Enumeration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum StateSelect {
    Never,
    Avoid,
    Default,
    Prefer,
    Always,
}

/// Name of the parameter that blends `homotopy(actual, simplified)` during initialization.
pub const HOMOTOPY_LAMBDA: &str = "$homotopy.lambda";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Variable {
    pub name: String,
    pub kind: VarKind,
    pub ty: VarType,
    /// Binding equation (`= expr` in the declaration) for parameters/constants.
    #[serde(skip)]
    pub binding: Option<ExprId>,
    #[serde(skip)]
    pub start: Option<ExprId>,
    pub fixed: Option<bool>,
    #[serde(skip)]
    pub nominal: Option<ExprId>,
    #[serde(skip)]
    pub min: Option<ExprId>,
    #[serde(skip)]
    pub max: Option<ExprId>,
    pub unit: String,
    pub display_unit: String,
    pub description: String,
    pub state_select: StateSelect,
    pub is_flow: bool,
    pub is_protected: bool,
    /// Declared `final` (or part of a final component): its value cannot be changed.
    pub is_final: bool,
    pub is_output: bool,
    /// Variable introduced by the compiler (derivative of a derivative, dummy, ...).
    pub generated: bool,
}

impl Variable {
    pub fn new(name: impl Into<String>, kind: VarKind, ty: VarType) -> Self {
        Variable {
            name: name.into(),
            kind,
            ty,
            binding: None,
            start: None,
            fixed: None,
            nominal: None,
            min: None,
            max: None,
            unit: String::new(),
            display_unit: String::new(),
            description: String::new(),
            state_select: StateSelect::Default,
            is_flow: false,
            is_protected: false,
            is_final: false,
            is_output: false,
            generated: false,
        }
    }
}

/// Residual equation `lhs - rhs = 0`, kept in the two-sided form for nicer solving.
#[derive(Debug, Clone, Copy)]
pub struct Equation {
    pub lhs: ExprId,
    pub rhs: ExprId,
    /// Index into [`FlatModel::origins`] (source description for diagnostics).
    pub origin: u32,
}

#[derive(Debug, Clone)]
pub enum WhenAction {
    /// Discrete assignment `v := expr` (from `v = expr` inside a when-clause).
    Assign(VarId, ExprId),
    /// `reinit(state, expr)`.
    Reinit(VarId, ExprId),
    /// `terminate("msg")`.
    Terminate(String),
    /// `assert(cond, msg)` evaluated at the event.
    Assert(ExprId, String),
    /// `Effect(cond, call)`: a call of an impure external function (a [`crate::expr::Node::Call`]
    /// of a native program), executed when the branch fires and `cond` holds, before the
    /// other actions read its results. Effects run in the order of the list.
    Effect(ExprId, ExprId),
}

/// `when c1 then .. elsewhen c2 then .. end when` lowered to an ordered list of branches.
#[derive(Debug, Clone)]
pub struct WhenClause {
    pub branches: Vec<(ExprId, Vec<WhenAction>)>,
    /// True if the clause is `when initial()` (or contains `initial()` in the condition).
    pub origin: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Experiment {
    pub start_time: Option<f64>,
    pub stop_time: Option<f64>,
    pub tolerance: Option<f64>,
    pub interval: Option<f64>,
    pub algorithm: Option<String>,
}

/// A position in a source file: the parser's source id (a hash of the document URI, see
/// `rumoca_core::source_id_for_name`) and a 1-based line and column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SrcLoc {
    pub source: u64,
    pub line: u32,
    pub col: u32,
}

/// Where a variable's `start` attribute was written: the instance whose modification
/// (or declaration) set it, and its position.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StartSrc {
    /// Instance path of the scope the modifier was written in (`""`: the model itself).
    pub scope: String,
    pub loc: Option<SrcLoc>,
}

#[derive(Clone, Default)]
pub struct FlatModel {
    pub name: String,
    pub arena: Arena,
    pub vars: Vec<Variable>,
    pub equations: Vec<Equation>,
    pub initial_equations: Vec<Equation>,
    pub whens: Vec<WhenClause>,
    /// Runtime assertions `(condition, message)` checked after each accepted step.
    pub asserts: Vec<(ExprId, String)>,
    pub origins: Vec<String>,
    pub experiment: Experiment,
    pub warnings: Vec<String>,
    /// Instances of FMU wrapper blocks (see [`crate::external_component`]).
    pub externals: Vec<crate::external_component::ExternalComponent>,
    /// Source position of an origin (index into `origins`), where the frontend knows it.
    pub origin_locs: rustc_hash::FxHashMap<u32, SrcLoc>,
    /// Where the `start` attribute of a variable comes from (equation debugger).
    pub start_src: rustc_hash::FxHashMap<VarId, StartSrc>,
}

impl FlatModel {
    pub fn add_var(&mut self, v: Variable) -> VarId {
        self.vars.push(v);
        (self.vars.len() - 1) as VarId
    }

    pub fn origin(&mut self, s: impl Into<String>) -> u32 {
        self.origins.push(s.into());
        (self.origins.len() - 1) as u32
    }

    /// Record the source position of origin `o`.
    pub fn set_origin_loc(&mut self, o: u32, loc: Option<SrcLoc>) {
        if let Some(l) = loc {
            self.origin_locs.insert(o, l);
        }
    }

    pub fn var_name(&self, v: VarId) -> &str {
        &self.vars[v as usize].name
    }

    /// Render an expression with variable names.
    pub fn show(&self, e: ExprId) -> String {
        self.arena
            .render(e, &|v| self.vars[v as usize].name.clone())
    }

    /// Human-readable dump of the flat model (like `instantiateModel` output).
    pub fn dump(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "model {}", self.name);
        for v in &self.vars {
            let kind = match v.kind {
                VarKind::Constant => "constant ",
                VarKind::Parameter => "parameter ",
                VarKind::Input => "input ",
                VarKind::Discrete => "discrete ",
                VarKind::Continuous => "",
            };
            let ty = match v.ty {
                VarType::Real => "Real",
                VarType::Integer => "Integer",
                VarType::Boolean => "Boolean",
                VarType::Enumeration => "enumeration",
            };
            let mut attrs = Vec::new();
            if let Some(st) = v.start {
                attrs.push(format!("start = {}", self.show(st)));
            }
            if let Some(f) = v.fixed {
                attrs.push(format!("fixed = {f}"));
            }
            let a = if attrs.is_empty() {
                String::new()
            } else {
                format!("({})", attrs.join(", "))
            };
            let b = v
                .binding
                .map(|b| format!(" = {}", self.show(b)))
                .unwrap_or_default();
            let _ = writeln!(s, "  {kind}{ty} {}{a}{b};", v.name);
        }
        let _ = writeln!(s, "equation");
        for e in &self.equations {
            let _ = writeln!(s, "  {} = {};", self.show(e.lhs), self.show(e.rhs));
        }
        for w in &self.whens {
            for (i, (c, acts)) in w.branches.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "  {} {} then",
                    if i == 0 { "when" } else { "elsewhen" },
                    self.show(*c)
                );
                for a in acts {
                    match a {
                        WhenAction::Assign(v, e) => {
                            let _ = writeln!(s, "    {} = {};", self.var_name(*v), self.show(*e));
                        }
                        WhenAction::Reinit(v, e) => {
                            let _ = writeln!(
                                s,
                                "    reinit({}, {});",
                                self.var_name(*v),
                                self.show(*e)
                            );
                        }
                        WhenAction::Terminate(m) => {
                            let _ = writeln!(s, "    terminate({m:?});");
                        }
                        WhenAction::Assert(c, m) => {
                            let _ = writeln!(s, "    assert({}, {m:?});", self.show(*c));
                        }
                        WhenAction::Effect(c, e) => {
                            let _ = writeln!(s, "    if {} then {};", self.show(*c), self.show(*e));
                        }
                    }
                }
            }
            let _ = writeln!(s, "  end when;");
        }
        if !self.initial_equations.is_empty() {
            let _ = writeln!(s, "initial equation");
            for e in &self.initial_equations {
                let _ = writeln!(s, "  {} = {};", self.show(e.lhs), self.show(e.rhs));
            }
        }
        let _ = writeln!(s, "end {};", self.name);
        s
    }
}
