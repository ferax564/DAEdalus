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
