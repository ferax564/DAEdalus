//! Calibration (parameter estimation): fit free parameters to measured trajectories by
//! weighted least squares, with a projected Levenberg–Marquardt method on box bounds.
//! The Jacobian comes from the local sensitivities of [`crate::sensitivity`] (exact
//! forward sensitivities where they apply, else central differences).
//!
//! The result reports the fitted values, the residual RMS before and after, approximate
//! standard errors and correlations from `J^T J`, and a Modelica modifier with the
//! fitted values to write back to the model. Dynamic optimization with constraints,
//! moving-horizon estimation and large least-squares problems are the job of the
//! CasADi export (`python/daedalus_casadi`).
//!
//! With [`CalibOptimizer::Lbfgsb`] the cost `0.5 |r|^2` is minimized by projected
//! L-BFGS ([`crate::lbfgsb`]) instead, with its gradient `J^T r` from one adjoint solve
//! per iteration (a point term per measurement, see [`crate::adjoint`]) where it
//! applies, so the cost per iteration does not grow with the number of parameters.
//! The standard errors still come from one sensitivity Jacobian at the solution.

use crate::adjoint::{GradMethod, Term, TermKind, adjoint_terms};
use crate::codegen::Model;
use crate::sensitivity::ExactError;
use crate::sensitivity::{SensMethod, SensOptions, sensitivities};
use crate::sim::SimOptions;
use crate::uq::{Job, check_var, free_param, interp, output_grid, param_values, run_outputs};
use serde::{Deserialize, Serialize};

/// A parameter to tune, with optional bounds and start value (default: the model's).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Tuner {
    pub name: String,
    #[serde(default)]
    pub lo: Option<f64>,
    #[serde(default)]
    pub hi: Option<f64>,
    #[serde(default)]
    pub start: Option<f64>,
}

impl Tuner {
    /// Parse `name=lo:hi` or `name=lo:hi:start` (empty or `inf` bounds are open).
    pub fn parse(s: &str) -> Result<Tuner, String> {
        let (name, spec) = s
            .split_once('=')
            .ok_or_else(|| format!("expected NAME=LO:HI[:START], got '{s}'"))?;
        let parts: Vec<&str> = spec.split(':').map(str::trim).collect();
        if parts.len() < 2 || parts.len() > 3 {
            return Err(format!("expected NAME=LO:HI[:START], got '{s}'"));
        }
        let num = |x: &str| -> Result<Option<f64>, String> {
            if x.is_empty() {
                return Ok(None);
            }
            let v: f64 = x
                .parse()
                .map_err(|_| format!("bad number '{x}' in '{s}'"))?;
            Ok(v.is_finite().then_some(v))
        };
        let t = Tuner {
            name: name.trim().to_string(),
            lo: num(parts[0])?,
            hi: num(parts[1])?,
            start: if parts.len() == 3 {
                num(parts[2])?
            } else {
                None
            },
        };
        if let (Some(a), Some(b)) = (t.lo, t.hi)
            && a > b
        {
            return Err(format!("lower bound above upper bound in '{s}'"));
        }
        Ok(t)
    }

    pub fn lo(&self) -> f64 {
        self.lo.unwrap_or(f64::NEG_INFINITY)
    }

    pub fn hi(&self) -> f64 {
        self.hi.unwrap_or(f64::INFINITY)
    }

    pub fn clamp(&self, v: f64) -> f64 {
        v.max(self.lo()).min(self.hi())
    }
}

/// Start values of the tuners: given, else the model's value, clamped to the bounds.
pub fn tuner_starts(m: &Model, opts: &SimOptions, tuners: &[Tuner]) -> Result<Vec<f64>, String> {
    for t in tuners {
        free_param(m, &t.name)?;
    }
    let names: Vec<String> = tuners.iter().map(|t| t.name.clone()).collect();
    let model = param_values(m, opts, &names)?;
    Ok(tuners
        .iter()
        .zip(model)
        .map(|(t, v)| t.clamp(t.start.unwrap_or(v)))
        .collect())
}

/// Options with the parameters set (overriding earlier overrides of the same name).
pub fn with_params(opts: &SimOptions, names: &[String], values: &[f64]) -> SimOptions {
    let mut o = opts.clone();
    for (n, &v) in names.iter().zip(values) {
        match o.params.iter_mut().find(|(k, _)| k == n) {
            Some(e) => e.1 = v,
            None => o.params.push((n.clone(), v)),
        }
    }
    o
}

