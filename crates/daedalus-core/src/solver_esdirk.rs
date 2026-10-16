//! ESDIRK4(3)6L[2]SA of Kennedy & Carpenter (the implicit part of ARK4(3)6L[2]SA,
//! Appl. Numer. Math. 44 (2003)): six stages, the first explicit, the other five
//! with the same diagonal `γ = 1/4`; stiffly accurate and L-stable, order 4 with an
//! embedded order-3 solution for the error estimate.
//!
//! Each implicit stage is a Newton iteration on `(I/(hγ) - J)`, so one real
//! factorization serves the whole step (and the following steps while `h` stays put).
//! The error estimate is filtered by `(I - hγ J)^-1`, which keeps it meaningful for
//! stiff components. Dense output is cubic Hermite interpolation between the step
//! end points.

#![allow(clippy::needless_range_loop)]

use crate::codegen::Runtime;
use crate::solver::{Integrator, SolverStats, initial_step, rms};
use crate::solver_jac::{JacobianMode, RealSystem, StateJac};

const S: usize = 6;
const GAMMA: f64 = 0.25;
const C: [f64; S] = [0.0, 0.5, 83.0 / 250.0, 31.0 / 50.0, 17.0 / 20.0, 1.0];
const A: [[f64; S]; S] = [
    [0.0; S],
    [0.25, 0.25, 0.0, 0.0, 0.0, 0.0],
    [8611.0 / 62500.0, -1743.0 / 31250.0, 0.25, 0.0, 0.0, 0.0],
    [
        5012029.0 / 34652500.0,
        -654441.0 / 2922500.0,
        174375.0 / 388108.0,
        0.25,
        0.0,
        0.0,
    ],
    [
        15267082809.0 / 155376265600.0,
        -71443401.0 / 120774400.0,
        730878875.0 / 902184768.0,
        2285395.0 / 8070912.0,
        0.25,
        0.0,
    ],
    [
        82889.0 / 524892.0,
        0.0,
        15625.0 / 83664.0,
        69875.0 / 102672.0,
        -2260.0 / 8211.0,
        0.25,
    ],
];
/// Embedded third-order weights.
const BHAT: [f64; S] = [
    4586570599.0 / 29645900160.0,
    0.0,
    178811875.0 / 945068544.0,
    814220225.0 / 1159782912.0,
    -3700637.0 / 11593932.0,
    61727.0 / 225920.0,
];

const NEWTON_MAXITER: usize = 8;

pub struct Esdirk {
    pub rtol: f64,
    /// Absolute tolerance of each state (`tolerance * |nominal|`).
    pub atol: Vec<f64>,
    pub max_step: f64,
    n: usize,
    t: f64,
    t_old: f64,
    h: f64,
    x: Vec<f64>,
    x_old: Vec<f64>,
    /// `f(t, x)` and `f(t_old, x_old)` for the dense output.
    f: Vec<f64>,
    f_old: Vec<f64>,
    f_end: Vec<f64>,
    /// Derivative at `t` from the last stage equation (first stage of the next step).
    fsal: Vec<f64>,
    k: [Vec<f64>; S],
    z: Vec<f64>,
    base: Vec<f64>,
    dz: Vec<f64>,
    fz: Vec<f64>,
    scale: Vec<f64>,
    jac: StateJac,
    sys: RealSystem,
    /// `h γ` the factorization belongs to (NaN: none).
    lu_hg: f64,
    need_jac: bool,
    jac_current: bool,
    /// Last observed Newton contraction rate.
    rate: f64,
    stats: SolverStats,
}

impl Esdirk {
    /// `pattern[i]` lists the states derivative `i` depends on.
    pub fn new(
        n: usize,
        rtol: f64,
        atol: Vec<f64>,
        max_step: f64,
        pattern: &[Vec<u32>],
        mode: JacobianMode,
    ) -> Self {
        let jac = StateJac::new(n, pattern, mode);
        let sys = RealSystem::new(&jac);
        let z = || vec![0.0; n];
        Esdirk {
            rtol,
            atol,
            max_step,
            n,
            t: 0.0,
            t_old: 0.0,
            h: 0.0,
            x: z(),
            x_old: z(),
            f: z(),
            f_old: z(),
            f_end: z(),
            fsal: z(),
            k: [z(), z(), z(), z(), z(), z()],
            z: z(),
            base: z(),
            dz: z(),
            fz: z(),
            scale: z(),
            jac,
            sys,
            lu_hg: f64::NAN,
            need_jac: true,
            jac_current: false,
            rate: 0.5,
            stats: SolverStats::default(),
        }
    }

