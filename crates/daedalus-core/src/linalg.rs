//! Small dense linear algebra: LU factorisation with partial pivoting.
//! Matrices are row-major `n x n` slices.

/// In-place LU factorisation. Returns false if the matrix is (numerically) singular.
pub fn lu_factor(a: &mut [f64], n: usize, piv: &mut [usize]) -> bool {
    // Rows and columns can differ in scale by many orders of magnitude (pressures in Pa next to
    // positions in m), so singularity is judged per column against that column's own magnitude
    // rather than against the largest entry of the whole matrix.
    let mut colmax = vec![0.0f64; n];
    for i in 0..n {
        for (j, c) in colmax.iter_mut().enumerate() {
            *c = c.max(a[i * n + j].abs());
        }
    }
    if colmax.iter().any(|c| !c.is_finite()) {
        return false;
    }
    for k in 0..n {
        let mut p = k;
        let mut best = a[k * n + k].abs();
        for i in k + 1..n {
            let v = a[i * n + k].abs();
            if v > best {
                best = v;
                p = i;
            }
        }
        piv[k] = p;
        if !(best > colmax[k] * 1e-14 + 1e-300) {
            return false;
        }
        if p != k {
            for j in 0..n {
                a.swap(k * n + j, p * n + j);
            }
        }
        let d = 1.0 / a[k * n + k];
        for i in k + 1..n {
            let f = a[i * n + k] * d;
            if f == 0.0 {
                continue;
            }
            a[i * n + k] = f;
            let (top, bot) = a.split_at_mut(i * n);
            let rk = &top[k * n + k + 1..k * n + n];
            let ri = &mut bot[k + 1..n];
            for (x, y) in ri.iter_mut().zip(rk) {
                *x -= f * y;
            }
        }
    }
    true
}