/// A number with at most 8 significant digits, as Modelica source text.
pub fn fmt_num(v: f64) -> String {
    if v == 0.0 || !v.is_finite() {
        return format!("{}", if v.is_finite() { 0.0 } else { v });
    }
    let r: f64 = format!("{v:.7e}").parse().unwrap_or(v);
    if (1e-4..1e7).contains(&r.abs()) {
        format!("{r}")
    } else {
        format!("{r:e}")
    }
}

/// A Modelica modifier setting the parameters, nested by component:
/// `(k = 2, body(m = 1.5, J = 0.1))`.
pub fn modifier(params: &[(String, f64)]) -> String {
    #[derive(Default)]
    struct Node {
        value: Option<f64>,
        children: Vec<(String, Node)>,
    }
    fn split(name: &str) -> Vec<String> {
        let mut parts = Vec::new();
        let mut cur = String::new();
        let mut depth = 0;
        for c in name.chars() {
            match c {
                '[' => depth += 1,
                ']' => depth -= 1,
                '.' if depth == 0 => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(c);
        }
        parts.push(cur);
        parts
    }
    fn render(n: &Node) -> String {
        let items: Vec<String> = n
            .children
            .iter()
            .map(|(k, c)| match (c.value, c.children.is_empty()) {
                (Some(v), true) => format!("{k} = {}", fmt_num(v)),
                (Some(v), false) => format!("{k}{} = {}", render(c), fmt_num(v)),
                (None, _) => format!("{k}{}", render(c)),
            })
            .collect();
        format!("({})", items.join(", "))
    }
    let mut root = Node::default();
    for (name, v) in params {
        let mut node = &mut root;
        for part in split(name) {
            let i = match node.children.iter().position(|(k, _)| *k == part) {
                Some(i) => i,
                None => {
                    node.children.push((part, Node::default()));
                    node.children.len() - 1
                }
            };
            node = &mut node.children[i].1;
        }
        node.value = Some(*v);
    }
    render(&root)
}

// ---------------------------------------------------------------------------------
// Measured data
// ---------------------------------------------------------------------------------

/// A table of numbers with a header row.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Table {
    pub header: Vec<String>,
    pub cols: Vec<Vec<f64>>,
}

impl Table {
    pub fn col(&self, name: &str) -> Option<&[f64]> {
        self.header
            .iter()
            .position(|h| h == name)
            .map(|i| self.cols[i].as_slice())
    }
}

/// Parse CSV (comma, semicolon or tab separated; quoted headers; empty or non-numeric
/// cells become NaN = missing).
pub fn parse_csv(text: &str) -> Result<Table, String> {
    let mut lines = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'));
    let head = lines.next().ok_or("empty CSV")?;
    let sep = [',', ';', '\t']
        .into_iter()
        .max_by_key(|&c| head.matches(c).count())
        .unwrap();
    let unquote = |s: &str| s.trim().trim_matches('"').trim().to_string();
    let header: Vec<String> = head.split(sep).map(unquote).collect();
    if header.len() < 2 {
        return Err("the CSV needs a time column and at least one data column".into());
    }
    let mut cols = vec![Vec::new(); header.len()];
    for (ln, l) in lines.enumerate() {
        let cells: Vec<&str> = l.split(sep).collect();
        if cells.len() != header.len() {
            return Err(format!(
                "CSV row {} has {} cells, the header {}",
                ln + 2,
                cells.len(),
                header.len()
            ));
        }
        for (c, cell) in cells.iter().enumerate() {
            cols[c].push(unquote(cell).parse::<f64>().unwrap_or(f64::NAN));
        }
    }
    if cols[0].is_empty() {
        return Err("the CSV has no data rows".into());
    }
    Ok(Table { header, cols })
}

/// A data column fitted to a model variable.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DataMap {
    pub column: String,
    pub var: String,
    /// Residual weight (default: 1 / the column's standard deviation in the window).
    #[serde(default)]
    pub weight: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CalibSpec {
    pub tuners: Vec<Tuner>,
    /// Measured data as CSV text.
    pub csv: String,
    /// The time column (default: `time`, else the first column).
    #[serde(default)]
    pub time_column: Option<String>,
    /// Columns to fit (default: every column named like a model variable).
    #[serde(default)]
    pub map: Vec<DataMap>,
    /// Only measurements in this time window count.
    #[serde(default)]
    pub window: Option<(f64, f64)>,
    #[serde(default)]
    pub method: SensMethod,
    #[serde(default = "default_max_iter")]
    pub max_iter: usize,
    #[serde(default)]
    pub optimizer: CalibOptimizer,
    /// Gradient source of L-BFGS-B (default: adjoint where it applies).
    #[serde(default)]
    pub gradient: GradMethod,
}