    pub fn n_colors(&self) -> usize {
        self.jac.n_colors()
    }

    fn newton_tol(&self) -> f64 {
        0.5 * (10.0 * f64::EPSILON / self.rtol).max(0.03f64.min(self.rtol.sqrt()))
    }

    /// Solve stage `s` at step size `h`; the stage derivative goes to `k[s]`.
    /// False if Newton does not converge.
    fn stage(&mut self, rt: &mut Runtime, s: usize, h: f64) -> bool {
        let n = self.n;
        let hg = h * GAMMA;
        for i in 0..n {
            let mut acc = self.x[i];
            for j in 0..s {
                let a = A[s][j];
                if a != 0.0 {
                    acc += h * a * self.k[j][i];
                }
            }
            self.base[i] = acc;
            // Predictor after a restart: the explicit part plus the last stage derivative.
            self.z[i] = acc + hg * self.k[s - 1][i];
            self.scale[i] = self.atol[i] + self.rtol * acc.abs();
        }
        // Better: the cubic Hermite interpolant of the last step, extrapolated.
        let hp = self.t - self.t_old;
        if hp > 0.0 {
            let th = (self.t + C[s] * h - self.t_old) / hp;
            let h00 = (1.0 + 2.0 * th) * (1.0 - th) * (1.0 - th);
            let h10 = th * (1.0 - th) * (1.0 - th);
            let h01 = th * th * (3.0 - 2.0 * th);
            let h11 = th * th * (th - 1.0);
            for i in 0..n {
                self.z[i] = h00 * self.x_old[i]
                    + h10 * hp * self.f_old[i]
                    + h01 * self.x[i]
                    + h11 * hp * self.f[i];
            }
        }
        let tol = self.newton_tol();
        let ts = self.t + C[s] * h;
        let mut norm_old: Option<f64> = None;
        let nmi = NEWTON_MAXITER;
        for it in 0..nmi {
            rt.rhs(ts, &self.z, &mut self.fz);
            self.stats.rhs_evals += 1;
            if rt.error.is_some() || !self.fz.iter().all(|v| v.is_finite()) {
                rt.error = None;
                return false;
            }
            // (I - hγ J) dz = base + hγ f(z) - z  <=>  (I/(hγ) - J) dz = (...)/(hγ)
            for i in 0..n {
                self.dz[i] = self.fz[i] + (self.base[i] - self.z[i]) / hg;
            }
            self.sys.solve(&mut self.dz);
            let norm = rms(self.dz.iter().zip(&self.scale).map(|(d, s)| d / s), n);
            if !norm.is_finite() {
                return false;
            }
            for i in 0..n {
                self.z[i] += self.dz[i];
            }
            let rate = match norm_old {
                Some(o) if o > 0.0 => norm / o,
                // Before a rate is observed in this stage, assume no better than 0.1:
                // the last stage's rate says little about this stage's predictor.
                _ => self.rate.max(0.1),
            };
            if norm_old.is_some() {
                self.rate = rate;
                if rate >= 1.0 || rate.powi((nmi - it) as i32) / (1.0 - rate) * norm > tol {
                    return false;
                }
            }
            if norm == 0.0 || (rate < 1.0 && rate / (1.0 - rate) * norm < tol) {
                break;
            }
            if it + 1 == nmi {
                return false;
            }
            norm_old = Some(norm);
        }
        // The stage derivative from the solved equation (avoids amplifying the
        // Newton residual by the stiff Jacobian).
        for i in 0..n {
            self.k[s][i] = (self.z[i] - self.base[i]) / hg;
        }
        true
    }
}

