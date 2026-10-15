//! Small dense linear algebra for the structure report: condition numbers, eigenvectors
//! by inverse iteration, and modal participation factors.
//!
//! The matrices here are state Jacobians and the Jacobians of single algebraic loops, so
//! dense `O(n^3)` methods are fine; callers cap `n`.

use crate::linearize::eigenvalues;

/// 1-norm of a dense matrix (largest absolute column sum).
pub fn norm1(a: &[Vec<f64>]) -> f64 {
    let n = a.first().map_or(0, |r| r.len());
    (0..n)
        .map(|j| a.iter().map(|r| r[j].abs()).sum::<f64>())
        .fold(0.0, f64::max)
}

/// Condition number `||A||_1 ||A^-1||_1` of a square matrix (infinite when singular to
/// working precision).
pub fn cond1(a: &[Vec<f64>]) -> f64 {
    if a.is_empty() {
        return 1.0;
    }
    let c = norm1(a) * inv_norm1(a);
    if c.is_finite() { c } else { f64::INFINITY }
}

/// `||A^-1||_1` of a square matrix (infinite when singular to working precision), from an
/// LU factorisation with partial pivoting.
pub fn inv_norm1(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    if n == 0 {
        return 0.0;
    }
    let an = norm1(a);
    if an == 0.0 || !an.is_finite() {
        return f64::INFINITY;
    }
    let mut lu: Vec<Vec<f64>> = a.to_vec();
    let mut perm: Vec<usize> = (0..n).collect();
    for k in 0..n {
        let p = (k..n)
            .max_by(|&i, &j| lu[i][k].abs().total_cmp(&lu[j][k].abs()))
            .unwrap();
        if lu[p][k].abs() <= f64::EPSILON * an * 1e-3 {
            return f64::INFINITY;
        }
        lu.swap(k, p);
        perm.swap(k, p);
        for i in k + 1..n {
            let f = lu[i][k] / lu[k][k];
            lu[i][k] = f;
            eliminate(&mut lu, k, i, f);
        }
    }
    // Columns of the inverse: solve A x = e_j.
    let mut inv_norm = 0.0f64;
    let mut x = vec![0.0; n];
    for j in 0..n {
        for (i, xi) in x.iter_mut().enumerate() {
            *xi = if perm[i] == j { 1.0 } else { 0.0 };
        }
        for i in 0..n {
            let s: f64 = (0..i).map(|k| lu[i][k] * x[k]).sum();
            x[i] -= s;
        }
        for i in (0..n).rev() {
            let s: f64 = (i + 1..n).map(|k| lu[i][k] * x[k]).sum();
            x[i] = (x[i] - s) / lu[i][i];
        }
        inv_norm = inv_norm.max(x.iter().map(|v| v.abs()).sum());
    }
    if inv_norm.is_finite() {
        inv_norm
    } else {
        f64::INFINITY
    }
}

/// Row `i` -= `f` * row `k`, right of column `k` (`k < i`).
fn eliminate(lu: &mut [Vec<f64>], k: usize, i: usize, f: f64) {
    let (top, bottom) = lu.split_at_mut(i);
    for (x, y) in bottom[0][k + 1..].iter_mut().zip(&top[k][k + 1..]) {
        *x -= f * y;
    }
}

