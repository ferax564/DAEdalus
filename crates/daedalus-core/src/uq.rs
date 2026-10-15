//! Uncertainty quantification and global sensitivity analysis: Monte Carlo trajectory
//! statistics, Sobol indices (Saltelli / Jansen estimators) and Morris elementary
//! effects over parameter distributions.
//!
//! Every analysis is split into three parts so that its runs can be spread over
//! threads (natively) or workers (in the browser):
//! 1. a *plan*: the parameter vectors to simulate, from a seeded generator;
//! 2. *runs*: independent simulations ([`GsaJob::run_sample`] takes `&self`);
//! 3. *aggregation* of the run outputs into statistics ([`GsaJob::result`]).
//!
//! [`GsaJob::step`] runs the next sample (for a single worker that must stay
//! responsive between runs); [`GsaJob::run_parallel`] runs them on all cores.
//!
//! This module also holds what the other analyses share: the random generator,
//! distributions, scalar quantities of interest ([`Qoi`]), the output grid and the
//! [`Job`] interface the CLI and the browser drive analyses through.

use crate::codegen::Model;
use crate::expr::VarId;
use crate::flat::VarKind;
use crate::sim::{SimOptions, simulate};
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------
// Random numbers
// ---------------------------------------------------------------------------------

/// A small, fast, seeded generator (xoshiro256**, seeded through SplitMix64).
#[derive(Clone, Debug)]
pub struct Rng {
    s: [u64; 4],
}

