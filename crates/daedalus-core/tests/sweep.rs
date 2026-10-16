//! Parameter sweeps: value parsing, grid/grouped expansion, and a parallel sweep over a
//! model with an analytic solution, mixing run-time and structural parameters.

use daedalus_core::frontend::Library;
use daedalus_core::sim::SimOptions;
use daedalus_core::sweep::*;

#[test]
fn parse_value_lists() {
    assert_eq!(parse_values("1,2,3").unwrap(), vec![1.0, 2.0, 3.0]);
    assert_eq!(parse_values(" 0.5 , -1e-3 ").unwrap(), vec![0.5, -1e-3]);
    assert_eq!(
        parse_values("lin:0:10:11").unwrap(),
        (0..=10).map(f64::from).collect::<Vec<_>>()
    );
    assert_eq!(parse_values("lin:2:4:1").unwrap(), vec![2.0]);
    let l = parse_values("log:1e-3:1:7").unwrap();
    assert_eq!(l.len(), 7);
    assert_eq!((l[0], l[6]), (1e-3, 1.0));
    for (i, x) in l.iter().enumerate() {
        let want = 10f64.powf(-3.0 + 0.5 * i as f64);
        assert!((x - want).abs() <= 1e-12 * want, "{i}: {x} vs {want}");
    }
    for bad in ["", "a,b", "lin:0:1", "lin:0:1:0", "log:0:1:3", "log:1:x:3"] {
        assert!(parse_values(bad).is_err(), "{bad}");
    }
    let p = parse_param("a.b.k=lin:1:2:3").unwrap();
    assert_eq!(p.name, "a.b.k");
    assert_eq!(p.values, vec![1.0, 1.5, 2.0]);
    assert!(parse_param("k").is_err());
    assert!(parse_param("=1,2").is_err());
    assert_eq!(Stat::parse("at=0.5").unwrap(), Stat::At(0.5));
    assert!(Stat::parse("median").is_err());
}

#[test]
fn grid_and_grouped_expansion() {
    let a = SweepParam {
        name: "a".into(),
        values: vec![1.0, 2.0],
    };
    let b = SweepParam {
        name: "b".into(),
        values: vec![10.0, 20.0, 30.0],
    };
    let g = expand(&[a.clone(), b.clone()], Combine::Grid).unwrap();
    let flat: Vec<(f64, f64)> = g.iter().map(|p| (p[0].1, p[1].1)).collect();
    assert_eq!(
        flat,
        vec![
            (1.0, 10.0),
            (1.0, 20.0),
            (1.0, 30.0),
            (2.0, 10.0),
            (2.0, 20.0),
            (2.0, 30.0)
        ]
    );
    assert!(g.iter().all(|p| p[0].0 == "a" && p[1].0 == "b"));
    // grouped needs equal lengths
    assert!(expand(&[a.clone(), b.clone()], Combine::Grouped).is_err());
    let c = SweepParam {
        name: "c".into(),
        values: vec![5.0, 6.0],
    };
    let z = expand(&[a.clone(), c], Combine::Grouped).unwrap();
    assert_eq!(
        z,
        vec![
            vec![("a".into(), 1.0), ("c".into(), 5.0)],
            vec![("a".into(), 2.0), ("c".into(), 6.0)]
        ]
    );
    assert!(expand(&[a.clone(), a], Combine::Grid).is_err());
    assert_eq!(expand(&[], Combine::Grid).unwrap(), vec![Vec::new()]);
}

