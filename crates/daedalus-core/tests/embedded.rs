//! Embedded C generation: the generated code, compiled for the host with the system C
//! compiler, reproduces the simulator's results (skipped when no `cc` is installed).

use daedalus_core::embedded::{self, Integrator, Options, RealType};
use daedalus_core::frontend::Library;
use daedalus_core::sim::{SimOptions, simulate};
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn lib() -> Library {
    let dir = env!("CARGO_MANIFEST_DIR");
    let path = format!("{dir}/tests/models/embedded.mo");
    let mut lib = Library::new();
    lib.load_dir(&PathBuf::from(format!("{dir}/../../library/Daedalus")))
        .unwrap();
    let errs = lib.set_document(&path, &std::fs::read_to_string(&path).unwrap());
    assert!(errs.is_empty(), "{errs:?}");
    lib
}

fn have_cc() -> bool {
    Command::new("cc")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// Generate, compile and run `n` steps with constant inputs (their start values);
/// returns rows `[time, outputs...]` and the output names.
fn run_c(name: &str, opts: &Options, n: usize) -> (Vec<String>, Vec<Vec<f64>>) {
    let (names, rows, _) = run_c_err(name, opts, n);
    (names, rows)
}

fn run_c_err(name: &str, opts: &Options, n: usize) -> (Vec<String>, Vec<Vec<f64>>, String) {
    let lib = lib();
    let m = daedalus_core::translate(&lib, name, &Default::default()).unwrap();
    let g = embedded::generate(&m, opts).unwrap();
    let dir = std::env::temp_dir().join(format!(
        "daedalus_emb_{}_{}_{}",
        g.manifest.prefix,
        opts.real.c_name(),
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(g.header_name()), &g.header).unwrap();
    std::fs::write(dir.join(g.source_name()), &g.source).unwrap();
    std::fs::write(dir.join("main.c"), embedded::host_main(&g.manifest)).unwrap();
    let exe = dir.join("prog");
    let out = Command::new("cc")
        .args([
            "-std=c99",
            "-O1",
            "-Wall",
            "-Wextra",
            "-Werror",
            "-pedantic",
            "-Wshadow",
            "-Wconversion",
            "-Wno-sign-conversion",
            "-o",
        ])
        .arg(&exe)
        .arg(dir.join("main.c"))
        .arg(dir.join(g.source_name()))
        .arg("-lm")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "cc failed:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let line: Vec<String> = g
        .manifest
        .inputs
        .iter()
        .map(|p| p.start.to_string())
        .collect();
    let input = format!("{}\n", line.join(",")).repeat(n + 1);
    let mut child = Command::new(&exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    use std::io::Write;
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    assert!(out.status.success(), "generated program failed");
    let text = String::from_utf8(out.stdout).unwrap();
    let mut lines = text.lines();
    let names: Vec<String> = lines
        .next()
        .unwrap()
        .split(',')
        .skip(1)
        .map(String::from)
        .collect();
    let rows = lines
        .map(|l| l.split(',').map(|x| x.parse::<f64>().unwrap()).collect())
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    (names, rows, String::from_utf8_lossy(&out.stderr).into_owned())
}

fn reference(name: &str, stop: f64, n: usize) -> daedalus_core::results::Results {
    let lib = lib();
    let m = daedalus_core::translate(&lib, name, &Default::default()).unwrap();
    let so = SimOptions {
        stop_time: stop,
        intervals: n,
        tolerance: 1e-9,
        ..SimOptions::default()
    };
    simulate(&m, so).unwrap().0
}

fn opts(step: f64, real: RealType, integ: Integrator) -> Options {
    Options {
        prefix: None,
        step,
        real,
        integrator: integ,
        params: Vec::new(),
    }
}

fn max_err(
    names: &[String],
    rows: &[Vec<f64>],
    r: &daedalus_core::results::Results,
    skip: &[&str],
) -> f64 {
    let mut worst = 0.0f64;
    for row in rows {
        for (k, nme) in names.iter().enumerate() {
            if skip.contains(&nme.as_str()) {
                continue;
            }
            // At an event the reference has rows before and after it: the generated code
            // reports the value after.
            let t = row[0];
            let ys = r.values(nme).unwrap();
            let mut e = (row[k + 1] - r.at(nme, t).unwrap()).abs();
            for (i, &ti) in r.time.iter().enumerate() {
                if (ti - t).abs() <= 1e-12 {
                    e = e.min((row[k + 1] - ys[i]).abs());
                }
            }
            worst = worst.max(e);
        }
    }
    worst
}

#[test]
fn algebraic_loop_matches() {
    if !have_cc() {
        return;
    }
    let (names, rows) = run_c(
        "Emb.Loop",
        &opts(0.01, RealType::Double, Integrator::Rk4),
        10,
    );
    let r = reference("Emb.Loop", 0.1, 10);
    assert!(max_err(&names, &rows, &r, &[]) < 1e-8);
}

#[test]
fn linear_lag_rk4_and_euler() {
    if !have_cc() {
        return;
    }
    let r = reference("Emb.Lag", 1.0, 100);
    let (names, rows) = run_c(
        "Emb.Lag",
        &opts(0.01, RealType::Double, Integrator::Rk4),
        100,
    );
    let e_rk4 = max_err(&names, &rows, &r, &[]);
    assert!(e_rk4 < 1e-6, "rk4 error {e_rk4}");
    let (names, rows) = run_c(
        "Emb.Lag",
        &opts(0.01, RealType::Double, Integrator::Euler),
        100,
    );
    let e_euler = max_err(&names, &rows, &r, &[]);
    assert!(
        e_euler < 2e-2 && e_euler > 10.0 * e_rk4,
        "euler error {e_euler}"
    );
    let (names, rows) = run_c(
        "Emb.Lag",
        &opts(0.01, RealType::Float, Integrator::Rk4),
        100,
    );
    assert!(max_err(&names, &rows, &r, &[]) < 1e-5);
}

#[test]
fn state_events_with_reinit() {
    if !have_cc() {
        return;
    }
    // Bounces are found at step ends: the error is of the order of the step.
    let (names, rows) = run_c(
        "Emb.Ball",
        &opts(1e-4, RealType::Double, Integrator::Rk4),
        15000,
    );
    let r = reference("Emb.Ball", 1.5, 15000);
    let e = max_err(&names, &rows, &r, &[]);
    assert!(e < 5e-3, "error {e}");
    // It bounced: h went negative at most by one step's travel and came back up.
    let last = rows.last().unwrap()[1];
    assert!(last > -1e-2);
}

#[test]
fn tables_integers_booleans_and_samples() {
    if !have_cc() {
        return;
    }
    let (names, rows) = run_c(
        "Emb.Mixed",
        &opts(0.01, RealType::Double, Integrator::Rk4),
        200,
    );
    let r = reference("Emb.Mixed", 2.0, 200);
    let e = max_err(&names, &rows, &r, &[]);
    assert!(e < 1e-9, "error {e}");
    let ny = names.iter().position(|n| n == "ny").unwrap() + 1;
    assert_eq!(rows.last().unwrap()[ny], 21.0);
}

#[test]
fn parameter_overrides_are_baked_in() {
    let lib = lib();
    let m = daedalus_core::translate(&lib, "Emb.Lag", &Default::default()).unwrap();
    let mut o = opts(0.01, RealType::Float, Integrator::Rk4);
    o.params.push(("T".into(), 0.5));
    let g = embedded::generate(&m, &o).unwrap();
    assert!(g.source.contains("RC(5e-1)"), "override not in the code");
    o.params = vec![("nope".into(), 1.0)];
    assert!(embedded::generate(&m, &o).is_err());
}

#[test]
fn assertions_set_the_status_bit() {
    if !have_cc() {
        return;
    }
    // y ramps up and passes the asserted limit at t = 0.5
    let (_, rows, err) = run_c_err(
        "Emb.Guarded",
        &opts(0.01, RealType::Float, Integrator::Rk4),
        100,
    );
    assert!(!rows.is_empty());
    assert!(err.contains("assertion failed at t = 0.5"), "{err}");
}

#[test]
fn step_counter_width_and_float_horizon() {
    let lib = lib();
    let m = daedalus_core::translate(&lib, "Emb.Lag", &Default::default()).unwrap();
    let f = embedded::generate(&m, &opts(0.001, RealType::Float, Integrator::Rk4)).unwrap();
    assert!((f.manifest.horizon_s - 16777.216).abs() < 1e-6);
    assert!(f.header.contains("uint32_t k;"));
    let d = embedded::generate(&m, &opts(0.001, RealType::Double, Integrator::Rk4)).unwrap();
    assert!(d.header.contains("uint64_t k;"));
}
