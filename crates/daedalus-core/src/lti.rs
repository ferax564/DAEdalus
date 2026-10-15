//! Linear analysis of a linear time-invariant model `der(x) = A x + B u`, `y = C x + D u`
//! (usually from [`crate::linearize`]): frequency response (Bode, Nyquist, stability
//! margins), poles and transmission zeros, step and impulse responses, balanced truncation
//! model reduction, LQR state feedback and PID tuning suggestions.
//!
//! The dense linear algebra is self-contained: LU with partial pivoting, Householder
//! Hessenberg reduction (so that each frequency point costs `O(n^2)`), Padé matrix
//! exponential with scaling and squaring, Lyapunov and Riccati equations via the matrix
//! sign function, the cyclic Jacobi method for symmetric eigenproblems and one-sided
//! Jacobi SVD. Everything is `O(n^3)` and meant for models with up to a few hundred states.

use crate::linearize::{Linearization, eigenvalues};
use serde::{Deserialize, Serialize};

// ------------------------------------------------------------------ dense matrices

/// A dense row-major real matrix.
#[derive(Clone, Debug, PartialEq)]
pub struct Mat {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

impl std::ops::Index<(usize, usize)> for Mat {
    type Output = f64;
    fn index(&self, (i, j): (usize, usize)) -> &f64 {
        &self.data[i * self.cols + j]
    }
}

impl std::ops::IndexMut<(usize, usize)> for Mat {
    fn index_mut(&mut self, (i, j): (usize, usize)) -> &mut f64 {
        &mut self.data[i * self.cols + j]
    }
}

impl Mat {
    pub fn zeros(rows: usize, cols: usize) -> Mat {
        Mat {
            rows,
            cols,
            data: vec![0.0; rows * cols],
        }
    }

    pub fn eye(n: usize) -> Mat {
        let mut m = Mat::zeros(n, n);
        for i in 0..n {
            m[(i, i)] = 1.0;
        }
        m
    }

    /// From nested rows; `cols` gives the width when there are no rows.
    pub fn from_rows(rows: &[Vec<f64>], cols: usize) -> Mat {
        let mut m = Mat::zeros(rows.len(), cols);
        for (i, r) in rows.iter().enumerate() {
            assert_eq!(r.len(), cols, "ragged matrix");
            m.data[i * cols..(i + 1) * cols].copy_from_slice(r);
        }
        m
    }

    pub fn to_rows(&self) -> Vec<Vec<f64>> {
        (0..self.rows)
            .map(|i| self.data[i * self.cols..(i + 1) * self.cols].to_vec())
            .collect()
    }

    pub fn t(&self) -> Mat {
        let mut m = Mat::zeros(self.cols, self.rows);
        for i in 0..self.rows {
            for j in 0..self.cols {
                m[(j, i)] = self[(i, j)];
            }
        }
        m
    }

    pub fn mul(&self, b: &Mat) -> Mat {
        assert_eq!(self.cols, b.rows, "dimension mismatch");
        let mut c = Mat::zeros(self.rows, b.cols);
        for i in 0..self.rows {
            for k in 0..self.cols {
                let a = self[(i, k)];
                if a == 0.0 {
                    continue;
                }
                let brow = &b.data[k * b.cols..(k + 1) * b.cols];
                let crow = &mut c.data[i * b.cols..(i + 1) * b.cols];
                for (x, y) in crow.iter_mut().zip(brow) {
                    *x += a * y;
                }
            }
        }
        c
    }

    pub fn add(&self, b: &Mat) -> Mat {
        self.lin(1.0, b, 1.0)
    }

    pub fn sub(&self, b: &Mat) -> Mat {
        self.lin(1.0, b, -1.0)
    }

    /// `p * self + q * b`
    pub fn lin(&self, p: f64, b: &Mat, q: f64) -> Mat {
        assert_eq!((self.rows, self.cols), (b.rows, b.cols));
        Mat {
            rows: self.rows,
            cols: self.cols,
            data: self
                .data
                .iter()
                .zip(&b.data)
                .map(|(x, y)| p * x + q * y)
                .collect(),
        }
    }

    pub fn scale(&self, s: f64) -> Mat {
        Mat {
            rows: self.rows,
            cols: self.cols,
            data: self.data.iter().map(|x| x * s).collect(),
        }
    }

    /// Maximum absolute column sum.
    pub fn norm1(&self) -> f64 {
        (0..self.cols)
            .map(|j| (0..self.rows).map(|i| self[(i, j)].abs()).sum::<f64>())
            .fold(0.0, f64::max)
    }

    pub fn norm_fro(&self) -> f64 {
        self.data.iter().map(|x| x * x).sum::<f64>().sqrt()
    }

    /// Sub-matrix of rows `r0..r1` and columns `c0..c1`.
    pub fn block(&self, r0: usize, r1: usize, c0: usize, c1: usize) -> Mat {
        let mut m = Mat::zeros(r1 - r0, c1 - c0);
        for i in r0..r1 {
            for j in c0..c1 {
                m[(i - r0, j - c0)] = self[(i, j)];
            }
        }
        m
    }

    /// Symmetric part `(M + M^T) / 2`.
    pub fn sym(&self) -> Mat {
        self.add(&self.t()).scale(0.5)
    }

    /// LU factorisation with partial pivoting, or `None` when singular.
    pub fn lu(&self) -> Option<Lu> {
        assert_eq!(self.rows, self.cols);
        let n = self.rows;
        let mut a = self.clone();
        let mut piv = vec![0usize; n];
        // Singularity is judged per column against the column's own magnitude, so that
        // badly scaled models (pressures next to positions) are not reported singular.
        let colmax: Vec<f64> = (0..n)
            .map(|j| (0..n).fold(0.0f64, |m, i| m.max(self[(i, j)].abs())))
            .collect();
        if colmax.iter().any(|c| !c.is_finite()) {
            return None;
        }
        let mut sign = 1.0;
        let mut log_det = 0.0;
        for k in 0..n {
            let mut p = k;
            for i in k + 1..n {
                if a[(i, k)].abs() > a[(p, k)].abs() {
                    p = i;
                }
            }
            piv[k] = p;
            let best = a[(p, k)];
            if best.abs() <= colmax[k] * 1e-14 || best == 0.0 {
                return None;
            }
            if p != k {
                for j in 0..n {
                    a.data.swap(k * n + j, p * n + j);
                }
                sign = -sign;
            }
            if best < 0.0 {
                sign = -sign;
            }
            log_det += best.abs().ln();
            let d = 1.0 / a[(k, k)];
            for i in k + 1..n {
                let f = a[(i, k)] * d;
                if f == 0.0 {
                    continue;
                }
                a[(i, k)] = f;
                for j in k + 1..n {
                    let v = a[(k, j)];
                    a[(i, j)] -= f * v;
                }
            }
        }
        Some(Lu {
            a,
            piv,
            sign,
            log_det,
        })
    }

    pub fn inv(&self) -> Option<Mat> {
        Some(self.lu()?.solve(&Mat::eye(self.rows)))
    }

    /// Solve `self * X = b`.
    pub fn solve(&self, b: &Mat) -> Option<Mat> {
        Some(self.lu()?.solve(b))
    }

