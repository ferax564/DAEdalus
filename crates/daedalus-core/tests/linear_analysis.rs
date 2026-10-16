//! Linear analysis (`daedalus_core::lti`) against analytic results.

use daedalus_core::frontend::Library;
use daedalus_core::linearize::{JacobianMethod, Linearization, linearize_with_outputs};
use daedalus_core::lti::*;
use daedalus_core::sim::SimOptions;

#[track_caller]
fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol * (1.0 + b.abs()), "{a} vs {b}");
}

fn m(rows: &[&[f64]]) -> Mat {
    let cols = rows.first().map_or(0, |r| r.len());
    Mat::from_rows(&rows.iter().map(|r| r.to_vec()).collect::<Vec<_>>(), cols)
}

/// `wn^2 / (s^2 + 2 zeta wn s + wn^2)` in controllable canonical form.
fn second_order(wn: f64, zeta: f64) -> (Mat, Mat, Mat, Mat) {
    (
        m(&[&[0.0, 1.0], &[-wn * wn, -2.0 * zeta * wn]]),
        m(&[&[0.0], &[1.0]]),
        m(&[&[wn * wn, 0.0]]),
        m(&[&[0.0]]),
    )
}

#[test]
fn bode_first_order() {
    // 1 / (s + 1): -3.01 dB and -45 deg at 1 rad/s, -20 dB/decade, phase -> -90
    let (a, b, c, d) = (m(&[&[-1.0]]), m(&[&[1.0]]), m(&[&[1.0]]), m(&[&[0.0]]));
    let ev = FreqEvaluator::new(&a, &b, &c, &d);
    let bd = bode(&ev, 0, 0, &[1e-3, 1.0, 1e3]);
    close(bd.mag_db[1], -10.0 * 2f64.log10(), 1e-12);
    close(bd.phase_deg[1], -45.0, 1e-12);
    close(bd.mag_db[2], -60.0, 1e-5);
    close(bd.phase_deg[2], -89.9427, 1e-5);
    close(bd.mag_db[0], 0.0, 1e-5);
    // Nyquist point at 1 rad/s: 1/(1+j) = 0.5 - 0.5j
    close(bd.re[1], 0.5, 1e-14);
    close(bd.im[1], -0.5, 1e-14);
}

#[test]
fn bode_second_order_resonance_and_margins() {
    let (wn, zeta) = (2.0, 0.1);
    let (a, b, c, d) = second_order(wn, zeta);
    let ev = FreqEvaluator::new(&a, &b, &c, &d);
    let bd = bode(&ev, 0, 0, &logspace(0.01, 100.0, 801));
    // at wn: |G| = 1 / (2 zeta), phase -90
    let k = bd
        .omega
        .iter()
        .position(|w| (w - wn).abs() < 1e-9)
        .unwrap_or_else(|| {
            // 801 points over 4 decades contain wn = 2? use direct evaluation instead
            usize::MAX
        });
    let at = if k == usize::MAX {
        bode(&ev, 0, 0, &[wn])
    } else {
        bd.clone()
    };
    let kk = if k == usize::MAX { 0 } else { k };
    close(at.mag_db[kk], 20.0 * (1.0 / (2.0 * zeta)).log10(), 1e-10);
    close(at.phase_deg[kk], -90.0, 1e-10);
    // phase decreases continuously towards -180 without wrapping
    assert!(bd.phase_deg.windows(2).all(|w| w[1] <= w[0] + 1e-9));
    close(*bd.phase_deg.last().unwrap(), -180.0, 1e-2);
    // gain crossover: |G(jw)| = 1 -> w^4 + (4 zeta^2 - 2) wn^2 w^2 + wn^4 - wn^4 = 0 -> w^2 = (2 - 4 zeta^2) wn^2
    let mg = margins(&bd);
    let wc = wn * (2.0 - 4.0 * zeta * zeta).sqrt();
    close(mg.gain_crossover.unwrap(), wc, 1e-3);
    let ph = -(2.0 * zeta * wn * wc).atan2(wn * wn - wc * wc).to_degrees();
    close(mg.phase_margin_deg.unwrap(), 180.0 + ph, 1e-2);
    // a second-order lag never reaches -180: infinite gain margin
    assert!(mg.gain_margin_db.is_none());
}

