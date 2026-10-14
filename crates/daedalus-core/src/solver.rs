//! Time integrators for the state-space form `dx/dt = f(t, x)` produced by the
//! symbolic backend.
//!
//! * [`Bdf`]: variable-order (1-5) variable-step BDF/NDF with a colored
//!   finite-difference Jacobian, the stiff default (the role DASSL plays in Dymola).
//! * [`Dopri5`]: explicit Dormand–Prince 5(4) with 4th-order dense output.
//! * [`FixedStep`]: explicit Euler and classic RK4 for real-time use.
//!
//! All integrators expose dense output so the simulation driver can place
//! output points and locate events without restricting the step size.

use crate::codegen::Runtime;
use crate::linalg::{lu_factor, lu_solve};
use crate::sparse::{Csc, SparseLu, min_degree};

/// Systems larger than this use the sparse LU.
const DENSE_LIMIT: usize = 48;

#[derive(Clone, Copy, Debug, Default, serde::Serialize)]
pub struct SolverStats {
    pub steps: u64,
    pub rejected: u64,
    /// Rejections caused by Newton non-convergence (the rest are error-test failures).
    pub newton_failures: u64,
    pub rhs_evals: u64,
    pub jac_evals: u64,
    pub lu_factorizations: u64,
}

pub trait Integrator {
    fn reset(&mut self, rt: &mut Runtime, t: f64, x: &[f64]);
    /// Take one step, never beyond `t_max`. Returns the new time.
    fn step(&mut self, rt: &mut Runtime, t_max: f64) -> Result<f64, String>;
    fn t(&self) -> f64;
    fn t_prev(&self) -> f64;
    fn x(&self) -> &[f64];
    /// Dense output on the last step `[t_prev, t]`.
    fn interpolate(&self, t: f64, out: &mut [f64]);
    fn stats(&self) -> SolverStats;
}

fn rms(v: impl Iterator<Item = f64>, n: usize) -> f64 {
    if n == 0 {
        return 0.0;
    }
    (v.map(|x| x * x).sum::<f64>() / n as f64).sqrt()
}

/// Hairer's initial step size heuristic.
fn initial_step(rt: &mut Runtime, t: f64, x: &[f64], f0: &[f64], order: f64, rtol: f64, atol: f64, span: f64) -> f64 {
    let n = x.len();
    if n == 0 {
        return span.max(1e-6);
    }
    let scale: Vec<f64> = x.iter().map(|v| atol + rtol * v.abs()).collect();
    let d0 = rms(x.iter().zip(&scale).map(|(a, s)| a / s), n);
    let d1 = rms(f0.iter().zip(&scale).map(|(a, s)| a / s), n);
    let h0 = if d0 < 1e-5 || d1 < 1e-5 { 1e-6 } else { 0.01 * d0 / d1 };
    let h0 = h0.min(span);
    let x1: Vec<f64> = x.iter().zip(f0).map(|(a, f)| a + h0 * f).collect();
    let mut f1 = vec![0.0; n];
    rt.rhs(t + h0, &x1, &mut f1);
    let d2 = rms(f1.iter().zip(f0).zip(&scale).map(|((a, b), s)| (a - b) / s), n) / h0;
    let h1 = if d1 <= 1e-15 && d2 <= 1e-15 {
        (h0 * 1e-3).max(1e-6)
    } else {
        (0.01 / d1.max(d2)).powf(1.0 / (order + 1.0))
    };
    (100.0 * h0).min(h1).min(span)
}

// ----------------------------------------------------------------------------
// Dormand–Prince 5(4)
// ----------------------------------------------------------------------------

pub struct Dopri5 {
    pub rtol: f64,
    pub atol: f64,
    pub max_step: f64,
    t: f64,
    t_old: f64,
    h: f64,
    x: Vec<f64>,
    k: [Vec<f64>; 7],
    cont: [Vec<f64>; 5],
    tmp: Vec<f64>,
    stats: SolverStats,
}

