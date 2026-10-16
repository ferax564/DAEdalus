//! Symbolic backend: from a flat model to a sorted, causalised equation system.
//!
//! Pipeline (the part of Dymola's toolchain the research brief identifies as
//! the real source of its speed):
//! 1. parameter ordering and evaluation,
//! 2. alias / trivial equation elimination,
//! 3. derivative variables, Pantelides index reduction, dummy-derivative state selection,
//! 4. maximum matching and BLT sorting into strongly connected components,
//! 5. symbolic solving of scalar blocks and tearing of algebraic loops,
//! 6. a separate, structurally analysed initialization system.

pub mod alias;
pub mod eval;
pub mod graph;
pub mod index;
pub mod sort;
pub mod state_select;
pub mod trace;

use crate::expr::{Arena, ExprId, Node, VarId};
use crate::flat::*;
use anyhow::{Result, bail};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

/// One causal computation step.
#[derive(Debug, Clone)]
pub enum Block {
    /// `var := expr` (expr only depends on already computed quantities).
    Assign { var: VarId, expr: ExprId },
    /// Algebraic loop after tearing: Newton on `tears`; `inner` assignments are
    /// evaluated in order from the current tear values, then `residuals` must vanish.
    Solve {
        tears: Vec<VarId>,
        inner: Vec<(VarId, ExprId)>,
        residuals: Vec<ExprId>,
        linear: bool,
    },
}

#[derive(Debug, Clone, Serialize, serde::Deserialize)]
pub enum AliasKind {
    /// `var = sign * of`
    Var { of: VarId, sign: f64 },
    /// `var` is a function of parameters / inputs / discrete values only.
    Expr,
}

#[derive(Debug, Clone)]
pub struct Alias {
    pub var: VarId,
    pub kind: AliasKind,
    pub expr: ExprId,
}

#[derive(Debug, Clone, Default, Serialize, serde::Deserialize)]
pub struct Stats {
    pub flat_variables: usize,
    pub flat_equations: usize,
    pub aliases_removed: usize,
    pub equations_after_alias: usize,
    pub differentiated_equations: usize,
    pub states: usize,
    pub dummy_derivatives: usize,
    pub blocks: usize,
    pub algebraic_loops: usize,
    pub largest_loop: usize,
    pub torn_loop_vars: usize,
    pub linear_loops: usize,
    pub init_blocks: usize,
    pub init_loops: usize,
    pub relations: usize,
    pub when_clauses: usize,
    pub backend_ms: f64,
}

/// The causalised system handed to code generation.
pub struct System {
    pub name: String,
    pub arena: Arena,
    pub vars: Vec<Variable>,
    pub der_of: Vec<Option<VarId>>,
    pub states: Vec<VarId>,
    pub state_derivs: Vec<VarId>,
    /// Parameters in dependency order (binding evaluated in order).
    pub params: Vec<VarId>,
    pub inputs: Vec<VarId>,
    /// Discrete variables assigned in when-clauses.
    pub discretes: Vec<VarId>,
    /// Runtime blocks: compute all non-state unknowns from states, params, time, discretes.
    pub blocks: Vec<Block>,
    /// Initialization blocks: compute states and all unknowns at t0.
    pub init_blocks: Vec<Block>,
    /// `(v, p)`: initialization solved `pre(v)` as variable `p`.
    pub pre_init: Vec<(VarId, VarId)>,
    pub whens: Vec<WhenClause>,
    pub asserts: Vec<(ExprId, String)>,
    pub aliases: Vec<Alias>,
    pub experiment: Experiment,
    pub stats: Stats,
    pub warnings: Vec<String>,
    pub origins: Vec<String>,
    /// Structural record for the structure report (only with [`Options::trace`]).
    pub trace: Option<trace::Trace>,
}

#[derive(Clone, Copy, Debug)]
pub struct Eq {
    pub lhs: ExprId,
    pub rhs: ExprId,
    pub origin: u32,
    /// Index of the equation this one was differentiated from.
    pub parent: Option<usize>,
    /// Index of the derivative of this equation (Pantelides `B`).
    pub derived: Option<usize>,
}

