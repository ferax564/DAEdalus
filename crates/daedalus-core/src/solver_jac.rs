//! The state Jacobian `J = df/dx` of `dx/dt = f(t, x)` for the implicit integrators,
//! and the linear systems they solve with it.
//!
//! Columns that share no row are grouped by a greedy coloring of the dependency
//! pattern, so one pass evaluates a whole group. A pass is either
//! * a forward-mode (tangent) run of the compiled tape seeded with the group's states
//!   ([`JacobianMode::Exact`], the default): exact up to rounding, algebraic loops
//!   differentiated implicitly (see [`Runtime::run_tangent`]); or
//! * a forward difference of the right-hand side ([`JacobianMode::Fd`]).
//!
//! Either way it costs one pass per color. Exact columns that come out non-finite
//! fall back to differences for that evaluation.
//!
//! [`RealSystem`] factors `a I - J` and [`ComplexSystem`] factors `(a + i b) I - J`
//! (as the real `2n x 2n` system with interleaved real and imaginary parts), dense up
//! to [`DENSE_LIMIT`] states and sparse beyond.

#![allow(clippy::needless_range_loop)]

use crate::codegen::Runtime;
use crate::linalg::{lu_factor, lu_solve};
use crate::solver::SolverStats;
use crate::sparse::{Csc, SparseLu, min_degree};
use serde::{Deserialize, Serialize};

/// Systems with more states than this use the sparse LU.
pub const DENSE_LIMIT: usize = 48;

/// How the implicit solvers compute the state Jacobian.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum JacobianMode {
    /// Forward-mode differentiation of the tape, one pass per color group.
    #[default]
    Exact,
    /// Forward differences of the right-hand side, one evaluation per color group.
    Fd,
}

impl JacobianMode {
    pub fn parse(s: &str) -> Option<JacobianMode> {
        Some(match s.to_ascii_lowercase().as_str() {
            "exact" | "ad" | "analytic" => JacobianMode::Exact,
            "fd" | "cd" | "central" | "numeric" => JacobianMode::Fd,
            _ => return None,
        })
    }
}

/// Coloring and storage of the state Jacobian.
pub struct StateJac {
    pub n: usize,
    pub mode: JacobianMode,
    pub colors: Vec<Vec<usize>>,
    pub rows_of_col: Vec<Vec<usize>>,
    /// Dense values, row-major (empty for sparse systems).
    pub dense: Vec<f64>,
    /// Sparse structure: the pattern plus the diagonal, sorted per column.
    pub csc: Option<Csc>,
    /// Sparse values aligned with `csc` (zero on structural diagonal entries).
    pub vals: Vec<f64>,
    in_pat: Vec<bool>,
    /// Evaluations that fell back from exact to differences.
    pub fallbacks: u64,
    f0: Vec<f64>,
    f1: Vec<f64>,
    yp: Vec<f64>,
    dr: Vec<f64>,
}

