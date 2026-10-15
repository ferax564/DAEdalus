//! Linearization: the linear state-space model `der(x) = A x + B u`, `y = C x + D u`
//! of a compiled model around a solution point (after initialization, or after
//! simulating to a given time), like Dymola's `linearizeModel` and OpenModelica's
//! `linearize`.
//!
//! `x` are the selected states, `u` the top-level Real `input`s and `y` the top-level
//! Real `output`s. The matrices are computed exactly by default: a forward-mode
//! (tangent) run of the compiled tape per state and input, differentiating algebraic
//! loops implicitly with their symbolic Jacobians (see [`Runtime::run_tangent`]).
//! Central differences of the compiled right-hand side are available as a cross-check
//! (and are used automatically if the exact tangents are not finite).
//!
//! Relations, discrete variables and `delay()` outputs are held at their values at the
//! linearization point.

use crate::codegen::{Model, Runtime};
use crate::expr::VarId;
use crate::flat::{VarKind, VarType};
use crate::sim::{SimOptions, Simulation};
use serde::Serialize;

/// How the Jacobians are computed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum JacobianMethod {
    /// Forward-mode differentiation of the tape (exact up to rounding).
    Exact,
    /// Central differences of the compiled right-hand side.
    CentralDifference,
}

/// A linear state-space model (row-major matrices as nested vectors).
#[derive(Clone, Debug, Serialize)]
pub struct Linearization {
    pub model: String,
    /// Time of the linearization point.
    pub time: f64,
    pub states: Vec<String>,
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
    /// Values at the linearization point.
    pub x0: Vec<f64>,
    pub u0: Vec<f64>,
    pub y0: Vec<f64>,
    /// State derivatives at the linearization point (zero at an equilibrium).
    pub dx0: Vec<f64>,
    pub a: Vec<Vec<f64>>,
    pub b: Vec<Vec<f64>>,
    pub c: Vec<Vec<f64>>,
    pub d: Vec<Vec<f64>>,
    pub method: JacobianMethod,
}

/// The top-level Real inputs of a model (the `u` of the linearization).
pub fn real_inputs(m: &Model) -> Vec<VarId> {
    m.inputs
        .iter()
        .copied()
        .filter(|&v| m.vars[v as usize].ty == VarType::Real)
        .collect()
}

/// The top-level Real outputs of a model (the `y` of the linearization).
pub fn real_outputs(m: &Model) -> Vec<VarId> {
    (0..m.vars.len() as VarId)
        .filter(|&v| {
            let var = &m.vars[v as usize];
            var.is_output
                && var.ty == VarType::Real
                && var.kind != VarKind::Input
                && !var.name.starts_with('$')
        })
        .collect()
}

/// Simulate `m` to `at` (default: the start time, i.e. right after initialization) and
/// linearize it there. Inputs keep their start values (0 without one) during the run.
pub fn linearize(
    m: &Model,
    opts: SimOptions,
    at: Option<f64>,
    method: JacobianMethod,
) -> Result<Linearization, String> {
    let t_lin = at.unwrap_or(opts.start_time);
    if t_lin < opts.start_time {
        return Err(format!(
            "linearization time {t_lin} is before the start time {}",
            opts.start_time
        ));
    }
    let mut o = opts;
    o.stop_time = t_lin;
    let mut sim = Simulation::new(m, o);
    sim.initialize()?;
    if t_lin > sim.opts.start_time {
        let mut st = sim.stepper();
        sim.advance_to(&mut st, t_lin)?;
        if let Some(msg) = sim.terminated() {
            return Err(format!(
                "the model terminated at t = {} before the linearization time: {msg}",
                sim.rt.time()
            ));
        }
    } else {
        sim.rt.run(&m.ode);
        sim.rt.run(&m.outputs);
    }
    linearize_at(&mut sim.rt, method)
}

