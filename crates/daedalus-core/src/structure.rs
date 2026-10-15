//! Model structure and Jacobian explorer.
//!
//! [`structure`] translates a model with the backend's trace on and reports:
//! - every variable and its role (state, derivative, dummy derivative, algebraic,
//!   alias, parameter, discrete, input);
//! - the BLT blocks in evaluation order: which equation computes which unknown,
//!   assignments, linear and nonlinear loops, and the tear variables of torn loops;
//! - the incidence pattern (equation x unknown) in BLT order;
//! - the state-selection sets of index reduction: for each set of candidates, which
//!   became states and which dummy derivatives, with their `stateSelect` tiers and why.
//!
//! [`analyze`] adds the numerics at a solution point (the start, or after simulating to
//! `at`, as `linearize --at` does): the exact state Jacobian `df/dx` with its sparsity
//! and condition number, eigenvalues, time constants, stiffness ratio and participation
//! factors, the Jacobian of each algebraic loop with respect to its tear variables, and a
//! check of the static state selection along the way.

use crate::backend::state_select::{AppliedOverride, tier_name};
use crate::backend::trace::Trace;
use crate::backend::{self, Block, System};
use crate::codegen::Model;
use crate::expr::{Arena, ExprId, Node, VarId};
use crate::flat::{StateSelect, VarKind};
use crate::linearize::{JacobianMethod, linearize_at};
use crate::modal;
use crate::sim::{SimOptions, Simulation};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;