    pub fn col(&self, j: usize) -> Vec<f64> {
        (0..self.rows).map(|i| self[(i, j)]).collect()
    }
}

/// An LU factorisation `P A = L U`.
pub struct Lu {
    a: Mat,
    piv: Vec<usize>,
    /// Sign of the determinant.
    pub sign: f64,
    /// `ln |det A|`.
    pub log_det: f64,
}

impl Lu {
    pub fn solve(&self, b: &Mat) -> Mat {
        let n = self.a.rows;
        let mut x = b.clone();
        for k in 0..n {
            let p = self.piv[k];
            if p != k {
                for j in 0..x.cols {
                    x.data.swap(k * x.cols + j, p * x.cols + j);
                }
            }
        }
        for c in 0..x.cols {
            for i in 0..n {
                let mut s = x[(i, c)];
                for k in 0..i {
                    s -= self.a[(i, k)] * x[(k, c)];
                }
                x[(i, c)] = s;
            }
            for i in (0..n).rev() {
                let mut s = x[(i, c)];
                for k in i + 1..n {
                    s -= self.a[(i, k)] * x[(k, c)];
                }
                x[(i, c)] = s / self.a[(i, i)];
            }
        }
        x
    }
}

/// Diagonal balancing (Parlett–Reinsch, powers of two): `(D^-1 A D, d)` with row and
/// column norms of the off-diagonal part made comparable. Eigenvalues, zeros and
/// transfer functions are unchanged when `B` and `C` are scaled to `D^-1 B` and `C D`.
pub fn balance(a: &Mat) -> (Mat, Vec<f64>) {
    let n = a.rows;
    let mut b = a.clone();
    let mut d = vec![1.0; n];
    for _ in 0..100 {
        let mut done = true;
        for i in 0..n {
            let c: f64 = (0..n).filter(|&j| j != i).map(|j| b[(j, i)].abs()).sum();
            let r: f64 = (0..n).filter(|&j| j != i).map(|j| b[(i, j)].abs()).sum();
            if c == 0.0 || r == 0.0 {
                continue;
            }
            // find f = 2^k with c f ~ r / f
            let mut f = 1.0f64;
            let s = c + r;
            while c * f * f < r / 2.0 && f < 1e150 {
                f *= 2.0;
            }
            while c * f * f >= r * 2.0 && f > 1e-150 {
                f /= 2.0;
            }
            // D_ii *= f: column i times f, row i divided by f
            if (c * f + r / f) < 0.95 * s && f != 1.0 {
                done = false;
                d[i] *= f;
                for j in 0..n {
                    b[(j, i)] *= f;
                    b[(i, j)] /= f;
                }
            }
        }
        if done {
            break;
        }
    }
    (b, d)
}

/// Balance `(A, B, C)` together.
fn balance_system(a: &Mat, b: &Mat, c: &Mat) -> (Mat, Mat, Mat) {
    let (ab, d) = balance(a);
    let mut bb = b.clone();
    let mut cb = c.clone();
    for (i, &di) in d.iter().enumerate() {
        for j in 0..bb.cols {
            bb[(i, j)] /= di;
        }
        for r in 0..cb.rows {
            cb[(r, i)] *= di;
        }
    }
    (ab, bb, cb)
}

/// Least-squares solution of the overdetermined `a x = b` (Householder QR).
pub fn lstsq(a: &Mat, b: &Mat) -> Option<Mat> {
    let (m, n) = (a.rows, a.cols);
    assert!(m >= n && b.rows == m);
    let mut r = a.clone();
    let mut q = b.clone();
    for k in 0..n {
        let norm = (k..m).map(|i| r[(i, k)] * r[(i, k)]).sum::<f64>().sqrt();
        if norm == 0.0 {
            return None;
        }
        let alpha = if r[(k, k)] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k..m).map(|i| r[(i, k)]).collect();
        v[0] -= alpha;
        let vn = v.iter().map(|x| x * x).sum::<f64>();
        if vn == 0.0 {
            continue;
        }
        for j in k..n {
            let s: f64 = (k..m).map(|i| v[i - k] * r[(i, j)]).sum::<f64>() * 2.0 / vn;
            for i in k..m {
                r[(i, j)] -= s * v[i - k];
            }
        }
        for j in 0..q.cols {
            let s: f64 = (k..m).map(|i| v[i - k] * q[(i, j)]).sum::<f64>() * 2.0 / vn;
            for i in k..m {
                q[(i, j)] -= s * v[i - k];
            }
        }
    }
    let rmax = (0..n).fold(0.0f64, |x, i| x.max(r[(i, i)].abs()));
    let mut x = Mat::zeros(n, b.cols);
    for c in 0..b.cols {
        for i in (0..n).rev() {
            if r[(i, i)].abs() <= rmax * 1e-14 {
                return None;
            }
            let mut s = q[(i, c)];
            for k in i + 1..n {
                s -= r[(i, k)] * x[(k, c)];
            }
            x[(i, c)] = s / r[(i, i)];
        }
    }
    Some(x)
}

/// Matrix exponential (diagonal Padé approximant of degree 8 with scaling and squaring).
pub fn expm(a: &Mat) -> Mat {
    let n = a.rows;
    let norm = a.norm1();
    let s = if norm > 0.5 {
        (norm / 0.5).log2().ceil() as i32
    } else {
        0
    };
    let a = a.scale(0.5f64.powi(s));
    const Q: usize = 8;
    let mut c = 1.0;
    let mut x = Mat::eye(n);
    let mut num = Mat::eye(n);
    let mut den = Mat::eye(n);
    for k in 1..=Q {
        c *= (Q - k + 1) as f64 / (k * (2 * Q - k + 1)) as f64;
        x = a.mul(&x);
        num = num.lin(1.0, &x, c);
        den = den.lin(1.0, &x, if k % 2 == 0 { c } else { -c });
    }
    let mut e = den.solve(&num).unwrap_or_else(|| Mat::eye(n));
    for _ in 0..s {
        e = e.mul(&e);
    }
    e
}

/// Solve the Lyapunov equation `A X + X A^T + Q = 0` for a stable `A` (matrix sign
/// function iteration with determinant scaling).
pub fn lyap(a: &Mat, q: &Mat) -> Result<Mat, String> {
    let n = a.rows;
    if n == 0 {
        return Ok(Mat::zeros(0, 0));
    }
    let mut ak = a.clone();
    let mut qk = q.clone();
    for _ in 0..100 {
        let lu = ak.lu().ok_or("Lyapunov equation: A is singular")?;
        let ai = lu.solve(&Mat::eye(n));
        let c = (lu.log_det / n as f64).exp();
        let an = ak.lin(0.5 / c, &ai, 0.5 * c);
        let qn = qk.lin(0.5 / c, &ai.mul(&qk).mul(&ai.t()), 0.5 * c);
        let diff = an.sub(&ak).norm1();
        ak = an;
        qk = qn.sym();
        if !diff.is_finite() {
            break;
        }
        if diff <= 1e-13 * ak.norm1() {
            let err = ak.add(&Mat::eye(n)).norm1();
            if err > 1e-6 {
                return Err("Lyapunov equation: A is not stable".into());
            }
            return Ok(qk.scale(0.5));
        }
    }
    Err("Lyapunov equation: the sign iteration did not converge (is A stable?)".into())
}

/// Stabilising solution of the continuous algebraic Riccati equation
/// `A^T X + X A - X B R^-1 B^T X + Q = 0` (sign function of the Hamiltonian, then one
/// Newton–Kleinman refinement step).
pub fn care(a: &Mat, b: &Mat, q: &Mat, r: &Mat) -> Result<Mat, String> {
    let n = a.rows;
    let ri = r.inv().ok_or("Riccati equation: R is singular")?;
    let g = b.mul(&ri).mul(&b.t()).sym();
    let mut h = Mat::zeros(2 * n, 2 * n);
    for i in 0..n {
        for j in 0..n {
            h[(i, j)] = a[(i, j)];
            h[(i, n + j)] = -g[(i, j)];
            h[(n + i, j)] = -q[(i, j)];
            h[(n + i, n + j)] = -a[(j, i)];
        }
    }
    let mut z = h;
    let mut converged = false;
    for _ in 0..100 {
        let lu = z
            .lu()
            .ok_or("Riccati equation: the Hamiltonian has eigenvalues on the imaginary axis")?;
        let zi = lu.solve(&Mat::eye(2 * n));
        let c = (lu.log_det / (2 * n) as f64).exp();
        let zn = z.lin(0.5 / c, &zi, 0.5 * c);
        let diff = zn.sub(&z).norm1();
        z = zn;
        if !diff.is_finite() {
            break;
        }
        if diff <= 1e-12 * z.norm1() {
            converged = true;
            break;
        }
    }
    if !converged {
        return Err("Riccati equation: the sign iteration did not converge (is (A, B) stabilizable and (A, Q) detectable?)".into());
    }
    let mut lhs = Mat::zeros(2 * n, n);
    let mut rhs = Mat::zeros(2 * n, n);
    for i in 0..n {
        for j in 0..n {
            lhs[(i, j)] = z[(i, n + j)];
            lhs[(n + i, j)] = z[(n + i, n + j)] + if i == j { 1.0 } else { 0.0 };
            rhs[(i, j)] = -(z[(i, j)] + if i == j { 1.0 } else { 0.0 });
            rhs[(n + i, j)] = -z[(n + i, j)];
        }
    }
    let mut x = lstsq(&lhs, &rhs)
        .ok_or("Riccati equation: no stabilising solution (is (A, B) stabilizable?)")?
        .sym();
    // Newton–Kleinman: (A - G X)^T X+ + X+ (A - G X) + Q + X G X = 0
    for _ in 0..2 {
        let acl = a.sub(&g.mul(&x));
        let rhs = q.add(&x.mul(&g).mul(&x));
        match lyap(&acl.t(), &rhs) {
            Ok(xn) => x = xn.sym(),
            Err(_) => break,
        }
    }
    Ok(x)
}