/// Sign of the determinant of a square matrix (0 when singular to working precision).
pub fn det_sign(a: &[Vec<f64>]) -> f64 {
    let n = a.len();
    let mut lu: Vec<Vec<f64>> = a.to_vec();
    let mut sign = 1.0;
    for k in 0..n {
        let p = (k..n)
            .max_by(|&i, &j| lu[i][k].abs().total_cmp(&lu[j][k].abs()))
            .unwrap();
        if lu[p][k] == 0.0 || !lu[p][k].is_finite() {
            return 0.0;
        }
        if p != k {
            lu.swap(k, p);
            sign = -sign;
        }
        if lu[k][k] < 0.0 {
            sign = -sign;
        }
        for i in k + 1..n {
            let f = lu[i][k] / lu[k][k];
            eliminate(&mut lu, k, i, f);
        }
    }
    sign
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct C {
    pub re: f64,
    pub im: f64,
}

impl C {
    pub const ZERO: C = C { re: 0.0, im: 0.0 };
    pub fn new(re: f64, im: f64) -> C {
        C { re, im }
    }
    pub fn abs(self) -> f64 {
        self.re.hypot(self.im)
    }
    fn add(self, o: C) -> C {
        C::new(self.re + o.re, self.im + o.im)
    }
    fn sub(self, o: C) -> C {
        C::new(self.re - o.re, self.im - o.im)
    }
    fn mul(self, o: C) -> C {
        C::new(
            self.re * o.re - self.im * o.im,
            self.re * o.im + self.im * o.re,
        )
    }
    fn div(self, o: C) -> C {
        let d = o.re * o.re + o.im * o.im;
        C::new(
            (self.re * o.re + self.im * o.im) / d,
            (self.im * o.re - self.re * o.im) / d,
        )
    }
    fn scale(self, s: f64) -> C {
        C::new(self.re * s, self.im * s)
    }
}

/// LU factors of a complex matrix with partial pivoting; tiny pivots are replaced by a
/// small multiple of the norm, as inverse iteration wants (the matrix is singular by
/// design there).
struct ComplexLu {
    lu: Vec<Vec<C>>,
    perm: Vec<usize>,
}

impl ComplexLu {
    fn new(mut lu: Vec<Vec<C>>) -> ComplexLu {
        let n = lu.len();
        let norm = lu
            .iter()
            .flatten()
            .map(|z| z.abs())
            .fold(0.0, f64::max)
            .max(f64::MIN_POSITIVE);
        let mut perm: Vec<usize> = (0..n).collect();
        for k in 0..n {
            let p = (k..n)
                .max_by(|&i, &j| lu[i][k].abs().total_cmp(&lu[j][k].abs()))
                .unwrap();
            lu.swap(k, p);
            perm.swap(k, p);
            if lu[k][k].abs() < f64::EPSILON * norm {
                lu[k][k] = C::new(f64::EPSILON * norm, 0.0);
            }
            for i in k + 1..n {
                let f = lu[i][k].div(lu[k][k]);
                lu[i][k] = f;
                let (top, bottom) = lu.split_at_mut(i);
                for (x, y) in bottom[0][k + 1..].iter_mut().zip(&top[k][k + 1..]) {
                    *x = x.sub(f.mul(*y));
                }
            }
        }
        ComplexLu { lu, perm }
    }

    fn solve(&self, b: &[C]) -> Vec<C> {
        let n = b.len();
        let lu = &self.lu;
        let mut x: Vec<C> = self.perm.iter().map(|&p| b[p]).collect();
        for i in 0..n {
            let mut s = x[i];
            for k in 0..i {
                s = s.sub(lu[i][k].mul(x[k]));
            }
            x[i] = s;
        }
        for i in (0..n).rev() {
            let mut s = x[i];
            for k in i + 1..n {
                s = s.sub(lu[i][k].mul(x[k]));
            }
            x[i] = s.div(lu[i][i]);
        }
        x
    }
}

/// Eigenvector of `a` (or of its transpose: a left eigenvector) for the eigenvalue
/// `lambda`, by inverse iteration with a slightly shifted `lambda`. Normalised to unit
/// 2-norm.
pub fn eigenvector(a: &[Vec<f64>], lambda: C, transpose: bool) -> Vec<C> {
    let n = a.len();
    let scale = lambda.abs().max(norm1(a)).max(1e-300);
    let mu = lambda.add(C::new(1e-10 * scale, 0.0));
    let m: Vec<Vec<C>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| {
                    let x = if transpose { a[j][i] } else { a[i][j] };
                    if i == j {
                        C::new(x, 0.0).sub(mu)
                    } else {
                        C::new(x, 0.0)
                    }
                })
                .collect()
        })
        .collect();
    let lu = ComplexLu::new(m);
    // A start vector with no special structure.
    let mut v: Vec<C> = (0..n)
        .map(|i| C::new(1.0 + 0.37 * ((i * 7 + 3) % 11) as f64 / 11.0, 0.0))
        .collect();
    for _ in 0..3 {
        let x = lu.solve(&v);
        let nrm = x.iter().map(|z| z.abs().powi(2)).sum::<f64>().sqrt();
        if !(nrm.is_finite() && nrm > 0.0) {
            break;
        }
        v = x.into_iter().map(|z| z.scale(1.0 / nrm)).collect();
    }
    v
}

