//! Linearization against hand-derived state-space matrices.

use daedalus_core::frontend::Library;
use daedalus_core::linearize::{JacobianMethod, Linearization, linearize};
use daedalus_core::sim::SimOptions;

fn lin(model: &str, at: Option<f64>, method: JacobianMethod) -> Linearization {
    let path = format!("{}/tests/models/linearize.mo", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    let opts = SimOptions {
        tolerance: 1e-10,
        ..SimOptions::from_model(&m)
    };
    linearize(&m, opts, at, method).unwrap()
}

fn idx(names: &[String], n: &str) -> usize {
    names
        .iter()
        .position(|x| x == n)
        .unwrap_or_else(|| panic!("{n} not in {names:?}"))
}

#[track_caller]
fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol * (1.0 + b.abs()), "{a} vs {b}");
}

/// Matrices in the order of the given state names.
fn reorder(l: &Linearization, states: &[&str]) -> (Vec<Vec<f64>>, Vec<Vec<f64>>, Vec<Vec<f64>>) {
    let p: Vec<usize> = states.iter().map(|s| idx(&l.states, s)).collect();
    let a = p
        .iter()
        .map(|&i| p.iter().map(|&j| l.a[i][j]).collect())
        .collect();
    let b = p.iter().map(|&i| l.b[i].clone()).collect();
    let c =
        l.c.iter()
            .map(|row| p.iter().map(|&j| row[j]).collect())
            .collect();
    (a, b, c)
}

fn check(got: &[Vec<f64>], want: &[Vec<f64>], tol: f64) {
    assert_eq!(got.len(), want.len());
    for (g, w) in got.iter().zip(want) {
        assert_eq!(g.len(), w.len());
        for (x, y) in g.iter().zip(w) {
            close(*x, *y, tol);
        }
    }
}

#[test]
fn cubic_table_second_derivative() {
    // der(x3) = s'' - x3 with s = T(x1) (Akima table), der(x1) = x2 and
    // der(x2) = u - x1: s'' = T''(x1) x2^2 + T'(x1) (u - x1), so the exact Jacobian
    // needs the third derivative of the table (the tangent of its second derivative).
    // (index reduction may pick s and ds as states in place of x1 and x2)
    let ex = lin("Lin.CubicTable", None, JacobianMethod::Exact);
    let fd = lin("Lin.CubicTable", None, JacobianMethod::CentralDifference);
    let states: Vec<&str> = ex.states.iter().map(|s| s.as_str()).collect();
    assert_eq!(states.len(), 3, "{states:?}");
    let (a, b, c) = reorder(&ex, &states);
    let (fa, fb, fc) = reorder(&fd, &states);
    check(&a, &fa, 1e-6);
    check(&b, &fb, 1e-6);
    check(&c, &fc, 1e-6);
    check(&ex.d, &fd.d, 1e-6);
    // the row of der(x3) depends on the first two states through T'''
    let r = idx(&ex.states, "x3");
    assert!(
        a[r].iter()
            .enumerate()
            .any(|(j, v)| j != r && v.abs() > 1e-3),
        "{a:?}"
    );
}

#[test]
fn mass_spring_damper() {
    for method in [JacobianMethod::Exact, JacobianMethod::CentralDifference] {
        let l = lin("Lin.MassSpringDamper", None, method);
        assert_eq!(l.method, method);
        assert_eq!(l.inputs, vec!["f"]);
        assert_eq!(l.outputs, vec!["pos"]);
        let (a, b, c) = reorder(&l, &["x", "v"]);
        let tol = if method == JacobianMethod::Exact {
            1e-14
        } else {
            1e-8
        };
        check(&a, &[vec![0.0, 1.0], vec![-4.0, -0.25]], tol);
        check(&b, &[vec![0.0], vec![0.5]], tol);
        check(&c, &[vec![1.0, 0.0]], tol);
        check(&l.d, &[vec![0.0]], tol);
        close(l.x0[idx(&l.states, "x")], 0.1, 1e-14);
        // eigenvalues: -c/2m +- i sqrt(k/m - (c/2m)^2)
        let e = l.eigenvalues();
        let im = (4.0f64 - 0.125 * 0.125).sqrt();
        close(e[0].0, -0.125, 1e-10);
        close(e[0].1.abs(), im, 1e-10);
        close(e[1].1.abs(), im, 1e-10);
    }
}

#[test]
fn algebraic_loop() {
    let l = lin("Lin.AlgebraicLoop", None, JacobianMethod::Exact);
    assert_eq!(l.states, vec!["x"]);
    close(l.u0[0], 0.3, 0.0);
    let b = l.y0[0];
    // da + 0.5 cos(b) db = dx + du, db = da - 0.3 b^2 db
    let g = 1.0 / (1.0 + 0.3 * b * b + 0.5 * b.cos());
    let da = (1.0 + 0.3 * b * b) * g;
    check(&l.a, &[vec![-da]], 1e-12);
    check(&l.b, &[vec![-da]], 1e-12);
    check(&l.c, &[vec![g]], 1e-12);
    check(&l.d, &[vec![g]], 1e-12);
    // the point solves the loop
    let a = b + 0.1 * b.powi(3);
    close(a + 0.5 * b.sin(), 1.0 + 0.3, 1e-10);
    let fd = lin("Lin.AlgebraicLoop", None, JacobianMethod::CentralDifference);
    for (x, y) in [(&fd.a, &l.a), (&fd.b, &l.b), (&fd.c, &l.c), (&fd.d, &l.d)] {
        check(x, y, 1e-7);
    }
}

