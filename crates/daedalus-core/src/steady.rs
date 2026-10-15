//! Steady states and trim: find the states `x` with `der(x) = 0` (an equilibrium of
//! the model at a fixed time), optionally solving at the same time for chosen inputs or
//! parameters ("free" variables) so that chosen variables reach target values (trim).
//!
//! The unknowns are `z = [x; p]` (states, then free variables) and the residuals
//! `F(z) = [der(x); y - y_target]`; the system must be square (as many targets as free
//! variables). It is solved by a damped Newton iteration whose Jacobian is exact: a
//! forward-mode (tangent) run of the compiled tape per unknown, the same machinery as
//! [`crate::linearize`]. When the Jacobian is singular (for example a state that no
//! equation pins down, like the position of a free mass), the step is a
//! Levenberg–Marquardt step instead, which leaves such directions where they are.
//!
//! The iteration starts from the initial state, or from the state reached by
//! simulating to a given time. Time stays fixed at that point, relations (`if`
//! conditions) are held at their latched values while Newton iterates and re-latched
//! after convergence (the solve is repeated when one of them changed), and
//! when-clauses are not executed.

use crate::codegen::{Model, Runtime};
use crate::expr::VarId;
use crate::flat::VarKind;
use crate::linearize::{real_inputs, real_outputs};
use crate::sim::{SimOptions, Simulation};
use serde::Serialize;

/// What to solve for.
#[derive(Clone, Debug)]
pub struct SteadyOptions {
    /// Simulate to this time first and start from the state reached there
    /// (default: start from the initial state, at the start time).
    pub at_time: Option<f64>,
    /// Inputs or parameters to solve for (trim).
    pub free: Vec<String>,
    /// `(variable, value)` pairs the free variables must achieve.
    pub targets: Vec<(String, f64)>,
    /// Convergence tolerance on the scaled residual.
    pub tolerance: f64,
    pub max_iterations: usize,
}

impl Default for SteadyOptions {
    fn default() -> Self {
        SteadyOptions {
            at_time: None,
            free: Vec::new(),
            targets: Vec::new(),
            tolerance: 1e-10,
            max_iterations: 100,
        }
    }
}

/// A named value.
#[derive(Clone, Debug, Serialize)]
pub struct NamedValue {
    pub name: String,
    pub value: f64,
}

/// A trim target and the value reached.
#[derive(Clone, Debug, Serialize)]
pub struct TargetValue {
    pub name: String,
    pub target: f64,
    pub value: f64,
}

/// The result of a steady-state / trim computation.
#[derive(Clone, Debug, Serialize)]
pub struct SteadyState {
    pub model: String,
    pub time: f64,
    pub states: Vec<NamedValue>,
    /// `der(x)` at the solution (zero up to the tolerance).
    pub derivatives: Vec<NamedValue>,
    /// The free inputs / parameters at the solution.
    pub free: Vec<NamedValue>,
    pub targets: Vec<TargetValue>,
    pub inputs: Vec<NamedValue>,
    pub outputs: Vec<NamedValue>,
    pub iterations: usize,
    /// Largest |der(x)| or |y - target| at the solution.
    pub residual: f64,
    /// Eigenvalues of the state Jacobian at the solution, `(re, im)`.
    pub eigenvalues: Vec<(f64, f64)>,
    /// All eigenvalues have negative real part.
    pub stable: bool,
}

/// Initialize `m` (and simulate to `sopts.at_time` if given), then solve for a steady
/// state there.
pub fn steady_state(
    m: &Model,
    opts: SimOptions,
    sopts: &SteadyOptions,
) -> Result<SteadyState, String> {
    let t = sopts.at_time.unwrap_or(opts.start_time);
    if t < opts.start_time {
        return Err(format!(
            "time {t} is before the start time {}",
            opts.start_time
        ));
    }
    let mut o = opts;
    o.stop_time = o.stop_time.max(t);
    let mut sim = Simulation::new(m, o);
    sim.initialize()?;
    if t > sim.opts.start_time {
        let mut st = sim.stepper();
        sim.advance_to(&mut st, t)?;
        if let Some(msg) = sim.terminated() {
            return Err(format!(
                "the model terminated at t = {} before t = {t}: {msg}",
                sim.rt.time()
            ));
        }
    }
    let overrides: Vec<String> = sim.opts.params.iter().map(|(n, _)| n.clone()).collect();
    steady_state_at(&mut sim.rt, &overrides, sopts)
}

