//! End-to-end tests: parse, flatten, compile and simulate small models and compare
//! against analytic solutions.

use daedalus_core::frontend::Library;
use daedalus_core::results::Results;
use daedalus_core::sim::{Method, SimOptions, simulate};

fn run(file: &str, model: &str, stop: f64, method: Method) -> Results {
    let path = format!("{}/tests/models/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    let opts = SimOptions {
        stop_time: stop,
        method,
        tolerance: 1e-8,
        interval: 0.01,
        ..SimOptions::from_model(&m)
    };
    simulate(&m, opts).map_err(|e| e.message).unwrap().0
}

fn close(a: f64, b: f64, tol: f64) -> bool {
    (a - b).abs() <= tol * (1.0 + b.abs())
}

#[test]
fn mass_spring_through_connectors() {
    for method in [Method::Bdf, Method::Dopri5, Method::Rk4] {
        let r = run("connectors.mo", "Lib.Test", 3.0, method);
        let w = (100.0f64 / 2.0).sqrt();
        let tol = if method == Method::Rk4 { 1e-3 } else { 1e-5 };
        for t in [0.5, 1.7, 3.0] {
            let s = r.at("mass.s", t).unwrap();
            assert!(
                close(s, (w * t).cos(), tol),
                "{method:?} t={t}: {s} vs {}",
                (w * t).cos()
            );
        }
        // aliases are reconstructed
        assert!(close(
            r.final_value("spring.b.s").unwrap(),
            r.final_value("mass.s").unwrap(),
            1e-12
        ));
    }
}

#[test]
fn index3_pendulum_keeps_constraint() {
    let r = run("index.mo", "T.Pendulum", 0.5, Method::Bdf);
    let x = r.final_value("x").unwrap();
    let y = r.final_value("y").unwrap();
    assert!(close(x * x + y * y, 1.0, 1e-9));
    // energy conservation: m g (0 - y) = m v^2 / 2
    let vx = r.final_value("vx").unwrap();
    let vy = r.final_value("vy").unwrap();
    assert!(close(9.81 * -y, 0.5 * (vx * vx + vy * vy), 1e-4));
}

#[test]
fn index2_gear_constraint() {
    let r = run("index.mo", "T.TwoInertias", 2.0, Method::Bdf);
    assert!(close(r.final_value("w1").unwrap(), 10.0 / 3.0 * 2.0, 1e-6));
    assert!(close(
        r.final_value("phi2").unwrap() * 3.0,
        r.final_value("phi1").unwrap(),
        1e-9
    ));
}

#[test]
fn bouncing_ball_events() {
    let r = run("events.mo", "E.Ball", 2.0, Method::Bdf);
    assert_eq!(r.final_value("bounces").unwrap(), 3.0);
    let g: f64 = 9.81;
    let t1 = (2.0 / g).sqrt();
    let v1 = g * t1 * 0.8;
    let t2 = t1 + 2.0 * v1 / g;
    let v2 = v1 * 0.8;
    let t3 = t2 + 2.0 * v2 / g;
    let v3 = v2 * 0.8;
    let dt = 2.0 - t3;
    let h = v3 * dt - 0.5 * g * dt * dt;
    assert!(
        close(r.final_value("h").unwrap(), h, 1e-6),
        "{} vs {h}",
        r.final_value("h").unwrap()
    );
}

#[test]
fn sampled_input() {
    let r = run("events.mo", "E.Sampled", 1.0, Method::Bdf);
    let x05 = 1.0 - (-0.5f64).exp();
    assert!(close(r.at("x", 0.5).unwrap(), x05, 1e-6));
    let x1 = -1.0 + (x05 + 1.0) * (-0.5f64).exp();
    assert!(close(r.final_value("x").unwrap(), x1, 1e-6));
}

#[test]
fn hysteresis_elsewhen() {
    let r = run("events.mo", "E.Hyst", 3.5, Method::Dopri5);
    assert!(close(r.final_value("x").unwrap(), 0.5, 1e-6));
    assert!(close(r.at("x", 1.5).unwrap(), 0.5, 1e-6));
}