#[test]
fn gain_margin_third_order() {
    // 1/(s+1)^3: phase crossover at sqrt(3) rad/s where |G| = 1/8: GM = 18.06 dB
    let a = m(&[&[-1.0, 1.0, 0.0], &[0.0, -1.0, 1.0], &[0.0, 0.0, -1.0]]);
    let b = m(&[&[0.0], &[0.0], &[1.0]]);
    let c = m(&[&[1.0, 0.0, 0.0]]);
    let d = m(&[&[0.0]]);
    let ev = FreqEvaluator::new(&a, &b, &c, &d);
    let bd = bode(&ev, 0, 0, &logspace(0.01, 100.0, 2001));
    let mg = margins(&bd);
    close(mg.phase_crossover.unwrap(), 3f64.sqrt(), 1e-3);
    close(mg.gain_margin_db.unwrap(), 20.0 * 8f64.log10(), 1e-3);
    assert!(mg.gain_crossover.is_none()); // |G| <= 1 everywhere
}

#[test]
fn zeros_strictly_proper_and_feedthrough() {
    // (s + 2) / ((s + 1)(s + 3)): A = [0 1; -3 -4], b = [0; 1], c = [2 1]
    let a = m(&[&[0.0, 1.0], &[-3.0, -4.0]]);
    let (z, rd, hf) = siso_zeros(&a, &[0.0, 1.0], &[2.0, 1.0], 0.0);
    assert_eq!(rd, Some(1));
    close(hf, 1.0, 1e-14);
    assert_eq!(z.len(), 1);
    close(z[0].0, -2.0, 1e-12);
    // relative degree 2: no finite zeros
    let (z, rd, _) = siso_zeros(&a, &[0.0, 1.0], &[1.0, 0.0], 0.0);
    assert_eq!(rd, Some(2));
    assert!(z.is_empty());
    // 1 + 1/(s+1) = (s + 2)/(s + 1)
    let (z, rd, hf) = siso_zeros(&m(&[&[-1.0]]), &[1.0], &[1.0], 1.0);
    assert_eq!((rd, hf), (Some(0), 1.0));
    close(z[0].0, -2.0, 1e-14);
    // complex zeros: (s^2 + 2 s + 5) / (s^3 + 6 s^2 + 11 s + 6), zeros -1 ± 2j
    let a = m(&[&[0.0, 1.0, 0.0], &[0.0, 0.0, 1.0], &[-6.0, -11.0, -6.0]]);
    let (z, rd, _) = siso_zeros(&a, &[0.0, 0.0, 1.0], &[5.0, 2.0, 1.0], 0.0);
    assert_eq!(rd, Some(1));
    assert_eq!(z.len(), 2);
    close(z[0].0, -1.0, 1e-10);
    close(z[0].1.abs(), 2.0, 1e-10);
    // a non-minimum-phase zero
    let a = m(&[&[0.0, 1.0], &[-2.0, -3.0]]);
    let (z, _, _) = siso_zeros(&a, &[0.0, 1.0], &[1.0, -1.0], 0.0); // (1 - s)/(s^2 + 3 s + 2)
    close(z[0].0, 1.0, 1e-12);
}

#[test]
fn transmission_zeros_skip_uncontrollable_and_unobservable_modes() {
    // (s + 2)/((s + 1)(s + 3)) plus an uncontrollable mode at -5 and an unobservable
    // one at -7: invariant zeros include -5 and -7, transmission zeros only -2.
    let a = m(&[
        &[0.0, 1.0, 0.0, 0.0],
        &[-3.0, -4.0, 1.0, 0.0],
        &[0.0, 0.0, -5.0, 0.0],
        &[0.0, 1.0, 0.0, -7.0],
    ]);
    let (b, c) = ([0.0, 1.0, 0.0, 0.0], [2.0, 1.0, 0.3, 0.0]);
    let (zi, _, _) = siso_zeros(&a, &b, &c, 0.0);
    assert_eq!(zi.len(), 3, "{zi:?}");
    let (am, _, _) = minimal_siso(&a, &b, &c);
    assert_eq!(am.rows, 2);
    let (z, rd, hf) = transmission_zeros(&a, &b, &c, 0.0);
    assert_eq!(rd, Some(1));
    close(hf, 1.0, 1e-12);
    assert_eq!(z.len(), 1, "{z:?}");
    close(z[0].0, -2.0, 1e-10);
}