/// Mutable working state of the backend.
pub struct Work {
    pub arena: Arena,
    pub vars: Vec<Variable>,
    pub eqs: Vec<Eq>,
    pub init_eqs: Vec<Eq>,
    pub whens: Vec<WhenClause>,
    pub asserts: Vec<(ExprId, String)>,
    pub unknown: Vec<bool>,
    pub der_of: Vec<Option<VarId>>,
    pub base_of: Vec<Option<VarId>>,
    pub aliases: Vec<Alias>,
    pub param_values: Vec<f64>,
    pub warnings: Vec<String>,
    pub origins: Vec<String>,
    pub when_assigned: FxHashSet<VarId>,
    pub dummy: FxHashSet<VarId>,
    /// Unknowns fixed by equations over parameters only: their time derivative is zero.
    pub invariant: FxHashSet<VarId>,
    /// The equations that define the `invariant` unknowns.
    pub invariant_eqs: FxHashSet<usize>,
    /// Variables only the initialization problem determines (`fixed = false` parameters).
    pub init_only: FxHashSet<VarId>,
    /// `(v, p)`: initialization solved `pre(v)` as variable `p`.
    pub pre_init: Vec<(VarId, VarId)>,
    pub trace: Option<trace::Trace>,
}

impl Work {
    pub fn name(&self, v: VarId) -> &str {
        &self.vars[v as usize].name
    }

    pub fn show(&self, e: ExprId) -> String {
        self.arena
            .render(e, &|v| self.vars[v as usize].name.clone())
    }

    pub fn eq_text(&self, e: &Eq) -> String {
        let o = self
            .origins
            .get(e.origin as usize)
            .cloned()
            .unwrap_or_default();
        format!("{} = {}   ({})", self.show(e.lhs), self.show(e.rhs), o)
    }

    pub fn add_var(&mut self, v: Variable, unknown: bool) -> VarId {
        self.vars.push(v);
        self.unknown.push(unknown);
        self.der_of.push(None);
        self.base_of.push(None);
        self.param_values.push(0.0);
        (self.vars.len() - 1) as VarId
    }

    /// Is `v` a known quantity in the continuous system (parameter, input, event state)?
    pub fn is_known(&self, v: VarId) -> bool {
        !self.unknown[v as usize]
    }

    /// Leaves of an expression that are unknown variables.
    pub fn unknowns_in(&self, e: ExprId, out: &mut Vec<VarId>) {
        self.arena.visit_leaves(e, &mut |n| {
            if let Node::Var(v) = n
                && self.unknown[v as usize]
                && !out.contains(&v)
            {
                out.push(v);
            }
        });
    }

    /// Evaluate a parameter-only expression with current parameter values.
    pub fn eval_params(&self, e: ExprId) -> f64 {
        let pv = &self.param_values;
        let vals = eval::Values {
            var: &|v| pv[v as usize],
            pre: &|v| pv[v as usize],
            time: 0.0,
            initial: true,
            rel: None,
        };
        eval::eval(&self.arena, e, &vals, &mut FxHashMap::default())
    }

    /// Start value of a variable (evaluated with parameters), or 0.
    pub fn start_value(&self, v: VarId) -> f64 {
        match self.vars[v as usize].start {
            Some(s) => {
                let x = self.eval_params(s);
                if x.is_finite() { x } else { 0.0 }
            }
            None => 0.0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    pub index_reduction: bool,
    pub tearing: bool,
    /// `stateSelect` overrides applied before index reduction (later ones win).
    pub state_overrides: Vec<state_select::StateOverride>,
    /// Keep a [`trace::Trace`] of the structural decisions in [`System::trace`].
    pub trace: bool,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            index_reduction: true,
            tearing: true,
            state_overrides: Vec::new(),
            trace: false,
        }
    }
}

