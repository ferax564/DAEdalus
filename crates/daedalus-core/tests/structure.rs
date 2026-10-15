//! Structure report, Jacobian analysis and state-selection overrides.

use daedalus_core::backend::Options;
use daedalus_core::backend::state_select::StateOverride;
use daedalus_core::frontend::Library;
use daedalus_core::linearize::{JacobianMethod, linearize};
use daedalus_core::sim::{SimOptions, simulate};
use daedalus_core::structure::{StructureOptions, StructureReport, as_modifiers, structure};

fn lib() -> Library {
    let path = format!("{}/tests/models/structure.mo", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    lib
}

fn options(overrides: &[&str]) -> Options {
    Options {
        state_overrides: overrides
            .iter()
            .map(|s| StateOverride::parse(s).unwrap())
            .collect(),
        ..Default::default()
    }
}

fn report(model: &str, overrides: &[&str], at: Option<f64>) -> StructureReport {
    let lib = lib();
    let opts = StructureOptions {
        jacobian: true,
        at,
        sim: None,
        params: Vec::new(),
    };
    let r = structure(&lib, model, &options(overrides), &opts).unwrap();
    assert!(r.jacobian_error.is_none(), "{:?}", r.jacobian_error);
    r
}

fn dense(r: &StructureReport) -> Vec<Vec<f64>> {
    let j = r.jacobian.as_ref().unwrap();
    let n = j.states.len();
    let mut a = vec![vec![0.0; n]; n];
    for &(i, k, v) in &j.entries {
        a[i as usize][k as usize] = v;
    }
    a
}

#[track_caller]
fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol * (1.0 + b.abs()), "{a} vs {b}");
}

/// The exact state Jacobian of the report agrees with central differences of the compiled
/// right-hand side at the same point.
#[test]
fn jacobian_matches_central_differences() {
    let lib = lib();
    for model in [
        "Struct.MassSpringDamper",
        "Struct.Loop",
        "Struct.Nonlinear",
        "Struct.Pendulum",
        "Struct.Gear",
    ] {
        let r = report(model, &[], None);
        let a = dense(&r);
        let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
        let so = SimOptions {
            tolerance: 1e-10,
            ..SimOptions::from_model(&m)
        };
        let fd = linearize(&m, so, None, JacobianMethod::CentralDifference).unwrap();
        assert_eq!(fd.states, r.jacobian.as_ref().unwrap().states, "{model}");
        for (ra, rf) in a.iter().zip(&fd.a) {
            for (x, y) in ra.iter().zip(rf) {
                close(*x, *y, 1e-6);
            }
        }
    }
    // After simulating: the nonlinear oscillator's Jacobian at t = 2, against the
    // analytic one of der(v) = -sin(x) - 0.3 v x^2 + 0.1 x v.
    let r = report("Struct.Nonlinear", &[], Some(2.0));
    let j = r.jacobian.as_ref().unwrap();
    close(j.time, 2.0, 1e-12);
    let ix = j.states.iter().position(|s| s == "x").unwrap();
    let iv = j.states.iter().position(|s| s == "v").unwrap();
    let (x, v) = (j.x0[ix], j.x0[iv]);
    let a = dense(&r);
    close(a[ix][iv], 1.0, 1e-12);
    close(a[ix][ix], 0.0, 1e-12);
    close(a[iv][ix], -x.cos() - 0.6 * v * x + 0.1 * v, 1e-10);
    close(a[iv][iv], -0.3 * x * x + 0.1 * x, 1e-10);
}

#[test]
fn mass_spring_damper_modes() {
    let r = report("Struct.MassSpringDamper", &[], None);
    let j = r.jacobian.as_ref().unwrap();
    assert_eq!(j.modes.len(), 2);
    // -c/2m +- i sqrt(k/m - (c/2m)^2) with m = 2, c = 0.5, k = 8
    let im = (4.0f64 - 0.125 * 0.125).sqrt();
    for md in &j.modes {
        close(md.re, -0.125, 1e-12);
        close(md.im.abs(), im, 1e-12);
        close(md.time_constant.unwrap(), 8.0, 1e-12);
        close(md.damping.unwrap(), 0.125 / 2.0, 1e-12);
        close(
            md.frequency.unwrap(),
            im / (2.0 * std::f64::consts::PI),
            1e-12,
        );
        let total: f64 = md.participation.iter().map(|p| p.1).sum();
        close(total, 1.0, 1e-12);
    }
    assert_eq!(j.suggested_solver, "non-stiff");
    // A = [[0, 1], [-4, -0.25]]: ||A||_1 = 4; A^-1 = [[-1/16, -1/4], [1, 0]]: ||A^-1||_1 = 17/16
    close(j.cond.unwrap(), 4.0 * 17.0 / 16.0, 1e-12);
}