impl Esdirk {
    /// Error estimate of the dense output at the middle of the step just computed (not
    /// yet accepted; the solution is in `z`), relative to the tolerance: the defect of
    /// the Hermite interpolant at the midpoint filtered by `(I/(hγ) - J)^-1` (see
    /// `Radau5::dense_defect`). Leaves `f(t + h, z)` in `f_end`.
    fn dense_defect(&mut self, rt: &mut Runtime, h: f64) -> f64 {
        let n = self.n;
        rt.rhs(self.t + h, &self.z, &mut self.f_end);
        self.stats.rhs_evals += 1;
        if rt.error.is_some() || !self.f_end.iter().all(|v| v.is_finite()) {
            // Keep the stage derivative instead.
            rt.error = None;
            self.f_end.copy_from_slice(&self.k[S - 1]);
        }
        for i in 0..n {
            let (x0, x1, f0, f1) = (self.x[i], self.z[i], self.f[i], self.f_end[i]);
            self.base[i] = 0.5 * (x0 + x1) + 0.125 * h * (f0 - f1);
            self.dz[i] = 1.5 * (x1 - x0) / h - 0.25 * (f0 + f1);
        }
        rt.rhs(self.t + 0.5 * h, &self.base, &mut self.fz);
        self.stats.rhs_evals += 1;
        rt.error = None;
        for i in 0..n {
            self.dz[i] -= self.fz[i];
        }
        self.sys.solve(&mut self.dz);
        let est = rms(
            self.dz
                .iter()
                .zip(&self.base)
                .zip(&self.atol)
                .map(|((e, u), a)| e / (a + self.rtol * u.abs())),
            n,
        );
        if est.is_finite() { est } else { 1e10 }
    }
}