#[test]
fn index_reduction() {
    let l = lin("Lin.RigidMasses", None, JacobianMethod::Exact);
    assert_eq!(l.states.len(), 2, "{:?}", l.states);
    let pos = l
        .states
        .iter()
        .find(|s| s.starts_with('x'))
        .unwrap()
        .clone();
    let vel = l
        .states
        .iter()
        .find(|s| s.starts_with('v'))
        .unwrap()
        .clone();
    let (a, b, c) = reorder(&l, &[&pos, &vel]);
    // (m1 + m2) der(v) = f - k x
    check(&a, &[vec![0.0, 1.0], vec![-2.5, 0.0]], 1e-12);
    check(&b, &[vec![0.0], vec![0.25]], 1e-12);
    check(&c, &[vec![1.0, 0.0]], 1e-12);
    check(&l.d, &[vec![0.0]], 1e-12);
}

#[test]
fn pendulum_after_simulation() {
    let l = lin("Lin.Pendulum", Some(2.0), JacobianMethod::Exact);
    close(l.time, 2.0, 0.0);
    let phi = l.x0[idx(&l.states, "phi")];
    assert!((phi - 0.5).abs() > 1e-3, "the point moved: {phi}");
    let (a, b, c) = reorder(&l, &["phi", "w"]);
    check(&a, &[vec![0.0, 1.0], vec![-9.81 * phi.cos(), -0.2]], 1e-12);
    check(&b, &[vec![0.0], vec![1.0]], 1e-12);
    assert_eq!(l.outputs, vec!["angle", "height"]);
    check(&c, &[vec![1.0, 0.0], vec![phi.sin(), 0.0]], 1e-12);
    check(&l.d, &[vec![0.0], vec![0.0]], 1e-12);
    let fd = lin("Lin.Pendulum", Some(2.0), JacobianMethod::CentralDifference);
    check(&fd.a, &l.a, 1e-7);
    check(&fd.c, &l.c, 1e-7);
    // the Modelica model OpenModelica's linearize writes
    let mo = l.to_modelica();
    assert!(mo.starts_with("model linearized_model"));
    assert!(mo.contains("der(x) = A * x + B * u;"));
    assert!(mo.contains("Real 'x_phi' = x["));
}

#[test]
fn linearized_model_translates() {
    // The written model is valid Modelica that DAEdalus itself can translate.
    let l = lin("Lin.MassSpringDamper", None, JacobianMethod::Exact);
    let mo = l.to_modelica();
    let mut lib = Library::new();
    let errs = lib.set_document("linearized_model.mo", &mo);
    assert!(errs.is_empty(), "{errs:?}\n{mo}");
    let m = daedalus_core::translate(&lib, "linearized_model", &Default::default()).unwrap();
    let l2 = linearize(&m, SimOptions::from_model(&m), None, JacobianMethod::Exact).unwrap();
    assert_eq!(l2.states.len(), 2);
    let e1 = l.eigenvalues();
    let e2 = l2.eigenvalues();
    for (p, q) in e1.iter().zip(&e2) {
        close(p.0, q.0, 1e-12);
        close(p.1, q.1, 1e-12);
    }
}

#[test]
fn eigenvalues_of_larger_matrices() {
    // Block-diagonal with a rotation block and a dense similarity transform.
    let d = vec![
        vec![-1.0, 0.0, 0.0, 0.0, 0.0],
        vec![0.0, -2.0, 0.0, 0.0, 0.0],
        vec![0.0, 0.0, -3.0, 0.0, 0.0],
        vec![0.0, 0.0, 0.0, -1.0, 2.0],
        vec![0.0, 0.0, 0.0, -2.0, -1.0],
    ];
    // T = I + upper ones (unit upper triangular), T^-1 known: alternating
    let n = 5;
    let t: Vec<Vec<f64>> = (0..n)
        .map(|i| (0..n).map(|j| if j >= i { 1.0 } else { 0.0 }).collect())
        .collect();
    let ti: Vec<Vec<f64>> = (0..n)
        .map(|i| {
            (0..n)
                .map(|j| {
                    if j == i {
                        1.0
                    } else if j == i + 1 {
                        -1.0
                    } else {
                        0.0
                    }
                })
                .collect()
        })
        .collect();
    let mul = |a: &Vec<Vec<f64>>, b: &Vec<Vec<f64>>| -> Vec<Vec<f64>> {
        (0..n)
            .map(|i| {
                (0..n)
                    .map(|j| (0..n).map(|k| a[i][k] * b[k][j]).sum())
                    .collect()
            })
            .collect()
    };
    let a = mul(&mul(&t, &d), &ti);
    let e = daedalus_core::linearize::eigenvalues(&a);
    assert_eq!(e.len(), 5);
    let want = [
        (-3.0, 0.0),
        (-2.0, 0.0),
        (-1.0, -2.0),
        (-1.0, 0.0),
        (-1.0, 2.0),
    ];
    for w in &want {
        assert!(
            e.iter()
                .any(|g| (g.0 - w.0).abs() < 1e-9 && (g.1 - w.1).abs() < 1e-9),
            "{w:?} not in {e:?}"
        );
    }
}