/// Symmetric eigendecomposition by the cyclic Jacobi method: `(eigenvalues, V)` with
/// `S = V diag(λ) V^T`, eigenvalues in descending order.
pub fn sym_eig(s: &Mat) -> (Vec<f64>, Mat) {
    let n = s.rows;
    let mut a = s.sym();
    let mut v = Mat::eye(n);
    for _sweep in 0..100 {
        let off: f64 = (0..n)
            .flat_map(|i| (0..n).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| a[(i, j)] * a[(i, j)])
            .sum();
        let total = a.norm_fro().powi(2);
        if off <= 1e-30 * total || off == 0.0 {
            break;
        }
        for p in 0..n {
            for q in p + 1..n {
                let apq = a[(p, q)];
                if apq.abs() < 1e-300 {
                    continue;
                }
                let theta = (a[(q, q)] - a[(p, p)]) / (2.0 * apq);
                let t = theta.signum() / (theta.abs() + (theta * theta + 1.0).sqrt());
                let t = if theta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let sn = t * c;
                for k in 0..n {
                    let akp = a[(k, p)];
                    let akq = a[(k, q)];
                    a[(k, p)] = c * akp - sn * akq;
                    a[(k, q)] = sn * akp + c * akq;
                }
                for k in 0..n {
                    let apk = a[(p, k)];
                    let aqk = a[(q, k)];
                    a[(p, k)] = c * apk - sn * aqk;
                    a[(q, k)] = sn * apk + c * aqk;
                }
                for k in 0..n {
                    let vkp = v[(k, p)];
                    let vkq = v[(k, q)];
                    v[(k, p)] = c * vkp - sn * vkq;
                    v[(k, q)] = sn * vkp + c * vkq;
                }
            }
        }
    }
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| a[(j, j)].total_cmp(&a[(i, i)]));
    let vals = idx.iter().map(|&i| a[(i, i)]).collect();
    let mut vs = Mat::zeros(n, n);
    for (c, &i) in idx.iter().enumerate() {
        for k in 0..n {
            vs[(k, c)] = v[(k, i)];
        }
    }
    (vals, vs)
}

/// Singular value decomposition `M = U diag(σ) V^T` of a square or tall matrix by
/// one-sided Jacobi rotations; σ in descending order.
pub fn svd(m: &Mat) -> (Mat, Vec<f64>, Mat) {
    let (rows, n) = (m.rows, m.cols);
    let mut u = m.clone();
    let mut v = Mat::eye(n);
    for _sweep in 0..100 {
        let mut rotated = false;
        for i in 0..n {
            for j in i + 1..n {
                let (mut al, mut be, mut ga) = (0.0, 0.0, 0.0);
                for k in 0..rows {
                    al += u[(k, i)] * u[(k, i)];
                    be += u[(k, j)] * u[(k, j)];
                    ga += u[(k, i)] * u[(k, j)];
                }
                if ga.abs() <= 1e-15 * (al * be).sqrt() || ga == 0.0 {
                    continue;
                }
                rotated = true;
                let zeta = (be - al) / (2.0 * ga);
                let t = zeta.signum() / (zeta.abs() + (1.0 + zeta * zeta).sqrt());
                let t = if zeta == 0.0 { 1.0 } else { t };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = c * t;
                for k in 0..rows {
                    let x = u[(k, i)];
                    let y = u[(k, j)];
                    u[(k, i)] = c * x - s * y;
                    u[(k, j)] = s * x + c * y;
                }
                for k in 0..n {
                    let x = v[(k, i)];
                    let y = v[(k, j)];
                    v[(k, i)] = c * x - s * y;
                    v[(k, j)] = s * x + c * y;
                }
            }
        }
        if !rotated {
            break;
        }
    }
    let sig: Vec<f64> = (0..n)
        .map(|j| (0..rows).map(|k| u[(k, j)] * u[(k, j)]).sum::<f64>().sqrt())
        .collect();
    let mut idx: Vec<usize> = (0..n).collect();
    idx.sort_by(|&i, &j| sig[j].total_cmp(&sig[i]));
    let mut us = Mat::zeros(rows, n);
    let mut vs = Mat::zeros(n, n);
    let mut ss = Vec::with_capacity(n);
    for (c, &i) in idx.iter().enumerate() {
        let s = sig[i];
        ss.push(s);
        for k in 0..rows {
            us[(k, c)] = if s > 0.0 { u[(k, i)] / s } else { 0.0 };
        }
        for k in 0..n {
            vs[(k, c)] = v[(k, i)];
        }
    }
    (us, ss, vs)
}

/// Reduce `A` to upper Hessenberg form by Householder reflections: `(H, Q)` with
/// `A = Q H Q^T`.
pub fn hessenberg(a: &Mat) -> (Mat, Mat) {
    let n = a.rows;
    let mut h = a.clone();
    let mut q = Mat::eye(n);
    for k in 0..n.saturating_sub(2) {
        let norm = (k + 1..n)
            .map(|i| h[(i, k)] * h[(i, k)])
            .sum::<f64>()
            .sqrt();
        if norm == 0.0 {
            continue;
        }
        let alpha = if h[(k + 1, k)] > 0.0 { -norm } else { norm };
        let mut v: Vec<f64> = (k + 1..n).map(|i| h[(i, k)]).collect();
        v[0] -= alpha;
        let vn: f64 = v.iter().map(|x| x * x).sum();
        if vn == 0.0 {
            continue;
        }
        // H = P H P with P = I - 2 v v^T / vn acting on rows/cols k+1..n
        for j in 0..n {
            let s: f64 = (k + 1..n).map(|i| v[i - k - 1] * h[(i, j)]).sum::<f64>() * 2.0 / vn;
            for i in k + 1..n {
                h[(i, j)] -= s * v[i - k - 1];
            }
        }
        for i in 0..n {
            let s: f64 = (k + 1..n).map(|j| h[(i, j)] * v[j - k - 1]).sum::<f64>() * 2.0 / vn;
            for j in k + 1..n {
                h[(i, j)] -= s * v[j - k - 1];
            }
        }
        for i in 0..n {
            let s: f64 = (k + 1..n).map(|j| q[(i, j)] * v[j - k - 1]).sum::<f64>() * 2.0 / vn;
            for j in k + 1..n {
                q[(i, j)] -= s * v[j - k - 1];
            }
        }
        for i in k + 2..n {
            h[(i, k)] = 0.0;
        }
    }
    (h, q)
}

// ------------------------------------------------------------------ complex helpers

#[derive(Clone, Copy, Debug, PartialEq)]
struct C64 {
    re: f64,
    im: f64,
}

impl C64 {
    fn new(re: f64, im: f64) -> C64 {
        C64 { re, im }
    }
    fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn mul(self, o: C64) -> C64 {
        C64::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    fn div(self, o: C64) -> C64 {
        // Smith's algorithm
        if o.re.abs() >= o.im.abs() {
            let r = o.im / o.re;
            let d = o.re + o.im * r;
            C64::new((self.re + self.im * r) / d, (self.im - self.re * r) / d)
        } else {
            let r = o.re / o.im;
            let d = o.re * r + o.im;
            C64::new((self.re * r + self.im) / d, (self.im * r - self.re) / d)
        }
    }
    fn sub(self, o: C64) -> C64 {
        C64::new(self.re - o.re, self.im - o.im)
    }
}

/// A state-space model prepared for fast frequency-response evaluation:
/// `A = Q H Q^T` with `H` Hessenberg, `B' = Q^T B`, `C' = C Q`.
pub struct FreqEvaluator {
    h: Mat,
    b: Mat,
    c: Mat,
    d: Mat,
}

impl FreqEvaluator {
    pub fn new(a: &Mat, b: &Mat, c: &Mat, d: &Mat) -> FreqEvaluator {
        let (a, b, c) = balance_system(a, b, c);
        let (h, q) = hessenberg(&a);
        FreqEvaluator {
            h,
            b: q.t().mul(&b),
            c: c.mul(&q),
            d: d.clone(),
        }
    }