#[test]
fn participation_and_stiffness() {
    let r = report("Struct.Stiff", &[], None);
    let j = r.jacobian.as_ref().unwrap();
    // A = [[-1, 0], [1, -1000]]: each mode belongs to one state.
    assert_eq!(j.modes.len(), 2);
    for md in &j.modes {
        let total: f64 = md.participation.iter().map(|p| p.1).sum();
        close(total, 1.0, 1e-12);
        let (k, f) = md.participation[0];
        let want = if md.re < -10.0 { "b" } else { "a" };
        assert_eq!(j.states[k], want);
        close(f, 1.0, 1e-9);
    }
    close(j.stiffness_ratio.unwrap(), 1000.0, 1e-9);
    close(j.fastest_rate.unwrap(), 1000.0, 1e-9);
    assert_eq!(j.suggested_solver, "stiff");
    // Coupled modes: participation of a nonsymmetric 2x2 system sums to 1 per mode,
    // with x and v sharing the underdamped modes equally.
    let r = report("Struct.Nonlinear", &[], Some(1.0));
    for md in &r.jacobian.as_ref().unwrap().modes {
        let total: f64 = md.participation.iter().map(|p| p.1).sum();
        close(total, 1.0, 1e-12);
    }
}

#[test]
fn blocks_of_an_algebraic_loop() {
    let r = report("Struct.Loop", &[], None);
    // der(x) := -y after the loop in (y, z)
    let loops: Vec<_> = r.blocks.iter().filter(|b| b.kind != "assign").collect();
    assert_eq!(loops.len(), 1, "{:#?}", r.blocks);
    let b = loops[0];
    assert_eq!(b.kind, "nonlinear");
    assert_eq!(b.size, 2);
    let mut vars = b.vars.clone();
    vars.sort();
    assert_eq!(vars, vec!["y", "z"]);
    assert!(b.torn);
    assert_eq!(b.tears.len(), 1);
    assert_eq!(b.inner, 1);
    assert_eq!(b.residuals, 1);
    // which equation computes which variable
    let solved = b.eqs.iter().find(|e| e.var.is_some()).unwrap();
    let text = &r.equations[solved.eq].text;
    assert!(text.contains('z') && text.contains('y'), "{text}");
    let assign = r
        .blocks
        .iter()
        .find(|b| b.vars == vec!["der(x)".to_string()])
        .unwrap();
    assert!(assign.index > b.index);
    assert_eq!(r.equations[assign.eqs[0].eq].text, "der(x) = -y");
    // roles
    let role = |n: &str| r.variables.iter().find(|v| v.name == n).unwrap().role;
    assert_eq!(role("x"), "state");
    assert_eq!(role("der(x)"), "derivative");
    assert_eq!(role("y"), "algebraic");
    // incidence in BLT order: square, blocks on the diagonal, lower triangular outside
    let inc = &r.incidence;
    assert_eq!(inc.rows.len(), 3);
    assert_eq!(inc.cols.len(), 3);
    assert_eq!(inc.block_starts, vec![0, 2, 3]);
    let block_of_row = |row: u32| {
        inc.block_starts
            .iter()
            .rposition(|&s| s as u32 <= row)
            .unwrap()
    };
    for &[row, col] in &inc.entries {
        assert!(
            block_of_row(col) <= block_of_row(row),
            "not BLT: {row},{col}"
        );
    }
    // Loop Jacobian w.r.t. the tear: finite and well conditioned at the start.
    let j = r.jacobian.as_ref().unwrap();
    assert_eq!(j.loops.len(), 1);
    assert!(!j.loops[0].singular);
    assert!(j.loops[0].cond < 10.0);
}

fn states(r: &StructureReport) -> Vec<String> {
    let mut s = r.states.clone();
    s.sort();
    s
}