impl Dopri5 {
    pub fn new(n: usize, rtol: f64, atol: f64, max_step: f64) -> Self {
        let z = || vec![0.0; n];
        Dopri5 {
            rtol,
            atol,
            max_step,
            t: 0.0,
            t_old: 0.0,
            h: 0.0,
            x: z(),
            k: [z(), z(), z(), z(), z(), z(), z()],
            cont: [z(), z(), z(), z(), z()],
            tmp: z(),
            stats: SolverStats::default(),
        }
    }
}

impl Integrator for Dopri5 {
    fn reset(&mut self, rt: &mut Runtime, t: f64, x: &[f64]) {
        self.t = t;
        self.t_old = t;
        self.x.copy_from_slice(x);
        rt.rhs(t, x, &mut self.k[0]);
        self.stats.rhs_evals += 1;
        self.h = 0.0;
        for c in self.cont.iter_mut() {
            c.fill(0.0);
        }
        self.cont[0].copy_from_slice(x);
    }

    fn step(&mut self, rt: &mut Runtime, t_max: f64) -> Result<f64, String> {
        const C: [f64; 7] = [0.0, 0.2, 0.3, 0.8, 8.0 / 9.0, 1.0, 1.0];
        const A: [[f64; 6]; 7] = [
            [0.0; 6],
            [0.2, 0.0, 0.0, 0.0, 0.0, 0.0],
            [3.0 / 40.0, 9.0 / 40.0, 0.0, 0.0, 0.0, 0.0],
            [44.0 / 45.0, -56.0 / 15.0, 32.0 / 9.0, 0.0, 0.0, 0.0],
            [19372.0 / 6561.0, -25360.0 / 2187.0, 64448.0 / 6561.0, -212.0 / 729.0, 0.0, 0.0],
            [9017.0 / 3168.0, -355.0 / 33.0, 46732.0 / 5247.0, 49.0 / 176.0, -5103.0 / 18656.0, 0.0],
            [35.0 / 384.0, 0.0, 500.0 / 1113.0, 125.0 / 192.0, -2187.0 / 6784.0, 11.0 / 84.0],
        ];
        const E: [f64; 7] =
            [71.0 / 57600.0, 0.0, -71.0 / 16695.0, 71.0 / 1920.0, -17253.0 / 339200.0, 22.0 / 525.0, -1.0 / 40.0];
        const D: [f64; 7] = [
            -12715105075.0 / 11282082432.0,
            0.0,
            87487479700.0 / 32700410799.0,
            -10690763975.0 / 1880347072.0,
            701980252875.0 / 199316789632.0,
            -1453857185.0 / 822651844.0,
            69997945.0 / 29380423.0,
        ];
        let n = self.x.len();
        let span = t_max - self.t;
        if span <= 0.0 {
            return Ok(self.t);
        }
        if n == 0 {
            self.t_old = self.t;
            self.t = t_max;
            return Ok(self.t);
        }
        if self.h <= 0.0 {
            let f0 = self.k[0].clone();
            self.h = initial_step(rt, self.t, &self.x, &f0, 4.0, self.rtol, self.atol, span.min(self.max_step));
        }
        let min_h = 1e-14 * self.t.abs().max(1.0);
        loop {
            let mut h = self.h.min(self.max_step);
            let last = h >= span * (1.0 - 1e-12);
            if last {
                h = span;
            }
            for s in 1..7 {
                for i in 0..n {
                    let mut acc = self.x[i];
                    for j in 0..s {
                        let a = A[s][j];
                        if a != 0.0 {
                            acc += h * a * self.k[j][i];
                        }
                    }
                    self.tmp[i] = acc;
                }
                let (_, rest) = self.k.split_at_mut(s);
                rt.rhs(self.t + C[s] * h, &self.tmp, &mut rest[0]);
                self.stats.rhs_evals += 1;
            }
            // tmp now holds the 5th-order solution (stage 7 is FSAL).
            let mut err = 0.0;
            for i in 0..n {
                let mut e = 0.0;
                for j in 0..7 {
                    if E[j] != 0.0 {
                        e += E[j] * self.k[j][i];
                    }
                }
                e *= h;
                let sk = self.atol + self.rtol * self.x[i].abs().max(self.tmp[i].abs());
                err += (e / sk) * (e / sk);
            }
            err = (err / n as f64).sqrt();
            if !err.is_finite() {
                self.h = h * 0.25;
                self.stats.rejected += 1;
                if self.h < min_h {
                    return Err(format!("step size too small at t = {}", self.t));
                }
                continue;
            }
            if err <= 1.0 {
                // Dense output coefficients.
                for i in 0..n {
                    let y0 = self.x[i];
                    let y1 = self.tmp[i];
                    let ydiff = y1 - y0;
                    let bspl = h * self.k[0][i] - ydiff;
                    self.cont[0][i] = y0;
                    self.cont[1][i] = ydiff;
                    self.cont[2][i] = bspl;
                    self.cont[3][i] = ydiff - h * self.k[6][i] - bspl;
                    let mut d = 0.0;
                    for j in 0..7 {
                        if D[j] != 0.0 {
                            d += D[j] * self.k[j][i];
                        }
                    }
                    self.cont[4][i] = h * d;
                }
                self.t_old = self.t;
                self.t = if last { t_max } else { self.t + h };
                self.x.copy_from_slice(&self.tmp);
                let (first, rest) = self.k.split_at_mut(1);
                first[0].copy_from_slice(&rest[5]);
                self.stats.steps += 1;
                let fac = (0.9 * err.max(1e-10).powf(-0.2)).clamp(0.2, 10.0);
                self.h = h * fac;
                return Ok(self.t);
            }
            self.stats.rejected += 1;
            let fac = (0.9 * err.powf(-0.2)).clamp(0.2, 1.0);
            self.h = h * fac;
            if self.h < min_h {
                return Err(format!("step size too small at t = {}", self.t));
            }
        }
    }

