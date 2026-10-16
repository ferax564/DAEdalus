//! Compiled tapes (`jit.rs`) give bit-identical results to the interpreter.
#![cfg(all(feature = "jit", not(target_arch = "wasm32")))]

use daedalus_core::frontend::Library;
use daedalus_core::sim::{SimOptions, simulate};

fn csv(file: &str, model: &str, jit: bool) -> Vec<u8> {
    let path = format!("{}/tests/models/{file}", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    assert!(lib.set_document(&path, &text).is_empty());
    let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    daedalus_core::jit::set_enabled(jit);
    let (r, _) = simulate(&m, SimOptions::from_model(&m))
        .map_err(|e| e.message)
        .unwrap();
    assert_eq!(daedalus_core::jit::compiled_tapes(&m) > 0, jit, "{model}");
    let mut out = Vec::new();
    r.write_csv(&mut out, None).unwrap();
    out
}

// One test: the switch is global.
#[test]
fn compiled_tapes_match_the_interpreter() {
    for (file, model) in [
        ("events.mo", "E.Ball"),
        ("events.mo", "E.Sampled"),
        ("events.mo", "E.Hyst"),
        ("stiff.mo", "S.VanDerPol"),
        ("stiff.mo", "S.Loop"),
        ("tables.mo", "TablesTest.Akima2D"),
        ("tables.mo", "TablesTest.PeriodicTime"),
        ("index.mo", "T.Pendulum"),
        ("functions_control.mo", "FnTest"),
    ] {
        let a = csv(file, model, false);
        let b = csv(file, model, true);
        assert!(a == b, "{model}: compiled tapes changed the results");
    }
    daedalus_core::jit::set_enabled(false);
}
