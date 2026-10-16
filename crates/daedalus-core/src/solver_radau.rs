//! Radau IIA of order 5 (3 stages), after Hairer & Wanner's RADAU5 (Solving ODEs II,
//! section IV.8).
//!
//! The stage equations are solved by a simplified Newton iteration in transformed
//! coordinates: with `A^-1 = T Λ T^-1`, `Λ` holds one real eigenvalue and one complex
//! pair, so each iteration solves one real `n x n` system `(γ/h) I - J` and one complex
//! system `((α + iβ)/h) I - J` instead of a `3n x 3n` one. The Jacobian is the shared
//! colored one of [`StateJac`] (exact by default); it is kept while Newton contracts
//! fast, and the factorizations are kept while the step size stays within `[1, 1.2]`
//! of the last.
//!
//! The error estimate is the embedded one of RADAU5 filtered by `((γ/h) I - J)^-1`,
//! the step size control the predictive (Gustafsson) controller, and the dense
//! output the collocation polynomial (order 3 at any point, 5 at the step ends).

#![allow(clippy::excessive_precision, clippy::needless_range_loop)]

use crate::codegen::Runtime;
use crate::solver::{Integrator, SolverStats, initial_step, rms};
use crate::solver_jac::{ComplexSystem, JacobianMode, RealSystem, StateJac};

const SQ6: f64 = 2.449_489_742_783_178;
const C1: f64 = (4.0 - SQ6) / 10.0;
const C2: f64 = (4.0 + SQ6) / 10.0;
const C1M1: f64 = C1 - 1.0;
const C2M1: f64 = C2 - 1.0;
const C1MC2: f64 = C1 - C2;
const DD1: f64 = -(13.0 + 7.0 * SQ6) / 3.0;
const DD2: f64 = (-13.0 + 7.0 * SQ6) / 3.0;
const DD3: f64 = -1.0 / 3.0;

const T11: f64 = 9.123_239_487_089_294_279_2e-2;
const T12: f64 = -0.141_255_295_020_954_208_43;
const T13: f64 = -3.002_919_410_514_742_449_2e-2;
const T21: f64 = 0.241_717_932_707_107_018_96;
const T22: f64 = 0.204_129_352_293_799_931_99;
const T23: f64 = 0.382_942_112_757_261_937_79;
const T31: f64 = 0.966_048_182_615_092_936_19;
const TI11: f64 = 4.325_579_890_063_155_351_0;
const TI12: f64 = 0.339_199_251_815_809_869_54;
const TI13: f64 = 0.541_770_539_935_874_871_19;
const TI21: f64 = -4.178_718_591_551_904_727_3;
const TI22: f64 = -0.327_682_820_761_062_387_08;
const TI23: f64 = 0.476_623_554_500_550_451_96;
const TI31: f64 = -0.502_872_634_945_786_875_95;
const TI32: f64 = 2.571_926_949_855_605_429_2;
const TI33: f64 = -0.596_039_204_828_224_924_97;

/// Newton iterations per step.
const NIT: usize = 7;
/// Contraction rate below which the Jacobian is reused (RADAU5 suggests 0.001 for
/// cheap Jacobians and 0.1 for costly ones; a Jacobian here costs one tape pass per
/// color group).
const THET: f64 = 0.01;
const SAFE: f64 = 0.9;
/// Step size ratio bounds: `h_new` in `[h / FACL, h * 8]`.
const FACL: f64 = 5.0;
const FACR: f64 = 1.0 / 8.0;
/// Bound on the estimated error of the dense output at the step's midpoint, relative
/// to the tolerance (see [`Radau5::dense_defect`]).
pub(crate) const DENSE_TOL: f64 = 1.0;
/// Keep the step (and the factorizations) when the proposed ratio is in this range.
const QUOT1: f64 = 1.0;
const QUOT2: f64 = 1.2;

/// The eigenvalues of `A^-1`: real `u1`, complex pair `alph ± i beta`.
fn eigen() -> (f64, f64, f64) {
    let c81 = 81f64.cbrt();
    let c9 = 9f64.cbrt();
    let u1 = (6.0 + c81 - c9) / 30.0;
    let alph = (12.0 - c81 + c9) / 60.0;
    let beta = (c81 + c9) * 3f64.sqrt() / 60.0;
    let cno = alph * alph + beta * beta;
    (1.0 / u1, alph / cno, beta / cno)
}

