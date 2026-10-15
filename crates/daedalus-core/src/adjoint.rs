//! Adjoint sensitivities: the gradient of one scalar objective with respect to many
//! parameters at the cost of one forward and one backward pass.
//!
//! The objective is a weighted sum of quantities of interest (the [`Qoi`] syntax of
//! the global analysis), each of which is a functional of the trajectory
//!
//! ```text
//! J(p) = sum_k w_k y_k(tau_k)  +  sum_l w_l int_{a_l}^{b_l} y_l dt,   dx/dt = f(x, p, t)
//! ```
//!
//! with `y` any model variable: `final(y)` and `at(y, t)` are point terms, `integral`
//! and `mean` (an integral over the window divided by its length) are integral terms,
//! and `max` / `min` are point terms at the time of the extremum on the output grid
//! (differentiable almost everywhere). A target `w (q - c)^2` contributes
//! `2 w (q - c) dq/dp`.
//!
//! **Forward pass.** The model is simulated with the usual integrator. The states are
//! stored (checkpointed) at the nodes of a mesh (the output grid, plus the window ends
//! and point times, split into `substeps` pieces) and at the internal stage times of
//! the backward method; the algebraic variables are recomputed from them when needed.
//!
//! **Backward pass.** The adjoint `l(t)` solves `-dl/dt = A^T l + g_x` from `l = 0` at
//! the last time the objective depends on, with jumps `l += w dy/dx` at point terms,
//! and the parameter gradient accumulates `int (f_p^T l + g_p) dt` plus the point terms'
//! direct `w dy/dp`. Time steps use the L-stable two-stage Radau IIA method (order 3)
//! backwards on the mesh, the quadrature its weights. Finally
//! `dJ/dp = acc . dr/dp + l(t0) . dx(t0)/dp`, where `dr/dp` are the tangents of the
//! parameters (including parameters computed from them) and of the initial states
//! (a tangent run of the initialization, as for forward sensitivities).
//!
//! The vector-Jacobian products `l^T df/dp`, `l^T df/dx` and `dy/d(x, p)` are single
//! **reverse-mode passes of the tape** ([`crate::tape_reverse`]): their cost does not
//! grow with the number of parameters. The implicit steps need the state Jacobian
//! `A = df/dx` itself, which comes from `n` forward tangent passes per stage (as in the
//! implicit integrators and the forward sensitivity method). Per backward step the cost
//! is thus `2 (n + 1..2)` tape passes and one LU of size `2n`; forward sensitivities
//! need `2 (n + np)` passes and `np` solves. The adjoint wins when there are many
//! parameters and one (or few) objectives.
//!
//! Like the exact forward method it needs a model without when-clauses and `delay()`,
//! and a run without events; otherwise [`GradMethod::Auto`] falls back to forward
//! sensitivities (exact or differences, see [`crate::sensitivity`]).

use crate::codegen::{Model, Runtime};
use crate::expr::VarId;
use crate::optimize::ObjTerm;
use crate::sensitivity::{
    ExactError, SensJob, SensMethod, SensOptions, Sensitivities, exact_applicable, param_bases,
    sensitivities,
};
use crate::sim::{SimOptions, Simulation};
use crate::tape_reverse::run_reverse;
use crate::uq::{Job, Qoi, QoiKind, check_var, free_param, interp, output_grid};
use serde::{Deserialize, Serialize};

/// How the gradient is computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GradMethod {
    /// Adjoint where it applies, else forward sensitivities.
    #[default]
    Auto,
    /// Backward adjoint solve with reverse-mode tape passes.
    Adjoint,
    /// Forward sensitivities (exact where they apply, else differences).
    Forward,
    /// Central differences.
    Fd,
}

impl GradMethod {
    pub fn parse(s: &str) -> Option<GradMethod> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" => GradMethod::Auto,
            "adjoint" | "reverse" => GradMethod::Adjoint,
            "forward" | "exact" => GradMethod::Forward,
            "fd" | "cd" | "central" => GradMethod::Fd,
            _ => return None,
        })
    }
}

/// Where a term of the objective reads its variable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TermKind {
    /// `y(t)`.
    Point(f64),
    /// `int_a^b y dt`.
    Integral { a: f64, b: f64 },
}

/// `weight * y(...)`.
#[derive(Clone, Debug)]
pub struct Term {
    pub var: VarId,
    pub weight: f64,
    pub kind: TermKind,
}

/// Work done by an adjoint gradient.
#[derive(Clone, Debug, Default, Serialize)]
pub struct AdjStats {
    pub states: usize,
    pub params: usize,
    /// Mesh nodes of the checkpointed forward run.
    pub nodes: usize,
    /// Backward Radau steps.
    pub steps: usize,
    /// Forward-mode tape passes (state Jacobians).
    pub tangent_passes: usize,
    /// Reverse-mode tape passes (vector-Jacobian products).
    pub reverse_passes: usize,
    pub forward_ms: f64,
    pub backward_ms: f64,
}

