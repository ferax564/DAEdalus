//! Stepping a simulation from outside (FMI Co-Simulation, `linearize --at`) and the
//! values an FMU starts from.

use daedalus_core::codegen::Model;
use daedalus_core::frontend::Library;
use daedalus_core::sim::{SimOptions, Simulation, simulate};

const SRC: &str = r#"
package S
  model Counter "counts the instants of sample(0, 0.25)"
    discrete Integer n(start = 0, fixed = true);
    Real x(start = 0, fixed = true);
  equation
    der(x) = 1;
    when sample(0, 0.25) then
      n = pre(n) + 1;
    end when;
  end Counter;

  model Inputs "an input with a start value feeding a state's start and an output"
    input Real u(start = 2);
    output Real y;
    Real x(start = 0, fixed = false);
  initial equation
    x = 3 * u;
  equation
    der(x) = -x + u;
    y = 10 * u;
  end Inputs;
end S;
"#;

fn model(name: &str) -> Model {
    let mut lib = Library::new();
    let errs = lib.set_document("stepping.mo", SRC);
    assert!(errs.is_empty(), "{errs:?}");
    daedalus_core::translate(&lib, name, &Default::default()).unwrap()
}

fn var(m: &Model, name: &str) -> usize {
    m.vars.iter().position(|v| v.name == name).unwrap()
}

#[test]
fn advance_to_handles_a_time_event_at_its_end() {
    let m = model("S.Counter");
    let n = var(&m, "n");
    // The full run counts the instants 0, 0.25, 0.5, 0.75 when it stops at 0.75.
    let opts = SimOptions {
        stop_time: 0.75,
        ..SimOptions::from_model(&m)
    };
    let (r, _) = simulate(&m, opts.clone()).unwrap();
    assert_eq!(r.final_value("n").unwrap(), 4.0);
    // Stepping on the sample grid sees the same right values, each instant counted once,
    // also when a step ends between two instants.
    let mut sim = Simulation::new(
        &m,
        SimOptions {
            stop_time: 2.0,
            ..opts
        },
    );
    sim.initialize().unwrap();
    assert_eq!(sim.rt.r[n], 1.0);
    let mut st = sim.stepper();
    let events0 = sim.stats.events;
    for (t, want) in [
        (0.25, 2.0),
        (0.5, 3.0),
        (0.75, 4.0),
        (0.875, 4.0),
        (1.0, 5.0),
        (1.25, 6.0),
    ] {
        sim.advance_to(&mut st, t).unwrap();
        assert_eq!(sim.rt.r[n], want, "n at t = {t}");
    }
    assert_eq!(sim.stats.events - events0, 5);
}

#[test]
fn input_override_before_initialization() {
    // fmi2SetReal on an input before fmi2ExitInitializationMode: the value replaces the
    // input's start value in the initialization.
    let m = model("S.Inputs");
    let opts = SimOptions {
        params: vec![("u".to_string(), 5.0)],
        ..SimOptions::from_model(&m)
    };
    let mut sim = Simulation::new(&m, opts);
    sim.initialize().unwrap();
    sim.rt.run(&m.outputs);
    assert_eq!(sim.rt.r[var(&m, "u")], 5.0);
    assert_eq!(sim.rt.r[var(&m, "x")], 15.0);
    assert_eq!(sim.rt.r[var(&m, "y")], 50.0);
}

#[test]
fn fmi_input_start_values() {
    let m = model("S.Inputs");
    let d = daedalus_core::fmi::describe(&m);
    let u = d.vars.iter().find(|v| v.name == "u").unwrap();
    assert_eq!(u.start, Some(2.0));
}
