//! Sparse LU factorisation (left-looking Gilbert–Peierls with threshold partial
//! pivoting, after Davis' CSparse) and a minimum-degree fill-reducing ordering.
//! Used for the Newton iteration matrices of large stiff systems.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

/// Compressed sparse column matrix.
#[derive(Clone, Debug, Default)]
pub struct Csc {
    pub n: usize,
    pub colptr: Vec<usize>,
    pub rowidx: Vec<usize>,
    pub vals: Vec<f64>,
}

impl Csc {
    /// Build the structure from per-column sorted row lists.
    pub fn from_columns(n: usize, cols: &[Vec<usize>]) -> Csc {
        let mut colptr = Vec::with_capacity(n + 1);
        let mut rowidx = Vec::new();
        colptr.push(0);
        for c in cols {
            rowidx.extend_from_slice(c);
            colptr.push(rowidx.len());
        }
        let nnz = rowidx.len();
        Csc { n, colptr, rowidx, vals: vec![0.0; nnz] }
    }

    pub fn nnz(&self) -> usize {
        self.rowidx.len()
    }
}

/// Minimum-degree ordering of the symmetric pattern `A + A^T`.
pub fn min_degree(a: &Csc) -> Vec<usize> {
    let n = a.n;
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); n];
    for j in 0..n {
        for &i in &a.rowidx[a.colptr[j]..a.colptr[j + 1]] {
            if i != j {
                adj[i].push(j);
                adj[j].push(i);
            }
        }
    }
    for l in adj.iter_mut() {
        l.sort_unstable();
        l.dedup();
    }
    let mut eliminated = vec![false; n];
    let mut heap: BinaryHeap<Reverse<(usize, usize)>> = (0..n).map(|i| Reverse((adj[i].len(), i))).collect();
    let mut order = Vec::with_capacity(n);
    let mut mark = vec![usize::MAX; n];
    while let Some(Reverse((d, p))) = heap.pop() {
        if eliminated[p] || d != adj[p].len() {
            continue;
        }
        eliminated[p] = true;
        order.push(p);
        let nb: Vec<usize> = std::mem::take(&mut adj[p]).into_iter().filter(|&x| !eliminated[x]).collect();
        // Form the clique of the neighbours (fill-in), remove p.
        for &u in &nb {
            mark[u] = p;
        }
        for &u in &nb {
            let mut l: Vec<usize> = adj[u].iter().copied().filter(|&x| x != p && !eliminated[x]).collect();
            for &x in &l {
                if mark[x] == p {
                    // already adjacent
                }
            }
            let present: rustc_hash::FxHashSet<usize> = l.iter().copied().collect();
            for &v in &nb {
                if v != u && !present.contains(&v) {
                    l.push(v);
                }
            }
            adj[u] = l;
            heap.push(Reverse((adj[u].len(), u)));
        }
    }
    order
}

/// LU factors with row pivoting `P A Q = L U`.
#[derive(Clone, Debug, Default)]
pub struct SparseLu {
    n: usize,
    lp: Vec<usize>,
    li: Vec<usize>,
    lx: Vec<f64>,
    up: Vec<usize>,
    ui: Vec<usize>,
    ux: Vec<f64>,
    pinv: Vec<isize>,
    q: Vec<usize>,
    // work
    x: Vec<f64>,
    xi: Vec<usize>,
    stack: Vec<usize>,
    mark: Vec<bool>,
}

impl SparseLu {
    pub fn new(q: Vec<usize>) -> SparseLu {
        let n = q.len();
        SparseLu {
            n,
            q,
            x: vec![0.0; n],
            xi: vec![0; n],
            stack: vec![0; n],
            mark: vec![false; n],
            pinv: vec![-1; n],
            ..Default::default()
        }
    }

    pub fn fill(&self) -> usize {
        self.li.len() + self.ui.len()
    }

    /// Factorise `a` (structure and values). `tol` in (0,1]: diagonal preference threshold.
    pub fn factor(&mut self, a: &Csc, tol: f64) -> bool {
        let n = self.n;
        self.lp.clear();
        self.li.clear();
        self.lx.clear();
        self.up.clear();
        self.ui.clear();
        self.ux.clear();
        self.pinv.iter_mut().for_each(|p| *p = -1);
        for k in 0..n {
            self.lp.push(self.li.len());
            self.up.push(self.ui.len());
            let col = self.q[k];
            let top = self.spsolve(a, col);
            let mut ipiv: isize = -1;
            let mut amax = -1.0f64;
            for px in top..n {
                let i = self.xi[px];
                if self.pinv[i] < 0 {
                    let v = self.x[i].abs();
                    if v > amax {
                        amax = v;
                        ipiv = i as isize;
                    }
                } else {
                    self.ui.push(self.pinv[i] as usize);
                    self.ux.push(self.x[i]);
                }
            }
            if ipiv < 0 || !(amax > 0.0) || !amax.is_finite() {
                // clear work
                for px in top..n {
                    self.x[self.xi[px]] = 0.0;
                }
                return false;
            }
            if self.pinv[col] < 0 && self.x[col].abs() >= amax * tol {
                ipiv = col as isize;
            }
            let ip = ipiv as usize;
            let pivot = self.x[ip];
            self.ui.push(k);
            self.ux.push(pivot);
            self.pinv[ip] = k as isize;
            self.li.push(ip);
            self.lx.push(1.0);
            for px in top..n {
                let i = self.xi[px];
                if self.pinv[i] < 0 {
                    self.li.push(i);
                    self.lx.push(self.x[i] / pivot);
                }
                self.x[i] = 0.0;
            }
        }
        self.lp.push(self.li.len());
        self.up.push(self.ui.len());
        for p in 0..self.li.len() {
            self.li[p] = self.pinv[self.li[p]] as usize;
        }
        true
    }