/// Outcome of [`adjoint_terms`].
#[derive(Clone, Debug)]
pub struct AdjCore {
    /// Output grid and the trajectories of the requested variables on it.
    pub grid: Vec<f64>,
    pub traj: Vec<Vec<f64>>,
    pub param_values: Vec<f64>,
    /// `d(sum of terms)/dp`.
    pub grad: Vec<f64>,
    pub stats: AdjStats,
}

fn now_ms() -> f64 {
    crate::profile::now() * 1000.0
}

/// Mesh: the grid with `extra` times inserted, every interval split into `sub` pieces.
/// Returns the nodes and, per node, its index in the grid.
fn mesh(grid: &[f64], extra: &[f64], sub: usize) -> (Vec<f64>, Vec<Option<usize>>) {
    let t0 = grid[0];
    let t1 = *grid.last().unwrap();
    let tol = 1e-10 * t1.abs().max(t0.abs()).max(1.0);
    let mut base: Vec<(f64, Option<usize>)> = grid
        .iter()
        .enumerate()
        .map(|(i, &t)| (t, Some(i)))
        .collect();
    for &e in extra {
        if !e.is_finite() || e <= t0 + tol || e >= t1 - tol {
            continue;
        }
        if base.iter().any(|(t, _)| (t - e).abs() <= tol) {
            continue;
        }
        base.push((e, None));
    }
    base.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut nodes = vec![base[0].0];
    let mut gi = vec![base[0].1];
    for w in base.windows(2) {
        let (a, b) = (w[0].0, w[1].0);
        for q in 1..sub {
            nodes.push(a + (b - a) * q as f64 / sub as f64);
            gi.push(None);
        }
        nodes.push(b);
        gi.push(w[1].1);
    }
    (nodes, gi)
}

/// Index of the mesh node at time `t` (the nearest one).
fn node_index(nodes: &[f64], t: f64) -> usize {
    let i = nodes.partition_point(|&x| x < t);
    if i == 0 {
        return 0;
    }
    if i >= nodes.len() {
        return nodes.len() - 1;
    }
    if (nodes[i] - t).abs() < (t - nodes[i - 1]).abs() {
        i
    } else {
        i - 1
    }
}

/// Put the runtime at `(t, x)` and solve the algebraic part.
fn set_point(rt: &mut Runtime, t: f64, x: &[f64]) {
    let m = rt.m;
    rt.set_time(t);
    rt.set_states(x);
    rt.run(&m.ode);
    rt.run(&m.outputs);
}

/// Reverse pass of outputs and right-hand side seeded with `l` on the derivatives and
/// `seeds` on variables; `adj` holds the adjoints of the inputs afterwards.
fn reverse(rt: &mut Runtime, adj: &mut [f64], l: Option<&[f64]>, seeds: &[(VarId, f64)]) {
    let m = rt.m;
    adj.iter_mut().for_each(|x| *x = 0.0);
    for &(v, w) in seeds {
        adj[v as usize] += w;
    }
    if let Some(l) = l {
        for (i, &d) in m.derivs.iter().enumerate() {
            adj[d as usize] += l[i];
        }
    }
    run_reverse(rt, &m.outputs, adj);
    run_reverse(rt, &m.ode, adj);
}

/// State Jacobian `df/dx` (row-major) at the runtime's point by forward tangents.
fn state_jacobian(rt: &mut Runtime, dr: &mut [f64], a: &mut [f64]) {
    let m = rt.m;
    let n = m.states.len();
    for c in 0..n {
        dr.iter_mut().for_each(|x| *x = 0.0);
        dr[m.states[c] as usize] = 1.0;
        rt.run_tangent(&m.ode, dr);
        for (i, &d) in m.derivs.iter().enumerate() {
            a[i * n + c] = dr[d as usize];
        }
    }
}

fn newton_error(rt: &mut Runtime, t: f64) -> Result<(), ExactError> {
    if let Some(e) = rt.error.take() {
        return Err(ExactError::Failed(format!(
            "nonlinear system for [{}] did not converge at t = {t}",
            rt.m.newton[e.block].names.join(", ")
        )));
    }
    Ok(())
}