    /// `G(s)` for `s = sigma + j omega`, as `(re, im)` row-major `ny x nu` matrices.
    pub fn eval(&self, sigma: f64, omega: f64) -> (Mat, Mat) {
        let n = self.h.rows;
        let (ny, nu) = (self.d.rows, self.d.cols);
        let mut gr = self.d.clone();
        let mut gi = Mat::zeros(ny, nu);
        if n == 0 {
            return (gr, gi);
        }
        // M = s I - H (Hessenberg), solve M X = B' column by column with partial
        // pivoting between adjacent rows.
        for col in 0..nu {
            let mut m: Vec<C64> = Vec::with_capacity(n * n);
            for i in 0..n {
                for j in 0..n {
                    let v = -self.h[(i, j)];
                    m.push(if i == j {
                        C64::new(v + sigma, omega)
                    } else {
                        C64::new(v, 0.0)
                    });
                }
            }
            let mut x: Vec<C64> = (0..n).map(|i| C64::new(self.b[(i, col)], 0.0)).collect();
            for k in 0..n {
                if k + 1 < n && m[(k + 1) * n + k].abs() > m[k * n + k].abs() {
                    for j in k..n {
                        m.swap(k * n + j, (k + 1) * n + j);
                    }
                    x.swap(k, k + 1);
                }
                let piv = m[k * n + k];
                if piv.abs() == 0.0 {
                    // s is a pole
                    for i in 0..ny {
                        gr[(i, col)] = f64::INFINITY;
                        gi[(i, col)] = 0.0;
                    }
                    x.clear();
                    break;
                }
                if k + 1 < n {
                    let f = m[(k + 1) * n + k].div(piv);
                    if f.abs() != 0.0 {
                        for j in k..n {
                            let t = m[k * n + j];
                            m[(k + 1) * n + j] = m[(k + 1) * n + j].sub(f.mul(t));
                        }
                        x[k + 1] = x[k + 1].sub(f.mul(x[k]));
                    }
                }
            }
            if x.is_empty() {
                continue;
            }
            for i in (0..n).rev() {
                let mut s = x[i];
                for j in i + 1..n {
                    s = s.sub(m[i * n + j].mul(x[j]));
                }
                x[i] = s.div(m[i * n + i]);
            }
            for r in 0..ny {
                let (mut re, mut im) = (0.0, 0.0);
                for (i, xi) in x.iter().enumerate() {
                    re += self.c[(r, i)] * xi.re;
                    im += self.c[(r, i)] * xi.im;
                }
                gr[(r, col)] += re;
                gi[(r, col)] += im;
            }
        }
        (gr, gi)
    }
}

// ------------------------------------------------------------------ analysis results

/// A linear model `der(x) = A x + B u`, `y = C x + D u`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StateSpace {
    pub a: Vec<Vec<f64>>,
    pub b: Vec<Vec<f64>>,
    pub c: Vec<Vec<f64>>,
    pub d: Vec<Vec<f64>>,
}

impl StateSpace {
    pub fn mats(&self, nu: usize) -> (Mat, Mat, Mat, Mat) {
        let n = self.a.len();
        (
            Mat::from_rows(&self.a, n),
            Mat::from_rows(&self.b, nu),
            Mat::from_rows(&self.c, n),
            Mat::from_rows(&self.d, nu),
        )
    }
}

/// Frequency response of one input-output pair.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Bode {
    /// Frequencies in rad/s.
    pub omega: Vec<f64>,
    pub mag_db: Vec<f64>,
    /// Unwrapped phase in degrees.
    pub phase_deg: Vec<f64>,
    /// Nyquist curve: real and imaginary parts.
    pub re: Vec<f64>,
    pub im: Vec<f64>,
}

/// Classical stability margins (of the open loop `G`, for unity negative feedback).
#[derive(Clone, Debug, Default, Serialize)]
pub struct Margins {
    /// Gain margin in dB at the phase crossover frequency.
    pub gain_margin_db: Option<f64>,
    pub phase_crossover: Option<f64>,
    /// Phase margin in degrees at the gain crossover frequency.
    pub phase_margin_deg: Option<f64>,
    pub gain_crossover: Option<f64>,
}

/// A sampled time response.
#[derive(Clone, Debug, Default, Serialize)]
pub struct TimeResponse {
    pub t: Vec<f64>,
    pub y: Vec<f64>,
}

/// Characteristics of a step response.
#[derive(Clone, Debug, Default, Serialize)]
pub struct StepInfo {
    pub final_value: Option<f64>,
    /// 10 % to 90 % rise time.
    pub rise_time: Option<f64>,
    /// Time after which the response stays within 2 % of the step.
    pub settling_time: Option<f64>,
    /// Overshoot in percent of the step.
    pub overshoot: Option<f64>,
    pub peak: f64,
    pub peak_time: f64,
}

/// A first-order-plus-dead-time model `K e^{-L s} / (T s + 1)` fitted to a step response.
#[derive(Clone, Debug, Serialize)]
pub struct Fopdt {
    pub k: f64,
    pub t: f64,
    pub l: f64,
}

/// PID gains `u = Kp e + Ki ∫e + Kd de/dt` (`Ki = Kp/Ti`, `Kd = Kp Td`).
#[derive(Clone, Debug, Serialize)]
pub struct PidGains {
    pub method: String,
    pub kp: f64,
    pub ti: f64,
    pub td: f64,
    pub ki: f64,
    pub kd: f64,
}

/// PID tuning suggestion from the step response.
#[derive(Clone, Debug, Serialize)]
pub struct PidSuggestion {
    pub fopdt: Fopdt,
    /// SIMC PI rule (Skogestad), with closed-loop time constant `tau_c`.
    pub simc: PidGains,
    pub tau_c: f64,
    /// Ziegler–Nichols reaction-curve PID (only with a dead time).
    pub ziegler_nichols: Option<PidGains>,
    pub note: Option<String>,
}

/// Analysis of one input → output pair.
#[derive(Clone, Debug, Serialize)]
pub struct PairAnalysis {
    pub input: String,
    pub output: String,
    /// `G(0)`, when `A` is non-singular.
    pub dc_gain: Option<f64>,
    /// Relative degree (`None` when the transfer function is identically zero).
    pub relative_degree: Option<usize>,
    /// High-frequency gain: `D`, or `C A^{r-1} B` for relative degree `r`.
    pub hf_gain: f64,
    /// Transmission zeros (invariant zeros of a minimal realization) as (re, im).
    pub zeros: Vec<(f64, f64)>,
    pub margins: Margins,
    pub bode: Option<Bode>,
    pub step: Option<TimeResponse>,
    pub impulse: Option<TimeResponse>,
    pub step_info: Option<StepInfo>,
    pub pid: Option<PidSuggestion>,
    /// Frequency and step response of the balanced-truncation model (with `reduce`).
    pub reduced_bode: Option<Bode>,
    pub reduced_step: Option<TimeResponse>,
}

/// Balanced truncation result.
#[derive(Clone, Debug, Serialize)]
pub struct Reduction {
    pub order: usize,
    pub hankel_singular_values: Vec<f64>,
    /// `2 Σ_{i>order} σ_i`, an upper bound on the H∞ norm of the error.
    pub error_bound: f64,
    pub a: Vec<Vec<f64>>,
    pub b: Vec<Vec<f64>>,
    pub c: Vec<Vec<f64>>,
    pub d: Vec<Vec<f64>>,
    pub poles: Vec<(f64, f64)>,
}

/// LQR state feedback `u = -K x`.
#[derive(Clone, Debug, Serialize)]
pub struct Lqr {
    pub q: f64,
    pub r: f64,
    pub k: Vec<Vec<f64>>,
    /// Riccati solution.
    pub x: Vec<Vec<f64>>,
    pub closed_loop_poles: Vec<(f64, f64)>,
}

/// What to compute. Empty input/output lists select all.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LinearOptions {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// Frequency range in rad/s (default from the poles and zeros).
    pub w_min: Option<f64>,
    pub w_max: Option<f64>,
    /// Frequency points (default 400).
    pub n_freq: usize,
    /// Step response horizon (default from the slowest pole).
    pub t_final: Option<f64>,
    /// Time points (default 600, more for oscillatory models).
    pub n_time: usize,
    /// Keep Bode and Nyquist data in the result.
    pub bode: bool,
    /// Keep step and impulse responses in the result.
    pub step: bool,
    /// Balanced truncation to this order.
    pub reduce: Option<usize>,
    /// LQR with `Q = q I`, `R = r I`.
    pub lqr: Option<(f64, f64)>,
}

/// The result of [`analyze`].
#[derive(Clone, Debug, Serialize)]
pub struct LinearAnalysis {
    pub model: String,
    pub time: f64,
    pub states: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// The selected subsystem.
    pub system: StateSpace,
    pub poles: Vec<(f64, f64)>,
    /// All poles have a negative real part.
    pub stable: bool,
    pub pairs: Vec<PairAnalysis>,
    pub reduction: Option<Reduction>,
    pub reduction_error: Option<String>,
    pub lqr: Option<Lqr>,
    pub lqr_error: Option<String>,
}

// ------------------------------------------------------------------ algorithms

fn sorted_eigs(a: &Mat) -> Vec<(f64, f64)> {
    let mut e = eigenvalues(&balance(a).0.to_rows());
    e.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
    e.into_iter()
        .map(|(re, im)| (re + 0.0, if im.abs() < 1e-300 { 0.0 } else { im }))
        .collect()
}

/// Poles of the model: the eigenvalues of `A`.
pub fn poles(a: &Mat) -> Vec<(f64, f64)> {
    sorted_eigs(a)
}

