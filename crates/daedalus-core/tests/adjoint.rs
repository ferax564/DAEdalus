//! Adjoint gradients (reverse-mode tape passes and a backward Radau solve) against
//! analytic results, forward sensitivities and finite differences; gradient-based
//! optimization and calibration; designs of experiments and response surfaces.

use daedalus_core::adjoint::{GradMethod, GradSpec, Gradient, gradient};
use daedalus_core::codegen::Model;
use daedalus_core::doe::{Design, DoeJob, DoeSpec, Factor};
use daedalus_core::frontend::Library;
use daedalus_core::optimize::ObjTerm;
use daedalus_core::sim::{SimOptions, Simulation};
use daedalus_core::tape_reverse::run_reverse;
use daedalus_core::uq::{Job, Rng, run_job};

fn model_from(file: &str, name: &str) -> Model {
    let path = format!("{}/tests/models/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    daedalus_core::translate(&lib, name, &Default::default()).unwrap()
}

fn opts(m: &Model, tol: f64, intervals: usize) -> SimOptions {
    SimOptions {
        tolerance: tol,
        intervals,
        ..SimOptions::from_model(m)
    }
}

fn obj(q: &str) -> Vec<ObjTerm> {
    vec![ObjTerm {
        qoi: q.into(),
        weight: 1.0,
        target: None,
    }]
}

fn grad(m: &Model, o: &SimOptions, params: &[&str], q: &str, method: GradMethod) -> Gradient {
    let spec = GradSpec::new(
        params.iter().map(|s| s.to_string()).collect(),
        obj(q),
        method,
    );
    gradient(m, o, &spec).unwrap()
}

fn rel(a: f64, b: f64, scale: f64) -> f64 {
    (a - b).abs() / (b.abs().max(scale))
}

#[test]
fn reverse_pass_is_the_transpose_of_tangents() {
    // <v, J u> = <J^T v, u> for random u (states and parameters) and v (derivatives
    // and outputs), at a few points of the trajectory.
    for (file, name) in [
        ("adjoint.mo", "Adjoint.Chain"),
        ("analysis.mo", "Analysis.Loop"),
        ("analysis.mo", "Analysis.MassSpringDamper"),
    ] {
        let m = model_from(file, name);
        let mut sim = Simulation::new(&m, opts(&m, 1e-8, 50));
        sim.initialize().unwrap();
        let mut st = sim.stepper();
        let mut rng = Rng::new(7);
        for t in [0.3, 1.1] {
            sim.advance_to(&mut st, t).unwrap();
            sim.rt.run(&m.ode);
            sim.rt.run(&m.outputs);
            let nr = sim.rt.r.len();
            let mut inputs: Vec<u32> = m.states.clone();
            inputs.extend(m.free_params.iter().copied());
            let mut outs: Vec<u32> = m.derivs.clone();
            outs.extend(
                m.vars
                    .iter()
                    .enumerate()
                    .filter(|(_, v)| !v.name.starts_with('$'))
                    .map(|(i, _)| i as u32)
                    .filter(|i| !inputs.contains(i) && !m.derivs.contains(i))
                    .filter(|&i| !m.param_vars.contains(&i)),
            );
            let mut dr = vec![0.0; nr];
            let mut u = vec![0.0; nr];
            for &i in &inputs {
                u[i as usize] = rng.normal();
            }
            dr.copy_from_slice(&u);
            sim.rt.run_tangent(&m.ode, &mut dr);
            sim.rt.run_tangent(&m.outputs, &mut dr);
            let mut adj = vec![0.0; nr];
            let mut lhs = 0.0;
            for &o in &outs {
                let v = rng.normal();
                adj[o as usize] += v;
                lhs += v * dr[o as usize];
            }
            run_reverse(&mut sim.rt, &m.outputs, &mut adj);
            run_reverse(&mut sim.rt, &m.ode, &mut adj);
            let rhs: f64 = inputs
                .iter()
                .map(|&i| adj[i as usize] * u[i as usize])
                .sum();
            assert!(
                (lhs - rhs).abs() <= 1e-10 * (1.0 + lhs.abs()),
                "{name} t={t}: {lhs} vs {rhs}"
            );
        }
    }
}

#[test]
fn decay_adjoint_matches_analytic() {
    let m = model_from("analysis.mo", "Analysis.Decay");
    let o = opts(&m, 1e-10, 300);
    let (k, x0, t) = (0.7f64, 2.0f64, 3.0f64);
    let e = (-k * t).exp();
    // final(x) = x0 e^{-kT}
    let g = grad(&m, &o, &["k", "x0"], "final(x)", GradMethod::Adjoint);
    assert_eq!(g.method, GradMethod::Adjoint);
    assert!(
        rel(g.gradient[0], -t * x0 * e, 1e-6) < 1e-6,
        "{:?}",
        g.gradient
    );
    assert!(rel(g.gradient[1], e, 1e-6) < 1e-6, "{:?}", g.gradient);
    // integral(y) = 3 x0 (1 - e^{-kT}) / k
    let g = grad(&m, &o, &["k", "x0"], "integral(y)", GradMethod::Adjoint);
    let dk = 3.0 * x0 * (t * e / k - (1.0 - e) / (k * k));
    let dx0 = 3.0 * (1.0 - e) / k;
    assert!(
        rel(g.gradient[0], dk, 1e-6) < 1e-6,
        "{} vs {dk}",
        g.gradient[0]
    );
    assert!(
        rel(g.gradient[1], dx0, 1e-6) < 1e-6,
        "{} vs {dx0}",
        g.gradient[1]
    );
    // windowed mean and a point value
    let g = grad(&m, &o, &["k"], "mean(x, 1, 2)", GradMethod::Adjoint);
    let mean_dk = x0
        * ((2.0 * (-2.0 * k).exp() - (-k).exp()) / k - ((-k).exp() - (-2.0 * k).exp()) / (k * k));
    assert!(
        rel(g.gradient[0], mean_dk, 1e-6) < 1e-5,
        "{} vs {mean_dk}",
        g.gradient[0]
    );
    let g = grad(&m, &o, &["k"], "at(x, 1.234)", GradMethod::Adjoint);
    let at_dk = -1.234 * x0 * (-k * 1.234f64).exp();
    assert!(
        rel(g.gradient[0], at_dk, 1e-6) < 1e-6,
        "{} vs {at_dk}",
        g.gradient[0]
    );
}

/// Adjoint vs forward sensitivities vs central differences of the objective. The QoIs
/// integrate the output grid with the trapezoidal rule (an O(dt^2) error the adjoint of
/// the exact integral does not share), hence the fine grid.
fn compare(file: &str, name: &str, params: &[&str], q: &str, tol: f64) -> (f64, f64) {
    let m = model_from(file, name);
    let o = opts(&m, 1e-10, 4000);
    let a = grad(&m, &o, params, q, GradMethod::Adjoint);
    let f = grad(&m, &o, params, q, GradMethod::Forward);
    let d = grad(&m, &o, params, q, GradMethod::Fd);
    assert_eq!(f.method, GradMethod::Forward);
    let scale = a.gradient.iter().fold(0.0f64, |s, x| s.max(x.abs()));
    let mut ef = 0.0f64;
    let mut ed = 0.0f64;
    for j in 0..params.len() {
        ef = ef.max(rel(a.gradient[j], f.gradient[j], 1e-3 * scale));
        ed = ed.max(rel(a.gradient[j], d.gradient[j], 1e-3 * scale));
    }
    eprintln!(
        "{name} {q}: J = {:.8e}; max rel. difference adjoint vs forward {ef:.2e}, vs fd {ed:.2e}",
        a.value
    );
    assert!(
        ef < tol,
        "{name} {q}: adjoint {:?} forward {:?}",
        a.gradient,
        f.gradient
    );
    assert!(
        ed < 10.0 * tol,
        "{name} {q}: adjoint {:?} fd {:?}",
        a.gradient,
        d.gradient
    );
    assert!((a.value - f.value).abs() <= 1e-6 * (1.0 + a.value.abs()));
    (ef, ed)
}

#[test]
fn mass_spring_damper_adjoint_matches_forward_and_fd() {
    let p = ["m", "c", "k", "x0"];
    compare(
        "analysis.mo",
        "Analysis.MassSpringDamper",
        &p,
        "integral(energy)",
        1e-5,
    );
    compare(
        "analysis.mo",
        "Analysis.MassSpringDamper",
        &p,
        "final(x)",
        1e-5,
    );
    compare(
        "analysis.mo",
        "Analysis.MassSpringDamper",
        &p,
        "mean(v, 1, 3)",
        1e-5,
    );
    compare(
        "analysis.mo",
        "Analysis.MassSpringDamper",
        &p,
        "max(x, 1, 4)",
        1e-4,
    );
}

#[test]
fn algebraic_loop_adjoint() {
    compare(
        "analysis.mo",
        "Analysis.Loop",
        &["p", "q"],
        "integral(a)",
        1e-5,
    );
    compare(
        "analysis.mo",
        "Analysis.Loop",
        &["p", "q"],
        "final(b)",
        1e-5,
    );
}

#[test]
fn many_parameter_chain_adjoint() {
    let p = ["k1", "k2", "k3", "k4", "k5", "k6", "g", "w", "e"];
    compare("adjoint.mo", "Adjoint.Chain", &p, "integral(y)", 1e-5);
    compare("adjoint.mo", "Adjoint.Chain", &p, "mean(z, 1, 3)", 1e-5);
    compare("adjoint.mo", "Adjoint.Chain", &p, "final(x6)", 1e-5);
}

#[test]
fn stiff_adjoint() {
    compare(
        "adjoint.mo",
        "Adjoint.Stiff",
        &["a", "b", "c"],
        "integral(x)",
        1e-5,
    );
}

#[test]
fn weighted_objective_with_target() {
    let m = model_from("analysis.mo", "Analysis.MassSpringDamper");
    let o = opts(&m, 1e-10, 400);
    let terms = vec![
        ObjTerm {
            qoi: "final(x)".into(),
            weight: 2.0,
            target: Some(0.05),
        },
        ObjTerm {
            qoi: "integral(v)".into(),
            weight: -0.5,
            target: None,
        },
    ];
    let params = vec!["c".to_string(), "k".to_string()];
    let a = gradient(
        &m,
        &o,
        &GradSpec::new(params.clone(), terms.clone(), GradMethod::Adjoint),
    )
    .unwrap();
    let f = gradient(&m, &o, &GradSpec::new(params, terms, GradMethod::Fd)).unwrap();
    for j in 0..2 {
        assert!(
            rel(a.gradient[j], f.gradient[j], 1e-4) < 1e-4,
            "{:?} vs {:?}",
            a.gradient,
            f.gradient
        );
    }
}

#[test]
fn events_fall_back_to_forward() {
    let m = model_from("analysis.mo", "Analysis.Bounce");
    let o = opts(&m, 1e-8, 200);
    let g = grad(&m, &o, &["k"], "final(x)", GradMethod::Auto);
    assert_ne!(g.method, GradMethod::Adjoint);
    assert!(g.notes[0].contains("event"), "{:?}", g.notes);
    // x(2) = 1 + 0.5 k (2 - 1/k) = 0.5 + k  =>  dx/dk = 1
    assert!((g.gradient[0] - 1.0).abs() < 1e-3, "{:?}", g.gradient);
    let spec = GradSpec::new(vec!["k".into()], obj("final(x)"), GradMethod::Adjoint);
    assert!(gradient(&m, &o, &spec).is_err());
}

#[test]
fn gradient_job_and_report() {
    let m = model_from("analysis.mo", "Analysis.Decay");
    let o = opts(&m, 1e-8, 100);
    let spec = GradSpec::new(
        vec!["k".into(), "x0".into()],
        obj("integral(x)"),
        GradMethod::Auto,
    );
    let mut job = daedalus_core::adjoint::GradJob::new(m, o, spec).unwrap();
    run_job(&mut job, &mut |_, _| true).unwrap();
    let r = job.report();
    assert!(r.contains("adjoint") && r.contains("dJ/dp"), "{r}");
    let j = job.result_json();
    assert_eq!(j["method"], "adjoint");
    assert_eq!(j["gradient"].as_array().unwrap().len(), 2);
}

// ---------------------------------------------------------------------------------
// Design of experiments
// ---------------------------------------------------------------------------------

fn factor(name: &str, lo: f64, hi: f64) -> Factor {
    Factor {
        name: name.into(),
        lo,
        hi,
        levels: 0,
    }
}

fn doe_spec(design: Design) -> DoeSpec {
    serde_json::from_value(serde_json::json!({
        "factors": [
            {"name": "a", "lo": 0.0, "hi": 2.0},
            {"name": "b", "lo": 0.0, "hi": 2.0},
            {"name": "c", "lo": 0.0, "hi": 2.0}
        ],
        "design": design,
        "qois": ["final(y)", "final(y2)"],
    }))
    .unwrap()
}

#[test]
fn ccd_and_bbd_recover_a_quadratic_through_the_simulator() {
    let m = model_from("adjoint.mo", "Adjoint.Quad");
    for design in [Design::Ccd, Design::Bbd] {
        let mut job = DoeJob::new(m.clone(), opts(&m, 1e-8, 10), doe_spec(design)).unwrap();
        job.run_parallel(4, &|_, _| {}).unwrap();
        let r = job.result();
        assert_eq!(r.failed, 0);
        let s = &r.surfaces[0];
        assert!(s.r2 > 1.0 - 1e-10, "{design:?} R2 {}", s.r2);
        let c = |n: &str| s.coef[s.terms.iter().position(|t| t == n).unwrap()];
        assert!(
            (c("1") - 1.0).abs() < 1e-8,
            "{design:?} {:?} {:?}",
            s.terms,
            s.coef
        );
        assert!((c("a") - 2.0).abs() < 1e-8);
        assert!((c("b") + 1.0).abs() < 1e-8);
        assert!((c("a*c") - 0.5).abs() < 1e-8);
        assert!((c("b^2") - 3.0).abs() < 1e-8);
        assert!(c("a*b").abs() < 1e-8);
        // main effect of b: means at its levels; interaction a x c exists
        assert_eq!(r.main_effects[0][1].factor, "b");
        assert_eq!(r.interactions[0].len(), 3);
        // Pareto front of y (min) and y2 (min) is non-empty and non-dominated
        let p = r.pareto.as_ref().unwrap();
        assert!(!p.front.is_empty());
        for &i in &p.front {
            for j in 0..r.runs {
                let dom = r.responses[0][j] <= r.responses[0][i]
                    && r.responses[1][j] <= r.responses[1][i]
                    && (r.responses[0][j] < r.responses[0][i]
                        || r.responses[1][j] < r.responses[1][i]);
                assert!(!dom, "run {j} dominates front run {i}");
            }
        }
        assert!(r.report().contains("R^2"));
    }
}

#[test]
fn factorial_designs_and_sliced_job() {
    let m = model_from("adjoint.mo", "Adjoint.Quad");
    // a 3-level full factorial also recovers the quadratic
    let mut spec = doe_spec(Design::Full);
    for f in &mut spec.factors {
        f.levels = 3;
    }
    let mut job = DoeJob::new(m.clone(), opts(&m, 1e-8, 10), spec).unwrap();
    run_job(&mut job, &mut |_, _| true).unwrap();
    let r = job.result();
    assert_eq!(r.runs, 27);
    assert!(r.surfaces[0].r2 > 1.0 - 1e-10);
    // a 2-level fraction of 3 factors in 4 runs: main effects only
    let mut spec = doe_spec(Design::Frac);
    spec.resolution = Some(3);
    let mut job = DoeJob::new(m.clone(), opts(&m, 1e-8, 10), spec).unwrap();
    run_job(&mut job, &mut |_, _| true).unwrap();
    let r = job.result();
    assert_eq!(r.runs, 4);
    assert!(
        r.notes.iter().any(|n| n.contains("resolution III")),
        "{:?}",
        r.notes
    );
    // LHS with a fixed seed is reproducible
    let mut spec = doe_spec(Design::Lhs);
    spec.samples = 20;
    let j1 = DoeJob::new(m.clone(), opts(&m, 1e-8, 10), spec.clone()).unwrap();
    let j2 = DoeJob::new(m.clone(), opts(&m, 1e-8, 10), spec).unwrap();
    assert_eq!(j1.plan().values, j2.plan().values);
    // factors must be free parameters
    let mut bad = doe_spec(Design::Ccd);
    bad.factors.push(factor("nope", 0.0, 1.0));
    assert!(
        DoeJob::new(
            m,
            opts(&model_from("adjoint.mo", "Adjoint.Quad"), 1e-8, 10),
            bad
        )
        .is_err()
    );
}

#[test]
fn factor_parse() {
    let f = Factor::parse("body.m=1:3:4").unwrap();
    assert_eq!(
        (f.name.as_str(), f.lo, f.hi, f.levels),
        ("body.m", 1.0, 3.0, 4)
    );
    assert!(Factor::parse("k=3:1").is_err());
    assert!(Factor::parse("k").is_err());
}

// ---------------------------------------------------------------------------------
// Gradient-based optimization and calibration
// ---------------------------------------------------------------------------------

use daedalus_core::calibrate::{CalibOptimizer, CalibSpec, Calibration, DataMap, Tuner};
use daedalus_core::optimize::{Algorithm, OptSpec, Optimization};

fn tuner(name: &str, lo: f64, hi: f64, start: Option<f64>) -> Tuner {
    Tuner {
        name: name.into(),
        lo: Some(lo),
        hi: Some(hi),
        start,
    }
}

#[test]
fn lbfgsb_optimization_with_adjoint_gradients() {
    let m = model_from("adjoint.mo", "Adjoint.Chain");
    let o = opts(&m, 1e-9, 200);
    let names = ["k1", "k2", "k3", "k4", "k5", "k6"];
    let spec = |algorithm| OptSpec {
        tuners: names.iter().map(|n| tuner(n, 0.5, 2.0, None)).collect(),
        objective: vec![ObjTerm {
            qoi: "mean(x6, 2, 4)".into(),
            weight: 1.0,
            target: Some(0.12),
        }],
        max_iter: 200,
        tol: 1e-12,
        algorithm,
        gradient: GradMethod::Auto,
    };
    let mut opt = Optimization::new(m.clone(), o.clone(), spec(Algorithm::Lbfgsb)).unwrap();
    run_job(&mut opt, &mut |_, _| true).unwrap();
    let r = opt.result();
    eprintln!("{}", Optimization::report_of(&r));
    assert_eq!(r.gradient_method, Some(GradMethod::Adjoint));
    assert!(r.objective_after < 1e-10, "{}", Optimization::report_of(&r));
    let mut nm = Optimization::new(m, o, spec(Algorithm::NelderMead)).unwrap();
    run_job(&mut nm, &mut |_, _| true).unwrap();
    let rn = nm.result();
    eprintln!(
        "L-BFGS-B: {} runs + {} gradients; Nelder-Mead: {} runs, objective {:.3e}",
        r.runs, r.gradients, rn.runs, rn.objective_after
    );
    assert!(r.runs + r.gradients < rn.runs);
}

#[test]
fn lbfgsb_stops_at_an_active_bound() {
    // final(x) = x0 exp(-k T) decreases in k: the minimum is at the upper bound.
    let m = model_from("analysis.mo", "Analysis.Decay");
    let o = opts(&m, 1e-9, 100);
    let spec = OptSpec {
        tuners: vec![
            tuner("k", 0.1, 1.0, Some(0.3)),
            tuner("x0", 1.0, 3.0, Some(2.0)),
        ],
        objective: obj("final(x)"),
        max_iter: 50,
        tol: 1e-12,
        algorithm: Algorithm::Lbfgsb,
        gradient: GradMethod::Adjoint,
    };
    let mut opt = Optimization::new(m, o, spec).unwrap();
    run_job(&mut opt, &mut |_, _| true).unwrap();
    let r = opt.result();
    assert!(r.converged, "{}", Optimization::report_of(&r));
    assert!((r.values[0] - 1.0).abs() < 1e-12, "{:?}", r.values);
    assert!((r.values[1] - 1.0).abs() < 1e-12, "{:?}", r.values);
}

#[test]
fn lbfgsb_calibration_with_adjoint_gradients() {
    let m = model_from("analysis.mo", "Analysis.MassSpringDamper");
    let o = opts(&m, 1e-9, 200);
    let mut oo = o.clone();
    oo.params = vec![("c".into(), 0.4), ("k".into(), 6.0)];
    oo.store_events = false;
    oo.intervals = 80;
    let (res, _) = daedalus_core::sim::simulate(&m, oo).unwrap();
    let x = res.values("x").unwrap();
    let mut rng = Rng::new(42);
    let mut csv = String::from("time,pos\n");
    for (i, t) in res.time.iter().enumerate() {
        csv.push_str(&format!("{t},{}\n", x[i] + 0.002 * rng.normal()));
    }
    let spec = CalibSpec {
        tuners: vec![
            tuner("c", 0.01, 5.0, Some(1.0)),
            tuner("k", 1.0, 20.0, Some(4.0)),
        ],
        csv,
        time_column: None,
        map: vec![DataMap {
            column: "pos".into(),
            var: "x".into(),
            weight: None,
        }],
        window: None,
        method: Default::default(),
        max_iter: 100,
        optimizer: CalibOptimizer::Lbfgsb,
        gradient: GradMethod::Auto,
    };
    let mut cal = Calibration::new(m, o, spec).unwrap();
    run_job(&mut cal, &mut |_, _| true).unwrap();
    let r = cal.result();
    eprintln!("{}", Calibration::report_of(&r));
    assert_eq!(r.gradient_method, Some(GradMethod::Adjoint));
    let (c, k) = (r.params[0].value, r.params[1].value);
    assert!((c - 0.4).abs() < 0.02, "c = {c}");
    assert!((k - 6.0).abs() < 0.03, "k = {k}");
    assert!(r.params[0].std_error > 0.0 && r.params[0].std_error < 0.05);
    assert!(
        (r.columns[0].rms_after - 0.002).abs() < 0.0007,
        "{}",
        r.columns[0].rms_after
    );
}