/// The least-squares method.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum CalibOptimizer {
    /// Levenberg–Marquardt on the residual Jacobian (sensitivities of every output).
    #[default]
    Lm,
    /// Projected L-BFGS on the cost, gradients from adjoints or sensitivities.
    #[serde(alias = "l-bfgs-b", alias = "lbfgs")]
    Lbfgsb,
}

impl CalibOptimizer {
    pub fn parse(s: &str) -> Option<CalibOptimizer> {
        Some(match s.to_ascii_lowercase().as_str() {
            "lm" | "levenberg-marquardt" | "gauss-newton" => CalibOptimizer::Lm,
            "lbfgsb" | "l-bfgs-b" | "lbfgs" | "gradient" => CalibOptimizer::Lbfgsb,
            _ => return None,
        })
    }
}

fn default_max_iter() -> usize {
    40
}

#[derive(Clone, Debug, Serialize)]
pub struct FitParam {
    pub name: String,
    pub initial: f64,
    pub value: f64,
    pub lo: Option<f64>,
    pub hi: Option<f64>,
    /// Approximate standard error from `s^2 (J^T J)^-1` (NaN if not identifiable).
    pub std_error: f64,
    pub at_bound: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct ColumnFit {
    pub column: String,
    pub var: String,
    pub weight: f64,
    /// Unweighted RMS of the residuals.
    pub rms_before: f64,
    pub rms_after: f64,
    /// Measurement times and values in the window.
    pub t: Vec<f64>,
    pub measured: Vec<f64>,
    /// Simulated trajectory on `time` before and after the fit.
    pub before: Vec<f64>,
    pub after: Vec<f64>,
}

#[derive(Clone, Debug, Serialize)]
pub struct CalibResult {
    pub model: String,
    pub method: SensMethod,
    pub params: Vec<FitParam>,
    /// Correlation matrix of the estimates.
    pub correlation: Vec<Vec<f64>>,
    /// Weighted RMS of all residuals.
    pub rms_before: f64,
    pub rms_after: f64,
    pub residuals: usize,
    pub columns: Vec<ColumnFit>,
    pub time: Vec<f64>,
    pub iterations: usize,
    /// Cost `0.5 |r|^2` per iteration.
    pub history: Vec<f64>,
    pub converged: bool,
    pub message: String,
    /// The fitted values as a Modelica modifier.
    pub modifier: String,
    pub optimizer: CalibOptimizer,
    /// Gradient evaluations of L-BFGS-B and the method that computed them.
    pub gradients: usize,
    pub gradient_method: Option<GradMethod>,
    pub notes: Vec<String>,
}

/// The calibration problem and the state of its Levenberg–Marquardt iteration.
pub struct Calibration {
    m: Model,
    opts: SimOptions,
    tuners: Vec<Tuner>,
    names: Vec<String>,
    method: SensMethod,
    max_iter: usize,
    cols: Vec<(DataMap, f64, Vec<f64>, Vec<f64>)>,
    grid: Vec<f64>,
    // iteration state
    p0: Vec<f64>,
    p: Vec<f64>,
    r: Vec<f64>,
    jac: Vec<Vec<f64>>,
    cost: f64,
    lambda: f64,
    iter: usize,
    history: Vec<f64>,
    before: Vec<Vec<f64>>,
    after: Vec<Vec<f64>>,
    used: SensMethod,
    started: bool,
    done: bool,
    converged: bool,
    message: String,
    optimizer: CalibOptimizer,
    grad_method: GradMethod,
    grad: Vec<f64>,
    memory: crate::lbfgsb::Memory,
    gradients: usize,
    grad_used: Option<GradMethod>,
    notes: Vec<String>,
    scale: Vec<f64>,
}

impl Calibration {
    pub fn new(m: Model, opts: SimOptions, spec: CalibSpec) -> Result<Calibration, String> {
        if spec.tuners.is_empty() {
            return Err("no parameters to tune".into());
        }
        let table = parse_csv(&spec.csv)?;
        let tcol = match &spec.time_column {
            Some(c) => table
                .col(c)
                .ok_or_else(|| format!("no column '{c}' in the data"))?,
            None => table.col("time").unwrap_or(&table.cols[0]),
        };
        let tname = spec.time_column.clone().unwrap_or_else(|| {
            if table.col("time").is_some() {
                "time".into()
            } else {
                table.header[0].clone()
            }
        });
        let map: Vec<DataMap> = if spec.map.is_empty() {
            table
                .header
                .iter()
                .filter(|h| **h != tname && check_var(&m, h).is_ok())
                .map(|h| DataMap {
                    column: h.clone(),
                    var: h.clone(),
                    weight: None,
                })
                .collect()
        } else {
            spec.map.clone()
        };
        if map.is_empty() {
            return Err(
                "no data columns to fit: name columns like model variables or map them (column=variable)"
                    .into(),
            );
        }
        let (w0, w1) = spec.window.unwrap_or((f64::NEG_INFINITY, f64::INFINITY));
        let w0 = w0.max(opts.start_time);
        let mut cols = Vec::new();
        let mut t_end = f64::NEG_INFINITY;
        for d in map {
            check_var(&m, &d.var)?;
            let y = table
                .col(&d.column)
                .ok_or_else(|| format!("no column '{}' in the data", d.column))?;
            let (mut ts, mut ys) = (Vec::new(), Vec::new());
            for (&t, &v) in tcol.iter().zip(y) {
                if t.is_finite() && v.is_finite() && t >= w0 && t <= w1 {
                    ts.push(t);
                    ys.push(v);
                }
            }
            if ts.is_empty() {
                return Err(format!(
                    "column '{}' has no measurements in the time window",
                    d.column
                ));
            }
            t_end = t_end.max(*ts.last().unwrap());
            let weight = d.weight.unwrap_or_else(|| {
                let n = ys.len() as f64;
                let mean = ys.iter().sum::<f64>() / n;
                let sd = (ys.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n).sqrt();
                let scale = if sd > 1e-12 * mean.abs().max(1e-300) {
                    sd
                } else {
                    mean.abs().max(1.0)
                };
                1.0 / scale
            });
            cols.push((d, weight, ts, ys));
        }
        let mut o = opts;
        o.stop_time = t_end.max(o.start_time);
        o.store_events = false;
        let n_meas: usize = cols.iter().map(|c| c.2.len()).sum();
        o.interval = 0.0;
        o.intervals = o.intervals.max((4 * n_meas).min(5000));
        let grid = output_grid(&o);
        let names: Vec<String> = spec.tuners.iter().map(|t| t.name.clone()).collect();
        let p0 = tuner_starts(&m, &o, &spec.tuners)?;
        Ok(Calibration {
            optimizer: spec.optimizer,
            grad_method: spec.gradient,
            grad: Vec::new(),
            memory: crate::lbfgsb::Memory::new(8),
            gradients: 0,
            grad_used: None,
            notes: Vec::new(),
            scale: p0
                .iter()
                .map(|v| if *v != 0.0 { v.abs() } else { 1.0 })
                .collect(),
            m,
            opts: o,
            tuners: spec.tuners,
            names,
            method: spec.method,
            max_iter: spec.max_iter.max(1),
            cols,
            grid,
            p: p0.clone(),
            p0,
            r: Vec::new(),
            jac: Vec::new(),
            cost: f64::INFINITY,
            lambda: 1e-3,
            iter: 0,
            history: Vec::new(),
            before: Vec::new(),
            after: Vec::new(),
            used: SensMethod::Auto,
            started: false,
            done: false,
            converged: false,
            message: String::new(),
        })
    }

