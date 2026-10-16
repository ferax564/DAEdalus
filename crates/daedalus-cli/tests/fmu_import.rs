//! FMU and SSP import end to end: FMUs exported by `daedalus fmu` (FMI 2.0 and 3.0) are
//! imported back (`simulate X.fmu`, `import-fmu`, `export-ssp`, `simulate X.ssp`,
//! `import-ssp`) and their trajectories compared with the native simulation.
//!
//! The FMUs embed the runtime `libdaedalus_fmi.so`: build it first with
//! `cargo build --release -p daedalus-fmi` (or set `DAEDALUS_FMI_LIB`); without it these
//! tests are skipped with a message.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn models() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/models/FmuImport.mo")
}

/// The FMI runtime the FMUs embed: `$DAEDALUS_FMI_LIB` or next to the `daedalus` binary.
fn runtime() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("DAEDALUS_FMI_LIB") {
        return Some(PathBuf::from(p)).filter(|p| p.is_file());
    }
    let file = if cfg!(target_os = "windows") {
        "daedalus_fmi.dll"
    } else if cfg!(target_os = "macos") {
        "libdaedalus_fmi.dylib"
    } else {
        "libdaedalus_fmi.so"
    };
    let dir = Path::new(env!("CARGO_BIN_EXE_daedalus"))
        .parent()?
        .to_path_buf();
    [dir.join(file), dir.join("deps").join(file)]
        .into_iter()
        .find(|p| p.is_file())
}

macro_rules! need_runtime {
    () => {
        match runtime() {
            Some(r) => r,
            None => {
                eprintln!(
                    "skipped: the FMI runtime is not built (cargo build --release -p daedalus-fmi)"
                );
                return;
            }
        }
    };
}

struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("daedalus-fmu-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Scratch(dir)
    }
    fn path(&self, f: &str) -> PathBuf {
        self.0.join(f)
    }
    fn write(&self, f: &str, text: &str) {
        std::fs::write(self.path(f), text).unwrap();
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Run `daedalus args...` in `dir`; panics with its output on failure.
#[track_caller]
fn daedalus(dir: &Scratch, rt: &Path, args: &[&str]) -> String {
    let o = Command::new(env!("CARGO_BIN_EXE_daedalus"))
        .args(args)
        .current_dir(&dir.0)
        .env("DAEDALUS_FMI_LIB", rt)
        .env("HOME", &dir.0)
        .env_remove("DAEDALUS_MSL")
        .env_remove("MODELICAPATH")
        .env_remove("DAEDALUS_LIB")
        .output()
        .unwrap();
    let out = format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(o.status.success(), "daedalus {args:?} failed:\n{out}");
    out
}

/// A CSV result: column name to values.
struct Csv(HashMap<String, Vec<f64>>);

impl Csv {
    fn read(p: &Path) -> Csv {
        let text = std::fs::read_to_string(p).unwrap();
        let mut lines = text.lines();
        let names: Vec<String> = lines
            .next()
            .unwrap()
            .split(',')
            .map(|h| h.trim_matches('"').to_string())
            .collect();
        let mut cols = vec![Vec::new(); names.len()];
        for l in lines {
            for (k, x) in l.split(',').enumerate() {
                cols[k].push(x.parse::<f64>().unwrap());
            }
        }
        Csv(names.into_iter().zip(cols).collect())
    }
    fn col(&self, n: &str) -> &[f64] {
        self.0
            .get(n)
            .unwrap_or_else(|| panic!("{n} not in result ({:?})", self.0.keys()))
    }
    /// The right-hand values at each time (drop the left values at events).
    fn right(self) -> Csv {
        let t = self.col("time").to_vec();
        let keep: Vec<bool> = (0..t.len())
            .map(|i| i + 1 == t.len() || t[i + 1] > t[i])
            .collect();
        Csv(self
            .0
            .into_iter()
            .map(|(n, v)| {
                let v = v
                    .into_iter()
                    .zip(&keep)
                    .filter(|(_, k)| **k)
                    .map(|(x, _)| x)
                    .collect();
                (n, v)
            })
            .collect())
    }
}

fn interp(t: &[f64], y: &[f64], x: f64) -> f64 {
    let k = t.partition_point(|&s| s <= x);
    if k == 0 {
        return y[0];
    }
    if k >= t.len() {
        return y[t.len() - 1];
    }
    let (t0, t1) = (t[k - 1], t[k]);
    if t1 == t0 {
        return y[k];
    }
    y[k - 1] + (y[k] - y[k - 1]) * (x - t0) / (t1 - t0)
}

/// Largest deviation of `res.var_res` from `reference.var`, relative to the range of the
/// reference, away from the reference's events (where both jump).
fn deviation(reference: &Csv, var: &str, res: &Csv, var_res: &str) -> f64 {
    let (tr, yr) = (reference.col("time"), reference.col(var));
    let events: Vec<f64> = tr
        .windows(2)
        .filter(|w| w[0] == w[1])
        .map(|w| w[0])
        .collect();
    let span =
        yr.iter().cloned().fold(f64::MIN, f64::max) - yr.iter().cloned().fold(f64::MAX, f64::min);
    let span = span.max(1e-12);
    let (t, y) = (res.col("time"), res.col(var_res));
    let mut worst: f64 = 0.0;
    for (x, v) in t.iter().zip(y) {
        if events.iter().any(|e| (x - e).abs() <= 1e-3) {
            continue;
        }
        worst = worst.max((v - interp(tr, yr, *x)).abs() / span);
    }
    eprintln!("{var_res}: max deviation {worst:.2e} of range");
    worst
}

fn last(c: &Csv, v: &str) -> f64 {
    *c.col(v).last().unwrap()
}

/// Bouncing ball (state events with reinit inside the FMU), FMI 2.0 and 3.0, run
/// standalone as Model Exchange and Co-Simulation.
#[test]
fn ball_standalone() {
    let rt = need_runtime!();
    let s = Scratch::new("ball");
    let m = models();
    let m = m.to_str().unwrap();
    daedalus(
        &s,
        &rt,
        &[
            "simulate",
            m,
            "-m",
            "FmuImport.Ball",
            "-o",
            "ref.csv",
            "--vars",
            "height,v",
        ],
    );
    let reference = Csv::read(&s.path("ref.csv"));
    for v in ["2", "3"] {
        let fmu = format!("Ball{v}.fmu");
        daedalus(
            &s,
            &rt,
            &[
                "fmu",
                m,
                "-m",
                "FmuImport.Ball",
                "-o",
                &fmu,
                "--fmi-version",
                v,
            ],
        );
        daedalus(
            &s,
            &rt,
            &["simulate", &fmu, "-o", "me.csv", "--vars", "height,v"],
        );
        let me = Csv::read(&s.path("me.csv"));
        assert!(
            deviation(&reference, "height", &me, "height") < 1e-6,
            "FMI {v} ME"
        );
        assert!(deviation(&reference, "v", &me, "v") < 1e-6, "FMI {v} ME");
        daedalus(
            &s,
            &rt,
            &[
                "simulate",
                &fmu,
                "--fmi-type",
                "cs",
                "--comm-step",
                "0.006",
                "-o",
                "cs.csv",
                "--vars",
                "height",
            ],
        );
        let cs = Csv::read(&s.path("cs.csv")).right();
        assert!(
            deviation(&reference, "height", &cs, "height") < 1e-4,
            "FMI {v} CS"
        );
    }
}

const LOOP: &str = "model Loop{K}
  Plant{K} plant(k = 2.5);
  Controller{K} ctrl(kp = 1.2);
  Reference{K} ref;
equation
  connect(ref.r, ctrl.r);
  connect(plant.y, ctrl.ym);
  connect(ctrl.u, plant.u);
  annotation(experiment(StopTime = 5, Interval = 0.01));
end Loop{K};
";

/// A closed loop of three imported FMUs (a controller with direct feedthrough) against
/// the same loop built natively; then the Co-Simulation loop through SSP export/import.
#[test]
fn closed_loop_and_ssp() {
    let rt = need_runtime!();
    let s = Scratch::new("loop");
    let m = models();
    let m = m.to_str().unwrap();
    let vars = "plant.y,ctrl.u";
    daedalus(
        &s,
        &rt,
        &[
            "simulate",
            m,
            "-m",
            "FmuImport.Loop",
            "-o",
            "ref.csv",
            "--vars",
            vars,
        ],
    );
    let reference = Csv::read(&s.path("ref.csv"));
    for (v, c) in [("2", "Plant"), ("3", "Controller"), ("2", "Reference")] {
        // mixed FMI versions in one model
        let fmu = format!("{c}.fmu");
        daedalus(
            &s,
            &rt,
            &[
                "fmu",
                m,
                "-m",
                &format!("FmuImport.{c}"),
                "-o",
                &fmu,
                "--fmi-version",
                v,
            ],
        );
        for (kind, step) in [("me", None), ("cs", Some("0.01"))] {
            let class = format!("{c}{}", kind.to_uppercase());
            let mo = format!("{class}.mo");
            let mut args = vec![
                "import-fmu",
                &fmu,
                "-o",
                &mo,
                "--class",
                &class,
                "--fmi-type",
                kind,
            ];
            if let Some(h) = step {
                args.extend(["--comm-step", h]);
            }
            daedalus(&s, &rt, &args);
        }
    }
    for k in ["ME", "CS"] {
        s.write(&format!("Loop{k}.mo"), &LOOP.replace("{K}", k));
        daedalus(
            &s,
            &rt,
            &[
                "simulate",
                ".",
                "-m",
                &format!("Loop{k}"),
                "-o",
                &format!("{k}.csv"),
                "--vars",
                vars,
            ],
        );
    }
    // Model Exchange: the FMUs' equations are integrated with the model's: same answer.
    let me = Csv::read(&s.path("ME.csv"));
    assert!(deviation(&reference, "plant.y", &me, "plant.y") < 1e-5);
    assert!(deviation(&reference, "ctrl.u", &me, "ctrl.u") < 1e-5);
    // every integrator (the implicit ones with a difference Jacobian through the FMUs)
    for method in ["bdf", "radau", "esdirk", "auto", "dopri5"] {
        let out = format!("ME_{method}.csv");
        daedalus(
            &s,
            &rt,
            &[
                "simulate", ".", "-m", "LoopME", "--method", method, "-o", &out, "--vars", vars,
            ],
        );
        let r = Csv::read(&s.path(&out));
        for v in ["plant.y", "ctrl.u"] {
            let d = deviation(&reference, v, &r, v);
            assert!(d < 1e-4, "{method} {v}: {d}");
        }
    }
    // Co-Simulation: inputs held over a step (h = 0.01): O(h) error, here mostly the
    // reference step reaching the plant one or two communication steps late.
    let cs = Csv::read(&s.path("CS.csv")).right();
    assert!(deviation(&reference, "plant.y", &cs, "plant.y") < 0.1);
    assert!((last(&cs, "plant.y") - last(&reference, "plant.y")).abs() < 1e-3);
    assert!((last(&cs, "ctrl.u") - last(&reference, "ctrl.u")).abs() < 1e-3);

    // SSP: export the Co-Simulation loop, simulate the .ssp directly (h = 0.01 matches
    // the wrappers', so the same answer), and with h = 0.001 (error ~10x smaller).
    let out = daedalus(
        &s,
        &rt,
        &["export-ssp", ".", "-m", "LoopCS", "-o", "Loop.ssp"],
    );
    assert!(out.contains("Loop.ssp"), "{out}");
    daedalus(
        &s,
        &rt,
        &[
            "simulate",
            "Loop.ssp",
            "--comm-step",
            "0.01",
            "-o",
            "ssp.csv",
            "--vars",
            vars,
        ],
    );
    let ssp = Csv::read(&s.path("ssp.csv")).right();
    for v in ["plant.y", "ctrl.u"] {
        assert!(
            deviation(&cs, v, &ssp, v) < 1e-9,
            "{v}: SSP vs Modelica CS loop"
        );
    }
    daedalus(
        &s,
        &rt,
        &[
            "simulate",
            "Loop.ssp",
            "--comm-step",
            "0.001",
            "-o",
            "fine.csv",
            "--vars",
            vars,
        ],
    );
    let fine = Csv::read(&s.path("fine.csv")).right();
    assert!(deviation(&reference, "plant.y", &fine, "plant.y") < 0.01);

    // import-ssp: a Modelica model with the FMUs unpacked next to it.
    std::fs::create_dir_all(s.path("imp")).unwrap();
    daedalus(
        &s,
        &rt,
        &[
            "import-ssp",
            "Loop.ssp",
            "-o",
            "imp/Loop.mo",
            "--comm-step",
            "0.01",
        ],
    );
    daedalus(
        &s,
        &rt,
        &[
            "simulate",
            "imp/Loop.mo",
            "-m",
            "LoopCS",
            "-o",
            "imp.csv",
            "--vars",
            vars,
        ],
    );
    let imp = Csv::read(&s.path("imp.csv")).right();
    for v in ["plant.y", "ctrl.u"] {
        assert!(
            deviation(&cs, v, &imp, v) < 1e-9,
            "{v}: imported SSP vs Modelica CS loop"
        );
    }
}

/// Daedalus.Examples.TorqueConverter.StallTest exported as FMI 2.0 and 3.0, simulated
/// standalone (Model Exchange and Co-Simulation) against the native run.
#[test]
fn stall_test() {
    let rt = need_runtime!();
    let s = Scratch::new("stall");
    let lib = repo().join("library/Daedalus");
    let lib = lib.to_str().unwrap();
    let model = "Daedalus.Examples.TorqueConverter.StallTest";
    daedalus(
        &s,
        &rt,
        &[
            "simulate",
            lib,
            "-m",
            model,
            "-o",
            "ref.csv",
            "--vars",
            "rpm_engine,rpm_stall_theory",
        ],
    );
    let reference = Csv::read(&s.path("ref.csv"));
    for v in ["2", "3"] {
        let fmu = format!("Stall{v}.fmu");
        daedalus(
            &s,
            &rt,
            &["fmu", lib, "-m", model, "-o", &fmu, "--fmi-version", v],
        );
        for kind in ["me", "cs"] {
            daedalus(
                &s,
                &rt,
                &[
                    "simulate",
                    &fmu,
                    "--fmi-type",
                    kind,
                    "-o",
                    "fmu.csv",
                    "--vars",
                    "rpm_engine,rpm_stall_theory",
                ],
            );
            let res = Csv::read(&s.path("fmu.csv")).right();
            for var in ["rpm_engine", "rpm_stall_theory"] {
                let d = deviation(&reference, var, &res, var);
                assert!(d < 1e-4, "FMI {v} {kind} {var}: {d}");
            }
        }
    }
}

const HOST_INIT: &str = "model HostInit{K}
  Follower{K} f;
  parameter Real p = 3;
  Real a;
initial equation
  a = p;
equation
  der(a) = 0;
  f.u = a;
  annotation(experiment(StopTime = 1, Interval = 0.1));
end HostInit{K};
";

/// An FMU input driven by a host variable that only the host's initial equations
/// determine: the FMU initializes with the solved value (its state `x(0) = u(0)` is 3),
/// not with the start value 0 the host variable had before its initialization.
#[test]
fn fmu_initialized_with_solved_inputs() {
    let rt = need_runtime!();
    let s = Scratch::new("hostinit");
    let m = models();
    let m = m.to_str().unwrap();
    for v in ["2", "3"] {
        let fmu = format!("Follower{v}.fmu");
        daedalus(
            &s,
            &rt,
            &[
                "fmu",
                m,
                "-m",
                "FmuImport.Follower",
                "-o",
                &fmu,
                "--fmi-version",
                v,
            ],
        );
        for kind in ["me", "cs"] {
            let class = format!("Follower{}", kind.to_uppercase());
            let mut args = vec![
                "import-fmu",
                &fmu,
                "-o",
                "F.mo",
                "--class",
                &class,
                "--fmi-type",
                kind,
            ];
            if kind == "cs" {
                args.extend(["--comm-step", "0.1"]);
            }
            daedalus(&s, &rt, &args);
            let k = kind.to_uppercase();
            std::fs::rename(s.path("F.mo"), s.path(&format!("{class}.mo"))).unwrap();
            s.write(&format!("HostInit{k}.mo"), &HOST_INIT.replace("{K}", &k));
            daedalus(
                &s,
                &rt,
                &[
                    "simulate",
                    ".",
                    "-m",
                    &format!("HostInit{k}"),
                    "-o",
                    "r.csv",
                    "--vars",
                    "a,f.y",
                ],
            );
            let r = Csv::read(&s.path("r.csv"));
            for (i, y) in r.col("f.y").iter().enumerate() {
                assert!(
                    (y - 3.0).abs() < 1e-9,
                    "FMI {v} {kind}: f.y = {y} at row {i} (expected 3)"
                );
            }
        }
    }
}

/// A wrapper whose FMU file is missing fails with a clear message.
#[test]
fn missing_fmu() {
    let rt = need_runtime!();
    let s = Scratch::new("missing");
    s.write(
        "W.mo",
        "block W\n  output Real y;\n  annotation(__Daedalus_FMU(fileName = \"nowhere.fmu\", fmiType = \"CoSimulation\"));\nend W;\n",
    );
    let o = Command::new(env!("CARGO_BIN_EXE_daedalus"))
        .args(["simulate", "W.mo", "-m", "W"])
        .current_dir(&s.0)
        .env("DAEDALUS_FMI_LIB", &rt)
        .output()
        .unwrap();
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(!o.status.success());
    assert!(err.contains("nowhere.fmu"), "{err}");
}