impl Integrator for Esdirk {
    fn reset(&mut self, rt: &mut Runtime, t: f64, x: &[f64]) {
        self.t = t;
        self.t_old = t;
        self.x.copy_from_slice(x);
        self.x_old.copy_from_slice(x);
        rt.rhs(t, x, &mut self.f);
        self.stats.rhs_evals += 1;
        self.f_old.copy_from_slice(&self.f);
        self.fsal.copy_from_slice(&self.f);
        self.h = 0.0;
        self.need_jac = true;
        self.jac_current = false;
        self.lu_hg = f64::NAN;
        self.rate = 0.5;
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
            return Ok(self.t);
        }
        if self.h <= 0.0 {
            let f0 = self.f.clone();
            self.h = initial_step(
                rt,
                self.t,
                &self.x,
                &f0,
                3.0,
                self.rtol,
                &self.atol,
                span.min(self.max_step),
            );
            self.stats.rhs_evals += 1;
        }
        let min_h = 1e-14 * self.t.abs().max(1.0);
        loop {
            let planned = self.h.min(self.max_step);
            let mut h = planned;
            let last = h >= span * (1.0 - 1e-12);
            if last {
                h = span;
            }
            if h < min_h {
                return Err(format!("step size too small at t = {}", self.t));
            }
            if self.need_jac {
                let x = self.x.clone();
                self.jac
                    .compute(rt, self.t, &x, &self.atol, self.rtol, &mut self.stats);
                self.need_jac = false;
                self.jac_current = true;
                self.lu_hg = f64::NAN;
            }
            let hg = h * GAMMA;
            if self.lu_hg != hg {
                self.stats.lu_factorizations += 1;
                if self.sys.factor(&self.jac, 1.0 / hg) {
                    self.lu_hg = hg;
                } else {
                    self.lu_hg = f64::NAN;
                    self.stats.rejected += 1;
                    self.h = h * 0.5;
                    continue;
                }
            }
            self.k[0].copy_from_slice(&self.fsal);
            let mut converged = true;
            let mut max_rate: f64 = 0.0;
            for s in 1..S {
                if !self.stage(rt, s, h) {
                    converged = false;
                    break;
                }
                max_rate = max_rate.max(self.rate);
            }
            if !converged {
                self.stats.rejected += 1;
                self.stats.newton_failures += 1;
                if self.jac_current {
                    self.h = h * 0.5;
                } else {
                    // Retry with a fresh Jacobian first.
                    self.need_jac = true;
                }
                continue;
            }
            // z holds the last stage = the 4th-order solution (stiffly accurate).
            for i in 0..n {
                let mut e = 0.0;
                for j in 0..S {
                    let d = A[S - 1][j] - BHAT[j];
                    if d != 0.0 {
                        e += d * self.k[j][i];
                    }
                }
                // (I - hγJ)^-1 (h e) = (I/(hγ) - J)^-1 (e / γ)
                self.dz[i] = e / GAMMA;
                self.scale[i] = self.atol[i] + self.rtol * self.x[i].abs().max(self.z[i].abs());
            }
            self.sys.solve(&mut self.dz);
            let err = rms(self.dz.iter().zip(&self.scale).map(|(d, s)| d / s), n);
            let err = if err.is_finite() { err } else { 1e10 };
            // The dense output must be as good as the tolerance too.
            let dens = if err <= 1.0 {
                self.dense_defect(rt, h)
            } else {
                0.0
            };
            if dens > 1.0 {
                self.stats.rejected += 1;
                self.h = h * (0.9 * dens.powf(-0.25)).clamp(0.2, 0.9);
                continue;
            }
            if err <= 1.0 {
                self.t_old = self.t;
                self.t = if last { t_max } else { self.t + h };
                self.x_old.copy_from_slice(&self.x);
                self.f_old.copy_from_slice(&self.f);
                self.x.copy_from_slice(&self.z);
                self.f.copy_from_slice(&self.f_end);
                self.fsal.copy_from_slice(&self.k[S - 1]);
                self.stats.steps += 1;
                self.jac_current = false;
                // Slow contraction: the Jacobian is out of date.
                if max_rate > 0.3 {
                    self.need_jac = true;
                }
                let fac = (0.9 * err.max(dens).max(1e-10).powf(-0.25)).clamp(0.2, 5.0);
                let mut hnew = h * fac;
                if last && h < planned {
                    hnew = hnew.max(planned);
                }
                // Keep the step (and the factorization) for small increases.
                if !(1.0..=1.2).contains(&(hnew / h)) || last {
                    self.h = hnew;
                }
                return Ok(self.t);
            }
            self.stats.rejected += 1;
            let fac = (0.9 * err.powf(-0.25)).clamp(0.1, 0.9);
            self.h = h * fac;
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
        let s = (t - self.t_old) / h;
        let h00 = (1.0 + 2.0 * s) * (1.0 - s) * (1.0 - s);
        let h10 = s * (1.0 - s) * (1.0 - s);
        let h01 = s * s * (3.0 - 2.0 * s);
        let h11 = s * s * (s - 1.0);
        for (i, o) in out.iter_mut().enumerate() {
            *o = h00 * self.x_old[i]
                + h10 * h * self.f_old[i]
                + h01 * self.x[i]
                + h11 * h * self.f[i];
        }
    }
    fn stats(&self) -> SolverStats {
        self.stats
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The tableau satisfies the order conditions: 4 for `b`, 3 for `bhat`.
    #[test]
    fn order_conditions() {
        let dot = |u: &[f64], v: &[f64]| u.iter().zip(v).map(|(a, b)| a * b).sum::<f64>();
        let c: Vec<f64> = A.iter().map(|r| r.iter().sum()).collect();
        for (ci, cc) in c.iter().zip(C) {
            assert!((ci - cc).abs() < 1e-14);
        }
        let mv = |v: &[f64]| -> Vec<f64> { A.iter().map(|r| dot(r, v)).collect() };
        let c2: Vec<f64> = c.iter().map(|x| x * x).collect();
        let c3: Vec<f64> = c.iter().map(|x| x * x * x).collect();
        let ac = mv(&c);
        let cac: Vec<f64> = c.iter().zip(&ac).map(|(a, b)| a * b).collect();
        let check = |b: &[f64], order: usize| {
            let mut conds = vec![
                (b.iter().sum::<f64>(), 1.0),
                (dot(b, &c), 0.5),
                (dot(b, &c2), 1.0 / 3.0),
                (dot(b, &ac), 1.0 / 6.0),
            ];
            if order >= 4 {
                conds.extend([
                    (dot(b, &c3), 0.25),
                    (dot(b, &cac), 0.125),
                    (dot(b, &mv(&c2)), 1.0 / 12.0),
                    (dot(b, &mv(&ac)), 1.0 / 24.0),
                ]);
            }
            for (got, want) in conds {
                assert!((got - want).abs() < 1e-13, "{got} vs {want}");
            }
        };
        check(&A[S - 1], 4);
        check(&BHAT, 3);
        // L-stability: stiffly accurate with an explicit first stage, R(inf) = 0
        // needs the last row to end in the diagonal.
        assert_eq!(A[S - 1][S - 1], GAMMA);
    }
}