pub struct Radau5 {
    pub rtol: f64,
    /// Absolute tolerance of each state (`tolerance * |nominal|`).
    pub atol: Vec<f64>,
    pub max_step: f64,
    /// Tolerances as RADAU5 uses them internally (`0.1 rtol^(2/3)`).
    rtol_i: f64,
    atol_i: Vec<f64>,
    fnewt: f64,
    n: usize,
    t: f64,
    t_old: f64,
    /// Step size of the next attempt (0: choose an initial one).
    h: f64,
    /// Last accepted step size.
    h_old: f64,
    x: Vec<f64>,
    /// `f(t, x)`.
    f0: Vec<f64>,
    z: [Vec<f64>; 3],
    w: [Vec<f64>; 3],
    fz: [Vec<f64>; 3],
    cont: [Vec<f64>; 4],
    scal: Vec<f64>,
    tmp: Vec<f64>,
    tmp2: Vec<f64>,
    jac: StateJac,
    e1: RealSystem,
    e2: ComplexSystem,
    /// Step size the factorizations belong to (NaN: none).
    lu_h: f64,
    need_jac: bool,
    /// The Jacobian was evaluated at the current point.
    caljac: bool,
    first: bool,
    reject: bool,
    faccon: f64,
    theta: f64,
    h_acc: f64,
    err_acc: f64,
    n_accept: u64,
    u1: f64,
    alph: f64,
    beta: f64,
    stats: SolverStats,
}

impl Radau5 {
    /// `pattern[i]` lists the states derivative `i` depends on.
    pub fn new(
        n: usize,
        rtol: f64,
        atol: Vec<f64>,
        max_step: f64,
        pattern: &[Vec<u32>],
        mode: JacobianMode,
    ) -> Self {
        let rtol_i = 0.1 * rtol.powf(2.0 / 3.0);
        let atol_i: Vec<f64> = atol.iter().map(|a| rtol_i * a / rtol).collect();
        let fnewt = (10.0 * f64::EPSILON / rtol_i).max(0.03f64.min(rtol_i.sqrt()));
        let jac = StateJac::new(n, pattern, mode);
        let e1 = RealSystem::new(&jac);
        let e2 = ComplexSystem::new(&jac);
        let z = || vec![0.0; n];
        let (u1, alph, beta) = eigen();
        Radau5 {
            rtol,
            atol,
            max_step,
            rtol_i,
            atol_i,
            fnewt,
            n,
            t: 0.0,
            t_old: 0.0,
            h: 0.0,
            h_old: 0.0,
            x: z(),
            f0: z(),
            z: [z(), z(), z()],
            w: [z(), z(), z()],
            fz: [z(), z(), z()],
            cont: [z(), z(), z(), z()],
            scal: z(),
            tmp: z(),
            tmp2: z(),
            jac,
            e1,
            e2,
            lu_h: f64::NAN,
            need_jac: true,
            caljac: false,
            first: true,
            reject: false,
            faccon: 1.0,
            theta: THET,
            h_acc: 0.0,
            err_acc: 0.0,
            n_accept: 0,
            u1,
            alph,
            beta,
            stats: SolverStats::default(),
        }
    }

    pub fn n_colors(&self) -> usize {
        self.jac.n_colors()
    }

    fn set_scal(&mut self) {
        for ((s, x), a) in self.scal.iter_mut().zip(&self.x).zip(&self.atol_i) {
            *s = a + self.rtol_i * x.abs();
        }
    }

    /// Factor both iteration matrices for step size `h`.
    fn factor(&mut self, h: f64) -> bool {
        self.stats.lu_factorizations += 2;
        let ok = self.e1.factor(&self.jac, self.u1 / h)
            && self.e2.factor(&self.jac, self.alph / h, self.beta / h);
        self.lu_h = if ok { h } else { f64::NAN };
        ok
    }