impl Rng {
    pub fn new(seed: u64) -> Rng {
        let mut z = seed;
        let mut next = || {
            z = z.wrapping_add(0x9e37_79b9_7f4a_7c15);
            let mut x = z;
            x = (x ^ (x >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            x = (x ^ (x >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            x ^ (x >> 31)
        };
        Rng {
            s: [next(), next(), next(), next()],
        }
    }

    pub fn next_u64(&mut self) -> u64 {
        let s = &mut self.s;
        let r = s[1].wrapping_mul(5).rotate_left(7).wrapping_mul(9);
        let t = s[1] << 17;
        s[2] ^= s[0];
        s[3] ^= s[1];
        s[1] ^= s[2];
        s[0] ^= s[3];
        s[2] ^= t;
        s[3] = s[3].rotate_left(45);
        r
    }

    /// Uniform in the open interval (0, 1).
    pub fn uniform(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    /// Standard normal (Box–Muller).
    pub fn normal(&mut self) -> f64 {
        let (u, v) = (self.uniform(), self.uniform());
        (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
    }

    /// Uniform integer in `0..n`.
    pub fn below(&mut self, n: usize) -> usize {
        ((self.uniform() * n as f64) as usize).min(n.saturating_sub(1))
    }

    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.below(i + 1);
            v.swap(i, j);
        }
    }
}

/// Inverse of the standard normal CDF (Acklam's rational approximation with one
/// Halley refinement step; relative error near machine precision).
pub fn norm_inv(p: f64) -> f64 {
    if p <= 0.0 {
        return f64::NEG_INFINITY;
    }
    if p >= 1.0 {
        return f64::INFINITY;
    }
    const A: [f64; 6] = [
        -3.969_683_028_665_376e1,
        2.209_460_984_245_205e2,
        -2.759_285_104_469_687e2,
        1.383_577_518_672_69e2,
        -3.066_479_806_614_716e1,
        2.506_628_277_459_239,
    ];
    const B: [f64; 5] = [
        -5.447_609_879_822_406e1,
        1.615_858_368_580_409e2,
        -1.556_989_798_598_866e2,
        6.680_131_188_771_972e1,
        -1.328_068_155_288_572e1,
    ];
    const C: [f64; 6] = [
        -7.784_894_002_430_293e-3,
        -3.223_964_580_411_365e-1,
        -2.400_758_277_161_838,
        -2.549_732_539_343_734,
        4.374_664_141_464_968,
        2.938_163_982_698_783,
    ];
    const D: [f64; 4] = [
        7.784_695_709_041_462e-3,
        3.224_671_290_700_398e-1,
        2.445_134_137_142_996,
        3.754_408_661_907_416,
    ];
    let pl = 0.02425;
    let x = if p < pl {
        let q = (-2.0 * p.ln()).sqrt();
        (((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    } else if p <= 1.0 - pl {
        let q = p - 0.5;
        let r = q * q;
        (((((A[0] * r + A[1]) * r + A[2]) * r + A[3]) * r + A[4]) * r + A[5]) * q
            / (((((B[0] * r + B[1]) * r + B[2]) * r + B[3]) * r + B[4]) * r + 1.0)
    } else {
        let q = (-2.0 * (1.0 - p).ln()).sqrt();
        -(((((C[0] * q + C[1]) * q + C[2]) * q + C[3]) * q + C[4]) * q + C[5])
            / ((((D[0] * q + D[1]) * q + D[2]) * q + D[3]) * q + 1.0)
    };
    // One Halley step on Phi(x) - p.
    let e = 0.5 * erfc(-x / std::f64::consts::SQRT_2) - p;
    let u = e * (2.0 * std::f64::consts::PI).sqrt() * (x * x / 2.0).exp();
    x - u / (1.0 + x * u / 2.0)
}

/// Complementary error function (Numerical Recipes' Chebyshev fit, |error| < 1.2e-7,
/// refined by the Halley step that uses it).
fn erfc(x: f64) -> f64 {
    let z = x.abs();
    let t = 1.0 / (1.0 + 0.5 * z);
    let r = t
        * (-z * z - 1.265_512_23
            + t * (1.000_023_68
                + t * (0.374_091_96
                    + t * (0.096_784_18
                        + t * (-0.186_288_06
                            + t * (0.278_868_07
                                + t * (-1.135_203_98
                                    + t * (1.488_515_87
                                        + t * (-0.822_152_23 + t * 0.170_872_77)))))))))
            .exp();
    if x >= 0.0 { r } else { 2.0 - r }
}

// ---------------------------------------------------------------------------------
// Distributions and sampling
// ---------------------------------------------------------------------------------

/// Distribution of an uncertain parameter.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Dist {
    Uniform {
        lo: f64,
        hi: f64,
    },
    Normal {
        mean: f64,
        std: f64,
    },
    /// Log-normal with the given mean and standard deviation of the parameter itself.
    LogNormal {
        mean: f64,
        std: f64,
    },
}

impl Dist {
    /// The value at cumulative probability `u` in (0, 1).
    pub fn quantile(&self, u: f64) -> f64 {
        match *self {
            Dist::Uniform { lo, hi } => lo + (hi - lo) * u,
            Dist::Normal { mean, std } => mean + std * norm_inv(u),
            Dist::LogNormal { mean, std } => {
                let s2 = (1.0 + (std / mean).powi(2)).ln();
                let mu = mean.ln() - 0.5 * s2;
                (mu + s2.sqrt() * norm_inv(u)).exp()
            }
        }
    }

    pub fn mean(&self) -> f64 {
        match *self {
            Dist::Uniform { lo, hi } => 0.5 * (lo + hi),
            Dist::Normal { mean, .. } | Dist::LogNormal { mean, .. } => mean,
        }
    }

    /// Whether the support is bounded (Morris maps unbounded ones through the
    /// 0.1 % .. 99.9 % quantiles).
    pub fn bounded(&self) -> bool {
        matches!(self, Dist::Uniform { .. })
    }

    /// Parse `uniform:lo:hi`, `lo:hi` (uniform), `normal:mean:std` or
    /// `lognormal:mean:std`.
    pub fn parse(s: &str) -> Result<Dist, String> {
        let parts: Vec<&str> = s.split(':').map(str::trim).collect();
        let num = |x: &str| -> Result<f64, String> {
            x.parse::<f64>()
                .map_err(|_| format!("bad number '{x}' in distribution '{s}'"))
        };
        let d = match parts.as_slice() {
            [lo, hi] => Dist::Uniform {
                lo: num(lo)?,
                hi: num(hi)?,
            },
            [k, a, b] => match k.to_ascii_lowercase().as_str() {
                "uniform" | "u" => Dist::Uniform {
                    lo: num(a)?,
                    hi: num(b)?,
                },
                "normal" | "n" | "gauss" => Dist::Normal {
                    mean: num(a)?,
                    std: num(b)?,
                },
                "lognormal" | "ln" | "lognorm" => Dist::LogNormal {
                    mean: num(a)?,
                    std: num(b)?,
                },
                _ => {
                    return Err(format!(
                        "unknown distribution '{k}' (uniform | normal | lognormal)"
                    ));
                }
            },
            _ => {
                return Err(format!(
                    "bad distribution '{s}' (lo:hi, uniform:lo:hi, normal:mean:std or lognormal:mean:std)"
                ));
            }
        };
        d.validate()?;
        Ok(d)
    }

    pub fn validate(&self) -> Result<(), String> {
        let ok = match *self {
            Dist::Uniform { lo, hi } => lo.is_finite() && hi.is_finite() && hi >= lo,
            Dist::Normal { mean, std } => mean.is_finite() && std.is_finite() && std >= 0.0,
            Dist::LogNormal { mean, std } => mean > 0.0 && std.is_finite() && std >= 0.0,
        };
        if ok {
            Ok(())
        } else {
            Err(format!("invalid distribution {self:?}"))
        }
    }
}

/// How unit-cube points are drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Sampling {
    /// Latin hypercube: each dimension's n strata are hit exactly once.
    #[default]
    Lhs,
    /// Plain independent uniform draws.
    Random,
}

/// `n` points in the unit cube `(0, 1)^dim`.
pub fn unit_samples(n: usize, dim: usize, sampling: Sampling, rng: &mut Rng) -> Vec<Vec<f64>> {
    let mut pts = vec![vec![0.0; dim]; n];
    match sampling {
        Sampling::Random => {
            for p in &mut pts {
                for x in p.iter_mut() {
                    *x = rng.uniform();
                }
            }
        }
        Sampling::Lhs => {
            let mut perm: Vec<usize> = (0..n).collect();
            for d in 0..dim {
                rng.shuffle(&mut perm);
                for (i, p) in pts.iter_mut().enumerate() {
                    p[d] = (perm[i] as f64 + rng.uniform()) / n as f64;
                }
            }
        }
    }
    pts
}

// ---------------------------------------------------------------------------------
// Quantities of interest and trajectories
// ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QoiKind {
    Final,
    Max,
    Min,
    Mean,
    Integral,
    /// Value at a time.
    At(f64),
}

/// A scalar quantity of interest of one variable's trajectory, optionally over a time
/// window: `final(x)`, `max(x)`, `min(x)`, `mean(x)`, `integral(x)`, `at(x, t)`, with
/// an optional window `max(x, t0, t1)`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Qoi {
    pub var: String,
    pub kind: QoiKind,
    #[serde(default)]
    pub window: Option<(f64, f64)>,
}

impl Qoi {
    pub fn parse(s: &str) -> Result<Qoi, String> {
        let s = s.trim();
        let bad = || {
            format!(
                "bad quantity of interest '{s}' (final(x), max(x), min(x), mean(x), integral(x), at(x, t), \
                 optionally with a window: max(x, t0, t1))"
            )
        };
        let Some((k, rest)) = s.split_once('(') else {
            // A bare name is its final value.
            if s.is_empty() {
                return Err(bad());
            }
            return Ok(Qoi {
                var: s.to_string(),
                kind: QoiKind::Final,
                window: None,
            });
        };
        let inner = rest.strip_suffix(')').ok_or_else(bad)?;
        let args: Vec<&str> = inner.split(',').map(str::trim).collect();
        let var = args[0].to_string();
        if var.is_empty() {
            return Err(bad());
        }
        let num = |x: &str| x.parse::<f64>().map_err(|_| bad());
        let kind = match k.trim().to_ascii_lowercase().as_str() {
            "final" | "end" | "last" => QoiKind::Final,
            "max" => QoiKind::Max,
            "min" => QoiKind::Min,
            "mean" | "avg" => QoiKind::Mean,
            "integral" | "int" => QoiKind::Integral,
            "at" => {
                if args.len() != 2 {
                    return Err(bad());
                }
                QoiKind::At(num(args[1])?)
            }
            _ => return Err(bad()),
        };
        let window = match (kind, args.len()) {
            (QoiKind::At(_), _) | (_, 1) => None,
            (_, 3) => Some((num(args[1])?, num(args[2])?)),
            _ => return Err(bad()),
        };
        Ok(Qoi { var, kind, window })
    }

    pub fn label(&self) -> String {
        let k = match self.kind {
            QoiKind::Final => "final",
            QoiKind::Max => "max",
            QoiKind::Min => "min",
            QoiKind::Mean => "mean",
            QoiKind::Integral => "integral",
            QoiKind::At(t) => return format!("at({}, {t})", self.var),
        };
        match self.window {
            Some((a, b)) => format!("{k}({}, {a}, {b})", self.var),
            None => format!("{k}({})", self.var),
        }
    }

    /// Evaluate on a trajectory `y` sampled at `t` (trapezoidal integral, time-weighted mean).
    pub fn eval(&self, t: &[f64], y: &[f64]) -> f64 {
        if let QoiKind::At(tq) = self.kind {
            return interp(t, y, tq);
        }
        let (a, b) = self.window.unwrap_or((f64::NEG_INFINITY, f64::INFINITY));
        let idx: Vec<usize> = (0..t.len()).filter(|&i| t[i] >= a && t[i] <= b).collect();
        if idx.is_empty() {
            return f64::NAN;
        }
        let integral = || {
            idx.windows(2)
                .map(|w| 0.5 * (t[w[1]] - t[w[0]]) * (y[w[0]] + y[w[1]]))
                .sum::<f64>()
        };
        match self.kind {
            QoiKind::Final => y[*idx.last().unwrap()],
            QoiKind::Max => idx.iter().map(|&i| y[i]).fold(f64::NEG_INFINITY, f64::max),
            QoiKind::Min => idx.iter().map(|&i| y[i]).fold(f64::INFINITY, f64::min),
            QoiKind::Integral => integral(),
            QoiKind::Mean => {
                let span = t[*idx.last().unwrap()] - t[idx[0]];
                if span > 0.0 {
                    integral() / span
                } else {
                    y[idx[0]]
                }
            }
            QoiKind::At(_) => unreachable!(),
        }
    }
}

/// Linear interpolation of `y(t)` at `tq` (clamped to the ends).
pub fn interp(t: &[f64], y: &[f64], tq: f64) -> f64 {
    if t.is_empty() {
        return f64::NAN;
    }
    let i = t.partition_point(|&x| x <= tq);
    if i == 0 {
        return y[0];
    }
    if i >= t.len() {
        return y[t.len() - 1];
    }
    let (t0, t1) = (t[i - 1], t[i]);
    if t1 == t0 {
        return y[i];
    }
    let w = (tq - t0) / (t1 - t0);
    y[i - 1] + w * (y[i] - y[i - 1])
}

/// Resample `y(t)` onto `grid` (both increasing).
pub fn resample(t: &[f64], y: &[f64], grid: &[f64]) -> Vec<f64> {
    if t.len() == grid.len() && t.iter().zip(grid).all(|(a, b)| a == b) {
        return y.to_vec();
    }
    grid.iter().map(|&g| interp(t, y, g)).collect()
}

/// The output time grid of a run with these options (as the simulator records it
/// without event rows).
pub fn output_grid(o: &SimOptions) -> Vec<f64> {
    let span = o.stop_time - o.start_time;
    let dt = if o.interval > 0.0 {
        o.interval
    } else {
        span / o.intervals.max(1) as f64
    };
    let mut g = Vec::new();
    if dt.is_nan() || dt <= 0.0 {
        g.push(o.start_time);
        return g;
    }
    let mut k = 0u64;
    loop {
        let t = o.start_time + k as f64 * dt;
        if t > o.stop_time * (1.0 + 1e-14) + 1e-300 {
            break;
        }
        g.push(t.min(o.stop_time));
        k += 1;
    }
    if g.last()
        .is_none_or(|&l| l < o.stop_time - 1e-12 * o.stop_time.abs().max(1.0))
    {
        g.push(o.stop_time);
    }
    g
}

/// The parameter `name` if its value can be changed (and differentiated) at run time:
/// one of the model's free parameters. Otherwise an error that says why.
pub fn free_param(m: &Model, name: &str) -> Result<VarId, String> {
    if let Some(&p) = m
        .free_params
        .iter()
        .find(|&&p| m.vars[p as usize].name == name)
    {
        return Ok(p);
    }
    let list = || {
        let names: Vec<&str> = m
            .free_params
            .iter()
            .map(|&p| m.vars[p as usize].name.as_str())
            .filter(|n| !n.starts_with('$'))
            .take(12)
            .collect();
        if names.is_empty() {
            "the model has no free parameters".to_string()
        } else {
            format!("free parameters: {}", names.join(", "))
        }
    };
    match m.vars.iter().find(|v| v.name == name) {
        Some(v) if matches!(v.kind, VarKind::Parameter | VarKind::Input) => Err(format!(
            "parameter '{name}' is computed from other parameters (or protected) and cannot be \
             varied on its own; vary the parameters it depends on ({})",
            list()
        )),
        Some(_) => Err(format!("'{name}' is not a parameter ({})", list())),
        None => Err(format!(
            "unknown parameter '{name}': it does not exist or was evaluated when the model was \
             translated (structural or Evaluate=true parameters cannot be varied without \
             re-translating) ({})",
            list()
        )),
    }
}

/// Check that `name` is a variable that results can report.
pub fn check_var(m: &Model, name: &str) -> Result<VarId, String> {
    m.vars
        .iter()
        .position(|v| v.name == name && !v.name.starts_with('$'))
        .map(|i| i as VarId)
        .ok_or_else(|| format!("unknown variable '{name}'"))
}

/// Simulate with `params` overriding the base options' parameters and return the
/// trajectories of `vars` resampled onto `grid`.
pub fn run_outputs(
    m: &Model,
    base: &SimOptions,
    params: &[(String, f64)],
    vars: &[String],
    grid: &[f64],
) -> Result<Vec<Vec<f64>>, String> {
    let mut o = base.clone();
    o.store_events = false;
    for (n, v) in params {
        match o.params.iter_mut().find(|(k, _)| k == n) {
            Some(e) => e.1 = *v,
            None => o.params.push((n.clone(), *v)),
        }
    }
    let (res, _) = simulate(m, o).map_err(|e| e.message)?;
    if let Some(l) = res.log.iter().find(|l| l.starts_with("terminated"))
        && res
            .time
            .last()
            .is_some_and(|&t| t < base.stop_time * (1.0 - 1e-9))
    {
        return Err(format!("the run stopped early ({l})"));
    }
    vars.iter()
        .map(|n| {
            let y = res
                .values(n)
                .ok_or_else(|| format!("unknown variable '{n}'"))?;
            Ok(resample(&res.time, &y, grid))
        })
        .collect()
}

/// Value of the parameters `names` in the model with `opts` (overrides applied).
pub fn param_values(m: &Model, opts: &SimOptions, names: &[String]) -> Result<Vec<f64>, String> {
    let mut sim = crate::sim::Simulation::new(m, opts.clone());
    sim.initialize()?;
    names
        .iter()
        .map(|n| {
            let v = check_var(m, n)?;
            Ok(sim.rt.r[v as usize])
        })
        .collect()
}

// ---------------------------------------------------------------------------------
// Jobs
// ---------------------------------------------------------------------------------

/// An analysis that advances in steps (one or a few simulations each), so a single
/// browser worker can report progress and accept a cancel between steps.
pub trait Job {
    /// Do the next piece of work; returns true once the analysis is complete.
    fn step(&mut self) -> Result<bool, String>;
    /// `(done, total)` units of work.
    fn progress(&self) -> (usize, usize);
    /// The result so far (complete after `step` returned true) as JSON.
    fn result_json(&self) -> serde_json::Value;
    /// Human-readable summary of the result.
    fn report(&self) -> String;
}

/// Drive a job to completion, calling `progress(done, total)` after every step; the
/// job stops early (with an error) when `progress` returns false.
pub fn run_job(
    job: &mut dyn Job,
    progress: &mut dyn FnMut(usize, usize) -> bool,
) -> Result<(), String> {
    loop {
        let done = job.step()?;
        let (d, t) = job.progress();
        if !progress(d, t) {
            return Err("cancelled".into());
        }
        if done {
            return Ok(());
        }
    }
}

// ---------------------------------------------------------------------------------
// Global analysis
// ---------------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum GsaMode {
    #[default]
    MonteCarlo,
    Sobol,
    Morris,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParamDist {
    pub name: String,
    pub dist: Dist,
}

/// What to analyse.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct GsaSpec {
    #[serde(default)]
    pub mode: GsaMode,
    pub params: Vec<ParamDist>,
    /// Trajectories to collect statistics of (Monte Carlo).
    #[serde(default)]
    pub outputs: Vec<String>,
    /// Scalar quantities of interest (`final(x)`, `max(x)`, ...).
    #[serde(default)]
    pub qois: Vec<String>,
    /// Monte Carlo: samples. Sobol: base samples N (runs N (k + 2)). Morris:
    /// trajectories r (runs r (k + 1)).
    pub n: usize,
    #[serde(default)]
    pub sampling: Sampling,
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// Morris grid levels.
    #[serde(default = "default_levels")]
    pub levels: usize,
}

fn default_seed() -> u64 {
    12345
}
fn default_levels() -> usize {
    4
}

/// Outputs of one run: trajectories of the outputs and the quantities of interest.
#[derive(Clone, Debug, Default, Serialize)]
pub struct RunOut {
    pub traj: Vec<Vec<f64>>,
    pub qoi: Vec<f64>,
}

/// Statistics of a trajectory over the samples, per time point.
#[derive(Clone, Debug, Serialize)]
pub struct Band {
    pub name: String,
    pub mean: Vec<f64>,
    pub std: Vec<f64>,
    pub q05: Vec<f64>,
    pub q50: Vec<f64>,
    pub q95: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ScalarStats {
    pub name: String,
    pub mean: f64,
    pub std: f64,
    pub q05: f64,
    pub q50: f64,
    pub q95: f64,
    pub min: f64,
    pub max: f64,
}

#[derive(Clone, Debug, Serialize)]
pub struct SobolIndices {
    pub qoi: String,
    pub mean: f64,
    pub variance: f64,
    /// First-order indices per parameter (Saltelli 2010).
    pub s1: Vec<f64>,
    /// Total-effect indices (Jansen 1999).
    pub st: Vec<f64>,
    /// 95 % bootstrap half-widths.
    pub s1_conf: Vec<f64>,
    pub st_conf: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MorrisEffects {
    pub qoi: String,
    /// Mean of the elementary effects (unit-scaled parameter ranges).
    pub mu: Vec<f64>,
    /// Mean of their absolute values.
    pub mu_star: Vec<f64>,
    /// Their standard deviation (nonlinearity / interactions).
    pub sigma: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct GsaResult {
    pub model: String,
    pub mode: GsaMode,
    pub params: Vec<String>,
    pub runs: usize,
    pub failed: usize,
    /// First errors of failed runs.
    pub errors: Vec<String>,
    pub time: Vec<f64>,
    /// Monte Carlo trajectory statistics.
    pub bands: Vec<Band>,
    /// Statistics of the quantities of interest over all runs.
    pub qois: Vec<ScalarStats>,
    pub sobol: Vec<SobolIndices>,
    pub morris: Vec<MorrisEffects>,
}

/// A planned global analysis and its run outputs.
pub struct GsaJob {
    m: Model,
    opts: SimOptions,
    spec: GsaSpec,
    qois: Vec<Qoi>,
    /// Variables simulated per run: outputs, then QoI variables not among them.
    vars: Vec<String>,
    qoi_var: Vec<usize>,
    grid: Vec<f64>,
    /// Parameter values per run.
    samples: Vec<Vec<f64>>,
    /// Morris: the step (+-Delta in unit space) of each run relative to the previous one,
    /// as (parameter, delta); None for a trajectory's base point.
    morris_steps: Vec<Option<(usize, f64)>>,
    results: Vec<Option<RunOut>>,
    errors: Vec<String>,
    next: usize,
}

impl GsaJob {
    pub fn new(m: Model, opts: SimOptions, spec: GsaSpec) -> Result<GsaJob, String> {
        if spec.params.is_empty() {
            return Err("no parameters to vary".into());
        }
        for p in &spec.params {
            free_param(&m, &p.name)?;
            p.dist.validate().map_err(|e| format!("{}: {e}", p.name))?;
        }
        let qois: Vec<Qoi> = spec
            .qois
            .iter()
            .map(|q| Qoi::parse(q))
            .collect::<Result<_, _>>()?;
        if spec.mode != GsaMode::MonteCarlo && qois.is_empty() {
            return Err("Sobol and Morris analyses need at least one quantity of interest".into());
        }
        if spec.mode == GsaMode::MonteCarlo && spec.outputs.is_empty() && qois.is_empty() {
            return Err("no outputs or quantities of interest given".into());
        }
        if spec.n == 0 {
            return Err("the number of samples must be positive".into());
        }
        let mut vars = spec.outputs.clone();
        let mut qoi_var = Vec::new();
        for q in &qois {
            let i = match vars.iter().position(|v| *v == q.var) {
                Some(i) => i,
                None => {
                    vars.push(q.var.clone());
                    vars.len() - 1
                }
            };
            qoi_var.push(i);
        }
        for v in &vars {
            check_var(&m, v)?;
        }
        let grid = output_grid(&opts);
        let k = spec.params.len();
        let mut rng = Rng::new(spec.seed);
        let to_values = |u: &[f64]| -> Vec<f64> {
            spec.params
                .iter()
                .zip(u)
                .map(|(p, &x)| p.dist.quantile(x))
                .collect()
        };
        let mut samples = Vec::new();
        let mut morris_steps = Vec::new();
        match spec.mode {
            GsaMode::MonteCarlo => {
                for u in unit_samples(spec.n, k, spec.sampling, &mut rng) {
                    samples.push(to_values(&u));
                }
            }
            GsaMode::Sobol => {
                // Rows of A and B from one 2k-dimensional design; per base row the runs
                // are A, B, then A with column i taken from B (i = 1..k).
                for u in unit_samples(spec.n, 2 * k, spec.sampling, &mut rng) {
                    let (a, b) = u.split_at(k);
                    samples.push(to_values(a));
                    samples.push(to_values(b));
                    for i in 0..k {
                        let mut ab = a.to_vec();
                        ab[i] = b[i];
                        samples.push(to_values(&ab));
                    }
                }
            }
            GsaMode::Morris => {
                let p = spec.levels.max(2);
                let delta = p as f64 / (2.0 * (p - 1) as f64);
                let to_u = |x: f64, d: &Dist| {
                    if d.bounded() {
                        x.clamp(1e-12, 1.0 - 1e-12)
                    } else {
                        0.001 + 0.998 * x
                    }
                };
                for _ in 0..spec.n {
                    let mut x: Vec<f64> = (0..k)
                        .map(|_| rng.below(p) as f64 / (p - 1) as f64)
                        .collect();
                    let unit = |x: &[f64]| -> Vec<f64> {
                        x.iter()
                            .zip(&spec.params)
                            .map(|(&v, pd)| to_u(v, &pd.dist))
                            .collect()
                    };
                    samples.push(to_values(&unit(&x)));
                    morris_steps.push(None);
                    let mut order: Vec<usize> = (0..k).collect();
                    rng.shuffle(&mut order);
                    for i in order {
                        let d = if x[i] + delta <= 1.0 + 1e-12 {
                            delta
                        } else {
                            -delta
                        };
                        x[i] += d;
                        samples.push(to_values(&unit(&x)));
                        morris_steps.push(Some((i, d)));
                    }
                }
            }
        }
        let n = samples.len();
        Ok(GsaJob {
            m,
            opts,
            spec,
            qois,
            vars,
            qoi_var,
            grid,
            samples,
            morris_steps,
            results: vec![None; n],
            errors: Vec::new(),
            next: 0,
        })
    }

    /// Parameter values of every run (the plan).
    pub fn samples(&self) -> &[Vec<f64>] {
        &self.samples
    }

    pub fn model(&self) -> &Model {
        &self.m
    }

    /// Simulate run `i` (independent of every other run).
    pub fn run_sample(&self, i: usize) -> Result<RunOut, String> {
        let params: Vec<(String, f64)> = self
            .spec
            .params
            .iter()
            .zip(&self.samples[i])
            .map(|(p, &v)| (p.name.clone(), v))
            .collect();
        let traj = run_outputs(&self.m, &self.opts, &params, &self.vars, &self.grid)?;
        let qoi = self
            .qois
            .iter()
            .zip(&self.qoi_var)
            .map(|(q, &vi)| q.eval(&self.grid, &traj[vi]))
            .collect();
        let keep = if self.spec.mode == GsaMode::MonteCarlo {
            self.spec.outputs.len()
        } else {
            0
        };
        Ok(RunOut {
            traj: traj.into_iter().take(keep).collect(),
            qoi,
        })
    }

    /// Store the outcome of run `i` (computed here or elsewhere).
    pub fn set_result(&mut self, i: usize, r: Result<RunOut, String>) {
        match r {
            Ok(o) => self.results[i] = Some(o),
            Err(e) => {
                if self.errors.len() < 5 {
                    self.errors.push(format!("run {}: {e}", i + 1));
                }
            }
        }
    }

    /// Run the remaining samples on `threads` threads, calling `progress(done, total)`
    /// about ten times a second; it returns false to cancel.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn run_parallel(
        &mut self,
        threads: usize,
        progress: &mut dyn FnMut(usize, usize) -> bool,
    ) -> Result<(), String> {
        use std::sync::Mutex;
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let total = self.samples.len();
        let start = self.next;
        let next = AtomicUsize::new(start);
        let done = AtomicUsize::new(start);
        let cancel = AtomicBool::new(false);
        let out: Mutex<Vec<(usize, Result<RunOut, String>)>> = Mutex::new(Vec::new());
        let this = &*self;
        std::thread::scope(|s| {
            for _ in 0..threads.max(1) {
                s.spawn(|| {
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= total {
                            break;
                        }
                        let r = this.run_sample(i);
                        out.lock().unwrap().push((i, r));
                        done.fetch_add(1, Ordering::Relaxed);
                    }
                });
            }
            loop {
                let d = done.load(Ordering::Relaxed);
                if !progress(d, total) {
                    cancel.store(true, Ordering::Relaxed);
                    break;
                }
                if d >= total {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
        });
        let mut out = out.into_inner().unwrap();
        out.sort_by_key(|(i, _)| *i);
        for (i, r) in out {
            self.set_result(i, r);
        }
        self.next = total;
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        Ok(())
    }

    /// Aggregate the run outputs.
    pub fn result(&self) -> GsaResult {
        let k = self.spec.params.len();
        let ok: Vec<&RunOut> = self.results.iter().flatten().collect();
        let failed = self.next.min(self.samples.len()) - ok.len();
        let mut r = GsaResult {
            model: self.m.name.clone(),
            mode: self.spec.mode,
            params: self.spec.params.iter().map(|p| p.name.clone()).collect(),
            runs: self.next,
            failed,
            errors: self.errors.clone(),
            time: self.grid.clone(),
            bands: Vec::new(),
            qois: Vec::new(),
            sobol: Vec::new(),
            morris: Vec::new(),
        };
        for (qi, q) in self.qois.iter().enumerate() {
            let v: Vec<f64> = ok.iter().map(|o| o.qoi[qi]).collect();
            r.qois.push(scalar_stats(&q.label(), &v));
        }
        match self.spec.mode {
            GsaMode::MonteCarlo => {
                for (oi, name) in self.spec.outputs.iter().enumerate() {
                    let trajs: Vec<&[f64]> = ok.iter().map(|o| o.traj[oi].as_slice()).collect();
                    r.bands.push(band(name, &trajs, self.grid.len()));
                }
            }
            GsaMode::Sobol => {
                let per = k + 2;
                for (qi, q) in self.qois.iter().enumerate() {
                    // Base rows whose k + 2 runs all succeeded.
                    let mut fa = Vec::new();
                    let mut fb = Vec::new();
                    let mut fab = vec![Vec::new(); k];
                    for row in 0..self.spec.n {
                        let vals: Option<Vec<f64>> = (0..per)
                            .map(|j| {
                                self.results
                                    .get(row * per + j)
                                    .and_then(|o| o.as_ref())
                                    .map(|o| o.qoi[qi])
                                    .filter(|x| x.is_finite())
                            })
                            .collect();
                        if let Some(v) = vals {
                            fa.push(v[0]);
                            fb.push(v[1]);
                            for i in 0..k {
                                fab[i].push(v[2 + i]);
                            }
                        }
                    }
                    let (s1, st) = sobol_indices(&fa, &fb, &fab);
                    let (s1_conf, st_conf) = sobol_bootstrap(&fa, &fb, &fab, 200, self.spec.seed);
                    let all: Vec<f64> = fa.iter().chain(&fb).copied().collect();
                    let (mean, var) = mean_var(&all);
                    r.sobol.push(SobolIndices {
                        qoi: q.label(),
                        mean,
                        variance: var,
                        s1,
                        st,
                        s1_conf,
                        st_conf,
                    });
                }
            }
            GsaMode::Morris => {
                for (qi, q) in self.qois.iter().enumerate() {
                    let mut ee: Vec<Vec<f64>> = vec![Vec::new(); k];
                    for j in 0..self.results.len() {
                        if let Some((i, d)) = self.morris_steps[j] {
                            let (Some(a), Some(b)) = (&self.results[j - 1], &self.results[j])
                            else {
                                continue;
                            };
                            let e = (b.qoi[qi] - a.qoi[qi]) / d;
                            if e.is_finite() {
                                ee[i].push(e);
                            }
                        }
                    }
                    r.morris.push(morris_stats(&q.label(), &ee));
                }
            }
        }
        r
    }

    pub fn report_of(r: &GsaResult) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let mode = match r.mode {
            GsaMode::MonteCarlo => "Monte Carlo",
            GsaMode::Sobol => "Sobol indices",
            GsaMode::Morris => "Morris elementary effects",
        };
        let _ = writeln!(
            s,
            "{mode} of {}: {} runs, {} failed",
            r.model, r.runs, r.failed
        );
        for e in &r.errors {
            let _ = writeln!(s, "  {e}");
        }
        if !r.qois.is_empty() {
            let _ = writeln!(
                s,
                "\n{:<28} {:>12} {:>12} {:>12} {:>12} {:>12}",
                "quantity", "mean", "std", "5%", "50%", "95%"
            );
            for q in &r.qois {
                let _ = writeln!(
                    s,
                    "{:<28} {:>12.5e} {:>12.5e} {:>12.5e} {:>12.5e} {:>12.5e}",
                    q.name, q.mean, q.std, q.q05, q.q50, q.q95
                );
            }
        }
        for b in &r.bands {
            let n = b.mean.len();
            if n == 0 {
                continue;
            }
            let _ = writeln!(
                s,
                "\n{} at t = {}: mean {:.5e}, std {:.5e}, 5..95% [{:.5e}, {:.5e}]",
                b.name,
                r.time[n - 1],
                b.mean[n - 1],
                b.std[n - 1],
                b.q05[n - 1],
                b.q95[n - 1]
            );
        }
        for so in &r.sobol {
            let _ = writeln!(
                s,
                "\n{} (mean {:.5e}, variance {:.5e}):",
                so.qoi, so.mean, so.variance
            );
            let _ = writeln!(s, "  {:<24} {:>16} {:>16}", "parameter", "S1", "ST");
            for (i, p) in r.params.iter().enumerate() {
                let _ = writeln!(
                    s,
                    "  {:<24} {:>8.4} +-{:<6.3} {:>8.4} +-{:<6.3}",
                    p, so.s1[i], so.s1_conf[i], so.st[i], so.st_conf[i]
                );
            }
        }
        for mo in &r.morris {
            let _ = writeln!(s, "\n{}:", mo.qoi);
            let _ = writeln!(
                s,
                "  {:<24} {:>12} {:>12} {:>12}",
                "parameter", "mu*", "mu", "sigma"
            );
            let mut idx: Vec<usize> = (0..r.params.len()).collect();
            idx.sort_by(|&a, &b| mo.mu_star[b].total_cmp(&mo.mu_star[a]));
            for i in idx {
                let _ = writeln!(
                    s,
                    "  {:<24} {:>12.5e} {:>12.5e} {:>12.5e}",
                    r.params[i], mo.mu_star[i], mo.mu[i], mo.sigma[i]
                );
            }
        }
        s
    }
}

impl Job for GsaJob {
    fn step(&mut self) -> Result<bool, String> {
        if self.next < self.samples.len() {
            let r = self.run_sample(self.next);
            self.set_result(self.next, r);
            self.next += 1;
        }
        Ok(self.next >= self.samples.len())
    }

    fn progress(&self) -> (usize, usize) {
        (self.next, self.samples.len())
    }

    fn result_json(&self) -> serde_json::Value {
        serde_json::to_value(self.result()).unwrap_or_default()
    }

    fn report(&self) -> String {
        GsaJob::report_of(&self.result())
    }
}

fn mean_var(v: &[f64]) -> (f64, f64) {
    if v.is_empty() {
        return (f64::NAN, f64::NAN);
    }
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
    (mean, var)
}

/// Quantile of sorted values (linear interpolation between order statistics).
pub fn quantile_sorted(s: &[f64], q: f64) -> f64 {
    if s.is_empty() {
        return f64::NAN;
    }
    let h = q * (s.len() - 1) as f64;
    let i = h.floor() as usize;
    let f = h - i as f64;
    if i + 1 < s.len() {
        s[i] + f * (s[i + 1] - s[i])
    } else {
        s[s.len() - 1]
    }
}

pub fn scalar_stats(name: &str, v: &[f64]) -> ScalarStats {
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    s.sort_by(f64::total_cmp);
    let (mean, var) = mean_var(&s);
    let std = if s.len() > 1 {
        (var * s.len() as f64 / (s.len() - 1) as f64).sqrt()
    } else {
        0.0
    };
    ScalarStats {
        name: name.to_string(),
        mean,
        std,
        q05: quantile_sorted(&s, 0.05),
        q50: quantile_sorted(&s, 0.5),
        q95: quantile_sorted(&s, 0.95),
        min: s.first().copied().unwrap_or(f64::NAN),
        max: s.last().copied().unwrap_or(f64::NAN),
    }
}

/// Per-time-point statistics of trajectories on a common grid.
pub fn band(name: &str, trajs: &[&[f64]], len: usize) -> Band {
    let mut b = Band {
        name: name.to_string(),
        mean: Vec::with_capacity(len),
        std: Vec::with_capacity(len),
        q05: Vec::with_capacity(len),
        q50: Vec::with_capacity(len),
        q95: Vec::with_capacity(len),
    };
    let mut col = Vec::with_capacity(trajs.len());
    for t in 0..len {
        col.clear();
        col.extend(trajs.iter().map(|y| y[t]).filter(|x| x.is_finite()));
        let s = scalar_stats("", &col);
        b.mean.push(s.mean);
        b.std.push(s.std);
        b.q05.push(s.q05);
        b.q50.push(s.q50);
        b.q95.push(s.q95);
    }
    b
}

/// First-order (Saltelli 2010) and total (Jansen 1999) Sobol indices from the model
/// values on the rows of A, B and A with column i from B (`fab[i]`).
pub fn sobol_indices(fa: &[f64], fb: &[f64], fab: &[Vec<f64>]) -> (Vec<f64>, Vec<f64>) {
    let n = fa.len();
    let k = fab.len();
    if n < 2 {
        return (vec![f64::NAN; k], vec![f64::NAN; k]);
    }
    let all: Vec<f64> = fa.iter().chain(fb).copied().collect();
    let (f0, var) = mean_var(&all);
    let mut s1 = Vec::with_capacity(k);
    let mut st = Vec::with_capacity(k);
    for f in fab {
        // f(B) is centred on the mean: same expectation, far less variance when the
        // mean is large against the spread.
        let v1 = (0..n).map(|j| (fb[j] - f0) * (f[j] - fa[j])).sum::<f64>() / n as f64;
        let vt = (0..n).map(|j| (fa[j] - f[j]).powi(2)).sum::<f64>() / (2.0 * n as f64);
        s1.push(if var > 0.0 { v1 / var } else { 0.0 });
        st.push(if var > 0.0 { vt / var } else { 0.0 });
    }
    (s1, st)
}

/// 95 % confidence half-widths of the Sobol indices by bootstrap over the base rows.
pub fn sobol_bootstrap(
    fa: &[f64],
    fb: &[f64],
    fab: &[Vec<f64>],
    resamples: usize,
    seed: u64,
) -> (Vec<f64>, Vec<f64>) {
    let n = fa.len();
    let k = fab.len();
    if n < 2 {
        return (vec![f64::NAN; k], vec![f64::NAN; k]);
    }
    let mut rng = Rng::new(seed ^ 0x5eed_b007);
    let mut s1s = vec![Vec::new(); k];
    let mut sts = vec![Vec::new(); k];
    let mut a = vec![0.0; n];
    let mut b = vec![0.0; n];
    let mut ab = vec![vec![0.0; n]; k];
    for _ in 0..resamples {
        for j in 0..n {
            let r = rng.below(n);
            a[j] = fa[r];
            b[j] = fb[r];
            for i in 0..k {
                ab[i][j] = fab[i][r];
            }
        }
        let (s1, st) = sobol_indices(&a, &b, &ab);
        for i in 0..k {
            s1s[i].push(s1[i]);
            sts[i].push(st[i]);
        }
    }
    let conf = |v: &Vec<f64>| {
        let (_, var) = mean_var(v);
        1.96 * var.sqrt()
    };
    (
        s1s.iter().map(conf).collect(),
        sts.iter().map(conf).collect(),
    )
}

/// mu, mu* and sigma of elementary effects per parameter.
pub fn morris_stats(name: &str, ee: &[Vec<f64>]) -> MorrisEffects {
    let mut m = MorrisEffects {
        qoi: name.to_string(),
        mu: Vec::new(),
        mu_star: Vec::new(),
        sigma: Vec::new(),
    };
    for e in ee {
        let n = e.len();
        if n == 0 {
            m.mu.push(f64::NAN);
            m.mu_star.push(f64::NAN);
            m.sigma.push(f64::NAN);
            continue;
        }
        let mu = e.iter().sum::<f64>() / n as f64;
        let mu_star = e.iter().map(|x| x.abs()).sum::<f64>() / n as f64;
        let sigma = if n > 1 {
            (e.iter().map(|x| (x - mu).powi(2)).sum::<f64>() / (n - 1) as f64).sqrt()
        } else {
            0.0
        };
        m.mu.push(mu);
        m.mu_star.push(mu_star);
        m.sigma.push(sigma);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_quantiles() {
        for (p, x) in [
            (0.5, 0.0),
            (0.975, 1.959_963_984_540_054),
            (0.05, -1.644_853_626_951_472_2),
        ] {
            assert!((norm_inv(p) - x).abs() < 1e-9, "{p}: {}", norm_inv(p));
        }
    }

    #[test]
    fn lhs_strata() {
        let mut rng = Rng::new(1);
        let pts = unit_samples(50, 3, Sampling::Lhs, &mut rng);
        for d in 0..3 {
            let mut bins: Vec<usize> = pts.iter().map(|p| (p[d] * 50.0) as usize).collect();
            bins.sort();
            assert_eq!(bins, (0..50).collect::<Vec<_>>());
        }
    }

    #[test]
    fn qoi_parse_eval() {
        let t: Vec<f64> = (0..=100).map(|i| i as f64 / 100.0).collect();
        let y: Vec<f64> = t.iter().map(|x| 2.0 * x).collect();
        let q = |s: &str| Qoi::parse(s).unwrap().eval(&t, &y);
        assert!((q("final(y)") - 2.0).abs() < 1e-12);
        assert!((q("max(y, 0, 0.5)") - 1.0).abs() < 1e-12);
        assert!((q("integral(y)") - 1.0).abs() < 1e-12);
        assert!((q("mean(y)") - 1.0).abs() < 1e-12);
        assert!((q("at(y, 0.255)") - 0.51).abs() < 1e-12);
        assert!(Qoi::parse("foo(y)").is_err());
    }
}