#[test]
fn step_and_impulse_first_and_second_order() {
    // 1/(s+1): y = 1 - exp(-t), impulse exp(-t)
    let (a, b, c, d) = (m(&[&[-1.0]]), m(&[&[1.0]]), m(&[&[1.0]]), m(&[&[0.0]]));
    let (t, st, im) = time_responses(&a, &b, &c, &d, 5.0, 101);
    for k in 0..t.len() {
        close(st[0][0][k], 1.0 - (-t[k]).exp(), 1e-12);
        close(im[0][0][k], (-t[k]).exp(), 1e-12);
    }
    let info = step_info(&t, &st[0][0], Some(1.0));
    close(info.rise_time.unwrap(), 9f64.ln(), 1e-3); // ln(0.9/0.1)
    close(info.settling_time.unwrap(), 50f64.ln(), 0.02); // within one sample of ln 50
    close(info.overshoot.unwrap(), 0.0, 1e-12);

    // underdamped: overshoot exp(-zeta pi / sqrt(1 - zeta^2)), peak at pi / wd
    let (wn, zeta) = (3.0, 0.3);
    let (a, b, c, d) = second_order(wn, zeta);
    let (t, st, _) = time_responses(&a, &b, &c, &d, 10.0, 20001);
    let info = step_info(&t, &st[0][0], Some(1.0));
    let os = (-zeta * std::f64::consts::PI / (1.0 - zeta * zeta).sqrt()).exp() * 100.0;
    close(info.overshoot.unwrap(), os, 1e-6);
    let wd = wn * (1.0 - zeta * zeta).sqrt();
    close(info.peak_time, std::f64::consts::PI / wd, 1e-3);
    // analytic response at a sample
    let k = 7000;
    let tt = t[k];
    let phi = zeta.acos();
    let y = 1.0 - (-zeta * wn * tt).exp() / (1.0 - zeta * zeta).sqrt() * (wd * tt + phi).sin();
    close(st[0][0][k], y, 1e-10);
}

#[test]
fn expm_rotation() {
    let th = 0.7f64;
    let e = expm(&m(&[&[0.0, -th], &[th, 0.0]]));
    close(e[(0, 0)], th.cos(), 1e-14);
    close(e[(0, 1)], -th.sin(), 1e-14);
    close(e[(1, 0)], th.sin(), 1e-14);
    let e = expm(&m(&[&[-50.0, 1.0], &[0.0, -2.0]]));
    close(e[(1, 1)], (-2.0f64).exp(), 1e-12);
    close(
        e[(0, 1)],
        ((-2.0f64).exp() - (-50.0f64).exp()) / 48.0,
        1e-12,
    );
}

#[test]
fn lyapunov_and_hankel_singular_values() {
    // 1/(s + a): P = Q = 1/(2a), HSV = 1/(2a)
    let a = m(&[&[-2.0]]);
    let p = lyap(&a, &m(&[&[1.0]])).unwrap();
    close(p[(0, 0)], 0.25, 1e-14);
    let r = balanced_truncation(&a, &m(&[&[1.0]]), &m(&[&[1.0]]), &m(&[&[0.0]]), 1).unwrap();
    close(r.hankel_singular_values[0], 0.25, 1e-12);
    // A X + X A^T + Q = 0 residual on a non-normal matrix
    let a = m(&[&[-1.0, 10.0, 0.0], &[0.0, -2.0, 5.0], &[0.0, 0.0, -0.5]]);
    let q = m(&[&[1.0, 0.0, 0.0], &[0.0, 2.0, 0.0], &[0.0, 0.0, 3.0]]);
    let x = lyap(&a, &q).unwrap();
    let res = a.mul(&x).add(&x.mul(&a.t())).add(&q);
    assert!(res.norm1() < 1e-9 * x.norm1(), "residual {}", res.norm1());
    // unstable A is rejected
    assert!(lyap(&m(&[&[1.0]]), &m(&[&[1.0]])).is_err());
}

