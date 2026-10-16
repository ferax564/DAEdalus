//! Design of experiments: plans of simulation runs over factor ranges, quadratic
//! response surfaces fitted to scalar quantities of interest, main effects,
//! interactions and the Pareto front of two quantities.
//!
//! Designs (in coded units, `-1` = low, `+1` = high, `x = (p - center) / half-range`):
//! - `full`: every combination of each factor's levels (2 by default; 3 or more
//!   levels let the surface estimate curvature);
//! - `frac`: a two-level fractional factorial `2^(k-p)` from standard generators
//!   (minimum aberration, Montgomery's tables) of the requested resolution (IV by
//!   default), or from given generators such as `D=ABC`;
//! - `ccd`: central composite: a two-level factorial (a resolution V fraction from five
//!   factors), `2k` axial points at `+-alpha` and center points; `alpha` is `face`
//!   (1, the default: every run stays within the bounds), `rotatable`
//!   (`nf^(1/4)`), `orthogonal` or a number;
//! - `bbd`: Box–Behnken: `+-1` on every pair of factors with the others at the center,
//!   plus center points (three or more factors);
//! - `lhs`: a Latin hypercube of `samples` points (the sampler of [`crate::uq`]).
//!
//! Simulations are deterministic, so one center point is enough (replicates would
//! repeat the same run).
//!
//! The runs are independent simulations with run-time parameter overrides of free
//! parameters, so they share the compiled model: natively they run on all cores
//! ([`crate::sweep::run_parallel`]); in the browser on the worker pool. The surface is
//! the full quadratic model (intercept, linear, two-factor interactions, squares) with
//! the terms the design cannot estimate (aliased or without curvature levels) left out
//! by a rank-revealing Gram–Schmidt QR, fitted by least squares with R², adjusted R²,
//! RMSE and a coefficient table (standard errors and t values).

use crate::codegen::Model;
use crate::sim::SimOptions;
use crate::uq::{Job, Qoi, Rng, Sampling, check_var, free_param, output_grid, run_outputs};
use serde::{Deserialize, Serialize};

/// A factor: a free parameter and its range.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Factor {
    pub name: String,
    pub lo: f64,
    pub hi: f64,
    /// Levels of a full factorial (0: the design's default).
    #[serde(default)]
    pub levels: usize,
}

impl Factor {
    /// Parse `name=lo:hi` or `name=lo:hi:levels`.
    pub fn parse(s: &str) -> Result<Factor, String> {
        let bad = || format!("expected NAME=LO:HI[:LEVELS], got '{s}'");
        let (name, spec) = s.split_once('=').ok_or_else(bad)?;
        let parts: Vec<&str> = spec.split(':').map(str::trim).collect();
        if parts.len() < 2 || parts.len() > 3 {
            return Err(bad());
        }
        let num = |x: &str| {
            x.parse::<f64>()
                .map_err(|_| format!("bad number '{x}' in '{s}'"))
        };
        let f = Factor {
            name: name.trim().to_string(),
            lo: num(parts[0])?,
            hi: num(parts[1])?,
            levels: if parts.len() == 3 {
                parts[2]
                    .parse()
                    .map_err(|_| format!("bad level count '{}' in '{s}'", parts[2]))?
            } else {
                0
            },
        };
        f.validate()?;
        Ok(f)
    }

    fn validate(&self) -> Result<(), String> {
        if self.name.is_empty() {
            return Err("a factor has no name".into());
        }
        if !(self.lo.is_finite() && self.hi.is_finite() && self.hi > self.lo) {
            return Err(format!(
                "factor '{}': the range needs lo < hi, got {}:{}",
                self.name, self.lo, self.hi
            ));
        }
        if self.levels == 1 {
            return Err(format!("factor '{}': at least 2 levels", self.name));
        }
        Ok(())
    }