#[test]
fn parallel_sweep_matches_analytic_solution() {
    let path = format!("{}/tests/models/sweep.mo", env!("CARGO_MANIFEST_DIR"));
    let mut lib = Library::new();
    assert!(
        lib.set_document(&path, &std::fs::read_to_string(&path).unwrap())
            .is_empty()
    );
    let model = "Sweep.Decay";
    let base = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    assert_eq!(classify(&base, "k"), ParamClass::Runtime);
    assert_eq!(classify(&base, "x0"), ParamClass::Runtime);
    assert_eq!(classify(&base, "n"), ParamClass::Structural);
    assert_eq!(classify(&base, "k2"), ParamClass::Structural);

    let params = [
        parse_param("k=lin:0.5:2:3").unwrap(),
        parse_param("x0=1,3").unwrap(),
        parse_param("n=1,2,3").unwrap(),
    ];
    let points = expand(&params, Combine::Grid).unwrap();
    assert_eq!(points.len(), 18);
    // one translation per distinct structural combination
    let mut models: Vec<(Vec<(String, f64)>, daedalus_core::codegen::Model)> = vec![];
    let mut model_of = vec![];
    let mut runtime = vec![];
    for p in &points {
        let (rt, st) = split_overrides(&base, p, false);
        assert_eq!(st.len(), 1);
        let i = match models.iter().position(|(s, _)| s == &st) {
            Some(i) => i,
            None => {
                let m = translate_with_modifiers(
                    &mut lib,
                    model,
                    &st,
                    Some(&base),
                    &Default::default(),
                )
                .unwrap();
                assert_eq!(m.name, model);
                models.push((st, m));
                models.len() - 1
            }
        };
        model_of.push(i);
        runtime.push(rt);
    }
    assert_eq!(models.len(), 3);
    let opts = SimOptions {
        tolerance: 1e-9,
        ..SimOptions::from_model(&base)
    };
    let vars = vec!["y".to_string(), "z".to_string()];
    let finished = std::sync::atomic::AtomicUsize::new(0);
    // The first two runs wait (up to 10 s) until they are both in flight: proof that
    // runs execute concurrently and not one after the other.
    let inside = std::sync::atomic::AtomicUsize::new(0);
    let peak = std::sync::atomic::AtomicUsize::new(0);
    let runs = run_parallel(
        points.len(),
        4,
        None,
        |i| {
            use std::sync::atomic::Ordering::SeqCst;
            peak.fetch_max(inside.fetch_add(1, SeqCst) + 1, SeqCst);
            let t0 = std::time::Instant::now();
            while i < 2 && peak.load(SeqCst) < 2 && t0.elapsed().as_secs() < 10 {
                std::thread::yield_now();
            }
            let r = simulate_run(&models[model_of[i]].1, &opts, &runtime[i]);
            inside.fetch_sub(1, SeqCst);
            r.map(|(res, _)| subset(&res, &vars))
        },
        |_, _| {
            finished.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        },
    );
    assert_eq!(finished.into_inner(), 18);
    assert!(peak.into_inner() >= 2, "runs did not overlap");
    for (i, r) in runs.into_iter().enumerate() {
        let res = r.unwrap().map_err(|e| e.message).unwrap();
        let get = |n: &str| points[i].iter().find(|(m, _)| m == n).unwrap().1;
        let (k, x0, n) = (get("k"), get("x0"), get("n"));
        for t in [0.25, 0.5, 1.0] {
            let y = res.at("y", t).unwrap();
            let want = n * x0 * (-k * t).exp();
            assert!(
                (y - want).abs() < 1e-6 * want,
                "run {i} (k={k} x0={x0} n={n}) t={t}: {y} vs {want}"
            );
            // k2 = 2 k keeps following the overridden k
            let z = res.at("z", t).unwrap();
            assert!(
                (z - 2.0 * k * t).abs() < 1e-6,
                "run {i}: z {z} vs {}",
                2.0 * k * t
            );
        }
        assert_eq!(Stat::Final.eval(&res, "y"), res.final_value("y"));
        assert!((Stat::Max.eval(&res, "y").unwrap() - n * x0).abs() < 1e-9);
        // time average of n x0 exp(-k t) over [0, 1]
        let mean = n * x0 * (1.0 - (-k).exp()) / k;
        assert!((Stat::Mean.eval(&res, "y").unwrap() - mean).abs() < 1e-4 * mean);
    }
}

#[test]
fn structural_override_of_computed_parameter() {
    // k2 is computed (2 k): a modifier replaces its binding.
    let path = format!("{}/tests/models/sweep.mo", env!("CARGO_MANIFEST_DIR"));
    let mut lib = Library::new();
    lib.set_document(&path, &std::fs::read_to_string(&path).unwrap());
    let base = daedalus_core::translate(&lib, "Sweep.Decay", &Default::default()).unwrap();
    let m = translate_with_modifiers(
        &mut lib,
        "Sweep.Decay",
        &[("k2".into(), 5.0)],
        Some(&base),
        &Default::default(),
    )
    .unwrap();
    let (res, _) = simulate_run(&m, &SimOptions::from_model(&base), &[])
        .map_err(|e| e.message)
        .unwrap();
    assert!((res.final_value("z").unwrap() - 5.0).abs() < 1e-6);
    // the wrapper is gone again
    assert!(lib.docs.iter().all(|d| !d.uri.contains("sweep__")));
}