/// Invariant zeros of the SISO system `(A, b, c, d)` and its relative degree and
/// high-frequency gain. With `d != 0` they are the eigenvalues of `A - b c / d`;
/// otherwise those of the zero dynamics `P A` restricted to the kernel of
/// `[c; c A; ...; c A^{r-1}]`, with `P = I - b (c A^{r-1} b)^{-1} c A^{r-1}`.
pub fn siso_zeros(a: &Mat, b: &[f64], c: &[f64], d: f64) -> (Vec<(f64, f64)>, Option<usize>, f64) {
    let n = a.rows;
    let bm = Mat {
        rows: n,
        cols: 1,
        data: b.to_vec(),
    };
    let cm = Mat {
        rows: 1,
        cols: n,
        data: c.to_vec(),
    };
    let scale_b = bm.norm_fro();
    let scale_c = cm.norm_fro();
    let scale_a = a.norm1().max(1e-300);
    if d != 0.0 && d.abs() > 1e-14 * scale_b * scale_c / scale_a {
        let az = a.sub(&bm.mul(&cm).scale(1.0 / d));
        return (sorted_eigs(&az), Some(0), d);
    }
    if n == 0 {
        return (Vec::new(), None, 0.0);
    }
    // Markov parameters c A^k b
    let mut rows: Vec<Mat> = Vec::new(); // c A^k
    let mut cak = cm.clone();
    let mut r = None;
    for k in 0..n {
        let mk = cak.mul(&bm)[(0, 0)];
        rows.push(cak.clone());
        let tol = 1e-10 * cak.norm_fro() * scale_b;
        if mk.abs() > tol {
            r = Some((k + 1, mk));
            break;
        }
        cak = cak.mul(a);
    }
    let Some((r, g)) = r else {
        return (Vec::new(), None, 0.0);
    };
    // P = I - b (c A^{r-1} b)^{-1} c A^{r-1}
    let car1 = &rows[r - 1];
    let p = Mat::eye(n).sub(&bm.mul(car1).scale(1.0 / g));
    let pa = p.mul(a);
    // orthonormal basis of ker [c; ...; c A^{r-1}]
    let basis = null_space(&rows, n);
    if basis.cols == 0 {
        return (Vec::new(), Some(r), g);
    }
    let z = basis.t().mul(&pa).mul(&basis);
    (sorted_eigs(&z), Some(r), g)
}

/// Orthonormal basis (columns) of the Krylov space `span{b, A b, A^2 b, ...}` (Arnoldi
/// with re-orthogonalisation): the controllable subspace of `(A, b)`.
fn krylov_basis(a: &Mat, b: &[f64]) -> Mat {
    let n = a.rows;
    let anorm = a.norm1().max(1e-300);
    let mut q: Vec<Vec<f64>> = Vec::new();
    let mut w = b.to_vec();
    for _ in 0..n {
        let n0 = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        for _ in 0..2 {
            for u in &q {
                let d: f64 = u.iter().zip(&w).map(|(x, y)| x * y).sum();
                for (x, y) in w.iter_mut().zip(u) {
                    *x -= d * y;
                }
            }
        }
        let nw = w.iter().map(|x| x * x).sum::<f64>().sqrt();
        // negligible new direction relative to A q_k itself (or to |A|)
        if nw <= 1e-10 * n0.max(1e-12 * anorm) || nw == 0.0 {
            break;
        }
        let v: Vec<f64> = w.iter().map(|x| x / nw).collect();
        w = (0..n)
            .map(|i| (0..n).map(|j| a[(i, j)] * v[j]).sum())
            .collect();
        q.push(v);
    }
    let mut m = Mat::zeros(n, q.len());
    for (c, v) in q.iter().enumerate() {
        for i in 0..n {
            m[(i, c)] = v[i];
        }
    }
    m
}

/// A minimal realization of the SISO system `(A, b, c)`: the controllable part, then
/// its observable part (orthogonal projections onto Krylov subspaces).
pub fn minimal_siso(a: &Mat, b: &[f64], c: &[f64]) -> (Mat, Vec<f64>, Vec<f64>) {
    let n = a.rows;
    let bm = Mat {
        rows: n,
        cols: 1,
        data: b.to_vec(),
    };
    let cm = Mat {
        rows: 1,
        cols: n,
        data: c.to_vec(),
    };
    let v = krylov_basis(a, b);
    let (a1, b1, c1) = (v.t().mul(a).mul(&v), v.t().mul(&bm), cm.mul(&v));
    let w = krylov_basis(&a1.t(), &c1.data);
    let (a2, b2, c2) = (w.t().mul(&a1).mul(&w), w.t().mul(&b1), c1.mul(&w));
    (a2, b2.data, c2.data)
}

/// Transmission zeros of the SISO pair: the invariant zeros of a minimal realization
/// (uncontrollable and unobservable modes removed), with relative degree and
/// high-frequency gain as in [`siso_zeros`].
pub fn transmission_zeros(
    a: &Mat,
    b: &[f64],
    c: &[f64],
    d: f64,
) -> (Vec<(f64, f64)>, Option<usize>, f64) {
    let n = a.rows;
    let (ab, bb, cb) = balance_system(
        a,
        &Mat {
            rows: n,
            cols: 1,
            data: b.to_vec(),
        },
        &Mat {
            rows: 1,
            cols: n,
            data: c.to_vec(),
        },
    );
    let (am, bm, cm) = minimal_siso(&ab, &bb.data, &cb.data);
    let (mut z, r, g) = siso_zeros(&am, &bm, &cm, d);
    if am.rows == 0 && d == 0.0 {
        return (z, None, 0.0);
    }
    // A multiple zero at the origin splits by about sqrt(eps) |A|: snap it back.
    let tiny = 2e-8 * am.norm1();
    for p in z.iter_mut() {
        if p.0.hypot(p.1) < tiny {
            *p = (0.0, 0.0);
        }
    }
    (z, r, g)
}

/// Orthonormal basis (columns) of the complement of the span of the given row vectors.
fn null_space(rows: &[Mat], n: usize) -> Mat {
    let mut q: Vec<Vec<f64>> = Vec::new();
    let push = |q: &mut Vec<Vec<f64>>, mut v: Vec<f64>| -> bool {
        let n0 = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        for _ in 0..2 {
            for u in q.iter() {
                let d: f64 = u.iter().zip(&v).map(|(a, b)| a * b).sum();
                for (x, y) in v.iter_mut().zip(u) {
                    *x -= d * y;
                }
            }
        }
        let nv = v.iter().map(|x| x * x).sum::<f64>().sqrt();
        if nv > 1e-10 * n0.max(1e-300) && nv > 0.0 {
            q.push(v.into_iter().map(|x| x / nv).collect());
            true
        } else {
            false
        }
    };
    for r in rows {
        push(&mut q, r.data.clone());
    }
    let k = q.len();
    for i in 0..n {
        if q.len() == n {
            break;
        }
        let mut e = vec![0.0; n];
        e[i] = 1.0;
        push(&mut q, e);
    }
    let mut m = Mat::zeros(n, n - k);
    for (c, v) in q[k..].iter().enumerate() {
        for i in 0..n {
            m[(i, c)] = v[i];
        }
    }
    m
}

/// Logarithmic grid of `n` points between `lo` and `hi`.
pub fn logspace(lo: f64, hi: f64, n: usize) -> Vec<f64> {
    let (a, b) = (lo.log10(), hi.log10());
    (0..n)
        .map(|i| 10f64.powf(a + (b - a) * i as f64 / (n.max(2) - 1) as f64))
        .collect()
}

/// Default frequency range from the magnitudes of poles and zeros: one decade beyond
/// the smallest and the largest.
pub fn default_freq_range(points: &[(f64, f64)]) -> (f64, f64) {
    let mags: Vec<f64> = points
        .iter()
        .map(|&(r, i)| r.hypot(i))
        .filter(|m| *m > 1e-9 && m.is_finite())
        .collect();
    if mags.is_empty() {
        return (1e-2, 1e2);
    }
    let lo = mags.iter().copied().fold(f64::INFINITY, f64::min);
    let hi = mags.iter().copied().fold(0.0, f64::max);
    let mut a = lo.log10().floor() - 1.0;
    let mut b = hi.log10().ceil() + 1.0;
    if b - a > 12.0 {
        a = b - 12.0;
    }
    if b - a < 2.0 {
        a -= 1.0;
        b += 1.0;
    }
    (10f64.powf(a), 10f64.powf(b))
}