    fn vars(&self) -> Vec<String> {
        self.cols.iter().map(|c| c.0.var.clone()).collect()
    }

    /// Residuals at `p` from the trajectories of the fitted variables.
    fn residuals(&self, traj: &[Vec<f64>]) -> Vec<f64> {
        let mut r = Vec::new();
        for (k, (_, w, ts, ys)) in self.cols.iter().enumerate() {
            for (&t, &y) in ts.iter().zip(ys) {
                r.push(w * (interp(&self.grid, &traj[k], t) - y));
            }
        }
        r
    }

    /// Simulate at `p`: trajectories, and with `jac` the residual Jacobian.
    #[allow(clippy::type_complexity)]
    fn eval(
        &mut self,
        p: &[f64],
        jac: bool,
    ) -> Result<(Vec<Vec<f64>>, Option<Vec<Vec<f64>>>), String> {
        let o = with_params(&self.opts, &self.names, p);
        let vars = self.vars();
        if !jac {
            return Ok((run_outputs(&self.m, &o, &[], &vars, &self.grid)?, None));
        }
        let so = SensOptions::new(self.names.clone(), vars, self.method);
        let s = sensitivities(&self.m, &o, &so)?;
        self.used = s.method;
        let traj: Vec<Vec<f64>> = s
            .values
            .iter()
            .map(|y| crate::uq::resample(&s.time, y, &self.grid))
            .collect();
        // J[row][j] = w d y(t_i) / d p_j
        let mut jm = Vec::new();
        for (k, (_, w, ts, _)) in self.cols.iter().enumerate() {
            for &t in ts {
                jm.push(
                    (0..p.len())
                        .map(|j| w * interp(&s.time, &s.dydp[k][j], t))
                        .collect(),
                );
            }
        }
        Ok((traj, Some(jm)))
    }