/// Linearize at the runtime's current (consistent) solution point.
pub fn linearize_at(rt: &mut Runtime, method: JacobianMethod) -> Result<Linearization, String> {
    let m = rt.m;
    let inputs = real_inputs(m);
    let outputs = real_outputs(m);
    let name = |v: &VarId| m.vars[*v as usize].name.clone();
    let mut lin = Linearization {
        model: m.name.clone(),
        time: rt.time(),
        states: m.states.iter().map(name).collect(),
        inputs: inputs.iter().map(name).collect(),
        outputs: outputs.iter().map(name).collect(),
        x0: m.states.iter().map(|&s| rt.r[s as usize]).collect(),
        u0: inputs.iter().map(|&s| rt.r[s as usize]).collect(),
        y0: outputs.iter().map(|&s| rt.r[s as usize]).collect(),
        dx0: m.derivs.iter().map(|&s| rt.r[s as usize]).collect(),
        a: Vec::new(),
        b: Vec::new(),
        c: Vec::new(),
        d: Vec::new(),
        method,
    };
    let saved = rt.r.clone();
    let seeds: Vec<VarId> = m.states.iter().chain(inputs.iter()).copied().collect();
    let mut cols = match method {
        JacobianMethod::Exact => exact_columns(rt, &seeds, &outputs),
        JacobianMethod::CentralDifference => fd_columns(rt, &seeds, &outputs),
    };
    if method == JacobianMethod::Exact && cols.iter().flatten().any(|v| !v.is_finite()) {
        rt.r.copy_from_slice(&saved);
        cols = fd_columns(rt, &seeds, &outputs);
        lin.method = JacobianMethod::CentralDifference;
    }
    rt.r.copy_from_slice(&saved);
    if let Some(e) = rt.error.take() {
        return Err(format!(
            "nonlinear system for [{}] did not converge while linearizing",
            m.newton[e.block].names.join(", ")
        ));
    }
    // cols[j] = d(derivatives ++ outputs) / d seed_j
    let (n, nu, ny) = (m.states.len(), inputs.len(), outputs.len());
    let row = |i: usize, range: std::ops::Range<usize>| -> Vec<f64> {
        // `+ 0.0` turns -0 into 0
        range.map(|j| cols[j][i] + 0.0).collect()
    };
    lin.a = (0..n).map(|i| row(i, 0..n)).collect();
    lin.b = (0..n).map(|i| row(i, n..n + nu)).collect();
    lin.c = (0..ny).map(|i| row(n + i, 0..n)).collect();
    lin.d = (0..ny).map(|i| row(n + i, n..n + nu)).collect();
    Ok(lin)
}

fn exact_columns(rt: &mut Runtime, seeds: &[VarId], outputs: &[VarId]) -> Vec<Vec<f64>> {
    let m = rt.m;
    let mut dr = vec![0.0; rt.r.len()];
    seeds
        .iter()
        .map(|&s| {
            dr.iter_mut().for_each(|x| *x = 0.0);
            dr[s as usize] = 1.0;
            rt.run_tangent(&m.ode, &mut dr);
            rt.run_tangent(&m.outputs, &mut dr);
            m.derivs
                .iter()
                .chain(outputs.iter())
                .map(|&v| dr[v as usize])
                .collect()
        })
        .collect()
}

fn fd_columns(rt: &mut Runtime, seeds: &[VarId], outputs: &[VarId]) -> Vec<Vec<f64>> {
    let m = rt.m;
    let eval = |rt: &mut Runtime| -> Vec<f64> {
        rt.run(&m.ode);
        rt.run(&m.outputs);
        m.derivs
            .iter()
            .chain(outputs.iter())
            .map(|&v| rt.r[v as usize])
            .collect()
    };
    seeds
        .iter()
        .map(|&s| {
            let x = rt.r[s as usize];
            // Step balancing truncation (h^2) against rounding (eps / h) error.
            let h = 6e-6 * x.abs().max(1.0);
            rt.r[s as usize] = x + h;
            let fp = eval(rt);
            rt.r[s as usize] = x - h;
            let fm = eval(rt);
            rt.r[s as usize] = x;
            let col = fp
                .iter()
                .zip(&fm)
                .map(|(a, b)| (a - b) / (2.0 * h))
                .collect();
            col
        })
        .collect()
}

impl Linearization {
    /// Eigenvalues of `A` as `(re, im)` pairs, sorted by real part, then imaginary part.
    pub fn eigenvalues(&self) -> Vec<(f64, f64)> {
        let mut e = eigenvalues(&self.a);
        e.sort_by(|p, q| p.0.total_cmp(&q.0).then(p.1.total_cmp(&q.1)));
        e
    }