#[test]
fn balanced_truncation_removes_weak_and_redundant_states() {
    // G = 1/(s+1) + 0.001/(s+100) with a third, uncontrollable state
    let a = m(&[&[-1.0, 0.0, 0.0], &[0.0, -100.0, 0.0], &[0.0, 0.0, -3.0]]);
    let b = m(&[&[1.0], &[1.0], &[0.0]]);
    let c = m(&[&[1.0, 0.001, 1.0]]);
    let d = m(&[&[0.0]]);
    let r = balanced_truncation(&a, &b, &c, &d, 1).unwrap();
    assert_eq!(r.order, 1);
    let hsv = &r.hankel_singular_values;
    close(hsv[0], 0.5, 1e-3);
    assert!(hsv[1] < 1e-5 && hsv[2] < 1e-12, "{hsv:?}");
    assert!(r.error_bound < 2e-5);
    close(r.poles[0].0, -1.0, 1e-3);
    // DC gain of the reduced model within the bound
    let ar = Mat::from_rows(&r.a, 1);
    let dc = -r.c[0][0] * r.b[0][0] / ar[(0, 0)];
    close(dc, 1.0 + 1e-5, r.error_bound + 1e-9);
    // full order reproduces the frequency response
    let full = balanced_truncation(&a, &b, &c, &d, 3).unwrap();
    assert_eq!(full.order, 2); // the uncontrollable state is dropped
    let (ar, br, cr, dr) = (
        Mat::from_rows(&full.a, 2),
        Mat::from_rows(&full.b, 1),
        Mat::from_rows(&full.c, 2),
        Mat::from_rows(&full.d, 1),
    );
    let e0 = FreqEvaluator::new(&a, &b, &c, &d);
    let e1 = FreqEvaluator::new(&ar, &br, &cr, &dr);
    for w in [0.1, 1.0, 10.0, 300.0] {
        let (r0, i0) = e0.eval(0.0, w);
        let (r1, i1) = e1.eval(0.0, w);
        close(r1[(0, 0)], r0[(0, 0)], 1e-9);
        close(i1[(0, 0)], i0[(0, 0)], 1e-9);
    }
}

#[test]
fn riccati_known_solutions() {
    // scalar: a = 1, b = 1, q = 1, r = 1: x = 1 + sqrt(2), k = x
    let (k, x) = lqr(&m(&[&[1.0]]), &m(&[&[1.0]]), &m(&[&[1.0]]), &m(&[&[1.0]])).unwrap();
    close(x[(0, 0)], 1.0 + 2f64.sqrt(), 1e-12);
    close(k[(0, 0)], 1.0 + 2f64.sqrt(), 1e-12);
    // double integrator, Q = I, R = 1: X = [sqrt3 1; 1 sqrt3], K = [1 sqrt3]
    let a = m(&[&[0.0, 1.0], &[0.0, 0.0]]);
    let b = m(&[&[0.0], &[1.0]]);
    let (k, x) = lqr(&a, &b, &Mat::eye(2), &m(&[&[1.0]])).unwrap();
    let s3 = 3f64.sqrt();
    close(x[(0, 0)], s3, 1e-12);
    close(x[(0, 1)], 1.0, 1e-12);
    close(x[(1, 1)], s3, 1e-12);
    close(k[(0, 0)], 1.0, 1e-12);
    close(k[(0, 1)], s3, 1e-12);
    // residual on a larger MIMO problem
    let a = m(&[
        &[-0.1, 1.0, 0.0, 0.0],
        &[-4.0, -0.2, 2.0, 0.0],
        &[0.0, 0.0, 0.3, 1.0],
        &[1.0, 0.0, -9.0, -0.1],
    ]);
    let b = m(&[&[0.0, 0.0], &[1.0, 0.0], &[0.0, 0.0], &[0.0, 1.0]]);
    let q = Mat::eye(4).scale(2.0);
    let r = Mat::eye(2).scale(0.5);
    let x = care(&a, &b, &q, &r).unwrap();
    let ri = r.inv().unwrap();
    let res = a
        .t()
        .mul(&x)
        .add(&x.mul(&a))
        .sub(&x.mul(&b).mul(&ri).mul(&b.t()).mul(&x))
        .add(&q);
    assert!(res.norm1() < 1e-9 * x.norm1(), "residual {}", res.norm1());
    let (k, _) = lqr(&a, &b, &q, &r).unwrap();
    let cl = poles(&a.sub(&b.mul(&k)));
    assert!(cl.iter().all(|p| p.0 < 0.0), "{cl:?}");
}