/// Solve for a steady state from the runtime's current (initialized) point. `overrides`
/// names the parameters whose values were set from outside (they are not recomputed
/// from their bindings when a free parameter changes).
pub fn steady_state_at(
    rt: &mut Runtime,
    overrides: &[String],
    sopts: &SteadyOptions,
) -> Result<SteadyState, String> {
    let m = rt.m;
    let find = |name: &str| -> Result<VarId, String> {
        m.vars
            .iter()
            .position(|v| v.name == name)
            .map(|i| i as VarId)
            .ok_or_else(|| format!("no variable '{name}' in {}", m.name))
    };
    let mut free = Vec::new();
    for f in &sopts.free {
        let v = find(f)?;
        let var = &m.vars[v as usize];
        if !matches!(var.kind, VarKind::Input | VarKind::Parameter) || m.states.contains(&v) {
            return Err(format!(
                "'{f}' cannot be freed: only inputs and parameters can (states are always solved for)"
            ));
        }
        free.push(v);
    }
    let mut targets = Vec::new();
    for (t, val) in &sopts.targets {
        targets.push((find(t)?, *val));
    }
    if free.len() != targets.len() {
        return Err(format!(
            "trim needs as many targets as free variables ({} free, {} targets)",
            free.len(),
            targets.len()
        ));
    }
    // Parameters computed from the free ones are re-evaluated with them.
    let free_param = free
        .iter()
        .any(|&v| m.vars[v as usize].kind == VarKind::Parameter);
    let param_segs: Vec<usize> = if free_param {
        (0..m.params.len())
            .filter(|&i| {
                let p = m.params[i].0;
                !free.contains(&p) && !overrides.iter().any(|o| *o == m.vars[p as usize].name)
            })
            .collect()
    } else {
        Vec::new()
    };
    let unknowns: Vec<VarId> = m
        .states
        .iter()
        .copied()
        .chain(free.iter().copied())
        .collect();
    let n = unknowns.len();
    let solver = Solver {
        m,
        unknowns: &unknowns,
        targets: &targets,
        param_segs: &param_segs,
    };
    let mut z: Vec<f64> = unknowns.iter().map(|&v| rt.r[v as usize]).collect();
    let mut iterations = 0;
    let mut latched = 0;
    loop {
        let it = solver.newton(rt, &mut z, sopts)?;
        iterations += it;
        // Conditions frozen during the iteration: re-latch them at the solution.
        if !rt.latch_relations() || latched >= 5 {
            break;
        }
        latched += 1;
    }
    let f = solver.eval(rt, &z);
    let residual = f.iter().fold(0.0f64, |a, x| a.max(x.abs()));
    let jac = solver.jacobian(rt);
    let named = |ids: &[VarId]| -> Vec<NamedValue> {
        ids.iter()
            .map(|&v| NamedValue {
                name: m.vars[v as usize].name.clone(),
                value: rt.r[v as usize] + 0.0,
            })
            .collect()
    };
    let states = named(&m.states);
    let derivatives = m
        .states
        .iter()
        .zip(&m.derivs)
        .map(|(&s, &d)| NamedValue {
            name: format!("der({})", m.vars[s as usize].name),
            value: rt.r[d as usize] + 0.0,
        })
        .collect();
    // Stability: eigenvalues of d der(x) / dx.
    let ns = m.states.len();
    let a: Vec<Vec<f64>> = (0..ns)
        .map(|i| (0..ns).map(|j| jac[i * n + j]).collect())
        .collect();
    let mut eigenvalues = crate::linearize::eigenvalues(&a);
    eigenvalues.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
    let stable = eigenvalues.iter().all(|e| e.0 < 0.0);
    Ok(SteadyState {
        model: m.name.clone(),
        time: rt.time(),
        states,
        derivatives,
        free: named(&free),
        targets: targets
            .iter()
            .map(|&(v, t)| TargetValue {
                name: m.vars[v as usize].name.clone(),
                target: t,
                value: rt.r[v as usize],
            })
            .collect(),
        inputs: named(&real_inputs(m)),
        outputs: named(&real_outputs(m)),
        iterations,
        residual,
        eigenvalues,
        stable,
    })
}

struct Solver<'a> {
    m: &'a Model,
    unknowns: &'a [VarId],
    targets: &'a [(VarId, f64)],
    param_segs: &'a [usize],
}