/// The adjoint gradient of the terms returned by `terms_of(grid, trajectories)` (called
/// after the forward pass, so weights can depend on the solution) with respect to the
/// free parameters `params`. `vars` are the variables whose trajectories `terms_of`
/// sees; `extra` are times the mesh must contain (window ends, point times).
pub fn adjoint_terms(
    m: &Model,
    opts: &SimOptions,
    params: &[String],
    vars: &[String],
    extra: &[f64],
    substeps: usize,
    terms_of: &mut dyn FnMut(&[f64], &[Vec<f64>]) -> Result<Vec<Term>, String>,
) -> Result<AdjCore, ExactError> {
    let fail = ExactError::Failed;
    let pids: Vec<VarId> = params
        .iter()
        .map(|n| free_param(m, n))
        .collect::<Result<_, _>>()
        .map_err(fail)?;
    let vids: Vec<VarId> = vars
        .iter()
        .map(|n| check_var(m, n))
        .collect::<Result<_, _>>()
        .map_err(fail)?;
    exact_applicable(m).map_err(ExactError::NotApplicable)?;
    let n = m.states.len();
    let np = pids.len();
    let mut o = opts.clone();
    o.store_events = false;
    let grid = output_grid(&o);
    let (nodes, gidx) = mesh(&grid, extra, substeps.max(1));
    let nn = nodes.len();
    let mut stats = AdjStats {
        states: n,
        params: np,
        nodes: nn,
        ..Default::default()
    };

    // ---- forward pass with checkpoints
    let t_fw = now_ms();
    let mut sim = Simulation::new(m, o.clone());
    sim.initialize().map_err(fail)?;
    sim.rt.run(&m.ode);
    sim.rt.run(&m.outputs);
    let param_values: Vec<f64> = pids.iter().map(|&p| sim.rt.r[p as usize]).collect();
    let bases = param_bases(&mut sim, &pids, &o).map_err(fail)?;
    let mut traj = vec![vec![f64::NAN; grid.len()]; vids.len()];
    let mut xs_node = vec![vec![0.0; n]; nn];
    let mut xs_stage = vec![vec![0.0; n]; nn.saturating_sub(1)];
    let record = |rt: &Runtime, k: usize, xs: &mut Vec<Vec<f64>>, traj: &mut Vec<Vec<f64>>| {
        rt.get_states(&mut xs[k]);
        if let Some(g) = gidx[k] {
            for (j, &v) in vids.iter().enumerate() {
                traj[j][g] = rt.r[v as usize];
            }
        }
    };
    record(&sim.rt, 0, &mut xs_node, &mut traj);
    // Sample times: the stage time 2/3 into every interval, then its end.
    let mut times = Vec::with_capacity(2 * (nn - 1));
    for k in 0..nn - 1 {
        times.push(nodes[k] + 2.0 * (nodes[k + 1] - nodes[k]) / 3.0);
        times.push(nodes[k + 1]);
    }
    let mut st = sim.stepper();
    let mut newton_fail = None;
    let ev = sim
        .advance_sampled(&mut st, &times, &mut |i, rt| {
            if let Some(e) = rt.error.take() {
                newton_fail.get_or_insert((times[i], e.block));
            }
            let k = i / 2;
            if i % 2 == 0 {
                rt.get_states(&mut xs_stage[k]);
            } else {
                record(rt, k + 1, &mut xs_node, &mut traj);
            }
            Ok(())
        })
        .map_err(fail)?;
    if let Some(te) = ev {
        return Err(ExactError::NotApplicable(format!(
            "an event occurred near t = {te} (adjoints jump at events)"
        )));
    }
    if let Some((t, b)) = newton_fail {
        return Err(fail(format!(
            "nonlinear system for [{}] did not converge at t = {t}",
            m.newton[b].names.join(", ")
        )));
    }
    stats.forward_ms = now_ms() - t_fw;
    let terms = terms_of(&grid, &traj).map_err(fail)?;

    // ---- backward pass
    let t_bw = now_ms();
    let rt = &mut sim.rt;
    let nr = rt.r.len();
    let mut points: Vec<Vec<(VarId, f64)>> = vec![Vec::new(); nn];
    let mut integrals: Vec<(VarId, f64, f64, f64)> = Vec::new();
    let mut kmax = 0;
    for t in &terms {
        if t.weight == 0.0 || !t.weight.is_finite() {
            continue;
        }
        match t.kind {
            TermKind::Point(tau) => {
                let k = node_index(&nodes, tau);
                points[k].push((t.var, t.weight));
                kmax = kmax.max(k);
            }
            TermKind::Integral { a, b } => {
                if b > a {
                    integrals.push((t.var, t.weight, a, b));
                    kmax = kmax.max(node_index(&nodes, b));
                }
            }
        }
    }
    let mut lam = vec![0.0; n];
    let mut acc = vec![0.0; nr];
    let mut adj = vec![0.0; nr];
    let mut dr = vec![0.0; nr];
    let mut a1 = vec![0.0; n * n];
    let mut a2 = vec![0.0; n * n];
    let mut c1 = vec![0.0; n];
    let mut c2 = vec![0.0; n];
    let w = 2 * n;
    let mut mat = vec![0.0; w * w];
    let mut piv = vec![0usize; w];
    let mut rhs = vec![0.0; w];
    let (a11, a12, a21, a22) = (5.0 / 12.0, -1.0 / 12.0, 0.75, 0.25);
    let (b1, b2) = (0.75, 0.25);

    let apply_points = |k: usize,
                        rt: &mut Runtime,
                        lam: &mut [f64],
                        acc: &mut [f64],
                        adj: &mut [f64],
                        stats: &mut AdjStats|
     -> Result<(), ExactError> {
        if points[k].is_empty() {
            return Ok(());
        }
        set_point(rt, nodes[k], &xs_node[k]);
        newton_error(rt, nodes[k])?;
        reverse(rt, adj, None, &points[k]);
        stats.reverse_passes += 1;
        for (i, &s) in rt.m.states.iter().enumerate() {
            lam[i] += adj[s as usize];
        }
        for (x, &y) in acc.iter_mut().zip(adj.iter()) {
            *x += y;
        }
        Ok(())
    };

    for k in (1..=kmax).rev() {
        apply_points(k, rt, &mut lam, &mut acc, &mut adj, &mut stats)?;
        let (tl, th) = (nodes[k - 1], nodes[k]);
        let h = th - tl;
        let mid = 0.5 * (tl + th);
        let seeds: Vec<(VarId, f64)> = integrals
            .iter()
            .filter(|(_, _, a, b)| *a <= mid && mid <= *b)
            .map(|&(v, wt, ..)| (v, wt))
            .collect();
        // stage 1 at th - h/3, stage 2 at tl
        let mut snap = [Vec::new(), Vec::new()];
        for (si, (tt, x)) in [(th - h / 3.0, &xs_stage[k - 1]), (tl, &xs_node[k - 1])]
            .into_iter()
            .enumerate()
        {
            set_point(rt, tt, x);
            newton_error(rt, tt)?;
            let (a, c) = if si == 0 {
                (&mut a1, &mut c1)
            } else {
                (&mut a2, &mut c2)
            };
            state_jacobian(rt, &mut dr, a);
            stats.tangent_passes += n;
            c.iter_mut().for_each(|x| *x = 0.0);
            if !seeds.is_empty() {
                reverse(rt, &mut adj, None, &seeds);
                stats.reverse_passes += 1;
                for (i, &s) in rt.m.states.iter().enumerate() {
                    c[i] = adj[s as usize];
                }
                let bw = h * if si == 0 { b1 } else { b2 };
                for (x, &y) in acc.iter_mut().zip(adj.iter()) {
                    *x += bw * y;
                }
            }
            snap[si] = rt.r.clone();
        }
        if n > 0 {
            // (I - h [a11 A1^T, a12 A2^T; a21 A1^T, a22 A2^T]) Y = l + h (a c)
            for r in 0..n {
                for c in 0..n {
                    mat[r * w + c] = -h * a11 * a1[c * n + r];
                    mat[r * w + n + c] = -h * a12 * a2[c * n + r];
                    mat[(n + r) * w + c] = -h * a21 * a1[c * n + r];
                    mat[(n + r) * w + n + c] = -h * a22 * a2[c * n + r];
                }
            }
            for i in 0..w {
                mat[i * w + i] += 1.0;
            }
            if !crate::linalg::lu_factor(&mut mat, w, &mut piv) {
                return Err(fail(format!(
                    "singular adjoint step matrix at t = {tl} (step {h})"
                )));
            }
            for i in 0..n {
                rhs[i] = lam[i] + h * (a11 * c1[i] + a12 * c2[i]);
                rhs[n + i] = lam[i] + h * (a21 * c1[i] + a22 * c2[i]);
            }
            crate::linalg::lu_solve(&mat, w, &piv, &mut rhs);
            // quadrature of f_p^T l at the stages
            for (si, bw) in [(0usize, b1), (1, b2)] {
                rt.r.copy_from_slice(&snap[si]);
                let y = &rhs[si * n..(si + 1) * n];
                reverse(rt, &mut adj, Some(y), &[]);
                stats.reverse_passes += 1;
                for (x, &v) in acc.iter_mut().zip(adj.iter()) {
                    *x += h * bw * v;
                }
            }
            lam.copy_from_slice(&rhs[n..]);
        }
        stats.steps += 1;
    }
    apply_points(0, rt, &mut lam, &mut acc, &mut adj, &mut stats)?;
    if lam.iter().any(|x| !x.is_finite()) {
        return Err(fail("the adjoint is not finite".into()));
    }
    let mut is_state = vec![false; nr];
    for &s in &m.states {
        is_state[s as usize] = true;
    }
    let grad: Vec<f64> = bases
        .iter()
        .map(|b| {
            let mut g = 0.0;
            for r in 0..nr {
                if !is_state[r] && b[r] != 0.0 {
                    g += acc[r] * b[r];
                }
            }
            for (i, &s) in m.states.iter().enumerate() {
                if b[s as usize] != 0.0 {
                    g += lam[i] * b[s as usize];
                }
            }
            g
        })
        .collect();
    if grad.iter().any(|x| !x.is_finite()) {
        return Err(fail("the adjoint gradient is not finite".into()));
    }
    stats.backward_ms = now_ms() - t_bw;
    Ok(AdjCore {
        grid,
        traj,
        param_values,
        grad,
        stats,
    })
}