impl StateJac {
    /// `pattern[i]` lists the states derivative `i` depends on.
    pub fn new(n: usize, pattern: &[Vec<u32>], mode: JacobianMode) -> StateJac {
        let mut rows_of_col: Vec<Vec<usize>> = vec![Vec::new(); n];
        for (i, deps) in pattern.iter().enumerate() {
            for &j in deps {
                rows_of_col[j as usize].push(i);
            }
        }
        for r in rows_of_col.iter_mut() {
            r.sort_unstable();
            r.dedup();
        }
        let mut color_of = vec![usize::MAX; n];
        let mut colors: Vec<Vec<usize>> = Vec::new();
        for j in 0..n {
            // The first color not used by any column sharing a row with j.
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
        let (csc, in_pat, nnz) = if n > DENSE_LIMIT {
            let cols: Vec<Vec<usize>> = (0..n)
                .map(|j| {
                    let mut c = rows_of_col[j].clone();
                    c.push(j);
                    c.sort_unstable();
                    c.dedup();
                    c
                })
                .collect();
            let in_pat: Vec<bool> = (0..n)
                .flat_map(|j| {
                    let r = &rows_of_col[j];
                    cols[j]
                        .iter()
                        .map(move |i| r.binary_search(i).is_ok())
                        .collect::<Vec<_>>()
                })
                .collect();
            let m = Csc::from_columns(n, &cols);
            let nnz = m.nnz();
            (Some(m), in_pat, nnz)
        } else {
            (None, Vec::new(), 0)
        };
        let dense_n = if csc.is_some() { 0 } else { n };
        StateJac {
            n,
            mode,
            colors,
            rows_of_col,
            dense: vec![0.0; dense_n * dense_n],
            csc,
            vals: vec![0.0; nnz],
            in_pat,
            fallbacks: 0,
            f0: vec![0.0; n],
            f1: vec![0.0; n],
            yp: vec![0.0; n],
            dr: Vec::new(),
        }
    }

    pub fn n_colors(&self) -> usize {
        self.colors.len()
    }

    pub fn is_sparse(&self) -> bool {
        self.csc.is_some()
    }

    /// Store column `j` from `col(i)` for the rows of its pattern.
    fn store_col(&mut self, j: usize, col: impl Fn(usize) -> f64) {
        let n = self.n;
        match &self.csc {
            None => {
                for &i in &self.rows_of_col[j] {
                    self.dense[i * n + j] = col(i);
                }
            }
            Some(m) => {
                for p in m.colptr[j]..m.colptr[j + 1] {
                    self.vals[p] = if self.in_pat[p] {
                        col(m.rowidx[p])
                    } else {
                        0.0
                    };
                }
            }
        }
    }

    /// Evaluate the Jacobian at `(t, y)`; false if the model could not be evaluated
    /// there (an algebraic loop failed or a derivative is not finite).
    pub fn compute(
        &mut self,
        rt: &mut Runtime,
        t: f64,
        y: &[f64],
        atol: f64,
        rtol: f64,
        stats: &mut SolverStats,
    ) -> bool {
        let mut f0 = std::mem::take(&mut self.f0);
        rt.rhs(t, y, &mut f0);
        stats.rhs_evals += 1;
        stats.jac_evals += 1;
        let ok = rt.error.is_none() && f0.iter().all(|v| v.is_finite());
        let done = self.mode == JacobianMode::Exact && ok && self.exact(rt, stats);
        let ok = if done {
            true
        } else {
            if self.mode == JacobianMode::Exact && ok {
                self.fallbacks += 1;
            }
            rt.rhs(t, y, &mut f0);
            self.differences(rt, t, y, &f0, atol, rtol, stats) && ok
        };
        self.f0 = f0;
        rt.error = None;
        ok
    }

    /// Exact columns by tangent runs; `rt` holds the solution at the point. Returns
    /// false (values partly written) if a column is not finite.
    fn exact(&mut self, rt: &mut Runtime, stats: &mut SolverStats) -> bool {
        let m = rt.m;
        let mut dr = std::mem::take(&mut self.dr);
        dr.clear();
        dr.resize(rt.r.len(), 0.0);
        let mut ok = true;
        for c in 0..self.colors.len() {
            for &j in &self.colors[c] {
                dr[m.states[j] as usize] = 1.0;
            }
            rt.run_tangent(&m.ode, &mut dr);
            stats.tangent_evals += 1;
            let group = std::mem::take(&mut self.colors[c]);
            for &j in &group {
                let d = &dr;
                self.store_col(j, |i| d[m.derivs[i] as usize]);
                ok &= self.rows_of_col[j]
                    .iter()
                    .all(|&i| dr[m.derivs[i] as usize].is_finite());
            }
            self.colors[c] = group;
            dr.fill(0.0);
            if rt.error.is_some() || !ok {
                ok = false;
                break;
            }
        }
        self.dr = dr;
        ok
    }

    #[allow(clippy::too_many_arguments)]
    fn differences(
        &mut self,
        rt: &mut Runtime,
        t: f64,
        y: &[f64],
        f0: &[f64],
        atol: f64,
        rtol: f64,
        stats: &mut SolverStats,
    ) -> bool {
        let mut ok = true;
        let mut yp = std::mem::take(&mut self.yp);
        let mut f1 = std::mem::take(&mut self.f1);
        yp.copy_from_slice(y);
        let eps = f64::EPSILON.sqrt();
        for c in 0..self.colors.len() {
            let group = std::mem::take(&mut self.colors[c]);
            let mut hs = Vec::with_capacity(group.len());
            for &j in &group {
                let h = eps * y[j].abs().max(atol / rtol.max(1e-12)).max(1e-8);
                yp[j] = y[j] + h;
                hs.push(h);
            }
            rt.rhs(t, &yp, &mut f1);
            stats.rhs_evals += 1;
            ok &= rt.error.is_none() && f1.iter().all(|v| v.is_finite());
            for (&j, &h) in group.iter().zip(&hs) {
                self.store_col(j, |i| (f1[i] - f0[i]) / h);
                yp[j] = y[j];
            }
            self.colors[c] = group;
        }
        self.yp = yp;
        self.f1 = f1;
        ok
    }

    /// `out = J v`.
    pub fn mul(&self, v: &[f64], out: &mut [f64]) {
        let n = self.n;
        out.fill(0.0);
        match &self.csc {
            None => {
                for i in 0..n {
                    out[i] = (0..n).map(|j| self.dense[i * n + j] * v[j]).sum();
                }
            }
            Some(m) => {
                for j in 0..n {
                    for p in m.colptr[j]..m.colptr[j + 1] {
                        out[m.rowidx[p]] += self.vals[p] * v[j];
                    }
                }
            }
        }
    }

    /// Estimate of the spectral radius by a few power iterations (used to judge
    /// whether an explicit method could take the current step).
    pub fn spectral_radius(&self) -> f64 {
        let n = self.n;
        if n == 0 {
            return 0.0;
        }
        let norm = |v: &[f64]| v.iter().map(|x| x * x).sum::<f64>().sqrt();
        // A deterministic, not-too-special start vector.
        let mut v: Vec<f64> = (0..n)
            .map(|i| 1.0 + 0.37 * ((i * 7919) % 13) as f64)
            .collect();
        let s = norm(&v);
        v.iter_mut().for_each(|x| *x /= s);
        let mut w = vec![0.0; n];
        let mut est = 0.0;
        // Products of two consecutive steps also catch complex pairs (|lambda|^2).
        for _ in 0..12 {
            self.mul(&v, &mut w);
            let a = norm(&w);
            if !a.is_finite() {
                return f64::INFINITY;
            }
            if a == 0.0 {
                return est;
            }
            w.iter_mut().for_each(|x| *x /= a);
            self.mul(&w, &mut v);
            let b = norm(&v);
            if !b.is_finite() {
                return f64::INFINITY;
            }
            est = (a * b).sqrt();
            if b == 0.0 {
                return est;
            }
            v.iter_mut().for_each(|x| *x /= b);
        }
        est
    }
}

/// LU factors of `a I - J`.
pub struct RealSystem {
    n: usize,
    lu: Vec<f64>,
    piv: Vec<usize>,
    sp: Option<(Csc, SparseLu)>,
}

impl RealSystem {
    pub fn new(jac: &StateJac) -> RealSystem {
        let n = jac.n;
        let sp = jac.csc.as_ref().map(|m| {
            let q = min_degree(m);
            (m.clone(), SparseLu::new(q))
        });
        let dn = if sp.is_some() { 0 } else { n };
        RealSystem {
            n,
            lu: vec![0.0; dn * dn],
            piv: vec![0; n],
            sp,
        }
    }

