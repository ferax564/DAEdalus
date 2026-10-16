//! Physics preflight: plausibility checks of a translated model before anyone trusts its
//! results. Meant for an AI assistant (the `physics_check` MCP tool) and for people:
//! every finding names the variable and says what to look at.
//!
//! The checks are cheap and conservative:
//! - parameters without any value (reported by the translator, they default to 0);
//! - physical parameters whose unit says they must be positive (mass, inertia,
//!   inductance, capacitance, resistance, stiffness, damping, time constants, lengths,
//!   absolute temperatures and pressures) but are zero or negative;
//! - states that start from a guessed 0 without a start value;
//! - the initial point: derivatives that are not finite, unstable modes, and time
//!   constants far below the output interval;
//! - a short simulation: solver failure, values that blow up, absolute temperatures or
//!   pressures that go negative.

use crate::codegen::Model;
use crate::flat::{VarKind, VarType};
use crate::sim::SimOptions;
use std::fmt::Write;

/// How bad a finding is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
    Note,
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct Finding {
    pub level: Level,
    /// The variable the finding is about (empty for the model as a whole).
    pub variable: String,
    pub message: String,
}

#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct Report {
    pub model: String,
    pub findings: Vec<Finding>,
    /// Checks that ran (for the summary line).
    pub checks: Vec<String>,
}

impl Report {
    fn push(&mut self, level: Level, variable: &str, message: impl Into<String>) {
        self.findings.push(Finding {
            level,
            variable: variable.to_string(),
            message: message.into(),
        });
    }

    pub fn count(&self, level: Level) -> usize {
        self.findings.iter().filter(|f| f.level == level).count()
    }

    /// Plain-text report, errors first.
    pub fn text(&self) -> String {
        let mut out = format!(
            "physics check of {}: {} errors, {} warnings, {} notes\n",
            self.model,
            self.count(Level::Error),
            self.count(Level::Warning),
            self.count(Level::Note)
        );
        let mut f = self.findings.clone();
        f.sort_by_key(|x| x.level);
        for x in &f {
            let tag = match x.level {
                Level::Error => "error",
                Level::Warning => "warning",
                Level::Note => "note",
            };
            if x.variable.is_empty() {
                let _ = writeln!(out, "  {tag}: {}", x.message);
            } else {
                let _ = writeln!(out, "  {tag}: {}: {}", x.variable, x.message);
            }
        }
        let _ = writeln!(out, "checked: {}", self.checks.join(", "));
        out
    }
}

/// Units whose quantities are positive in any physical model: (unit, what, zero allowed).
const POSITIVE_UNITS: &[(&str, &str, bool)] = &[
    ("kg", "a mass", false),
    ("kg.m2", "an inertia", false),
    ("H", "an inductance", false),
    ("F", "a capacitance", false),
    ("Ohm", "a resistance", true),
    ("S", "a conductance", true),
    ("N/m", "a stiffness", true),
    ("N.m/rad", "a stiffness", true),
    ("N.s/m", "a damping", true),
    ("N.m.s/rad", "a damping", true),
    ("K", "an absolute temperature", false),
    ("m3", "a volume", false),
    ("m2", "an area", false),
    ("J/K", "a heat capacity", false),
    ("J/(kg.K)", "a specific heat capacity", false),
    ("W/K", "a thermal conductance", true),
    ("kg/m3", "a density", false),
    ("Pa.s", "a viscosity", true),
];

/// Variables (not parameters) that can never go below zero.
const NONNEGATIVE_UNITS: &[(&str, &str)] = &[("K", "absolute temperature"), ("kg/m3", "density")];

/// Three significant digits.
fn sig(x: f64) -> String {
    if x == 0.0 || !x.is_finite() {
        return format!("{x}");
    }
    let e = x.abs().log10().floor() as i32;
    if (-3..6).contains(&e) {
        format!("{:.*}", (2 - e).max(0) as usize, x)
    } else {
        format!("{x:.2e}")
    }
}

fn positive_unit(unit: &str) -> Option<(&'static str, bool)> {
    let u = unit.replace(' ', "");
    POSITIVE_UNITS
        .iter()
        .find(|(n, ..)| *n == u)
        .map(|&(_, what, zero)| (what, zero))
}