pub fn compile(flat: FlatModel, opts: &Options) -> Result<System> {
    let t0 = web_time_now();
    let mut stats = Stats {
        flat_variables: flat.vars.len(),
        flat_equations: flat.equations.len(),
        relations: flat.arena.relations.len(),
        when_clauses: flat.whens.len(),
        ..Default::default()
    };
    let FlatModel {
        name,
        arena,
        vars,
        equations,
        initial_equations,
        whens,
        asserts,
        origins,
        experiment,
        warnings,
    } = flat;
    let mut vars = vars;
    let mut warnings = warnings;
    // `reinit(x, ..)` implicitly gives x `stateSelect = StateSelect.always` (spec 8.3.6):
    // otherwise index reduction may pick a variable related to x as the state
    // (`H1 = -beta - Hc`, Preisach hysteresis) and the reinit is lost.
    for w in &whens {
        for (_, acts) in &w.branches {
            for a in acts {
                if let WhenAction::Reinit(v, _) = a {
                    vars[*v as usize].state_select = StateSelect::Always;
                }
            }
        }
    }
    let applied = state_select::apply(&mut vars, &opts.state_overrides, &mut warnings);
    let mut arena = arena;
    let mut initial_equations = initial_equations;
    let mut origins = origins;
    // Parameters with `fixed = false` are unknowns of the initialization problem and
    // constant afterwards, as are parameters whose bindings depend on them. They become
    // discrete variables that only initialization assigns.
    let mut init_only: FxHashSet<VarId> = FxHashSet::default();
    for (i, v) in vars.iter().enumerate() {
        if v.kind == VarKind::Parameter && v.fixed == Some(false) {
            init_only.insert(i as VarId);
        }
    }
    if !init_only.is_empty() {
        loop {
            let mut grew = false;
            for (i, v) in vars.iter().enumerate() {
                if v.kind != VarKind::Parameter || init_only.contains(&(i as VarId)) {
                    continue;
                }
                let Some(b) = v.binding else { continue };
                let mut dep = false;
                arena.visit_leaves(b, &mut |n| {
                    if let Node::Var(x) = n
                        && init_only.contains(&x)
                    {
                        dep = true;
                    }
                });
                if dep {
                    init_only.insert(i as VarId);
                    grew = true;
                }
            }
            if !grew {
                break;
            }
        }
        let mut ids: Vec<VarId> = init_only.iter().copied().collect();
        ids.sort();
        for v in ids {
            let var = &mut vars[v as usize];
            var.kind = VarKind::Discrete;
            if let Some(b) = var.binding.take() {
                let lhs = arena.var(v);
                origins.push(format!("binding of {}", var.name));
                initial_equations.push(crate::flat::Equation {
                    lhs,
                    rhs: b,
                    origin: origins.len() as u32 - 1,
                });
            }
            var.fixed = None;
        }
    }
    let n = vars.len();
    let mut when_assigned = FxHashSet::default();
    for w in &whens {
        for (_, acts) in &w.branches {
            for a in acts {
                if let WhenAction::Assign(v, _) = a {
                    when_assigned.insert(*v);
                }
            }
        }
    }
    when_assigned.extend(init_only.iter().copied());
    let unknown: Vec<bool> = vars
        .iter()
        .enumerate()
        .map(|(i, v)| match v.kind {
            VarKind::Continuous => true,
            VarKind::Discrete => !when_assigned.contains(&(i as VarId)),
            _ => false,
        })
        .collect();
    let mut w = Work {
        arena,
        vars,
        eqs: equations
            .iter()
            .map(|e| Eq {
                lhs: e.lhs,
                rhs: e.rhs,
                origin: e.origin,
                parent: None,
                derived: None,
            })
            .collect(),
        init_eqs: initial_equations
            .iter()
            .map(|e| Eq {
                lhs: e.lhs,
                rhs: e.rhs,
                origin: e.origin,
                parent: None,
                derived: None,
            })
            .collect(),
        whens,
        asserts,
        unknown,
        der_of: vec![None; n],
        base_of: vec![None; n],
        aliases: Vec::new(),
        param_values: vec![0.0; n],
        warnings,
        origins,
        when_assigned,
        dummy: FxHashSet::default(),
        invariant: FxHashSet::default(),
        invariant_eqs: FxHashSet::default(),
        init_only,
        pre_init: Vec::new(),
        trace: opts.trace.then(|| trace::Trace {
            overrides: applied,
            ..Default::default()
        }),
    };

    // 1. Parameters
    let params = order_parameters(&mut w)?;
    let inputs: Vec<VarId> = (0..w.vars.len() as VarId)
        .filter(|&v| w.vars[v as usize].kind == VarKind::Input)
        .collect();
    let mut discretes: Vec<VarId> = w.when_assigned.iter().copied().collect();
    discretes.sort();

    // Final parameters that are zero drop out of the equations before any structural
    // analysis: `ir*m_resist = p.v - pin` with `m_resist = 0` is `p.v = pin`, not an
    // equation for `ir` (Spice3 diodes without series resistance).
    fold_final_zeros(&mut w);

    // 2. Alias elimination
    let removed = alias::eliminate(&mut w);
    stats.aliases_removed = removed;
    stats.equations_after_alias = w.eqs.len();

    // 3. Derivatives and index reduction
    index::find_invariant(&mut w);
    index::create_derivative_vars(&mut w);
    let n_before = w.eqs.len();
    if opts.index_reduction {
        index::pantelides(&mut w)?;
    }
    stats.differentiated_equations = w.eqs.len() - n_before;
    let states = index::select_states(&mut w)?;
    stats.dummy_derivatives = w.dummy.len();
    let state_derivs: Vec<VarId> = states
        .iter()
        .map(|&s| w.der_of[s as usize].unwrap())
        .collect();
    stats.states = states.len();

    // 4./5. Runtime system: unknowns = all unknown vars except states.
    let state_set: FxHashSet<VarId> = states.iter().copied().collect();
    let eq_list: Vec<Eq> = w.eqs.clone();
    let blocks = sort::causalize(
        &mut w,
        &eq_list,
        &|v| !state_set.contains(&v),
        opts.tearing,
        "simulation",
    )?;

    // 6. Initialization
    let init_blocks = sort::initialization(&mut w, &states, opts.tearing)?;

    for b in &blocks {
        stats.blocks += 1;
        if let Block::Solve {
            tears,
            inner,
            linear,
            ..
        } = b
        {
            stats.algebraic_loops += 1;
            stats.largest_loop = stats.largest_loop.max(tears.len() + inner.len());
            stats.torn_loop_vars += tears.len();
            if *linear {
                stats.linear_loops += 1;
            }
        }
    }
    stats.init_blocks = init_blocks.len();
    stats.init_loops = init_blocks
        .iter()
        .filter(|b| matches!(b, Block::Solve { .. }))
        .count();
    stats.backend_ms = (web_time_now() - t0) * 1000.0;

    if states.is_empty()
        && blocks.is_empty()
        && w.vars.iter().all(|v| v.kind != VarKind::Continuous)
    {
        // purely discrete / parametric model is fine
    }
    // Variables read through pre() outside when-clauses (`y = pre(aux)` in the Digital
    // gates): the event iteration also runs until they settle.
    {
        let mut roots: Vec<ExprId> = w.eqs.iter().flat_map(|e| [e.lhs, e.rhs]).collect();
        for wc in &w.whens {
            for (c, acts) in &wc.branches {
                roots.push(*c);
                for a in acts {
                    match a {
                        WhenAction::Assign(_, e)
                        | WhenAction::Reinit(_, e)
                        | WhenAction::Assert(e, _) => roots.push(*e),
                        WhenAction::Effect(c, e) => {
                            roots.push(*c);
                            roots.push(*e);
                        }
                        WhenAction::Terminate(_) => {}
                    }
                }
            }
        }
        let mut pre_read = FxHashSet::default();
        for r in roots {
            w.arena.visit_leaves(r, &mut |n| {
                if let Node::Pre(v) = n {
                    pre_read.insert(v);
                }
            });
        }
        let known: FxHashSet<VarId> = discretes.iter().copied().collect();
        let mut extra: Vec<VarId> = pre_read
            .into_iter()
            .filter(|v| !known.contains(v) && !state_set.contains(v))
            .filter(|&v| {
                !matches!(
                    w.vars[v as usize].kind,
                    VarKind::Parameter | VarKind::Constant
                )
            })
            .collect();
        discretes.append(&mut extra);
        discretes.sort();
    }
    let der_of = w.der_of.clone();
    Ok(System {
        name,
        arena: w.arena,
        vars: w.vars,
        der_of,
        states,
        state_derivs,
        params,
        inputs,
        discretes,
        blocks,
        init_blocks,
        pre_init: w.pre_init,
        whens: w.whens,
        asserts: w.asserts,
        aliases: w.aliases,
        experiment,
        stats,
        warnings: w.warnings,
        origins: w.origins,
        trace: w.trace,
    })
}