    /// Factor `a I - J`; false if singular.
    pub fn factor(&mut self, jac: &StateJac, a: f64) -> bool {
        let n = self.n;
        if let Some((m, lu)) = &mut self.sp {
            for j in 0..n {
                for p in m.colptr[j]..m.colptr[j + 1] {
                    let i = m.rowidx[p];
                    m.vals[p] = if i == j { a } else { 0.0 } - jac.vals[p];
                }
            }
            return lu.factor(m, 0.001);
        }
        for (l, j) in self.lu.iter_mut().zip(&jac.dense) {
            *l = -j;
        }
        for i in 0..n {
            self.lu[i * n + i] += a;
        }
        lu_factor(&mut self.lu, n, &mut self.piv)
    }

    pub fn solve(&mut self, b: &mut [f64]) {
        match &mut self.sp {
            Some((_, lu)) => lu.solve(b),
            None => lu_solve(&self.lu, self.n, &self.piv, b),
        }
    }
}

/// LU factors of `(a + i b) I - J`, as the real system of twice the size with the
/// real and imaginary part of each unknown next to each other:
/// `[a I - J, -b I; b I, a I - J]` with rows and columns interleaved.
pub struct ComplexSystem {
    n: usize,
    lu: Vec<f64>,
    piv: Vec<usize>,
    sp: Option<(Csc, SparseLu)>,
    buf: Vec<f64>,
}

impl ComplexSystem {
    pub fn new(jac: &StateJac) -> ComplexSystem {
        let n = jac.n;
        let sp = jac.csc.as_ref().map(|m| {
            let mut cols: Vec<Vec<usize>> = Vec::with_capacity(2 * n);
            for j in 0..n {
                let rows = &m.rowidx[m.colptr[j]..m.colptr[j + 1]];
                // Column 2j (real part of x_j): rows 2i of J's column, plus row 2j+1 (b).
                let mut re: Vec<usize> = rows.iter().map(|&i| 2 * i).collect();
                re.push(2 * j + 1);
                re.sort_unstable();
                // Column 2j+1 (imaginary part): rows 2i+1, plus row 2j (-b).
                let mut im: Vec<usize> = rows.iter().map(|&i| 2 * i + 1).collect();
                im.push(2 * j);
                im.sort_unstable();
                cols.push(re);
                cols.push(im);
            }
            let m2 = Csc::from_columns(2 * n, &cols);
            let q = min_degree(&m2);
            (m2, SparseLu::new(q))
        });
        let dn = if sp.is_some() { 0 } else { 2 * n };
        ComplexSystem {
            n,
            lu: vec![0.0; dn * dn],
            piv: vec![0; 2 * n],
            sp,
            buf: vec![0.0; 2 * n],
        }
    }

