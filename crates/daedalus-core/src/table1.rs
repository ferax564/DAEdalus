//! 1-D interpolation tables: the runtime half of the bundled `interpolate` builtin and of
//! the MSL `CombiTable1Ds`/`CombiTable1Dv` and `CombiTimeTable` blocks, whose external C
//! tables (`ModelicaStandardTables.c`) are implemented natively here.
//!
//! A lookup ([`crate::expr::Node::Interp`]) asks for one of the [`kind`]s: the value, the
//! first or the second derivative with respect to the abscissa, or (time tables) the next
//! time event. The value and derivatives follow the C functions `getValue`,
//! `getDerValue` and `getDer2Value` (the latter two without the chain-rule factors).
//!
//! Time tables are stateful in C: the table remembers the current event interval and
//! keeps its value on that interval until the event at its end has been handled. Here
//! the same information comes from the `nextTimeEvent` argument of the lookup, which
//! names the event that ends the current interval: the lookup stays on the rows of that
//! interval and computes the abscissa as `event - (nextTimeEvent - time)`, which is exact
//! near the event even for periodic tables far from the first period.

/// The quantity a lookup asks for.
pub mod kind {
    pub const VALUE: u32 = 0;
    /// First derivative with respect to the abscissa.
    pub const D1: u32 = 1;
    /// Second derivative with respect to the abscissa.
    pub const D2: u32 = 2;
    /// The next time event after the abscissa (time tables).
    pub const NEXT_EVENT: u32 = 3;
    /// Bits of the kind in a lookup's `view << BITS | kind` field.
    pub const BITS: u32 = 2;
    /// Third derivative (piecewise constant for the cubic splines): only for
    /// [`super::Table::eval`] (the tangent of a [`D2`] lookup), not a node kind.
    pub const D3: u32 = 4;
}

/// Constant interpolation table (rows of `[x, y1, y2, ...]`).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Table {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
    /// 0 = hold the first/last value, 1 = extrapolate the first/last two points,
    /// 2 = periodic.
    pub extrapolation: u8,
    /// 0 = linear segments, 1 = constant segments, 2 = cubic Hermite spline (`spline`).
    #[serde(default)]
    pub smoothness: u8,
    /// Spline coefficients `[c0, c1, c2]` per segment and column (`segment * cols + col`):
    /// `y = y_i + ((c0 v + c1) v + c2) v` with `v = x - x_i`.
    #[serde(default)]
    pub spline: Vec<[f64; 3]>,
    /// The abscissa is `x - shift` (time tables: `shiftTime`).
    #[serde(default)]
    pub shift: f64,
    /// Below `start` the table yields 0 (time tables: `startTime`).
    #[serde(default = "neg_inf")]
    pub start: f64,
    /// `CombiTimeTable` semantics (event intervals, see the module documentation).
    #[serde(default)]
    pub time: bool,
    /// Time events of a time table in table coordinates, ascending (one period for
    /// periodic tables, whose first event is after the first abscissa).
    #[serde(default)]
    pub events: Vec<f64>,
}

fn neg_inf() -> f64 {
    f64::NEG_INFINITY
}

/// `isNearlyEqual` of the C tables.
fn near(x: f64, y: f64) -> bool {
    let cmp = x.abs().max(y.abs()).max(1e-10) * 1e-10;
    (y - x).abs() < cmp
}

#[derive(Clone, Copy, PartialEq)]
enum Side {
    Left,
    Right,
}

impl Table {
    /// Linear interpolation (`extrapolation` as in [`Table::extrapolation`]).
    pub fn linear(rows: usize, cols: usize, data: Vec<f64>, extrapolation: u8) -> Table {
        Table {
            rows,
            cols,
            data,
            extrapolation,
            smoothness: 0,
            spline: Vec::new(),
            shift: 0.0,
            start: f64::NEG_INFINITY,
            time: false,
            events: Vec::new(),
        }
    }

