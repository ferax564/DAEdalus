//! `daedalus run script.mos` and `daedalus steady` end to end: the sample scripts in
//! `tests/mos` run in a scratch directory, as a user would run them.

use daedalus_core::results::read_mat;
use std::path::{Path, PathBuf};
use std::process::Command;

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/mos")
}

/// A fresh directory holding copies of the sample scripts and models.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("daedalus-mos-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    for e in std::fs::read_dir(src_dir()).unwrap() {
        let p = e.unwrap().path();
        if p.is_file() {
            std::fs::copy(&p, dir.join(p.file_name().unwrap())).unwrap();
        }
    }
    dir
}

struct Out {
    code: i32,
    stdout: String,
    stderr: String,
}

fn daedalus(dir: &Path, args: &[&str], env: &[(&str, &Path)]) -> Out {
    let mut c = Command::new(env!("CARGO_BIN_EXE_daedalus"));
    c.args(args).current_dir(dir);
    c.env_remove("DAEDALUS_MSL")
        .env_remove("MODELICAPATH")
        .env_remove("DAEDALUS_LIB");
    // no user library folder
    c.env("HOME", dir);
    for (k, v) in env {
        c.env(k, v);
    }
    let o = c.output().unwrap();
    Out {
        code: o.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&o.stdout).into(),
        stderr: String::from_utf8_lossy(&o.stderr).into(),
    }
}

fn final_value(path: &Path, var: &str) -> f64 {
    let r = read_mat(&std::fs::read(path).unwrap()).unwrap();
    r.final_value(var)
        .unwrap_or_else(|| panic!("{var} not in {}", path.display()))
}

#[track_caller]
fn close(a: f64, b: f64, tol: f64) {
    assert!((a - b).abs() <= tol * (1.0 + b.abs()), "{a} vs {b}");
}

#[test]
fn dymola_script() {
    let dir = scratch("dymola");
    let o = daedalus(&dir, &["run", "dymola.mos"], &[]);
    assert_eq!(o.code, 0, "{}\n{}", o.stdout, o.stderr);
    // tool settings and plots are accepted with a warning
    assert!(
        o.stderr
            .contains("Advanced.Define.DAEsolver is a tool setting: ignored")
    );
    assert!(o.stderr.contains("plot: plotting is not supported"));
    assert!(o.stdout.contains("Check of Plant.FirstOrder successful"));
    assert!(o.stdout.lines().any(|l| l == "= true"), "{}", o.stdout);
    assert!(!o.stdout.contains("simulation failed"));
    // simulateModel with a modifier, named arguments and resultFile
    let r = read_mat(&std::fs::read(dir.join("FirstOrder_k4.mat")).unwrap()).unwrap();
    assert_eq!(r.time.len(), 101, "numberOfIntervals = 100");
    close(*r.time.last().unwrap(), 5.0, 1e-12);
    // x(5) = k u (1 - exp(-5 / T)) with k = 4, u = 2
    close(
        final_value(&dir.join("FirstOrder_k4.mat"), "y"),
        8.0 * (1.0 - (-5.0f64 / 0.7).exp()),
        1e-6,
    );
    // positional arguments, result in a subdirectory
    let p = dir.join("out/FirstOrder.mat");
    let r = read_mat(&std::fs::read(&p).unwrap()).unwrap();
    close(*r.time.last().unwrap(), 2.0, 1e-12);
    // simulateExtendedModel in a for loop with a tuple assignment
    assert!(o.stdout.contains("T = 0.5: y(20) = 5"), "{}", o.stdout);
    assert!(o.stdout.contains("T = 1: y(20) = 4.99"), "{}", o.stdout);
    // linearizeModel: dslin.mat layout and JSON
    let bytes = std::fs::read(dir.join("pendulum_lin.mat")).unwrap();
    assert!(bytes.windows(4).any(|w| w == b"ABCD"));
    assert!(bytes.windows(7).any(|w| w == b"xuyName"));
    let json: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("pendulum_lin.json")).unwrap())
            .unwrap();
    assert_eq!(json["states"].as_array().unwrap().len(), 2);
    // steadyState and trim with record field access
    assert!(o.stdout.contains("= 8.0\n"), "{}", o.stdout);
    let tau = 9.81 * 0.8 * 0.5;
    let line = o.stdout.lines().last().unwrap();
    close(line.trim_start_matches("= ").parse().unwrap(), tau, 1e-9);
}

#[test]
fn openmodelica_script() {
    let dir = scratch("om");
    let o = daedalus(&dir, &["run", "openmodelica.mos"], &[]);
    // the unknown model makes the script fail at the end
    assert_eq!(o.code, 1, "{}\n{}", o.stdout, o.stderr);
    assert!(o.stdout.starts_with("= true\n= \"\"\n"), "{}", o.stdout);
    assert!(o.stdout.contains("record SimulationResult"));
    assert!(o.stdout.contains("fo_res.mat\""));
    let y4 = 5.0 * (1.0 - (-4.0f64 / 0.7).exp());
    let vals: Vec<f64> = o
        .stdout
        .lines()
        .filter_map(|l| l.strip_prefix("= ")?.parse().ok())
        .collect();
    assert_eq!(vals.len(), 2, "{}", o.stdout);
    for v in vals {
        close(v, y4, 1e-6);
    }
    close(final_value(&dir.join("fo_res.mat"), "y"), y4, 1e-6);
    let csv = std::fs::read_to_string(dir.join("Plant.FirstOrder_res.csv")).unwrap();
    assert!(
        csv.lines().next().unwrap().contains("\"y\"") || csv.lines().next().unwrap().contains('y')
    );
    let lin = std::fs::read_to_string(dir.join("linearized_model.mo")).unwrap();
    assert!(lin.starts_with("model linearized_model"));
    assert!(o.stderr.contains("simulate: simflags is ignored"));
    assert!(o.stdout.contains("model 'Plant.NoSuchModel' not found"));
    // getErrorString() returns and clears the messages
    assert!(
        o.stdout
            .contains("= \"Warning: simulate: simflags is ignored\nError:")
    );
}