    pub fn natural(&self, x: f64) -> f64 {
        0.5 * (self.lo + self.hi) + 0.5 * (self.hi - self.lo) * x
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Design {
    Full,
    Frac,
    #[default]
    Ccd,
    Bbd,
    Lhs,
}

impl Design {
    pub fn parse(s: &str) -> Option<Design> {
        Some(match s.to_ascii_lowercase().as_str() {
            "full" | "factorial" | "ff" => Design::Full,
            "frac" | "fractional" | "fracfact" => Design::Frac,
            "ccd" | "ccf" | "central" | "composite" => Design::Ccd,
            "bbd" | "box-behnken" | "boxbehnken" => Design::Bbd,
            "lhs" | "latin" => Design::Lhs,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            Design::Full => "full factorial",
            Design::Frac => "fractional factorial",
            Design::Ccd => "central composite",
            Design::Bbd => "Box-Behnken",
            Design::Lhs => "Latin hypercube",
        }
    }
}

/// What to run and analyse.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoeSpec {
    pub factors: Vec<Factor>,
    #[serde(default)]
    pub design: Design,
    /// Quantities of interest (`max(x)`, `final(y)`, ... as in [`Qoi`]).
    pub qois: Vec<String>,
    /// Latin hypercube points (default `max(10, 10 k)`).
    #[serde(default)]
    pub samples: usize,
    #[serde(default = "default_seed")]
    pub seed: u64,
    /// CCD axial distance: `face` | `rotatable` | `orthogonal` | a number.
    #[serde(default)]
    pub alpha: Option<String>,
    /// Center points of CCD and Box–Behnken designs (default 1).
    #[serde(default)]
    pub center: Option<usize>,
    /// Fractional factorial generators such as `D=ABC` (factor letters in order).
    #[serde(default)]
    pub generators: Vec<String>,
    /// Smallest resolution of the default fractional factorial (default 4).
    #[serde(default)]
    pub resolution: Option<usize>,
    /// Senses of the two Pareto quantities (`min` / `max`, default both `min`).
    #[serde(default)]
    pub pareto: Vec<String>,
    /// The two quantities of the Pareto front (indices into `qois`, default 0 and 1).
    #[serde(default)]
    pub pareto_qois: Option<(usize, usize)>,
}

fn default_seed() -> u64 {
    12345
}

/// The planned runs.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DoePlan {
    pub design: Design,
    pub factors: Vec<String>,
    /// Coded points, one per run.
    pub coded: Vec<Vec<f64>>,
    /// Parameter values, one vector per run.
    pub values: Vec<Vec<f64>>,
    pub notes: Vec<String>,
}

fn letter(i: usize) -> char {
    (b'A' + i as u8) as char
}

/// Two-level full factorial in standard order (first factor fastest).
fn two_level(k: usize) -> Vec<Vec<f64>> {
    (0..1usize << k)
        .map(|r| {
            (0..k)
                .map(|j| if r >> j & 1 == 1 { 1.0 } else { -1.0 })
                .collect()
        })
        .collect()
}

/// Standard generators per (k, p): minimum aberration designs.
fn standard_generators(k: usize, p: usize) -> Option<&'static [&'static str]> {
    Some(match (k, p) {
        (3, 1) => &["C=AB"],
        (4, 1) => &["D=ABC"],
        (5, 1) => &["E=ABCD"],
        (5, 2) => &["D=AB", "E=AC"],
        (6, 1) => &["F=ABCDE"],
        (6, 2) => &["E=ABC", "F=BCD"],
        (6, 3) => &["D=AB", "E=AC", "F=BC"],
        (7, 1) => &["G=ABCDEF"],
        (7, 2) => &["F=ABCD", "G=ABDE"],
        (7, 3) => &["E=ABC", "F=BCD", "G=ACD"],
        (7, 4) => &["D=AB", "E=AC", "F=BC", "G=ABC"],
        (8, 2) => &["G=ABCD", "H=ABEF"],
        (8, 3) => &["F=ABC", "G=ABD", "H=BCDE"],
        (8, 4) => &["E=BCD", "F=ACD", "G=ABC", "H=ABD"],
        (9, 3) => &["G=ABCD", "H=ACEF", "J=CDEF"],
        (9, 4) => &["F=BCDE", "G=ACDE", "H=ABDE", "J=ABCE"],
        (10, 4) => &["G=BCDF", "H=ACDF", "J=ABDE", "K=ABCE"],
        (10, 5) => &["F=ABCD", "G=ABCE", "H=ABDE", "J=ACDE", "K=BCDE"],
        _ => return None,
    })
}

/// Factor index of a generator letter (`I` is skipped, as in the literature).
fn letter_index(c: char) -> Option<usize> {
    let c = c.to_ascii_uppercase();
    if !c.is_ascii_uppercase() || c == 'I' {
        return None;
    }
    let i = (c as u8 - b'A') as usize;
    Some(if c > 'I' { i - 1 } else { i })
}

fn factor_letter(i: usize) -> char {
    if i >= 8 { letter(i + 1) } else { letter(i) }
}

/// Parse generators `X=ABC` into (generated factor, mask of base factors).
fn parse_generators(k: usize, gens: &[String]) -> Result<Vec<(usize, u64)>, String> {
    let p = gens.len();
    if p == 0 || p >= k {
        return Err(format!("{p} generators for {k} factors"));
    }
    let mut out = Vec::new();
    for (gi, g) in gens.iter().enumerate() {
        let (lhs, rhs) = g
            .split_once('=')
            .ok_or_else(|| format!("bad generator '{g}' (like D=ABC)"))?;
        let lhs: Vec<char> = lhs.trim().chars().collect();
        let target = match lhs.as_slice() {
            [c] => letter_index(*c).ok_or_else(|| format!("bad factor letter in '{g}'"))?,
            _ => return Err(format!("bad generator '{g}' (like D=ABC)")),
        };
        if target != k - p + gi {
            return Err(format!(
                "generator '{g}': the generated factors are the last {p} ({}..{}), in order",
                factor_letter(k - p),
                factor_letter(k - 1)
            ));
        }
        let mut mask = 0u64;
        for c in rhs.trim().chars() {
            let i = letter_index(c).ok_or_else(|| format!("bad factor letter '{c}' in '{g}'"))?;
            if i >= k - p {
                return Err(format!("generator '{g}' must use base factors only"));
            }
            mask ^= 1 << i;
        }
        if mask.count_ones() < 2 {
            return Err(format!("generator '{g}' aliases a main effect"));
        }
        out.push((target, mask));
    }
    Ok(out)
}

/// Words of the defining relation and the resolution.
fn defining_relation(gens: &[(usize, u64)]) -> (Vec<u64>, usize) {
    let words: Vec<u64> = gens.iter().map(|&(t, m)| m | 1 << t).collect();
    let mut all = Vec::new();
    for s in 1..(1u64 << words.len()) {
        let mut w = 0;
        for (i, &x) in words.iter().enumerate() {
            if s >> i & 1 == 1 {
                w ^= x;
            }
        }
        all.push(w);
    }
    all.sort_by_key(|w| (w.count_ones(), *w));
    let res = all
        .iter()
        .map(|w| w.count_ones() as usize)
        .min()
        .unwrap_or(usize::MAX);
    (all, res)
}

