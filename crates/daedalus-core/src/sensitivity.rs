//! Local parameter sensitivities `dy(t)/dp` along a trajectory.
//!
//! Two methods:
//! - **exact**: the forward sensitivity equations `ds/dt = df/dx s + df/dp`, with
//!   `s(t0) = dx(t0)/dp` from a tangent run of the initialization, integrated in a
//!   staggered way next to the state: the simulator advances the state, and between
//!   two points the sensitivities take a step of the L-stable Radau IIA (order 3)
//!   method. `df/dx` and `df/dp` are exact forward-mode tangents of the compiled tape,
//!   with algebraic loops differentiated implicitly (as in [`crate::linearize`]).
//!   Outputs follow as `dy/dp = dg/dx s + dg/dp`, again by tangents.
//!   This needs a model without when-clauses and `delay()`, and a run without events
//!   (sensitivities jump at state events); parameters must be free (see
//!   [`Model::free_params`]): values folded at translation have no derivative.
//! - **fd**: central differences of two extra simulations per parameter with a
//!   relative perturbation, for every model.
//!
//! `auto` uses `exact` when it applies and `fd` otherwise.

use crate::codegen::{Model, Runtime};
use crate::expr::VarId;
use crate::sim::{SimOptions, Simulation};
use crate::uq::{Job, check_var, free_param, output_grid, run_outputs};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum SensMethod {
    /// Exact where possible, else finite differences.
    #[default]
    Auto,
    /// Forward sensitivity equations with tape tangents.
    Exact,
    /// Central differences of perturbed simulations.
    Fd,
}

