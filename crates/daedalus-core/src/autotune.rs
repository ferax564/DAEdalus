//! Controller autotuning against a closed-loop step-response specification.
//!
//! The model is the closed loop (plant and controller); the gains are parameters of the
//! controller. Each candidate is simulated and its response over a window around a
//! set-point step is scored by an error integral (ITAE by default) plus penalties for
//! breaking the specification (overshoot above a limit, control effort above a limit,
//! settling slower than a limit). Gains are searched with Nelder–Mead in the unit box of
//! their bounds, on a logarithmic scale when both bounds are positive, so gains spanning
//! decades are tuned evenly. No derivatives are needed: sampled controllers, limits and
//! anti-windup logic tune like smooth ones.
//!
//! The linear-analysis helpers (SIMC or Ziegler–Nichols suggestions from the linearized
//! plant) give a starting point; this module refines gains on the nonlinear closed loop.

use crate::calibrate::Tuner;
use crate::codegen::Model;
use crate::results::Results;
use crate::sim::SimOptions;
use serde::{Deserialize, Serialize};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Criterion {
    /// ∫ |e| dt
    Iae,
    /// ∫ e² dt
    Ise,
    /// ∫ (t - t0) |e| dt
    Itae,
}

impl Criterion {
    pub fn parse(s: &str) -> Option<Criterion> {
        match s.to_ascii_lowercase().as_str() {
            "iae" => Some(Criterion::Iae),
            "ise" => Some(Criterion::Ise),
            "itae" => Some(Criterion::Itae),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Spec {
    /// Controller parameters to tune, with bounds (and optional start values).
    pub gains: Vec<Tuner>,
    /// The set-point variable.
    pub reference: String,
    /// The controlled (measured) variable.
    pub measurement: String,
    /// The controller output, for the effort limit and the report.
    #[serde(default)]
    pub control: Option<String>,
    /// The step to score: from `t0` (just before the set-point step) to `t1`; default the
    /// whole run.
    #[serde(default)]
    pub window: Option<(f64, f64)>,
    #[serde(default = "itae")]
    pub criterion: Criterion,
    /// Largest acceptable overshoot, percent of the step (default 5).
    #[serde(default = "five")]
    pub max_overshoot: f64,
    /// Largest acceptable |control|.
    #[serde(default)]
    pub max_control: Option<f64>,
    /// Largest acceptable 2 % settling time after `t0`.
    #[serde(default)]
    pub max_settling: Option<f64>,
    #[serde(default = "iters")]
    pub max_iter: usize,
}

fn itae() -> Criterion {
    Criterion::Itae
}
fn five() -> f64 {
    5.0
}
fn iters() -> usize {
    120
}

/// Step-response figures of one run.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Metrics {
    pub criterion: f64,
    pub overshoot: f64,
    pub rise_time: f64,
    pub settling_time: f64,
    pub steady_state_error: f64,
    pub max_control: f64,
    /// Criterion plus the specification penalties: what is minimized.
    pub cost: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct Outcome {
    pub model: String,
    pub gains: Vec<String>,
    pub initial: Vec<f64>,
    pub tuned: Vec<f64>,
    pub before: Metrics,
    pub after: Metrics,
    pub runs: usize,
    pub iterations: usize,
    pub meets_spec: bool,
    /// Modelica modifier with the tuned gains.
    pub modifier: String,
}

struct Scale {
    lo: f64,
    hi: f64,
    log: bool,
}

impl Scale {
    fn to_x(&self, z: f64) -> f64 {
        let z = z.clamp(0.0, 1.0);
        if self.log {
            (self.lo.ln() + z * (self.hi.ln() - self.lo.ln())).exp()
        } else {
            self.lo + z * (self.hi - self.lo)
        }
    }
    fn to_z(&self, x: f64) -> f64 {
        let z = if self.log {
            (x.max(self.lo).ln() - self.lo.ln()) / (self.hi.ln() - self.lo.ln())
        } else {
            (x - self.lo) / (self.hi - self.lo)
        };
        z.clamp(0.0, 1.0)
    }
}

fn series(res: &Results, name: &str) -> Result<Vec<f64>, String> {
    res.values(name)
        .ok_or_else(|| format!("no variable '{name}' in the results"))
}

/// Score a run's response over `spec.window`.
pub fn metrics(res: &Results, spec: &Spec) -> Result<Metrics, String> {
    let t = &res.time;
    let r = series(res, &spec.reference)?;
    let y = series(res, &spec.measurement)?;
    let u = match &spec.control {
        Some(c) => Some(series(res, c)?),
        None => None,
    };
    let (t0, t1) = spec.window.unwrap_or((t[0], *t.last().unwrap_or(&t[0])));
    let idx: Vec<usize> = (0..t.len()).filter(|&i| t[i] >= t0 && t[i] <= t1).collect();
    if idx.len() < 3 {
        return Err(format!(
            "fewer than 3 result points in the window [{t0}, {t1}]"
        ));
    }
    let i0 = idx[0];
    let il = *idx.last().unwrap();
    let target = r[il];
    let start = y[i0];
    let step = target - start;
    let mut m = Metrics::default();
    // error integral (trapezoid)
    for w in idx.windows(2) {
        let (a, b) = (w[0], w[1]);
        let dt = t[b] - t[a];
        let f = |i: usize| {
            let e = r[i] - y[i];
            match spec.criterion {
                Criterion::Iae => e.abs(),
                Criterion::Ise => e * e,
                Criterion::Itae => (t[i] - t0) * e.abs(),
            }
        };
        m.criterion += 0.5 * dt * (f(a) + f(b));
    }
    if step.abs() > 0.0 {
        let s = step.signum();
        let peak = idx
            .iter()
            .map(|&i| (y[i] - target) * s)
            .fold(f64::NEG_INFINITY, f64::max);
        m.overshoot = (peak / step.abs() * 100.0).max(0.0);
        let cross = |frac: f64| {
            idx.iter()
                .find(|&&i| (y[i] - start) * s >= frac * step.abs())
                .map(|&i| t[i])
        };
        m.rise_time = match (cross(0.1), cross(0.9)) {
            (Some(a), Some(b)) => b - a,
            _ => f64::INFINITY,
        };
        let band = 0.02 * step.abs();
        m.settling_time = match idx.iter().rev().find(|&&i| (y[i] - target).abs() > band) {
            Some(&i) if i == il => f64::INFINITY,
            Some(&i) => t[i] - t0,
            None => 0.0,
        };
        m.steady_state_error = (y[il] - target).abs() / step.abs() * 100.0;
    }
    if let Some(u) = &u {
        m.max_control = idx.iter().map(|&i| u[i].abs()).fold(0.0, f64::max);
    }
    // penalties, relative to the criterion of a response that never moves
    let scale = {
        let span = t1 - t0;
        let e = step.abs().max(1e-12);
        match spec.criterion {
            Criterion::Iae => e * span,
            Criterion::Ise => e * e * span,
            Criterion::Itae => e * span * span / 2.0,
        }
    };
    let mut pen = 0.0;
    if m.overshoot > spec.max_overshoot {
        let x = m.overshoot - spec.max_overshoot;
        pen += 0.2 * x + x * x;
    }
    if let (Some(lim), true) = (spec.max_control, u.is_some()) {
        if m.max_control > lim {
            pen += ((m.max_control - lim) / lim).powi(2) * 10.0;
        }
    }
    if let Some(ts) = spec.max_settling {
        let st = if m.settling_time.is_finite() {
            m.settling_time
        } else {
            t1 - t0
        };
        if st > ts {
            pen += ((st - ts) / ts).powi(2);
        }
    }
    m.cost = m.criterion / scale + pen;
    Ok(m)
}

fn meets(m: &Metrics, spec: &Spec) -> bool {
    m.overshoot <= spec.max_overshoot + 1e-9
        && spec
            .max_control
            .is_none_or(|l| m.max_control <= l * (1.0 + 1e-9))
        && spec.max_settling.is_none_or(|l| m.settling_time <= l)
}

/// Tune the gains. `opts` is the simulation set-up of the closed loop (its parameter
/// overrides apply to everything but the gains).
pub fn tune(m: &Model, opts: &SimOptions, spec: &Spec) -> Result<Outcome, String> {
    if spec.gains.is_empty() {
        return Err("give at least one gain to tune".into());
    }
    let names: Vec<String> = spec.gains.iter().map(|g| g.name.clone()).collect();
    let current = crate::uq::param_values(m, opts, &names)?;
    let mut scales = Vec::new();
    let mut x0 = Vec::new();
    for (g, &cur) in spec.gains.iter().zip(&current) {
        let start = g.start.unwrap_or(cur);
        let lo = g.lo.unwrap_or(if start > 0.0 {
            start / 100.0
        } else {
            start - 1.0
        });
        let hi = g.hi.unwrap_or(if start > 0.0 {
            start * 100.0
        } else {
            start + 1.0
        });
        if !(hi > lo) {
            return Err(format!("gain {}: empty range [{lo}, {hi}]", g.name));
        }
        scales.push(Scale {
            lo,
            hi,
            log: lo > 0.0,
        });
        x0.push(start.clamp(lo, hi));
    }
    let mut runs = 0usize;
    let mut eval = |z: &[f64]| -> (f64, Option<Metrics>) {
        runs += 1;
        let mut o = opts.clone();
        o.params.retain(|(n, _)| !names.iter().any(|g| g == n));
        for (k, s) in scales.iter().enumerate() {
            o.params.push((names[k].clone(), s.to_x(z[k])));
        }
        match crate::sim::simulate(m, o) {
            Ok((res, _)) => match metrics(&res, spec) {
                Ok(mt) if mt.cost.is_finite() => (mt.cost, Some(mt)),
                _ => (f64::INFINITY, None),
            },
            Err(_) => (f64::INFINITY, None),
        }
    };
    let z0: Vec<f64> = scales.iter().zip(&x0).map(|(s, &x)| s.to_z(x)).collect();
    let (f0, m0) = eval(&z0);
    let before = m0.ok_or("the closed loop does not simulate with the initial gains")?;
    // Nelder–Mead in the unit box.
    let n = z0.len();
    let mut simplex: Vec<(Vec<f64>, f64)> = vec![(z0.clone(), f0)];
    for k in 0..n {
        let mut z = z0.clone();
        z[k] = if z[k] > 0.5 { z[k] - 0.15 } else { z[k] + 0.15 };
        let f = eval(&z).0;
        simplex.push((z, f));
    }
    let clamp = |z: Vec<f64>| -> Vec<f64> { z.into_iter().map(|v| v.clamp(0.0, 1.0)).collect() };
    let mut iterations = 0;
    for _ in 0..spec.max_iter {
        iterations += 1;
        simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
        let (fb, fw) = (simplex[0].1, simplex[n].1);
        let spread = simplex
            .iter()
            .flat_map(|(z, _)| z.iter().zip(&simplex[0].0).map(|(a, b)| (a - b).abs()))
            .fold(0.0, f64::max);
        if (fw - fb).abs() <= 1e-6 * (fb.abs() + 1e-12) && spread < 1e-4 {
            break;
        }
        let centroid: Vec<f64> = (0..n)
            .map(|k| simplex[..n].iter().map(|(z, _)| z[k]).sum::<f64>() / n as f64)
            .collect();
        let at = |a: f64| -> Vec<f64> {
            clamp(
                centroid
                    .iter()
                    .zip(&simplex[n].0)
                    .map(|(c, w)| c + a * (c - w))
                    .collect(),
            )
        };
        let zr = at(1.0);
        let fr = eval(&zr).0;
        if fr < simplex[0].1 {
            let ze = at(2.0);
            let fe = eval(&ze).0;
            simplex[n] = if fe < fr { (ze, fe) } else { (zr, fr) };
        } else if fr < simplex[n - 1].1 {
            simplex[n] = (zr, fr);
        } else {
            let zc = if fr < simplex[n].1 { at(0.5) } else { at(-0.5) };
            let fc = eval(&zc).0;
            if fc < simplex[n].1.min(fr) {
                simplex[n] = (zc, fc);
            } else {
                let best = simplex[0].0.clone();
                for item in simplex.iter_mut().skip(1) {
                    let z: Vec<f64> = item
                        .0
                        .iter()
                        .zip(&best)
                        .map(|(a, b)| b + 0.5 * (a - b))
                        .collect();
                    let f = eval(&z).0;
                    *item = (z, f);
                }
            }
        }
    }
    simplex.sort_by(|a, b| a.1.total_cmp(&b.1));
    let zb = simplex[0].0.clone();
    let (_, mb) = eval(&zb);
    let after = mb.ok_or("the tuned closed loop does not simulate")?;
    let tuned: Vec<f64> = scales.iter().zip(&zb).map(|(s, &z)| s.to_x(z)).collect();
    let modifier = crate::calibrate::modifier(
        &names
            .iter()
            .cloned()
            .zip(tuned.iter().copied())
            .collect::<Vec<_>>(),
    );
    Ok(Outcome {
        model: m.name.clone(),
        gains: names,
        initial: x0,
        tuned,
        meets_spec: meets(&after, spec),
        before,
        after,
        runs,
        iterations,
        modifier,
    })
}

impl Outcome {
    pub fn report(&self, spec: &Spec) -> String {
        let mut s = format!(
            "autotune of {}: {} runs, {} iterations; {}\n\n",
            self.model,
            self.runs,
            self.iterations,
            if self.meets_spec {
                "meets the specification"
            } else {
                "does NOT meet the specification (relax it or widen the gain bounds)"
            }
        );
        let _ = writeln!(s, "{:<24} {:>14} {:>14}", "gain", "before", "tuned");
        for (k, g) in self.gains.iter().enumerate() {
            let _ = writeln!(
                s,
                "{g:<24} {:>14.6e} {:>14.6e}",
                self.initial[k], self.tuned[k]
            );
        }
        let crit = match spec.criterion {
            Criterion::Iae => "IAE",
            Criterion::Ise => "ISE",
            Criterion::Itae => "ITAE",
        };
        let fmt = |x: f64| {
            if x.is_finite() {
                format!("{x:.4}")
            } else {
                "never".into()
            }
        };
        let _ = writeln!(
            s,
            "\n{:<24} {:>14} {:>14}   limit",
            "response", "before", "tuned"
        );
        let rows: Vec<(&str, f64, f64, String)> = vec![
            (
                crit,
                self.before.criterion,
                self.after.criterion,
                String::new(),
            ),
            (
                "overshoot (%)",
                self.before.overshoot,
                self.after.overshoot,
                format!("{}", spec.max_overshoot),
            ),
            (
                "rise time 10-90 % (s)",
                self.before.rise_time,
                self.after.rise_time,
                String::new(),
            ),
            (
                "settling time 2 % (s)",
                self.before.settling_time,
                self.after.settling_time,
                spec.max_settling.map(|v| v.to_string()).unwrap_or_default(),
            ),
            (
                "final error (%)",
                self.before.steady_state_error,
                self.after.steady_state_error,
                String::new(),
            ),
        ];
        for (n, a, b, l) in rows {
            let _ = writeln!(s, "{n:<24} {:>14} {:>14}   {l}", fmt(a), fmt(b));
        }
        if spec.control.is_some() {
            let _ = writeln!(
                s,
                "{:<24} {:>14} {:>14}   {}",
                "max |control|",
                fmt(self.before.max_control),
                fmt(self.after.max_control),
                spec.max_control.map(|v| v.to_string()).unwrap_or_default()
            );
        }
        let _ = writeln!(s, "\nmodifier: {}", self.modifier);
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LOOP: &str = "model L
        parameter Real kp = 0.2;
        parameter Real ki = 0.1;
        Real r = if time < 0.1 then 0 else 1;
        Real y(start=0, fixed=true);
        Real v(start=0, fixed=true);
        Real xi(start=0, fixed=true);
        Real u;
      equation
        der(xi) = r - y;
        u = kp*(r - y) + ki*xi;
        der(v) = u - 3*v - 2*y;
        der(y) = v;
        annotation(experiment(StopTime=8, Interval=0.01));
      end L;";

    #[test]
    fn tunes_a_pi_loop_to_the_spec() {
        let mut lib = crate::frontend::Library::new();
        assert!(lib.set_document("l.mo", LOOP).is_empty());
        let m = crate::translate(&lib, "L", &Default::default()).unwrap();
        let o = SimOptions::from_model(&m);
        let spec = Spec {
            gains: vec![
                Tuner {
                    name: "kp".into(),
                    lo: Some(0.01),
                    hi: Some(20.0),
                    start: None,
                },
                Tuner {
                    name: "ki".into(),
                    lo: Some(0.01),
                    hi: Some(20.0),
                    start: None,
                },
            ],
            reference: "r".into(),
            measurement: "y".into(),
            control: Some("u".into()),
            window: Some((0.09, 8.0)),
            criterion: Criterion::Itae,
            max_overshoot: 5.0,
            max_control: Some(10.0),
            max_settling: None,
            max_iter: 80,
        };
        let out = tune(&m, &o, &spec).unwrap();
        let rep = out.report(&spec);
        assert!(out.after.criterion < 0.5 * out.before.criterion, "{rep}");
        assert!(out.meets_spec, "{rep}");
        assert!(out.after.overshoot <= 5.0 + 1e-9, "{rep}");
        assert!(rep.contains("modifier: (kp = "), "{rep}");
    }
}
