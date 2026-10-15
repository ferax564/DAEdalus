//! Steady states and trim against hand-derived equilibria.

use daedalus_core::frontend::Library;
use daedalus_core::sim::SimOptions;
use daedalus_core::steady::{SteadyOptions, SteadyState, steady_state};

fn solve(model: &str, so: SteadyOptions, params: &[(&str, f64)]) -> Result<SteadyState, String> {
    let path = format!("{}/tests/models/steady.mo", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    let mut opts = SimOptions::from_model(&m);
    opts.params = params.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    steady_state(&m, opts, &so)
}

#[track_caller]
fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol * (1.0 + b.abs()), "{a} vs {b}");
}

fn trim(free: &[&str], targets: &[(&str, f64)]) -> SteadyOptions {
    SteadyOptions {
        free: free.iter().map(|s| s.to_string()).collect(),
        targets: targets.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        ..Default::default()
    }
}

#[test]
fn first_order_equilibrium() {
    let s = solve("Steady.FirstOrder", Default::default(), &[]).unwrap();
    close(s.value("x").unwrap(), 5.0, 1e-12);
    close(s.value("y").unwrap(), 5.0, 1e-12);
    assert!(s.value("der(x)").unwrap().abs() < 1e-12);
    // linear: one Newton step
    assert!(s.iterations <= 2, "{}", s.iterations);
    assert!(s.stable);
    close(s.eigenvalues[0].0, -1.0 / 0.7, 1e-12);
    // with a parameter override
    let s = solve("Steady.FirstOrder", Default::default(), &[("k", 4.0)]).unwrap();
    close(s.value("x").unwrap(), 8.0, 1e-12);
}

#[test]
fn nonlinear_tank() {
    let s = solve("Steady.Tank", Default::default(), &[]).unwrap();
    close(s.value("h").unwrap(), 4.0, 1e-10);
}

#[test]
fn pendulum_with_torque() {
    let s = solve("Steady.Pendulum", Default::default(), &[]).unwrap();
    let phi = (2.0f64 / (9.81 * 0.8)).asin();
    close(s.value("phi").unwrap(), phi, 1e-10);
    close(s.value("w").unwrap(), 0.0, 1e-10);
    close(s.value("x").unwrap(), 0.8 * phi.sin(), 1e-10);
    assert!(s.stable);
    assert_eq!(s.eigenvalues.len(), 2);
}

#[test]
fn simulate_first_then_solve() {
    // From x = 0.1 Newton finds the unstable equilibrium 0 ...
    let s = solve("Steady.Bistable", Default::default(), &[]).unwrap();
    close(s.value("x").unwrap(), 0.0, 1e-10);
    assert!(!s.stable);
    // ... after simulating to t = 5 it finds the stable one.
    let so = SteadyOptions {
        at_time: Some(5.0),
        ..Default::default()
    };
    let s = solve("Steady.Bistable", so, &[]).unwrap();
    close(s.time, 5.0, 0.0);
    close(s.value("x").unwrap(), 1.0, 1e-10);
    assert!(s.stable);
}

#[test]
fn algebraic_loop() {
    let s = solve("Steady.Loop", Default::default(), &[]).unwrap();
    // u = a^3 => a = 1, x = 1 + 0.3 sin(1)
    close(s.value("y").unwrap(), 1.0, 1e-10);
    close(s.value("x").unwrap(), 1.0 + 0.3 * 1f64.sin(), 1e-10);
}

#[test]
fn singular_jacobian_keeps_undetermined_states() {
    let s = solve("Steady.FreeMass", Default::default(), &[]).unwrap();
    close(s.value("vel").unwrap(), 0.0, 1e-10);
    assert!(s.value("s").unwrap().is_finite());
    assert!(s.residual < 1e-10, "{}", s.residual);
}

#[test]
fn trim_input_for_output_target() {
    let s = solve("Steady.FirstOrder", trim(&["u"], &[("y", 3.0)]), &[]).unwrap();
    close(s.value("u").unwrap(), 1.2, 1e-12);
    close(s.value("x").unwrap(), 3.0, 1e-12);
    assert_eq!(s.targets[0].name, "y");
    // pendulum: the torque that holds the bob at x = 0.4
    let s = solve("Steady.Pendulum", trim(&["tau"], &[("x", 0.4)]), &[]).unwrap();
    let phi = (0.4f64 / 0.8).asin();
    close(s.value("phi").unwrap(), phi, 1e-10);
    close(s.value("tau").unwrap(), 9.81 * 0.8 * phi.sin(), 1e-10);
}

#[test]
fn trim_parameter_with_dependent_parameters() {
    // h = (q / c)^2 = 9 with q = 1 => c = 1/3, reaching the equation through cEff = 2 c
    let s = solve("Steady.Tank", trim(&["c"], &[("level", 9.0)]), &[]).unwrap();
    close(s.value("c").unwrap(), 1.0 / 3.0, 1e-10);
    close(s.value("h").unwrap(), 9.0, 1e-10);
}

#[test]
fn trim_errors() {
    let e = solve("Steady.Tank", trim(&["c", "q"], &[("level", 9.0)]), &[]).unwrap_err();
    assert!(e.contains("as many targets"), "{e}");
    let e = solve("Steady.Tank", trim(&["h"], &[("level", 9.0)]), &[]).unwrap_err();
    assert!(e.contains("cannot be freed"), "{e}");
    let e = solve("Steady.Tank", trim(&["nope"], &[("level", 9.0)]), &[]).unwrap_err();
    assert!(e.contains("no variable"), "{e}");
}