    #[inline]
    pub fn at(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    #[inline]
    fn x(&self, r: usize) -> f64 {
        self.data[r * self.cols]
    }

    /// Cubic Hermite spline by the `Smoothness` of the C tables: 2 = Akima,
    /// 4 = Fritsch–Butland, 5 = Steffen, 6 = modified Akima. Tables of at most two rows
    /// stay linear, as in C.
    pub fn make_spline(&mut self, method: u8) {
        let n = self.rows;
        if n <= 2 {
            return;
        }
        self.smoothness = 2;
        self.spline = vec![[0.0; 3]; (n - 1) * self.cols];
        for col in 1..self.cols {
            let d: Vec<f64> = (0..n - 1)
                .map(|i| (self.at(i + 1, col) - self.at(i, col)) / (self.x(i + 1) - self.x(i)))
                .collect();
            // slopes at the rows
            let mut s = vec![0.0; n];
            match method {
                4 | 5 => {
                    s[0] = d[0];
                    s[n - 1] = d[n - 2];
                    for i in 0..n - 2 {
                        let (a, b) = (d[i], d[i + 1]);
                        s[i + 1] = if a == 0.0 || b == 0.0 || (a < 0.0) != (b < 0.0) {
                            0.0
                        } else {
                            let dx = self.x(i + 1) - self.x(i);
                            let dx_ = self.x(i + 2) - self.x(i + 1);
                            if method == 4 {
                                3.0 * (dx + dx_) / ((dx + 2.0 * dx_) / a + (dx_ + 2.0 * dx) / b)
                            } else {
                                let c2 = (a * dx_ + b * dx) / (dx + dx_);
                                let h = 0.5 * c2.abs();
                                if h > a.abs() || h > b.abs() {
                                    let two = if a > 0.0 { 2.0 } else { -2.0 };
                                    two * a.abs().min(b.abs())
                                } else {
                                    c2
                                }
                            }
                        };
                    }
                }
                _ => {
                    // Akima (2) or modified Akima (6), with the end extension of the
                    // divided differences.
                    let mut e = vec![0.0; n + 3];
                    e[2..n + 1].copy_from_slice(&d);
                    e[0] = 3.0 * e[2] - 2.0 * e[3];
                    e[1] = 2.0 * e[2] - e[3];
                    e[n + 1] = 2.0 * e[n] - e[n - 1];
                    e[n + 2] = 3.0 * e[n] - 2.0 * e[n - 1];
                    let makima = method == 6;
                    for (i, si) in s.iter_mut().enumerate() {
                        let (m1, m2, m3, m4) = (e[i], e[i + 1], e[i + 2], e[i + 3]);
                        *si = if makima {
                            let w1 = (m4 - m3).abs() + 0.5 * (m4 + m3).abs();
                            let w2 = (m2 - m1).abs() + 0.5 * (m2 + m1).abs();
                            if w1 + w2 > 0.0 {
                                let a = w2 / (w1 + w2);
                                (1.0 - a) * m2 + a * m3
                            } else {
                                0.0
                            }
                        } else {
                            let c2 = (m4 - m3).abs() + (m2 - m1).abs();
                            if c2 > 0.0 {
                                let a = (m2 - m1).abs() / c2;
                                (1.0 - a) * m2 + a * m3
                            } else {
                                0.5 * m2 + 0.5 * m3
                            }
                        };
                    }
                }
            }
            for i in 0..n - 1 {
                let dx = self.x(i + 1) - self.x(i);
                let (c2, c2n) = (s[i], s[i + 1]);
                self.spline[i * self.cols + col] = [
                    (c2 + c2n - 2.0 * d[i]) / (dx * dx),
                    (3.0 * d[i] - 2.0 * c2 - c2n) / dx,
                    c2,
                ];
            }
        }
    }

    /// Make this a time table with the time events of `CombiTimeTable` (`timeEvents`:
    /// 1 = always, 2 = at discontinuities, 3 = none), after the smoothness is set.
    pub fn set_time_events(&mut self, mode: u8) {
        self.time = true;
        self.events.clear();
        let n = self.rows;
        if n < 2 {
            return;
        }
        let (tmin, tmax) = (self.x(0), self.x(n - 1));
        let periodic = self.extrapolation == 2;
        if !periodic {
            self.events.push(tmin);
        }
        // In C a spline table only has events at the ends unless periodic.
        if (mode == 1 || mode == 2) && (self.smoothness != 2 || periodic) {
            let mut te = tmin;
            for i in 0..n - 1 {
                let (t0, t1) = (self.x(i), self.x(i + 1));
                if t1 > te && !near(t1, tmax) {
                    let eq = near(t0, t1);
                    if (mode == 1 && !eq) || (mode == 2 && eq) {
                        te = t1;
                        self.events.push(t1);
                    }
                }
            }
        }
        if self.events.last().is_none_or(|&l| !near(l, tmax)) {
            self.events.push(tmax);
        }
    }

    /// Value of column `col` at `x` (no time-event information).
    pub fn interp(&self, col: usize, x: f64) -> f64 {
        self.eval(kind::VALUE, col, x, f64::INFINITY)
    }

    /// Slope of column `col` at `x` (no time-event information).
    pub fn slope(&self, col: usize, x: f64) -> f64 {
        self.eval(kind::D1, col, x, f64::INFINITY)
    }

    /// The lookup `kind` of column `col` at abscissa `x`; for a time table `ne` is the
    /// current `nextTimeEvent` (`+inf` when there is none).
    pub fn eval(&self, kind: u32, col: usize, x: f64, ne: f64) -> f64 {
        if kind == kind::NEXT_EVENT {
            return self.next_event(x);
        }
        if x < self.start || self.rows == 0 {
            return 0.0;
        }
        // Before the start-time event has been handled the output is zero.
        let event = self.time && !ne.is_nan() && ne < 1e60;
        if event && ne <= self.start {
            return 0.0;
        }
        if self.rows == 1 {
            return if kind == kind::VALUE {
                self.at(0, col)
            } else {
                0.0
            };
        }
        let u = x - self.shift;
        if !event {
            return self.generic(kind, col, u);
        }
        let n = self.rows;
        let (tmin, tmax) = (self.x(0), self.x(n - 1));
        let periodic = self.extrapolation == 2;
        let mut e = ne - self.shift;
        if periodic {
            let p = tmax - tmin;
            if p > 0.0 {
                let r = (e - tmax) / p;
                let k = if (r - r.round()).abs() < 1e-9 {
                    r.round()
                } else {
                    r.ceil()
                };
                e -= k * p;
            }
        }
        let tol = 1e-9 * (tmax - tmin).abs().max(e.abs()).max(1.0);
        let Some(j) = self.events.iter().position(|&ev| (ev - e).abs() <= tol) else {
            return self.generic(kind, col, u);
        };
        if !periodic && j == 0 {
            // left of the first abscissa
            let v = (tmin - (ne - x)).min(tmin);
            return self.extrapolate(kind, col, Side::Left, v);
        }
        let prev = if j == 0 { tmin } else { self.events[j - 1] };
        let i1 = (0..n).find(|&r| near(self.x(r), self.events[j]));
        let i0 = (0..n).rev().find(|&r| near(self.x(r), prev));
        let (Some(i0), Some(i1)) = (i0, i1) else {
            return self.generic(kind, col, u);
        };
        if i0 >= i1 {
            return self.generic(kind, col, u);
        }
        let v = (self.x(i1) - (ne - x)).clamp(self.x(i0), self.x(i1));
        let seg = self.find(v, i0, i1);
        self.segment(kind, col, seg, v, false)
    }

    /// The last row `i` in `lo..hi` with `x_i <= v` (`lo` if none).
    fn find(&self, v: f64, lo: usize, hi: usize) -> usize {
        let (mut a, mut b) = (lo, hi);
        while b > a + 1 {
            let m = (a + b) / 2;
            if v < self.x(m) {
                b = m;
            } else {
                a = m;
            }
        }
        a
    }

    /// Lookup without event intervals (`u` in table coordinates).
    fn generic(&self, kind: u32, col: usize, mut u: f64) -> f64 {
        let n = self.rows;
        let (umin, umax) = (self.x(0), self.x(n - 1));
        if self.extrapolation == 2 {
            let p = umax - umin;
            if p > 0.0 {
                if u < umin {
                    u += ((umin - u) / p).ceil() * p;
                    if u < umin {
                        u += p;
                    }
                } else if u > umax {
                    u -= ((u - umax) / p).ceil() * p;
                    if u > umax {
                        u -= p;
                    }
                }
                u = u.clamp(umin, umax);
            }
        } else if u < umin {
            return self.extrapolate(kind, col, Side::Left, u);
        } else if u > umax || (self.time && u >= umax) {
            return self.extrapolate(kind, col, Side::Right, u);
        }
        let seg = self.find(u, 0, n - 1);
        self.segment(kind, col, seg, u, true)
    }

    /// Interpolation on segment `i` (rows `i`, `i + 1`); `step` lets constant segments
    /// take the right value at the right end.
    fn segment(&self, kind: u32, col: usize, i: usize, u: f64, step: bool) -> f64 {
        let (t0, t1) = (self.x(i), self.x(i + 1));
        let (y0, y1) = (self.at(i, col), self.at(i + 1, col));
        match self.smoothness {
            1 => match kind {
                kind::VALUE if step && u >= t1 => y1,
                kind::VALUE => y0,
                _ => 0.0,
            },
            2 => {
                let [c0, c1, c2] = self.spline[i * self.cols + col];
                let v = u - t0;
                match kind {
                    kind::VALUE => y0 + ((c0 * v + c1) * v + c2) * v,
                    kind::D1 => (3.0 * c0 * v + 2.0 * c1) * v + c2,
                    kind::D2 => 6.0 * c0 * v + 2.0 * c1,
                    _ => 6.0 * c0,
                }
            }
            _ => {
                if t1 <= t0 || (self.time && near(t0, t1)) {
                    return if kind == kind::VALUE { y1 } else { 0.0 };
                }
                match kind {
                    kind::VALUE => y0 + (y1 - y0) * (u - t0) / (t1 - t0),
                    kind::D1 => (y1 - y0) / (t1 - t0),
                    _ => 0.0,
                }
            }
        }
    }

    /// Lookup outside the abscissa range.
    fn extrapolate(&self, kind: u32, col: usize, side: Side, u: f64) -> f64 {
        let n = self.rows;
        if self.extrapolation != 1 {
            return match (kind, side) {
                (kind::VALUE, Side::Left) => self.at(0, col),
                (kind::VALUE, Side::Right) => self.at(n - 1, col),
                _ => 0.0,
            };
        }
        if kind >= kind::D2 {
            return 0.0;
        }
        if self.smoothness == 2 {
            let (y, s, x0) = if side == Side::Left {
                (self.at(0, col), self.spline[col][2], self.x(0))
            } else {
                let [c0, c1, c2] = self.spline[(n - 2) * self.cols + col];
                let v = self.x(n - 1) - self.x(n - 2);
                (
                    self.at(n - 1, col),
                    (3.0 * c0 * v + 2.0 * c1) * v + c2,
                    self.x(n - 1),
                )
            };
            return if kind == kind::VALUE {
                y + s * (u - x0)
            } else {
                s
            };
        }
        let i = if side == Side::Left { 0 } else { n - 2 };
        let (t0, t1) = (self.x(i), self.x(i + 1));
        let (y0, y1) = (self.at(i, col), self.at(i + 1, col));
        if t1 <= t0 || (self.time && near(t0, t1)) {
            return match (kind, side) {
                (kind::VALUE, Side::Left) => y0,
                (kind::VALUE, Side::Right) => y1,
                _ => 0.0,
            };
        }
        if kind == kind::VALUE {
            y0 + (y1 - y0) * (u - t0) / (t1 - t0)
        } else {
            (y1 - y0) / (t1 - t0)
        }
    }

    /// The first time event strictly after time `x` (in unshifted coordinates), or +inf.
    pub fn next_event(&self, x: f64) -> f64 {
        if x < self.start {
            return self.start;
        }
        if self.rows < 2 || self.events.is_empty() {
            return f64::INFINITY;
        }
        let xs = x - self.shift;
        let eps = 1e-12 * (1.0 + xs.abs());
        if self.extrapolation == 2 {
            let (x0, x1) = (self.x(0), self.x(self.rows - 1));
            let p = x1 - x0;
            if p > 0.0 {
                let mut k = ((xs - x0) / p).floor();
                let mut local = xs - k * p;
                if local < x0 {
                    k -= 1.0;
                    local += p;
                }
                let t = match self.events.iter().find(|&&e| e > local + eps) {
                    Some(&e) => e + k * p,
                    None => self.events[0] + (k + 1.0) * p,
                };
                return t + self.shift;
            }
        }
        self.events
            .iter()
            .find(|&&e| e > xs + eps)
            .map_or(f64::INFINITY, |&e| e + self.shift)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_modes() {
        let data = vec![0.0, 0.0, 1.0, 1.0, 2.0, 4.0, 3.0, 9.0];
        let mut t = Table::linear(4, 2, data.clone(), 0);
        assert_eq!(t.interp(1, 1.5), 2.5);
        assert_eq!(t.interp(1, 5.0), 9.0);
        t.extrapolation = 1;
        assert_eq!(t.interp(1, 4.0), 14.0);
        t.smoothness = 1;
        assert_eq!(t.interp(1, 1.5), 1.0);
        assert_eq!(t.slope(1, 1.5), 0.0);
        // constant segments extrapolate linearly from the edge segment
        assert_eq!(t.interp(1, 4.0), 14.0);
        assert_eq!(t.slope(1, 4.0), 5.0);
        // Periodic with a shift: period 3, so x = 4.5 + 1 maps to 1.5 + 1 = 2.5.
        let mut p = Table::linear(4, 2, data.clone(), 2);
        p.shift = 1.0;
        p.set_time_events(1);
        assert_eq!(p.interp(1, 5.5), p.interp(1, 2.5));
        assert_eq!(p.next_event(4.2), 5.0);
        assert_eq!(p.next_event(6.5), 7.0);
        // Cubic Hermite passes through the points and has a continuous slope.
        let mut c = Table::linear(4, 2, data, 0);
        c.make_spline(2);
        for (x, y) in [(0.0, 0.0), (1.0, 1.0), (2.0, 4.0), (3.0, 9.0)] {
            assert!((c.interp(1, x) - y).abs() < 1e-12);
        }
        assert!((c.slope(1, 1.0 - 1e-9) - c.slope(1, 1.0 + 1e-9)).abs() < 1e-6);
        let h = 1e-6;
        let fd = (c.interp(1, 1.3 + h) - c.interp(1, 1.3 - h)) / (2.0 * h);
        assert!((fd - c.slope(1, 1.3)).abs() < 1e-6);
        let fd2 = (c.slope(1, 1.3 + h) - c.slope(1, 1.3 - h)) / (2.0 * h);
        assert!((fd2 - c.eval(kind::D2, 1, 1.3, f64::INFINITY)).abs() < 1e-5);
        let fd3 = (c.eval(kind::D2, 1, 1.3 + h, f64::INFINITY)
            - c.eval(kind::D2, 1, 1.3 - h, f64::INFINITY))
            / (2.0 * h);
        assert!((fd3 - c.eval(kind::D3, 1, 1.3, f64::INFINITY)).abs() < 1e-5);
        assert_eq!(c.eval(kind::D3, 1, 4.0, f64::INFINITY), 0.0);
        // Start time: zero output and the first event at the start.
        let mut s = Table::linear(2, 2, vec![0.0, 1.0, 1.0, 1.0], 0);
        s.start = 0.5;
        s.set_time_events(1);
        assert_eq!(s.interp(1, 0.2), 0.0);
        assert_eq!(s.next_event(0.2), 0.5);
        assert_eq!(s.eval(kind::NEXT_EVENT, 0, 0.7, 0.0), 1.0);
    }

    #[test]
    fn time_event_intervals() {
        // periodic with a jump at 0.51
        let rows = [
            (0.1, 0.0),
            (0.3, 0.0),
            (0.5, -1.0),
            (0.51, -2.0),
            (0.51, -3.0),
            (0.6, -2.0),
            (1.0, -4.0),
        ];
        let data: Vec<f64> = rows.iter().flat_map(|&(x, y)| [x, y]).collect();
        let mut t = Table::linear(rows.len(), 2, data, 2);
        t.start = 0.0;
        t.set_time_events(1);
        assert_eq!(t.events, vec![0.3, 0.5, 0.51, 0.6, 1.0]);
        // before the event at 0.51 (period 2): left value; after it: right value
        let ne = t.next_event(1.405);
        assert!((ne - 1.41).abs() < 1e-12);
        assert!((t.eval(kind::VALUE, 1, 1.41, ne) + 2.0).abs() < 1e-9);
        assert!((t.eval(kind::D1, 1, 1.41, ne) + 100.0).abs() < 1e-6);
        let ne2 = t.next_event(1.41);
        assert!((ne2 - 1.5).abs() < 1e-12);
        assert!((t.eval(kind::VALUE, 1, 1.41, ne2) + 3.0).abs() < 1e-9);
        // the end of a period is the start of the next one
        let ne3 = t.next_event(1.85);
        assert!((ne3 - 1.9).abs() < 1e-12);
        assert!((t.eval(kind::VALUE, 1, 1.9, ne3) + 4.0).abs() < 1e-9);
        let ne4 = t.next_event(1.9);
        assert!((ne4 - 2.1).abs() < 1e-12);
        assert!(t.eval(kind::VALUE, 1, 1.9, ne4).abs() < 1e-9);
    }

    #[test]
    fn monotone_splines() {
        // Fritsch-Butland and Steffen keep monotone data monotone.
        let rows = [
            (0.0, 5.0),
            (1.0, 3.0),
            (2.0, 1.0),
            (3.0, 1.0),
            (4.0, 1.0),
            (5.0, 3.0),
        ];
        let data: Vec<f64> = rows.iter().flat_map(|&(x, y)| [x, y]).collect();
        for m in [4, 5] {
            let mut t = Table::linear(rows.len(), 2, data.clone(), 0);
            t.make_spline(m);
            for k in 0..=40 {
                let x = 2.0 + k as f64 * 0.05;
                assert!((t.interp(1, x) - 1.0).abs() < 1e-12, "method {m} at {x}");
            }
            for (x, y) in rows {
                assert!((t.interp(1, x) - y).abs() < 1e-12);
            }
        }
    }
}
