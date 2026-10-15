//! Two-dimensional interpolation tables: the native implementation of the MSL
//! `CombiTable2Ds` / `CombiTable2Dv` external table object.
//!
//! The interpolation follows `ModelicaStandardTables.c` of the Modelica Standard
//! Library (BSD-3-Clause, Copyright (C) 2013-2024 Modelica Association and
//! contributors, Thomas Beutlich): bilinear, constant and bivariate Akima
//! interpolation (Akima 1974, with the same boundary extension of the data), and
//! hold / linear / periodic extrapolation. Instead of the case-by-case evaluation of
//! the C code, every grid cell carries one polynomial `P(v1, v2) = sum a_ij v1^i v2^j`
//! in the local coordinates `v = u - u_left`; extrapolation evaluates `P` at the
//! table boundary (hold) or continues it linearly in each extrapolated direction
//! (last two points), which is what the C code does in each of its cases. Partial
//! derivatives of the interpolant, up to second order, are exact derivatives of the
//! same piecewise function.

/// Which value [`Table2::eval`] returns: the interpolant or one of its partial
/// derivatives with respect to `u1` and `u2` (bits 0-2), and on which side of a grid
/// line a derivative is taken (bits 3-4).
pub mod kind {
    pub const VALUE: u32 = 0;
    pub const D1: u32 = 1;
    pub const D2: u32 = 2;
    pub const D11: u32 = 3;
    pub const D12: u32 = 4;
    pub const D22: u32 = 5;
    /// On a grid line of `u1` (`u2`), take the cell left of it: the abscissa moves
    /// to smaller values, so the one-sided derivative in that direction is wanted
    /// (as `getDerValue` in C chooses the cell by the sign of `der_u`).
    pub const LEFT1: u32 = 8;
    pub const LEFT2: u32 = 16;
    /// Bits of a kind (the arena packs `table << BITS | kind`).
    pub const BITS: u32 = 5;

    /// Derivative orders `(r1, r2)` of a kind.
    pub fn orders(k: u32) -> (u8, u8) {
        match k & 7 {
            D1 => (1, 0),
            D2 => (0, 1),
            D11 => (2, 0),
            D12 => (1, 1),
            D22 => (0, 2),
            _ => (0, 0),
        }
    }

    /// The kind of the partial derivative of kind `k` with respect to `u1` (`dim == 0`)
    /// or `u2` (same sides); `None` beyond second order.
    pub fn partial(k: u32, dim: usize) -> Option<u32> {
        let sides = k & (LEFT1 | LEFT2);
        let p = match (k & 7, dim) {
            (VALUE, 0) => Some(D1),
            (VALUE, _) => Some(D2),
            (D1, 0) => Some(D11),
            (D1, _) | (D2, 0) => Some(D12),
            (D2, _) => Some(D22),
            _ => None,
        };
        p.map(|p| p | sides)
    }

    /// Sides for a derivative along `(du1, du2)`.
    pub fn sides(du1: f64, du2: f64) -> u32 {
        (if du1 < 0.0 { LEFT1 } else { 0 }) | (if du2 < 0.0 { LEFT2 } else { 0 })
    }
}

/// MSL `Modelica.Blocks.Types.Smoothness` values (as passed to the C functions).
pub const LINEAR_SEGMENTS: u8 = 1;
pub const AKIMA: u8 = 2;
pub const CONSTANT_SEGMENTS: u8 = 3;
/// MSL `Modelica.Blocks.Types.Extrapolation` values.
pub const HOLD_LAST_POINT: u8 = 1;
pub const LAST_TWO_POINTS: u8 = 2;
pub const PERIODIC: u8 = 3;
pub const NO_EXTRAPOLATION: u8 = 4;

/// A 2-D table: values `z[i][j]` at `(u1[i], u2[j])` with per-cell polynomials.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Table2 {
    /// Abscissae along the first dimension (the table's first column without its head).
    pub u1: Vec<f64>,
    /// Abscissae along the second dimension (the first row without its head).
    pub u2: Vec<f64>,
    /// `u1.len() x u2.len()` values, row-wise.
    pub z: Vec<f64>,
    pub smoothness: u8,
    pub extrapolation: u8,
    /// Coefficients `a[4*i + j]` of `v1^i v2^j` per cell (`cells1 x cells2`, row-wise).
    pub coef: Vec<[f64; 16]>,
}