    fn t(&self) -> f64 {
        self.t
    }
    fn t_prev(&self) -> f64 {
        self.t_old
    }
    fn x(&self) -> &[f64] {
        &self.x
    }
    fn interpolate(&self, t: f64, out: &mut [f64]) {
        let h = self.t - self.t_old;
        if h <= 0.0 {
            out.copy_from_slice(&self.x);
            return;
        }
        let th = (t - self.t_old) / h;
        let th1 = 1.0 - th;
        for i in 0..out.len() {
            let c = &self.cont;
            out[i] = c[0][i] + th * (c[1][i] + th1 * (c[2][i] + th * (c[3][i] + th1 * c[4][i])));
        }
    }
    fn stats(&self) -> SolverStats {
        self.stats
    }
}

// ----------------------------------------------------------------------------
// BDF / NDF (variable order, quasi-constant step size, after Shampine & Reichelt)
// ----------------------------------------------------------------------------

const MAX_ORDER: usize = 5;
const NEWTON_MAXITER: usize = 4;

pub struct Bdf {
    pub rtol: f64,
    pub atol: f64,
    pub max_step: f64,
    n: usize,
    t: f64,
    t_old: f64,
    h_abs: f64,
    h_dense: f64,
    order: usize,
    dense_order: usize,
    n_equal_steps: usize,
    d: Vec<Vec<f64>>,
    d_dense: Vec<Vec<f64>>,
    jac: Vec<f64>,
    lu: Vec<f64>,
    piv: Vec<usize>,
    lu_valid: bool,
    jac_current: bool,
    jac_needed: bool,
    lu_c: f64,
    gamma: [f64; MAX_ORDER + 1],
    alpha: [f64; MAX_ORDER + 1],
    error_const: [f64; MAX_ORDER + 2],
    colors: Vec<Vec<usize>>,
    rows_of_col: Vec<Vec<usize>>,
    /// Sparse iteration matrix `I - c J` (structure = pattern + diagonal).
    sp: Option<(Csc, Vec<f64>, Vec<bool>, SparseLu)>,
    x: Vec<f64>,
    stats: SolverStats,
}

