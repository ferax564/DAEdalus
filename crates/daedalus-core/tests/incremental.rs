//! Runs in slices (`sim::Incremental`): live plotting and interactive dashboards.

use daedalus_core::codegen::Model;
use daedalus_core::frontend::Library;
use daedalus_core::sim::{Incremental, Method, SimOptions, simulate};

const SRC: &str = r#"
package L
  model Ball "bouncing ball: state events and reinit"
    parameter Real e = 0.8;
    parameter Real g = 9.81;
    Real h(start = 1, fixed = true);
    Real v(start = 0, fixed = true);
  equation
    der(h) = v;
    der(v) = -g;
    when h < 0 then
      reinit(v, -e * pre(v));
    end when;
  end Ball;

  model Sampled "time events of sample()"
    discrete Integer n(start = 0, fixed = true);
    Real x(start = 0, fixed = true);
  equation
    der(x) = 1 + n;
    when sample(0, 0.1) then
      n = pre(n) + 1;
    end when;
  end Sampled;

  model Gain "a parameter bound to another one, and a threshold"
    parameter Real k = 1;
    parameter Real k2 = 2 * k;
    parameter Real limit = 10;
    parameter Integer mode = 1;
    Real x(start = 0, fixed = true);
    Real y;
    Boolean high;
  equation
    der(x) = k;
    y = k2 * x;
    high = x > limit;
  end Gain;
end L;
"#;

fn model(name: &str) -> Model {
    let mut lib = Library::new();
    let errs = lib.set_document("incremental.mo", SRC);
    assert!(errs.is_empty(), "{errs:?}");
    daedalus_core::translate(&lib, name, &Default::default()).unwrap()
}

fn opts(m: &Model, stop: f64, method: Method) -> SimOptions {
    SimOptions {
        stop_time: stop,
        method,
        intervals: 200,
        ..SimOptions::from_model(m)
    }
}

/// Slices that end only on the wall-clock budget take the same steps as a full run.
#[test]
fn budget_slices_reproduce_the_full_run_exactly() {
    for (name, method) in [("L.Ball", Method::Bdf), ("L.Ball", Method::Dopri5), ("L.Sampled", Method::Bdf)] {
        let m = model(name);
        let o = opts(&m, 3.0, method);
        let (full, _) = simulate(&m, o.clone()).unwrap();
        let mut inc = Incremental::new(&m, o).unwrap();
        let mut slices = 0;
        // A budget this small ends a slice every few steps.
        while !inc.advance(f64::INFINITY, 1e-9).unwrap() {
            slices += 1;
            assert!(slices < 1_000_000);
        }
        assert!(slices > 3, "{name}: only {slices} slices");
        let (r, stats, err) = inc.finish();
        assert!(err.is_none());
        assert_eq!(stats.output_points, r.time.len());
        assert_eq!(r.time, full.time, "{name} {method:?}: time axis");
        assert_eq!(r.series, full.series, "{name} {method:?}: values");
    }
}

/// Slices ending on intermediate times (a paced run) stay as accurate as a full run, and
/// see every time event once, also those exactly on a slice boundary.
#[test]
fn time_slices_match_the_full_run() {
    let m = model("L.Sampled");
    let o = opts(&m, 1.05, Method::Bdf);
    let (full, _) = simulate(&m, o.clone()).unwrap();
    let mut inc = Incremental::new(&m, o).unwrap();
    let mut t = 0.0;
    while !inc.finished() {
        // slice ends on sample instants (0.1, 0.2, ...) and between them
        t += 0.05;
        inc.advance(t, 0.0).unwrap();
        assert!(inc.time() <= t + 1e-12);
        // rows are readable between slices
        assert_eq!(*inc.results().time.last().unwrap() <= inc.time() + 1e-12, true);
    }
    let (r, _, _) = inc.finish();
    assert_eq!(r.final_value("n"), full.final_value("n"));
    let (a, b) = (r.final_value("x").unwrap(), full.final_value("x").unwrap());
    assert!((a - b).abs() < 1e-6, "x: {a} vs {b}");

    let m = model("L.Ball");
    let o = opts(&m, 2.0, Method::Bdf);
    let (full, _) = simulate(&m, o.clone()).unwrap();
    let mut inc = Incremental::new(&m, o).unwrap();
    let mut t = 0.0;
    while !inc.advance(t, 0.0).unwrap() {
        t += 0.037;
    }
    let (r, _, _) = inc.finish();
    for v in ["h", "v"] {
        let (a, b) = (r.final_value(v).unwrap(), full.final_value(v).unwrap());
        assert!((a - b).abs() < 1e-3, "{v}: {a} vs {b}");
    }
}