    fn start(&mut self) -> Result<(), String> {
        let p = self.p.clone();
        let (traj, jac) = self.eval(&p, true)?;
        self.r = self.residuals(&traj);
        self.jac = jac.unwrap();
        self.cost = 0.5 * self.r.iter().map(|x| x * x).sum::<f64>();
        if !self.cost.is_finite() {
            return Err("the residuals are not finite at the start values".into());
        }
        self.history.push(self.cost);
        self.before = traj.clone();
        self.after = traj;
        self.started = true;
        Ok(())
    }

    /// One Levenberg–Marquardt iteration with projection onto the bounds.
    fn iterate(&mut self) -> Result<(), String> {
        let np = self.p.len();
        let mrow = self.r.len();
        let mut g = vec![0.0; np];
        let mut h = vec![0.0; np * np];
        for i in 0..mrow {
            for a in 0..np {
                g[a] += self.jac[i][a] * self.r[i];
                for b in 0..np {
                    h[a * np + b] += self.jac[i][a] * self.jac[i][b];
                }
            }
        }
        // Bounds that the gradient pushes against are held.
        let free: Vec<usize> = (0..np)
            .filter(|&i| {
                let t = &self.tuners[i];
                !((self.p[i] <= t.lo() && g[i] > 0.0) || (self.p[i] >= t.hi() && g[i] < 0.0))
            })
            .collect();
        let gnorm = free.iter().map(|&i| g[i].abs()).fold(0.0, f64::max);
        if free.is_empty() || gnorm <= 1e-14 * (1.0 + self.cost) {
            self.converged = true;
            self.message = "gradient vanishes (stationary point)".into();
            self.done = true;
            return Ok(());
        }
        let nf = free.len();
        let dmax = free.iter().map(|&i| h[i * np + i]).fold(0.0, f64::max);
        for _ in 0..12 {
            let mut a = vec![0.0; nf * nf];
            let mut rhs = vec![0.0; nf];
            for (x, &i) in free.iter().enumerate() {
                for (y, &j) in free.iter().enumerate() {
                    a[x * nf + y] = h[i * np + j];
                }
                a[x * nf + x] += self.lambda * h[i * np + i].max(1e-9 * dmax).max(1e-300);
                rhs[x] = -g[i];
            }
            let mut piv = vec![0; nf];
            let ok = crate::linalg::lu_factor(&mut a, nf, &mut piv);
            let mut pn = self.p.clone();
            if ok {
                crate::linalg::lu_solve(&a, nf, &piv, &mut rhs);
                for (x, &i) in free.iter().enumerate() {
                    pn[i] = self.tuners[i].clamp(self.p[i] + rhs[x]);
                }
            }
            let step = (0..np)
                .map(|i| (pn[i] - self.p[i]).abs() / self.p[i].abs().max(1e-12))
                .fold(0.0, f64::max);
            if ok && step < 1e-12 {
                self.converged = true;
                self.message = "step below 1e-12 (relative)".into();
                self.done = true;
                return Ok(());
            }
            let cost_new = if ok {
                match self.eval(&pn, false) {
                    Ok((traj, _)) => 0.5 * self.residuals(&traj).iter().map(|x| x * x).sum::<f64>(),
                    Err(_) => f64::INFINITY,
                }
            } else {
                f64::INFINITY
            };
            if cost_new.is_finite() && cost_new < self.cost {
                let (traj, jac) = self.eval(&pn, true)?;
                let r = self.residuals(&traj);
                let cost = 0.5 * r.iter().map(|x| x * x).sum::<f64>();
                let rel = (self.cost - cost) / self.cost.max(1e-300);
                self.p = pn;
                self.r = r;
                self.jac = jac.unwrap();
                self.cost = cost;
                self.after = traj;
                self.lambda = (self.lambda / 3.0).max(1e-12);
                self.history.push(cost);
                if rel < 1e-10 || step < 1e-9 {
                    self.converged = true;
                    self.message = "cost and step no longer decrease".into();
                    self.done = true;
                }
                return Ok(());
            }
            self.lambda *= 4.0;
            if self.lambda > 1e12 {
                break;
            }
        }
        self.converged = true;
        self.message = "no step reduces the cost further (local minimum)".into();
        self.done = true;
        Ok(())
    }