impl Bdf {
    /// `pattern[i]` lists the states derivative `i` depends on.
    pub fn new(n: usize, rtol: f64, atol: f64, max_step: f64, pattern: &[Vec<u32>]) -> Self {
        let kappa = [0.0, -0.1850, -1.0 / 9.0, -0.0823, -0.0415, 0.0];
        let mut gamma = [0.0; MAX_ORDER + 1];
        for k in 1..=MAX_ORDER {
            gamma[k] = gamma[k - 1] + 1.0 / k as f64;
        }
        let mut alpha = [0.0; MAX_ORDER + 1];
        for k in 0..=MAX_ORDER {
            alpha[k] = (1.0 - kappa[k]) * gamma[k];
        }
        let mut error_const = [0.0; MAX_ORDER + 2];
        for k in 0..=MAX_ORDER {
            error_const[k] = kappa[k] * gamma[k] + 1.0 / (k as f64 + 1.0);
        }
        error_const[MAX_ORDER + 1] = 1.0 / (MAX_ORDER as f64 + 2.0);
        // Column coloring for the finite-difference Jacobian.
        let mut rows_of_col: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, deps) in pattern.iter().enumerate() {
            for &j in deps {
                rows_of_col[j as usize].push(i);
            }
        }
        let mut color_of = vec![usize::MAX; n];
        let mut colors: Vec<Vec<usize>> = Vec::new();
        for j in 0..n {
            // Find the first color not used by any column sharing a row with j.
            let mut forbidden: Vec<usize> = Vec::new();
            for &r in &rows_of_col[j] {
                for &k in &pattern[r] {
                    let c = color_of[k as usize];
                    if c != usize::MAX {
                        forbidden.push(c);
                    }
                }
            }
            forbidden.sort_unstable();
            forbidden.dedup();
            let mut c = 0;
            for &f in &forbidden {
                if f == c {
                    c += 1;
                } else if f > c {
                    break;
                }
            }
            color_of[j] = c;
            if c == colors.len() {
                colors.push(Vec::new());
            }
            colors[c].push(j);
        }
        let sp = if n > DENSE_LIMIT {
            let cols: Vec<Vec<usize>> = (0..n)
                .map(|j| {
                    let mut c = rows_of_col[j].clone();
                    c.push(j);
                    c.sort_unstable();
                    c.dedup();
                    c
                })
                .collect();
            let m = Csc::from_columns(n, &cols);
            let in_pat: Vec<bool> = (0..n)
                .flat_map(|j| {
                    let r = &rows_of_col[j];
                    cols[j].iter().map(move |i| r.contains(i)).collect::<Vec<_>>()
                })
                .collect();
            let q = min_degree(&m);
            let nnz = m.nnz();
            Some((m, vec![0.0; nnz], in_pat, SparseLu::new(q)))
        } else {
            None
        };
        let dense_n = if sp.is_some() { 0 } else { n };
        Bdf {
            rtol,
            atol,
            max_step,
            n,
            sp,
            t: 0.0,
            t_old: 0.0,
            h_abs: 0.0,
            h_dense: 0.0,
            order: 1,
            dense_order: 1,
            n_equal_steps: 0,
            d: vec![vec![0.0; n]; MAX_ORDER + 3],
            d_dense: vec![vec![0.0; n]; MAX_ORDER + 1],
            jac: vec![0.0; dense_n * dense_n],
            lu: vec![0.0; dense_n * dense_n],
            piv: vec![0; n],
            lu_valid: false,
            jac_current: false,
            jac_needed: true,
            lu_c: 0.0,
            gamma,
            alpha,
            error_const,
            colors,
            rows_of_col,
            x: vec![0.0; n],
            stats: SolverStats::default(),
        }
    }

    pub fn n_colors(&self) -> usize {
        self.colors.len()
    }

    fn compute_jac(&mut self, rt: &mut Runtime, t: f64, y: &[f64]) {
        let n = self.n;
        let mut f0 = vec![0.0; n];
        rt.rhs(t, y, &mut f0);
        let mut yp = y.to_vec();
        let mut f1 = vec![0.0; n];
        self.jac.fill(0.0);
        let dense = self.sp.is_none();
        let eps = f64::EPSILON.sqrt();
        for group in &self.colors {
            let mut hs = Vec::with_capacity(group.len());
            for &j in group {
                let h = eps * y[j].abs().max(self.atol / self.rtol.max(1e-12)).max(1e-8);
                yp[j] = y[j] + h;
                hs.push(h);
            }
            rt.rhs(t, &yp, &mut f1);
            self.stats.rhs_evals += 1;
            for (&j, &h) in group.iter().zip(&hs) {
                if dense {
                    for &i in &self.rows_of_col[j] {
                        self.jac[i * n + j] = (f1[i] - f0[i]) / h;
                    }
                } else if let Some((m, jv, in_pat, _)) = &mut self.sp {
                    for p in m.colptr[j]..m.colptr[j + 1] {
                        let i = m.rowidx[p];
                        jv[p] = if in_pat[p] { (f1[i] - f0[i]) / h } else { 0.0 };
                    }
                }
                yp[j] = y[j];
            }
        }
        self.stats.rhs_evals += 1;
        self.stats.jac_evals += 1;
    }

    fn factor(&mut self, c: f64) -> bool {
        let n = self.n;
        if let Some((m, jv, _, lu)) = &mut self.sp {
            for j in 0..n {
                for p in m.colptr[j]..m.colptr[j + 1] {
                    let i = m.rowidx[p];
                    m.vals[p] = if i == j { 1.0 } else { 0.0 } - c * jv[p];
                }
            }
            self.stats.lu_factorizations += 1;
            self.lu_c = c;
            self.lu_valid = lu.factor(m, 0.1);
            return self.lu_valid;
        }
        for i in 0..n {
            for j in 0..n {
                self.lu[i * n + j] = -c * self.jac[i * n + j];
            }
            self.lu[i * n + i] += 1.0;
        }
        self.stats.lu_factorizations += 1;
        self.lu_c = c;
        self.lu_valid = lu_factor(&mut self.lu, n, &mut self.piv);
        self.lu_valid
    }

    fn change_d(&mut self, factor: f64) {
        let order = self.order;
        let r = compute_r(order, factor);
        let u = compute_r(order, 1.0);
        // RU = R * U ; D[:order+1] = RU^T D[:order+1]
        let m = order + 1;
        let mut ru = vec![0.0; m * m];
        for i in 0..m {
            for j in 0..m {
                let mut s = 0.0;
                for k in 0..m {
                    s += r[i * m + k] * u[k * m + j];
                }
                ru[i * m + j] = s;
            }
        }
        let n = self.n;
        let mut newd = vec![vec![0.0; n]; m];
        for (i, row) in newd.iter_mut().enumerate() {
            for k in 0..m {
                let c = ru[k * m + i];
                if c != 0.0 {
                    for (x, y) in row.iter_mut().zip(&self.d[k]) {
                        *x += c * y;
                    }
                }
            }
        }
        for (i, row) in newd.into_iter().enumerate() {
            self.d[i] = row;
        }
    }
}