    /// x = L \ A(:,col) during factorisation; returns `top` (nonzero pattern in xi[top..n]).
    fn spsolve(&mut self, a: &Csc, col: usize) -> usize {
        let n = self.n;
        let mut top = n;
        for p in a.colptr[col]..a.colptr[col + 1] {
            let i = a.rowidx[p];
            if !self.mark[i] {
                top = self.dfs(i, top);
            }
        }
        for px in top..n {
            self.mark[self.xi[px]] = false;
        }
        for p in a.colptr[col]..a.colptr[col + 1] {
            self.x[a.rowidx[p]] = a.vals[p];
        }
        for px in top..n {
            let j = self.xi[px];
            let jj = self.pinv[j];
            if jj < 0 {
                continue;
            }
            let jj = jj as usize;
            let xj = self.x[j]; // unit diagonal
            if xj == 0.0 {
                continue;
            }
            for p in self.lp[jj] + 1..self.lp[jj + 1] {
                self.x[self.li[p]] -= self.lx[p] * xj;
            }
        }
        top
    }

    fn dfs(&mut self, j0: usize, mut top: usize) -> usize {
        // `stack` holds node ids, `pos` the next edge index for each stacked node.
        let mut head: isize = 0;
        self.stack[0] = j0;
        let mut pos: Vec<usize> = Vec::new();
        pos.push(0);
        while head >= 0 {
            let h = head as usize;
            let j = self.stack[h];
            let jnew = self.pinv[j];
            if !self.mark[j] {
                self.mark[j] = true;
                pos[h] = if jnew < 0 { 0 } else { self.lp[jnew as usize] + 1 };
            }
            let mut done = true;
            let end = if jnew < 0 { 0 } else { self.lp[jnew as usize + 1] };
            let mut p = pos[h];
            while p < end {
                let i = self.li[p];
                if self.mark[i] {
                    p += 1;
                    continue;
                }
                pos[h] = p + 1;
                head += 1;
                let hh = head as usize;
                self.stack[hh] = i;
                if pos.len() <= hh {
                    pos.push(0);
                } else {
                    pos[hh] = 0;
                }
                done = false;
                break;
            }
            if done {
                pos[h] = p;
                head -= 1;
                top -= 1;
                self.xi[top] = j;
            }
        }
        top
    }

    /// Solve `A x = b` in place.
    pub fn solve(&mut self, b: &mut [f64]) {
        let n = self.n;
        let x = &mut self.x;
        for i in 0..n {
            x[self.pinv[i] as usize] = b[i];
        }
        for j in 0..n {
            let xj = x[j];
            if xj != 0.0 {
                for p in self.lp[j] + 1..self.lp[j + 1] {
                    x[self.li[p]] -= self.lx[p] * xj;
                }
            }
        }
        for j in (0..n).rev() {
            let d = self.up[j + 1] - 1;
            x[j] /= self.ux[d];
            let xj = x[j];
            if xj != 0.0 {
                for p in self.up[j]..d {
                    x[self.ui[p]] -= self.ux[p] * xj;
                }
            }
        }
        for k in 0..n {
            b[self.q[k]] = x[k];
        }
        x.iter_mut().for_each(|v| *v = 0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sparse_lu_matches_dense() {
        // Random-ish sparse 30x30 with dominant diagonal plus a few off-diagonals.
        let n = 30;
        let mut cols: Vec<Vec<usize>> = vec![Vec::new(); n];
        let mut dense = vec![0.0; n * n];
        let mut seed = 12345u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            ((seed >> 33) as f64) / (1u64 << 31) as f64
        };
        for j in 0..n {
            for i in 0..n {
                if i == j || rnd() < 0.1 {
                    cols[j].push(i);
                }
            }
        }
        let mut a = Csc::from_columns(n, &cols);
        for j in 0..n {
            for p in a.colptr[j]..a.colptr[j + 1] {
                let i = a.rowidx[p];
                let v = if i == j { 0.1 + rnd() } else { rnd() - 0.5 };
                a.vals[p] = v;
                dense[i * n + j] = v;
            }
        }
        let q = min_degree(&a);
        let mut lu = SparseLu::new(q);
        assert!(lu.factor(&a, 0.1));
        let xs: Vec<f64> = (0..n).map(|i| i as f64 * 0.1 - 1.0).collect();
        let mut b: Vec<f64> = (0..n).map(|i| (0..n).map(|j| dense[i * n + j] * xs[j]).sum()).collect();
        lu.solve(&mut b);
        for i in 0..n {
            assert!((b[i] - xs[i]).abs() < 1e-9, "{i}: {} vs {}", b[i], xs[i]);
        }
    }
}