    /// Simplified Newton iteration for the stages at step size `h`. Returns the
    /// iteration count on convergence, or the factor to shrink `h` by.
    fn newton(&mut self, rt: &mut Runtime, h: f64) -> Result<usize, f64> {
        let n = self.n;
        let (fac1, alphn, betan) = (self.u1 / h, self.alph / h, self.beta / h);
        // Starting values: zero, or the collocation polynomial of the last step.
        if self.first || self.h_old <= 0.0 {
            for k in 0..3 {
                self.z[k].fill(0.0);
                self.w[k].fill(0.0);
            }
        } else {
            let c3q = h / self.h_old;
            let c1q = C1 * c3q;
            let c2q = C2 * c3q;
            for i in 0..n {
                let (ak1, ak2, ak3) = (self.cont[1][i], self.cont[2][i], self.cont[3][i]);
                let ext = |q: f64| q * (ak1 + (q - C2M1) * (ak2 + (q - C1M1) * ak3));
                let (z1, z2, z3) = (ext(c1q), ext(c2q), ext(c3q));
                self.z[0][i] = z1;
                self.z[1][i] = z2;
                self.z[2][i] = z3;
                self.w[0][i] = TI11 * z1 + TI12 * z2 + TI13 * z3;
                self.w[1][i] = TI21 * z1 + TI22 * z2 + TI23 * z3;
                self.w[2][i] = TI31 * z1 + TI32 * z2 + TI33 * z3;
            }
        }
        self.faccon = self.faccon.max(f64::EPSILON).powf(0.8);
        self.theta = THET;
        let mut dynold = 0.0;
        let mut thqold = 0.0;
        let mut newt = 0;
        let c = [C1, C2, 1.0];
        loop {
            if newt >= NIT {
                return Err(0.5);
            }
            for k in 0..3 {
                for i in 0..n {
                    self.tmp[i] = self.x[i] + self.z[k][i];
                }
                rt.rhs(self.t + c[k] * h, &self.tmp, &mut self.fz[k]);
                self.stats.rhs_evals += 1;
                if rt.error.is_some() || !self.fz[k].iter().all(|v| v.is_finite()) {
                    rt.error = None;
                    return Err(0.5);
                }
            }
            for i in 0..n {
                let (a1, a2, a3) = (self.fz[0][i], self.fz[1][i], self.fz[2][i]);
                let (w1, w2, w3) = (self.w[0][i], self.w[1][i], self.w[2][i]);
                self.z[0][i] = TI11 * a1 + TI12 * a2 + TI13 * a3 - fac1 * w1;
                self.z[1][i] = TI21 * a1 + TI22 * a2 + TI23 * a3 - alphn * w2 + betan * w3;
                self.z[2][i] = TI31 * a1 + TI32 * a2 + TI33 * a3 - alphn * w3 - betan * w2;
            }
            self.e1.solve(&mut self.z[0]);
            {
                let (_, zs) = self.z.split_at_mut(1);
                let (z2, z3) = zs.split_at_mut(1);
                self.e2.solve(&mut z2[0], &mut z3[0]);
            }
            newt += 1;
            let dyno = rms(
                (0..3).flat_map(|k| {
                    let z = &self.z[k];
                    let s = &self.scal;
                    (0..n).map(move |i| z[i] / s[i])
                }),
                3 * n,
            );
            if !dyno.is_finite() {
                return Err(0.5);
            }
            if newt > 1 && newt < NIT {
                let thq = dyno / dynold;
                self.theta = if newt == 2 {
                    thq
                } else {
                    (thq * thqold).sqrt()
                };
                thqold = thq;
                if self.theta < 0.99 {
                    self.faccon = self.theta / (1.0 - self.theta);
                    let dyth =
                        self.faccon * dyno * self.theta.powi((NIT - 1 - newt) as i32) / self.fnewt;
                    if dyth >= 1.0 {
                        let qnewt = dyth.clamp(1e-4, 20.0);
                        return Err(0.8 * qnewt.powf(-1.0 / (4.0 + (NIT - 1 - newt) as f64)));
                    }
                } else {
                    return Err(0.5);
                }
            }
            dynold = dyno.max(f64::EPSILON);
            for i in 0..n {
                let w1 = self.w[0][i] + self.z[0][i];
                let w2 = self.w[1][i] + self.z[1][i];
                let w3 = self.w[2][i] + self.z[2][i];
                self.w[0][i] = w1;
                self.w[1][i] = w2;
                self.w[2][i] = w3;
                self.z[0][i] = T11 * w1 + T12 * w2 + T13 * w3;
                self.z[1][i] = T21 * w1 + T22 * w2 + T23 * w3;
                self.z[2][i] = T31 * w1 + w2;
            }
            if self.faccon * dyno <= self.fnewt {
                return Ok(newt);
            }
        }
    }