fn compute_r(order: usize, factor: f64) -> Vec<f64> {
    let m = order + 1;
    let mut mm = vec![0.0; m * m];
    for j in 0..m {
        mm[j] = 1.0;
    }
    for i in 1..m {
        for j in 1..m {
            mm[i * m + j] = (i as f64 - 1.0 - factor * j as f64) / i as f64;
        }
    }
    // cumulative product down the columns
    for i in 1..m {
        for j in 0..m {
            mm[i * m + j] *= mm[(i - 1) * m + j];
        }
    }
    mm
}

impl Integrator for Bdf {
    fn reset(&mut self, rt: &mut Runtime, t: f64, x: &[f64]) {
        self.t = t;
        self.t_old = t;
        self.x.copy_from_slice(x);
        self.order = 1;
        self.n_equal_steps = 0;
        self.h_abs = 0.0;
        self.lu_valid = false;
        self.jac_current = false;
        for row in self.d.iter_mut() {
            row.fill(0.0);
        }
        self.d[0].copy_from_slice(x);
        let mut f = vec![0.0; self.n];
        rt.rhs(t, x, &mut f);
        self.stats.rhs_evals += 1;
        self.d[1].copy_from_slice(&f);
        self.dense_order = 0;
        self.d_dense[0].copy_from_slice(x);
        self.jac.fill(0.0);
        self.jac_needed = true;
    }

