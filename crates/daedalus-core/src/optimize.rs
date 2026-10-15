//! Design optimization: minimize a weighted sum of scalar quantities of interest (the
//! [`Qoi`]s of the global analysis) over bounded parameters with the Nelder–Mead
//! simplex method, projected onto the bounds. Each iteration costs one or two
//! simulations (a shrink costs one per parameter), and no derivatives are needed, so
//! models with events work as well as smooth ones.
//!
//! Objective terms: `weight * q` (minimize; a negative weight maximizes) or, with a
//! target, `weight * (q - target)^2`.
//!
//! With [`Algorithm::Lbfgsb`] the optimizer is a bound-constrained quasi-Newton method
//! ([`crate::lbfgsb`]) driven by gradients of the whole objective: one adjoint solve
//! per iteration where it applies (event-free models, see [`crate::adjoint`]), else
//! forward sensitivities. Each iteration then costs one gradient and usually one
//! simulation, whatever the number of parameters.

use crate::adjoint::{GradMethod, objective_gradient};
use crate::calibrate::{Tuner, modifier, tuner_starts, with_params};
use crate::codegen::Model;
use crate::sim::SimOptions;
use crate::uq::{Job, Qoi, check_var, output_grid, run_outputs};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjTerm {
    pub qoi: String,
    #[serde(default = "one")]
    pub weight: f64,
    #[serde(default)]
    pub target: Option<f64>,
}

fn one() -> f64 {
    1.0
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OptSpec {
    pub tuners: Vec<Tuner>,
    pub objective: Vec<ObjTerm>,
    #[serde(default = "default_iter")]
    pub max_iter: usize,
    /// Stop when the simplex values differ by less than this (relative).
    #[serde(default = "default_tol")]
    pub tol: f64,
    #[serde(default)]
    pub algorithm: Algorithm,
    /// Gradient source of L-BFGS-B (default: adjoint where it applies).
    #[serde(default)]
    pub gradient: GradMethod,
}

/// The optimization method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Algorithm {
    /// Derivative-free simplex search (works with events).
    #[default]
    #[serde(alias = "nelder-mead", alias = "nm")]
    NelderMead,
    /// Projected L-BFGS with bounds, gradients from adjoints or sensitivities.
    #[serde(alias = "l-bfgs-b", alias = "lbfgs")]
    Lbfgsb,
}

impl Algorithm {
    pub fn parse(s: &str) -> Option<Algorithm> {
        Some(match s.to_ascii_lowercase().as_str() {
            "nelder-mead" | "neldermead" | "nm" | "simplex" => Algorithm::NelderMead,
            "lbfgsb" | "l-bfgs-b" | "lbfgs" | "gradient" => Algorithm::Lbfgsb,
            _ => return None,
        })
    }
}

fn default_iter() -> usize {
    200
}
fn default_tol() -> f64 {
    1e-8
}

#[derive(Clone, Debug, Serialize)]
pub struct OptResult {
    pub model: String,
    pub params: Vec<String>,
    pub initial: Vec<f64>,
    pub values: Vec<f64>,
    pub objective_before: f64,
    pub objective_after: f64,
    pub qois: Vec<String>,
    pub qoi_before: Vec<f64>,
    pub qoi_after: Vec<f64>,
    pub iterations: usize,
    pub runs: usize,
    pub history: Vec<f64>,
    pub converged: bool,
    pub message: String,
    pub modifier: String,
    pub algorithm: Algorithm,
    /// Gradient evaluations (L-BFGS-B) and the method that computed them.
    pub gradients: usize,
    pub gradient_method: Option<GradMethod>,
    pub notes: Vec<String>,
}

/// Nelder–Mead in the unit box `[0, 1]^n` of the bounds.
pub struct Optimization {
    m: Model,
    opts: SimOptions,
    tuners: Vec<Tuner>,
    names: Vec<String>,
    terms: Vec<(Qoi, f64, Option<f64>)>,
    vars: Vec<String>,
    grid: Vec<f64>,
    max_iter: usize,
    tol: f64,
    simplex: Vec<(Vec<f64>, f64, Vec<f64>)>,
    x0: Vec<f64>,
    f0: (f64, Vec<f64>),
    iter: usize,
    runs: usize,
    history: Vec<f64>,
    done: bool,
    converged: bool,
    message: String,
    algorithm: Algorithm,
    grad_method: GradMethod,
    objective: Vec<ObjTerm>,
    // L-BFGS-B state (unit box)
    grad: Vec<f64>,
    memory: crate::lbfgsb::Memory,
    gradients: usize,
    grad_used: Option<GradMethod>,
    notes: Vec<String>,
}