/// Normalised participation factors `p[mode][state] = |l_k r_k| / sum_j |l_j r_j|` of the
/// eigenvalues `eigs` of `a`, from the right (`r`) and left (`l`) eigenvectors. Each
/// row sums to 1.
pub fn participation(a: &[Vec<f64>], eigs: &[(f64, f64)]) -> Vec<Vec<f64>> {
    let n = a.len();
    eigs.iter()
        .map(|&(re, im)| {
            let lambda = C::new(re, im);
            let r = eigenvector(a, lambda, false);
            let l = eigenvector(a, lambda, true);
            let p: Vec<f64> = (0..n).map(|k| l[k].abs() * r[k].abs()).collect();
            let s: f64 = p.iter().sum();
            if s > 0.0 && s.is_finite() {
                p.into_iter().map(|x| x / s).collect()
            } else {
                vec![1.0 / n as f64; n]
            }
        })
        .collect()
}

/// Eigenvalues sorted by real part, then imaginary part.
pub fn sorted_eigenvalues(a: &[Vec<f64>]) -> Vec<(f64, f64)> {
    let mut e = eigenvalues(a);
    e.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
    e
}

/// `|| A r - lambda r ||_2` for a unit eigenvector `r` (a check on [`eigenvector`]).
pub fn eigen_residual(a: &[Vec<f64>], lambda: C, r: &[C]) -> f64 {
    let n = a.len();
    (0..n)
        .map(|i| {
            let mut s = C::ZERO;
            for j in 0..n {
                s = s.add(r[j].scale(a[i][j]));
            }
            s.sub(lambda.mul(r[i])).abs().powi(2)
        })
        .sum::<f64>()
        .sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cond_of_diagonal() {
        let a = vec![vec![2.0, 0.0], vec![0.0, 1e-3]];
        assert!((cond1(&a) - 2000.0).abs() < 1e-9);
        let s = vec![vec![1.0, 1.0], vec![1.0, 1.0]];
        assert!(cond1(&s).is_infinite());
    }

    #[test]
    fn eigenvectors_and_participation() {
        // Upper triangular with distinct eigenvalues -1, -2, -5.
        let a = vec![
            vec![-1.0, 3.0, 0.5],
            vec![0.0, -2.0, 1.0],
            vec![0.0, 0.0, -5.0],
        ];
        let e = sorted_eigenvalues(&a);
        for &(re, im) in &e {
            let l = C::new(re, im);
            let r = eigenvector(&a, l, false);
            assert!(eigen_residual(&a, l, &r) < 1e-8);
        }
        let p = participation(&a, &e);
        for row in &p {
            assert!((row.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        }
        // Decoupled states: each mode belongs to one state.
        let d = vec![
            vec![-1.0, 0.0, 0.0],
            vec![0.0, -10.0, 0.0],
            vec![0.0, 0.0, -100.0],
        ];
        let e = sorted_eigenvalues(&d);
        let p = participation(&d, &e);
        // e sorted: -100, -10, -1  -> states 2, 1, 0
        for (m, s) in [(0, 2), (1, 1), (2, 0)] {
            assert!((p[m][s] - 1.0).abs() < 1e-8, "{p:?}");
        }
    }
}