    fn step(&mut self, rt: &mut Runtime, t_max: f64) -> Result<f64, String> {
        let n = self.n;
        let span = t_max - self.t;
        if span <= 0.0 {
            return Ok(self.t);
        }
        if n == 0 {
            self.t_old = self.t;
            self.t = t_max;
            self.dense_order = 0;
            return Ok(self.t);
        }
        if self.h_abs == 0.0 {
            let f0 = self.d[1].clone();
            let h = initial_step(rt, self.t, &self.x, &f0, 1.0, self.rtol, self.atol, span.min(self.max_step));
            self.h_abs = h;
            for v in self.d[1].iter_mut() {
                *v *= h;
            }
        }
        if self.jac_needed {
            let x = self.x.clone();
            self.compute_jac(rt, self.t, &x);
            self.jac_needed = false;
            self.jac_current = true;
            self.lu_valid = false;
        }
        let min_step = 10.0 * (self.t.abs() * f64::EPSILON).max(f64::MIN_POSITIVE);
        if self.h_abs > self.max_step {
            let f = self.max_step / self.h_abs;
            self.change_d(f);
            self.h_abs = self.max_step;
            self.n_equal_steps = 0;
            self.lu_valid = false;
        }
        let mut y_new = vec![0.0; n];
        let mut dsum = vec![0.0; n];
        let mut scale = vec![0.0; n];
        let mut psi = vec![0.0; n];
        let mut y_pred = vec![0.0; n];
        let mut f = vec![0.0; n];
        let mut dy = vec![0.0; n];
        let safety;
        let error_norm;
        loop {
            if self.h_abs < min_step {
                return Err(format!("step size too small at t = {}", self.t));
            }
            let mut t_new = self.t + self.h_abs;
            if t_new >= t_max * (1.0 - 1e-15) - 1e-300 || t_new > t_max {
                t_new = t_max;
                let f = (t_new - self.t) / self.h_abs;
                if (f - 1.0).abs() > 1e-15 {
                    self.change_d(f);
                    self.lu_valid = false;
                }
                self.n_equal_steps = 0;
            }
            let h = t_new - self.t;
            self.h_abs = h;
            let order = self.order;
            for i in 0..n {
                let mut s = 0.0;
                for k in 0..=order {
                    s += self.d[k][i];
                }
                y_pred[i] = s;
                scale[i] = self.atol + self.rtol * s.abs();
                let mut p = 0.0;
                for k in 1..=order {
                    p += self.d[k][i] * self.gamma[k];
                }
                psi[i] = p / self.alpha[order];
            }
            let c = h / self.alpha[order];
            let mut converged = false;
            let mut n_iter = 0;
            loop {
                if !self.lu_valid || self.lu_c != c {
                    if !self.factor(c) {
                        break;
                    }
                }
                // Newton iteration
                y_new.copy_from_slice(&y_pred);
                dsum.fill(0.0);
                let mut dy_norm_old: Option<f64> = None;
                converged = false;
                for k in 0..NEWTON_MAXITER {
                    n_iter = k + 1;
                    rt.rhs(t_new, &y_new, &mut f);
                    self.stats.rhs_evals += 1;
                    if !f.iter().all(|v| v.is_finite()) || rt.error.is_some() {
                        rt.error = None;
                        break;
                    }
                    for i in 0..n {
                        dy[i] = c * f[i] - psi[i] - dsum[i];
                    }
                    match &mut self.sp {
                        Some((_, _, _, lu)) => lu.solve(&mut dy),
                        None => lu_solve(&self.lu, n, &self.piv, &mut dy),
                    }
                    let dy_norm = rms(dy.iter().zip(&scale).map(|(a, s)| a / s), n);
                    let rate = dy_norm_old.map(|o| dy_norm / o);
                    if let Some(r) = rate
                        && (r >= 1.0 || r.powi((NEWTON_MAXITER - k) as i32) / (1.0 - r) * dy_norm > self.newton_tol())
                    {
                        break;
                    }
                    for i in 0..n {
                        y_new[i] += dy[i];
                        dsum[i] += dy[i];
                    }
                    if dy_norm == 0.0 || rate.is_some_and(|r| r / (1.0 - r) * dy_norm < self.newton_tol()) {
                        converged = true;
                        break;
                    }
                    dy_norm_old = Some(dy_norm);
                }
                if converged || self.jac_current {
                    break;
                }
                self.compute_jac(rt, t_new, &y_pred);
                self.jac_current = true;
                self.lu_valid = false;
            }
            if !converged {
                self.stats.rejected += 1;
                self.stats.newton_failures += 1;
                self.h_abs *= 0.5;
                self.change_d(0.5);
                self.n_equal_steps = 0;
                self.lu_valid = false;
                continue;
            }
            let s = 0.9 * (2.0 * NEWTON_MAXITER as f64 + 1.0) / (2.0 * NEWTON_MAXITER as f64 + n_iter as f64);
            for i in 0..n {
                scale[i] = self.atol + self.rtol * y_new[i].abs();
            }
            let ec = self.error_const[order];
            let en = rms(dsum.iter().zip(&scale).map(|(d, s)| ec * d / s), n);
            if en > 1.0 {
                self.stats.rejected += 1;
                let factor = (s * en.powf(-1.0 / (order as f64 + 1.0))).max(0.2);
                self.h_abs *= factor;
                self.change_d(factor);
                self.n_equal_steps = 0;
                continue;
            }
            safety = s;
            error_norm = en;
            self.t_old = self.t;
            self.t = t_new;
            break;
        }
        self.stats.steps += 1;
        self.n_equal_steps += 1;
        self.jac_current = false;
        let order = self.order;
        // Update differences.
        for i in 0..n {
            self.d[order + 2][i] = dsum[i] - self.d[order + 1][i];
            self.d[order + 1][i] = dsum[i];
        }
        for k in (0..=order).rev() {
            let (a, b) = self.d.split_at_mut(k + 1);
            for (x, y) in a[k].iter_mut().zip(&b[0]) {
                *x += y;
            }
        }
        self.x.copy_from_slice(&y_new);
        // Dense output uses the differences of this step.
        self.dense_order = order;
        self.h_dense = self.t - self.t_old;
        for k in 0..=order {
            self.d_dense[k].copy_from_slice(&self.d[k]);
        }
        if self.n_equal_steps < order + 1 {
            return Ok(self.t);
        }
        let em = if order > 1 {
            let ec = self.error_const[order - 1];
            rms(self.d[order].iter().zip(&scale).map(|(d, s)| ec * d / s), n)
        } else {
            f64::INFINITY
        };
        let ep = if order < MAX_ORDER {
            let ec = self.error_const[order + 1];
            rms(self.d[order + 2].iter().zip(&scale).map(|(d, s)| ec * d / s), n)
        } else {
            f64::INFINITY
        };
        let norms = [em, error_norm, ep];
        let mut best = 0;
        let mut factors = [0.0; 3];
        for (k, &e) in norms.iter().enumerate() {
            let p = (order + k) as f64; // order-1+k+1
            factors[k] = if e == 0.0 { f64::INFINITY } else { e.powf(-1.0 / p) };
            if factors[k] > factors[best] {
                best = k;
            }
        }
        self.order = (order as isize + best as isize - 1) as usize;
        let factor = (safety * factors[best]).min(10.0);
        self.h_abs *= factor;
        self.change_d(factor);
        self.n_equal_steps = 0;
        self.lu_valid = false;
        Ok(self.t)
    }