/// Where an abscissa lies relative to one axis of the table.
struct Axis {
    /// Cell (interval) index.
    cell: usize,
    /// Local coordinate inside the cell (the boundary when extrapolating).
    v: f64,
    /// Distance beyond the boundary when extrapolating linearly, else 0.
    delta: f64,
    /// Outside the table (and not periodic).
    out: bool,
    /// The axis has a single point (a 2-D table degraded to 1-D).
    absent: bool,
    /// Index of the grid point whose value holds for constant segments.
    point: usize,
}

impl Table2 {
    /// Build a table from the MSL matrix layout (`rows x cols`, row-wise): `data[0][0]`
    /// unused, first column `u1`, first row `u2`.
    pub fn new(
        rows: usize,
        cols: usize,
        data: &[f64],
        smoothness: u8,
        extrapolation: u8,
    ) -> Result<Table2, String> {
        if rows < 2 || cols < 2 || data.len() < rows * cols {
            return Err(format!(
                "table matrix ({rows},{cols}) does not have appropriate dimensions for 2D-interpolation"
            ));
        }
        let tab = |i: usize, j: usize| data[i * cols + j];
        let u1: Vec<f64> = (1..rows).map(|i| tab(i, 0)).collect();
        let u2: Vec<f64> = (1..cols).map(|j| tab(0, j)).collect();
        if let Some(i) = (1..u1.len()).find(|&i| u1[i - 1] >= u1[i]) {
            return Err(format!(
                "the values of the first column of the table are not strictly increasing \
                 because table[{},1] (={}) >= table[{},1] (={})",
                i + 1,
                u1[i - 1],
                i + 2,
                u1[i]
            ));
        }
        if let Some(j) = (1..u2.len()).find(|&j| u2[j - 1] >= u2[j]) {
            return Err(format!(
                "the values of the first row of the table are not strictly increasing \
                 because table[1,{}] (={}) >= table[1,{}] (={})",
                j + 1,
                u2[j - 1],
                j + 2,
                u2[j]
            ));
        }
        match smoothness {
            LINEAR_SEGMENTS | AKIMA | CONSTANT_SEGMENTS => {}
            4 | 5 => return Err("bivariate monotone C1 interpolation is not implemented".into()),
            6 => return Err("bivariate modified Akima interpolation is not implemented".into()),
            s => return Err(format!("unknown smoothness kind {s}")),
        }
        if !(1..=4).contains(&extrapolation) {
            return Err(format!("unknown extrapolation kind {extrapolation}"));
        }
        let z: Vec<f64> = (1..rows)
            .flat_map(|i| (1..cols).map(move |j| (i, j)))
            .map(|(i, j)| tab(i, j))
            .collect();
        // Akima on a table with at most 2x2 points is bilinear (as in the C code).
        let smoothness = if smoothness == AKIMA && rows <= 3 && cols <= 3 {
            LINEAR_SEGMENTS
        } else {
            smoothness
        };
        let mut t = Table2 {
            u1,
            u2,
            z,
            smoothness,
            extrapolation,
            coef: Vec::new(),
        };
        t.coef = if smoothness == AKIMA {
            akima_coefficients(rows, cols, data)
        } else {
            t.bilinear_coefficients()
        };
        Ok(t)
    }

    fn cells(&self) -> (usize, usize) {
        (self.u1.len().max(2) - 1, self.u2.len().max(2) - 1)
    }

    fn bilinear_coefficients(&self) -> Vec<[f64; 16]> {
        let (n1, n2) = (self.u1.len(), self.u2.len());
        let (c1, c2) = self.cells();
        let z = |i: usize, j: usize| self.z[i * n2 + j];
        let mut out = Vec::with_capacity(c1 * c2);
        for i in 0..c1 {
            for j in 0..c2 {
                let mut a = [0.0; 16];
                let z00 = z(i, j);
                a[0] = z00;
                if n1 > 1 {
                    a[4] = (z(i + 1, j) - z00) / (self.u1[i + 1] - self.u1[i]);
                }
                if n2 > 1 {
                    a[1] = (z(i, j + 1) - z00) / (self.u2[j + 1] - self.u2[j]);
                }
                if n1 > 1 && n2 > 1 {
                    a[5] = (z(i + 1, j + 1) - z(i + 1, j) - z(i, j + 1) + z00)
                        / ((self.u1[i + 1] - self.u1[i]) * (self.u2[j + 1] - self.u2[j]));
                }
                out.push(a);
            }
        }
        out
    }