impl Optimization {
    pub fn new(m: Model, opts: SimOptions, spec: OptSpec) -> Result<Optimization, String> {
        if spec.tuners.is_empty() {
            return Err("no parameters to optimize".into());
        }
        if spec.objective.is_empty() {
            return Err("no objective given".into());
        }
        for t in &spec.tuners {
            if t.lo.is_none() || t.hi.is_none() {
                return Err(format!(
                    "optimization needs finite bounds for '{}' (name=lo:hi)",
                    t.name
                ));
            }
        }
        let mut terms = Vec::new();
        let mut vars: Vec<String> = Vec::new();
        for o in &spec.objective {
            let q = Qoi::parse(&o.qoi)?;
            check_var(&m, &q.var)?;
            if !vars.contains(&q.var) {
                vars.push(q.var.clone());
            }
            terms.push((q, o.weight, o.target));
        }
        let mut o = opts;
        o.store_events = false;
        let grid = output_grid(&o);
        let p0 = tuner_starts(&m, &o, &spec.tuners)?;
        let x0 = spec
            .tuners
            .iter()
            .zip(&p0)
            .map(|(t, &p)| {
                let (a, b) = (t.lo(), t.hi());
                if b > a { (p - a) / (b - a) } else { 0.0 }
            })
            .collect();
        Ok(Optimization {
            algorithm: spec.algorithm,
            grad_method: spec.gradient,
            objective: spec.objective.clone(),
            grad: Vec::new(),
            memory: crate::lbfgsb::Memory::new(8),
            gradients: 0,
            grad_used: None,
            notes: Vec::new(),
            m,
            opts: o,
            names: spec.tuners.iter().map(|t| t.name.clone()).collect(),
            tuners: spec.tuners,
            terms,
            vars,
            grid,
            max_iter: spec.max_iter.max(1),
            tol: spec.tol,
            simplex: Vec::new(),
            x0,
            f0: (f64::NAN, Vec::new()),
            iter: 0,
            runs: 0,
            history: Vec::new(),
            done: false,
            converged: false,
            message: String::new(),
        })
    }

    fn to_params(&self, x: &[f64]) -> Vec<f64> {
        self.tuners
            .iter()
            .zip(x)
            .map(|(t, &u)| t.lo() + (t.hi() - t.lo()) * u.clamp(0.0, 1.0))
            .collect()
    }

    /// Objective and QoI values at unit point `x` (infinite if the run fails).
    fn eval(&mut self, x: &[f64]) -> (f64, Vec<f64>) {
        self.runs += 1;
        let p = self.to_params(x);
        let o = with_params(&self.opts, &self.names, &p);
        let Ok(traj) = run_outputs(&self.m, &o, &[], &self.vars, &self.grid) else {
            return (f64::INFINITY, vec![f64::NAN; self.terms.len()]);
        };
        let mut f = 0.0;
        let mut qs = Vec::new();
        for (q, w, target) in &self.terms {
            let vi = self.vars.iter().position(|v| *v == q.var).unwrap();
            let v = q.eval(&self.grid, &traj[vi]);
            qs.push(v);
            f += match target {
                Some(t) => w * (v - t).powi(2),
                None => w * v,
            };
        }
        (if f.is_finite() { f } else { f64::INFINITY }, qs)
    }

    /// Gradient of the objective in unit coordinates.
    fn gradient(&mut self, x: &[f64]) -> Result<Vec<f64>, String> {
        let p = self.to_params(x);
        let o = with_params(&self.opts, &self.names, &p);
        let g = objective_gradient(&self.m, &o, &self.names, &self.objective, self.grad_method)?;
        self.gradients += 1;
        if self.grad_used != Some(g.method) {
            self.grad_used = Some(g.method);
            self.notes.push(format!(
                "gradients by {}{}",
                match g.method {
                    GradMethod::Adjoint => "the adjoint method",
                    GradMethod::Forward => "forward sensitivities",
                    GradMethod::Fd => "finite differences",
                    GradMethod::Auto => "auto",
                },
                g.notes
                    .iter()
                    .find(|n| n.contains("because"))
                    .map(|n| format!(" ({n})"))
                    .unwrap_or_default()
            ));
        }
        if g.gradient.iter().any(|v| !v.is_finite()) {
            return Err("the gradient is not finite".into());
        }
        Ok(g.gradient
            .iter()
            .zip(&self.tuners)
            .map(|(d, t)| d * (t.hi() - t.lo()))
            .collect())
    }

    fn start_lbfgs(&mut self) -> Result<(), String> {
        let x0 = self.x0.clone();
        let (f, q) = self.eval(&x0);
        self.f0 = (f, q.clone());
        self.simplex = vec![(x0.clone(), f, q)];
        self.history.push(f);
        if !f.is_finite() {
            return Ok(());
        }
        self.grad = self.gradient(&x0)?;
        Ok(())
    }