/// Bode and Nyquist data of output `i` / input `j` of the evaluator over `omega`.
pub fn bode(ev: &FreqEvaluator, i: usize, j: usize, omega: &[f64]) -> Bode {
    let mut out = Bode {
        omega: omega.to_vec(),
        ..Default::default()
    };
    let mut prev: Option<f64> = None;
    for &w in omega {
        let (gr, gi) = ev.eval(0.0, w);
        let (re, im) = (gr[(i, j)], gi[(i, j)]);
        let mag = re.hypot(im);
        let mut ph = im.atan2(re).to_degrees();
        if let Some(p) = prev {
            ph -= 360.0 * ((ph - p) / 360.0).round();
        }
        if mag > 0.0 {
            prev = Some(ph);
        }
        out.mag_db.push(20.0 * mag.log10());
        out.phase_deg.push(if mag > 0.0 { ph } else { f64::NAN });
        out.re.push(re);
        out.im.push(im);
    }
    // Start the unwrapped phase in (-270, 90] for the common case of lags.
    if let Some(&p0) = out.phase_deg.iter().find(|p| p.is_finite()) {
        let shift = 360.0 * ((p0 - 90.0) / 360.0).ceil();
        if shift != 0.0 {
            out.phase_deg.iter_mut().for_each(|p| *p -= shift);
        }
    }
    out
}

/// Gain and phase margins from Bode data (first crossings, interpolated in log ω).
pub fn margins(b: &Bode) -> Margins {
    let mut m = Margins::default();
    let lw: Vec<f64> = b.omega.iter().map(|w| w.log10()).collect();
    for k in 1..b.omega.len() {
        let (m0, m1) = (b.mag_db[k - 1], b.mag_db[k]);
        if m.gain_crossover.is_none()
            && m0.is_finite()
            && m1.is_finite()
            && (m0 >= 0.0) != (m1 >= 0.0)
        {
            let f = m0 / (m0 - m1);
            let w = 10f64.powf(lw[k - 1] + f * (lw[k] - lw[k - 1]));
            let ph = b.phase_deg[k - 1] + f * (b.phase_deg[k] - b.phase_deg[k - 1]);
            m.gain_crossover = Some(w);
            let pm = 180.0 + ph;
            m.phase_margin_deg = Some(pm - 360.0 * (pm / 360.0).round());
        }
        let (p0, p1) = (b.phase_deg[k - 1], b.phase_deg[k]);
        if m.phase_crossover.is_none() && p0.is_finite() && p1.is_finite() {
            // crossing of -180 + 360 k
            let k0 = ((p0 + 180.0) / 360.0).floor();
            let k1 = ((p1 + 180.0) / 360.0).floor();
            let target = -180.0 + 360.0 * k0.max(k1);
            // ignore a phase that only hovers at -180 (rounding of a pure s^2 or -1)
            if k0 != k1 && (p0 - target).abs().max((p1 - target).abs()) > 1e-6 {
                let f = (target - p0) / (p1 - p0);
                let w = 10f64.powf(lw[k - 1] + f * (lw[k] - lw[k - 1]));
                let mag = b.mag_db[k - 1] + f * (b.mag_db[k] - b.mag_db[k - 1]);
                m.phase_crossover = Some(w);
                m.gain_margin_db = Some(-mag);
            }
        }
    }
    m
}

/// Default time horizon of step responses: about seven time constants of the slowest
/// stable pole (rounded up to 1, 2 or 5 × 10^k), 10 s without a usable pole.
pub fn default_horizon(poles: &[(f64, f64)]) -> f64 {
    let stable: Vec<f64> = poles.iter().filter(|p| p.0 < -1e-9).map(|p| -p.0).collect();
    let unstable = poles.iter().any(|p| p.0 >= -1e-9);
    let raw = if !stable.is_empty() && !unstable {
        7.0 / stable.iter().copied().fold(f64::INFINITY, f64::min)
    } else {
        let mags: Vec<f64> = poles
            .iter()
            .map(|p| p.0.hypot(p.1))
            .filter(|m| *m > 1e-9)
            .collect();
        if mags.is_empty() {
            10.0
        } else {
            10.0 / mags.iter().copied().fold(f64::INFINITY, f64::min)
        }
    };
    let raw = raw.clamp(1e-9, 1e9);
    let e = raw.log10().floor();
    let base = 10f64.powf(e);
    let f = raw / base;
    let nice = if f <= 1.0 {
        1.0
    } else if f <= 2.0 {
        2.0
    } else if f <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * base
}

/// Step and impulse responses of all input/output pairs on `n` points over
/// `[0, t_final]`, exact for piecewise constant inputs (zero-order hold discretization
/// with the matrix exponential). Returns `(t, step[i][j], impulse[i][j])` indexed by
/// output, then input.
#[allow(clippy::type_complexity)]
pub fn time_responses(
    a: &Mat,
    b: &Mat,
    c: &Mat,
    d: &Mat,
    t_final: f64,
    n: usize,
) -> (Vec<f64>, Vec<Vec<Vec<f64>>>, Vec<Vec<Vec<f64>>>) {
    let nx = a.rows;
    let (ny, nu) = (d.rows, d.cols);
    let n = n.max(2);
    let h = t_final / (n - 1) as f64;
    let t: Vec<f64> = (0..n).map(|k| k as f64 * h).collect();
    let mut aug = Mat::zeros(nx + nu, nx + nu);
    for i in 0..nx {
        for j in 0..nx {
            aug[(i, j)] = a[(i, j)] * h;
        }
        for j in 0..nu {
            aug[(i, nx + j)] = b[(i, j)] * h;
        }
    }
    let e = expm(&aug);
    let ad = e.block(0, nx, 0, nx);
    let bd = e.block(0, nx, nx, nx + nu);
    let mut step = vec![vec![Vec::with_capacity(n); nu]; ny];
    let mut imp = vec![vec![Vec::with_capacity(n); nu]; ny];
    for j in 0..nu {
        let bdj = bd.col(j);
        let mut xs = vec![0.0; nx];
        let mut xi = b.col(j);
        for _k in 0..n {
            for i in 0..ny {
                let ys: f64 = (0..nx).map(|l| c[(i, l)] * xs[l]).sum::<f64>() + d[(i, j)];
                let yi: f64 = (0..nx).map(|l| c[(i, l)] * xi[l]).sum();
                step[i][j].push(ys);
                imp[i][j].push(yi);
            }
            let ns: Vec<f64> = (0..nx)
                .map(|r| (0..nx).map(|l| ad[(r, l)] * xs[l]).sum::<f64>() + bdj[r])
                .collect();
            let ni: Vec<f64> = (0..nx)
                .map(|r| (0..nx).map(|l| ad[(r, l)] * xi[l]).sum::<f64>())
                .collect();
            xs = ns;
            xi = ni;
        }
    }
    (t, step, imp)
}

/// Time at which `y` first reaches `level` (rising from `y[0]` towards `level`),
/// linearly interpolated.
fn first_crossing(t: &[f64], y: &[f64], level: f64) -> Option<f64> {
    let up = level >= y[0];
    for k in 1..y.len() {
        let hit = if up { y[k] >= level } else { y[k] <= level };
        if hit {
            let (y0, y1) = (y[k - 1], y[k]);
            let f = if y1 != y0 {
                (level - y0) / (y1 - y0)
            } else {
                1.0
            };
            return Some(t[k - 1] + f.clamp(0.0, 1.0) * (t[k] - t[k - 1]));
        }
    }
    None
}

/// Rise time, settling time and overshoot of a step response with final value `yf`.
pub fn step_info(t: &[f64], y: &[f64], yf: Option<f64>) -> StepInfo {
    let (mut pk, mut pt) = (y[0], t[0]);
    let y0 = y[0];
    let mut info = StepInfo::default();
    let up = yf.is_none_or(|f| f >= y0);
    for (k, &v) in y.iter().enumerate() {
        if (up && v > pk) || (!up && v < pk) {
            pk = v;
            pt = t[k];
        }
    }
    info.peak = pk;
    info.peak_time = pt;
    info.final_value = yf;
    if let Some(f) = yf {
        let span = f - y0;
        if span.abs() > 1e-300 {
            info.rise_time = first_crossing(t, y, y0 + 0.9 * span)
                .zip(first_crossing(t, y, y0 + 0.1 * span))
                .map(|(t9, t1)| t9 - t1);
            info.overshoot = Some(((pk - f) / span * 100.0).max(0.0));
            let band = 0.02 * span.abs();
            let last_out = y.iter().rposition(|v| (v - f).abs() > band);
            info.settling_time = match last_out {
                None => Some(0.0),
                Some(k) if k + 1 < y.len() => Some(t[k + 1]),
                _ => None,
            };
        }
    }
    info
}