    /// Gradient `J^T r` of the cost at `p`: by the adjoint method (one backward solve with a
    /// point term per measurement) where it applies, else from the residual Jacobian.
    fn cost_gradient(&mut self, p: &[f64]) -> Result<Vec<f64>, String> {
        let o = with_params(&self.opts, &self.names, p);
        let mut why = None;
        if matches!(self.grad_method, GradMethod::Auto | GradMethod::Adjoint) {
            let vars = self.vars();
            let extra: Vec<f64> = self.cols.iter().flat_map(|c| c.2.iter().copied()).collect();
            let cols = &self.cols;
            let m = &self.m;
            let r = adjoint_terms(m, &o, &self.names, &vars, &extra, 2, &mut |grid, traj| {
                let mut terms = Vec::new();
                for (k, (d, w, ts, ys)) in cols.iter().enumerate() {
                    let var = check_var(m, &d.var)?;
                    for (&t, &y) in ts.iter().zip(ys) {
                        let res = w * (interp(grid, &traj[k], t) - y);
                        terms.push(Term {
                            var,
                            weight: res * w,
                            kind: TermKind::Point(t),
                        });
                    }
                }
                Ok(terms)
            });
            match r {
                Ok(core) => {
                    self.note_grad(GradMethod::Adjoint, None);
                    return Ok(core.grad);
                }
                Err(ExactError::NotApplicable(w)) if self.grad_method == GradMethod::Auto => {
                    why = Some(format!("forward sensitivities used because {w}"));
                }
                Err(ExactError::Failed(w)) if self.grad_method == GradMethod::Auto => {
                    why = Some(format!(
                        "forward sensitivities used because the adjoint method failed: {w}"
                    ));
                }
                Err(ExactError::NotApplicable(w)) => {
                    return Err(format!("adjoint gradients are not available: {w}"));
                }
                Err(ExactError::Failed(w)) => return Err(w),
            }
        }
        let (traj, jac) = self.eval(p, true)?;
        let r = self.residuals(&traj);
        let jac = jac.unwrap();
        let mut g = vec![0.0; p.len()];
        for (row, ri) in jac.iter().zip(&r) {
            for j in 0..g.len() {
                g[j] += row[j] * ri;
            }
        }
        let used = if self.used == SensMethod::Fd {
            GradMethod::Fd
        } else {
            GradMethod::Forward
        };
        self.note_grad(used, why);
        Ok(g)
    }

    fn note_grad(&mut self, m: GradMethod, why: Option<String>) {
        self.gradients += 1;
        if self.grad_used != Some(m) {
            self.grad_used = Some(m);
            self.notes.push(format!(
                "gradients by {}{}",
                match m {
                    GradMethod::Adjoint => "the adjoint method",
                    GradMethod::Forward => "forward sensitivities",
                    GradMethod::Fd => "finite differences",
                    GradMethod::Auto => "auto",
                },
                why.map(|w| format!(" ({w})")).unwrap_or_default()
            ));
        }
    }

    fn start_lbfgs(&mut self) -> Result<(), String> {
        let p = self.p.clone();
        let (traj, _) = self.eval(&p, false)?;
        self.r = self.residuals(&traj);
        self.cost = 0.5 * self.r.iter().map(|x| x * x).sum::<f64>();
        if !self.cost.is_finite() {
            return Err("the residuals are not finite at the start values".into());
        }
        self.history.push(self.cost);
        self.before = traj.clone();
        self.after = traj;
        let g = self.cost_gradient(&p)?;
        self.grad = g.iter().zip(&self.scale).map(|(a, s)| a * s).collect();
        self.started = true;
        Ok(())
    }