    fn iterate_lbfgs(&mut self) -> Result<(), String> {
        use crate::lbfgsb::{Outcome, iterate};
        let n = self.x0.len();
        let (lo, hi) = (vec![0.0; n], vec![1.0; n]);
        let mut x = self.simplex[0].0.clone();
        let mut f = self.simplex[0].1;
        let mut g = std::mem::take(&mut self.grad);
        let mut mem = std::mem::take(&mut self.memory);
        let mut last_q = None;
        let gtol = 1e-9 * (1.0 + f.abs());
        let r = {
            let this = std::cell::RefCell::new(&mut *self);
            let mut evals: Vec<(Vec<f64>, Vec<f64>)> = Vec::new();
            let out = iterate(
                &mut x,
                &mut f,
                &mut g,
                &lo,
                &hi,
                &mut mem,
                gtol,
                &mut |p| {
                    let (v, q) = this.borrow_mut().eval(p);
                    evals.push((p.to_vec(), q));
                    v
                },
                &mut |p| this.borrow_mut().gradient(p),
            );
            if let Ok(Outcome::Moved { .. }) = &out {
                last_q = evals.into_iter().rev().find(|(p, _)| *p == x).map(|e| e.1);
            }
            out
        };
        self.memory = mem;
        self.grad = g;
        let out = r?;
        match out {
            Outcome::Moved { decrease, step } => {
                let q = last_q.unwrap_or_else(|| self.simplex[0].2.clone());
                self.simplex[0] = (x, f, q);
                self.history.push(f);
                if decrease.abs() < self.tol || step < 1e-12 {
                    self.converged = true;
                    self.done = true;
                    self.message = "objective and step no longer decrease".into();
                }
            }
            Outcome::Restart => {}
            Outcome::Stationary => {
                self.converged = true;
                self.done = true;
                self.message = "projected gradient vanishes (stationary point)".into();
            }
            Outcome::NoDecrease => {
                self.converged = true;
                self.done = true;
                self.message = "no step lowers the objective (local minimum)".into();
            }
        }
        Ok(())
    }

    fn start(&mut self) {
        let x0 = self.x0.clone();
        let (f, q) = self.eval(&x0);
        self.f0 = (f, q.clone());
        self.simplex.push((x0.clone(), f, q));
        for i in 0..x0.len() {
            let mut x = x0.clone();
            x[i] = if x[i] + 0.15 <= 1.0 {
                x[i] + 0.15
            } else {
                x[i] - 0.15
            };
            let (f, q) = self.eval(&x);
            self.simplex.push((x, f, q));
        }
        self.sort();
        self.history.push(self.simplex[0].1);
    }

    fn sort(&mut self) {
        self.simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    }

    fn iterate(&mut self) {
        let n = self.x0.len();
        let best = self.simplex[0].1;
        let worst = self.simplex[n].1;
        let size = self
            .simplex
            .iter()
            .skip(1)
            .map(|(x, ..)| {
                x.iter()
                    .zip(&self.simplex[0].0)
                    .map(|(a, b)| (a - b).abs())
                    .fold(0.0, f64::max)
            })
            .fold(0.0, f64::max);
        if (worst - best).abs() <= self.tol * (best.abs() + self.tol) || size < 1e-10 {
            self.converged = true;
            self.done = true;
            self.message = "simplex converged".into();
            return;
        }
        let centroid: Vec<f64> = (0..n)
            .map(|i| self.simplex[..n].iter().map(|s| s.0[i]).sum::<f64>() / n as f64)
            .collect();
        let worst_x = self.simplex[n].0.clone();
        let point = |c: f64| -> Vec<f64> {
            (0..n)
                .map(|i| (centroid[i] + c * (centroid[i] - worst_x[i])).clamp(0.0, 1.0))
                .collect()
        };
        let xr = point(1.0);
        let (fr, qr) = self.eval(&xr);
        if fr < self.simplex[0].1 {
            let xe = point(2.0);
            let (fe, qe) = self.eval(&xe);
            self.simplex[n] = if fe < fr { (xe, fe, qe) } else { (xr, fr, qr) };
        } else if fr < self.simplex[n - 1].1 {
            self.simplex[n] = (xr, fr, qr);
        } else {
            let (xc, (fc, qc)) = if fr < self.simplex[n].1 {
                let x = point(0.5);
                let v = self.eval(&x);
                (x, v)
            } else {
                let x = point(-0.5);
                let v = self.eval(&x);
                (x, v)
            };
            if fc < self.simplex[n].1.min(fr) {
                self.simplex[n] = (xc, fc, qc);
            } else {
                // Shrink towards the best point.
                let b = self.simplex[0].0.clone();
                for k in 1..=n {
                    let x: Vec<f64> = self.simplex[k]
                        .0
                        .iter()
                        .zip(&b)
                        .map(|(xi, bi)| bi + 0.5 * (xi - bi))
                        .collect();
                    let (f, q) = self.eval(&x);
                    self.simplex[k] = (x, f, q);
                }
            }
        }
        self.sort();
        self.history.push(self.simplex[0].1);
    }