    fn axis(&self, grid: &[f64], u: f64, left: bool, deriv: bool) -> Axis {
        let n = grid.len();
        if n < 2 {
            return Axis {
                cell: 0,
                v: 0.0,
                delta: 0.0,
                out: false,
                absent: true,
                point: 0,
            };
        }
        let (lo, hi) = (grid[0], grid[n - 1]);
        let mut u = u;
        if self.extrapolation == PERIODIC {
            let p = hi - lo;
            if u < lo {
                u += ((lo - u) / p).ceil() * p;
                if u < lo {
                    u += p;
                }
            } else if u > hi {
                u -= ((u - hi) / p).ceil() * p;
                if u > hi {
                    u -= p;
                }
            }
        } else if u < lo || u > hi || (u == lo && left) || (u == hi && deriv && !left) {
            let right = u > hi || (u == hi && !left);
            let linear = self.extrapolation == LAST_TWO_POINTS;
            let cell = if right { n - 2 } else { 0 };
            return Axis {
                cell,
                v: if right { hi - grid[n - 2] } else { 0.0 },
                delta: if linear {
                    u - if right { hi } else { lo }
                } else {
                    0.0
                },
                out: true,
                absent: false,
                point: if right { n - 1 } else { 0 },
            };
        }
        // the last i <= n-2 with grid[i] <= u (grid[i] < u on the left side)
        let (mut a, mut b) = (0usize, n - 1);
        while b - a > 1 {
            let m = (a + b) / 2;
            if u < grid[m] || (left && u == grid[m]) {
                b = m;
            } else {
                a = m;
            }
        }
        Axis {
            cell: a,
            v: u - grid[a],
            delta: 0.0,
            out: false,
            absent: false,
            point: if u >= grid[a + 1] { a + 1 } else { a },
        }
    }

