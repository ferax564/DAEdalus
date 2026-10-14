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

#[cfg(test)]
mod tests {
    use super::*;

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