    /// The scaled error estimate of the converged stages.
    fn error(&mut self, rt: &mut Runtime, h: f64) -> f64 {
        let n = self.n;
        let (hee1, hee2, hee3) = (DD1 / h, DD2 / h, DD3 / h);
        for i in 0..n {
            let f2 = hee1 * self.z[0][i] + hee2 * self.z[1][i] + hee3 * self.z[2][i];
            self.tmp2[i] = f2;
            self.tmp[i] = f2 + self.f0[i];
        }
        self.e1.solve(&mut self.tmp);
        let mut err = rms(self.tmp.iter().zip(&self.scal).map(|(e, s)| e / s), n).max(1e-10);
        if err >= 1.0 && (self.first || self.reject) {
            // A second, better filtered estimate (Hairer & Wanner IV.8 (8.20)).
            for i in 0..n {
                self.tmp[i] += self.x[i];
            }
            let mut f1 = std::mem::take(&mut self.fz[0]);
            rt.rhs(self.t, &self.tmp, &mut f1);
            self.stats.rhs_evals += 1;
            rt.error = None;
            for i in 0..n {
                self.tmp[i] = f1[i] + self.tmp2[i];
            }
            self.fz[0] = f1;
            self.e1.solve(&mut self.tmp);
            err = rms(self.tmp.iter().zip(&self.scal).map(|(e, s)| e / s), n).max(1e-10);
        }
        if err.is_finite() { err } else { 1e10 }
    }
}

impl Radau5 {
    /// Error estimate of the dense output at the middle of the step just computed (not
    /// yet accepted), relative to the tolerance. The defect `d = u' - f(t, u)` of the
    /// collocation polynomial `u` at the midpoint, filtered by `((γ/h) I - J)^-1`,
    /// approximates the interpolation error both for smooth non-stiff components
    /// (`~ h d`) and for stiff ones that track a quasi-static solution (`~ -d / λ`),
    /// where the step end points are exact and nothing else limits the step.
    fn dense_defect(&mut self, rt: &mut Runtime, h: f64) -> f64 {
        let n = self.n;
        let s = -0.5;
        for i in 0..n {
            let (z1, z2, z3) = (self.z[0][i], self.z[1][i], self.z[2][i]);
            let c0 = self.x[i] + z3;
            let c1 = (z2 - z3) / C2M1;
            let ak = (z1 - z2) / C1MC2;
            let c2 = (ak - c1) / C1M1;
            let c3 = c2 - (ak - z1 / C1) / C2;
            let p = c2 + (s - C1M1) * c3;
            let q = c1 + (s - C2M1) * p;
            let dq = p + (s - C2M1) * c3;
            self.tmp[i] = c0 + s * q;
            self.tmp2[i] = (q + s * dq) / h;
        }
        let mut f = std::mem::take(&mut self.fz[0]);
        rt.rhs(self.t + 0.5 * h, &self.tmp, &mut f);
        self.stats.rhs_evals += 1;
        rt.error = None;
        for i in 0..n {
            self.tmp2[i] -= f[i];
        }
        self.fz[0] = f;
        self.e1.solve(&mut self.tmp2);
        let est = rms(
            self.tmp2
                .iter()
                .zip(&self.tmp)
                .zip(&self.atol)
                .map(|((e, u), a)| e / (a + self.rtol * u.abs())),
            n,
        );
        if est.is_finite() { est } else { 1e10 }
    }
}