#[test]
fn svd_and_symmetric_eigen() {
    let a = m(&[&[3.0, 1.0], &[1.0, 3.0]]);
    let (l, v) = sym_eig(&a);
    close(l[0], 4.0, 1e-14);
    close(l[1], 2.0, 1e-14);
    close(v[(0, 0)].abs(), 0.5f64.sqrt(), 1e-14);
    let mm = m(&[&[2.0, 0.0], &[0.0, -5.0], &[0.0, 0.0]]);
    let (u, s, vv) = svd(&mm);
    close(s[0], 5.0, 1e-14);
    close(s[1], 2.0, 1e-14);
    let mut sd = Mat::zeros(2, 2);
    sd[(0, 0)] = s[0];
    sd[(1, 1)] = s[1];
    let back = u.mul(&sd).mul(&vv.t());
    assert!(back.sub(&mm).norm1() < 1e-13);
}

#[test]
fn pid_suggestion_from_lag_chain() {
    // 2/(s+1)^3: FOPDT fit with a dead time, positive gains
    let a = m(&[&[-1.0, 1.0, 0.0], &[0.0, -1.0, 1.0], &[0.0, 0.0, -1.0]]);
    let b = m(&[&[0.0], &[0.0], &[2.0]]);
    let c = m(&[&[1.0, 0.0, 0.0]]);
    let d = m(&[&[0.0]]);
    let (t, st, _) = time_responses(&a, &b, &c, &d, 20.0, 4001);
    let p = pid_suggestion(&t, &st[0][0], 2.0).unwrap();
    close(p.fopdt.k, 2.0, 1e-12);
    // Smith's method on (s+1)^-3: t28 = 1.42, t63 = 3.26 -> T = 2.11, L = 1.15
    close(p.fopdt.t, 2.110, 2e-3);
    close(p.fopdt.l, 1.147, 2e-3);
    // SIMC with tau_c = L: Kp = T / (2 K L), Ti = min(T, 8 L)
    close(p.simc.kp, p.fopdt.t / (2.0 * 2.0 * p.fopdt.l), 1e-12);
    close(p.simc.ti, p.fopdt.t.min(8.0 * p.fopdt.l), 1e-12);
    let zn = p.ziegler_nichols.unwrap();
    close(zn.kp, 1.2 * p.fopdt.t / (2.0 * p.fopdt.l), 1e-12);
}

#[test]
fn default_ranges() {
    let (lo, hi) = default_freq_range(&[(-1.0, 0.0), (-100.0, 0.0)]);
    assert_eq!((lo, hi), (0.1, 1000.0));
    close(default_horizon(&[(-1.0, 0.0), (-10.0, 0.0)]), 10.0, 0.0);
    close(default_horizon(&[(-0.25, 2.0)]), 50.0, 0.0);
}