#[test]
fn nonlinear_loop_tearing() {
    let r = run("index.mo", "T.Loop", 1.0, Method::Bdf);
    let x = r.final_value("x").unwrap();
    let y = r.final_value("y").unwrap();
    let z = r.final_value("z").unwrap();
    assert!(close(y, z + (y).sin() * 0.1 + x, 1e-9));
    assert!(close(z, 0.5 * y - x * z, 1e-9));
}

#[test]
fn clocked_partitions() {
    let r = run("clocked.mo", "Clocked", 0.65, Method::Bdf);
    // The fast clock ticks at 0, 0.1, 0.2, ...; the sub-sampled one at 0, 0.3, 0.6.
    assert_eq!(r.at("k", 0.05).unwrap(), 1.0);
    assert_eq!(r.at("k", 0.25).unwrap(), 3.0);
    assert_eq!(r.at("ksh", 0.25).unwrap(), 1.0);
    assert_eq!(r.at("ksh", 0.35).unwrap(), 2.0);
    assert_eq!(r.at("ksh", 0.62).unwrap(), 3.0);
    // Zero-order hold: u = -2 x(0) on [0, 0.1).
    let e = (-0.1f64).exp();
    let x1 = e - 2.0 * (1.0 - e);
    assert!(close(r.at("x", 0.1).unwrap(), x1, 1e-6));
    assert!(close(r.at("u", 0.15).unwrap(), -2.0 * x1, 1e-6));
}

#[test]
fn break_under_time_varying_condition() {
    let r = run("break.mo", "B.Test", 1.0, Method::Bdf);
    for (t, y) in [(0.1, 0.0), (0.3, 1.0), (0.6, 3.0), (0.8, 6.0), (1.0, 10.0)] {
        assert_eq!(r.at("y", t).unwrap(), y, "t = {t}");
    }
}

#[test]
fn return_and_break_under_runtime_conditions() {
    let r = run("functions_control.mo", "FnTest", 1.0, Method::Bdf);
    // firstIdx({time > 0.3, time > 0.6, true}): 3 before 0.3, then 1
    assert_eq!(r.at("k", 0.1).unwrap(), 3.0);
    assert_eq!(r.at("k", 0.9).unwrap(), 1.0);
    // break at a translation-time condition ends the loop (not just the iteration)
    assert_eq!(r.final_value("s").unwrap(), 6.0);
    // break at a run-time condition: count up while y <= 10*time
    assert_eq!(r.at("d", 0.1).unwrap(), 2.0);
    assert_eq!(r.at("d", 0.45).unwrap(), 5.0);
    // early return keeps the value assigned before it
    assert_eq!(r.at("r", 0.1).unwrap(), 10.0);
    assert_eq!(r.at("r", 0.4).unwrap(), 30.0);
    assert_eq!(r.at("r", 0.9).unwrap(), 2.0);
}

#[test]
fn function_arguments_and_partial_application() {
    let r = run("function_args.mo", "FunctionArgs", 1.0, Method::Bdf);
    assert!(close(
        r.final_value("fixedRoot").unwrap(),
        2f64.sqrt(),
        1e-9
    ));
    assert!(close(r.at("movingRoot", 0.5).unwrap(), 1.5f64.sqrt(), 1e-9));
    assert!(close(
        r.final_value("movingRoot").unwrap(),
        2f64.sqrt(),
        1e-9
    ));
}

/// Values of `var` just after each of the first ticks of a clock with period `h`,
/// compared with a hand-computed sequence.
fn ticks(r: &Results, var: &str, h: f64, expected: &[f64]) {
    let got: Vec<f64> = (0..expected.len())
        .map(|k| r.at(var, (k as f64 + 0.5) * h).unwrap())
        .collect();
    assert_eq!(got, expected, "{var}");
}

#[test]
fn state_machine_reference_example() {
    // ModelicaReference.StateMachines on the default clock (1 s).
    let r = run("statemachines.mo", "SM.Reference", 12.0, Method::Bdf);
    let v = [2., 4., 6., 5., 4., 3., 2., 1., 0., 2., 4., 6.];
    ticks(&r, "v", 1.0, &v);
}