impl SensMethod {
    pub fn parse(s: &str) -> Option<SensMethod> {
        Some(match s.to_ascii_lowercase().as_str() {
            "auto" => SensMethod::Auto,
            "exact" | "forward" | "ad" => SensMethod::Exact,
            "fd" | "cd" | "central" => SensMethod::Fd,
            _ => return None,
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SensOptions {
    pub params: Vec<String>,
    pub outputs: Vec<String>,
    #[serde(default)]
    pub method: SensMethod,
    /// Relative perturbation of the finite differences.
    #[serde(default = "default_rel_step")]
    pub rel_step: f64,
    /// Radau steps per output interval (exact method).
    #[serde(default = "default_substeps")]
    pub substeps: usize,
}

fn default_rel_step() -> f64 {
    1e-4
}
fn default_substeps() -> usize {
    2
}

impl SensOptions {
    pub fn new(params: Vec<String>, outputs: Vec<String>, method: SensMethod) -> Self {
        SensOptions {
            params,
            outputs,
            method,
            rel_step: default_rel_step(),
            substeps: default_substeps(),
        }
    }
}

/// Largest number of states for the exact method (its implicit steps use dense LU).
pub const EXACT_MAX_STATES: usize = 300;

#[derive(Clone, Debug, Serialize)]
pub struct RankEntry {
    pub param: String,
    /// Time average of |normalized sensitivity|, summed over the outputs.
    pub score: f64,
    /// The same per output.
    pub per_output: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Sensitivities {
    pub model: String,
    /// The method actually used.
    pub method: SensMethod,
    pub time: Vec<f64>,
    pub params: Vec<String>,
    pub param_values: Vec<f64>,
    pub outputs: Vec<String>,
    /// `values[o][t]`
    pub values: Vec<Vec<f64>>,
    /// `dydp[o][p][t]`
    pub dydp: Vec<Vec<Vec<f64>>>,
    /// `p / y * dy/dp`, with |y| floored at 1e-3 of its largest magnitude.
    pub normalized: Vec<Vec<Vec<f64>>>,
    /// Parameters by decreasing influence.
    pub ranking: Vec<RankEntry>,
    pub notes: Vec<String>,
}

/// Why the exact method cannot be used (the caller may fall back to differences).
#[derive(Debug)]
pub enum ExactError {
    NotApplicable(String),
    Failed(String),
}

/// Whether the exact method applies to the model (before running it).
pub fn exact_applicable(m: &Model) -> Result<(), String> {
    if !m.whens.is_empty() {
        return Err("the model has when-clauses".into());
    }
    if !m.delays.is_empty() {
        return Err("the model uses delay()".into());
    }
    if m.states.len() > EXACT_MAX_STATES {
        return Err(format!(
            "the model has {} states (the exact method handles up to {EXACT_MAX_STATES})",
            m.states.len()
        ));
    }
    Ok(())
}

/// Compute sensitivities with the requested method.
pub fn sensitivities(
    m: &Model,
    opts: &SimOptions,
    so: &SensOptions,
) -> Result<Sensitivities, String> {
    validate(m, so)?;
    match so.method {
        SensMethod::Fd => fd(m, opts, so, &mut |_, _| true),
        SensMethod::Exact => exact(m, opts, so).map_err(|e| match e {
            ExactError::NotApplicable(s) => format!(
                "exact sensitivities are not available: {s}; use the finite-difference method (fd)"
            ),
            ExactError::Failed(s) => s,
        }),
        SensMethod::Auto => match exact(m, opts, so) {
            Ok(s) => Ok(s),
            Err(ExactError::NotApplicable(why)) => {
                let mut s = fd(m, opts, so, &mut |_, _| true)?;
                s.notes
                    .insert(0, format!("finite differences used because {why}"));
                Ok(s)
            }
            Err(ExactError::Failed(e)) => Err(e),
        },
    }
}

fn validate(m: &Model, so: &SensOptions) -> Result<(Vec<VarId>, Vec<VarId>), String> {
    if so.params.is_empty() {
        return Err("no parameters given".into());
    }
    if so.outputs.is_empty() {
        return Err("no outputs given".into());
    }
    let p = so
        .params
        .iter()
        .map(|n| free_param(m, n))
        .collect::<Result<Vec<_>, _>>()?;
    let o = so
        .outputs
        .iter()
        .map(|n| check_var(m, n))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((p, o))
}

/// Tangents of parameters, start values and the initialization w.r.t. parameter `p`,
/// at the runtime's (initialized, consistent) point.
fn base_tangent(rt: &mut Runtime, p: VarId, fixed: &[bool]) -> Vec<f64> {
    let m = rt.m;
    let mut dr = vec![0.0; rt.r.len()];
    dr[p as usize] = 1.0;
    // Dependent parameters (in binding order); overridden and free ones are constants.
    for (q, seg) in &m.params {
        if *q == p || fixed[*q as usize] {
            continue;
        }
        rt.run_tangent(seg, &mut dr);
    }
    let saved = rt.r.clone();
    rt.r[m.layout.initial] = 1.0;
    rt.run_tangent(&m.starts, &mut dr);
    // Start values only depend on parameters: put the solution back before
    // differentiating the initial equations at it.
    rt.r.copy_from_slice(&saved);
    rt.r[m.layout.initial] = 1.0;
    rt.init_mode = true;
    rt.run_tangent(&m.init, &mut dr);
    rt.init_mode = false;
    rt.r.copy_from_slice(&saved);
    dr
}

/// Forward sensitivity equations (see the module documentation).
pub fn exact(m: &Model, opts: &SimOptions, so: &SensOptions) -> Result<Sensitivities, ExactError> {
    let fail = ExactError::Failed;
    let (pids, oids) = validate(m, so).map_err(fail)?;
    exact_applicable(m).map_err(ExactError::NotApplicable)?;
    let n = m.states.len();
    let np = pids.len();
    let mut o = opts.clone();
    o.store_events = false;
    let grid = output_grid(&o);
    let mut sim = Simulation::new(m, o.clone());
    sim.initialize().map_err(fail)?;
    sim.rt.run(&m.ode);
    sim.rt.run(&m.outputs);
    // Parameters whose value is not computed from others here: free or overridden.
    let mut fixed = vec![false; m.vars.len()];
    for &q in &m.free_params {
        fixed[q as usize] = true;
    }
    for (name, _) in &o.params {
        if let Some(i) = m.vars.iter().position(|v| &v.name == name) {
            fixed[i] = true;
        }
    }
    let param_values: Vec<f64> = pids.iter().map(|&p| sim.rt.r[p as usize]).collect();
    let bases: Vec<Vec<f64>> = pids
        .iter()
        .map(|&p| base_tangent(&mut sim.rt, p, &fixed))
        .collect();
    if let Some(e) = sim.rt.error.take() {
        return Err(fail(format!(
            "nonlinear system for [{}] did not converge while differentiating the initialization",
            m.newton[e.block].names.join(", ")
        )));
    }
    let mut s: Vec<Vec<f64>> = bases
        .iter()
        .map(|b| m.states.iter().map(|&x| b[x as usize]).collect())
        .collect();
    let nt = grid.len();
    let no = oids.len();
    let mut values = vec![Vec::with_capacity(nt); no];
    let mut dydp = vec![vec![Vec::with_capacity(nt); np]; no];
    let mut dr = vec![0.0; sim.rt.r.len()];

    // Outputs and their sensitivities at the current point.
    let record = |rt: &mut Runtime,
                  s: &[Vec<f64>],
                  dr: &mut Vec<f64>,
                  values: &mut Vec<Vec<f64>>,
                  dydp: &mut Vec<Vec<Vec<f64>>>| {
        rt.run(&m.ode);
        rt.run(&m.outputs);
        for (k, &v) in oids.iter().enumerate() {
            values[k].push(rt.r[v as usize]);
        }
        for j in 0..np {
            dr.copy_from_slice(&bases[j]);
            for (i, &x) in m.states.iter().enumerate() {
                dr[x as usize] = s[j][i];
            }
            rt.run_tangent(&m.ode, dr);
            rt.run_tangent(&m.outputs, dr);
            for (k, &v) in oids.iter().enumerate() {
                dydp[k][j].push(dr[v as usize]);
            }
        }
    };
    // Jacobians df/dx (row-major n x n) and df/dp (np x n) at the current point.
    let jacobians = |rt: &mut Runtime, dr: &mut Vec<f64>| -> (Vec<f64>, Vec<Vec<f64>>) {
        rt.run(&m.ode);
        let mut a = vec![0.0; n * n];
        for c in 0..n {
            dr.iter_mut().for_each(|x| *x = 0.0);
            dr[m.states[c] as usize] = 1.0;
            rt.run_tangent(&m.ode, dr);
            for (i, &d) in m.derivs.iter().enumerate() {
                a[i * n + c] = dr[d as usize];
            }
        }
        let b = (0..np)
            .map(|j| {
                dr.copy_from_slice(&bases[j]);
                for &x in &m.states {
                    dr[x as usize] = 0.0;
                }
                rt.run_tangent(&m.ode, dr);
                m.derivs.iter().map(|&d| dr[d as usize]).collect()
            })
            .collect();
        (a, b)
    };

    record(&mut sim.rt, &s, &mut dr, &mut values, &mut dydp);
    let mut st = sim.stepper();
    let sub = so.substeps.max(1);
    let events0 = sim.stats.events;
    let mut t = grid[0];
    let mut mat = vec![0.0; 4 * n * n];
    let mut piv = vec![0usize; 2 * n];
    let mut rhs = vec![0.0; 2 * n];
    // Radau IIA, two stages: c = (1/3, 1), A = [[5/12, -1/12], [3/4, 1/4]].
    let (a11, a12, a21, a22) = (5.0 / 12.0, -1.0 / 12.0, 0.75, 0.25);
    let mut reached = 1;
    'grid: for &tk in &grid[1..] {
        let h = (tk - t) / sub as f64;
        for q in 0..sub {
            let t0 = t + q as f64 * h;
            let t2 = if q + 1 == sub { tk } else { t0 + h };
            let h = t2 - t0;
            let mut stage = |tt: f64, sim: &mut Simulation, dr: &mut Vec<f64>| {
                sim.advance_to(&mut st, tt)?;
                if sim.terminated().is_some() {
                    return Ok(None);
                }
                Ok(Some(jacobians(&mut sim.rt, dr)))
            };
            let r1: Result<_, String> = stage(t0 + h / 3.0, &mut sim, &mut dr);
            let Some((j1, b1)) = r1.map_err(fail)? else {
                break 'grid;
            };
            let r2: Result<_, String> = stage(t2, &mut sim, &mut dr);
            let Some((j2, b2)) = r2.map_err(fail)? else {
                break 'grid;
            };
            if sim.stats.events != events0 {
                return Err(ExactError::NotApplicable(format!(
                    "an event occurred near t = {} (sensitivities jump at events)",
                    sim.rt.time()
                )));
            }
            if n > 0 {
                // (I - h [a11 J1, a12 J2; a21 J1, a22 J2]) Y = [s + h (a11 b1 + a12 b2); ...]
                let w = 2 * n;
                for r in 0..n {
                    for c in 0..n {
                        mat[r * w + c] = -h * a11 * j1[r * n + c];
                        mat[r * w + n + c] = -h * a12 * j2[r * n + c];
                        mat[(n + r) * w + c] = -h * a21 * j1[r * n + c];
                        mat[(n + r) * w + n + c] = -h * a22 * j2[r * n + c];
                    }
                }
                for i in 0..w {
                    mat[i * w + i] += 1.0;
                }
                if !crate::linalg::lu_factor(&mut mat, w, &mut piv) {
                    return Err(fail(format!(
                        "singular sensitivity step matrix at t = {t0} (step {h})"
                    )));
                }
                for j in 0..np {
                    for i in 0..n {
                        rhs[i] = s[j][i] + h * (a11 * b1[j][i] + a12 * b2[j][i]);
                        rhs[n + i] = s[j][i] + h * (a21 * b1[j][i] + a22 * b2[j][i]);
                    }
                    crate::linalg::lu_solve(&mat, w, &piv, &mut rhs);
                    s[j].copy_from_slice(&rhs[n..]);
                }
            }
        }
        t = tk;
        record(&mut sim.rt, &s, &mut dr, &mut values, &mut dydp);
        reached += 1;
        if let Some(e) = sim.rt.error.take() {
            return Err(fail(format!(
                "nonlinear system for [{}] did not converge at t = {t}",
                m.newton[e.block].names.join(", ")
            )));
        }
    }
    let time = grid[..reached].to_vec();
    let mut notes = Vec::new();
    if reached < nt {
        notes.push(format!("the model terminated at t = {}", time[reached - 1]));
    }
    if s.iter().flatten().any(|x| !x.is_finite()) {
        return Err(fail("the sensitivities are not finite".into()));
    }
    Ok(finish(
        m,
        SensMethod::Exact,
        time,
        so,
        param_values,
        values,
        dydp,
        notes,
    ))
}

/// Central differences: run `i` of `1 + 2 np` (0: nominal; 2j + 1, 2j + 2: p_j up, down).
fn fd_run(
    m: &Model,
    o: &SimOptions,
    so: &SensOptions,
    p0: &[f64],
    grid: &[f64],
    i: usize,
) -> Result<Vec<Vec<f64>>, String> {
    if i == 0 {
        return run_outputs(m, o, &[], &so.outputs, grid);
    }
    let j = (i - 1) / 2;
    let h = fd_step(p0[j], so.rel_step);
    let v = if i % 2 == 1 { p0[j] + h } else { p0[j] - h };
    run_outputs(m, o, &[(so.params[j].clone(), v)], &so.outputs, grid)
}

fn fd_step(p: f64, rel: f64) -> f64 {
    rel * p.abs().max(if p == 0.0 { 1.0 } else { 1e-12 })
}

/// Options of the difference runs: tolerance at most 1e-8 (solver noise is divided by
/// the perturbation).
fn fd_opts(opts: &SimOptions) -> SimOptions {
    let mut o = opts.clone();
    o.tolerance = o.tolerance.min(1e-8);
    o.store_events = false;
    o
}

fn fd(
    m: &Model,
    opts: &SimOptions,
    so: &SensOptions,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<Sensitivities, String> {
    validate(m, so)?;
    let o = fd_opts(opts);
    let grid = output_grid(&o);
    let p0 = crate::uq::param_values(m, &o, &so.params)?;
    let total = 1 + 2 * so.params.len();
    let mut runs = Vec::with_capacity(total);
    for i in 0..total {
        runs.push(fd_run(m, &o, so, &p0, &grid, i)?);
        if !progress(i + 1, total) {
            return Err("cancelled".into());
        }
    }
    Ok(fd_finish(m, so, &o, p0, grid, runs))
}

fn fd_finish(
    m: &Model,
    so: &SensOptions,
    o: &SimOptions,
    p0: Vec<f64>,
    grid: Vec<f64>,
    runs: Vec<Vec<Vec<f64>>>,
) -> Sensitivities {
    let np = so.params.len();
    let values = runs[0].clone();
    let dydp = (0..so.outputs.len())
        .map(|k| {
            (0..np)
                .map(|j| {
                    let h = fd_step(p0[j], so.rel_step);
                    runs[2 * j + 1][k]
                        .iter()
                        .zip(&runs[2 * j + 2][k])
                        .map(|(a, b)| (a - b) / (2.0 * h))
                        .collect()
                })
                .collect()
        })
        .collect();
    let notes = vec![format!(
        "central differences, relative step {:e}, tolerance {:e}",
        so.rel_step, o.tolerance
    )];
    finish(m, SensMethod::Fd, grid, so, p0, values, dydp, notes)
}

#[allow(clippy::too_many_arguments)]
fn finish(
    m: &Model,
    method: SensMethod,
    time: Vec<f64>,
    so: &SensOptions,
    param_values: Vec<f64>,
    values: Vec<Vec<f64>>,
    dydp: Vec<Vec<Vec<f64>>>,
    notes: Vec<String>,
) -> Sensitivities {
    let np = so.params.len();
    let normalized: Vec<Vec<Vec<f64>>> = values
        .iter()
        .zip(&dydp)
        .map(|(y, d)| {
            let ymax = y.iter().fold(0.0f64, |a, b| a.max(b.abs()));
            let floor = 1e-3 * ymax;
            (0..np)
                .map(|j| {
                    y.iter()
                        .zip(&d[j])
                        .map(|(&yv, &dv)| {
                            let den = if yv.abs() >= floor {
                                yv
                            } else if yv < 0.0 {
                                -floor
                            } else {
                                floor
                            };
                            if den == 0.0 {
                                0.0
                            } else {
                                param_values[j] * dv / den
                            }
                        })
                        .collect()
                })
                .collect()
        })
        .collect();
    let span = time.last().copied().unwrap_or(0.0) - time.first().copied().unwrap_or(0.0);
    let avg = |v: &[f64]| -> f64 {
        if v.len() < 2 || span <= 0.0 {
            return v.first().map_or(0.0, |x| x.abs());
        }
        time.windows(2)
            .zip(v.windows(2))
            .map(|(t, y)| 0.5 * (t[1] - t[0]) * (y[0].abs() + y[1].abs()))
            .sum::<f64>()
            / span
    };
    let mut ranking: Vec<RankEntry> = (0..np)
        .map(|j| {
            let per_output: Vec<f64> = normalized.iter().map(|o| avg(&o[j])).collect();
            RankEntry {
                param: so.params[j].clone(),
                score: per_output.iter().sum(),
                per_output,
            }
        })
        .collect();
    ranking.sort_by(|a, b| b.score.total_cmp(&a.score));
    Sensitivities {
        model: m.name.clone(),
        method,
        time,
        params: so.params.clone(),
        param_values,
        outputs: so.outputs.clone(),
        values,
        dydp,
        normalized,
        ranking,
        notes,
    }
}

impl Sensitivities {
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "sensitivities of {} ({}), {} points from t = {} to {}",
            self.model,
            match self.method {
                SensMethod::Exact => "exact: forward sensitivity equations with tape tangents",
                SensMethod::Fd => "central finite differences",
                SensMethod::Auto => "auto",
            },
            self.time.len(),
            self.time.first().copied().unwrap_or(0.0),
            self.time.last().copied().unwrap_or(0.0)
        );
        for n in &self.notes {
            let _ = writeln!(s, "  note: {n}");
        }
        let _ = writeln!(
            s,
            "\nranking (time average of |p/y dy/dp|, summed over outputs):"
        );
        for (i, r) in self.ranking.iter().enumerate() {
            let pv = self.params.iter().position(|p| *p == r.param).unwrap();
            let _ = writeln!(
                s,
                "  {:>2}. {:<28} {:>12.5e}   (p = {})",
                i + 1,
                r.param,
                r.score,
                self.param_values[pv]
            );
        }
        let last = self.time.len().saturating_sub(1);
        let _ = writeln!(
            s,
            "\nat t = {}:",
            self.time.get(last).copied().unwrap_or(0.0)
        );
        for (k, o) in self.outputs.iter().enumerate() {
            let _ = writeln!(
                s,
                "  {o} = {:.6e}",
                self.values[k].get(last).copied().unwrap_or(f64::NAN)
            );
            for (j, p) in self.params.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "    d/d{p:<24} {:>13.6e}   normalized {:>11.4e}",
                    self.dydp[k][j].get(last).copied().unwrap_or(f64::NAN),
                    self.normalized[k][j].get(last).copied().unwrap_or(f64::NAN)
                );
            }
        }
        s
    }

    /// CSV: time, then per output the value, its sensitivities and the normalized ones.
    pub fn to_csv(&self) -> String {
        use std::fmt::Write;
        let mut s = String::from("time");
        for o in &self.outputs {
            let _ = write!(s, ",\"{o}\"");
            for p in &self.params {
                let _ = write!(s, ",\"d({o})/d({p})\"");
            }
            for p in &self.params {
                let _ = write!(s, ",\"n({o})/n({p})\"");
            }
        }
        s.push('\n');
        for (i, t) in self.time.iter().enumerate() {
            let _ = write!(s, "{t}");
            for k in 0..self.outputs.len() {
                let _ = write!(s, ",{}", self.values[k][i]);
                for j in 0..self.params.len() {
                    let _ = write!(s, ",{}", self.dydp[k][j][i]);
                }
                for j in 0..self.params.len() {
                    let _ = write!(s, ",{}", self.normalized[k][j][i]);
                }
            }
            s.push('\n');
        }
        s
    }
}

/// Sensitivities as a [`Job`]: the exact method in one step, differences one run per
/// step.
pub struct SensJob {
    m: Model,
    opts: SimOptions,
    so: SensOptions,
    state: SensState,
    result: Option<Sensitivities>,
    note: Option<String>,
}

enum SensState {
    Start,
    Fd {
        o: SimOptions,
        p0: Vec<f64>,
        grid: Vec<f64>,
        runs: Vec<Vec<Vec<f64>>>,
    },
    Done,
}

impl SensJob {
    pub fn new(m: Model, opts: SimOptions, so: SensOptions) -> Result<SensJob, String> {
        validate(&m, &so)?;
        Ok(SensJob {
            m,
            opts,
            so,
            state: SensState::Start,
            result: None,
            note: None,
        })
    }