/// A parameter changed mid-run takes effect from that time on; parameters bound to it
/// follow; states keep their values.
#[test]
fn set_params_mid_run() {
    let m = model("L.Gain");
    let o = opts(&m, 1.0, Method::Bdf);
    let mut inc = Incremental::new(&m, o).unwrap();
    inc.advance(0.5, 0.0).unwrap();
    inc.set_params(&[("k".into(), 3.0)]).unwrap();
    while !inc.advance(f64::INFINITY, 0.0).unwrap() {}
    let (r, _, err) = inc.finish();
    assert!(err.is_none());
    let x = r.final_value("x").unwrap();
    assert!((x - 2.0).abs() < 1e-6, "x(1) = 0.5 * 1 + 0.5 * 3 = 2, got {x}");
    let y = r.final_value("y").unwrap();
    assert!((y - 12.0).abs() < 1e-5, "y = k2 * x with k2 = 2 * 3, got {y}");
    // The result lists the parameters as they are at the end.
    assert_eq!(r.final_value("k2"), Some(6.0));
    // Mid-run, x is still continuous: the value at t = 0.5 is 0.5.
    let i = r.time.iter().position(|&t| (t - 0.5).abs() < 1e-9).unwrap();
    let xs = r.values("x").unwrap();
    assert!((xs[i] - 0.5).abs() < 1e-6);
}

/// A change at the start time gives the same run as the override from the start.
#[test]
fn set_params_at_start_equals_override() {
    let m = model("L.Gain");
    let o = opts(&m, 1.0, Method::Dopri5);
    let mut with = o.clone();
    with.params = vec![("k".into(), 2.5)];
    let (full, _) = simulate(&m, with).unwrap();
    let mut inc = Incremental::new(&m, o).unwrap();
    inc.set_params(&[("k".into(), 2.5)]).unwrap();
    while !inc.advance(f64::INFINITY, 0.0).unwrap() {}
    let (r, _, _) = inc.finish();
    for v in ["x", "y"] {
        let (a, b) = (r.final_value(v).unwrap(), full.final_value(v).unwrap());
        assert!((a - b).abs() < 1e-9, "{v}: {a} vs {b}");
    }
}

/// A threshold lowered below the current value switches the relation at once (an event
/// iteration runs at the change).
#[test]
fn set_params_runs_an_event_iteration() {
    let m = model("L.Gain");
    let mut inc = Incremental::new(&m, opts(&m, 1.0, Method::Bdf)).unwrap();
    inc.advance(0.5, 0.0).unwrap();
    assert_eq!(inc.results().values("high").unwrap().last(), Some(&0.0));
    inc.set_params(&[("limit".into(), 0.2)]).unwrap();
    inc.advance(0.6, 0.0).unwrap();
    let high = inc.results().values("high").unwrap();
    assert_eq!(high.last(), Some(&1.0));
}

/// Only free Real parameters change at run time: others are refused and nothing changes.
#[test]
fn set_params_refuses_other_names() {
    let m = model("L.Gain");
    let mut inc = Incremental::new(&m, opts(&m, 1.0, Method::Bdf)).unwrap();
    inc.advance(0.3, 0.0).unwrap();
    for bad in ["k2", "mode", "x", "nope"] {
        let e = inc.set_params(&[("k".into(), 5.0), (bad.into(), 1.0)]).unwrap_err();
        assert!(e.contains(bad), "{e}");
    }
    while !inc.advance(f64::INFINITY, 0.0).unwrap() {}
    let (r, _, _) = inc.finish();
    let x = r.final_value("x").unwrap();
    assert!((x - 1.0).abs() < 1e-6, "k unchanged, got x(1) = {x}");
}

/// An initialization failure is reported by `new`; a run can be ended early with the
/// rows so far.
#[test]
fn finish_early_keeps_partial_rows() {
    let m = model("L.Ball");
    let mut inc = Incremental::new(&m, opts(&m, 3.0, Method::Bdf)).unwrap();
    inc.advance(1.0, 0.0).unwrap();
    assert!(!inc.finished());
    let (r, stats, err) = inc.finish();
    assert!(err.is_none());
    assert!(r.time.len() > 10 && *r.time.last().unwrap() <= 1.0 + 1e-9);
    assert!(stats.solver.steps > 0);
}