/// Preferring the other inertia's angle swaps the states; the results stay the same.
#[test]
fn gear_state_override() {
    let base = report("Struct.Gear", &[], None);
    let swapped = report("Struct.Gear", &["phi2=prefer", "w2=prefer"], None);
    assert_ne!(states(&base), states(&swapped));
    assert_eq!(states(&swapped), vec!["phi2", "w2"]);
    let back = report(
        "Struct.Gear",
        &["phi*=avoid", "w*=avoid", "phi1=prefer", "w1=prefer"],
        None,
    );
    assert_eq!(states(&back), vec!["phi1", "w1"]);
    assert_eq!(swapped.overrides.len(), 2);
    assert!(swapped.state_sets.iter().any(|s| {
        s.candidates
            .iter()
            .any(|c| c.var == "phi2" && c.overridden && c.chosen == "state")
    }));
    assert!(
        swapped.state_reasons[0]
            .reason
            .contains("overridden from default"),
        "{:?}",
        swapped.state_reasons
    );
    // Same eigenvalues: (J1 + J2) der(w1) = -k phi1 gives +- i sqrt(k / (J1 + J2))
    let w = (5.0f64 / 3.0).sqrt();
    for r in [&base, &swapped] {
        for md in &r.jacobian.as_ref().unwrap().modes {
            close(md.re, 0.0, 1e-12);
            close(md.im.abs(), w, 1e-12);
        }
    }
    assert_eq!(
        as_modifiers(&swapped.overrides),
        "phi2(stateSelect = StateSelect.prefer),\nw2(stateSelect = StateSelect.prefer)"
    );
    // Simulations agree with each other and with phi1 = 0.2 cos(w t).
    let lib = lib();
    let run = |ov: &[&str]| {
        let m = daedalus_core::translate(&lib, "Struct.Gear", &options(ov)).unwrap();
        let so = SimOptions {
            tolerance: 1e-9,
            ..SimOptions::from_model(&m)
        };
        simulate(&m, so).unwrap().0
    };
    let (ra, rb) = (run(&[]), run(&["phi2=prefer", "w2=prefer"]));
    let pa = ra.values("phi1").unwrap();
    let pb = rb.values("phi1").unwrap();
    assert_eq!(ra.time, rb.time);
    for (k, &t) in ra.time.iter().enumerate() {
        close(pa[k], 0.2 * (w * t).cos(), 1e-6);
        close(pb[k], pa[k], 1e-6);
    }
}

/// A pendulum in cartesian coordinates with `y` as state passes through a singular state
/// choice at the bottom (x = 0): the report flags it as a dynamic state-selection case.
#[test]
fn pendulum_needs_dynamic_selection_with_y() {
    let good = report("Struct.Pendulum", &[], Some(1.0));
    assert_eq!(states(&good), vec!["vx", "x"]);
    assert!(good.state_sets.iter().all(|s| s.state_dependent));
    assert!(
        good.state_sets
            .iter()
            .all(|s| !s.worst.as_ref().unwrap().fails)
    );
    assert!(
        !good
            .warnings
            .iter()
            .any(|w| w.contains("dynamic state selection"))
    );

    let bad = report("Struct.Pendulum", &["y=prefer", "vy=prefer"], Some(1.0));
    assert_eq!(states(&bad), vec!["vy", "y"]);
    assert!(
        bad.state_sets
            .iter()
            .any(|s| s.worst.as_ref().is_some_and(|w| w.fails)),
        "{:#?}",
        bad.state_sets
    );
    assert!(
        bad.warnings
            .iter()
            .any(|w| w.contains("needs dynamic state selection")),
        "{:?}",
        bad.warnings
    );
}

/// The trace and an empty override list leave the translated system unchanged.
#[test]
fn trace_does_not_change_the_system() {
    let lib = lib();
    for model in ["Struct.Loop", "Struct.Pendulum", "Struct.Gear"] {
        let dump = |o: &Options| {
            let flat = lib.flatten(model).unwrap();
            let d = daedalus_core::backend::compile(flat, o).unwrap().dump();
            // all but the timing
            d.split("\"backend_ms\"").next().unwrap().to_string()
        };
        let plain = dump(&Options::default());
        let traced = dump(&Options {
            trace: true,
            ..Default::default()
        });
        assert_eq!(plain, traced, "{model}");
    }
    // An override that matches nothing warns.
    let flat = lib.flatten("Struct.Gear").unwrap();
    let sys = daedalus_core::backend::compile(flat, &options(&["nothing=prefer"])).unwrap();
    assert!(sys.warnings.iter().any(|w| w.contains("matches no")));
}