    /// Human-readable report.
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(s, "linearization of {} at t = {}", self.model, self.time);
        let _ = writeln!(
            s,
            "  jacobians: {}",
            match self.method {
                JacobianMethod::Exact => "exact (forward-mode differentiation of the tape)",
                JacobianMethod::CentralDifference => "central differences",
            }
        );
        let list = |s: &mut String, title: &str, sym: &str, names: &[String], vals: &[f64]| {
            let _ = writeln!(s, "\n{title} ({}):", names.len());
            for (i, (n, v)) in names.iter().zip(vals).enumerate() {
                let _ = writeln!(s, "  {sym}[{}] = {n}  ({v:.6e})", i + 1);
            }
        };
        list(&mut s, "states", "x", &self.states, &self.x0);
        list(&mut s, "inputs", "u", &self.inputs, &self.u0);
        list(&mut s, "outputs", "y", &self.outputs, &self.y0);
        let mat = |s: &mut String, name: &str, a: &[Vec<f64>], cols: usize| {
            let _ = writeln!(s, "\n{name} ({} x {}):", a.len(), cols);
            for row in a {
                let cells: Vec<String> = row.iter().map(|v| format!("{v:>13.6e}")).collect();
                let _ = writeln!(s, "  [{}]", cells.join(" "));
            }
        };
        let (n, nu) = (self.states.len(), self.inputs.len());
        mat(&mut s, "A", &self.a, n);
        mat(&mut s, "B", &self.b, nu);
        mat(&mut s, "C", &self.c, n);
        mat(&mut s, "D", &self.d, nu);
        if n > 0 {
            let _ = writeln!(s, "\neigenvalues of A:");
            for (re, im) in self.eigenvalues() {
                if im == 0.0 {
                    let _ = writeln!(s, "  {re:.6e}");
                } else {
                    let _ = writeln!(
                        s,
                        "  {re:.6e} {} {:.6e}i",
                        if im < 0.0 { '-' } else { '+' },
                        im.abs()
                    );
                }
            }
        }
        s
    }

    /// The model in the layout of OpenModelica's `linearize` (`model linearized_model`
    /// with parameters `A, B, C, D, x0, u0`), written in affine form around the
    /// linearization point so that it reproduces it: `der(x) = A*(x - x0) + B*(u - u0)
    /// + dx0; y = y0 + C*(x - x0) + D*(u - u0)` with `x(start = x0)`.
    pub fn to_modelica(&self) -> String {
        use std::fmt::Write;
        let (n, nu, ny) = (self.states.len(), self.inputs.len(), self.outputs.len());
        let num = |v: f64| {
            if v == 0.0 {
                "0".to_string()
            } else if (1e-4..1e6).contains(&v.abs()) {
                format!("{v}")
            } else {
                format!("{v:e}")
            }
        };
        let vector = |v: &[f64]| -> String {
            format!(
                "{{{}}}",
                v.iter().map(|&x| num(x)).collect::<Vec<_>>().join(", ")
            )
        };
        let matrix = |a: &[Vec<f64>], rows: usize, cols: usize| -> String {
            if rows == 0 || cols == 0 {
                return format!("zeros({rows}, {cols})");
            }
            let rows: Vec<String> = a
                .iter()
                .map(|r| r.iter().map(|&x| num(x)).collect::<Vec<_>>().join(", "))
                .collect();
            format!("\n\t[{}]", rows.join(";\n\t"))
        };
        let quote = |prefix: &str, name: &str| {
            let n: String = name
                .chars()
                .map(|c| if c == '\'' || c == '\\' { '_' } else { c })
                .collect();
            format!("'{prefix}_{n}'")
        };
        let mut s = String::new();
        let _ = writeln!(s, "model linearized_model \"{}\"", self.model);
        let _ = writeln!(
            s,
            "  parameter Integer n = {n} \"number of states\";\n  parameter Integer m = {nu} \"number of inputs\";\n  parameter Integer p = {ny} \"number of outputs\";"
        );
        let _ = writeln!(s, "  parameter Real x0[n] = {};", vector(&self.x0));
        let _ = writeln!(s, "  parameter Real u0[m] = {};", vector(&self.u0));
        let _ = writeln!(s, "  parameter Real y0[p] = {};", vector(&self.y0));
        let _ = writeln!(
            s,
            "  parameter Real dx0[n] = {} \"der(x) at the linearization point\";",
            vector(&self.dx0)
        );
        let _ = writeln!(s);
        let _ = writeln!(s, "  parameter Real A[n, n] ={};", matrix(&self.a, n, n));
        let _ = writeln!(s);
        let _ = writeln!(s, "  parameter Real B[n, m] ={};", matrix(&self.b, n, nu));
        let _ = writeln!(s);
        let _ = writeln!(s, "  parameter Real C[p, n] ={};", matrix(&self.c, ny, n));
        let _ = writeln!(s);
        let _ = writeln!(s, "  parameter Real D[p, m] ={};", matrix(&self.d, ny, nu));
        let _ = writeln!(s);
        let _ = writeln!(s, "  Real x[n](start=x0);");
        let _ = writeln!(s, "  input Real u[m](start=u0);");
        let _ = writeln!(s, "  output Real y[p];");
        let _ = writeln!(s);
        for (i, nm) in self.states.iter().enumerate() {
            let _ = writeln!(s, "  Real {} = x[{}];", quote("x", nm), i + 1);
        }
        for (i, nm) in self.inputs.iter().enumerate() {
            let _ = writeln!(s, "  Real {} = u[{}];", quote("u", nm), i + 1);
        }
        for (i, nm) in self.outputs.iter().enumerate() {
            let _ = writeln!(s, "  Real {} = y[{}];", quote("y", nm), i + 1);
        }
        let _ = writeln!(s, "equation");
        let _ = writeln!(s, "  der(x) = A * (x - x0) + B * (u - u0) + dx0;");
        let _ = writeln!(s, "  y = y0 + C * (x - x0) + D * (u - u0);");
        let _ = writeln!(s, "end linearized_model;");
        s
    }
}

