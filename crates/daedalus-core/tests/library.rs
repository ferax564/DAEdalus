//! Regression tests for the Daedalus Modelica library: every example simulates with its own
//! experiment settings and reproduces the physics it documents.

use daedalus_core::frontend::Library;
use daedalus_core::results::Results;
use daedalus_core::sim::{SimOptions, simulate};
use std::sync::OnceLock;

fn library() -> &'static Library {
    static LIB: OnceLock<Library> = OnceLock::new();
    LIB.get_or_init(|| {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/Daedalus");
        let mut lib = Library::new();
        lib.load_dir(&dir).expect("library parses");
        lib
    })
}

fn run_with(model: &str, params: &[(&str, f64)], stop: Option<f64>) -> Results {
    let m = daedalus_core::translate(library(), model, &Default::default())
        .unwrap_or_else(|e| panic!("{model}: {e:#}"));
    let mut opts = SimOptions::from_model(&m);
    opts.params = params.iter().map(|(k, v)| (k.to_string(), *v)).collect();
    if let Some(t) = stop {
        opts.stop_time = t;
    }
    simulate(&m, opts)
        .map_err(|e| format!("{model}: {}", e.message))
        .unwrap()
        .0
}

fn run(model: &str) -> Results {
    run_with(model, &[], None)
}

fn max_of(r: &Results, var: &str) -> f64 {
    r.values(var)
        .unwrap()
        .iter()
        .cloned()
        .fold(f64::MIN, f64::max)
}

#[test]
fn torque_converter_stall_speed_matches_k_factor() {
    let r = run("Daedalus.Examples.TorqueConverter.StallTest");
    let rpm = r.final_value("rpm_engine").unwrap();
    let theory = r.final_value("rpm_stall_theory").unwrap();
    assert!((rpm - theory).abs() < 1.0, "{rpm} vs {theory}");
    assert!((2000.0..2300.0).contains(&rpm), "{rpm}");
}

#[test]
fn torque_converter_launch_locks_up() {
    let r = run("Daedalus.Examples.TorqueConverter.Launch");
    for d in ["static", "dynamic"] {
        let t_lock = r.final_value(&format!("{d}.t_lock")).unwrap();
        assert!(t_lock > 1.0 && t_lock < 4.0, "{d}: lock-up at {t_lock}");
        // after lock-up the converter no longer slips
        assert!(r.final_value(&format!("{d}.converter.speedRatio")).unwrap() > 0.99);
        assert!(r.final_value(&format!("{d}.v_kmh")).unwrap() > 40.0);
    }
}

#[test]
fn landing_gear_load_factor_rises_with_sink_rate() {
    let r = run("Daedalus.Examples.LandingGear.SinkRates");
    let n: Vec<f64> = ["low", "limit", "reserve"]
        .iter()
        .map(|k| max_of(&r, &format!("{k}.loadFactor")))
        .collect();
    assert!(n[0] < n[1] && n[1] < n[2], "{n:?}");
    assert!(
        (1.2..2.0).contains(&n[1]),
        "limit drop load factor {}",
        n[1]
    );
    let x = max_of(&r, "reserve.strut.x");
    assert!(x < 0.4, "reserve drop bottoms out the strut: {x}");
}

#[test]
fn aileron_follows_command() {
    let r = run("Daedalus.Examples.AileronActuator.PositionControl");
    assert!((r.at("delta_deg", 0.95).unwrap() - 10.0).abs() < 0.2);
    assert!((r.at("delta_deg", 1.75).unwrap() + 10.0).abs() < 0.2);
    assert!(r.final_value("delta_deg").unwrap().abs() < 0.05);
}

#[test]
fn actuator_speed_falls_with_load() {
    let r = run("Daedalus.Examples.AileronActuator.SpeedVsLoad");
    let t = &r.time;
    let v = r.values("v_rod").unwrap();
    let vt = r.values("v_theory").unwrap();
    let mut last = f64::MAX;
    for i in 0..t.len() {
        if t[i] > 1.0 && t[i] < 20.4 {
            assert!(
                v[i] <= vt[i] + 1e-3,
                "faster than the ideal valve at t={}",
                t[i]
            );
            assert!(
                v[i] <= last + 1e-3,
                "speed must fall as load rises at t={}",
                t[i]
            );
            last = v[i];
        }
    }
    assert!(v[t.len() - 1] > 0.05, "still moving at 95 % of stall force");
}