#[test]
fn state_machine_immediate_delayed_reset() {
    // a -> b when ticksInState() >= 3, b -> a (delayed) when ticksInState() >= 2.
    let r = run("statemachines.mo", "SM.TwoStates", 1.2, Method::Bdf);
    ticks(&r, "y", 0.1, &[1., 2., -1., -1., 1., 2., -1., -1., 1., 2.]);
    ticks(&r, "z", 0.1, &[0., 0., 3., 4., 0., 0., 7., 8., 0., 0.]);
    // Without reset the local counter of `a` continues.
    let r = run("statemachines.mo", "SM.TwoStatesNoReset", 1.2, Method::Bdf);
    ticks(&r, "y", 0.1, &[1., 2., -1., -1., 3., 4., -1., -1., 5., 6.]);
    // A delayed transition out of `a` takes effect one tick later.
    let r = run("statemachines.mo", "SM.TwoStatesDelayed", 1.2, Method::Bdf);
    ticks(&r, "y", 0.1, &[1., 2., 3., -1., -1., 1., 2., 3., -1., -1.]);
}

#[test]
fn state_machine_priorities_time_and_synchronize() {
    let r = run("statemachines.mo", "SM.Priorities", 8.0, Method::Bdf);
    ticks(&r, "y", 1.0, &[10., 10., 11., 10., 12., 10., 12., 10.]);
    let r = run("statemachines.mo", "SM.Timed", 3.0, Method::Bdf);
    ticks(&r, "y", 0.25, &[1., 1., 1., 2., 2., 1., 1., 1., 2., 2., 1.]);
    let r = run("statemachines.mo", "SM.Sync", 8.0, Method::Bdf);
    ticks(&r, "y", 1.0, &[1., 1., 2., 3., 1., 1., 2., 3.]);
}

#[test]
fn state_machine_hierarchical() {
    let r = run("statemachines.mo", "SM.Hierarchical", 12.0, Method::Bdf);
    let v = [1., 2., 2., 2., 100., 101., 101., 101., 101., 100., 101.];
    ticks(&r, "v", 1.0, &v);
    ticks(
        &r,
        "outer1.c",
        1.0,
        &[1., 2., 3., 4., 4., 1., 2., 3., 4., 4., 1.],
    );
    // Re-entered without reset: the inner machine resumes in `hold`.
    let r = run(
        "statemachines.mo",
        "SM.HierarchicalNoReset",
        12.0,
        Method::Bdf,
    );
    ticks(&r, "v", 1.0, &[1., 2., 2., 2., 100., 100., 100., 100.]);
    ticks(
        &r,
        "outer1.c",
        1.0,
        &[1., 2., 3., 4., 4., 5., 5., 6., 6., 7.],
    );
}

#[test]
fn state_machine_mls_example() {
    // MLS 17.4: hierarchical and parallel state machines.
    let r = run(
        "statemachines.mo",
        "SM.HierarchicalAndParallel",
        30.0,
        Method::Bdf,
    );
    let mut v = vec![2., 4., 6., 5., 4., 3., 2., 1., 0., 0.];
    v.extend([2., 4., 6., 5., 4., 3., 2., 1., 0., 0., 0., 0.]);
    v.extend([5., 10., 15., 20., 2., 4., 6., 5.]);
    ticks(&r, "v", 1.0, &v);
    let mut count = vec![0.; 9];
    count.extend([1.; 10]);
    count.extend([2.; 7]);
    count.extend([0.; 4]);
    ticks(&r, "state1.count", 1.0, &count);
    let mut i: Vec<f64> = (1..=21).map(f64::from).collect();
    i.extend([21., 21., 21., 21., 21., 1., 2., 3., 4.]);
    ticks(&r, "state1.stateX.i", 1.0, &i);
}

#[test]
fn state_machine_errors() {
    let src = std::fs::read_to_string(format!(
        "{}/tests/models/statemachines.mo",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap();
    let mut lib = Library::new();
    assert!(lib.set_document("statemachines.mo", &src).is_empty());
    let err = |model: &str| {
        daedalus_core::translate(&lib, model, &Default::default())
            .err()
            .map(|e| format!("{e:#}"))
            .unwrap_or_default()
    };
    assert!(err("SM.ImmediateLoop").contains("algebraic loop"));
    assert!(err("SM.TicksInStateInState").contains("transition conditions"));
    assert!(err("SM.SamePriority").contains("different priorities"));
}