    fn iterate_lbfgs(&mut self) -> Result<(), String> {
        use crate::lbfgsb::{Outcome, iterate};
        let sc = self.scale.clone();
        let lo: Vec<f64> = self
            .tuners
            .iter()
            .zip(&sc)
            .map(|(t, s)| t.lo() / s)
            .collect();
        let hi: Vec<f64> = self
            .tuners
            .iter()
            .zip(&sc)
            .map(|(t, s)| t.hi() / s)
            .collect();
        let mut x: Vec<f64> = self.p.iter().zip(&sc).map(|(p, s)| p / s).collect();
        let mut f = self.cost;
        let mut g = std::mem::take(&mut self.grad);
        let mut mem = std::mem::take(&mut self.memory);
        let gtol = 1e-10 * (1.0 + f);
        let to_p = |x: &[f64]| -> Vec<f64> { x.iter().zip(&sc).map(|(a, s)| a * s).collect() };
        let mut last: Option<(Vec<f64>, Vec<Vec<f64>>)> = None;
        let out = {
            let this = std::cell::RefCell::new(&mut *self);
            iterate(
                &mut x,
                &mut f,
                &mut g,
                &lo,
                &hi,
                &mut mem,
                gtol,
                &mut |xp| {
                    let p = to_p(xp);
                    let mut me = this.borrow_mut();
                    match me.eval(&p, false) {
                        Ok((traj, _)) => {
                            let c = 0.5 * me.residuals(&traj).iter().map(|v| v * v).sum::<f64>();
                            last = Some((xp.to_vec(), traj));
                            c
                        }
                        Err(_) => f64::INFINITY,
                    }
                },
                &mut |xp| {
                    let p = to_p(xp);
                    let g = this.borrow_mut().cost_gradient(&p)?;
                    Ok(g.iter().zip(&sc).map(|(a, s)| a * s).collect())
                },
            )
        };
        self.memory = mem;
        self.grad = g;
        match out? {
            Outcome::Moved { decrease, step } => {
                self.p = to_p(&x);
                self.cost = f;
                if let Some((xl, traj)) = last
                    && xl == x
                {
                    self.r = self.residuals(&traj);
                    self.after = traj;
                }
                self.history.push(f);
                if decrease.abs() < 1e-10 || step < 1e-12 {
                    self.converged = true;
                    self.done = true;
                    self.message = "cost and step no longer decrease".into();
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
                self.message = "no step reduces the cost further (local minimum)".into();
            }
        }
        Ok(())
    }

    /// The residual Jacobian at the solution, for the standard errors.
    fn finish_lbfgs(&mut self) {
        let p = self.p.clone();
        if let Ok((traj, Some(jac))) = self.eval(&p, true) {
            self.r = self.residuals(&traj);
            self.jac = jac;
            self.cost = 0.5 * self.r.iter().map(|x| x * x).sum::<f64>();
            self.after = traj;
        }
    }

    pub fn result(&self) -> CalibResult {
        let np = self.p.len();
        let mrow = self.r.len();
        // Covariance s^2 (J^T J)^-1 with s^2 = |r|^2 / (m - np).
        let mut cov = vec![vec![f64::NAN; np]; np];
        if !self.jac.is_empty() && mrow > np {
            let mut h = vec![0.0; np * np];
            for row in &self.jac {
                for a in 0..np {
                    for b in 0..np {
                        h[a * np + b] += row[a] * row[b];
                    }
                }
            }
            let s2 = 2.0 * self.cost / (mrow - np) as f64;
            let mut piv = vec![0; np];
            if crate::linalg::lu_factor(&mut h, np, &mut piv) {
                for j in 0..np {
                    let mut e = vec![0.0; np];
                    e[j] = 1.0;
                    crate::linalg::lu_solve(&h, np, &piv, &mut e);
                    for i in 0..np {
                        cov[i][j] = s2 * e[i];
                    }
                }
            }
        }
        let corr = (0..np)
            .map(|i| {
                (0..np)
                    .map(|j| cov[i][j] / (cov[i][i] * cov[j][j]).sqrt())
                    .collect()
            })
            .collect();
        let params: Vec<FitParam> = self
            .tuners
            .iter()
            .enumerate()
            .map(|(i, t)| FitParam {
                name: t.name.clone(),
                initial: self.p0[i],
                value: self.p[i],
                lo: t.lo,
                hi: t.hi,
                std_error: if cov[i][i] >= 0.0 {
                    cov[i][i].sqrt()
                } else {
                    f64::NAN
                },
                at_bound: self.p[i] <= t.lo() || self.p[i] >= t.hi(),
            })
            .collect();
        let rms = |traj: &[Vec<f64>]| -> (f64, Vec<f64>) {
            if traj.is_empty() {
                return (f64::NAN, vec![f64::NAN; self.cols.len()]);
            }
            let r = self.residuals(traj);
            let all = (r.iter().map(|x| x * x).sum::<f64>() / r.len().max(1) as f64).sqrt();
            let per = self
                .cols
                .iter()
                .enumerate()
                .map(|(k, (_, _, ts, ys))| {
                    let s: f64 = ts
                        .iter()
                        .zip(ys)
                        .map(|(&t, &y)| (interp(&self.grid, &traj[k], t) - y).powi(2))
                        .sum();
                    (s / ts.len() as f64).sqrt()
                })
                .collect();
            (all, per)
        };
        let (rb, pb) = rms(&self.before);
        let (ra, pa) = rms(&self.after);
        let columns = self
            .cols
            .iter()
            .enumerate()
            .map(|(k, (d, w, ts, ys))| ColumnFit {
                column: d.column.clone(),
                var: d.var.clone(),
                weight: *w,
                rms_before: pb[k],
                rms_after: pa[k],
                t: ts.clone(),
                measured: ys.clone(),
                before: self.before.get(k).cloned().unwrap_or_default(),
                after: self.after.get(k).cloned().unwrap_or_default(),
            })
            .collect();
        let fitted: Vec<(String, f64)> = self.names.iter().cloned().zip(self.p.clone()).collect();
        CalibResult {
            model: self.m.name.clone(),
            method: self.used,
            params,
            correlation: corr,
            rms_before: rb,
            rms_after: ra,
            residuals: mrow,
            columns,
            time: self.grid.clone(),
            iterations: self.iter,
            history: self.history.clone(),
            converged: self.converged,
            message: self.message.clone(),
            modifier: modifier(&fitted),
            optimizer: self.optimizer,
            gradients: self.gradients,
            gradient_method: self.grad_used,
            notes: self.notes.clone(),
        }
    }

    pub fn report_of(r: &CalibResult) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "calibration of {}: {} iterations, {} residuals, sensitivities: {}",
            r.model,
            r.iterations,
            r.residuals,
            match r.method {
                SensMethod::Exact => "exact",
                SensMethod::Fd => "finite differences",
                SensMethod::Auto => "auto",
            }
        );
        if r.optimizer == CalibOptimizer::Lbfgsb {
            let _ = writeln!(
                s,
                "  optimizer: L-BFGS-B, {} gradient evaluations",
                r.gradients
            );
        }
        for n in &r.notes {
            let _ = writeln!(s, "  note: {n}");
        }
        let _ = writeln!(
            s,
            "  {}{}",
            if r.converged {
                "converged: "
            } else {
                "stopped: "
            },
            r.message
        );
        let _ = writeln!(
            s,
            "  weighted residual RMS: {:.6e} -> {:.6e}",
            r.rms_before, r.rms_after
        );
        let _ = writeln!(
            s,
            "\n  {:<24} {:>14} {:>14} {:>12}",
            "parameter", "initial", "fitted", "std. error"
        );
        for p in &r.params {
            let _ = writeln!(
                s,
                "  {:<24} {:>14.7e} {:>14.7e} {:>12.3e}{}",
                p.name,
                p.initial,
                p.value,
                p.std_error,
                if p.at_bound { "  (at bound)" } else { "" }
            );
        }
        if r.params.len() > 1 {
            let _ = writeln!(s, "\n  correlation:");
            for row in &r.correlation {
                let cells: Vec<String> = row.iter().map(|v| format!("{v:>7.3}")).collect();
                let _ = writeln!(s, "    [{}]", cells.join(" "));
            }
        }
        let _ = writeln!(
            s,
            "\n  {:<24} {:>12} {:>14} {:>14}",
            "column", "weight", "RMS before", "RMS after"
        );
        for c in &r.columns {
            let label = if c.column == c.var {
                c.column.clone()
            } else {
                format!("{} -> {}", c.column, c.var)
            };
            let _ = writeln!(
                s,
                "  {:<24} {:>12.4e} {:>14.6e} {:>14.6e}",
                label, c.weight, c.rms_before, c.rms_after
            );
        }
        let _ = writeln!(s, "\nmodifier: {}", r.modifier);
        s
    }
}