impl Solver<'_> {
    /// Residuals at `z` (non-finite if an algebraic loop failed).
    fn eval(&self, rt: &mut Runtime, z: &[f64]) -> Vec<f64> {
        let m = self.m;
        for (&v, &x) in self.unknowns.iter().zip(z) {
            rt.r[v as usize] = x;
        }
        for &i in self.param_segs {
            rt.run(&m.params[i].1);
        }
        rt.error = None;
        rt.run(&m.ode);
        rt.run(&m.outputs);
        let failed = rt.error.take().is_some();
        let mut f: Vec<f64> = m.derivs.iter().map(|&d| rt.r[d as usize]).collect();
        f.extend(self.targets.iter().map(|&(v, t)| rt.r[v as usize] - t));
        if failed {
            f.iter_mut().for_each(|x| *x = f64::NAN);
        }
        f
    }

    /// Row-major `n x n` Jacobian dF/dz at the point last evaluated.
    fn jacobian(&self, rt: &mut Runtime) -> Vec<f64> {
        let m = self.m;
        let n = self.unknowns.len();
        let mut jac = vec![0.0; n * n];
        let mut dr = vec![0.0; rt.r.len()];
        let saved = rt.r.clone();
        for (j, &s) in self.unknowns.iter().enumerate() {
            dr.iter_mut().for_each(|x| *x = 0.0);
            dr[s as usize] = 1.0;
            for &i in self.param_segs {
                rt.run_tangent(&m.params[i].1, &mut dr);
            }
            rt.run_tangent(&m.ode, &mut dr);
            rt.run_tangent(&m.outputs, &mut dr);
            let col = m
                .derivs
                .iter()
                .copied()
                .chain(self.targets.iter().map(|t| t.0));
            for (i, v) in col.enumerate() {
                jac[i * n + j] = dr[v as usize];
            }
        }
        rt.r.copy_from_slice(&saved);
        if jac.iter().any(|x| !x.is_finite()) {
            // Central differences as a fallback.
            let z: Vec<f64> = self.unknowns.iter().map(|&v| rt.r[v as usize]).collect();
            for j in 0..n {
                let h = 6e-6 * z[j].abs().max(1.0);
                let mut zp = z.clone();
                zp[j] += h;
                let fp = self.eval(rt, &zp);
                zp[j] -= 2.0 * h;
                let fm = self.eval(rt, &zp);
                for i in 0..n {
                    jac[i * n + j] = (fp[i] - fm[i]) / (2.0 * h);
                }
            }
            self.eval(rt, &z);
        }
        jac
    }

    /// Backtracking along `dz` until the weighted residual norm decreases; moves `z` and
    /// returns the new residuals.
    fn line_search(
        &self,
        rt: &mut Runtime,
        z: &mut [f64],
        dz: &[f64],
        n0: f64,
        norm2: &dyn Fn(&[f64]) -> f64,
    ) -> Option<Vec<f64>> {
        if dz.iter().any(|x| !x.is_finite()) {
            return None;
        }
        let mut lambda = 1.0;
        while lambda >= 1e-4 {
            let zt: Vec<f64> = z.iter().zip(dz).map(|(a, b)| a + lambda * b).collect();
            let ft = self.eval(rt, &zt);
            if ft.iter().all(|x| x.is_finite()) && norm2(&ft) <= (1.0 - 1e-4 * lambda) * n0 {
                z.copy_from_slice(&zt);
                return Some(ft);
            }
            lambda *= 0.5;
        }
        None
    }

    /// Damped Newton from `z`; returns the number of iterations.
    fn newton(&self, rt: &mut Runtime, z: &mut [f64], o: &SteadyOptions) -> Result<usize, String> {
        let n = z.len();
        let mut f = self.eval(rt, z);
        if f.iter().any(|x| !x.is_finite()) {
            return Err("the model cannot be evaluated at the starting point".into());
        }
        if n == 0 {
            return Ok(0);
        }
        let mut mu = 1e-6f64;
        for it in 0..o.max_iterations {
            let jac = self.jacobian(rt);
            let w = row_weights(&jac, z);
            let m0 = f
                .iter()
                .zip(&w)
                .fold(0.0f64, |a, (x, w)| a.max((x * w).abs()));
            if m0 <= o.tolerance {
                return Ok(it);
            }
            let norm2 = |f: &[f64]| f.iter().zip(&w).map(|(x, w)| (x * w).powi(2)).sum::<f64>();
            let n0 = norm2(&f);
            // Damped Newton step: backtracking on the scaled residual norm.
            let mut lu = jac.clone();
            let mut piv = vec![0; n];
            if crate::linalg::lu_factor(&mut lu, n, &mut piv) {
                let mut dz: Vec<f64> = f.iter().map(|x| -x).collect();
                crate::linalg::lu_solve(&lu, n, &piv, &mut dz);
                if let Some(ft) = self.line_search(rt, z, &dz, n0, &norm2) {
                    f = ft;
                    continue;
                }
            }
            // Singular Jacobian or no descent: Levenberg–Marquardt steps, growing the
            // damping until one reduces the residual.
            let mut accepted = false;
            while mu <= 1e10 {
                let dz = lm_step(&jac, &f, &w, n, mu);
                if dz.iter().all(|x| x.is_finite()) {
                    let zt: Vec<f64> = z.iter().zip(&dz).map(|(a, b)| a + b).collect();
                    let ft = self.eval(rt, &zt);
                    if ft.iter().all(|x| x.is_finite()) && norm2(&ft) < n0 {
                        z.copy_from_slice(&zt);
                        f = ft;
                        accepted = true;
                        mu = (mu * 0.1).max(1e-12);
                        break;
                    }
                }
                mu *= 10.0;
            }
            if !accepted {
                self.eval(rt, z);
                return Err(format!(
                    "steady state: no convergence (scaled residual {m0:.3e}); try another starting point (simulate to a later time first)"
                ));
            }
        }
        let jac = self.jacobian(rt);
        let w = row_weights(&jac, z);
        let m0 = f
            .iter()
            .zip(&w)
            .fold(0.0f64, |a, (x, w)| a.max((x * w).abs()));
        if m0 <= o.tolerance.sqrt() {
            // Close enough to accept (rounding-limited), but not to the requested tolerance.
            return Ok(o.max_iterations);
        }
        Err(format!(
            "steady state: no convergence in {} iterations (scaled residual {m0:.3e})",
            o.max_iterations
        ))
    }
}

