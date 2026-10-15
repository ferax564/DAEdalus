//! MSL table objects (`ModelicaStandardTables_*`, implemented natively): 2-D tables
//! and tables read from files. The models in `models/tables.mo` call the same
//! external functions as the MSL blocks, without depending on the MSL.

// reference values are copied digit for digit from the reference CSV files
#![allow(clippy::excessive_precision)]

use daedalus_core::frontend::Library;
use daedalus_core::results::Results;
use daedalus_core::sim::{Method, SimOptions, simulate};

fn run(model: &str, stop: f64, interval: f64) -> Results {
    let path = format!("{}/tests/models/tables.mo", env!("CARGO_MANIFEST_DIR"));
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&path, &text);
    assert!(errs.is_empty(), "{errs:?}");
    let m = daedalus_core::translate(&lib, model, &Default::default()).unwrap();
    let opts = SimOptions {
        stop_time: stop,
        method: Method::Bdf,
        tolerance: 1e-8,
        interval,
        ..SimOptions::from_model(&m)
    };
    simulate(&m, opts).map_err(|e| e.message).unwrap().0
}

fn check(r: &Results, name: &str, t: f64, want: f64, tol: f64) {
    let got = r
        .at(name, t)
        .unwrap_or_else(|| panic!("no variable {name}"));
    assert!(
        (got - want).abs() <= tol * (1.0 + want.abs()),
        "{name}({t}) = {got}, expected {want}"
    );
}

#[test]
fn bilinear_2d_table_with_extrapolation() {
    // z = 1 + 4 (u1-1) + 2 (u2-1) + (u1-1)(u2-1) on [1,2]^2, continued linearly in each
    // direction beyond it; with u1 = 1 + t, u2 = 1 + 2 t: y = 1 + 8 t + 2 t^2.
    let r = run("TablesTest.Linear2D", 2.0, 0.01);
    for t in [0.0, 0.3, 0.5, 0.8, 1.0, 1.7, 2.0] {
        check(&r, "t.y", t, 1.0 + 8.0 * t + 2.0 * t * t, 1e-9);
        check(&r, "dy", t, 8.0 + 4.0 * t, 1e-9);
    }
    check(&r, "t.u_min[1]", 0.0, 1.0, 0.0);
    check(&r, "t.u_max[2]", 0.0, 2.0, 0.0);
}

#[test]
fn akima_2d_table_matches_msl_reference() {
    // Values of the reference results of ModelicaTest.Tables.CombiTable2Ds.Test1 (u1 =
    // time, u2 = 2) and Test2 (u1 = 2, u2 = time; extrapolated beyond u2 = 40), the
    // second derivatives included.
    let r = run("TablesTest.Akima2D", 60.0, 0.5);
    let test1 = [
        (
            7.5,
            30.163040606070712,
            -3.6085905473716977,
            0.47387403836449488,
        ),
        (
            12.0,
            22.166283891335347,
            0.17583626305713662,
            0.37164491197551919,
        ),
        (
            33.0,
            3.6684680691159874,
            -0.94794203044514103,
            0.045485957216300915,
        ),
    ];
    for (t, y, d, dd) in test1 {
        check(&r, "t1.y", t, y, 1e-9);
        check(&r, "d1", t, d, 1e-9);
        check(&r, "dd1", t, dd, 1e-9);
    }
    let test2 = [
        (
            7.5,
            45.876745836021684,
            -3.7526237796098285,
            -0.10486436225106335,
        ),
        (
            12.0,
            43.389585484896465,
            4.7071335647317012,
            1.0392402355334924,
        ),
        (45.0, 32.471072164948453, -0.13731546391752553, 0.0),
        (58.5, 30.617313402061857, -0.13731546391752553, 0.0),
    ];
    for (t, y, d, dd) in test2 {
        check(&r, "t2.y", t, y, 1e-9);
        check(&r, "d2", t, d, 1e-9);
        check(&r, "dd2", t, dd, 1e-9);
    }
}

#[test]
fn tables_read_from_text_and_csv_files() {
    // tab2 (text file) and tables2d.csv: z = 1 + 2 u1 + u2 + u1 u2 on [0,1]^2, with
    // u1 = t, u2 = 2 t; tab1 (text file, rows spanning lines) columns 2 and 3.
    let r = run("TablesTest.FileTables", 1.0, 0.05);
    for t in [0.0, 0.25, 0.5, 1.0] {
        let z = 1.0 + 4.0 * t + 2.0 * t * t;
        check(&r, "t2.y", t, z, 1e-9);
        check(&r, "tcsv.y", t, z, 1e-9);
    }
    check(&r, "y1a", 0.25, 0.5, 1e-9);
    check(&r, "y1a", 0.75, 2.5, 1e-9);
    check(&r, "y1b", 0.25, 15.0, 1e-9);
    check(&r, "y1b", 0.75, 30.0, 1e-9);
}

#[test]
fn monotone_1d_splines_with_second_derivatives() {
    // Reference values of ModelicaTest.Tables.CombiTable1Ds.Test26 and Test30 (the same
    // on these points).
    let r = run("TablesTest.Monotone1D", 4.0, 0.0008);
    for (a, b, c) in [
        ("fb.y", "dfb", "d2fb"),
        ("steffen.y", "dsteffen", "d2steffen"),
    ] {
        check(&r, a, 0.7, 3.6, 1e-9);
        check(&r, b, 0.7, -2.0, 1e-9);
        check(&r, c, 0.7, 0.0, 1e-9);
        check(&r, a, 1.296, 2.284636672, 1e-9);
        check(&r, b, 1.296, -2.658304, 1e-9);
        check(&r, c, 1.296, -0.448, 1e-9);
        check(&r, a, 3.5, 1.0, 1e-9);
        check(&r, b, 3.5, 0.0, 1e-9);
    }
}

#[test]
fn periodic_time_table_with_jumps() {
    // Reference values of ModelicaTest.Tables.CombiTimeTable.Test1.
    let r = run("TablesTest.PeriodicTime", 3.0, 0.005);
    check(&r, "t.y", 0.505, -1.5, 1e-9);
    check(&r, "dy", 0.505, -100.0, 1e-9);
    check(&r, "t.y", 1.405, -1.5, 1e-9);
    check(&r, "dy", 1.405, -100.0, 1e-9);
    check(&r, "t.y", 1.95, 0.0, 1e-9);
    check(&r, "t.y", 2.5, -2.0, 1e-9);
    check(&r, "dy", 2.5, -20.0 / 3.0, 1e-9);
}