    fn t(&self) -> f64 {
        self.t
    }
    fn t_prev(&self) -> f64 {
        self.t_old
    }
    fn x(&self) -> &[f64] {
        &self.x
    }
    fn interpolate(&self, t: f64, out: &mut [f64]) {
        let order = self.dense_order;
        if order == 0 || self.h_dense <= 0.0 {
            out.copy_from_slice(&self.x);
            return;
        }
        // y(t) = D0 + sum_k D_k prod_{j<k} (t - (t_n - j h)) / ((j+1) h)
        let h = self.h_dense;
        out.copy_from_slice(&self.d_dense[0]);
        let mut p = 1.0;
        for k in 1..=order {
            let shift = self.t - h * (k as f64 - 1.0);
            p *= (t - shift) / (h * k as f64);
            for (o, d) in out.iter_mut().zip(&self.d_dense[k]) {
                *o += d * p;
            }
        }
    }
    fn stats(&self) -> SolverStats {
        self.stats
    }
}

impl Bdf {
    fn newton_tol(&self) -> f64 {
        (10.0 * f64::EPSILON / self.rtol).max(0.03f64.min(self.rtol.sqrt()))
    }
}

// ----------------------------------------------------------------------------
// Fixed step methods
// ----------------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixedMethod {
    Euler,
    Rk4,
}