// ---------------------------------------------------------------------------------
// Objectives made of quantities of interest
// ---------------------------------------------------------------------------------

/// The window of a QoI clipped to `[t0, t1]`.
fn window(q: &Qoi, t0: f64, t1: f64) -> (f64, f64) {
    let (a, b) = q.window.unwrap_or((f64::NEG_INFINITY, f64::INFINITY));
    (a.max(t0), b.min(t1))
}

/// Times a QoI's adjoint terms need as mesh nodes.
fn qoi_nodes(q: &Qoi, out: &mut Vec<f64>) {
    match q.kind {
        QoiKind::At(t) => out.push(t),
        _ => {
            if let Some((a, b)) = q.window {
                out.push(a);
                out.push(b);
            }
        }
    }
}

/// Time of the extremum of `y` on the grid within the QoI's window (ends included).
fn extremum_time(q: &Qoi, t: &[f64], y: &[f64], max: bool) -> f64 {
    let (a, b) = window(q, t[0], t[t.len() - 1]);
    let mut best = (a, interp(t, y, a));
    let better = |v: f64, w: f64| if max { v > w } else { v < w };
    for i in 0..t.len() {
        if t[i] > a && t[i] < b && better(y[i], best.1) {
            best = (t[i], y[i]);
        }
    }
    let yb = interp(t, y, b);
    if better(yb, best.1) {
        best = (b, yb);
    }
    best.0
}

