//! Automatic switching between a non-stiff and a stiff method (`--method auto`), in
//! the spirit of LSODAR.
//!
//! The run starts with [`Dopri5`]. After every accepted step Dopri5 estimates
//! `h |λ|` from its last two stages, which share the same time (Hairer's stiffness
//! test, Solving ODEs II, IV.2): when the estimate exceeds the stability limit 3.25
//! on 15 accepted steps, without 6 non-stiff steps in a row in between, the step size
//! is limited by stability rather than accuracy and the run switches to [`Bdf`].
//!
//! On BDF the spectral radius `ρ` of the Jacobian (power iteration, refreshed when
//! the Jacobian is) tells whether Dopri5 could take the current step: after at least
//! 20 BDF steps, 15 accepted steps in a row with `h ρ < 1` switch back.
//!
//! Switches happen between steps: the new method restarts from the current point,
//! and dense output of the last step still comes from the method that took it.

use crate::codegen::Runtime;
use crate::solver::{Bdf, Dopri5, Integrator, SolverStats};

/// Accepted stiff-looking Dopri5 steps that trigger the switch to BDF.
const STIFF_HITS: u32 = 15;
/// Consecutive non-stiff Dopri5 steps that clear the count.
const NONSTIFF_RESET: u32 = 6;
/// Minimum BDF steps before switching back.
const MIN_STIFF_STEPS: u64 = 20;
/// Consecutive BDF steps with `h ρ` below [`BACK_LIMIT`] that switch back.
const CALM_STEPS: u32 = 15;
const BACK_LIMIT: f64 = 1.0;
/// Switch messages written to the log (later switches are only counted).
const MAX_LOG: u64 = 20;

pub struct AutoSwitch {
    dopri: Dopri5,
    bdf: Bdf,
    stiff: bool,
    pending: Option<bool>,
    iasti: u32,
    nonsti: u32,
    calm: u32,
    steps_in_mode: u64,
    /// Spectral radius of the BDF Jacobian, and the Jacobian count it belongs to.
    rho: f64,
    rho_jac: u64,
    switches: u64,
    log: Vec<String>,
}

impl AutoSwitch {
    pub fn new(dopri: Dopri5, bdf: Bdf) -> Self {
        AutoSwitch {
            dopri,
            bdf,
            stiff: false,
            pending: None,
            iasti: 0,
            nonsti: 0,
            calm: 0,
            steps_in_mode: 0,
            rho: 0.0,
            rho_jac: u64::MAX,
            switches: 0,
            log: Vec::new(),
        }
    }

    /// Whether the stiff method is active.
    pub fn is_stiff(&self) -> bool {
        self.stiff
    }

    fn active(&self) -> &dyn Integrator {
        if self.stiff { &self.bdf } else { &self.dopri }
    }

    fn switch(&mut self, rt: &mut Runtime, to_stiff: bool, why: String) {
        let t = self.active().t();
        let x = self.active().x().to_vec();
        self.stiff = to_stiff;
        if to_stiff {
            self.bdf.reset(rt, t, &x);
        } else {
            self.dopri.reset(rt, t, &x);
        }
        self.switches += 1;
        self.steps_in_mode = 0;
        self.iasti = 0;
        self.nonsti = 0;
        self.calm = 0;
        if self.switches <= MAX_LOG {
            self.log.push(format!(
                "auto: switched to {} at t = {t} ({why})",
                if to_stiff {
                    "BDF (stiff)"
                } else {
                    "Dopri5 (non-stiff)"
                }
            ));
        } else if self.switches == MAX_LOG + 1 {
            self.log
                .push("auto: further switches are only counted in the statistics".into());
        }
    }
}

impl Integrator for AutoSwitch {
    fn reset(&mut self, rt: &mut Runtime, t: f64, x: &[f64]) {
        if let Some(to) = self.pending.take() {
            self.stiff = to;
            self.switches += 1;
            self.steps_in_mode = 0;
        }
        if self.stiff {
            self.bdf.reset(rt, t, x);
        } else {
            self.dopri.reset(rt, t, x);
        }
        self.calm = 0;
    }

    fn step(&mut self, rt: &mut Runtime, t_max: f64) -> Result<f64, String> {
        if let Some(to) = self.pending.take() {
            let why = if to {
                format!(
                    "step size limited by stability on {STIFF_HITS} steps, h|λ| = {:.3}",
                    self.dopri.hlamb
                )
            } else {
                format!("h ρ(J) = {:.3} on {CALM_STEPS} steps", {
                    let h = self.bdf.t() - self.bdf.t_prev();
                    h * self.rho
                })
            };
            self.switch(rt, to, why);
        }
        if !self.stiff {
            let t = self.dopri.step(rt, t_max)?;
            self.steps_in_mode += 1;
            if self.dopri.hlamb > 3.25 {
                self.nonsti = 0;
                self.iasti += 1;
                if self.iasti >= STIFF_HITS {
                    self.pending = Some(true);
                }
            } else {
                self.nonsti += 1;
                if self.nonsti >= NONSTIFF_RESET {
                    self.iasti = 0;
                }
            }
            return Ok(t);
        }
        let t = self.bdf.step(rt, t_max)?;
        self.steps_in_mode += 1;
        let jac_evals = self.bdf.stats().jac_evals;
        if jac_evals != self.rho_jac {
            self.rho = self.bdf.jacobian().spectral_radius();
            self.rho_jac = jac_evals;
        }
        let h = t - self.bdf.t_prev();
        if h * self.rho < BACK_LIMIT && t < t_max {
            self.calm += 1;
            if self.calm >= CALM_STEPS && self.steps_in_mode >= MIN_STIFF_STEPS {
                self.pending = Some(false);
            }
        } else {
            self.calm = 0;
        }
        Ok(t)
    }

    fn t(&self) -> f64 {
        self.active().t()
    }
    fn t_prev(&self) -> f64 {
        self.active().t_prev()
    }
    fn x(&self) -> &[f64] {
        self.active().x()
    }
    fn interpolate(&self, t: f64, out: &mut [f64]) {
        self.active().interpolate(t, out)
    }
    fn stats(&self) -> SolverStats {
        let mut s = self.dopri.stats().add(&self.bdf.stats());
        s.switches = self.switches;
        s
    }
    fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
}