#[test]
fn load_model_from_msl_env_and_flag() {
    let dir = scratch("msl");
    let msl = src_dir().join("msl");
    let check = |o: &Out| {
        assert_eq!(o.code, 0, "{}\n{}", o.stdout, o.stderr);
        assert!(
            o.stdout.starts_with("= true\n= true\n= true\n= false\n"),
            "{}",
            o.stdout
        );
        assert!(!o.stdout.contains("not reached"));
        assert!(
            o.stderr
                .contains("loadModel(Modelica, {\"3.2.3\"}): library not found")
        );
        let r = read_mat(&std::fs::read(dir.join("lag.mat")).unwrap()).unwrap();
        close(*r.time.last().unwrap(), 1.0, 1e-12);
        // step at 0.5 into a lag with T = 0.2
        close(r.final_value("y").unwrap(), 1.0 - (-2.5f64).exp(), 1e-4);
    };
    check(&daedalus(
        &dir,
        &["run", "msl.mos"],
        &[("DAEDALUS_MSL", &msl)],
    ));
    std::fs::remove_file(dir.join("lag.mat")).unwrap();
    check(&daedalus(
        &dir,
        &["run", "msl.mos", "--msl", msl.to_str().unwrap()],
        &[],
    ));
    std::fs::remove_file(dir.join("lag.mat")).unwrap();
    check(&daedalus(
        &dir,
        &["run", "msl.mos"],
        &[("MODELICAPATH", &msl)],
    ));
    // not found anywhere
    let o = daedalus(&dir, &["run", "msl.mos"], &[]);
    assert!(o.stdout.starts_with("= false\n"), "{}", o.stdout);
    assert!(o.stderr.contains("library not found"));
}

#[test]
fn script_errors() {
    let dir = scratch("errors");
    std::fs::write(dir.join("bad.mos"), "x := 1;\ny := x +;\n").unwrap();
    let o = daedalus(&dir, &["run", "bad.mos"], &[]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("line 2"), "{}", o.stderr);
    std::fs::write(
        dir.join("unknown.mos"),
        "frobnicate(1);\nprint(\"after\");\n",
    )
    .unwrap();
    let o = daedalus(&dir, &["run", "unknown.mos"], &[]);
    assert_eq!(o.code, 1);
    assert!(o.stderr.contains("unknown function 'frobnicate'"));
    assert!(o.stdout.contains("after"));
    std::fs::write(
        dir.join("runtime.mos"),
        "x := undefinedVar + 1;\nprint(\"after\");\n",
    )
    .unwrap();
    let o = daedalus(&dir, &["run", "runtime.mos"], &[]);
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("line 1: unknown variable 'undefinedVar'"),
        "{}",
        o.stderr
    );
    assert!(!o.stdout.contains("after"));
}

#[test]
fn steady_command() {
    let dir = scratch("steady");
    let o = daedalus(
        &dir,
        &["steady", "Plant.mo", "-m", "Plant.FirstOrder", "-p", "k=4"],
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(
        o.stdout.contains("steady state of Plant.FirstOrder"),
        "{}",
        o.stdout
    );
    assert!(o.stdout.contains("x = 8"), "{}", o.stdout);
    assert!(o.stdout.contains("(stable)"), "{}", o.stdout);
    // trim, JSON
    let o = daedalus(
        &dir,
        &[
            "steady",
            "Plant.mo",
            "-m",
            "Plant.Pendulum",
            "--free",
            "tau",
            "--target",
            "xPos=0.4",
            "--format",
            "json",
        ],
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    let v: serde_json::Value = serde_json::from_str(&o.stdout).unwrap();
    close(
        v["free"][0]["value"].as_f64().unwrap(),
        9.81 * 0.8 * 0.5,
        1e-9,
    );
    close(v["targets"][0]["value"].as_f64().unwrap(), 0.4, 1e-9);
    // after a simulation
    let o = daedalus(
        &dir,
        &[
            "steady",
            "Plant.mo",
            "-m",
            "Plant.Pendulum",
            "--at-time",
            "2",
        ],
        &[],
    );
    assert_eq!(o.code, 0, "{}", o.stderr);
    assert!(o.stdout.contains("at t = 2"), "{}", o.stdout);
    // a non-square trim is refused
    let o = daedalus(
        &dir,
        &[
            "steady",
            "Plant.mo",
            "-m",
            "Plant.Pendulum",
            "--free",
            "tau,d",
            "--target",
            "xPos=0.4",
        ],
        &[],
    );
    assert_eq!(o.code, 1);
    assert!(
        o.stderr.contains("as many targets as free variables"),
        "{}",
        o.stderr
    );
}