/// Row weights of the residuals: one over the size of the terms in each row,
/// `max_j |J_ij| * max(|z_j|, 1)` (at least 1).
fn row_weights(jac: &[f64], z: &[f64]) -> Vec<f64> {
    let n = z.len();
    (0..n)
        .map(|i| {
            let s = (0..n).fold(0.0f64, |a, j| {
                a.max(jac[i * n + j].abs() * z[j].abs().max(1.0))
            });
            1.0 / s.max(1.0)
        })
        .collect()
}

/// Levenberg–Marquardt step on the weighted residuals `W f`:
/// `(J^T W^2 J + mu (diag(J^T W^2 J) + eps I)) dz = -J^T W^2 f`.
fn lm_step(jac: &[f64], f: &[f64], w: &[f64], n: usize, mu: f64) -> Vec<f64> {
    let mut a = vec![0.0; n * n];
    let mut g = vec![0.0; n];
    for k in 0..n {
        let w2 = w[k] * w[k];
        for i in 0..n {
            let jki = jac[k * n + i] * w2;
            g[i] -= jki * f[k];
            for j in 0..n {
                a[i * n + j] += jki * jac[k * n + j];
            }
        }
    }
    let scale = (0..n).fold(0.0f64, |s, i| s.max(a[i * n + i])).max(1e-300);
    for i in 0..n {
        a[i * n + i] += mu * (a[i * n + i] + 1e-9 * scale);
    }
    let mut piv = vec![0; n];
    if crate::linalg::lu_factor(&mut a, n, &mut piv) {
        crate::linalg::lu_solve(&a, n, &piv, &mut g);
        g
    } else {
        vec![f64::NAN; n]
    }
}

impl SteadyState {
    /// Human-readable report.
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let trim = !self.free.is_empty();
        let _ = writeln!(
            s,
            "{} of {} at t = {} ({} Newton iterations, residual {:.3e})",
            if trim { "trim" } else { "steady state" },
            self.model,
            self.time,
            self.iterations,
            self.residual
        );
        let list = |s: &mut String, title: &str, v: &[NamedValue]| {
            if v.is_empty() {
                return;
            }
            let _ = writeln!(s, "\n{title}:");
            let w = v.iter().map(|x| x.name.len()).max().unwrap_or(0);
            for x in v {
                let _ = writeln!(s, "  {:w$} = {}", x.name, x.value);
            }
        };
        list(&mut s, "states", &self.states);
        list(&mut s, "derivatives", &self.derivatives);
        list(&mut s, "free variables", &self.free);
        if !self.targets.is_empty() {
            let _ = writeln!(s, "\ntargets:");
            for t in &self.targets {
                let _ = writeln!(s, "  {} = {} (target {})", t.name, t.value, t.target);
            }
        }
        list(&mut s, "inputs", &self.inputs);
        list(&mut s, "outputs", &self.outputs);
        if !self.eigenvalues.is_empty() {
            let _ = writeln!(
                s,
                "\neigenvalues of the state Jacobian ({}):",
                if self.stable {
                    "stable"
                } else {
                    "not asymptotically stable"
                }
            );
            for &(re, im) in &self.eigenvalues {
                if im == 0.0 {
                    let _ = writeln!(s, "  {re:.6e}");
                } else {
                    let _ = writeln!(
                        s,
                        "  {re:.6e} {} {:.6e}i",
                        if im < 0.0 { '-' } else { '+' },
                        im.abs()
                    );
                }
            }
        }
        s
    }

    /// The value of a state, free variable, target, input or output by name.
    pub fn value(&self, name: &str) -> Option<f64> {
        self.states
            .iter()
            .chain(&self.derivatives)
            .chain(&self.free)
            .chain(&self.inputs)
            .chain(&self.outputs)
            .find(|v| v.name == name)
            .map(|v| v.value)
            .or_else(|| {
                self.targets
                    .iter()
                    .find(|t| t.name == name)
                    .map(|t| t.value)
            })
    }
}