#[test]
fn quarter_car_returns_to_ride_height() {
    let r = run("Daedalus.Examples.QuarterCar.BumpComparison");
    for k in ["linear", "clean", "aerated"] {
        let s = r.final_value(&format!("{k}.body.s")).unwrap();
        assert!((s - 0.6).abs() < 2e-3, "{k}: {s}");
        assert!(max_of(&r, &format!("{k}.F_tyre")) > 8000.0);
    }
    // the hydraulic damper is asymmetric: rebound force exceeds compression force
    let f = r.values("clean.damper.F_damper").unwrap();
    let (lo, hi) = f
        .iter()
        .fold((0.0f64, 0.0f64), |(a, b), &x| (a.min(x), b.max(x)));
    assert!(lo < -1000.0 && hi > 1000.0, "{lo} {hi}");
}

#[test]
fn ev_sensitivity_one_cycle() {
    let r = run_with(
        "Daedalus.Examples.VehicleEfficiency.NEDCSensitivity",
        &[("repetitions", 1.0)],
        Some(1180.0),
    );
    let base = r.final_value("baseline.meter.Wh_per_km").unwrap();
    assert!((70.0..160.0).contains(&base), "{base} Wh/km");
    let km = r.final_value("baseline.distance_km").unwrap();
    assert!((km - 10.70).abs() < 0.05, "{km} km");
    for d in ["delta_uphill", "delta_crosswind", "delta_roofBox"] {
        assert!(r.final_value(d).unwrap() > 1.0, "{d}");
    }
}

#[test]
fn coast_down_and_road_load_agree() {
    let r = run("Daedalus.Examples.VehicleEfficiency.CoastDown");
    let v = r.final_value("v_kmh").unwrap();
    assert!((v - 15.0).abs() < 0.1, "terminated at {v} km/h");
    let r = run("Daedalus.Examples.VehicleEfficiency.UDDSEnergy");
    assert!(r.final_value("energy_difference").unwrap().abs() < 0.5);
    let km = r.final_value("physical.distance_km").unwrap();
    assert!((km - 11.99).abs() < 0.05, "{km} km");
}

#[test]
fn casadi_export_of_aileron_servo() {
    let flat = library()
        .flatten("Daedalus.Examples.OptimalControl.AileronServo")
        .unwrap();
    let sys = daedalus_core::backend::compile(flat, &Default::default()).unwrap();
    let doc = daedalus_core::casadi::export(sys, &SimOptions::default()).unwrap();
    assert_eq!(doc["format"], "daedalus-casadi");
    assert_eq!(doc["initialized"], true);
    let len = |k: &str| doc[k].as_array().unwrap().len();
    assert_eq!(len("states"), 6);
    assert_eq!(len("derivatives"), 6);
    assert_eq!(len("inputs"), 1);
    assert_eq!(len("algebraics"), len("residuals"));
    let vars = doc["variables"].as_array().unwrap();
    let u = &vars[doc["inputs"][0].as_u64().unwrap() as usize];
    assert_eq!(u["name"], "u");
    // Every node refers only to earlier nodes (the loader relies on it for children).
    for (i, n) in doc["nodes"].as_array().unwrap().iter().enumerate() {
        let n = n.as_array().unwrap();
        let op = n[0].as_str().unwrap();
        let children: &[serde_json::Value] = match op {
            "const" | "time" | "var" => &[],
            "f1" => &n[2..],
            "f2" | "cmp" => &n[2..],
            "interp" | "interp_der" => &n[3..],
            _ => &n[1..],
        };
        for c in children {
            assert!(
                (c.as_u64().unwrap() as usize) < i,
                "node {i} ({op}) refers forward"
            );
        }
    }
}