/// The terms of `coef * q` on the trajectory `y` (sampled on `t`).
fn qoi_terms(q: &Qoi, var: VarId, t: &[f64], y: &[f64], coef: f64, out: &mut Vec<Term>) {
    let (t0, t1) = (t[0], t[t.len() - 1]);
    let (a, b) = window(q, t0, t1);
    let point = |tau: f64| Term {
        var,
        weight: coef,
        kind: TermKind::Point(tau),
    };
    match q.kind {
        QoiKind::At(tq) => out.push(point(tq.clamp(t0, t1))),
        QoiKind::Final => out.push(point(b)),
        QoiKind::Max => out.push(point(extremum_time(q, t, y, true))),
        QoiKind::Min => out.push(point(extremum_time(q, t, y, false))),
        QoiKind::Integral | QoiKind::Mean => {
            if b > a {
                let s = if q.kind == QoiKind::Mean {
                    1.0 / (b - a)
                } else {
                    1.0
                };
                out.push(Term {
                    var,
                    weight: coef * s,
                    kind: TermKind::Integral { a, b },
                });
            } else if q.kind == QoiKind::Mean {
                out.push(point(a));
            }
        }
    }
}

/// `dq/dp` from forward sensitivities of q's variable (`k`-th output of `s`).
fn qoi_grad_from_sens(q: &Qoi, s: &Sensitivities, k: usize) -> Vec<f64> {
    let t = &s.time;
    match q.kind {
        QoiKind::Max | QoiKind::Min => {
            let ts = extremum_time(q, t, &s.values[k], q.kind == QoiKind::Max);
            s.dydp[k].iter().map(|d| interp(t, d, ts)).collect()
        }
        // final, at, integral and mean are linear in the trajectory
        _ => s.dydp[k].iter().map(|d| q.eval(t, d)).collect(),
    }
}

/// What to differentiate.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GradSpec {
    pub params: Vec<String>,
    /// `sum weight * q` or `weight * (q - target)^2` (see [`ObjTerm`]).
    pub objective: Vec<ObjTerm>,
    #[serde(default)]
    pub method: GradMethod,
    /// Backward Radau steps per output interval.
    #[serde(default = "default_substeps")]
    pub substeps: usize,
}

fn default_substeps() -> usize {
    2
}

