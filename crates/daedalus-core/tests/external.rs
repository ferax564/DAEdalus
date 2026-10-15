//! The external C functions of the Modelica Standard Library that DAEdalus implements
//! natively (ModelicaRandom, ModelicaFFT, ModelicaInternal, ModelicaStrings), checked
//! against values computed with the original C sources.

use daedalus_core::frontend::Library;
use daedalus_core::results::Results;
use daedalus_core::sim::{SimOptions, simulate};

fn run(model: &str, stop: f64) -> Results {
    let path = format!("{}/tests/models/external.mo", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    let opts = SimOptions {
        stop_time: stop,
        interval: 0.05,
        ..SimOptions::from_model(&m)
    };
    simulate(&m, opts).map_err(|e| e.message).unwrap().0
}

fn at(r: &Results, name: &str, t: f64) -> f64 {
    r.at(name, t)
        .unwrap_or_else(|| panic!("no variable {name}"))
}

#[test]
fn xorshift_generators_match_c() {
    let r = run("Ext.Generators", 0.52);
    // Folded at translation time.
    assert_eq!(at(&r, "y64", 0.0), 0.25753429030116642);
    assert_eq!(at(&r, "y128", 0.0), 0.50390628213108712);
    assert_eq!(at(&r, "y1024", 0.0), 0.59117605911612614);
    // Evaluated at events: the sequence of ModelicaRandom_xorshift64star from {1, 2}.
    let seq = [
        (1898718301.0, -1041382294.0, 0.25753429030116642),
        (179960774.0, -231500805.0, 0.44609951857521674),
        (1309405701.0, -668831868.0, 0.34427544574832303),
        (-1452074118.0, -1893606856.0, 0.059110297044253135),
        (881447704.0, 803509121.0, 0.68708154587196835),
    ];
    for (k, &(s1, s2, y)) in seq.iter().enumerate() {
        let t = 0.1 * (k + 1) as f64 + 0.01;
        assert_eq!(at(&r, "n", t), (k + 1) as f64);
        assert_eq!(at(&r, "s[1]", t), s1, "s[1] after {} draws", k + 1);
        assert_eq!(at(&r, "s[2]", t), s2, "s[2] after {} draws", k + 1);
        assert_eq!(at(&r, "y", t), y, "y after {} draws", k + 1);
    }
}

#[test]
fn impure_random_draws_in_order() {
    let r = run("Ext.Impure", 0.25);
    // ModelicaRandom_impureRandom_xorshift1024star after setInternalState: two calls per
    // event, in the order of the algorithm.
    assert_eq!(at(&r, "r1", 0.05), 0.0);
    assert_eq!(at(&r, "r1", 0.15), 0.59117605911612614);
    assert_eq!(at(&r, "r2", 0.15), 0.042044935035719877);
    assert_eq!(at(&r, "r1", 0.25), 0.051573980671262842);
    assert_eq!(at(&r, "r2", 0.25), 0.11950806770465205);
    // print() reaches the simulation log once per event.
    assert_eq!(
        r.log.iter().filter(|l| l.contains("drawn")).count(),
        2,
        "{:?}",
        r.log
    );
    // when terminal() runs once, at the end.
    assert_eq!(
        r.log.iter().filter(|l| l.contains("done")).count(),
        1,
        "{:?}",
        r.log
    );
}

#[test]
fn impure_random_has_one_global_state() {
    let path = format!("{}/tests/models/external.mo", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    assert!(lib.set_document(&path, &text).is_empty());
    let e = daedalus_core::translate(&lib, "Ext.TwoSeeds", &Default::default())
        .err()
        .expect("two different seeds must be rejected");
    assert!(format!("{e:#}").contains("single global"), "{e:#}");
}

#[test]
fn real_fft_amplitudes_and_phases() {
    let r = run("Ext.FFT", 1.0);
    let tol = 1e-12;
    // amplitudes = |X_k| / nf with nf = 5: |X| = 8, 8, 2, 0, 0 at time = 1.
    for (k, a) in [1.6, 1.6, 0.4, 0.0, 0.0].iter().enumerate() {
        let v = at(&r, &format!("amp[{}]", k + 1), 1.0);
        assert!((v - a).abs() < tol, "amp[{}] = {v}, expected {a}", k + 1);
        let h = at(&r, &format!("amp[{}]", k + 1), 0.5);
        assert!((h - a / 2.0).abs() < tol, "amp[{}] at 0.5 = {h}", k + 1);
    }
    let p3 = at(&r, "phi[3]", 1.0);
    assert!(
        (p3 + std::f64::consts::FRAC_PI_2).abs() < 1e-12,
        "phi[3] = {p3}"
    );
    assert!(at(&r, "phi[2]", 1.0).abs() < 1e-12);
    assert_eq!(at(&r, "info", 1.0), 0.0);
    // Folded at translation time.
    assert!((at(&r, "a2", 0.0) - 1.6).abs() < tol);
}

#[test]
fn string_scanning() {
    let r = run("Ext.Scan", 1.0);
    assert_eq!(at(&r, "x", 0.0), 2000.0);
    // nextIndex after " 42" (the first output).
    assert_eq!(at(&r, "k", 0.0), 4.0);
}