/// Fit a first-order-plus-dead-time model to a step response (Smith's two-point method,
/// 28.3 % and 63.2 % of the step) and suggest SIMC PI and Ziegler–Nichols PID gains.
pub fn pid_suggestion(t: &[f64], y: &[f64], yf: f64) -> Option<PidSuggestion> {
    let y0 = y[0];
    let k = yf;
    if !k.is_finite() || k.abs() < 1e-300 || (yf - y0).abs() < 1e-300 {
        return None;
    }
    let t28 = first_crossing(t, y, y0 + 0.283 * (yf - y0))?;
    let t63 = first_crossing(t, y, y0 + 0.632 * (yf - y0))?;
    let tau = (1.5 * (t63 - t28)).max(1e-12);
    let l = (t63 - tau).max(0.0);
    let tau_c = l.max(0.1 * tau);
    let kp = tau / (k * (tau_c + l));
    let ti = tau.min(4.0 * (tau_c + l));
    let simc = PidGains {
        method: "SIMC PI".into(),
        kp,
        ti,
        td: 0.0,
        ki: kp / ti,
        kd: 0.0,
    };
    let zn = (l > 1e-3 * tau).then(|| {
        let kp = 1.2 * tau / (k * l);
        let (ti, td) = (2.0 * l, 0.5 * l);
        PidGains {
            method: "Ziegler-Nichols PID (reaction curve)".into(),
            kp,
            ti,
            td,
            ki: kp / ti,
            kd: kp * td,
        }
    });
    let info = step_info(t, y, Some(yf));
    let note = if info.overshoot.unwrap_or(0.0) > 10.0 {
        Some("the step response overshoots: a first-order fit is rough, check the loop by simulation".into())
    } else if zn.is_none() {
        Some("no apparent dead time: tau_c = 0.1 T; Ziegler-Nichols needs a dead time".into())
    } else {
        None
    };
    Some(PidSuggestion {
        fopdt: Fopdt { k, t: tau, l },
        simc,
        tau_c,
        ziegler_nichols: zn,
        note,
    })
}

/// Balanced truncation of a stable model to `order` states (square-root method).
pub fn balanced_truncation(
    a: &Mat,
    b: &Mat,
    c: &Mat,
    d: &Mat,
    order: usize,
) -> Result<Reduction, String> {
    let n = a.rows;
    let p = lyap(a, &b.mul(&b.t()))?;
    let q = lyap(&a.t(), &c.t().mul(c))?;
    let factor = |s: &Mat| -> Mat {
        let (l, v) = sym_eig(s);
        let mut f = v.clone();
        for j in 0..n {
            let r = l[j].max(0.0).sqrt();
            for i in 0..n {
                f[(i, j)] *= r;
            }
        }
        f
    };
    let lc = factor(&p);
    let lo = factor(&q);
    let (u, sig, v) = svd(&lo.t().mul(&lc));
    let smax = sig.first().copied().unwrap_or(0.0);
    let rank = sig.iter().filter(|&&s| s > 1e-12 * smax && s > 0.0).count();
    let r = order.min(rank);
    let mut t = Mat::zeros(n, r);
    let mut ti = Mat::zeros(r, n);
    let lcv = lc.mul(&v);
    let lou = lo.mul(&u);
    for j in 0..r {
        let s = 1.0 / sig[j].sqrt();
        for i in 0..n {
            t[(i, j)] = lcv[(i, j)] * s;
            ti[(j, i)] = lou[(i, j)] * s;
        }
    }
    let ar = ti.mul(a).mul(&t);
    let br = ti.mul(b);
    let cr = c.mul(&t);
    let error_bound = 2.0 * sig[r..].iter().sum::<f64>();
    Ok(Reduction {
        order: r,
        hankel_singular_values: sig,
        error_bound,
        poles: sorted_eigs(&ar),
        a: ar.to_rows(),
        b: br.to_rows(),
        c: cr.to_rows(),
        d: d.to_rows(),
    })
}

/// LQR gain `K = R^-1 B^T X` minimising `∫ x^T Q x + u^T R u dt`.
pub fn lqr(a: &Mat, b: &Mat, q: &Mat, r: &Mat) -> Result<(Mat, Mat), String> {
    let x = care(a, b, q, r)?;
    let k = r.solve(&b.t().mul(&x)).ok_or("LQR: R is singular")?;
    Ok((k, x))
}

/// Analyze the linearization: select inputs and outputs, then compute poles, zeros,
/// frequency and time responses for every pair, and optionally a reduced model and an
/// LQR gain.
pub fn analyze(lin: &Linearization, opts: &LinearOptions) -> Result<LinearAnalysis, String> {
    let n = lin.states.len();
    let pick = |all: &[String], want: &[String], what: &str| -> Result<Vec<usize>, String> {
        if want.is_empty() {
            return Ok((0..all.len()).collect());
        }
        want.iter()
            .map(|w| {
                all.iter()
                    .position(|x| x == w)
                    .ok_or_else(|| format!("'{w}' is not one of the {what} [{}]", all.join(", ")))
            })
            .collect()
    };
    let iu = pick(&lin.inputs, &opts.inputs, "inputs")?;
    let iy = pick(&lin.outputs, &opts.outputs, "outputs")?;
    let a = Mat::from_rows(&lin.a, n);
    let mut b = Mat::zeros(n, iu.len());
    let mut c = Mat::zeros(iy.len(), n);
    let mut d = Mat::zeros(iy.len(), iu.len());
    for (jj, &j) in iu.iter().enumerate() {
        for i in 0..n {
            b[(i, jj)] = lin.b[i][j];
        }
        for (ii, &i) in iy.iter().enumerate() {
            d[(ii, jj)] = lin.d[i][j];
        }
    }
    for (ii, &i) in iy.iter().enumerate() {
        for k in 0..n {
            c[(ii, k)] = lin.c[i][k];
        }
    }
    let poles = poles(&a);
    let stable = poles.iter().all(|p| p.0 < 0.0);
    let dc = if n == 0 {
        Some(d.clone())
    } else {
        a.solve(&b).map(|x| d.sub(&c.mul(&x)))
    };
    let ev = FreqEvaluator::new(&a, &b, &c, &d);
    let t_final = opts.t_final.unwrap_or_else(|| default_horizon(&poles));
    let n_time = if opts.n_time == 0 {
        // at least 40 points per period of the fastest oscillation (up to 4000)
        let wmax = poles.iter().map(|p| p.1.abs()).fold(0.0, f64::max);
        let periods = t_final * wmax / (2.0 * std::f64::consts::PI);
        ((40.0 * periods).ceil() as usize).clamp(600, 4000)
    } else {
        opts.n_time
    };
    let (t, step, imp) = time_responses(&a, &b, &c, &d, t_final, n_time);
    let mut pairs = Vec::new();
    for (jj, &j) in iu.iter().enumerate() {
        for (ii, &i) in iy.iter().enumerate() {
            let (zeros, rd, hf) =
                transmission_zeros(&a, &b.col(jj), &c.data[ii * n..(ii + 1) * n], d[(ii, jj)]);
            let pts: Vec<(f64, f64)> = poles.iter().chain(zeros.iter()).copied().collect();
            let (dlo, dhi) = default_freq_range(&pts);
            let (wlo, whi) = (opts.w_min.unwrap_or(dlo), opts.w_max.unwrap_or(dhi));
            let nf = if opts.n_freq == 0 { 400 } else { opts.n_freq };
            let bd = bode(&ev, ii, jj, &logspace(wlo, whi, nf));
            let mg = margins(&bd);
            let dc_gain = dc.as_ref().map(|m| m[(ii, jj)] + 0.0);
            let yf = if stable { dc_gain } else { None };
            let info = step_info(&t, &step[ii][jj], yf);
            let pid = yf.and_then(|f| pid_suggestion(&t, &step[ii][jj], f));
            pairs.push(PairAnalysis {
                input: lin.inputs[j].clone(),
                output: lin.outputs[i].clone(),
                dc_gain,
                relative_degree: rd,
                hf_gain: hf,
                zeros,
                margins: mg,
                bode: opts.bode.then_some(bd),
                step: opts.step.then(|| TimeResponse {
                    t: t.clone(),
                    y: step[ii][jj].clone(),
                }),
                impulse: opts.step.then(|| TimeResponse {
                    t: t.clone(),
                    y: imp[ii][jj].clone(),
                }),
                step_info: Some(info),
                pid,
                reduced_bode: None,
                reduced_step: None,
            });
        }
    }
    let (reduction, reduction_error) = match opts.reduce {
        None => (None, None),
        Some(order) => match balanced_truncation(&a, &b, &c, &d, order) {
            Ok(r) => (Some(r), None),
            Err(e) => (None, Some(e)),
        },
    };
    if let Some(red) = &reduction {
        let r = red.order;
        let (ar, br, cr, dr) = (
            Mat::from_rows(&red.a, r),
            Mat::from_rows(&red.b, iu.len()),
            Mat::from_rows(&red.c, r),
            Mat::from_rows(&red.d, iu.len()),
        );
        let evr = FreqEvaluator::new(&ar, &br, &cr, &dr);
        let (tr, str_, _) = time_responses(&ar, &br, &cr, &dr, t_final, n_time);
        for (k, pr) in pairs.iter_mut().enumerate() {
            let (jj, ii) = (k / iy.len(), k % iy.len());
            if let Some(bd) = &pr.bode {
                pr.reduced_bode = Some(bode(&evr, ii, jj, &bd.omega));
            }
            if opts.step {
                pr.reduced_step = Some(TimeResponse {
                    t: tr.clone(),
                    y: str_[ii][jj].clone(),
                });
            }
        }
    }
    let (lqr_res, lqr_error) = match opts.lqr {
        None => (None, None),
        Some((qw, rw)) => {
            let qm = Mat::eye(n).scale(qw);
            let rm = Mat::eye(iu.len()).scale(rw);
            match lqr(&a, &b, &qm, &rm) {
                Ok((k, x)) => {
                    let cl = a.sub(&b.mul(&k));
                    (
                        Some(Lqr {
                            q: qw,
                            r: rw,
                            k: k.to_rows(),
                            x: x.to_rows(),
                            closed_loop_poles: sorted_eigs(&cl),
                        }),
                        None,
                    )
                }
                Err(e) => (None, Some(e)),
            }
        }
    };
    Ok(LinearAnalysis {
        model: lin.model.clone(),
        time: lin.time,
        states: lin.states.clone(),
        inputs: iu.iter().map(|&j| lin.inputs[j].clone()).collect(),
        outputs: iy.iter().map(|&i| lin.outputs[i].clone()).collect(),
        system: StateSpace {
            a: a.to_rows(),
            b: b.to_rows(),
            c: c.to_rows(),
            d: d.to_rows(),
        },
        poles,
        stable,
        pairs,
        reduction,
        reduction_error,
        lqr: lqr_res,
        lqr_error,
    })
}

