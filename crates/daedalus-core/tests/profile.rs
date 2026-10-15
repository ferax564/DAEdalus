//! Performance analyzer and event log: step diagnostics, events and chattering checked
//! against analytic solutions, and profiling must not change the results.

use daedalus_core::frontend::Library;
use daedalus_core::profile::{CHATTER_COUNT, Profile};
use daedalus_core::results::Results;
use daedalus_core::sim::{Method, SimOptions, simulate, simulate_profiled};

fn model(file: &str, name: &str) -> daedalus_core::codegen::Model {
    let path = format!("{}/tests/models/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    daedalus_core::translate(&lib, name, &Default::default()).unwrap()
}

fn profiled(
    m: &daedalus_core::codegen::Model,
    opts: SimOptions,
) -> (Result<Results, String>, Profile) {
    let (r, p) = simulate_profiled(
        m,
        SimOptions {
            profile: true,
            ..opts
        },
    );
    (r.map(|x| x.0).map_err(|e| e.message), p.expect("profile"))
}

/// Two decoupled states, time constants 1 ms and 1 s: under Dopri5 the fast one sets
/// every step (stability), long after it has decayed.
#[test]
fn stiff_state_limits_the_explicit_step() {
    let m = model("profile.mo", "Prof.TwoScales");
    let opts = SimOptions {
        stop_time: 5.0,
        method: Method::Dopri5,
        tolerance: 1e-6,
        ..SimOptions::from_model(&m)
    };
    let (r, p) = profiled(&m, opts.clone());
    let r = r.unwrap();
    assert!((r.final_value("slow").unwrap() - (-5.0f64).exp()).abs() < 1e-5);
    assert_eq!(p.states, ["fast", "slow"]);
    let top = &p.limiting[0];
    assert_eq!(top.name, "fast");
    assert!(top.share > 0.95, "fast limited only {:.3}", top.share);
    assert_eq!(p.steps.t.len() as u64, p.solver.steps);
    assert_eq!(p.steps_total, p.solver.steps);
    assert_eq!(p.rejected.len() as u64, p.solver.rejected);
    // The accepted steps tile the interval.
    let sum: f64 = p.steps.h.iter().sum();
    assert!((sum - 5.0).abs() < 1e-9, "{sum}");
    // Every accepted step passed the error test, and the recorded norm is the RMS of the
    // recorded per-state terms, whose largest is the dominant state.
    let n = p.states.len();
    assert_eq!(p.step_errors.len(), n * p.steps.t.len());
    for (k, &e) in p.steps.err.iter().enumerate() {
        assert!(e <= 1.0 + 1e-12, "step {k}: err {e}");
        let row = &p.step_errors[k * n..k * n + n];
        let rms = (row.iter().map(|&x| (x as f64).powi(2)).sum::<f64>() / n as f64).sqrt();
        assert!((rms - e).abs() <= 1e-6 * e.max(1e-30), "{rms} vs {e}");
        let dom = if row[0] >= row[1] { 0 } else { 1 };
        assert_eq!(p.steps.dominant[k], dom);
        assert_eq!(p.steps.order[k], 5);
    }
    // Once the fast mode has decayed (t > 0.05), the step hovers around the stability
    // limit of Dopri5 on the negative real axis, h * 1000 = about 3.3, independent of
    // the tolerance: the profile shows a stability-bound, not accuracy-bound, step.
    let late: Vec<f64> = p
        .steps
        .t
        .iter()
        .zip(&p.steps.h)
        .filter(|(t, _)| **t > 0.5)
        .map(|(_, h)| *h)
        .collect();
    let hmax = late.iter().cloned().fold(0.0, f64::max);
    let hmean = late.iter().sum::<f64>() / late.len() as f64;
    assert!(
        hmax * 1000.0 < 4.0 && (2.9..3.5).contains(&(hmean * 1000.0)),
        "{hmax} {hmean}"
    );
    assert!(
        p.hints
            .iter()
            .any(|h| h.contains("`fast`") && h.contains("BDF"))
    );
    // BDF on the same model: far fewer steps, and the explicit hint is not given.
    let (_, pb) = profiled(
        &m,
        SimOptions {
            method: Method::Bdf,
            ..opts
        },
    );
    assert!(pb.solver.steps * 5 < p.solver.steps);
    assert!(pb.steps.order.iter().all(|&o| (1..=5).contains(&o)));
    assert!(!pb.hints.iter().any(|h| h.contains("BDF")));
}

/// Bouncing ball: one logged reinit per bounce, at the analytic impact times, with the
/// velocity before and after.
#[test]
fn bouncing_ball_event_log() {
    let m = model("events.mo", "E.Ball");
    let opts = SimOptions {
        stop_time: 3.0,
        tolerance: 1e-8,
        ..SimOptions::from_model(&m)
    };
    let (r, p) = profiled(&m, opts);
    let r = r.unwrap();
    let (g, e, h0) = (9.81f64, 0.8f64, 1.0f64);
    // Impact times: t1 = sqrt(2 h0 / g), then flights of 2 v_k / g with v_k = e^k v1.
    let v1 = (2.0 * g * h0).sqrt();
    let mut t = v1 / g;
    let mut v = v1;
    let mut impacts = Vec::new();
    while t < 3.0 {
        impacts.push((t, v));
        v *= e;
        t += 2.0 * v / g;
    }
    let bounces: Vec<_> = p
        .events
        .iter()
        .filter(|ev| ev.tags.iter().any(|x| x == "reinit"))
        .collect();
    assert_eq!(bounces.len(), impacts.len());
    assert_eq!(r.final_value("bounces").unwrap(), impacts.len() as f64);
    for (ev, &(ta, va)) in bounces.iter().zip(&impacts) {
        assert_eq!(ev.kind, "state");
        assert_eq!(ev.text, "h < 0");
        assert!((ev.t - ta).abs() < 1e-6, "{} vs {ta}", ev.t);
        assert!(ev.whens[0].contains("reinit(v)"));
        let c = ev.changes.iter().find(|c| c.name == "v").unwrap();
        assert!((c.before + va).abs() < 1e-5, "{} vs {}", c.before, -va);
        assert!((c.after - e * va).abs() < 1e-5);
    }
    // h < 0 switches on at each impact and off as the ball leaves the ground.
    let rel = &p.relations[0];
    assert_eq!(rel.text, "h < 0");
    assert_eq!(rel.count, 2 * impacts.len() as u64);
    assert_eq!(rel.rising, impacts.len() as u64);
    assert!((rel.first - impacts[0].0).abs() < 1e-6);
    assert!(rel.chattering.is_none());
    assert_eq!(p.events_total, p.events.len() as u64);
    assert!(p.event_log_text().contains("reinit(v)"));
}

/// `der(x) = if x > 0 then -1 else 1` from x = 1 reaches the switching surface at t = 1
/// and then chatters on it.
#[test]
fn chattering_relation_is_detected() {
    let m = model("profile.mo", "Prof.Chatter");
    let opts = SimOptions {
        stop_time: 2.0,
        ..SimOptions::from_model(&m)
    };
    let (r, p) = profiled(&m, opts);
    // The run stops on the chattering; the profile up to there is still returned.
    assert!(r.unwrap_err().contains("chatter"));
    let rel = &p.relations[0];
    assert_eq!(rel.text, "x > 0");
    let c = rel.chattering.as_ref().expect("chattering");
    assert!(c.count as usize >= CHATTER_COUNT);
    assert!((c.from - 1.0).abs() < 1e-6 && (c.to - 1.0).abs() < 1e-6);
    assert!(rel.min_interval < 1e-6);
    assert!(
        p.hints
            .iter()
            .any(|h| h.contains("`x > 0`") && h.contains("chattering"))
    );
    assert!(p.summary_text().contains("CHATTERING"));
}

/// Profiling observes, it does not change the numbers: the same results bit for bit.
#[test]
fn profiling_does_not_change_results() {
    for (file, name, method) in [
        ("events.mo", "E.Ball", Method::Bdf),
        ("events.mo", "E.Sampled", Method::Dopri5),
        ("connectors.mo", "Lib.Test", Method::Bdf),
        ("index.mo", "T.Pendulum", Method::Bdf),
    ] {
        let m = model(file, name);
        let opts = SimOptions {
            stop_time: 0.5,
            method,
            ..SimOptions::from_model(&m)
        };
        let (a, sa) = simulate(&m, opts.clone()).unwrap();
        let (b, p) = profiled(&m, opts);
        let b = b.unwrap();
        assert_eq!(a.time, b.time, "{name}");
        assert_eq!(a.series, b.series, "{name}");
        assert_eq!(sa.solver.steps, p.solver.steps);
        assert_eq!(sa.solver.rhs_evals, p.solver.rhs_evals);
        // Profiling off returns no profile.
        assert!(
            simulate_profiled(&m, SimOptions::from_model(&m))
                .1
                .is_none()
        );
    }
}

/// Newton blocks are timed with their iterations and shown with their equations.
#[test]
fn newton_block_costs() {
    let m = model("index.mo", "T.Pendulum");
    let opts = SimOptions {
        stop_time: 0.5,
        ..SimOptions::from_model(&m)
    };
    let (_, p) = profiled(&m, opts);
    let total_calls: u64 = p.newton_blocks.iter().map(|b| b.calls).sum();
    let total_iters: u64 = p.newton_blocks.iter().map(|b| b.iterations).sum();
    assert_eq!(total_calls, p.newton_solves);
    assert_eq!(total_iters, p.newton_iterations);
    // The constraint x^2 + y^2 = L^2 solved for x is a nonlinear block of one unknown.
    let b = p
        .blocks
        .iter()
        .find(|b| b.kind == "nonlinear" && b.vars == ["x"] && b.calls > 1)
        .expect("nonlinear block for x");
    assert!(b.eqs[0].starts_with("0 = ") && b.eqs[0].contains('x'));
    assert!(b.iterations >= b.calls && b.time_ms > 0.0);
    let explicit = p.blocks.iter().find(|b| b.kind == "explicit").unwrap();
    assert!(explicit.calls > 0);
    let shares: f64 = p.blocks.iter().map(|b| b.share).sum();
    assert!(shares <= 1.0 + 1e-9, "{shares}");
    // JSON for the workbench.
    let j = serde_json::to_value(&p).unwrap();
    assert!(j["steps"]["h"].as_array().unwrap().len() as u64 == p.solver.steps);
}