    /// Values (or derivatives of order `r`) of the monomials `1, v, v^2, v^3` as
    /// functions of the abscissa.
    fn basis(&self, ax: &Axis, r: u8) -> [f64; 4] {
        if ax.absent {
            return [if r == 0 { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
        }
        let v = ax.v;
        let linear = self.extrapolation == LAST_TWO_POINTS;
        if ax.out && !linear {
            // held at the boundary: independent of the abscissa
            return if r == 0 {
                [1.0, v, v * v, v * v * v]
            } else {
                [0.0; 4]
            };
        }
        if ax.out {
            // first-order continuation from the boundary
            let d = ax.delta;
            return match r {
                0 => [1.0, v + d, v * v + 2.0 * v * d, v * v * v + 3.0 * v * v * d],
                1 => [0.0, 1.0, 2.0 * v, 3.0 * v * v],
                _ => [0.0; 4],
            };
        }
        match r {
            0 => [1.0, v, v * v, v * v * v],
            1 => [0.0, 1.0, 2.0 * v, 3.0 * v * v],
            2 => [0.0, 0.0, 2.0, 6.0 * v],
            _ => [0.0; 4],
        }
    }

    /// The interpolant at `(u1, u2)`, or the partial derivative given by `kind`.
    pub fn eval(&self, kind: u32, u1: f64, u2: f64) -> f64 {
        let (r1, r2) = kind::orders(kind);
        let a1 = self.axis(&self.u1, u1, kind & kind::LEFT1 != 0, r1 + r2 > 0);
        let a2 = self.axis(&self.u2, u2, kind & kind::LEFT2 != 0, r1 + r2 > 0);
        let linear_out = self.extrapolation == LAST_TWO_POINTS && (a1.out || a2.out);
        if self.smoothness == CONSTANT_SEGMENTS && !linear_out {
            // Constant segments; extrapolating with the last two points is bilinear.
            return if r1 + r2 == 0 {
                self.z[a1.point * self.u2.len() + a2.point]
            } else {
                0.0
            };
        }
        let m1 = self.basis(&a1, r1);
        let m2 = self.basis(&a2, r2);
        let (_, c2) = self.cells();
        let a = &self.coef[a1.cell * c2 + a2.cell];
        let mut s = 0.0;
        for i in 0..4 {
            if m1[i] == 0.0 {
                continue;
            }
            let row = a[4 * i] * m2[0]
                + a[4 * i + 1] * m2[1]
                + a[4 * i + 2] * m2[2]
                + a[4 * i + 3] * m2[3];
            s += m1[i] * row;
        }
        s
    }

    pub fn min_abscissa(&self) -> [f64; 2] {
        [self.u1[0], self.u2[0]]
    }

    pub fn max_abscissa(&self) -> [f64; 2] {
        [self.u1[self.u1.len() - 1], self.u2[self.u2.len() - 1]]
    }
}

/// Univariate Akima spline (Akima 1970) through `(x, y)` with the MSL boundary
/// conditions: per interval `[c0, c1, c2]` of `c0 v^3 + c1 v^2 + c2 v + y_i`.
fn akima_1d(x: &[f64], y: &[f64]) -> Vec<[f64; 3]> {
    let n = x.len();
    let mut d = vec![0.0; n + 3];
    for i in 0..n - 1 {
        d[i + 2] = (y[i + 1] - y[i]) / (x[i + 1] - x[i]);
    }
    d[0] = 3.0 * d[2] - 2.0 * d[3];
    d[1] = 2.0 * d[2] - d[3];
    d[n + 1] = 2.0 * d[n] - d[n - 1];
    d[n + 2] = 3.0 * d[n] - 2.0 * d[n - 1];
    let slope = |m1: f64, m2: f64, m3: f64, m4: f64| {
        let c = (m4 - m3).abs() + (m2 - m1).abs();
        if c > 0.0 {
            let a = (m2 - m1).abs() / c;
            (1.0 - a) * m2 + a * m3
        } else {
            0.5 * m2 + 0.5 * m3
        }
    };
    let mut c2 = slope(d[0], d[1], d[2], d[3]);
    let mut out = Vec::with_capacity(n - 1);
    for i in 0..n - 1 {
        let dx = x[i + 1] - x[i];
        let s0 = c2;
        c2 = slope(d[i + 1], d[i + 2], d[i + 3], d[i + 4]);
        out.push([
            (s0 + c2 - 2.0 * d[i + 2]) / (dx * dx),
            (3.0 * d[i + 2] - 2.0 * s0 - c2) / dx,
            s0,
        ]);
    }
    out
}

fn extrapolate_left(x: [f64; 5], y: [f64; 3]) -> (f64, f64) {
    // x1..x5, y3..y5 -> (y1, y2)
    let (a, b, c) = (x[3] - x[4], x[2] - x[3], x[1] - x[2]);
    if a == 0.0 || b == 0.0 || c == 0.0 {
        return (y[0], y[0]);
    }
    let d = x[0] - x[1];
    let (e, f) = (y[1] - y[2], y[0] - y[1]);
    let y2 = c * (2.0 * f / b - e / a) + y[0];
    let y1 = y2 + d * ((y2 - y[0]) / c + f / b - e / a);
    (y1, y2)
}

fn extrapolate_right(x: [f64; 5], y: [f64; 3]) -> (f64, f64) {
    // x1..x5, y1..y3 -> (y4, y5)
    let (a, b, c) = (x[1] - x[0], x[2] - x[1], x[3] - x[2]);
    if a == 0.0 || b == 0.0 || c == 0.0 {
        return (y[2], y[2]);
    }
    let d = x[4] - x[3];
    let (e, f) = (y[1] - y[0], y[2] - y[1]);
    let y4 = c * (2.0 * f / b - e / a) + y[2];
    let y5 = y4 + d * ((y4 - y[2]) / c + f / b - e / a);
    (y4, y5)
}

/// Grid coordinates extended by two points on each side (`spline2DInit`).
fn extended_grid(g: &[f64]) -> Vec<f64> {
    let n = g.len(); // number of grid points (>= 2)
    let mut x = vec![0.0; n + 4];
    if n == 2 {
        x[0] = 3.0 * g[0] - 2.0 * g[1];
        x[1] = 2.0 * g[0] - g[1];
        x[2] = g[0];
        x[3] = g[1];
        x[4] = 2.0 * g[1] - g[0];
        x[5] = 3.0 * g[1] - 2.0 * g[0];
    } else {
        x[0] = 2.0 * g[0] - g[2];
        x[1] = g[0] + g[1] - g[2];
        x[2..n + 2].copy_from_slice(g);
        x[n + 2] = g[n - 1] + g[n - 2] - g[n - 3];
        x[n + 3] = 2.0 * g[n - 1] - g[n - 3];
    }
    x
}

/// Per-cell coefficients of the bivariate Akima interpolant (`spline2DInit`),
/// or of the univariate one along the remaining axis of a degraded table.
fn akima_coefficients(rows: usize, cols: usize, data: &[f64]) -> Vec<[f64; 16]> {
    let tab = |i: usize, j: usize| data[i * cols + j];
    if rows == 2 {
        let x: Vec<f64> = (1..cols).map(|j| tab(0, j)).collect();
        let y: Vec<f64> = (1..cols).map(|j| tab(1, j)).collect();
        return akima_1d(&x, &y)
            .into_iter()
            .zip(1..cols)
            .map(|(c, j)| {
                let mut a = [0.0; 16];
                a[0] = tab(1, j);
                a[1] = c[2];
                a[2] = c[1];
                a[3] = c[0];
                a
            })
            .collect();
    }
    if cols == 2 {
        let x: Vec<f64> = (1..rows).map(|i| tab(i, 0)).collect();
        let y: Vec<f64> = (1..rows).map(|i| tab(i, 1)).collect();
        return akima_1d(&x, &y)
            .into_iter()
            .zip(1..rows)
            .map(|(c, i)| {
                let mut a = [0.0; 16];
                a[0] = tab(i, 1);
                a[4] = c[2];
                a[8] = c[1];
                a[12] = c[0];
                a
            })
            .collect();
    }
    let n1 = rows - 1; // grid points along u1
    let n2 = cols - 1;
    let x = extended_grid(&(1..rows).map(|i| tab(i, 0)).collect::<Vec<_>>());
    let y = extended_grid(&(1..cols).map(|j| tab(0, j)).collect::<Vec<_>>());
    // table extended by two rows / columns on each side: ex[(i, j)], i in 0..n1+4
    let w = n2 + 4;
    let mut ex = vec![0.0; (n1 + 4) * w];
    for i in 0..n1 {
        for j in 0..n2 {
            ex[(i + 2) * w + j + 2] = tab(i + 1, j + 1);
        }
    }
    for i in 2..n1 + 2 {
        let r = i * w;
        if n2 == 2 {
            let (t1, t2) = (ex[r + 2], ex[r + 3]);
            ex[r] = 3.0 * t1 - 2.0 * t2;
            ex[r + 1] = 2.0 * t1 - t2;
            ex[r + 4] = 2.0 * t2 - t1;
            ex[r + 5] = 3.0 * t2 - 2.0 * t1;
        } else {
            let (y1, y2) = extrapolate_left(
                [y[0], y[1], y[2], y[3], y[4]],
                [ex[r + 2], ex[r + 3], ex[r + 4]],
            );
            ex[r] = y1;
            ex[r + 1] = y2;
            let k = n2 - 1; // C: y[nCol-2..nCol+2] with nCol = n2 + 1
            let (y4, y5) = extrapolate_right(
                [y[k], y[k + 1], y[k + 2], y[k + 3], y[k + 4]],
                [ex[r + k], ex[r + k + 1], ex[r + k + 2]],
            );
            ex[r + k + 3] = y4;
            ex[r + k + 4] = y5;
        }
    }
    for j in 0..w {
        let at = |i: usize| i * w + j;
        if n1 == 2 {
            let (t1, t2) = (ex[at(2)], ex[at(3)]);
            ex[at(0)] = 3.0 * t1 - 2.0 * t2;
            ex[at(1)] = 2.0 * t1 - t2;
            ex[at(4)] = 2.0 * t2 - t1;
            ex[at(5)] = 3.0 * t2 - 2.0 * t1;
        } else {
            let (y1, y2) = extrapolate_left(
                [x[0], x[1], x[2], x[3], x[4]],
                [ex[at(2)], ex[at(3)], ex[at(4)]],
            );
            ex[at(0)] = y1;
            ex[at(1)] = y2;
            let k = n1 - 1;
            let (y4, y5) = extrapolate_right(
                [x[k], x[k + 1], x[k + 2], x[k + 3], x[k + 4]],
                [ex[at(k)], ex[at(k + 1)], ex[at(k + 2)]],
            );
            ex[at(k + 3)] = y4;
            ex[at(k + 4)] = y5;
        }
    }
    let e = |i: usize, j: usize| ex[i * w + j];
    // partial derivatives at the grid points
    let mut zx = vec![0.0; n1 * n2];
    let mut zy = vec![0.0; n1 * n2];
    let mut zxy = vec![0.0; n1 * n2];
    for i in 2..n1 + 2 {
        for j in 2..n2 + 2 {
            let k = (i - 2) * n2 + (j - 2);
            let d31 = (e(i - 1, j) - e(i - 2, j)) / (x[i - 1] - x[i - 2]);
            let d32 = (e(i, j) - e(i - 1, j)) / (x[i] - x[i - 1]);
            let d33 = (e(i + 1, j) - e(i, j)) / (x[i + 1] - x[i]);
            let d34 = (e(i + 2, j) - e(i + 1, j)) / (x[i + 2] - x[i + 1]);
            let (mut wx2, mut wx3);
            if d31 == d32 && d33 == d34 {
                wx2 = 0.0;
                wx3 = 0.0;
                zx[k] = 0.5 * d32 + 0.5 * d33;
            } else {
                wx2 = (d34 - d33).abs();
                wx3 = (d32 - d31).abs();
                zx[k] = (wx2 * d32 + wx3 * d33) / (wx2 + wx3);
            }
            let d31 = (e(i, j - 1) - e(i, j - 2)) / (y[j - 1] - y[j - 2]);
            let d32 = (e(i, j) - e(i, j - 1)) / (y[j] - y[j - 1]);
            let d33 = (e(i, j + 1) - e(i, j)) / (y[j + 1] - y[j]);
            let d34 = (e(i, j + 2) - e(i, j + 1)) / (y[j + 2] - y[j + 1]);
            let (mut wy2, mut wy3);
            if d31 == d32 && d33 == d34 {
                wy2 = 0.0;
                wy3 = 0.0;
                zy[k] = 0.5 * d32 + 0.5 * d33;
            } else {
                wy2 = (d34 - d33).abs();
                wy3 = (d32 - d31).abs();
                zy[k] = (wy2 * d32 + wy3 * d33) / (wy2 + wy3);
            }
            let d22 = (e(i - 1, j) - e(i - 1, j - 1)) / (y[j] - y[j - 1]);
            let d23 = (e(i - 1, j + 1) - e(i - 1, j)) / (y[j + 1] - y[j]);
            let d42 = (e(i + 1, j) - e(i + 1, j - 1)) / (y[j] - y[j - 1]);
            let d43 = (e(i + 1, j + 1) - e(i + 1, j)) / (y[j + 1] - y[j]);
            let e22 = (d32 - d22) / (x[i] - x[i - 1]);
            let e23 = (d33 - d23) / (x[i] - x[i - 1]);
            let e32 = (d42 - d32) / (x[i + 1] - x[i]);
            let e33 = (d43 - d33) / (x[i + 1] - x[i]);
            if wx2 == 0.0 && wx3 == 0.0 {
                wx2 = 1.0;
                wx3 = 1.0;
            }
            if wy2 == 0.0 && wy3 == 0.0 {
                wy2 = 1.0;
                wy3 = 1.0;
            }
            zxy[k] = (wx2 * (wy2 * e22 + wy3 * e23) + wx3 * (wy2 * e32 + wy3 * e33))
                / ((wx2 + wx3) * (wy2 + wy3));
        }
    }
    // bicubic Hermite coefficients per cell
    let mut out = Vec::with_capacity((n1 - 1) * (n2 - 1));
    let p = |v: &[f64], i: usize, j: usize| v[i * n2 + j];
    for i in 0..n1 - 1 {
        let dx = tab(i + 2, 0) - tab(i + 1, 0);
        let (dx2, dx3) = (dx * dx, dx * dx * dx);
        for j in 0..n2 - 1 {
            let z00 = tab(i + 1, j + 1);
            let z01 = tab(i + 1, j + 2);
            let z10 = tab(i + 2, j + 1);
            let z11 = tab(i + 2, j + 2);
            let dy = tab(0, j + 2) - tab(0, j + 1);
            let (dy2, dy3) = (dy * dy, dy * dy * dy);
            let mut c = [0.0; 16];
            c[11] = p(&zx, i, j);
            let zx00 = c[11] * dx;
            let zx01 = p(&zx, i, j + 1) * dx;
            let zx10 = p(&zx, i + 1, j) * dx;
            let zx11 = p(&zx, i + 1, j + 1) * dx;
            c[14] = p(&zy, i, j);
            let zy00 = c[14] * dy;
            let zy01 = p(&zy, i, j + 1) * dy;
            let zy10 = p(&zy, i + 1, j) * dy;
            let zy11 = p(&zy, i + 1, j + 1) * dy;
            c[10] = p(&zxy, i, j);
            let zxy00 = c[10] * dx * dy;
            let zxy01 = p(&zxy, i, j + 1) * dx * dy;
            let zxy10 = p(&zxy, i + 1, j) * dx * dy;
            let zxy11 = p(&zxy, i + 1, j + 1) * dx * dy;
            let t1 = z00 - z10;
            let t2 = zx00 + zx10;
            let t3 = zy00 - zy10;
            let t4 = zy11 - zy01;
            let t5 = zxy00 + zxy10;
            let t6 = zxy11 + zxy01;
            let t7 = 2.0 * zx00 + zx10;
            let t8 = 2.0 * zxy00 + zxy10;
            let t9 = zxy11 + 2.0 * zxy01;
            let t10 = zx00 - zx01;
            let t11 = z00 - z01;
            let t12 = t1 + (z11 - z01);
            let t13 = t3 - t4;
            let t4 = 2.0 * t3 - t4;
            let t14 = 2.0 * t12 + (t2 - (zx11 + zx01));
            let t12 = 3.0 * t12 + (t7 - (zx11 + 2.0 * zx01));
            c[0] = (2.0 * t14 + (2.0 * t13 + (t5 + t6))) / (dx3 * dy3);
            c[1] = -(3.0 * t14 + (2.0 * t4 + (2.0 * t5 + t6))) / (dx3 * dy2);
            c[2] = (2.0 * t3 + t5) / (dx3 * dy);
            c[3] = (2.0 * t1 + t2) / dx3;
            c[4] = -(2.0 * t12 + (3.0 * t13 + (t8 + t9))) / (dx2 * dy3);
            c[5] = (3.0 * t12 + (3.0 * t4 + (2.0 * t8 + t9))) / (dx2 * dy2);
            c[6] = -(3.0 * t3 + t8) / (dx2 * dy);
            c[7] = -(3.0 * t1 + t7) / dx2;
            c[8] = (2.0 * t10 + (zxy00 + zxy01)) / (dx * dy3);
            c[9] = -(3.0 * t10 + (2.0 * zxy00 + zxy01)) / (dx * dy2);
            c[12] = (2.0 * t11 + (zy00 + zy01)) / dy3;
            c[13] = -(3.0 * t11 + (2.0 * zy00 + zy01)) / dy2;
            c[15] = z00;
            // C layout c[(3 - i) * 4 + (3 - j)] holds the coefficient of v1^i v2^j
            let mut a = [0.0; 16];
            for ii in 0..4 {
                for jj in 0..4 {
                    a[4 * ii + jj] = c[(3 - ii) * 4 + (3 - jj)];
                }
            }
            out.push(a);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num_d(t: &Table2, k: u32, u1: f64, u2: f64, dim: usize) -> f64 {
        let h = 1e-6;
        let (a, b) = if dim == 0 {
            (t.eval(k, u1 + h, u2), t.eval(k, u1 - h, u2))
        } else {
            (t.eval(k, u1, u2 + h), t.eval(k, u1, u2 - h))
        };
        (a - b) / (2.0 * h)
    }

    const AKIMA_TABLE: [f64; 20] = [
        0.0, 1.0, 2.0, 4.0, //
        0.0, 1.0, 3.0, 2.0, //
        1.0, 2.0, 2.5, 0.0, //
        3.0, 0.5, 1.0, 4.0, //
        4.0, 0.0, 2.0, 1.0,
    ];

    #[test]
    fn bilinear_values_and_extrapolation() {
        let d = [0.0, 1.0, 2.0, 1.0, 1.0, 3.0, 2.0, 5.0, 7.0];
        for ex in [HOLD_LAST_POINT, LAST_TWO_POINTS] {
            let t = Table2::new(3, 3, &d, LINEAR_SEGMENTS, ex).unwrap();
            assert_eq!(t.eval(0, 1.0, 1.0), 1.0);
            assert_eq!(t.eval(0, 2.0, 2.0), 7.0);
            assert!((t.eval(0, 1.5, 1.5) - 4.0).abs() < 1e-12);
            assert!((t.eval(kind::D1, 1.5, 1.5) - 4.0).abs() < 1e-12);
            assert!((t.eval(kind::D12, 1.5, 1.5) - 0.0).abs() < 1e-12);
        }
        let t = Table2::new(3, 3, &d, LINEAR_SEGMENTS, LAST_TWO_POINTS).unwrap();
        // continued plane z = 1 + 4 (u1 - 1) + 2 (u2 - 1)
        assert!((t.eval(0, 3.0, 0.0) - 7.0).abs() < 1e-12);
        let t = Table2::new(3, 3, &d, LINEAR_SEGMENTS, HOLD_LAST_POINT).unwrap();
        assert!((t.eval(0, 3.0, 0.0) - 5.0).abs() < 1e-12);
        assert_eq!(t.eval(kind::D1, 3.0, 1.5), 0.0);
        assert!((t.eval(kind::D2, 3.0, 1.5) - 2.0).abs() < 1e-12);
        let t = Table2::new(3, 3, &d, CONSTANT_SEGMENTS, HOLD_LAST_POINT).unwrap();
        assert_eq!(t.eval(0, 1.5, 1.99), 1.0);
        assert_eq!(t.eval(0, 2.0, 1.5), 5.0);
        assert_eq!(t.eval(0, 9.0, 9.0), 7.0);
        let t = Table2::new(3, 3, &d, LINEAR_SEGMENTS, PERIODIC).unwrap();
        assert!((t.eval(0, 2.5, 1.5) - t.eval(0, 1.5, 1.5)).abs() < 1e-12);
    }

    #[test]
    fn akima_interpolates_and_derivatives_are_consistent() {
        for ex in [HOLD_LAST_POINT, LAST_TWO_POINTS] {
            let t = Table2::new(5, 4, &AKIMA_TABLE, AKIMA, ex).unwrap();
            for i in 0..4 {
                for j in 0..3 {
                    let v = t.eval(0, AKIMA_TABLE[(i + 1) * 4], AKIMA_TABLE[j + 1]);
                    assert!((v - AKIMA_TABLE[(i + 1) * 4 + j + 1]).abs() < 1e-12);
                }
            }
            for &(u1, u2) in &[
                (0.3, 1.2),
                (2.2, 3.1),
                (-1.0, 2.5),
                (5.0, 0.5),
                (6.0, 7.0),
                (-1.0, -2.0),
            ] {
                for k in [0, kind::D1, kind::D2] {
                    for dim in 0..2 {
                        let an = t.eval(kind::partial(k, dim).unwrap(), u1, u2);
                        let nu = num_d(&t, k, u1, u2, dim);
                        assert!(
                            (an - nu).abs() < 1e-5 * (1.0 + an.abs()),
                            "{ex} {k} {dim} {u1} {u2}: {an} vs {nu}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn one_sided_derivatives_on_grid_lines() {
        let d = [0.0, 1.0, 2.0, 3.0, 1.0, 0.0, 1.0, 5.0, 2.0, 0.0, 2.0, 2.0];
        let t = Table2::new(3, 4, &d, LINEAR_SEGMENTS, LAST_TWO_POINTS).unwrap();
        // at u2 = 2 the slope in u2 is 1 to the left and 4 to the right
        assert_eq!(t.eval(kind::D2, 1.0, 2.0), 4.0);
        assert_eq!(t.eval(kind::D2 | kind::LEFT2, 1.0, 2.0), 1.0);
        assert_eq!(
            kind::partial(kind::D1 | kind::LEFT2, 1),
            Some(kind::D12 | kind::LEFT2)
        );
    }

    #[test]
    fn akima_degraded_tables() {
        // 2 x 5: Akima along u2 only
        let d = [0.0, 1.0, 2.0, 3.0, 5.0, 7.0, 1.0, 4.0, 2.0, 2.0];
        let t = Table2::new(2, 5, &d, AKIMA, LAST_TWO_POINTS).unwrap();
        assert!((t.eval(0, 9.0, 3.0) - 2.0).abs() < 1e-12);
        assert!((t.eval(0, 9.0, 2.0) - 4.0).abs() < 1e-12);
        assert_eq!(t.eval(kind::D1, 0.0, 2.5), 0.0);
        assert!((t.eval(kind::D2, 0.0, 2.5) - num_d(&t, 0, 0.0, 2.5, 1)).abs() < 1e-6);
        // 3 x 3 Akima is bilinear
        let d = [0.0, 1.0, 2.0, 1.0, 1.0, 3.0, 2.0, 5.0, 7.0];
        let t = Table2::new(3, 3, &d, AKIMA, LAST_TWO_POINTS).unwrap();
        assert_eq!(t.smoothness, LINEAR_SEGMENTS);
    }
}