impl GradSpec {
    pub fn new(params: Vec<String>, objective: Vec<ObjTerm>, method: GradMethod) -> Self {
        GradSpec {
            params,
            objective,
            method,
            substeps: default_substeps(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Gradient {
    pub model: String,
    /// The method actually used.
    pub method: GradMethod,
    pub objective: String,
    pub value: f64,
    pub qois: Vec<String>,
    pub qoi_values: Vec<f64>,
    pub params: Vec<String>,
    pub param_values: Vec<f64>,
    /// `dJ/dp`.
    pub gradient: Vec<f64>,
    /// `p / J dJ/dp` (0 where J = 0).
    pub normalized: Vec<f64>,
    /// Parameters by decreasing |normalized| (by |p dJ/dp| when J = 0).
    pub ranking: Vec<usize>,
    pub notes: Vec<String>,
    pub stats: Option<AdjStats>,
}

/// Parsed objective: QoIs, their weights and targets, the variables they read.
struct Objective {
    terms: Vec<(Qoi, f64, Option<f64>)>,
    vars: Vec<String>,
    var_of: Vec<usize>,
}

impl Objective {
    fn new(m: &Model, obj: &[ObjTerm]) -> Result<Objective, String> {
        if obj.is_empty() {
            return Err("no objective given".into());
        }
        let mut terms = Vec::new();
        let mut vars: Vec<String> = Vec::new();
        let mut var_of = Vec::new();
        for o in obj {
            let q = Qoi::parse(&o.qoi)?;
            check_var(m, &q.var)?;
            let i = match vars.iter().position(|v| *v == q.var) {
                Some(i) => i,
                None => {
                    vars.push(q.var.clone());
                    vars.len() - 1
                }
            };
            var_of.push(i);
            terms.push((q, o.weight, o.target));
        }
        Ok(Objective {
            terms,
            vars,
            var_of,
        })
    }

    fn label(&self) -> String {
        if self.terms.len() == 1 && self.terms[0].1 == 1.0 && self.terms[0].2.is_none() {
            return self.terms[0].0.label();
        }
        self.terms
            .iter()
            .map(|(q, w, t)| match t {
                Some(t) => format!("{w} * ({} - {t})^2", q.label()),
                None => format!("{w} * {}", q.label()),
            })
            .collect::<Vec<_>>()
            .join(" + ")
    }

    /// QoI values, objective value and the chain-rule coefficients `dJ/dq`.
    fn values(&self, t: &[f64], traj: &[Vec<f64>]) -> (Vec<f64>, f64, Vec<f64>) {
        let mut qs = Vec::new();
        let mut f = 0.0;
        let mut coef = Vec::new();
        for (i, (q, w, target)) in self.terms.iter().enumerate() {
            let v = q.eval(t, &traj[self.var_of[i]]);
            qs.push(v);
            match target {
                Some(c) => {
                    f += w * (v - c).powi(2);
                    coef.push(2.0 * w * (v - c));
                }
                None => {
                    f += w * v;
                    coef.push(*w);
                }
            }
        }
        (qs, f, coef)
    }

    fn nodes(&self) -> Vec<f64> {
        let mut v = Vec::new();
        for (q, ..) in &self.terms {
            qoi_nodes(q, &mut v);
        }
        v
    }
}

/// Gradient of the objective by the adjoint method (errors say whether it does not apply).
fn by_adjoint(
    m: &Model,
    opts: &SimOptions,
    spec: &GradSpec,
    obj: &Objective,
) -> Result<Gradient, ExactError> {
    let mut qv = (Vec::new(), 0.0);
    let core = adjoint_terms(
        m,
        opts,
        &spec.params,
        &obj.vars,
        &obj.nodes(),
        spec.substeps,
        &mut |t, traj| {
            let (qs, f, coef) = obj.values(t, traj);
            qv = (qs, f);
            let mut terms = Vec::new();
            for (i, (q, ..)) in obj.terms.iter().enumerate() {
                let var = check_var(m, &q.var)?;
                qoi_terms(q, var, t, &traj[obj.var_of[i]], coef[i], &mut terms);
            }
            Ok(terms)
        },
    )?;
    let s = &core.stats;
    let notes = vec![format!(
        "adjoint: {} backward Radau IIA steps over {} mesh nodes; {} reverse and {} tangent tape passes \
         (forward {:.1} ms, backward {:.1} ms)",
        s.steps, s.nodes, s.reverse_passes, s.tangent_passes, s.forward_ms, s.backward_ms
    )];
    Ok(finish(
        m,
        GradMethod::Adjoint,
        spec,
        obj,
        qv.0,
        qv.1,
        core.param_values,
        core.grad,
        notes,
        Some(core.stats),
    ))
}

/// Gradient of the objective from forward sensitivities `s` of the objective variables.
fn from_sens(
    m: &Model,
    spec: &GradSpec,
    obj: &Objective,
    s: &Sensitivities,
    mut notes: Vec<String>,
) -> Gradient {
    let (qs, f, coef) = obj.values(&s.time, &s.values);
    let mut g = vec![0.0; spec.params.len()];
    for (i, (q, ..)) in obj.terms.iter().enumerate() {
        let d = qoi_grad_from_sens(q, s, obj.var_of[i]);
        for j in 0..g.len() {
            g[j] += coef[i] * d[j];
        }
    }
    notes.extend(s.notes.iter().cloned());
    let method = match s.method {
        SensMethod::Fd => GradMethod::Fd,
        _ => GradMethod::Forward,
    };
    finish(
        m,
        method,
        spec,
        obj,
        qs,
        f,
        s.param_values.clone(),
        g,
        notes,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
fn finish(
    m: &Model,
    method: GradMethod,
    spec: &GradSpec,
    obj: &Objective,
    qoi_values: Vec<f64>,
    value: f64,
    param_values: Vec<f64>,
    gradient: Vec<f64>,
    notes: Vec<String>,
    stats: Option<AdjStats>,
) -> Gradient {
    let normalized: Vec<f64> = gradient
        .iter()
        .zip(&param_values)
        .map(|(g, p)| if value != 0.0 { p * g / value } else { 0.0 })
        .collect();
    let mut ranking: Vec<usize> = (0..gradient.len()).collect();
    let key = |i: usize| {
        if value != 0.0 {
            normalized[i].abs()
        } else {
            (param_values[i] * gradient[i]).abs()
        }
    };
    ranking.sort_by(|&a, &b| key(b).total_cmp(&key(a)));
    Gradient {
        model: m.name.clone(),
        method,
        objective: obj.label(),
        value,
        qois: obj.terms.iter().map(|t| t.0.label()).collect(),
        qoi_values,
        params: spec.params.clone(),
        param_values,
        gradient,
        normalized,
        ranking,
        notes,
        stats,
    }
}

fn sens_options(spec: &GradSpec, obj: &Objective, method: SensMethod) -> SensOptions {
    SensOptions::new(spec.params.clone(), obj.vars.clone(), method)
}

fn validate(m: &Model, spec: &GradSpec) -> Result<Objective, String> {
    if spec.params.is_empty() {
        return Err("no parameters given".into());
    }
    for p in &spec.params {
        free_param(m, p)?;
    }
    Objective::new(m, &spec.objective)
}

/// The gradient of the objective with the requested method.
pub fn gradient(m: &Model, opts: &SimOptions, spec: &GradSpec) -> Result<Gradient, String> {
    let obj = validate(m, spec)?;
    let forward = |method: SensMethod, notes: Vec<String>| -> Result<Gradient, String> {
        let s = sensitivities(m, opts, &sens_options(spec, &obj, method))?;
        Ok(from_sens(m, spec, &obj, &s, notes))
    };
    match spec.method {
        GradMethod::Adjoint => by_adjoint(m, opts, spec, &obj).map_err(|e| match e {
            ExactError::NotApplicable(s) => format!(
                "adjoint sensitivities are not available: {s}; use the forward method (forward | fd)"
            ),
            ExactError::Failed(s) => s,
        }),
        GradMethod::Forward => forward(SensMethod::Auto, Vec::new()),
        GradMethod::Fd => forward(SensMethod::Fd, Vec::new()),
        GradMethod::Auto => match by_adjoint(m, opts, spec, &obj) {
            Ok(g) => Ok(g),
            Err(ExactError::NotApplicable(why)) => forward(
                SensMethod::Auto,
                vec![format!("forward sensitivities used because {why}")],
            ),
            Err(ExactError::Failed(e)) => forward(
                SensMethod::Auto,
                vec![format!(
                    "forward sensitivities used because the adjoint method failed: {e}"
                )],
            ),
        },
    }
}

impl Gradient {
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let how = match self.method {
            GradMethod::Adjoint => "adjoint: backward solve with reverse-mode tape passes",
            GradMethod::Forward => "forward sensitivity equations",
            GradMethod::Fd => "central finite differences",
            GradMethod::Auto => "auto",
        };
        let _ = writeln!(
            s,
            "gradient of {} for {} ({how})",
            self.objective, self.model
        );
        for n in &self.notes {
            let _ = writeln!(s, "  note: {n}");
        }
        let _ = writeln!(s, "\n  J = {:.10e}", self.value);
        if self.qois.len() > 1 {
            for (q, v) in self.qois.iter().zip(&self.qoi_values) {
                let _ = writeln!(s, "  {q} = {v:.10e}");
            }
        }
        let _ = writeln!(
            s,
            "\n  {:<28} {:>14} {:>16} {:>12}",
            "parameter", "value", "dJ/dp", "p/J dJ/dp"
        );
        for &i in &self.ranking {
            let _ = writeln!(
                s,
                "  {:<28} {:>14.6e} {:>16.8e} {:>12.4e}",
                self.params[i], self.param_values[i], self.gradient[i], self.normalized[i]
            );
        }
        s
    }

    pub fn to_csv(&self) -> String {
        let mut s = String::from("parameter,value,gradient,normalized\n");
        for i in 0..self.params.len() {
            s.push_str(&format!(
                "\"{}\",{},{},{}\n",
                self.params[i], self.param_values[i], self.gradient[i], self.normalized[i]
            ));
        }
        s
    }
}

/// The gradient as a [`Job`]: the adjoint in one step; a fallback to forward
/// sensitivities steps through their job (one run per step for differences).
pub struct GradJob {
    m: Model,
    opts: SimOptions,
    spec: GradSpec,
    obj: Objective,
    sens: Option<(SensJob, Vec<String>)>,
    result: Option<Gradient>,
}

impl GradJob {
    pub fn new(m: Model, opts: SimOptions, spec: GradSpec) -> Result<GradJob, String> {
        let obj = validate(&m, &spec)?;
        Ok(GradJob {
            m,
            opts,
            spec,
            obj,
            sens: None,
            result: None,
        })
    }

    pub fn result(&self) -> Option<&Gradient> {
        self.result.as_ref()
    }

    fn start_sens(&mut self, method: SensMethod, notes: Vec<String>) -> Result<(), String> {
        let so = sens_options(&self.spec, &self.obj, method);
        self.sens = Some((SensJob::new(self.m.clone(), self.opts.clone(), so)?, notes));
        Ok(())
    }
}

impl Job for GradJob {
    fn step(&mut self) -> Result<bool, String> {
        if self.result.is_some() {
            return Ok(true);
        }
        if let Some((job, notes)) = &mut self.sens {
            if job.step()? {
                let s = job.result().unwrap();
                self.result = Some(from_sens(&self.m, &self.spec, &self.obj, s, notes.clone()));
                return Ok(true);
            }
            return Ok(false);
        }
        match self.spec.method {
            GradMethod::Forward => self.start_sens(SensMethod::Auto, Vec::new())?,
            GradMethod::Fd => self.start_sens(SensMethod::Fd, Vec::new())?,
            GradMethod::Adjoint | GradMethod::Auto => {
                match by_adjoint(&self.m, &self.opts, &self.spec, &self.obj) {
                    Ok(g) => {
                        self.result = Some(g);
                        return Ok(true);
                    }
                    Err(e) if self.spec.method == GradMethod::Auto => {
                        let why = match e {
                            ExactError::NotApplicable(w) => {
                                format!("forward sensitivities used because {w}")
                            }
                            ExactError::Failed(w) => format!(
                                "forward sensitivities used because the adjoint method failed: {w}"
                            ),
                        };
                        self.start_sens(SensMethod::Auto, vec![why])?;
                    }
                    Err(ExactError::NotApplicable(w)) => {
                        return Err(format!(
                            "adjoint sensitivities are not available: {w}; use the forward method"
                        ));
                    }
                    Err(ExactError::Failed(w)) => return Err(w),
                }
            }
        }
        Ok(false)
    }

    fn progress(&self) -> (usize, usize) {
        if self.result.is_some() {
            return (1, 1);
        }
        match &self.sens {
            Some((j, _)) => j.progress(),
            None => (0, 1),
        }
    }

    fn result_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.result).unwrap_or_default()
    }

    fn report(&self) -> String {
        self.result.as_ref().map(|g| g.report()).unwrap_or_default()
    }
}

/// Objective value, QoI values and gradient for the optimizer: adjoint where it applies,
/// else forward sensitivities. Returns the method used.
pub fn objective_gradient(
    m: &Model,
    opts: &SimOptions,
    params: &[String],
    objective: &[ObjTerm],
    method: GradMethod,
) -> Result<Gradient, String> {
    let spec = GradSpec::new(params.to_vec(), objective.to_vec(), method);
    gradient(m, opts, &spec)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mesh_inserts_and_splits() {
        let g = [0.0, 1.0, 2.0];
        let (n, gi) = mesh(&g, &[0.5, 1.0, 3.0], 2);
        assert_eq!(n, vec![0.0, 0.25, 0.5, 0.75, 1.0, 1.5, 2.0]);
        assert_eq!(gi[4], Some(1));
        assert_eq!(gi[2], None);
        assert_eq!(node_index(&n, 0.5), 2);
        assert_eq!(node_index(&n, 2.0), 6);
    }
}