impl Integrator for Radau5 {
    fn reset(&mut self, rt: &mut Runtime, t: f64, x: &[f64]) {
        self.t = t;
        self.t_old = t;
        self.x.copy_from_slice(x);
        rt.rhs(t, x, &mut self.f0);
        self.stats.rhs_evals += 1;
        self.h = 0.0;
        self.h_old = 0.0;
        self.first = true;
        self.reject = false;
        self.need_jac = true;
        self.caljac = false;
        self.lu_h = f64::NAN;
        self.faccon = 1.0;
        self.theta = THET;
        self.n_accept = 0;
        self.set_scal();
        for c in self.cont.iter_mut() {
            c.fill(0.0);
        }
        self.cont[0].copy_from_slice(x);
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
            let f0 = self.f0.clone();
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
        let mut n_sing = 0;
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
                self.caljac = true;
                self.lu_h = f64::NAN;
            }
            if self.lu_h != h && !self.factor(h) {
                n_sing += 1;
                if n_sing > 5 {
                    return Err(format!("singular iteration matrix at t = {}", self.t));
                }
                self.stats.rejected += 1;
                self.h = h * 0.5;
                self.reject = true;
                continue;
            }
            let newt = match self.newton(rt, h) {
                Ok(k) => k,
                Err(fac) => {
                    self.stats.rejected += 1;
                    self.stats.newton_failures += 1;
                    self.h = h * fac;
                    self.reject = true;
                    if !self.caljac {
                        self.need_jac = true;
                    }
                    continue;
                }
            };
            let err = self.error(rt, h);
            let fac = SAFE.min(SAFE * (1.0 + 2.0 * NIT as f64) / (newt + 2 * NIT) as f64);
            let mut quot = (err.powf(0.25) / fac).clamp(FACR, FACL);
            let mut hnew = h / quot;
            // The dense output must be as good as the tolerance too.
            let dens = if err < 1.0 {
                self.dense_defect(rt, h) / DENSE_TOL
            } else {
                0.0
            };
            if dens > 1.0 {
                self.stats.rejected += 1;
                self.reject = true;
                self.h = h * (0.9 * dens.powf(-0.25)).clamp(0.2, 0.9);
                continue;
            }
            if err < 1.0 {
                self.first = false;
                self.n_accept += 1;
                // Gustafsson's predictive controller.
                if self.n_accept > 1 {
                    let facgus = ((self.h_acc / h) * (err * err / self.err_acc).powf(0.25) / SAFE)
                        .clamp(FACR, FACL);
                    quot = quot.max(facgus);
                    hnew = h / quot;
                }
                self.h_acc = h;
                self.err_acc = err.max(1e-2);
                self.t_old = self.t;
                self.h_old = h;
                self.t = if last { t_max } else { self.t + h };
                for i in 0..n {
                    let (z1, z2, z3) = (self.z[0][i], self.z[1][i], self.z[2][i]);
                    self.x[i] += z3;
                    self.cont[0][i] = self.x[i];
                    self.cont[1][i] = (z2 - z3) / C2M1;
                    let ak = (z1 - z2) / C1MC2;
                    let acont3 = (ak - z1 / C1) / C2;
                    self.cont[2][i] = (ak - self.cont[1][i]) / C1M1;
                    self.cont[3][i] = self.cont[2][i] - acont3;
                }
                self.set_scal();
                rt.rhs(self.t, &self.x, &mut self.f0);
                self.stats.rhs_evals += 1;
                rt.error = None;
                self.stats.steps += 1;
                hnew = hnew.min(self.max_step);
                if dens > 0.0 {
                    hnew = hnew.min(h * (0.9 * dens.powf(-0.25)).min(FACL));
                }
                if self.reject {
                    hnew = hnew.min(h);
                }
                if last && h < planned {
                    // A step shortened to land on t_max says little about the next one.
                    hnew = hnew.max(planned);
                }
                self.reject = false;
                self.caljac = false;
                let qt = hnew / h;
                if !(self.theta <= THET && (QUOT1..=QUOT2).contains(&qt)) {
                    self.h = hnew;
                }
                if self.theta > THET {
                    self.need_jac = true;
                }
                return Ok(self.t);
            }
            self.stats.rejected += 1;
            self.reject = true;
            self.h = if self.first { h * 0.1 } else { hnew };
            if !self.caljac {
                self.need_jac = true;
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
        if h <= 0.0 || self.h_old <= 0.0 {
            out.copy_from_slice(&self.x);
            return;
        }
        let s = (t - self.t) / self.h_old;
        let c = &self.cont;
        for (i, o) in out.iter_mut().enumerate() {
            *o = c[0][i] + s * (c[1][i] + (s - C2M1) * (c[2][i] + (s - C1M1) * c[3][i]));
        }
    }
    fn stats(&self) -> SolverStats {
        self.stats
    }
}