/// Eigenvalues of a real square matrix (Hessenberg reduction and the shifted QR
/// algorithm, after EISPACK's `elmhes`/`hqr`). Returns `(re, im)` pairs.
#[allow(unused_assignments)]
pub fn eigenvalues(a: &[Vec<f64>]) -> Vec<(f64, f64)> {
    let n = a.len();
    if n == 0 {
        return Vec::new();
    }
    let mut h: Vec<Vec<f64>> = a.to_vec();
    // Reduction to upper Hessenberg form by stabilised elementary similarity transforms.
    for m in 1..n.saturating_sub(1) {
        let mut x = 0.0f64;
        let mut i = m;
        for j in m..n {
            if h[j][m - 1].abs() > x.abs() {
                x = h[j][m - 1];
                i = j;
            }
        }
        if i != m {
            for j in (m - 1)..n {
                let t = h[i][j];
                h[i][j] = h[m][j];
                h[m][j] = t;
            }
            for row in h.iter_mut() {
                row.swap(i, m);
            }
        }
        if x != 0.0 {
            for i in (m + 1)..n {
                let mut y = h[i][m - 1];
                if y != 0.0 {
                    y /= x;
                    h[i][m - 1] = y;
                    for j in m..n {
                        h[i][j] -= y * h[m][j];
                    }
                    for row in h.iter_mut() {
                        row[m] += y * row[i];
                    }
                }
            }
        }
    }
    for i in 2..n {
        for j in 0..i - 1 {
            h[i][j] = 0.0;
        }
    }
    // hqr
    let mut wr = vec![0.0; n];
    let mut wi = vec![0.0; n];
    let mut anorm = 0.0f64;
    for i in 0..n {
        for j in i.saturating_sub(1)..n {
            anorm += h[i][j].abs();
        }
    }
    let mut nn = n as isize - 1;
    let mut t = 0.0f64;
    let (mut p, mut q, mut r, mut s, mut w, mut x, mut y, mut z) = (
        0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64,
    );
    while nn >= 0 {
        let mut its = 0;
        let mut l;
        loop {
            l = nn;
            while l >= 1 {
                let lu = l as usize;
                s = h[lu - 1][lu - 1].abs() + h[lu][lu].abs();
                if s == 0.0 {
                    s = anorm;
                }
                if h[lu][lu - 1].abs() <= f64::EPSILON * s {
                    h[lu][lu - 1] = 0.0;
                    break;
                }
                l -= 1;
            }
            let nu = nn as usize;
            x = h[nu][nu];
            if l == nn {
                wr[nu] = x + t;
                wi[nu] = 0.0;
                nn -= 1;
                break;
            }
            y = h[nu - 1][nu - 1];
            w = h[nu][nu - 1] * h[nu - 1][nu];
            if l == nn - 1 {
                p = 0.5 * (y - x);
                q = p * p + w;
                z = q.abs().sqrt();
                x += t;
                if q >= 0.0 {
                    z = p + if p >= 0.0 { z.abs() } else { -z.abs() };
                    wr[nu - 1] = x + z;
                    wr[nu] = wr[nu - 1];
                    if z != 0.0 {
                        wr[nu] = x - w / z;
                    }
                    wi[nu - 1] = 0.0;
                    wi[nu] = 0.0;
                } else {
                    wr[nu - 1] = x + p;
                    wr[nu] = x + p;
                    wi[nu - 1] = -z;
                    wi[nu] = z;
                }
                nn -= 2;
                break;
            }
            if its == 60 {
                // No convergence: report what is left as NaN.
                for k in 0..=nu {
                    wr[k] = f64::NAN;
                    wi[k] = f64::NAN;
                }
                nn = -1;
                break;
            }
            if its == 10 || its == 20 {
                t += x;
                for i in 0..=nu {
                    h[i][i] -= x;
                }
                s = h[nu][nu - 1].abs() + h[nu - 1][nu - 2].abs();
                x = 0.75 * s;
                y = x;
                w = -0.4375 * s * s;
            }
            its += 1;
            let lu = l as usize;
            let mut mm = nn - 2;
            while mm >= l {
                let m = mm as usize;
                z = h[m][m];
                r = x - z;
                s = y - z;
                p = (r * s - w) / h[m + 1][m] + h[m][m + 1];
                q = h[m + 1][m + 1] - z - r - s;
                r = h[m + 2][m + 1];
                s = p.abs() + q.abs() + r.abs();
                p /= s;
                q /= s;
                r /= s;
                if mm == l {
                    break;
                }
                let u = h[m][m - 1].abs() * (q.abs() + r.abs());
                let v = p.abs() * (h[m - 1][m - 1].abs() + z.abs() + h[m + 1][m + 1].abs());
                if u <= f64::EPSILON * v {
                    break;
                }
                mm -= 1;
            }
            let m = mm as usize;
            for i in (m + 2)..=nu {
                h[i][i - 2] = 0.0;
                if i != m + 2 {
                    h[i][i - 3] = 0.0;
                }
            }
            let mut k = m;
            while k + 1 <= nu {
                if k != m {
                    p = h[k][k - 1];
                    q = h[k + 1][k - 1];
                    r = 0.0;
                    if k + 1 != nu {
                        r = h[k + 2][k - 1];
                    }
                    x = p.abs() + q.abs() + r.abs();
                    if x != 0.0 {
                        p /= x;
                        q /= x;
                        r /= x;
                    }
                }
                let sq = (p * p + q * q + r * r).sqrt();
                s = if p >= 0.0 { sq } else { -sq };
                if s != 0.0 {
                    if k == m {
                        if l as usize != m {
                            h[k][k - 1] = -h[k][k - 1];
                        }
                    } else {
                        h[k][k - 1] = -s * x;
                    }
                    p += s;
                    x = p / s;
                    y = q / s;
                    z = r / s;
                    q /= p;
                    r /= p;
                    for j in k..=nu {
                        p = h[k][j] + q * h[k + 1][j];
                        if k + 1 != nu {
                            p += r * h[k + 2][j];
                            h[k + 2][j] -= p * z;
                        }
                        h[k + 1][j] -= p * y;
                        h[k][j] -= p * x;
                    }
                    let mmin = if nu < k + 3 { nu } else { k + 3 };
                    for i in lu..=mmin {
                        p = x * h[i][k] + y * h[i][k + 1];
                        if k + 1 != nu {
                            p += z * h[i][k + 2];
                            h[i][k + 2] -= p * r;
                        }
                        h[i][k + 1] -= p * q;
                        h[i][k] -= p;
                    }
                }
                k += 1;
            }
            let _ = lu;
        }
    }
    wr.into_iter().zip(wi).collect()
}