/// Run the checks. `simulate` adds a run over the experiment (bounded by `max_wall`
/// seconds of wall-clock time).
pub fn check(m: &Model, opts: &SimOptions, simulate: bool, max_wall: f64) -> Report {
    let mut r = Report {
        model: m.name.clone(),
        ..Default::default()
    };
    for w in &m.warnings {
        r.push(Level::Warning, "", format!("translation: {w}"));
    }

    // Parameters: missing values and signs.
    r.checks.push("parameter values and signs".into());
    let mut overrides = SimOptions::from_model(m);
    overrides.params = opts.params.clone();
    let values = crate::uq::param_values(
        m,
        &overrides,
        &m.param_vars
            .iter()
            .map(|&v| m.vars[v as usize].name.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap_or_else(|_| crate::fmi::parameter_values(m));
    let value_of = |i: usize, k: usize| -> f64 {
        // `values` is either per parameter (uq) or per variable (fmi fallback)
        if values.len() == m.param_vars.len() {
            values[k]
        } else {
            values[i]
        }
    };
    for (k, &v) in m.param_vars.iter().enumerate() {
        let var = &m.vars[v as usize];
        if var.generated || var.name.starts_with('$') || var.ty != VarType::Real {
            continue;
        }
        let x = value_of(v as usize, k);
        if !x.is_finite() {
            r.push(Level::Error, &var.name, format!("parameter value is {x}"));
            continue;
        }
        if let Some((what, zero_ok)) = positive_unit(&var.unit) {
            if x < 0.0 {
                r.push(
                    Level::Error,
                    &var.name,
                    format!("{what} ({}) cannot be negative but is {x}", var.unit),
                );
            } else if x == 0.0 && !zero_ok {
                r.push(
                    Level::Note,
                    &var.name,
                    format!(
                        "{what} ({}) is 0: fine for an added amount, a mistake otherwise",
                        var.unit
                    ),
                );
            }
        }
    }

    // States without a start value.
    r.checks.push("state start values".into());
    let mut guessed = Vec::new();
    for &s in &m.states {
        let var = &m.vars[s as usize];
        if var.start.is_none() && var.fixed != Some(true) && !var.generated {
            guessed.push(var.name.clone());
        }
    }
    if !guessed.is_empty() {
        let shown: Vec<&str> = guessed.iter().take(10).map(String::as_str).collect();
        r.push(
            Level::Note,
            "",
            format!(
                "{} states have no start value and start from 0 unless initial equations fix them: {}{}",
                guessed.len(),
                shown.join(", "),
                if guessed.len() > 10 { ", ..." } else { "" }
            ),
        );
    }

    // The initial point.
    r.checks.push("initial derivatives and modes".into());
    let mut lo = opts.clone();
    if lo.max_wall_time <= 0.0 || lo.max_wall_time > max_wall {
        lo.max_wall_time = max_wall;
    }
    match crate::linearize::linearize(m, lo.clone(), None, crate::linearize::JacobianMethod::Exact)
    {
        Ok(lin) => {
            for (i, d) in lin.dx0.iter().enumerate() {
                if !d.is_finite() {
                    r.push(
                        Level::Error,
                        &lin.states[i],
                        format!("der({}) is {d} at the start", lin.states[i]),
                    );
                }
            }
            if lin.dx0.iter().all(|d| d.is_finite()) && !lin.a.is_empty() {
                let eig = lin.eigenvalues();
                // Growth that matters over the run (more than 10 %), not round-off.
                let span = (opts.stop_time - opts.start_time).max(1e-9);
                let unstable: Vec<(f64, f64)> = eig
                    .iter()
                    .copied()
                    .filter(|e| e.0 > 1e-9 && e.0 * span > 0.1)
                    .collect();
                if !unstable.is_empty() {
                    let fastest = unstable.iter().map(|e| e.0).fold(0.0, f64::max);
                    r.push(
                        Level::Warning,
                        "",
                        format!(
                            "{} unstable mode(s) at the start (largest growth rate {} 1/s, doubling in {} s): fine for an open-loop unstable plant, a sign error otherwise",
                            unstable.len(),
                            sig(fastest),
                            sig(std::f64::consts::LN_2 / fastest)
                        ),
                    );
                }
                let fastest = eig
                    .iter()
                    .map(|e| (e.0 * e.0 + e.1 * e.1).sqrt())
                    .fold(0.0, f64::max);
                let span = opts.stop_time - opts.start_time;
                if fastest > 0.0 && span > 0.0 && 1.0 / fastest < 1e-6 * span {
                    r.push(
                        Level::Note,
                        "",
                        format!(
                            "fastest time constant {} s is a million times shorter than the run ({span} s): stiff, use bdf, radau or esdirk",
                            sig(1.0 / fastest)
                        ),
                    );
                }
            }
        }
        Err(e) => r.push(Level::Error, "", format!("initialization failed: {e}")),
    }

    if simulate {
        r.checks.push("simulation over the experiment".into());
        let mut so = opts.clone();
        so.max_wall_time = max_wall;
        let (res, err) = match crate::sim::simulate(m, so) {
            Ok((res, _)) => (res, None),
            Err(e) => (e.partial.unwrap_or_default(), Some(e.message)),
        };
        if let Some(e) = err {
            r.push(Level::Error, "", format!("simulation failed: {e}"));
        }
        for var in &m.vars {
            if var.ty != VarType::Real
                || var.generated
                || var.name.starts_with('$')
                || matches!(var.kind, VarKind::Parameter | VarKind::Constant)
            {
                continue;
            }
            let Some(ys) = res.values(&var.name) else {
                continue;
            };
            if let Some(bad) = ys.iter().position(|y| !y.is_finite()) {
                // A ratio that is 0/0 before anything has happened is a blemish; a value
                // that blows up later is an error.
                let later = ys
                    .iter()
                    .zip(&res.time)
                    .position(|(y, &t)| !y.is_finite() && t > res.time[0]);
                match later {
                    Some(i) => r.push(
                        Level::Error,
                        &var.name,
                        format!("becomes {} at t = {}", ys[i], res.time[i]),
                    ),
                    None => r.push(
                        Level::Warning,
                        &var.name,
                        format!(
                            "is {} at the start (t = {}), e.g. 0/0 before anything has happened",
                            ys[bad], res.time[bad]
                        ),
                    ),
                }
                continue;
            }
            let unit = var.unit.replace(' ', "");
            if let Some((_, what)) = NONNEGATIVE_UNITS.iter().find(|(u, _)| *u == unit) {
                let (i, mn) =
                    ys.iter()
                        .copied()
                        .enumerate()
                        .fold(
                            (0, f64::INFINITY),
                            |a, (i, y)| if y < a.1 { (i, y) } else { a },
                        );
                if mn < 0.0 {
                    r.push(
                        Level::Error,
                        &var.name,
                        format!(
                            "{what} goes negative ({} {} at t = {})",
                            sig(mn),
                            var.unit,
                            res.time[i]
                        ),
                    );
                }
            }
            if unit == "Pa" && var.name.ends_with(".p") {
                let mn = ys.iter().copied().fold(f64::INFINITY, f64::min);
                if mn < 0.0 {
                    r.push(
                        Level::Warning,
                        &var.name,
                        format!(
                            "pressure goes negative ({} Pa): cavitation, or a sign error",
                            sig(mn)
                        ),
                    );
                }
            }
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model(src: &str, name: &str) -> Model {
        let mut lib = crate::frontend::Library::new();
        assert!(lib.set_document("t.mo", src).is_empty());
        crate::translate(&lib, name, &Default::default()).unwrap()
    }

    #[test]
    fn finds_bad_parameters_and_states() {
        let m = model(
            "model M
               parameter Real m(unit=\"kg\") = -1;
               parameter Real k(unit=\"N/m\");
               parameter Real T(unit=\"K\") = 300;
               Real x(start=1, fixed=true);
               Real v;
               Real temp(unit=\"K\");
             equation
               m*der(v) = -k*x;
               der(x) = v;
               der(temp) = -100;
               annotation(experiment(StopTime=5));
             end M;",
            "M",
        );
        let o = SimOptions::from_model(&m);
        let r = check(&m, &o, true, 10.0);
        let t = r.text();
        assert!(
            t.contains("m: a mass (kg) cannot be negative but is -1"),
            "{t}"
        );
        assert!(t.contains("parameter 'k' has no value"), "{t}");
        assert!(
            t.contains("temp: absolute temperature goes negative"),
            "{t}"
        );
        assert!(t.contains("states have no start value"), "{t}");
        assert!(!t.contains("T:"), "{t}");
    }

    #[test]
    fn flags_unstable_modes() {
        let m = model(
            "model U
               parameter Real a = 2;
               Real x(start=1, fixed=true);
             equation
               der(x) = a*x;
             end U;",
            "U",
        );
        let o = SimOptions::from_model(&m);
        let r = check(&m, &o, false, 10.0);
        assert!(r.text().contains("1 unstable mode"), "{}", r.text());
        assert_eq!(r.count(Level::Error), 0);
    }
}