fn word(w: u64) -> String {
    (0..64)
        .filter(|i| w >> i & 1 == 1)
        .map(factor_letter)
        .collect()
}

fn roman(r: usize) -> String {
    match r {
        3 => "III".into(),
        4 => "IV".into(),
        5 => "V".into(),
        6 => "VI".into(),
        7 => "VII".into(),
        8 => "VIII".into(),
        _ => r.to_string(),
    }
}

/// A two-level fraction from generators.
fn fraction(k: usize, gens: &[(usize, u64)]) -> Vec<Vec<f64>> {
    let base = k - gens.len();
    two_level(base)
        .into_iter()
        .map(|mut row| {
            for &(_, mask) in gens {
                let v = (0..base)
                    .filter(|i| mask >> i & 1 == 1)
                    .map(|i| row[i])
                    .product();
                row.push(v);
            }
            row
        })
        .collect()
}

/// Default fraction of resolution at least `min_res` with the fewest runs.
fn default_fraction(k: usize, min_res: usize) -> Option<Vec<(usize, u64)>> {
    for p in (1..k).rev() {
        if let Some(g) = standard_generators(k, p) {
            let gens: Vec<String> = g.iter().map(|s| s.to_string()).collect();
            let parsed = parse_generators(k, &gens).ok()?;
            if defining_relation(&parsed).1 >= min_res {
                return Some(parsed);
            }
        }
    }
    None
}