#[derive(Clone, Debug, Serialize)]
pub struct VarInfo {
    pub name: String,
    /// state | derivative | dummy derivative | algebraic | alias | parameter | input |
    /// discrete | unused
    pub role: &'static str,
    /// The block that computes it (algebraic, dummy derivative, derivative).
    pub block: Option<usize>,
    pub state_select: &'static str,
    /// The model's own `stateSelect` when an override changed it.
    pub model_state_select: Option<&'static str>,
    pub unit: String,
    /// Alias of (`-x` for a negated alias), derivative of, ...
    pub note: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct EqInfo {
    pub id: usize,
    pub text: String,
    pub origin: String,
    /// How often index reduction differentiated it (0 = as written).
    pub differentiated: u32,
    pub block: Option<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockEq {
    pub eq: usize,
    /// The unknown this equation is solved for; `None` for a residual of a loop.
    pub var: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct BlockInfo {
    pub index: usize,
    /// assign | linear | nonlinear
    pub kind: &'static str,
    /// Number of unknowns.
    pub size: usize,
    /// A loop with explicitly solved inner equations.
    pub torn: bool,
    pub tears: Vec<String>,
    pub inner: usize,
    pub residuals: usize,
    pub vars: Vec<String>,
    pub eqs: Vec<BlockEq>,
}

/// Equation x unknown pattern in BLT order (rows and columns permuted alike, so the
/// blocks sit on the diagonal).
#[derive(Clone, Debug, Serialize, Default)]
pub struct Incidence {
    /// Equation id of each row.
    pub rows: Vec<usize>,
    /// Unknown of each column.
    pub cols: Vec<String>,
    /// First row (= column) of each block, then the total.
    pub block_starts: Vec<usize>,
    /// `[row, col]` of each nonzero.
    pub entries: Vec<[u32; 2]>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Candidate {
    /// The variable that is or is not a state.
    pub var: String,
    pub derivative: String,
    pub state_select: &'static str,
    pub overridden: bool,
    /// Tie-breaking heuristics that made it less wanted as a state: `trig` (defined as the
    /// sine or cosine of another unknown), `nonconstant` (its derivative has a
    /// solution-dependent coefficient).
    pub flags: Vec<&'static str>,
    /// state | dummy
    pub chosen: &'static str,
}

/// The constraint Jacobian `H` of a state set evaluated at a solution point.
#[derive(Clone, Debug, Serialize)]
pub struct SetCheck {
    pub time: f64,
    /// `||H||_1 ||S^-1||_1` with `S` the columns of the chosen dummy derivatives: how well
    /// the constraints determine the dummies (infinite = the state choice is singular).
    pub cond: f64,
    /// The same for the choice the selection would make at this point.
    pub best_cond: f64,
    /// The chosen states are much worse than the best choice here (or singular).
    pub fails: bool,
    /// The dummies the selection would pick at this point.
    pub best: Vec<String>,
    /// Sign of the determinant of the chosen columns: a change between two checks means
    /// the choice went through a singular point in between.
    #[serde(skip)]
    pub det_sign: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct StateSetInfo {
    pub index: usize,
    pub level: usize,
    pub eqs: Vec<usize>,
    pub candidates: Vec<Candidate>,
    pub states: usize,
    pub dummies: usize,
    pub reason: String,
    /// The constraint Jacobian depends on the solution, so the best choice of states can
    /// change during a simulation (a dynamic state selection case).
    pub state_dependent: bool,
    /// At the analysis start point (filled by [`analyze`]).
    pub at_start: Option<SetCheck>,
    /// Worst point seen while simulating to the analysis time.
    pub worst: Option<SetCheck>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StateReason {
    pub state: String,
    pub reason: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct Mode {
    pub re: f64,
    pub im: f64,
    /// `1/|re|` (none for a mode on the imaginary axis).
    pub time_constant: Option<f64>,
    /// Oscillation frequency `|im| / 2 pi` in Hz (none for a real mode).
    pub frequency: Option<f64>,
    /// Damping ratio `-re/|lambda|`.
    pub damping: Option<f64>,
    /// Most participating states: `(state index, factor)`, largest first.
    pub participation: Vec<(usize, f64)>,
}

#[derive(Clone, Debug, Serialize)]
pub struct LoopJacobian {
    pub block: usize,
    pub tears: Vec<String>,
    pub size: usize,
    pub cond: f64,
    pub singular: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct JacobianReport {
    pub time: f64,
    pub states: Vec<String>,
    pub x0: Vec<f64>,
    pub method: JacobianMethod,
    /// `[row, col, value]` of the nonzeros of `df/dx`.
    pub entries: Vec<(u32, u32, f64)>,
    pub nnz: usize,
    /// 1-norm condition number of `df/dx` (none when skipped for size).
    pub cond: Option<f64>,
    pub singular: bool,
    pub modes: Vec<Mode>,
    /// `max |re| / min |re|` over the decaying modes.
    pub stiffness_ratio: Option<f64>,
    pub fastest_rate: Option<f64>,
    pub slowest_rate: Option<f64>,
    /// Simulation horizon used for the solver suggestion.
    pub horizon: f64,
    /// stiff | non-stiff
    pub suggested_solver: &'static str,
    pub solver_reason: String,
    pub loops: Vec<LoopJacobian>,
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct StructureReport {
    pub model: String,
    pub stats: backend::Stats,
    pub states: Vec<String>,
    pub state_reasons: Vec<StateReason>,
    pub variables: Vec<VarInfo>,
    pub equations: Vec<EqInfo>,
    pub blocks: Vec<BlockInfo>,
    pub incidence: Incidence,
    pub state_sets: Vec<StateSetInfo>,
    pub overrides: Vec<AppliedOverride>,
    /// Equations differentiated for `stateSelect = prefer/always` variables.
    pub state_change_eqs: usize,
    pub warnings: Vec<String>,
    pub jacobian: Option<JacobianReport>,
    pub jacobian_error: Option<String>,
}

/// Options of [`structure`].
#[derive(Clone, Debug, Default)]
pub struct StructureOptions {
    /// Run the Jacobian analysis.
    pub jacobian: bool,
    /// Simulate to this time first (default: the start time).
    pub at: Option<f64>,
    /// Simulation options; `None` takes them from the experiment annotation.
    pub sim: Option<SimOptions>,
    /// Applied to the experiment's options (when `sim` is `None`).
    pub params: Vec<(String, f64)>,
    pub stop_time: Option<f64>,
    pub tolerance: Option<f64>,
}

/// Largest state Jacobian for eigenvalues and condition numbers.
const MAX_DENSE: usize = 800;
/// Largest state Jacobian for participation factors (two complex LU per mode).
const MAX_PARTICIPATION: usize = 200;
/// Largest loop whose tear Jacobian is evaluated.
const MAX_LOOP: usize = 200;
/// Checkpoints of the state-selection check while simulating to the analysis time.
const CHECKPOINTS: usize = 50;
/// Relative condition above which a state choice counts as failing (when a much better
/// choice exists).
const COND_FAIL: f64 = 100.0;

/// Translate `model` and report its structure; with `opts.jacobian` also the Jacobian
/// analysis (whose failure is reported in `jacobian_error`, not as an error).
pub fn structure(
    lib: &crate::frontend::Library,
    model: &str,
    bopts: &backend::Options,
    opts: &StructureOptions,
) -> anyhow::Result<StructureReport> {
    let flat = lib.flatten(model)?;
    let mut o = bopts.clone();
    o.trace = true;
    let mut sys = backend::compile(flat, &o)?;
    let trace = sys.trace.take().unwrap_or_default();
    let mut report = report(&mut sys, &trace);
    if !opts.jacobian {
        return Ok(report);
    }
    let sets = SetJacobians::new(&mut sys, &trace);
    let arena = sys.arena.clone();
    let blocks = sys.blocks.clone();
    let m = crate::codegen::compile(sys);
    let mut so = opts
        .sim
        .clone()
        .unwrap_or_else(|| SimOptions::from_model(&m));
    if opts.sim.is_none() {
        if !opts.params.is_empty() {
            so.params = opts.params.clone();
        }
        if let Some(t) = opts.stop_time {
            so.stop_time = t;
        }
        if let Some(t) = opts.tolerance {
            so.tolerance = t;
        }
    }
    match analyze(&m, &arena, &blocks, &sets, so, opts.at, &mut report) {
        Ok(j) => report.jacobian = Some(j),
        Err(e) => report.jacobian_error = Some(e),
    }
    Ok(report)
}

fn names(sys: &System, vs: &[VarId]) -> Vec<String> {
    vs.iter().map(|&v| sys.name_of(v).to_string()).collect()
}

fn clip(mut s: String) -> String {
    if s.len() > 400 {
        let mut k = 400;
        while !s.is_char_boundary(k) {
            k -= 1;
        }
        s.truncate(k);
        s.push('…');
    }
    s
}

/// The static part of the report (no simulation).
pub fn report(sys: &mut System, trace: &Trace) -> StructureReport {
    let nv = sys.vars.len();
    let mut base_of: Vec<Option<VarId>> = vec![None; nv];
    for (v, d) in sys.der_of.iter().enumerate() {
        if let Some(d) = d {
            base_of[*d as usize] = Some(v as VarId);
        }
    }
    let states: FxHashSet<VarId> = sys.states.iter().copied().collect();
    let derivs: FxHashSet<VarId> = sys.state_derivs.iter().copied().collect();
    let discretes: FxHashSet<VarId> = sys.discretes.iter().copied().collect();
    let mut block_of: FxHashMap<VarId, usize> = FxHashMap::default();
    for (i, b) in sys.blocks.iter().enumerate() {
        match b {
            Block::Assign { var, .. } => {
                block_of.insert(*var, i);
            }
            Block::Solve { tears, inner, .. } => {
                for &t in tears {
                    block_of.insert(t, i);
                }
                for &(v, _) in inner {
                    block_of.insert(v, i);
                }
            }
        }
    }
    let alias_of: FxHashMap<VarId, &backend::AliasKind> =
        sys.aliases.iter().map(|a| (a.var, &a.kind)).collect();
    let overridden: FxHashMap<&str, &AppliedOverride> = trace
        .overrides
        .iter()
        .map(|o| (o.var.as_str(), o))
        .collect();

    // Variables
    let mut variables = Vec::new();
    for v in 0..nv as VarId {
        let var = &sys.vars[v as usize];
        if var.name.starts_with('$') {
            continue;
        }
        let mut note = None;
        let role = if matches!(var.kind, VarKind::Parameter | VarKind::Constant) {
            "parameter"
        } else if var.kind == VarKind::Input {
            "input"
        } else if let Some(k) = alias_of.get(&v) {
            note = Some(match k {
                backend::AliasKind::Var { of, sign } => {
                    format!("{}{}", if *sign < 0.0 { "-" } else { "" }, sys.name_of(*of))
                }
                backend::AliasKind::Expr => "fixed by parameters".into(),
            });
            "alias"
        } else if states.contains(&v) {
            "state"
        } else if derivs.contains(&v) {
            "derivative"
        } else if block_of.contains_key(&v) && base_of[v as usize].is_some() {
            note = base_of[v as usize].map(|b| format!("derivative of {}", sys.name_of(b)));
            "dummy derivative"
        } else if discretes.contains(&v) {
            "discrete"
        } else if block_of.contains_key(&v) {
            if var.kind == VarKind::Discrete {
                "discrete"
            } else {
                "algebraic"
            }
        } else {
            "unused"
        };
        let ov = overridden.get(var.name.as_str());
        variables.push(VarInfo {
            name: var.name.clone(),
            role,
            block: block_of.get(&v).copied(),
            state_select: tier_name(var.state_select),
            model_state_select: ov.map(|o| tier_name(o.model)),
            unit: var.unit.clone(),
            note,
        });
    }

    // Equations and blocks
    let mut eq_block: Vec<Option<usize>> = vec![None; trace.eqs.len()];
    let mut blocks = Vec::new();
    let mut inc = Incidence::default();
    let mut col_of: FxHashMap<VarId, u32> = FxHashMap::default();
    for (i, (b, tb)) in sys.blocks.iter().zip(&trace.blocks).enumerate() {
        inc.block_starts.push(inc.rows.len());
        let mut rows: Vec<usize> = Vec::new();
        let mut cols: Vec<VarId> = Vec::new();
        let mut eqs = Vec::new();
        for &(e, v) in &tb.solved {
            rows.push(e);
            cols.push(v);
            eqs.push(BlockEq {
                eq: e,
                var: Some(sys.name_of(v).to_string()),
            });
        }
        for &e in &tb.residual_eqs {
            rows.push(e);
            eqs.push(BlockEq { eq: e, var: None });
        }
        cols.extend(tb.tears.iter().copied());
        for &e in &rows {
            eq_block[e] = Some(i);
        }
        for &v in &cols {
            col_of.insert(v, inc.cols.len() as u32);
            inc.cols.push(sys.name_of(v).to_string());
        }
        inc.rows.extend(rows.iter().copied());
        let (kind, torn, tears, inner, residuals) = match b {
            Block::Assign { .. } => ("assign", false, vec![], 0, 0),
            Block::Solve {
                tears,
                inner,
                residuals,
                linear,
            } => (
                if *linear { "linear" } else { "nonlinear" },
                !inner.is_empty(),
                names(sys, tears),
                inner.len(),
                residuals.len(),
            ),
        };
        blocks.push(BlockInfo {
            index: i,
            kind,
            size: cols.len(),
            torn,
            tears,
            inner,
            residuals,
            vars: names(sys, &cols),
            eqs,
        });
    }
    inc.block_starts.push(inc.rows.len());
    for (r, &e) in inc.rows.iter().enumerate() {
        for v in &trace.incidence[e] {
            if let Some(&c) = col_of.get(v) {
                inc.entries.push([r as u32, c]);
            }
        }
    }
    let equations: Vec<EqInfo> = trace
        .eqs
        .iter()
        .enumerate()
        .map(|(i, e)| {
            let mut d = 0;
            let mut p = e.parent;
            while let Some(q) = p {
                d += 1;
                p = trace.eqs[q].parent;
            }
            EqInfo {
                id: i,
                text: clip(format!("{} = {}", sys.show(e.lhs), sys.show(e.rhs))),
                origin: sys
                    .origins
                    .get(e.origin as usize)
                    .cloned()
                    .unwrap_or_default(),
                differentiated: d,
                block: eq_block[i],
            }
        })
        .collect();

    // State sets
    let mut state_sets = Vec::new();
    let mut in_set: FxHashMap<VarId, usize> = FxHashMap::default();
    for (si, s) in trace.state_sets.iter().enumerate() {
        let dummies: FxHashSet<VarId> = s.dummies.iter().copied().collect();
        let mut cands = Vec::new();
        for (&z, &t) in s.candidates.iter().zip(&s.tiers) {
            let b = base_of[z as usize].unwrap_or(z);
            in_set.entry(b).or_insert(si);
            let mut flags = Vec::new();
            if (t % 4) / 2 == 1 {
                flags.push("trig");
            }
            if t % 2 == 1 {
                flags.push("nonconstant");
            }
            let name = sys.name_of(b).to_string();
            cands.push(Candidate {
                overridden: overridden.contains_key(name.as_str()),
                var: name,
                derivative: sys.name_of(z).to_string(),
                state_select: tier_name(sys.vars[b as usize].state_select),
                flags,
                chosen: if dummies.contains(&z) {
                    "dummy"
                } else {
                    "state"
                },
            });
        }
        // Does the constraint Jacobian depend on unknowns?
        let mut dependent = false;
        for &e in &s.eqs {
            let eq = trace.eqs[e];
            let r = sys.arena.sub(eq.lhs, eq.rhs);
            for &z in &s.candidates {
                let d = sys.arena.diff(r, Node::Var(z), &mut FxHashMap::default());
                if sys.arena.vars_in(&[d]).iter().any(|&v| {
                    matches!(
                        sys.vars[v as usize].kind,
                        VarKind::Continuous | VarKind::Discrete
                    )
                }) {
                    dependent = true;
                }
            }
        }
        let reason = set_reason(s, &cands, dependent);
        state_sets.push(StateSetInfo {
            index: si,
            level: s.level,
            eqs: s.eqs.clone(),
            states: cands.len() - s.dummies.len(),
            dummies: s.dummies.len(),
            candidates: cands,
            reason,
            state_dependent: dependent && s.candidates.len() > s.dummies.len(),
            at_start: None,
            worst: None,
        });
    }
    let state_reasons = sys
        .states
        .iter()
        .map(|&x| {
            let var = &sys.vars[x as usize];
            let tier = tier_name(var.state_select);
            let ov = overridden
                .get(var.name.as_str())
                .map(|o| format!(", overridden from {}", tier_name(o.model)))
                .unwrap_or_default();
            let reason = match in_set.get(&x) {
                Some(&si) => format!(
                    "kept as a state in state set {si} (stateSelect = {tier}{ov})"
                ),
                None if var.state_select == StateSelect::Never => format!(
                    "appears differentiated and no constraint lets it go, despite stateSelect = never{ov}"
                ),
                None => format!(
                    "appears differentiated and no constraint involves it (stateSelect = {tier}{ov})"
                ),
            };
            StateReason {
                state: var.name.clone(),
                reason,
            }
        })
        .collect();
    StructureReport {
        model: sys.name.clone(),
        stats: sys.stats.clone(),
        states: names(sys, &sys.states.clone()),
        state_reasons,
        variables,
        equations,
        blocks,
        incidence: inc,
        state_sets,
        overrides: trace.overrides.clone(),
        state_change_eqs: trace.state_change_eqs,
        warnings: sys.warnings.clone(),
        jacobian: None,
        jacobian_error: None,
    }
}

fn set_reason(s: &backend::trace::StateSet, cands: &[Candidate], dependent: bool) -> String {
    let n = cands.len();
    let k = s.dummies.len();
    let names = |which: &str| -> String {
        cands
            .iter()
            .filter(|c| c.chosen == which)
            .map(|c| c.var.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut r = if k == n {
        format!(
            "{k} constraint equation(s) fix all {n} candidates: none of [{}] can be a state.",
            names("dummy")
        )
    } else {
        format!(
            "{k} constraint equation(s) remove {k} of {n} candidates: [{}] become dummy derivatives (algebraic), [{}] stay states. ",
            names("dummy"),
            names("state")
        )
    };
    if k < n {
        let class = |t: i64| t / 4;
        let dummy_t: Vec<i64> = s
            .candidates
            .iter()
            .zip(&s.tiers)
            .filter(|(z, _)| s.dummies.contains(z))
            .map(|(_, &t)| t)
            .collect();
        let kept_t: Vec<i64> = s
            .candidates
            .iter()
            .zip(&s.tiers)
            .filter(|(z, _)| !s.dummies.contains(z))
            .map(|(_, &t)| t)
            .collect();
        let dmin = dummy_t.iter().copied().min().unwrap_or(0);
        let kmax = kept_t.iter().copied().max().unwrap_or(0);
        if dummy_t.iter().all(|&t| class(t) > class(kmax)) {
            r.push_str("Decided by stateSelect: the kept ones have the stronger tier.");
        } else if dmin > kmax {
            r.push_str("Same stateSelect tier; decided by heuristics (variables defined by sin/cos of another unknown, and derivatives with solution-dependent coefficients, are less wanted as states).");
        } else {
            r.push_str("Equal preference: picked by column pivoting on the constraint Jacobian at the start values (the best-conditioned choice).");
        }
        if dependent {
            r.push_str(" The constraint Jacobian depends on the solution, so the best choice can change during a simulation; DAEdalus selects states once, statically (no dynamic state selection). Use --state to pick other states if the check below fails.");
        }
    }
    r
}

/// Per set: `jac[row][candidate]`, the candidates, their tiers and the chosen columns.
type SetJacobian = (Vec<Vec<ExprId>>, Vec<VarId>, Vec<i64>, Vec<usize>);

/// Symbolic constraint Jacobians of the state sets, for the numeric checks.
pub struct SetJacobians {
    sets: Vec<SetJacobian>,
}

impl SetJacobians {
    pub fn new(sys: &mut System, trace: &Trace) -> SetJacobians {
        let mut sets = Vec::new();
        for s in &trace.state_sets {
            if s.eqs.len() > 64 || s.candidates.len() == s.dummies.len() {
                sets.push((Vec::new(), Vec::new(), Vec::new(), Vec::new()));
                continue;
            }
            let jac = s
                .eqs
                .iter()
                .map(|&e| {
                    let eq = trace.eqs[e];
                    let r = sys.arena.sub(eq.lhs, eq.rhs);
                    s.candidates
                        .iter()
                        .map(|&z| sys.arena.diff(r, Node::Var(z), &mut FxHashMap::default()))
                        .collect()
                })
                .collect();
            let chosen = s
                .dummies
                .iter()
                .filter_map(|d| s.candidates.iter().position(|z| z == d))
                .collect();
            sets.push((jac, s.candidates.clone(), s.tiers.clone(), chosen));
        }
        SetJacobians { sets }
    }

    /// Check of set `i` at the point `r` (registers indexed by variable).
    fn check(&self, arena: &Arena, m: &Model, i: usize, r: &[f64], t: f64) -> Option<SetCheck> {
        let (jac, cands, tiers, chosen) = &self.sets[i];
        if jac.is_empty() {
            return None;
        }
        let vals = crate::backend::eval::Values {
            var: &|v| r[v as usize],
            pre: &|v| r[v as usize],
            time: t,
            initial: false,
            rel: None,
        };
        let h: Vec<Vec<f64>> = jac
            .iter()
            .map(|row| {
                let mut memo = FxHashMap::default();
                row.iter()
                    .map(|&d| crate::backend::eval::eval(arena, d, &vals, &mut memo))
                    .collect()
            })
            .collect();
        let rel = |cols: &[usize]| {
            let sub: Vec<Vec<f64>> = h
                .iter()
                .map(|row| cols.iter().map(|&c| row[c]).collect())
                .collect();
            let c = modal::norm1(&h) * modal::inv_norm1(&sub);
            if c.is_finite() { c } else { f64::INFINITY }
        };
        let cond = rel(chosen);
        let det_sign = {
            let sub: Vec<Vec<f64>> = h
                .iter()
                .map(|row| chosen.iter().map(|&c| row[c]).collect())
                .collect();
            modal::det_sign(&sub)
        };
        // Pivoting over all candidates regardless of stateSelect: the best possible choice.
        let pick = pivot_choice(&h, &vec![0; tiers.len()], jac.len());
        let best_cond = if pick.len() == jac.len() {
            rel(&pick)
        } else {
            f64::INFINITY
        };
        let best = pick
            .into_iter()
            .map(|c| m.vars[cands[c] as usize].name.clone())
            .collect();
        Some(SetCheck {
            time: t,
            cond,
            best_cond,
            fails: !cond.is_finite() || (cond > COND_FAIL && cond > 100.0 * best_cond),
            best,
            det_sign,
        })
    }
}

/// Gram–Schmidt with column pivoting inside tiers (as the backend's selection does).
fn pivot_choice(h: &[Vec<f64>], tiers: &[i64], need: usize) -> Vec<usize> {
    let rows = h.len();
    let ncols = tiers.len();
    let mut cols: Vec<Vec<f64>> = (0..ncols)
        .map(|c| (0..rows).map(|r| h[r][c]).collect())
        .collect();
    let norm0: Vec<f64> = cols
        .iter()
        .map(|c| c.iter().map(|x| x * x).sum::<f64>().sqrt())
        .collect();
    let mut taken = vec![false; ncols];
    let mut chosen = Vec::new();
    let mut start = 0;
    while start < ncols && chosen.len() < need {
        let mut end = start;
        while end < ncols && tiers[end] == tiers[start] {
            end += 1;
        }
        while chosen.len() < need {
            let mut best = None;
            let mut best_n = 0.0;
            for c in start..end {
                if taken[c] || norm0[c] <= 0.0 || norm0[c].is_nan() {
                    continue;
                }
                let n = cols[c].iter().map(|x| x * x).sum::<f64>().sqrt();
                if n > 1e-8 * norm0[c] && n > best_n {
                    best_n = n;
                    best = Some(c);
                }
            }
            let Some(c) = best else { break };
            taken[c] = true;
            chosen.push(c);
            let q: Vec<f64> = cols[c].iter().map(|x| x / best_n).collect();
            for (k, col) in cols.iter_mut().enumerate() {
                if taken[k] {
                    continue;
                }
                let dot: f64 = col.iter().zip(&q).map(|(x, y)| x * y).sum();
                for (x, y) in col.iter_mut().zip(&q) {
                    *x -= dot * y;
                }
            }
        }
        start = end;
    }
    chosen
}

/// Simulate to `at` (default: the start), checking the state selection on the way, and
/// analyse the state Jacobian and the loop Jacobians there.
pub fn analyze(
    m: &Model,
    arena: &Arena,
    blocks: &[Block],
    sets: &SetJacobians,
    opts: SimOptions,
    at: Option<f64>,
    report: &mut StructureReport,
) -> Result<JacobianReport, String> {
    let t0 = opts.start_time;
    let horizon = (opts.stop_time - opts.start_time).abs();
    let t_lin = at.unwrap_or(t0);
    if t_lin < t0 {
        return Err(format!(
            "analysis time {t_lin} is before the start time {t0}"
        ));
    }
    let mut o = opts;
    o.stop_time = t_lin;
    let mut sim = Simulation::new(m, o);
    sim.initialize()?;
    let nv = m.vars.len();
    let mut notes = Vec::new();
    let mut signs: Vec<f64> = vec![0.0; sets.sets.len()];
    let mut check_all = |sim: &Simulation, report: &mut StructureReport, first: bool| {
        let r = &sim.rt.r[..nv];
        let t = sim.rt.time();
        for (i, sign) in signs.iter_mut().enumerate() {
            let Some(mut c) = sets.check(arena, m, i, r, t) else {
                continue;
            };
            let s = &mut report.state_sets[i];
            if first {
                s.at_start = Some(c.clone());
            } else if *sign * c.det_sign < 0.0 {
                // The determinant changed sign since the last check: singular in between.
                c.cond = f64::INFINITY;
                c.fails = true;
            }
            *sign = c.det_sign;
            if s.worst
                .as_ref()
                .is_none_or(|w| c.cond > w.cond || c.cond.is_nan())
            {
                s.worst = Some(c);
            }
        }
    };
    sim.rt.run(&m.ode);
    check_all(&sim, report, true);
    if t_lin > t0 {
        let mut st = sim.stepper();
        for k in 1..=CHECKPOINTS {
            let t = t0 + (t_lin - t0) * k as f64 / CHECKPOINTS as f64;
            sim.advance_to(&mut st, t)?;
            if let Some(msg) = sim.terminated() {
                return Err(format!(
                    "the model terminated at t = {} before the analysis time: {msg}",
                    sim.rt.time()
                ));
            }
            check_all(&sim, report, false);
        }
    } else {
        sim.rt.run(&m.outputs);
    }
    for s in &report.state_sets {
        if let Some(w) = &s.worst
            && w.fails
        {
            let msg = format!(
                "state set {}: the static state choice is singular or ill-conditioned at t = {} (cond {:.3e}, best possible {:.3e}); this model needs dynamic state selection, which DAEdalus does not do. Pick other states with --state (best choice there: dummies [{}])",
                s.index,
                w.time,
                w.cond,
                w.best_cond,
                w.best.join(", ")
            );
            report.warnings.push(msg);
        }
    }
    let lin = linearize_at(&mut sim.rt, JacobianMethod::Exact)?;
    let n = lin.states.len();
    let a = &lin.a;
    let mut entries = Vec::new();
    for (i, row) in a.iter().enumerate() {
        for (j, &v) in row.iter().enumerate() {
            if v != 0.0 {
                entries.push((i as u32, j as u32, v));
            }
        }
    }
    let (cond, eigs) = if n == 0 {
        (Some(1.0), Vec::new())
    } else if n <= MAX_DENSE {
        (Some(modal::cond1(a)), modal::sorted_eigenvalues(a))
    } else {
        notes.push(format!(
            "{n} states: eigenvalues and condition number are computed up to {MAX_DENSE} states"
        ));
        (None, Vec::new())
    };
    let part = if n > 0 && n <= MAX_PARTICIPATION {
        modal::participation(a, &eigs)
    } else {
        if n > MAX_PARTICIPATION && n <= MAX_DENSE {
            notes.push(format!(
                "participation factors are computed up to {MAX_PARTICIPATION} states"
            ));
        }
        Vec::new()
    };
    let modes: Vec<Mode> = eigs
        .iter()
        .enumerate()
        .map(|(k, &(re, im))| {
            let mag = re.hypot(im);
            let mut p: Vec<(usize, f64)> = part
                .get(k)
                .map(|row| row.iter().copied().enumerate().collect())
                .unwrap_or_default();
            p.sort_by(|x, y| y.1.total_cmp(&x.1));
            p.truncate(5);
            Mode {
                re,
                im,
                time_constant: (re != 0.0).then(|| 1.0 / re.abs()),
                frequency: (im != 0.0).then(|| im.abs() / (2.0 * std::f64::consts::PI)),
                damping: (mag > 0.0).then(|| -re / mag),
                participation: p,
            }
        })
        .collect();
    let scale = eigs.iter().map(|e| e.0.hypot(e.1)).fold(0.0, f64::max);
    let decaying: Vec<f64> = eigs
        .iter()
        .filter(|e| e.0 < -1e-12 * scale.max(1e-300))
        .map(|e| -e.0)
        .collect();
    let fastest = decaying
        .iter()
        .copied()
        .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.max(x))));
    let slowest = decaying
        .iter()
        .copied()
        .fold(None, |m: Option<f64>, x| Some(m.map_or(x, |m| m.min(x))));
    let ratio = fastest.zip(slowest).map(|(f, s)| f / s);
    let (suggested, reason) = match fastest {
        Some(f) if f * horizon > 500.0 => (
            "stiff",
            format!(
                "the fastest mode decays with time constant {:.3e} s, {:.0} times shorter than the {horizon} s horizon{}: an explicit method would be held to steps of that size long after the mode has died out; use BDF",
                1.0 / f,
                f * horizon,
                ratio
                    .map(|r| format!(" (stiffness ratio {r:.3e})"))
                    .unwrap_or_default()
            ),
        ),
        Some(f) => (
            "non-stiff",
            format!(
                "the fastest decaying mode (time constant {:.3e} s) is resolved over the {horizon} s horizon in a few hundred steps; an explicit method (Dopri5) is efficient",
                1.0 / f
            ),
        ),
        None if n == 0 => ("non-stiff", "no states".to_string()),
        None => (
            "non-stiff",
            "no decaying modes (all eigenvalues on or right of the imaginary axis)".to_string(),
        ),
    };
    // Loop Jacobians w.r.t. the tear variables.
    let mut loops = Vec::new();
    let r0: Vec<f64> = sim.rt.r[..nv].to_vec();
    let t = sim.rt.time();
    for (bi, b) in blocks.iter().enumerate() {
        let Block::Solve {
            tears,
            inner,
            residuals,
            ..
        } = b
        else {
            continue;
        };
        if tears.len() > MAX_LOOP {
            continue;
        }
        let jac = loop_jacobian(arena, &r0, t, tears, inner, residuals);
        let cond = modal::cond1(&jac);
        loops.push(LoopJacobian {
            block: bi,
            tears: tears
                .iter()
                .map(|&v| m.vars[v as usize].name.clone())
                .collect(),
            size: tears.len() + inner.len(),
            cond,
            singular: !cond.is_finite(),
        });
    }
    Ok(JacobianReport {
        time: lin.time,
        states: lin.states.clone(),
        x0: lin.x0.clone(),
        method: lin.method,
        nnz: entries.len(),
        entries,
        cond: cond.filter(|c| c.is_finite()),
        singular: cond.is_some_and(|c| !c.is_finite()),
        modes,
        stiffness_ratio: ratio,
        fastest_rate: fastest,
        slowest_rate: slowest,
        horizon,
        suggested_solver: suggested,
        solver_reason: reason,
        loops,
        notes,
    })
}

/// Jacobian of a loop's residuals with respect to its tear variables (the inner
/// assignments follow the tears), by central differences of the symbolic residuals.
fn loop_jacobian(
    arena: &Arena,
    r0: &[f64],
    t: f64,
    tears: &[VarId],
    inner: &[(VarId, ExprId)],
    residuals: &[ExprId],
) -> Vec<Vec<f64>> {
    let eval_res = |pt: &mut Vec<f64>| -> Vec<f64> {
        for &(v, e) in inner {
            let x = {
                let p: &Vec<f64> = pt;
                let vals = crate::backend::eval::Values {
                    var: &|u| p[u as usize],
                    pre: &|u| p[u as usize],
                    time: t,
                    initial: false,
                    rel: None,
                };
                crate::backend::eval::eval(arena, e, &vals, &mut FxHashMap::default())
            };
            pt[v as usize] = x;
        }
        let p: &Vec<f64> = pt;
        let vals = crate::backend::eval::Values {
            var: &|u| p[u as usize],
            pre: &|u| p[u as usize],
            time: t,
            initial: false,
            rel: None,
        };
        let mut memo = FxHashMap::default();
        residuals
            .iter()
            .map(|&r| crate::backend::eval::eval(arena, r, &vals, &mut memo))
            .collect()
    };
    let n = tears.len();
    let mut cols: Vec<Vec<f64>> = Vec::with_capacity(n);
    for &tv in tears {
        let x = r0[tv as usize];
        let h = 1e-6 * x.abs().max(1.0);
        let mut p = r0.to_vec();
        p[tv as usize] = x + h;
        let fp = eval_res(&mut p);
        let mut p = r0.to_vec();
        p[tv as usize] = x - h;
        let fm = eval_res(&mut p);
        cols.push(
            fp.iter()
                .zip(&fm)
                .map(|(a, b)| (a - b) / (2.0 * h))
                .collect(),
        );
    }
    (0..residuals.len())
        .map(|i| (0..n).map(|j| cols[j][i]).collect())
        .collect()
}

impl StructureReport {
    /// Human-readable report (the CLI's `--format text`).
    pub fn to_text(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "structure of {}", self.model);
        let count = |role: &str| self.variables.iter().filter(|v| v.role == role).count();
        let _ = writeln!(
            s,
            "  variables: {} states, {} dummy derivatives, {} algebraic, {} aliases, {} parameters, {} discrete",
            count("state"),
            count("dummy derivative"),
            count("algebraic"),
            count("alias"),
            count("parameter"),
            count("discrete")
        );
        let nb = |k: &str| self.blocks.iter().filter(|b| b.kind == k).count();
        let _ = writeln!(
            s,
            "  blocks: {} ({} assignments, {} linear loops, {} nonlinear (torn {}))",
            self.blocks.len(),
            nb("assign"),
            nb("linear"),
            nb("nonlinear"),
            self.blocks.iter().filter(|b| b.torn).count()
        );
        let _ = writeln!(
            s,
            "  incidence: {} x {}, {} nonzeros",
            self.incidence.rows.len(),
            self.incidence.cols.len(),
            self.incidence.entries.len()
        );
        if !self.overrides.is_empty() {
            let _ = writeln!(s, "\nstateSelect overrides:");
            for o in &self.overrides {
                let _ = writeln!(
                    s,
                    "  {}: {} -> {} (from '{}')",
                    o.var,
                    tier_name(o.model),
                    tier_name(o.tier),
                    o.pattern
                );
            }
        }
        let _ = writeln!(s, "\nstates ({}):", self.states.len());
        for r in &self.state_reasons {
            let _ = writeln!(s, "  {}: {}", r.state, r.reason);
        }
        if !self.state_sets.is_empty() {
            let _ = writeln!(s, "\nstate selection (dummy derivatives):");
            for set in &self.state_sets {
                let _ = writeln!(
                    s,
                    "  set {} (level {}, {} equations):",
                    set.index,
                    set.level,
                    set.eqs.len()
                );
                for c in &set.candidates {
                    let _ = writeln!(
                        s,
                        "    {:<5} {} (stateSelect = {}{}{})",
                        c.chosen,
                        c.var,
                        c.state_select,
                        if c.overridden { ", overridden" } else { "" },
                        if c.flags.is_empty() {
                            String::new()
                        } else {
                            format!("; {}", c.flags.join(", "))
                        }
                    );
                }
                let _ = writeln!(s, "    {}", set.reason);
                for (label, c) in [("at start", &set.at_start), ("worst", &set.worst)] {
                    if let Some(c) = c {
                        let _ = writeln!(
                            s,
                            "    check {label}: t = {}, cond = {:.3e} (best choice {:.3e}){}",
                            c.time,
                            c.cond,
                            c.best_cond,
                            if c.fails { " FAILS" } else { "" }
                        );
                    }
                }
            }
        }
        let _ = writeln!(s, "\nblocks:");
        for b in &self.blocks {
            match b.kind {
                "assign" => {
                    let e = &b.eqs[0];
                    let _ = writeln!(
                        s,
                        "  [{}] {} := from {}",
                        b.index,
                        e.var.as_deref().unwrap_or("?"),
                        self.equations[e.eq].text
                    );
                }
                _ => {
                    let _ = writeln!(
                        s,
                        "  [{}] {} loop, {} unknowns{}: [{}]",
                        b.index,
                        b.kind,
                        b.size,
                        if b.torn {
                            format!(
                                ", torn: {} tears, {} inner, {} residuals",
                                b.tears.len(),
                                b.inner,
                                b.residuals
                            )
                        } else {
                            String::new()
                        },
                        b.vars.join(", ")
                    );
                    for e in &b.eqs {
                        let _ = writeln!(
                            s,
                            "      {} {}",
                            match &e.var {
                                Some(v) => format!("{v} :="),
                                None => "residual:".to_string(),
                            },
                            self.equations[e.eq].text
                        );
                    }
                }
            }
        }
        if let Some(j) = &self.jacobian {
            let _ = writeln!(s, "\nstate Jacobian df/dx at t = {}:", j.time);
            let n = j.states.len();
            let _ = writeln!(
                s,
                "  {n} x {n}, {} nonzeros ({:.1} %), cond_1 = {}",
                j.nnz,
                if n > 0 {
                    100.0 * j.nnz as f64 / (n * n) as f64
                } else {
                    0.0
                },
                match j.cond {
                    Some(c) => format!("{c:.3e}"),
                    None if j.singular => "singular".into(),
                    None => "not computed".into(),
                }
            );
            if let Some(r) = j.stiffness_ratio {
                let _ = writeln!(s, "  stiffness ratio: {r:.3e}");
            }
            let _ = writeln!(
                s,
                "  suggested solver: {} ({})",
                j.suggested_solver, j.solver_reason
            );
            let _ = writeln!(s, "\n  modes (eigenvalue, time constant, top states):");
            for md in &j.modes {
                let ev = if md.im == 0.0 {
                    format!("{:.6e}", md.re)
                } else {
                    format!(
                        "{:.6e} {} {:.6e}i",
                        md.re,
                        if md.im < 0.0 { '-' } else { '+' },
                        md.im.abs()
                    )
                };
                let tc = md
                    .time_constant
                    .map(|t| format!("tau {t:.3e} s"))
                    .unwrap_or_else(|| {
                        if md.im == 0.0 {
                            "integrator"
                        } else {
                            "undamped"
                        }
                        .into()
                    });
                let p: Vec<String> = md
                    .participation
                    .iter()
                    .filter(|(_, f)| *f >= 0.05)
                    .map(|&(k, f)| format!("{} {:.0}%", j.states[k], 100.0 * f))
                    .collect();
                let _ = writeln!(s, "    {ev:<32} {tc:<16} {}", p.join(", "));
            }
            if !j.loops.is_empty() {
                let _ = writeln!(s, "\n  algebraic loops (Jacobian w.r.t. tear variables):");
                for l in &j.loops {
                    let _ = writeln!(
                        s,
                        "    block {}: {} unknowns, tears [{}], cond_1 = {}",
                        l.block,
                        l.size,
                        l.tears.join(", "),
                        if l.singular {
                            "singular".to_string()
                        } else {
                            format!("{:.3e}", l.cond)
                        }
                    );
                }
            }
            for n in &j.notes {
                let _ = writeln!(s, "  note: {n}");
            }
        }
        if let Some(e) = &self.jacobian_error {
            let _ = writeln!(s, "\nJacobian analysis failed: {e}");
        }
        for w in &self.warnings {
            let _ = writeln!(s, "warning: {w}");
        }
        s
    }
}

/// The modifications that make `overrides` part of the model:
/// `x(stateSelect = StateSelect.prefer)` per variable (the workbench's "copy as modifier").
pub fn as_modifiers(overrides: &[AppliedOverride]) -> String {
    overrides
        .iter()
        .map(|o| format!("{}(stateSelect = StateSelect.{})", o.var, tier_name(o.tier)))
        .collect::<Vec<_>>()
        .join(",\n")
}