/// Solve `A x = b` using the factorisation from [`lu_factor`]; `b` is overwritten with `x`.
pub fn lu_solve(a: &[f64], n: usize, piv: &[usize], b: &mut [f64]) {
    for k in 0..n {
        let p = piv[k];
        if p != k {
            b.swap(k, p);
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for j in 0..i {
            s -= a[i * n + j] * b[j];
        }
        b[i] = s;
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for j in i + 1..n {
            s -= a[i * n + j] * b[j];
        }
        b[i] = s / a[i * n + i];
    }
}

/// Minimum-norm least-squares solution of `A X = B` (LAPACK `dgelsy`): `A` is row-major
/// `m x n`, `B` row-major `m x nrhs`. The rank is estimated from a QR factorisation with
/// column pivoting, counting the diagonal entries of `R` above `rcond * |R11|`.
/// Returns `X` (row-major `n x nrhs`) and the rank.
pub fn lstsq(
    a: &[f64],
    m: usize,
    n: usize,
    b: &[f64],
    nrhs: usize,
    rcond: f64,
) -> (Vec<f64>, usize) {
    let mut r = a.to_vec();
    let mut c = b.to_vec();
    let mut perm: Vec<usize> = (0..n).collect();
    let mut norms: Vec<f64> = (0..n)
        .map(|j| (0..m).map(|i| r[i * n + j].powi(2)).sum::<f64>())
        .collect();
    let kmax = m.min(n);
    for k in 0..kmax {
        // Pivot: the remaining column of largest norm.
        let p = (k..n)
            .max_by(|&x, &y| norms[x].total_cmp(&norms[y]))
            .unwrap_or(k);
        if p != k {
            for i in 0..m {
                r.swap(i * n + k, i * n + p);
            }
            perm.swap(k, p);
            norms.swap(k, p);
        }
        // Householder reflection zeroing column k below the diagonal.
        let alpha = (k..m).map(|i| r[i * n + k].powi(2)).sum::<f64>().sqrt();
        if alpha == 0.0 {
            continue;
        }
        let beta = if r[k * n + k] > 0.0 { -alpha } else { alpha };
        let mut v: Vec<f64> = (k..m).map(|i| r[i * n + k]).collect();
        v[0] -= beta;
        let vv: f64 = v.iter().map(|x| x * x).sum();
        if vv == 0.0 {
            continue;
        }
        for j in k..n {
            let s: f64 = (k..m).map(|i| v[i - k] * r[i * n + j]).sum::<f64>() * 2.0 / vv;
            for i in k..m {
                r[i * n + j] -= s * v[i - k];
            }
        }
        for j in 0..nrhs {
            let s: f64 = (k..m).map(|i| v[i - k] * c[i * nrhs + j]).sum::<f64>() * 2.0 / vv;
            for i in k..m {
                c[i * nrhs + j] -= s * v[i - k];
            }
        }
        for j in k + 1..n {
            norms[j] = (k + 1..m).map(|i| r[i * n + j].powi(2)).sum();
        }
    }
    let r11 = if kmax > 0 { r[0].abs() } else { 0.0 };
    let rank = if r11 == 0.0 {
        0
    } else {
        (0..kmax)
            .take_while(|&k| r[k * n + k].abs() > rcond.max(0.0) * r11)
            .count()
    };
    let mut y = vec![0.0; n * nrhs];
    if rank == n {
        for j in 0..nrhs {
            for k in (0..n).rev() {
                let mut s = c[k * nrhs + j];
                for l in k + 1..n {
                    s -= r[k * n + l] * y[l * nrhs + j];
                }
                y[k * nrhs + j] = s / r[k * n + k];
            }
        }
    } else if rank > 0 {
        // Minimum norm solution of W y = c with W = R[0..rank, 0..n]: y = W' (W W')^-1 c.
        let w = |i: usize, j: usize| if j >= i { r[i * n + j] } else { 0.0 };
        let mut g = vec![0.0; rank * rank];
        for i in 0..rank {
            for l in 0..rank {
                g[i * rank + l] = (0..n).map(|j| w(i, j) * w(l, j)).sum();
            }
        }
        let mut piv = vec![0; rank];
        if lu_factor(&mut g, rank, &mut piv) {
            for j in 0..nrhs {
                let mut z: Vec<f64> = (0..rank).map(|i| c[i * nrhs + j]).collect();
                lu_solve(&g, rank, &piv, &mut z);
                for col in 0..n {
                    y[col * nrhs + j] = (0..rank).map(|i| w(i, col) * z[i]).sum();
                }
            }
        }
    }
    let mut x = vec![0.0; n * nrhs];
    for (k, &p) in perm.iter().enumerate() {
        for j in 0..nrhs {
            x[p * nrhs + j] = y[k * nrhs + j];
        }
    }
    (x, rank)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lstsq_line_fit() {
        // Fit y = 1 + 2 t through four exact points, and a rank-deficient system.
        let a = [1.0, 0.0, 1.0, 1.0, 1.0, 2.0, 1.0, 3.0];
        let b = [1.0, 3.0, 5.0, 7.0];
        let (x, rank) = lstsq(&a, 4, 2, &b, 1, 1e-12);
        assert_eq!(rank, 2);
        assert!((x[0] - 1.0).abs() < 1e-12 && (x[1] - 2.0).abs() < 1e-12);
        let (x, rank) = lstsq(&[1.0, 1.0], 1, 2, &[2.0], 1, 1e-12);
        assert_eq!(rank, 1);
        assert!((x[0] - 1.0).abs() < 1e-12 && (x[1] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn solve_3x3() {
        let mut a = vec![2.0, 1.0, 1.0, 4.0, -6.0, 0.0, -2.0, 7.0, 2.0];
        let mut piv = vec![0; 3];
        assert!(lu_factor(&mut a, 3, &mut piv));
        let mut b = vec![5.0, -2.0, 9.0];
        lu_solve(&a, 3, &piv, &mut b);
        for (x, e) in b.iter().zip([1.0, 1.0, 2.0]) {
            assert!((x - e).abs() < 1e-12);
        }
    }
}