    pub fn result(&self) -> Option<&Sensitivities> {
        self.result.as_ref()
    }

    fn start_fd(&mut self) -> Result<(), String> {
        let o = fd_opts(&self.opts);
        let grid = output_grid(&o);
        let p0 = crate::uq::param_values(&self.m, &o, &self.so.params)?;
        self.state = SensState::Fd {
            o,
            p0,
            grid,
            runs: Vec::new(),
        };
        Ok(())
    }
}

impl Job for SensJob {
    fn step(&mut self) -> Result<bool, String> {
        match &mut self.state {
            SensState::Start => {
                if self.so.method == SensMethod::Fd {
                    self.start_fd()?;
                    return Ok(false);
                }
                match exact(&self.m, &self.opts, &self.so) {
                    Ok(s) => {
                        self.result = Some(s);
                        self.state = SensState::Done;
                        Ok(true)
                    }
                    Err(ExactError::NotApplicable(why)) if self.so.method == SensMethod::Auto => {
                        self.note = Some(format!("finite differences used because {why}"));
                        self.start_fd()?;
                        Ok(false)
                    }
                    Err(ExactError::NotApplicable(why)) => Err(format!(
                        "exact sensitivities are not available: {why}; use the finite-difference method (fd)"
                    )),
                    Err(ExactError::Failed(e)) => Err(e),
                }
            }
            SensState::Fd { o, p0, grid, runs } => {
                let total = 1 + 2 * self.so.params.len();
                let r = fd_run(&self.m, o, &self.so, p0, grid, runs.len())?;
                runs.push(r);
                if runs.len() < total {
                    return Ok(false);
                }
                let SensState::Fd { o, p0, grid, runs } =
                    std::mem::replace(&mut self.state, SensState::Done)
                else {
                    unreachable!()
                };
                let mut s = fd_finish(&self.m, &self.so, &o, p0, grid, runs);
                if let Some(n) = self.note.take() {
                    s.notes.insert(0, n);
                }
                self.result = Some(s);
                Ok(true)
            }
            SensState::Done => Ok(true),
        }
    }

    fn progress(&self) -> (usize, usize) {
        let total = 1 + 2 * self.so.params.len();
        match &self.state {
            SensState::Start => (0, 1),
            SensState::Fd { runs, .. } => (runs.len(), total),
            SensState::Done => (1, 1),
        }
    }

    fn result_json(&self) -> serde_json::Value {
        serde_json::to_value(&self.result).unwrap_or_default()
    }

    fn report(&self) -> String {
        self.result.as_ref().map(|s| s.report()).unwrap_or_default()
    }
}