fn fmt_c((re, im): (f64, f64)) -> String {
    if im == 0.0 {
        format!("{re:.6e}")
    } else {
        format!(
            "{re:.6e} {} {:.6e}i",
            if im < 0.0 { '-' } else { '+' },
            im.abs()
        )
    }
}

fn opt(v: Option<f64>, unit: &str) -> String {
    v.map_or("-".into(), |x| format!("{}{unit}", sig4(x)))
}

/// Four significant digits.
fn sig4(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let e = x.abs().log10().floor();
    if (-3.0..5.0).contains(&e) {
        format!("{x:.*}", (3.0 - e).max(0.0) as usize)
    } else {
        format!("{x:.3e}")
    }
}

impl LinearAnalysis {
    /// Human-readable summary.
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "linear analysis of {} at t = {} ({} states, {} inputs, {} outputs)",
            self.model,
            self.time,
            self.states.len(),
            self.inputs.len(),
            self.outputs.len()
        );
        let _ = writeln!(
            s,
            "\npoles ({}):",
            if self.stable { "stable" } else { "NOT stable" }
        );
        for &p in &self.poles {
            let (re, im) = p;
            let w = re.hypot(im);
            let zeta = if w > 0.0 { -re / w } else { f64::NAN };
            let _ = writeln!(
                s,
                "  {:<36} |p| = {w:.4e} rad/s, damping {zeta:.3}",
                fmt_c(p)
            );
        }
        for pr in &self.pairs {
            let _ = writeln!(s, "\n{} -> {}:", pr.input, pr.output);
            let _ = writeln!(s, "  DC gain        {}", opt(pr.dc_gain, ""));
            match pr.relative_degree {
                Some(r) => {
                    let _ = writeln!(
                        s,
                        "  rel. degree    {r} (high-frequency gain {:.4e})",
                        pr.hf_gain
                    );
                }
                None => {
                    let _ = writeln!(s, "  transfer function is identically zero");
                }
            }
            if pr.zeros.is_empty() {
                let _ = writeln!(s, "  zeros          none");
            } else {
                let _ = writeln!(s, "  zeros:");
                for &z in &pr.zeros {
                    let _ = writeln!(s, "    {}", fmt_c(z));
                }
            }
            let m = &pr.margins;
            let _ = writeln!(
                s,
                "  gain margin    {} at {}",
                opt(m.gain_margin_db, " dB"),
                opt(m.phase_crossover, " rad/s")
            );
            let _ = writeln!(
                s,
                "  phase margin   {} at {}",
                opt(m.phase_margin_deg, " deg"),
                opt(m.gain_crossover, " rad/s")
            );
            if let Some(si) = &pr.step_info {
                let _ = writeln!(
                    s,
                    "  step: rise {}, settling (2 %) {}, overshoot {}, peak {:.4e} at {} s",
                    opt(si.rise_time, " s"),
                    opt(si.settling_time, " s"),
                    opt(si.overshoot, " %"),
                    si.peak,
                    sig4(si.peak_time)
                );
            }
            if let Some(p) = &pr.pid {
                let _ = writeln!(
                    s,
                    "  FOPDT fit: K = {:.4e}, T = {:.4e} s, L = {:.4e} s",
                    p.fopdt.k, p.fopdt.t, p.fopdt.l
                );
                for g in std::iter::once(&p.simc).chain(p.ziegler_nichols.as_ref()) {
                    let _ = writeln!(
                        s,
                        "  {}: Kp = {:.4e}, Ti = {:.4e} s, Td = {:.4e} s",
                        g.method, g.kp, g.ti, g.td
                    );
                }
                if let Some(n) = &p.note {
                    let _ = writeln!(s, "  note: {n}");
                }
            }
        }
        if let Some(r) = &self.reduction {
            let _ = writeln!(s, "\nbalanced truncation to {} states:", r.order);
            let hsv: Vec<String> = r
                .hankel_singular_values
                .iter()
                .map(|x| format!("{x:.3e}"))
                .collect();
            let _ = writeln!(s, "  Hankel singular values: {}", hsv.join(" "));
            let _ = writeln!(s, "  H-infinity error bound: {:.3e}", r.error_bound);
            let _ = writeln!(s, "  reduced poles:");
            for &p in &r.poles {
                let _ = writeln!(s, "    {}", fmt_c(p));
            }
        }
        if let Some(e) = &self.reduction_error {
            let _ = writeln!(s, "\nbalanced truncation failed: {e}");
        }
        if let Some(l) = &self.lqr {
            let _ = writeln!(s, "\nLQR (Q = {} I, R = {} I), u = -K x:", l.q, l.r);
            for row in &l.k {
                let cells: Vec<String> = row.iter().map(|v| format!("{v:>12.4e}")).collect();
                let _ = writeln!(s, "  [{}]", cells.join(" "));
            }
            let _ = writeln!(s, "  closed-loop poles:");
            for &p in &l.closed_loop_poles {
                let _ = writeln!(s, "    {}", fmt_c(p));
            }
        }
        if let Some(e) = &self.lqr_error {
            let _ = writeln!(s, "\nLQR failed: {e}");
        }
        s
    }

    /// CSV of the Bode data (`omega` column, then magnitude and phase per pair).
    pub fn bode_csv(&self) -> String {
        let pairs: Vec<&PairAnalysis> = self.pairs.iter().filter(|p| p.bode.is_some()).collect();
        let mut s = String::from("omega");
        for p in &pairs {
            s += &format!(
                ",\"|{o}/{i}| [dB]\",\"arg {o}/{i} [deg]\"",
                o = p.output,
                i = p.input
            );
        }
        s.push('\n');
        if let Some(first) = pairs.first().and_then(|p| p.bode.as_ref()) {
            for k in 0..first.omega.len() {
                s += &format!("{}", first.omega[k]);
                for p in &pairs {
                    let b = p.bode.as_ref().unwrap();
                    s += &format!(",{},{}", b.mag_db[k], b.phase_deg[k]);
                }
                s.push('\n');
            }
        }
        s
    }

    /// CSV of the step responses (`time` column, then one column per pair).
    pub fn step_csv(&self) -> String {
        let pairs: Vec<&PairAnalysis> = self.pairs.iter().filter(|p| p.step.is_some()).collect();
        let mut s = String::from("time");
        for p in &pairs {
            s += &format!(",\"{} <- step {}\"", p.output, p.input);
        }
        s.push('\n');
        if let Some(first) = pairs.first().and_then(|p| p.step.as_ref()) {
            for k in 0..first.t.len() {
                s += &format!("{}", first.t[k]);
                for p in &pairs {
                    s += &format!(",{}", p.step.as_ref().unwrap().y[k]);
                }
                s.push('\n');
            }
        }
        s
    }
}