pub struct FixedStep {
    pub method: FixedMethod,
    pub h: f64,
    t: f64,
    t_old: f64,
    x: Vec<f64>,
    x_old: Vec<f64>,
    f_old: Vec<f64>,
    f: Vec<f64>,
    k: [Vec<f64>; 4],
    tmp: Vec<f64>,
    stats: SolverStats,
}

impl FixedStep {
    pub fn new(n: usize, method: FixedMethod, h: f64) -> Self {
        let z = || vec![0.0; n];
        FixedStep {
            method,
            h,
            t: 0.0,
            t_old: 0.0,
            x: z(),
            x_old: z(),
            f_old: z(),
            f: z(),
            k: [z(), z(), z(), z()],
            tmp: z(),
            stats: SolverStats::default(),
        }
    }
}

impl Integrator for FixedStep {
    fn reset(&mut self, rt: &mut Runtime, t: f64, x: &[f64]) {
        self.t = t;
        self.t_old = t;
        self.x.copy_from_slice(x);
        self.x_old.copy_from_slice(x);
        rt.rhs(t, x, &mut self.f);
        self.f_old.copy_from_slice(&self.f);
        self.stats.rhs_evals += 1;
    }

    fn step(&mut self, rt: &mut Runtime, t_max: f64) -> Result<f64, String> {
        let n = self.x.len();
        let mut h = self.h;
        if self.t + h > t_max - 1e-12 * h {
            h = t_max - self.t;
        }
        if h <= 0.0 {
            return Ok(self.t);
        }
        self.x_old.copy_from_slice(&self.x);
        self.f_old.copy_from_slice(&self.f);
        match self.method {
            FixedMethod::Euler => {
                for i in 0..n {
                    self.x[i] += h * self.f[i];
                }
            }
            FixedMethod::Rk4 => {
                self.k[0].copy_from_slice(&self.f);
                for s in 1..4 {
                    let c = if s == 3 { 1.0 } else { 0.5 };
                    for i in 0..n {
                        self.tmp[i] = self.x[i] + c * h * self.k[s - 1][i];
                    }
                    let (_, rest) = self.k.split_at_mut(s);
                    rt.rhs(self.t + c * h, &self.tmp, &mut rest[0]);
                    self.stats.rhs_evals += 1;
                }
                for i in 0..n {
                    self.x[i] += h / 6.0 * (self.k[0][i] + 2.0 * self.k[1][i] + 2.0 * self.k[2][i] + self.k[3][i]);
                }
            }
        }
        self.t_old = self.t;
        self.t += h;
        if (t_max - self.t).abs() < 1e-12 * self.h {
            self.t = t_max;
        }
        rt.rhs(self.t, &self.x, &mut self.f);
        self.stats.rhs_evals += 1;
        self.stats.steps += 1;
        Ok(self.t)
    }

    fn t(&self) -> f64 {
        self.t
    }
    fn t_prev(&self) -> f64 {
        self.t_old
    }
    fn x(&self) -> &[f64] {
        &self.x
    }
    fn interpolate(&self, t: f64, out: &mut [f64]) {
        // Cubic Hermite interpolation between the step end points.
        let h = self.t - self.t_old;
        if h <= 0.0 {
            out.copy_from_slice(&self.x);
            return;
        }
        let s = (t - self.t_old) / h;
        let h00 = (1.0 + 2.0 * s) * (1.0 - s) * (1.0 - s);
        let h10 = s * (1.0 - s) * (1.0 - s);
        let h01 = s * s * (3.0 - 2.0 * s);
        let h11 = s * s * (s - 1.0);
        for i in 0..out.len() {
            out[i] = h00 * self.x_old[i] + h10 * h * self.f_old[i] + h01 * self.x[i] + h11 * h * self.f[i];
        }
    }
    fn stats(&self) -> SolverStats {
        self.stats
    }
}