    /// Factor `(a + i b) I - J`; false if singular.
    pub fn factor(&mut self, jac: &StateJac, a: f64, b: f64) -> bool {
        let n = self.n;
        if let (Some((m2, lu)), Some(m)) = (&mut self.sp, &jac.csc) {
            for j in 0..n {
                let src = m.colptr[j]..m.colptr[j + 1];
                for half in 0..2 {
                    let c = 2 * j + half;
                    let mut k = src.start;
                    for p in m2.colptr[c]..m2.colptr[c + 1] {
                        let r = m2.rowidx[p];
                        if r % 2 == half {
                            // a block entry: a δ_ij - J_ij
                            let i = r / 2;
                            debug_assert_eq!(m.rowidx[k], i);
                            m2.vals[p] = if i == j { a } else { 0.0 } - jac.vals[k];
                            k += 1;
                        } else {
                            // the coupling of real and imaginary parts
                            m2.vals[p] = if half == 0 { b } else { -b };
                        }
                    }
                }
            }
            return lu.factor(m2, 0.001);
        }
        let n2 = 2 * n;
        self.lu.fill(0.0);
        for i in 0..n {
            for j in 0..n {
                let v = -jac.dense[i * n + j];
                self.lu[(2 * i) * n2 + 2 * j] = v;
                self.lu[(2 * i + 1) * n2 + 2 * j + 1] = v;
            }
            self.lu[(2 * i) * n2 + 2 * i] += a;
            self.lu[(2 * i + 1) * n2 + 2 * i + 1] += a;
            self.lu[(2 * i) * n2 + 2 * i + 1] = -b;
            self.lu[(2 * i + 1) * n2 + 2 * i] = b;
        }
        lu_factor(&mut self.lu, n2, &mut self.piv)
    }

    /// Solve for `re + i im` in place.
    pub fn solve(&mut self, re: &mut [f64], im: &mut [f64]) {
        let n = self.n;
        for i in 0..n {
            self.buf[2 * i] = re[i];
            self.buf[2 * i + 1] = im[i];
        }
        match &mut self.sp {
            Some((_, lu)) => lu.solve(&mut self.buf),
            None => lu_solve(&self.lu, 2 * n, &self.piv, &mut self.buf),
        }
        for i in 0..n {
            re[i] = self.buf[2 * i];
            im[i] = self.buf[2 * i + 1];
        }
    }
}