fn lin(model: &str, extra: &[&str]) -> Linearization {
    let path = format!("{}/tests/models/linearize.mo", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    assert!(lib.set_document(&path, &text).is_empty());
    let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    let opts = SimOptions {
        tolerance: 1e-10,
        ..SimOptions::from_model(&m)
    };
    let extra: Vec<String> = extra.iter().map(|s| s.to_string()).collect();
    linearize_with_outputs(&m, opts, None, JacobianMethod::Exact, &extra).unwrap()
}

#[test]
fn mass_spring_damper_end_to_end() {
    // m = 2, c = 0.5, k = 8: G(s) = 0.5 / (s^2 + 0.25 s + 4), wn = 2, zeta = 1/16
    let l = lin("Lin.MassSpringDamper", &["v"]);
    assert_eq!(l.outputs, ["pos", "v"]);
    let r = analyze(
        &l,
        &LinearOptions {
            outputs: vec!["pos".into(), "v".into()],
            bode: true,
            step: true,
            reduce: Some(1),
            lqr: Some((1.0, 1.0)),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(r.stable);
    assert_eq!(r.poles.len(), 2);
    close(r.poles[0].0, -0.125, 1e-12);
    let pos = &r.pairs[0];
    assert_eq!((pos.input.as_str(), pos.output.as_str()), ("f", "pos"));
    close(pos.dc_gain.unwrap(), 1.0 / 8.0, 1e-12);
    assert_eq!(pos.relative_degree, Some(2));
    assert!(pos.zeros.is_empty());
    // velocity output: one zero at the origin, DC gain 0
    let vel = &r.pairs[1];
    assert_eq!(vel.relative_degree, Some(1));
    assert_eq!(vel.zeros.len(), 1);
    close(vel.zeros[0].0, 0.0, 1e-12);
    close(vel.dc_gain.unwrap(), 0.0, 1e-12);
    // resonance peak 1 / (2 zeta sqrt(1 - zeta^2)) * 0.5 / 4 = 0.017 dB (sampled on the grid)
    let bd = pos.bode.as_ref().unwrap();
    let peak = bd.mag_db.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    assert!((-0.05..=0.017).contains(&peak), "{peak}");
    let si = pos.step_info.as_ref().unwrap();
    let zeta: f64 = 1.0 / 16.0;
    let os = (-zeta * std::f64::consts::PI / (1.0 - zeta * zeta).sqrt()).exp() * 100.0;
    close(si.overshoot.unwrap(), os, 1e-2);
    assert!(pos.step.as_ref().unwrap().t.len() >= 600);
    // the reduced model's responses come along for comparison
    assert_eq!(pos.reduced_bode.as_ref().unwrap().omega.len(), 400);
    assert!(pos.reduced_step.is_some());
    let red = r.reduction.as_ref().unwrap();
    assert_eq!(red.order, 1);
    assert_eq!(red.hankel_singular_values.len(), 2);
    let lq = r.lqr.as_ref().unwrap();
    assert!(lq.closed_loop_poles.iter().all(|p| p.0 < -0.125));
    let txt = r.report();
    assert!(txt.contains("f -> pos") && txt.contains("LQR"), "{txt}");
    assert!(r.bode_csv().lines().count() == 401);
    assert!(r.step_csv().starts_with("time,"));
    // unknown names are reported
    assert!(
        analyze(
            &l,
            &LinearOptions {
                inputs: vec!["nope".into()],
                ..Default::default()
            }
        )
        .is_err()
    );
}

#[test]
fn badly_scaled_hydraulic_chain() {
    // AileronServo linearized: valve (2 states), cylinder pressures (1e10 scale), rod, inertia
    let a = m(&[
        &[0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
        &[-3.948e5, -8.796e2, 0.0, 0.0, 0.0, 0.0],
        &[0.0, 0.0, 0.0, 0.0, 0.0, 8.0e-2],
        &[3.518e10, 0.0, -3.211e6, -3.459, 2.307e-4, -2.334e9],
        &[-3.518e10, 0.0, 1.688e6, 2.307e-4, -3.459, 2.334e9],
        &[0.0, 0.0, -4.167e4, 5.027e-5, -5.027e-5, -8.952e2],
    ]);
    let b = [0.0, 3.948e5, 0.0, 0.0, 0.0, 0.0];
    let c = [0.0, 0.0, 7.162e2, 0.0, 0.0, 0.0];
    let (am, _, _) = minimal_siso(&a, &b, &c);
    let (z, rd, _) = transmission_zeros(&a, &b, &c, 0.0);
    assert_eq!(rd, Some(5), "minimal order {} zeros {z:?}", am.rows);
}
