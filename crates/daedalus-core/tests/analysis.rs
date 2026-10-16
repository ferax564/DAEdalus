//! Sensitivity analysis, uncertainty quantification, calibration and optimization
//! against analytic results.

use daedalus_core::calibrate::{CalibSpec, Calibration, DataMap, Tuner};
use daedalus_core::codegen::Model;
use daedalus_core::frontend::Library;
use daedalus_core::optimize::{ObjTerm, OptSpec, Optimization};
use daedalus_core::sensitivity::{SensMethod, SensOptions, Sensitivities, sensitivities};
use daedalus_core::sim::SimOptions;
use daedalus_core::uq::{
    Dist, GsaJob, GsaMode, GsaSpec, ParamDist, Rng, Sampling, run_job, sobol_indices, unit_samples,
};

fn model(name: &str) -> Model {
    let path = format!("{}/tests/models/analysis.mo", env!("CARGO_MANIFEST_DIR"));
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

fn sens(
    m: &Model,
    o: &SimOptions,
    params: &[&str],
    outputs: &[&str],
    method: SensMethod,
) -> Sensitivities {
    let so = SensOptions::new(
        params.iter().map(|s| s.to_string()).collect(),
        outputs.iter().map(|s| s.to_string()).collect(),
        method,
    );
    sensitivities(m, o, &so).unwrap()
}

fn idx(v: &[String], n: &str) -> usize {
    v.iter().position(|x| x == n).unwrap()
}

#[test]
fn decay_sensitivities_exact_and_fd() {
    let m = model("Analysis.Decay");
    let o = opts(&m, 1e-10, 300);
    let (k, x0) = (0.7f64, 2.0f64);
    for (method, tol) in [(SensMethod::Exact, 1e-6), (SensMethod::Fd, 1e-4)] {
        let s = sens(&m, &o, &["k", "x0"], &["x", "y"], method);
        assert_eq!(s.method, method);
        for (i, &t) in s.time.iter().enumerate() {
            let x = x0 * (-k * t).exp();
            let dk = -t * x; // dx/dk
            let dx0 = (-k * t).exp(); // dx/dx0
            let err = |a: f64, b: f64| (a - b).abs() / (1e-3 + b.abs());
            assert!(
                err(s.dydp[0][0][i], dk) < tol,
                "{method:?} t={t}: {} vs {dk}",
                s.dydp[0][0][i]
            );
            assert!(err(s.dydp[0][1][i], dx0) < tol, "{method:?} t={t}");
            assert!(err(s.dydp[1][0][i], 3.0 * dk) < tol, "{method:?} y t={t}");
            // normalized: k / x * dx/dk = -k t
            if i > 0 {
                assert!((s.normalized[0][0][i] + k * t).abs() < 10.0 * tol * (1.0 + k * t));
            }
        }
        // integrated |normalized|: k -> mean of k t over [0, 3] = 1.05; x0 -> 1 (per output)
        let r = &s.ranking;
        assert_eq!(r[0].param, "k");
        assert!(
            (r[0].per_output[0] - 1.05).abs() < 1e-3,
            "{}",
            r[0].per_output[0]
        );
        assert!((r[1].per_output[0] - 1.0).abs() < 1e-3);
    }
}

/// Closed-form position of the underdamped oscillator.
fn msd_x(m: f64, c: f64, k: f64, x0: f64, t: f64) -> f64 {
    let w = (k / m).sqrt();
    let z = c / (2.0 * (k * m).sqrt());
    let wd = w * (1.0 - z * z).sqrt();
    x0 * (-z * w * t).exp() * ((wd * t).cos() + z * w / wd * (wd * t).sin())
}

#[test]
fn mass_spring_damper_sensitivities() {
    let m = model("Analysis.MassSpringDamper");
    let o = opts(&m, 1e-10, 400);
    let p = [1.5, 0.4, 6.0, 0.2];
    let names = ["m", "c", "k", "x0"];
    let s = sens(&m, &o, &names, &["x", "energy"], SensMethod::Exact);
    assert_eq!(s.method, SensMethod::Exact);
    let fd = sens(&m, &o, &names, &["x", "energy"], SensMethod::Fd);
    let mut worst: f64 = 0.0;
    for (i, &t) in s.time.iter().enumerate() {
        for j in 0..4 {
            // d x / d p_j of the closed form by central differences (no solver noise).
            let h = 1e-6 * p[j];
            let (mut pp, mut pm) = (p, p);
            pp[j] += h;
            pm[j] -= h;
            let want = (msd_x(pp[0], pp[1], pp[2], pp[3], t)
                - msd_x(pm[0], pm[1], pm[2], pm[3], t))
                / (2.0 * h);
            let got = s.dydp[0][j][i];
            worst = worst.max((got - want).abs());
            assert!(
                (got - want).abs() < 2e-6,
                "t={t} d x/d {}: {got} vs {want}",
                names[j]
            );
            // The energy output follows by the chain rule: compare with differences.
            let (a, b) = (s.dydp[1][j][i], fd.dydp[1][j][i]);
            assert!(
                (a - b).abs() < 1e-4 * (1.0 + b.abs()),
                "t={t} energy/{}: {a} vs {b}",
                names[j]
            );
        }
    }
    eprintln!("largest error of the exact sensitivities: {worst:e}");
}

#[test]
fn algebraic_loop_exact_matches_fd() {
    let m = model("Analysis.Loop");
    let o = opts(&m, 1e-10, 200);
    let s = sens(&m, &o, &["p", "q"], &["x", "a", "b"], SensMethod::Exact);
    let f = sens(&m, &o, &["p", "q"], &["x", "a", "b"], SensMethod::Fd);
    for k in 0..3 {
        for j in 0..2 {
            for i in 0..s.time.len() {
                let (a, b) = (s.dydp[k][j][i], f.dydp[k][j][i]);
                assert!(
                    (a - b).abs() < 1e-5 * (1.0 + b.abs()),
                    "{k} {j} t={}: {a} vs {b}",
                    s.time[i]
                );
            }
        }
    }
}

#[test]
fn events_fall_back_to_differences() {
    let m = model("Analysis.Bounce");
    let o = opts(&m, 1e-10, 200);
    let so = SensOptions::new(vec!["k".into()], vec!["x".into()], SensMethod::Exact);
    let e = sensitivities(&m, &o, &so).unwrap_err();
    assert!(e.contains("event"), "{e}");
    let s = sens(&m, &o, &["k"], &["x"], SensMethod::Auto);
    assert_eq!(s.method, SensMethod::Fd);
    assert!(s.notes[0].contains("event"), "{:?}", s.notes);
    // x reaches 1 at 1/k, then x = 1 + 0.5 k (t - 1/k): dx/dk = 0.5 t at t = 2.
    let last = s.time.len() - 1;
    assert!(
        (s.dydp[0][0][last] - 1.0).abs() < 1e-3,
        "{}",
        s.dydp[0][0][last]
    );
}

#[test]
fn only_free_parameters() {
    let m = model("Analysis.Decay");
    let o = opts(&m, 1e-8, 50);
    for (p, msg) in [
        ("k2", "computed from other parameters"),
        ("nope", "unknown parameter"),
        ("x", "not a parameter"),
    ] {
        let so = SensOptions::new(vec![p.into()], vec!["x".into()], SensMethod::Exact);
        let e = sensitivities(&m, &o, &so).unwrap_err();
        assert!(e.contains(msg), "{p}: {e}");
    }
}

#[test]
fn sobol_estimators_on_ishigami() {
    // Directly on the analytic function.
    let f = |x: &[f64]| x[0].sin() + 7.0 * x[1].sin().powi(2) + 0.1 * x[2].powi(4) * x[0].sin();
    let pi = std::f64::consts::PI;
    let n = 40_000;
    let mut rng = Rng::new(7);
    let pts = unit_samples(n, 6, Sampling::Lhs, &mut rng);
    let map = |u: &[f64]| -> Vec<f64> { u.iter().map(|v| -pi + 2.0 * pi * v).collect() };
    let mut fa = Vec::new();
    let mut fb = Vec::new();
    let mut fab = vec![Vec::new(); 3];
    for u in &pts {
        let a = map(&u[..3]);
        let b = map(&u[3..]);
        fa.push(f(&a));
        fb.push(f(&b));
        for i in 0..3 {
            let mut ab = a.clone();
            ab[i] = b[i];
            fab[i].push(f(&ab));
        }
    }
    let (s1, st) = sobol_indices(&fa, &fb, &fab);
    let want_s1 = [0.3139, 0.4424, 0.0];
    let want_st = [0.5576, 0.4424, 0.2437];
    for i in 0..3 {
        assert!((s1[i] - want_s1[i]).abs() < 0.02, "S1[{i}] = {}", s1[i]);
        assert!((st[i] - want_st[i]).abs() < 0.02, "ST[{i}] = {}", st[i]);
    }
}

#[test]
fn sobol_through_the_simulator() {
    let m = model("Analysis.Ishigami");
    let mut o = opts(&m, 1e-6, 1);
    o.stop_time = 1.0;
    let pi = std::f64::consts::PI;
    let spec = GsaSpec {
        mode: GsaMode::Sobol,
        params: ["x1", "x2", "x3"]
            .iter()
            .map(|n| ParamDist {
                name: n.to_string(),
                dist: Dist::Uniform { lo: -pi, hi: pi },
            })
            .collect(),
        outputs: vec![],
        qois: vec!["final(y)".into()],
        n: 4000,
        sampling: Sampling::Lhs,
        seed: 3,
        levels: 4,
    };
    let mut job = GsaJob::new(m, o, spec).unwrap();
    job.run_parallel(4, &mut |_, _| true).unwrap();
    let r = job.result();
    assert_eq!(r.failed, 0, "{:?}", r.errors);
    assert_eq!(r.runs, 4000 * 5);
    let so = &r.sobol[0];
    let want_s1 = [0.3139, 0.4424, 0.0];
    let want_st = [0.5576, 0.4424, 0.2437];
    for i in 0..3 {
        assert!(
            (so.s1[i] - want_s1[i]).abs() < 0.05,
            "S1[{i}] = {}",
            so.s1[i]
        );
        assert!(
            (so.st[i] - want_st[i]).abs() < 0.05,
            "ST[{i}] = {}",
            so.st[i]
        );
        assert!(so.s1_conf[i] > 0.0 && so.s1_conf[i] < 0.1);
    }
    // Variance of Ishigami: a^2/8 + b pi^4/5 + b^2 pi^8/18 + 1/2 (a = 7, b = 0.1).
    let var = 49.0 / 8.0 + 0.1 * pi.powi(4) / 5.0 + 0.01 * pi.powi(8) / 18.0 + 0.5;
    assert!(
        (so.variance - var).abs() < 0.05 * var,
        "{} vs {var}",
        so.variance
    );
}

#[test]
fn monte_carlo_matches_analytic_statistics() {
    let m = model("Analysis.Linear");
    let o = opts(&m, 1e-9, 100);
    let spec = GsaSpec {
        mode: GsaMode::MonteCarlo,
        params: vec![ParamDist {
            name: "b".into(),
            dist: Dist::Normal {
                mean: 1.0,
                std: 0.2,
            },
        }],
        outputs: vec!["x".into()],
        qois: vec!["final(x)".into(), "integral(x)".into()],
        n: 400,
        sampling: Sampling::Lhs,
        seed: 1,
        levels: 4,
    };
    let mut job = GsaJob::new(m, o, spec).unwrap();
    // The stepwise (single worker) path.
    let mut calls = 0;
    run_job(&mut job, &mut |_, _| {
        calls += 1;
        true
    })
    .unwrap();
    assert_eq!(calls, 400);
    let r = job.result();
    let b = &r.bands[0];
    for (i, &t) in r.time.iter().enumerate() {
        let g = 1.0 - (-t).exp(); // x = b g(t)
        assert!(
            (b.mean[i] - g).abs() < 2e-3 * g + 1e-9,
            "t={t}: {} vs {g}",
            b.mean[i]
        );
        assert!(
            (b.std[i] - 0.2 * g).abs() < 0.02 * 0.2 * g + 1e-9,
            "t={t}: {}",
            b.std[i]
        );
        let q95 = g * (1.0 + 0.2 * 1.6448536);
        assert!(
            (b.q95[i] - q95).abs() < 0.01 * q95 + 1e-9,
            "t={t}: {} vs {q95}",
            b.q95[i]
        );
        assert!(b.q05[i] <= b.q50[i] && b.q50[i] <= b.q95[i]);
    }
    // integral of x over [0, 2] = b (2 - (1 - e^-2))
    let want = 2.0 - (1.0 - (-2.0f64).exp());
    assert!(
        (r.qois[1].mean - want).abs() < 2e-3 * want,
        "{}",
        r.qois[1].mean
    );
}

#[test]
fn morris_on_an_affine_function() {
    let m = model("Analysis.Affine");
    let mut o = opts(&m, 1e-6, 1);
    o.stop_time = 1.0;
    let u = |lo, hi| Dist::Uniform { lo, hi };
    let spec = GsaSpec {
        mode: GsaMode::Morris,
        params: vec![
            ParamDist {
                name: "p1".into(),
                dist: u(0.0, 1.0),
            },
            ParamDist {
                name: "p2".into(),
                dist: u(0.0, 2.0),
            },
            ParamDist {
                name: "p3".into(),
                dist: u(-1.0, 0.0),
            },
        ],
        outputs: vec![],
        qois: vec!["final(y)".into()],
        n: 10,
        sampling: Sampling::Random,
        seed: 9,
        levels: 4,
    };
    let mut job = GsaJob::new(m, o, spec).unwrap();
    run_job(&mut job, &mut |_, _| true).unwrap();
    let r = job.result();
    assert_eq!(r.runs, 40);
    let mo = &r.morris[0];
    // Elementary effects in unit-scaled ranges: c_i (hi - lo).
    for (i, want) in [3.0, -4.0, 0.5].iter().enumerate() {
        assert!((mo.mu[i] - want).abs() < 1e-6, "{i}: {}", mo.mu[i]);
        assert!((mo.mu_star[i] - want.abs()).abs() < 1e-6);
        assert!(mo.sigma[i] < 1e-6);
    }
}

fn synthetic_csv(m: &Model, o: &SimOptions, params: &[(&str, f64)], noise: f64) -> String {
    let mut oo = o.clone();
    oo.params = params.iter().map(|(n, v)| (n.to_string(), *v)).collect();
    oo.store_events = false;
    oo.intervals = 80;
    let (res, _) = daedalus_core::sim::simulate(m, oo).unwrap();
    let x = res.values("x").unwrap();
    let e = res.values("energy").unwrap();
    let mut rng = Rng::new(42);
    let mut s = String::from("time,pos,energy\n");
    for (i, t) in res.time.iter().enumerate() {
        s.push_str(&format!(
            "{t},{},{}\n",
            x[i] + noise * rng.normal(),
            e[i] + 0.1 * noise * rng.normal()
        ));
    }
    s
}

#[test]
fn calibration_recovers_parameters() {
    let m = model("Analysis.MassSpringDamper");
    let o = opts(&m, 1e-9, 200);
    let csv = synthetic_csv(&m, &o, &[("c", 0.4), ("k", 6.0)], 0.002);
    for method in [SensMethod::Exact, SensMethod::Fd] {
        let spec = CalibSpec {
            tuners: vec![
                Tuner {
                    name: "c".into(),
                    lo: Some(0.01),
                    hi: Some(5.0),
                    start: Some(1.0),
                },
                Tuner {
                    name: "k".into(),
                    lo: Some(1.0),
                    hi: Some(20.0),
                    start: Some(4.0),
                },
            ],
            csv: csv.clone(),
            time_column: None,
            map: vec![
                DataMap {
                    column: "pos".into(),
                    var: "x".into(),
                    weight: None,
                },
                DataMap {
                    column: "energy".into(),
                    var: "energy".into(),
                    weight: None,
                },
            ],
            window: None,
            method,
            max_iter: 50,
            optimizer: Default::default(),
            gradient: Default::default(),
        };
        let mut cal = Calibration::new(m.clone(), o.clone(), spec).unwrap();
        run_job(&mut cal, &mut |_, _| true).unwrap();
        let r = cal.result();
        eprintln!("{}", Calibration::report_of(&r));
        assert!(r.converged, "{}", r.message);
        let (c, k) = (r.params[0].value, r.params[1].value);
        assert!((c - 0.4).abs() < 0.02, "{method:?} c = {c}");
        assert!((k - 6.0).abs() < 0.03, "{method:?} k = {k}");
        // The error estimates cover the truth and are of the right size.
        for (p, truth) in r.params.iter().zip([0.4, 6.0]) {
            assert!(
                p.std_error > 0.0 && p.std_error < 0.05,
                "{} {}",
                p.name,
                p.std_error
            );
            assert!(
                (p.value - truth).abs() < 4.0 * p.std_error,
                "{} {} +- {}",
                p.name,
                p.value,
                p.std_error
            );
        }
        assert!(r.rms_after < 0.1 * r.rms_before);
        // The position residual is down to the noise level.
        assert!(
            (r.columns[0].rms_after - 0.002).abs() < 0.0007,
            "{}",
            r.columns[0].rms_after
        );
        assert!(
            r.modifier.starts_with("(c = 0.") && r.modifier.contains("k = "),
            "{}",
            r.modifier
        );
    }
}

#[test]
fn calibration_respects_bounds() {
    let m = model("Analysis.Decay");
    let o = opts(&m, 1e-9, 100);
    let mut oo = o.clone();
    oo.params = vec![("k".into(), 1.2)];
    let (res, _) = daedalus_core::sim::simulate(&m, oo).unwrap();
    let x = res.values("x").unwrap();
    let mut csv = String::from("time,x\n");
    for (t, v) in res.time.iter().zip(&x) {
        csv.push_str(&format!("{t},{v}\n"));
    }
    let spec = CalibSpec {
        tuners: vec![Tuner {
            name: "k".into(),
            lo: Some(0.1),
            hi: Some(1.0),
            start: None,
        }],
        csv,
        time_column: None,
        map: vec![],
        window: Some((0.5, 2.5)),
        optimizer: Default::default(),
        gradient: Default::default(),
        method: SensMethod::Auto,
        max_iter: 30,
    };
    let mut cal = Calibration::new(m, o, spec).unwrap();
    run_job(&mut cal, &mut |_, _| true).unwrap();
    let r = cal.result();
    assert_eq!(r.params[0].value, 1.0);
    assert!(r.params[0].at_bound);
    let t0 = r.columns[0].t[0];
    assert!((0.5..0.53).contains(&t0), "{t0}");
}

#[test]
fn optimization_hits_a_target() {
    let m = model("Analysis.Linear");
    let o = opts(&m, 1e-9, 100);
    let spec = OptSpec {
        tuners: vec![
            Tuner {
                name: "b".into(),
                lo: Some(0.0),
                hi: Some(5.0),
                start: None,
            },
            Tuner {
                name: "a".into(),
                lo: Some(0.5),
                hi: Some(3.0),
                start: Some(2.0),
            },
        ],
        objective: vec![
            ObjTerm {
                qoi: "final(x)".into(),
                weight: 1.0,
                target: Some(2.0),
            },
            // small preference for small a: optimum at the lower bound of a
            ObjTerm {
                qoi: "max(x)".into(),
                weight: 0.0,
                target: None,
            },
        ],
        max_iter: 300,
        tol: 1e-12,
        algorithm: Default::default(),
        gradient: Default::default(),
    };
    let mut opt = Optimization::new(m, o, spec).unwrap();
    run_job(&mut opt, &mut |_, _| true).unwrap();
    let r = opt.result();
    assert!(r.objective_after < 1e-8, "{}", Optimization::report_of(&r));
    let (b, a) = (r.values[0], r.values[1]);
    let x_end = b / a * (1.0 - (-2.0 * a).exp());
    assert!((x_end - 2.0).abs() < 1e-3, "{x_end}");
    assert_eq!(idx(&r.params, "a"), 1);
}