impl Job for Calibration {
    fn step(&mut self) -> Result<bool, String> {
        if self.done {
            return Ok(true);
        }
        let lbfgs = self.optimizer == CalibOptimizer::Lbfgsb;
        if !self.started {
            if lbfgs {
                self.start_lbfgs()?;
            } else {
                self.start()?;
            }
            return Ok(false);
        }
        if lbfgs {
            self.iterate_lbfgs()?;
        } else {
            self.iterate()?;
        }
        self.iter += 1;
        if !self.done && self.iter >= self.max_iter {
            self.done = true;
            self.message = format!("iteration limit ({}) reached", self.max_iter);
        }
        if self.done && lbfgs {
            self.finish_lbfgs();
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
        Calibration::report_of(&self.result())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_nesting() {
        let m = modifier(&[
            ("k".into(), 2.0),
            ("body.m".into(), 1.5),
            ("body.J".into(), 0.125),
            ("a[1].b".into(), -3e-9),
        ]);
        assert_eq!(m, "(k = 2, body(m = 1.5, J = 0.125), a[1](b = -3e-9))");
    }

    #[test]
    fn csv_and_tuners() {
        let t = parse_csv("time;\"x\";y\n0;1;\n1;2;3\n").unwrap();
        assert_eq!(t.header, ["time", "x", "y"]);
        assert!(t.cols[2][0].is_nan());
        let tu = Tuner::parse("k=0.1:10").unwrap();
        assert_eq!((tu.lo, tu.hi, tu.start), (Some(0.1), Some(10.0), None));
        let tu = Tuner::parse("k=:inf:3").unwrap();
        assert_eq!((tu.lo, tu.hi, tu.start), (None, None, Some(3.0)));
    }
}
