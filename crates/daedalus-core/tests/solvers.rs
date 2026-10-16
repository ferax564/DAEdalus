//! The solver suite: Radau IIA, ESDIRK, automatic stiff/non-stiff switching and the
//! exact state Jacobian, against reference solutions.

use daedalus_core::codegen::Model;
use daedalus_core::frontend::Library;
use daedalus_core::results::Results;
use daedalus_core::sim::{JacobianMode, Method, SimOptions, SimStats, Simulation, simulate};
use daedalus_core::solver::SolverStats;
use daedalus_core::solver_jac::StateJac;

fn compile(file: &str, model: &str) -> Model {
    let path = format!("{}/tests/models/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    daedalus_core::translate(&lib, model, &Default::default()).unwrap()
}

fn run_opts(m: &Model, opts: SimOptions) -> (Results, SimStats) {
    simulate(m, opts).map_err(|e| e.message).unwrap()
}

fn run(model: &str, stop: f64, method: Method, tol: f64) -> (Results, SimStats) {
    let m = compile("stiff.mo", model);
    let opts = SimOptions {
        stop_time: stop,
        method,
        tolerance: tol,
        intervals: 100,
        ..SimOptions::from_model(&m)
    };
    run_opts(&m, opts)
}

const STIFF: [Method; 4] = [Method::Bdf, Method::Radau, Method::Esdirk, Method::Auto];

fn rel(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs().max(1e-300)
}

#[test]
fn robertson_reference() {
    // Reference at t = 40 (Hairer & Wanner, Solving ODEs II, IV.10).
    let (y1, y2, y3) = (0.715_827_068_7, 9.185_534_765e-6, 0.284_163_745_7);
    for method in STIFF {
        let (r, s) = run("S.Robertson", 40.0, method, 1e-8);
        let f = |n: &str| r.final_value(n).unwrap();
        assert!((f("y1") - y1).abs() < 1e-6, "{method:?}: y1 = {}", f("y1"));
        assert!((f("y3") - y3).abs() < 1e-6, "{method:?}: y3 = {}", f("y3"));
        assert!(rel(f("y2"), y2) < 1e-3, "{method:?}: y2 = {}", f("y2"));
        assert!(
            s.solver.steps < 3000,
            "{method:?}: {} steps",
            s.solver.steps
        );
        println!("robertson {method:?}: {:?}", s.solver);
    }
}

#[test]
fn robertson_long_time() {
    // t = 1e11 (Hairer's test set ROBER): y1 = 2.083340149701255e-8, y3 near 1.
    for method in [Method::Bdf, Method::Radau, Method::Esdirk] {
        let m = compile("stiff.mo", "S.Robertson");
        let opts = SimOptions {
            stop_time: 1e11,
            method,
            tolerance: 1e-10,
            intervals: 50,
            ..SimOptions::from_model(&m)
        };
        let (r, s) = run_opts(&m, opts);
        let f = |n: &str| r.final_value(n).unwrap();
        assert!(
            rel(f("y1"), 2.083_340_149_701_255e-8) < 0.05,
            "{method:?}: y1 = {}",
            f("y1")
        );
        assert!((f("y3") - 0.999_999_979_166_505).abs() < 1e-7, "{method:?}");
        println!("robertson 1e11 {method:?}: {:?}", s.solver);
    }
}

#[test]
fn van_der_pol_mu_1000() {
    // VDPOL of the Hairer & Wanner test set, eps = 1e-6 (mu = 1000), at t = 2.
    let (y1, y2) = (1.706_167_732_170_483, -0.892_809_701_024_812_5);
    for method in STIFF {
        let (r, s) = run("S.VanDerPol", 2.0, method, 1e-7);
        let f = |n: &str| r.final_value(n).unwrap();
        assert!(rel(f("y1"), y1) < 1e-4, "{method:?}: y1 = {}", f("y1"));
        assert!(rel(f("y2"), y2) < 1e-3, "{method:?}: y2 = {}", f("y2"));
        println!("vdpol {method:?}: {:?}", s.solver);
    }
    // Auto starts explicit and must see the stiffness.
    let (r, s) = run("S.VanDerPol", 2.0, Method::Auto, 1e-7);
    assert!(s.solver.switches >= 1);
    assert!(
        r.log.iter().any(|l| l.contains("switched to BDF")),
        "{:?}",
        r.log
    );
    // The explicit method alone reports the stiff signature.
    let m = compile("stiff.mo", "S.VanDerPol");
    let opts = SimOptions {
        stop_time: 0.05,
        method: Method::Dopri5,
        tolerance: 1e-6,
        ..SimOptions::from_model(&m)
    };
    let (_, s) = run_opts(&m, opts);
    assert!(
        s.solver.stiff_steps * 2 > s.solver.steps,
        "{} of {}",
        s.solver.stiff_steps,
        s.solver.steps
    );
}

#[test]
fn prothero_robinson_stiff_accuracy() {
    // L-stable, stiffly accurate methods keep x = cos t at lambda = -1e6.
    for method in STIFF {
        let (r, s) = run("S.Prothero", 10.0, method, 1e-6);
        let x = r.final_value("x").unwrap();
        assert!((x - 10f64.cos()).abs() < 1e-5, "{method:?}: {x}");
        assert!(
            s.solver.steps < 2000,
            "{method:?}: {} steps",
            s.solver.steps
        );
        for k in 1..10 {
            let t = k as f64;
            assert!(
                (r.at("x", t).unwrap() - t.cos()).abs() < 1e-5,
                "{method:?} at {t}"
            );
        }
    }
}

#[test]
fn linear_accuracy_and_dense_output() {
    for method in [Method::Radau, Method::Esdirk, Method::Auto] {
        let (r, _) = run("S.Linear", 5.0, method, 1e-8);
        for k in 0..=50 {
            let t = k as f64 * 0.1;
            let x = (-0.5 * t).exp() * t.cos();
            let y = -(-0.5 * t).exp() * t.sin();
            // Dense output between steps.
            assert!(
                (r.at("x", t).unwrap() - x).abs() < 5e-6,
                "{method:?} x({t})"
            );
            assert!(
                (r.at("y", t).unwrap() - y).abs() < 5e-6,
                "{method:?} y({t})"
            );
        }
    }
}

/// Order of convergence with the step size forced by `max_step` (loose tolerance).
#[test]
fn convergence_order() {
    let m = compile("stiff.mo", "S.Linear");
    let err = |method: Method, h: f64| -> f64 {
        let opts = SimOptions {
            stop_time: 2.0,
            method,
            tolerance: 0.5,
            max_step: h,
            intervals: 4,
            ..SimOptions::from_model(&m)
        };
        let (r, s) = run_opts(&m, opts);
        assert_eq!(s.solver.rejected, 0);
        let x = (-1.0f64).exp() * 2f64.cos();
        let y = -(-1.0f64).exp() * 2f64.sin();
        (r.final_value("x").unwrap() - x)
            .abs()
            .max((r.final_value("y").unwrap() - y).abs())
    };
    for (method, p) in [(Method::Radau, 5.0), (Method::Esdirk, 4.0)] {
        let e1 = err(method, 0.2);
        let e2 = err(method, 0.1);
        let e3 = err(method, 0.05);
        let p1 = (e1 / e2).log2();
        let p2 = (e2 / e3).log2();
        println!("{method:?}: errors {e1:.3e} {e2:.3e} {e3:.3e}, orders {p1:.2} {p2:.2}");
        assert!((p2 - p).abs() < 0.4, "{method:?}: observed order {p1} {p2}");
    }
}

#[test]
fn sparse_heat_equation() {
    for method in STIFF {
        let (r, s) = run("S.Heat", 10.0, method, 1e-6);
        for i in [1usize, 15, 30, 45, 60] {
            let want = 1.0 - i as f64 / 61.0;
            let got = r.final_value(&format!("T[{i}]")).unwrap();
            assert!(
                (got - want).abs() < 1e-4,
                "{method:?} T[{i}] = {got} vs {want}"
            );
        }
        assert!(s.jacobian_colors <= 3, "{} colors", s.jacobian_colors);
        assert!(
            s.solver.steps < 1000,
            "{method:?}: {} steps",
            s.solver.steps
        );
        println!("heat {method:?}: {:?}", s.solver);
    }
}

#[test]
fn bouncing_ball_each_solver() {
    let m = compile("../models/events.mo", "E.Ball");
    let g: f64 = 9.81;
    let t1 = (2.0 / g).sqrt();
    let v1 = g * t1 * 0.8;
    let t2 = t1 + 2.0 * v1 / g;
    let v2 = v1 * 0.8;
    let t3 = t2 + 2.0 * v2 / g;
    let v3 = v2 * 0.8;
    let dt = 2.0 - t3;
    let h = v3 * dt - 0.5 * g * dt * dt;
    for method in [Method::Radau, Method::Esdirk, Method::Auto] {
        let opts = SimOptions {
            stop_time: 2.0,
            method,
            tolerance: 1e-8,
            interval: 0.01,
            ..SimOptions::from_model(&m)
        };
        let (r, s) = run_opts(&m, opts);
        assert_eq!(r.final_value("bounces").unwrap(), 3.0, "{method:?}");
        assert!(s.state_events >= 3, "{method:?}");
        let got = r.final_value("h").unwrap();
        assert!(
            (got - h).abs() < 1e-6 * (1.0 + h.abs()),
            "{method:?}: {got} vs {h}"
        );
        // The event time is located on the dense output: h(t1) = 0.
        assert!(r.at("h", t1 - 1e-3).unwrap() > 0.0);
    }
}

#[test]
fn auto_switches_both_ways() {
    let (r, s) = run("S.Switching", 20.0, Method::Auto, 1e-6);
    let (rb, _) = run("S.Switching", 20.0, Method::Bdf, 1e-9);
    for k in 0..=20 {
        let t = k as f64;
        let (a, b) = (r.at("x", t).unwrap(), rb.at("x", t).unwrap());
        assert!((a - b).abs() < 1e-4, "x({t}): {a} vs {b}");
    }
    println!("{:?}\n{}", s.solver, r.log.join("\n"));
    assert!(s.solver.switches >= 2, "{} switches", s.solver.switches);
    assert!(r.log.iter().any(|l| l.contains("switched to BDF")));
    assert!(r.log.iter().any(|l| l.contains("switched to Dopri5")));
    // A non-stiff problem stays explicit.
    let (_, s) = run("S.Linear", 5.0, Method::Auto, 1e-6);
    assert_eq!(s.solver.switches, 0);
    assert_eq!(s.solver.jac_evals, 0);
}

/// Exact Jacobian columns from the tape agree with central differences, also through
/// an algebraic loop, and the forward-difference mode agrees to its truncation error.
#[test]
fn exact_jacobian_matches_differences() {
    for model in ["S.Loop", "S.Robertson", "S.VanDerPol"] {
        let m = compile("stiff.mo", model);
        let mut sim = Simulation::new(&m, SimOptions::from_model(&m));
        sim.initialize().unwrap();
        let n = m.states.len();
        let mut x: Vec<f64> = vec![0.0; n];
        sim.rt.get_states(&mut x);
        // Move off the initial point so every entry is nonzero.
        for (i, v) in x.iter_mut().enumerate() {
            *v += 0.01 * (i as f64 + 1.0);
        }
        let t = 0.3;
        let mut st = SolverStats::default();
        let mut exact = StateJac::new(n, &m.jac_pattern, JacobianMode::Exact);
        assert!(exact.compute(&mut sim.rt, t, &x, 1e-10, 1e-10, &mut st));
        assert_eq!(exact.fallbacks, 0);
        assert_eq!(st.tangent_evals, exact.n_colors() as u64);
        let mut fd = StateJac::new(n, &m.jac_pattern, JacobianMode::Fd);
        assert!(fd.compute(&mut sim.rt, t, &x, 1e-6, 1e-6, &mut st));
        // Central differences, column by column.
        let mut f = |x: &[f64]| {
            let mut dx = vec![0.0; n];
            sim.rt.rhs(t, x, &mut dx);
            dx
        };
        for j in 0..n {
            let h = 1e-6 * x[j].abs().max(1.0);
            let mut xp = x.clone();
            xp[j] += h;
            let fp = f(&xp);
            xp[j] -= 2.0 * h;
            let fm = f(&xp);
            for i in 0..n {
                let cd = (fp[i] - fm[i]) / (2.0 * h);
                let e = exact.dense[i * n + j];
                let d = fd.dense[i * n + j];
                let scale = 1.0 + cd.abs();
                assert!(
                    (e - cd).abs() < 1e-6 * scale,
                    "{model} J[{i}][{j}]: {e} vs {cd}"
                );
                assert!(
                    (d - cd).abs() < 1e-4 * scale,
                    "{model} FD J[{i}][{j}]: {d} vs {cd}"
                );
            }
        }
    }
}

/// The exact and the difference Jacobian give the same BDF solution.
#[test]
fn bdf_exact_and_fd_agree() {
    for (model, stop) in [("S.Robertson", 40.0), ("S.VanDerPol", 2.0), ("S.Heat", 1.0)] {
        let m = compile("stiff.mo", model);
        let go = |jacobian: JacobianMode| {
            run_opts(
                &m,
                SimOptions {
                    stop_time: stop,
                    tolerance: 1e-8,
                    jacobian,
                    ..SimOptions::from_model(&m)
                },
            )
        };
        let (re, se) = go(JacobianMode::Exact);
        let (rf, sf) = go(JacobianMode::Fd);
        assert!(se.solver.tangent_evals > 0);
        assert_eq!(sf.solver.tangent_evals, 0);
        println!(
            "{model}: exact {:?}\n{model}: fd    {:?}",
            se.solver, sf.solver
        );
        for v in &re.vars {
            if let (Some(a), Some(b)) = (re.final_value(&v.name), rf.final_value(&v.name)) {
                assert!(
                    (a - b).abs() < 1e-5 * (1.0 + b.abs()),
                    "{model} {}: {a} vs {b}",
                    v.name
                );
            }
        }
    }
}

#[test]
fn dymola_algorithm_names() {
    for (name, m) in [
        ("Dassl", Method::Bdf),
        ("Cvode", Method::Bdf),
        ("Ida", Method::Bdf),
        ("Lsodar", Method::Auto),
        ("Radau", Method::Radau),
        ("Esdirk45a", Method::Esdirk),
        ("Esdirk23a", Method::Esdirk),
        ("Sdirk34hw", Method::Esdirk),
        ("Dopri45", Method::Dopri5),
        ("Dopri853", Method::Dopri5),
        ("Euler", Method::Euler),
        ("Rkfix4", Method::Rk4),
        ("Rkfix2", Method::Rk4),
    ] {
        assert_eq!(Method::parse(name), Some(m), "{name}");
    }
    // The experiment annotation selects the method.
    let src = "model A\n  Real x(start = 1, fixed = true);\nequation\n  der(x) = -x;\n  \
               annotation(experiment(StopTime = 1, __Dymola_Algorithm = \"Radau\"));\nend A;\n";
    let mut lib = Library::new();
    assert!(lib.set_document("a.mo", src).is_empty());
    let m = daedalus_core::translate(&lib, "A", &Default::default()).unwrap();
    let o = SimOptions::from_model(&m);
    assert_eq!(o.method, Method::Radau);
    let (r, _) = run_opts(&m, o);
    assert!((r.final_value("x").unwrap() - (-1f64).exp()).abs() < 1e-5);
}