/// Topologically order parameters by binding dependencies and evaluate them.
fn order_parameters(w: &mut Work) -> Result<Vec<VarId>> {
    let params: Vec<VarId> = (0..w.vars.len() as VarId)
        .filter(|&v| {
            matches!(
                w.vars[v as usize].kind,
                VarKind::Parameter | VarKind::Constant | VarKind::Input
            )
        })
        .collect();
    let is_param: FxHashSet<VarId> = params.iter().copied().collect();
    let mut deps: FxHashMap<VarId, Vec<VarId>> = FxHashMap::default();
    for &p in &params {
        let var = &w.vars[p as usize];
        let e = if var.kind == VarKind::Input {
            var.start
        } else {
            var.binding.or(var.start)
        };
        let mut d = Vec::new();
        if let Some(e) = e {
            w.arena.visit_leaves(e, &mut |n| {
                if let Node::Var(v) = n
                    && is_param.contains(&v)
                    && v != p
                {
                    d.push(v);
                }
            });
        }
        if var.binding.is_none() && var.start.is_none() && var.kind == VarKind::Parameter {
            w.warnings
                .push(format!("parameter '{}' has no value; using 0", var.name));
        }
        deps.insert(p, d);
    }
    // DFS topological sort
    let mut order = Vec::with_capacity(params.len());
    let mut state: FxHashMap<VarId, u8> = FxHashMap::default();
    for &p in &params {
        if state.get(&p).copied().unwrap_or(0) == 2 {
            continue;
        }
        let mut stack = vec![(p, 0usize)];
        while let Some(&mut (v, ref mut i)) = stack.last_mut() {
            if *i == 0 {
                if state.get(&v).copied().unwrap_or(0) == 1 {
                    // revisiting while on stack: cycle
                }
                state.insert(v, 1);
            }
            let d = &deps[&v];
            if *i < d.len() {
                let c = d[*i];
                *i += 1;
                match state.get(&c).copied().unwrap_or(0) {
                    0 => stack.push((c, 0)),
                    1 => bail!(
                        "cyclic parameter binding involving '{}' and '{}'",
                        w.vars[v as usize].name,
                        w.vars[c as usize].name
                    ),
                    _ => {}
                }
            } else {
                state.insert(v, 2);
                order.push(v);
                stack.pop();
            }
        }
    }
    for &p in &order {
        let var = &w.vars[p as usize];
        let e = if var.kind == VarKind::Input {
            var.start
        } else {
            var.binding.or(var.start)
        };
        let val = e.map(|e| w.eval_params(e)).unwrap_or(0.0);
        w.param_values[p as usize] = val;
    }
    Ok(order)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn web_time_now() -> f64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

#[cfg(target_arch = "wasm32")]
pub fn web_time_now() -> f64 {
    0.0
}

impl System {
    pub fn name_of(&self, v: VarId) -> &str {
        &self.vars[v as usize].name
    }

    pub fn show(&self, e: ExprId) -> String {
        self.arena
            .render(e, &|v| self.vars[v as usize].name.clone())
    }

    /// Human-readable listing of the sorted system (like Dymola's dsmodel translation log).
    pub fn dump(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let names = |vs: &[VarId]| {
            vs.iter()
                .map(|&v| self.name_of(v).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        };
        let _ = writeln!(s, "system {}", self.name);
        let _ = writeln!(s, "  states: {}", names(&self.states));
        let _ = writeln!(s, "  derivatives: {}", names(&self.state_derivs));
        let dump_blocks = |s: &mut String, blocks: &[Block]| {
            for b in blocks {
                match b {
                    Block::Assign { var, expr } => {
                        let _ = writeln!(s, "    {} := {}", self.name_of(*var), self.show(*expr));
                    }
                    Block::Solve {
                        tears,
                        inner,
                        residuals,
                        linear,
                    } => {
                        let _ = writeln!(
                            s,
                            "    {} loop, tearing vars [{}]",
                            if *linear { "linear" } else { "nonlinear" },
                            names(tears)
                        );
                        for (v, e) in inner {
                            let _ = writeln!(s, "      {} := {}", self.name_of(*v), self.show(*e));
                        }
                        for r in residuals {
                            let _ = writeln!(s, "      0 = {}", self.show(*r));
                        }
                    }
                }
            }
        };
        let _ = writeln!(s, "  initialization:");
        dump_blocks(&mut s, &self.init_blocks);
        let _ = writeln!(s, "  simulation:");
        dump_blocks(&mut s, &self.blocks);
        let _ = writeln!(
            s,
            "  stats: {}",
            serde_json::to_string(&self.stats).unwrap_or_default()
        );
        s
    }
}

/// Replace final parameters whose value is exactly zero by the constant 0.
fn fold_final_zeros(w: &mut Work) {
    let zero: FxHashSet<VarId> = (0..w.vars.len() as VarId)
        .filter(|&v| {
            let var = &w.vars[v as usize];
            var.is_final
                && matches!(var.kind, VarKind::Parameter | VarKind::Constant)
                && var.ty == VarType::Real
                && w.param_values[v as usize] == 0.0
        })
        .collect();
    if zero.is_empty() {
        return;
    }
    let mut memo = FxHashMap::default();
    let mut fold = |arena: &mut Arena, e: ExprId| {
        arena.rewrite(e, &mut memo, &mut |_, n| match n {
            Node::Var(v) if zero.contains(&v) => Some(Arena::ZERO),
            _ => None,
        })
    };
    for i in 0..w.eqs.len() {
        let (l, r) = (w.eqs[i].lhs, w.eqs[i].rhs);
        w.eqs[i].lhs = fold(&mut w.arena, l);
        w.eqs[i].rhs = fold(&mut w.arena, r);
    }
}