    pub fn result(&self) -> OptResult {
        let best = self.simplex.first();
        let values = best.map_or_else(|| self.to_params(&self.x0), |b| self.to_params(&b.0));
        let fitted: Vec<(String, f64)> = self.names.iter().cloned().zip(values.clone()).collect();
        OptResult {
            model: self.m.name.clone(),
            params: self.names.clone(),
            initial: self.to_params(&self.x0),
            values,
            objective_before: self.f0.0,
            objective_after: best.map_or(f64::NAN, |b| b.1),
            qois: self.terms.iter().map(|t| t.0.label()).collect(),
            qoi_before: self.f0.1.clone(),
            qoi_after: best.map_or_else(Vec::new, |b| b.2.clone()),
            iterations: self.iter,
            runs: self.runs,
            history: self.history.clone(),
            converged: self.converged,
            message: self.message.clone(),
            modifier: modifier(&fitted),
            algorithm: self.algorithm,
            gradients: self.gradients,
            gradient_method: self.grad_used,
            notes: self.notes.clone(),
        }
    }

    pub fn report_of(r: &OptResult) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "optimization of {} ({}): {} iterations, {} runs{}; {}",
            r.model,
            match r.algorithm {
                Algorithm::NelderMead => "Nelder-Mead",
                Algorithm::Lbfgsb => "L-BFGS-B",
            },
            r.iterations,
            r.runs,
            if r.gradients > 0 {
                format!(", {} gradients", r.gradients)
            } else {
                String::new()
            },
            r.message
        );
        for n in &r.notes {
            let _ = writeln!(s, "  note: {n}");
        }
        let _ = writeln!(
            s,
            "  objective: {:.6e} -> {:.6e}",
            r.objective_before, r.objective_after
        );
        for (i, q) in r.qois.iter().enumerate() {
            let _ = writeln!(
                s,
                "  {q}: {:.6e} -> {:.6e}",
                r.qoi_before.get(i).copied().unwrap_or(f64::NAN),
                r.qoi_after.get(i).copied().unwrap_or(f64::NAN)
            );
        }
        let _ = writeln!(
            s,
            "\n  {:<24} {:>14} {:>14}",
            "parameter", "initial", "optimal"
        );
        for (i, p) in r.params.iter().enumerate() {
            let _ = writeln!(
                s,
                "  {:<24} {:>14.7e} {:>14.7e}",
                p, r.initial[i], r.values[i]
            );
        }
        let _ = writeln!(s, "\nmodifier: {}", r.modifier);
        s
    }
}

impl Job for Optimization {
    fn step(&mut self) -> Result<bool, String> {
        if self.done {
            return Ok(true);
        }
        if self.algorithm == Algorithm::Lbfgsb {
            if self.simplex.is_empty() {
                self.start_lbfgs()?;
                if !self.simplex[0].1.is_finite() {
                    return Err(
                        "the objective is not finite at the start point (does the run fail?)"
                            .into(),
                    );
                }
                return Ok(false);
            }
            self.iterate_lbfgs()?;
            self.iter += 1;
            if !self.done && self.iter >= self.max_iter {
                self.done = true;
                self.message = format!("iteration limit ({}) reached", self.max_iter);
            }
            return Ok(self.done);
        }
        if self.simplex.is_empty() {
            self.start();
            if !self.simplex[0].1.is_finite() {
                return Err(
                    "the objective is not finite at any start point (do the runs fail?)".into(),
                );
            }
            return Ok(false);
        }
        self.iterate();
        self.iter += 1;
        if !self.done && self.iter >= self.max_iter {
            self.done = true;
            self.message = format!("iteration limit ({}) reached", self.max_iter);
        }
        Ok(self.done)
    }

    fn progress(&self) -> (usize, usize) {
        if self.done {
            (self.max_iter, self.max_iter)
        } else {
            (self.iter, self.max_iter)
        }
    }

    fn result_json(&self) -> serde_json::Value {
        serde_json::to_value(self.result()).unwrap_or_default()
    }

    fn report(&self) -> String {
        Optimization::report_of(&self.result())
    }
}