/// Plan the runs of a design.
pub fn plan(spec: &DoeSpec) -> Result<DoePlan, String> {
    let k = spec.factors.len();
    if k == 0 {
        return Err("no factors given".into());
    }
    for (i, f) in spec.factors.iter().enumerate() {
        f.validate()?;
        if spec.factors[..i].iter().any(|g| g.name == f.name) {
            return Err(format!("factor '{}' is given twice", f.name));
        }
    }
    let mut notes = Vec::new();
    let center = spec.center.unwrap_or(1);
    let coded: Vec<Vec<f64>> = match spec.design {
        Design::Full => {
            let levels: Vec<usize> = spec
                .factors
                .iter()
                .map(|f| if f.levels == 0 { 2 } else { f.levels })
                .collect();
            let total: usize = levels.iter().product();
            if total > 100_000 {
                return Err(format!("the full factorial has {total} runs"));
            }
            (0..total)
                .map(|mut r| {
                    levels
                        .iter()
                        .map(|&l| {
                            let i = r % l;
                            r /= l;
                            -1.0 + 2.0 * i as f64 / (l - 1) as f64
                        })
                        .collect()
                })
                .collect()
        }
        Design::Frac => {
            if k < 3 {
                return Err("a fractional factorial needs at least 3 factors".into());
            }
            let gens = if spec.generators.is_empty() {
                let res = spec.resolution.unwrap_or(4).max(3);
                match default_fraction(k, res) {
                    Some(g) => g,
                    None if k <= 10 => {
                        notes.push(format!(
                            "no standard fraction of resolution {} for {k} factors: full 2^{k} factorial",
                            roman(res)
                        ));
                        Vec::new()
                    }
                    None => {
                        return Err(format!(
                            "no standard fraction for {k} factors: give --generators or use lhs"
                        ));
                    }
                }
            } else {
                parse_generators(k, &spec.generators)?
            };
            if gens.is_empty() {
                two_level(k)
            } else {
                let (words, res) = defining_relation(&gens);
                notes.push(format!(
                    "2^({k}-{}) fraction, resolution {}: generators {}; defining relation I = {}",
                    gens.len(),
                    roman(res),
                    gens.iter()
                        .map(|&(t, m)| format!("{}={}", factor_letter(t), word(m)))
                        .collect::<Vec<_>>()
                        .join(", "),
                    words
                        .iter()
                        .map(|&w| word(w))
                        .collect::<Vec<_>>()
                        .join(" = ")
                ));
                notes.push(format!(
                    "factor letters: {}",
                    spec.factors
                        .iter()
                        .enumerate()
                        .map(|(i, f)| format!("{} = {}", factor_letter(i), f.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
                fraction(k, &gens)
            }
        }
        Design::Ccd => {
            if k < 2 {
                return Err("a central composite design needs at least 2 factors".into());
            }
            let fact = if k >= 5 {
                match default_fraction(k, 5) {
                    Some(g) => {
                        notes.push(format!(
                            "factorial part: resolution V fraction 2^({k}-{})",
                            g.len()
                        ));
                        fraction(k, &g)
                    }
                    None => two_level(k),
                }
            } else {
                two_level(k)
            };
            let nf = fact.len() as f64;
            let alpha = match spec.alpha.as_deref().unwrap_or("face") {
                "face" | "ccf" | "faced" => 1.0,
                "rotatable" | "rot" => nf.powf(0.25),
                "orthogonal" | "orth" => {
                    let q = ((nf + 2.0 * k as f64 + center as f64).sqrt() - nf.sqrt()).powi(2);
                    (q * nf / 4.0).powf(0.25)
                }
                s => s.parse::<f64>().ok().filter(|a| *a > 0.0).ok_or_else(|| {
                    format!("bad alpha '{s}' (face | rotatable | orthogonal | a number)")
                })?,
            };
            notes.push(format!("axial distance alpha = {alpha:.4}"));
            if alpha > 1.0 + 1e-12 {
                notes.push(
                    "axial runs lie outside the factor bounds (alpha > 1); use alpha=face to stay inside"
                        .into(),
                );
            }
            let mut pts = fact;
            for i in 0..k {
                for s in [-alpha, alpha] {
                    let mut p = vec![0.0; k];
                    p[i] = s;
                    pts.push(p);
                }
            }
            for _ in 0..center {
                pts.push(vec![0.0; k]);
            }
            pts
        }
        Design::Bbd => {
            if k < 3 {
                return Err("a Box-Behnken design needs at least 3 factors (use ccd)".into());
            }
            let mut pts = Vec::new();
            for i in 0..k {
                for j in i + 1..k {
                    for (a, b) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                        let mut p = vec![0.0; k];
                        p[i] = a;
                        p[j] = b;
                        pts.push(p);
                    }
                }
            }
            for _ in 0..center.max(1) {
                pts.push(vec![0.0; k]);
            }
            pts
        }
        Design::Lhs => {
            let n = if spec.samples == 0 {
                (10 * k).max(10)
            } else {
                spec.samples
            };
            let mut rng = Rng::new(spec.seed);
            crate::uq::unit_samples(n, k, Sampling::Lhs, &mut rng)
                .into_iter()
                .map(|u| u.into_iter().map(|x| 2.0 * x - 1.0).collect())
                .collect()
        }
    };
    let values = coded
        .iter()
        .map(|c| {
            c.iter()
                .zip(&spec.factors)
                .map(|(&x, f)| f.natural(x))
                .collect()
        })
        .collect();
    Ok(DoePlan {
        design: spec.design,
        factors: spec.factors.iter().map(|f| f.name.clone()).collect(),
        coded,
        values,
        notes,
    })
}

// ---------------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------------

/// A fitted quadratic response surface in coded units.
#[derive(Clone, Debug, Serialize)]
pub struct Surface {
    pub qoi: String,
    /// `1`, factor names, `a*b`, `a^2`.
    pub terms: Vec<String>,
    pub coef: Vec<f64>,
    pub std_err: Vec<f64>,
    pub t: Vec<f64>,
    pub r2: f64,
    pub adj_r2: f64,
    pub rmse: f64,
    /// Runs used and terms fitted.
    pub n: usize,
    pub p: usize,
    /// Terms the design cannot estimate (aliased, or no curvature levels).
    pub dropped: Vec<String>,
    /// Fitted values per run (NaN for failed runs).
    pub predicted: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct MainEffect {
    pub factor: String,
    /// Factor values (natural units) and the mean response at each.
    pub levels: Vec<f64>,
    pub means: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Interaction {
    pub a: String,
    pub b: String,
    /// Mean natural value of `a` in its low and high halves.
    pub a_levels: [f64; 2],
    pub b_levels: [f64; 2],
    /// `means[b half][a half]`.
    pub means: [[f64; 2]; 2],
}

#[derive(Clone, Debug, Serialize)]
pub struct Pareto {
    pub qois: (usize, usize),
    pub senses: (String, String),
    /// Non-dominated runs, by increasing first quantity.
    pub front: Vec<usize>,
}

#[derive(Clone, Debug, Serialize)]
pub struct DoeResult {
    pub model: String,
    pub design: Design,
    pub factors: Vec<String>,
    pub lo: Vec<f64>,
    pub hi: Vec<f64>,
    pub qois: Vec<String>,
    pub coded: Vec<Vec<f64>>,
    pub values: Vec<Vec<f64>>,
    /// `responses[q][run]` (NaN where the run failed).
    pub responses: Vec<Vec<f64>>,
    pub runs: usize,
    pub failed: usize,
    pub errors: Vec<String>,
    pub surfaces: Vec<Surface>,
    /// `main_effects[q][factor]`
    pub main_effects: Vec<Vec<MainEffect>>,
    /// `interactions[q]`, every pair of factors.
    pub interactions: Vec<Vec<Interaction>>,
    pub pareto: Option<Pareto>,
    pub notes: Vec<String>,
}

/// Least-squares quadratic surface of `y` over coded points `x`.
pub fn fit_surface(names: &[String], x: &[Vec<f64>], y: &[f64], qoi: &str) -> Surface {
    let k = names.len();
    let rows: Vec<usize> = (0..x.len()).filter(|&i| y[i].is_finite()).collect();
    let n = rows.len();
    // candidate columns
    let mut cols: Vec<(String, Box<dyn Fn(&[f64]) -> f64>)> = vec![("1".into(), Box::new(|_| 1.0))];
    for i in 0..k {
        cols.push((names[i].clone(), Box::new(move |p| p[i])));
    }
    for i in 0..k {
        for j in i + 1..k {
            cols.push((
                format!("{}*{}", names[i], names[j]),
                Box::new(move |p| p[i] * p[j]),
            ));
        }
    }
    for i in 0..k {
        cols.push((format!("{}^2", names[i]), Box::new(move |p| p[i] * p[i])));
    }
    // modified Gram-Schmidt with column dropping
    let mut q: Vec<Vec<f64>> = Vec::new();
    let mut kept: Vec<usize> = Vec::new();
    let mut r: Vec<Vec<f64>> = Vec::new(); // r[col j] = coefficients on q_0..q_j
    let mut dropped = Vec::new();
    for (ci, (name, f)) in cols.iter().enumerate() {
        let mut v: Vec<f64> = rows.iter().map(|&i| f(&x[i])).collect();
        let norm0 = v.iter().map(|a| a * a).sum::<f64>().sqrt();
        let mut rc = Vec::new();
        for qj in &q {
            let d: f64 = qj.iter().zip(&v).map(|(a, b)| a * b).sum();
            for (vi, qi) in v.iter_mut().zip(qj) {
                *vi -= d * qi;
            }
            rc.push(d);
        }
        let norm = v.iter().map(|a| a * a).sum::<f64>().sqrt();
        if norm0 == 0.0 || norm <= 1e-8 * norm0 || q.len() >= n {
            dropped.push(name.clone());
            continue;
        }
        v.iter_mut().for_each(|a| *a /= norm);
        rc.push(norm);
        q.push(v);
        r.push(rc);
        kept.push(ci);
    }
    let p = kept.len();
    let yv: Vec<f64> = rows.iter().map(|&i| y[i]).collect();
    let qty: Vec<f64> = q
        .iter()
        .map(|qj| qj.iter().zip(&yv).map(|(a, b)| a * b).sum())
        .collect();
    // back substitution R c = Q^T y, R upper triangular with R[i][j] = r[j][i]
    let mut c = vec![0.0; p];
    for i in (0..p).rev() {
        let mut s = qty[i];
        for j in i + 1..p {
            s -= r[j][i] * c[j];
        }
        c[i] = s / r[i][i];
    }
    // R^-1 rows for standard errors
    let mut rinv = vec![vec![0.0; p]; p];
    for col in 0..p {
        for i in (0..p).rev() {
            let mut s = if i == col { 1.0 } else { 0.0 };
            for j in i + 1..p {
                s -= r[j][i] * rinv[j][col];
            }
            rinv[i][col] = s / r[i][i];
        }
    }
    let predict = |pt: &[f64]| -> f64 {
        kept.iter()
            .zip(&c)
            .map(|(&ci, cv)| cv * (cols[ci].1)(pt))
            .sum()
    };
    let predicted: Vec<f64> = (0..x.len())
        .map(|i| {
            if y[i].is_finite() {
                predict(&x[i])
            } else {
                f64::NAN
            }
        })
        .collect();
    let mean = if n > 0 {
        yv.iter().sum::<f64>() / n as f64
    } else {
        f64::NAN
    };
    let sst: f64 = yv.iter().map(|v| (v - mean).powi(2)).sum();
    let sse: f64 = rows.iter().map(|&i| (y[i] - predicted[i]).powi(2)).sum();
    let r2 = if sst > 0.0 { 1.0 - sse / sst } else { 1.0 };
    let dof = n as f64 - p as f64;
    let (adj_r2, rmse) = if dof > 0.0 && n > 1 {
        let s2 = sse / dof;
        (
            if sst > 0.0 {
                1.0 - s2 / (sst / (n - 1) as f64)
            } else {
                1.0
            },
            s2.sqrt(),
        )
    } else {
        (f64::NAN, 0.0)
    };
    let std_err: Vec<f64> = (0..p)
        .map(|i| {
            if dof > 0.0 {
                rmse * rinv[i].iter().map(|a| a * a).sum::<f64>().sqrt()
            } else {
                f64::NAN
            }
        })
        .collect();
    let t = c
        .iter()
        .zip(&std_err)
        .map(|(a, s)| if *s > 0.0 { a / s } else { f64::NAN })
        .collect();
    Surface {
        qoi: qoi.to_string(),
        terms: kept.iter().map(|&i| cols[i].0.clone()).collect(),
        coef: c,
        std_err,
        t,
        r2,
        adj_r2,
        rmse,
        n,
        p,
        dropped,
        predicted,
    }
}

fn mean(v: &[f64]) -> f64 {
    let f: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if f.is_empty() {
        f64::NAN
    } else {
        f.iter().sum::<f64>() / f.len() as f64
    }
}

/// Mean response per level of each factor (continuous designs in five bins).
fn main_effects(spec: &DoeSpec, plan: &DoePlan, y: &[f64]) -> Vec<MainEffect> {
    spec.factors
        .iter()
        .enumerate()
        .map(|(j, f)| {
            let mut lv: Vec<f64> = plan
                .coded
                .iter()
                .map(|c| (c[j] * 1e6).round() / 1e6)
                .collect();
            lv.sort_by(f64::total_cmp);
            lv.dedup();
            let (levels, means) = if lv.len() <= 7 {
                let means = lv
                    .iter()
                    .map(|&l| {
                        let ys: Vec<f64> = plan
                            .coded
                            .iter()
                            .zip(y)
                            .filter(|(c, _)| ((c[j] * 1e6).round() / 1e6 - l).abs() < 1e-9)
                            .map(|(_, &v)| v)
                            .collect();
                        mean(&ys)
                    })
                    .collect();
                (lv.iter().map(|&l| f.natural(l)).collect(), means)
            } else {
                let nb = 5;
                let (mut lvl, mut mns) = (Vec::new(), Vec::new());
                for b in 0..nb {
                    let lo = -1.0 + 2.0 * b as f64 / nb as f64;
                    let hi = lo + 2.0 / nb as f64;
                    let ys: Vec<f64> = plan
                        .coded
                        .iter()
                        .zip(y)
                        .filter(|(c, _)| c[j] >= lo && (c[j] < hi || (b == nb - 1 && c[j] <= hi)))
                        .map(|(_, &v)| v)
                        .collect();
                    lvl.push(f.natural(0.5 * (lo + hi)));
                    mns.push(mean(&ys));
                }
                (lvl, mns)
            };
            MainEffect {
                factor: f.name.clone(),
                levels,
                means,
            }
        })
        .collect()
}

fn interactions(spec: &DoeSpec, plan: &DoePlan, y: &[f64]) -> Vec<Interaction> {
    let k = spec.factors.len();
    let half = |x: f64| -> Option<usize> {
        if x < -1e-9 {
            Some(0)
        } else if x > 1e-9 {
            Some(1)
        } else {
            None
        }
    };
    let mut out = Vec::new();
    for a in 0..k {
        for b in a + 1..k {
            let mut sums = [[0.0; 2]; 2];
            let mut cnt = [[0usize; 2]; 2];
            let mut av = [(0.0, 0usize); 2];
            let mut bv = [(0.0, 0usize); 2];
            for (i, c) in plan.coded.iter().enumerate() {
                let (Some(ha), Some(hb)) = (half(c[a]), half(c[b])) else {
                    continue;
                };
                av[ha].0 += plan.values[i][a];
                av[ha].1 += 1;
                bv[hb].0 += plan.values[i][b];
                bv[hb].1 += 1;
                if y[i].is_finite() {
                    sums[hb][ha] += y[i];
                    cnt[hb][ha] += 1;
                }
            }
            let m = |s: f64, c: usize| if c > 0 { s / c as f64 } else { f64::NAN };
            out.push(Interaction {
                a: spec.factors[a].name.clone(),
                b: spec.factors[b].name.clone(),
                a_levels: [m(av[0].0, av[0].1), m(av[1].0, av[1].1)],
                b_levels: [m(bv[0].0, bv[0].1), m(bv[1].0, bv[1].1)],
                means: [
                    [m(sums[0][0], cnt[0][0]), m(sums[0][1], cnt[0][1])],
                    [m(sums[1][0], cnt[1][0]), m(sums[1][1], cnt[1][1])],
                ],
            });
        }
    }
    out
}

/// Indices of the runs not dominated in `(f, g)` (both minimized).
pub fn pareto_front(f: &[f64], g: &[f64]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..f.len())
        .filter(|&i| f[i].is_finite() && g[i].is_finite())
        .collect();
    idx.sort_by(|&a, &b| f[a].total_cmp(&f[b]).then(g[a].total_cmp(&g[b])));
    let mut front = Vec::new();
    let mut best = f64::INFINITY;
    for i in idx {
        if g[i] < best {
            best = g[i];
            front.push(i);
        }
    }
    front
}

/// Fit and summarize the responses of a planned design: `responses[run][q]` (NaN for
/// failed runs).
pub fn analyze(
    model: &str,
    spec: &DoeSpec,
    plan: &DoePlan,
    responses: &[Vec<f64>],
    errors: Vec<String>,
) -> Result<DoeResult, String> {
    let labels: Vec<String> = spec
        .qois
        .iter()
        .map(|q| Qoi::parse(q).map(|q| q.label()))
        .collect::<Result<_, _>>()?;
    let nq = labels.len();
    let nr = plan.coded.len();
    let by_q: Vec<Vec<f64>> = (0..nq)
        .map(|q| {
            (0..nr)
                .map(|r| {
                    responses
                        .get(r)
                        .and_then(|v| v.get(q))
                        .copied()
                        .unwrap_or(f64::NAN)
                })
                .collect()
        })
        .collect();
    let failed = (0..nr)
        .filter(|&r| {
            by_q.iter().any(|v| !v[r].is_finite()) || nq == 0 && responses.get(r).is_none()
        })
        .count();
    let mut notes = plan.notes.clone();
    notes.push("surface coefficients in coded units: x = (p - center) / half-range".into());
    let surfaces = by_q
        .iter()
        .zip(&labels)
        .map(|(y, l)| fit_surface(&plan.factors, &plan.coded, y, l))
        .collect();
    let main = by_q.iter().map(|y| main_effects(spec, plan, y)).collect();
    let inter = by_q.iter().map(|y| interactions(spec, plan, y)).collect();
    let pareto = if nq >= 2 {
        let (a, b) = spec.pareto_qois.unwrap_or((0, 1));
        if a >= nq || b >= nq || a == b {
            return Err(format!("bad Pareto quantities ({a}, {b})"));
        }
        let sense = |i: usize| -> Result<String, String> {
            let s = spec
                .pareto
                .get(i)
                .map(|s| s.to_ascii_lowercase())
                .unwrap_or("min".into());
            match s.as_str() {
                "min" | "max" => Ok(s),
                _ => Err(format!("bad Pareto sense '{s}' (min | max)")),
            }
        };
        let (sa, sb) = (sense(0)?, sense(1)?);
        let sgn = |s: &str| if s == "max" { -1.0 } else { 1.0 };
        let f: Vec<f64> = by_q[a].iter().map(|v| sgn(&sa) * v).collect();
        let g: Vec<f64> = by_q[b].iter().map(|v| sgn(&sb) * v).collect();
        let mut front = pareto_front(&f, &g);
        front.sort_by(|&x, &y| by_q[a][x].total_cmp(&by_q[a][y]));
        Some(Pareto {
            qois: (a, b),
            senses: (sa, sb),
            front,
        })
    } else {
        None
    };
    Ok(DoeResult {
        model: model.to_string(),
        design: plan.design,
        factors: plan.factors.clone(),
        lo: spec.factors.iter().map(|f| f.lo).collect(),
        hi: spec.factors.iter().map(|f| f.hi).collect(),
        qois: labels,
        coded: plan.coded.clone(),
        values: plan.values.clone(),
        responses: by_q,
        runs: nr,
        failed,
        errors,
        surfaces,
        main_effects: main,
        interactions: inter,
        pareto,
        notes,
    })
}

impl DoeResult {
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "{} design of {}: {} runs over {}, {} failed",
            self.design.label(),
            self.model,
            self.runs,
            self.factors.join(", "),
            self.failed
        );
        for n in &self.notes {
            let _ = writeln!(s, "  note: {n}");
        }
        for e in &self.errors {
            let _ = writeln!(s, "  {e}");
        }
        for sf in &self.surfaces {
            let _ = writeln!(
                s,
                "\n{}: R^2 = {:.5}, adjusted R^2 = {:.5}, RMSE = {:.4e} ({} runs, {} terms)",
                sf.qoi, sf.r2, sf.adj_r2, sf.rmse, sf.n, sf.p
            );
            let _ = writeln!(
                s,
                "  {:<30} {:>14} {:>12} {:>11}",
                "term", "coefficient", "std error", "t"
            );
            for i in 0..sf.terms.len() {
                let _ = writeln!(
                    s,
                    "  {:<30} {:>14.6e} {:>12.4e} {:>11.3e}",
                    sf.terms[i], sf.coef[i], sf.std_err[i], sf.t[i]
                );
            }
            if !sf.dropped.is_empty() {
                let _ = writeln!(s, "  not estimable: {}", sf.dropped.join(", "));
            }
        }
        if let Some(p) = &self.pareto {
            let _ = writeln!(
                s,
                "\nPareto front of {} ({}) and {} ({}): {} runs",
                self.qois[p.qois.0],
                p.senses.0,
                self.qois[p.qois.1],
                p.senses.1,
                p.front.len()
            );
            for &i in &p.front {
                let vals: Vec<String> = self
                    .factors
                    .iter()
                    .zip(&self.values[i])
                    .map(|(f, v)| format!("{f} = {v:.6}"))
                    .collect();
                let _ = writeln!(
                    s,
                    "  run {:>4}: {:>12.5e} {:>12.5e}   {}",
                    i + 1,
                    self.responses[p.qois.0][i],
                    self.responses[p.qois.1][i],
                    vals.join(", ")
                );
            }
        }
        s
    }

    /// The runs as CSV: run, factor values, quantities.
    pub fn to_csv(&self) -> String {
        let mut s = String::from("run");
        for f in &self.factors {
            s.push_str(&format!(",\"{f}\""));
        }
        for q in &self.qois {
            s.push_str(&format!(",\"{q}\""));
        }
        s.push('\n');
        for i in 0..self.runs {
            s.push_str(&(i + 1).to_string());
            for v in &self.values[i] {
                s.push_str(&format!(",{v}"));
            }
            for q in &self.responses {
                s.push_str(&format!(",{}", q[i]));
            }
            s.push('\n');
        }
        s
    }
}

// ---------------------------------------------------------------------------------
// Running a design
// ---------------------------------------------------------------------------------

/// A planned design and its runs.
pub struct DoeJob {
    m: Model,
    opts: SimOptions,
    spec: DoeSpec,
    plan: DoePlan,
    qois: Vec<Qoi>,
    vars: Vec<String>,
    qoi_var: Vec<usize>,
    grid: Vec<f64>,
    results: Vec<Option<Vec<f64>>>,
    errors: Vec<String>,
    next: usize,
}

impl DoeJob {
    pub fn new(m: Model, opts: SimOptions, spec: DoeSpec) -> Result<DoeJob, String> {
        for f in &spec.factors {
            free_param(&m, &f.name)?;
        }
        if spec.qois.is_empty() {
            return Err("no quantities of interest given".into());
        }
        let qois: Vec<Qoi> = spec
            .qois
            .iter()
            .map(|q| Qoi::parse(q))
            .collect::<Result<_, _>>()?;
        let mut vars: Vec<String> = Vec::new();
        let mut qoi_var = Vec::new();
        for q in &qois {
            check_var(&m, &q.var)?;
            let i = match vars.iter().position(|v| *v == q.var) {
                Some(i) => i,
                None => {
                    vars.push(q.var.clone());
                    vars.len() - 1
                }
            };
            qoi_var.push(i);
        }
        let plan = plan(&spec)?;
        let mut o = opts;
        o.store_events = false;
        let grid = output_grid(&o);
        let n = plan.coded.len();
        Ok(DoeJob {
            m,
            opts: o,
            spec,
            plan,
            qois,
            vars,
            qoi_var,
            grid,
            results: vec![None; n],
            errors: Vec::new(),
            next: 0,
        })
    }

    pub fn plan(&self) -> &DoePlan {
        &self.plan
    }

    /// Simulate run `i` and evaluate the quantities.
    pub fn run_one(&self, i: usize) -> Result<Vec<f64>, String> {
        let params: Vec<(String, f64)> = self
            .plan
            .factors
            .iter()
            .cloned()
            .zip(self.plan.values[i].iter().copied())
            .collect();
        let traj = run_outputs(&self.m, &self.opts, &params, &self.vars, &self.grid)?;
        Ok(self
            .qois
            .iter()
            .zip(&self.qoi_var)
            .map(|(q, &vi)| q.eval(&self.grid, &traj[vi]))
            .collect())
    }

    fn store(&mut self, i: usize, r: Result<Vec<f64>, String>) {
        match r {
            Ok(v) => self.results[i] = Some(v),
            Err(e) => {
                if self.errors.len() < 5 {
                    self.errors.push(format!("run {}: {e}", i + 1));
                }
            }
        }
    }

    /// Run the remaining design points on `threads` native threads (the sweep runner).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn run_parallel(
        &mut self,
        threads: usize,
        progress: &(dyn Fn(usize, usize) + Sync),
    ) -> Result<(), String> {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let start = self.next;
        let total = self.plan.coded.len();
        let done = AtomicUsize::new(start);
        let this = &*self;
        let out = crate::sweep::run_parallel(
            total - start,
            threads,
            None,
            |i| this.run_one(start + i),
            |_, _| {
                let d = done.fetch_add(1, Ordering::Relaxed) + 1;
                progress(d, total);
            },
        );
        for (i, r) in out.into_iter().enumerate() {
            if let Some(r) = r {
                self.store(start + i, r);
            }
        }
        self.next = total;
        Ok(())
    }

    pub fn result(&self) -> DoeResult {
        let nq = self.qois.len();
        let resp: Vec<Vec<f64>> = self
            .results
            .iter()
            .map(|r| r.clone().unwrap_or_else(|| vec![f64::NAN; nq]))
            .collect();
        match analyze(
            &self.m.name,
            &self.spec,
            &self.plan,
            &resp,
            self.errors.clone(),
        ) {
            Ok(r) => r,
            Err(e) => {
                let mut r = analyze(
                    &self.m.name,
                    &DoeSpec {
                        pareto: Vec::new(),
                        pareto_qois: None,
                        ..self.spec.clone()
                    },
                    &self.plan,
                    &resp,
                    self.errors.clone(),
                )
                .expect("analysis without Pareto options");
                r.notes.push(e);
                r
            }
        }
    }
}

impl Job for DoeJob {
    fn step(&mut self) -> Result<bool, String> {
        if self.next < self.plan.coded.len() {
            let r = self.run_one(self.next);
            self.store(self.next, r);
            self.next += 1;
        }
        Ok(self.next >= self.plan.coded.len())
    }

    fn progress(&self) -> (usize, usize) {
        (self.next, self.plan.coded.len())
    }

    fn result_json(&self) -> serde_json::Value {
        serde_json::to_value(self.result()).unwrap_or_default()
    }

    fn report(&self) -> String {
        self.result().report()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(design: Design, k: usize) -> DoeSpec {
        DoeSpec {
            factors: (0..k)
                .map(|i| Factor {
                    name: format!("p{i}"),
                    lo: 0.0,
                    hi: 2.0,
                    levels: 0,
                })
                .collect(),
            design,
            qois: vec!["final(x)".into()],
            samples: 0,
            seed: 1,
            alpha: None,
            center: None,
            generators: Vec::new(),
            resolution: None,
            pareto: Vec::new(),
            pareto_qois: None,
        }
    }

    #[test]
    fn design_sizes() {
        assert_eq!(plan(&spec(Design::Full, 3)).unwrap().coded.len(), 8);
        assert_eq!(plan(&spec(Design::Ccd, 3)).unwrap().coded.len(), 8 + 6 + 1);
        assert_eq!(plan(&spec(Design::Bbd, 3)).unwrap().coded.len(), 12 + 1);
        assert_eq!(plan(&spec(Design::Lhs, 3)).unwrap().coded.len(), 30);
        // 2^(5-1) resolution V
        let p = plan(&spec(Design::Frac, 5)).unwrap();
        assert_eq!(p.coded.len(), 16);
        assert!(p.notes[0].contains("resolution V"), "{:?}", p.notes);
        // 2^(7-3) resolution IV
        assert_eq!(plan(&spec(Design::Frac, 7)).unwrap().coded.len(), 16);
    }

    #[test]
    fn fraction_columns_are_orthogonal() {
        let p = plan(&spec(Design::Frac, 6)).unwrap();
        let k = 6;
        for a in 0..k {
            for b in a + 1..k {
                let s: f64 = p.coded.iter().map(|r| r[a] * r[b]).sum();
                assert_eq!(s, 0.0);
            }
        }
    }

    #[test]
    fn quadratic_surface_is_recovered() {
        let mut sp = spec(Design::Ccd, 3);
        sp.alpha = Some("rotatable".into());
        let p = plan(&sp).unwrap();
        let f = |x: &[f64]| 1.0 + 2.0 * x[0] - x[1] + 0.5 * x[0] * x[2] + 3.0 * x[1] * x[1];
        let y: Vec<f64> = p.coded.iter().map(|x| f(x)).collect();
        let s = fit_surface(&p.factors, &p.coded, &y, "y");
        assert!((s.r2 - 1.0).abs() < 1e-12);
        let c = |n: &str| s.coef[s.terms.iter().position(|t| t == n).unwrap()];
        assert!((c("1") - 1.0).abs() < 1e-10);
        assert!((c("p0") - 2.0).abs() < 1e-10);
        assert!((c("p0*p2") - 0.5).abs() < 1e-10);
        assert!((c("p1^2") - 3.0).abs() < 1e-10);
        assert!(c("p2^2").abs() < 1e-10);
        // two-level designs cannot estimate squares
        let p2 = plan(&spec(Design::Full, 2)).unwrap();
        let y2: Vec<f64> = p2.coded.iter().map(|x| x[0] + x[0] * x[1]).collect();
        let s2 = fit_surface(&p2.factors, &p2.coded, &y2, "y");
        assert!(s2.dropped.contains(&"p0^2".to_string()));
    }

    #[test]
    fn pareto() {
        let f = [1.0, 2.0, 3.0, 2.5, 0.5];
        let g = [3.0, 2.0, 1.0, 2.5, 4.0];
        assert_eq!(pareto_front(&f, &g), vec![4, 0, 1, 2]);
    }
}
