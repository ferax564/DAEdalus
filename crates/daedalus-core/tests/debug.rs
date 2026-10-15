//! Equation and initialization debugger: transformation chains with source positions,
//! initialization and solver failure reports, structural diagnosis and unconnected
//! connectors, on small faulty models (`models/debug.mo`).

use daedalus_core::backend::Options;
use daedalus_core::debug::{Debugger, init_report, simulation_failure};
use daedalus_core::frontend::Library;
use daedalus_core::sim::SimOptions;

fn path() -> String {
    format!("{}/tests/models/debug.mo", env!("CARGO_MANIFEST_DIR"))
}

fn lib() -> Library {
    let p = path();
    let text = std::fs::read_to_string(&p).unwrap();
    let mut lib = Library::new();
    let errs = lib.set_document(&p, &text);
    assert!(errs.is_empty(), "{errs:?}");
    lib
}

/// 1-based line of the first line of the model file containing `needle` (after line `after`).
fn line_of(needle: &str, after: usize) -> u32 {
    let text = std::fs::read_to_string(path()).unwrap();
    text.lines()
        .enumerate()
        .skip(after)
        .find(|(_, l)| l.contains(needle))
        .map(|(i, _)| i as u32 + 1)
        .unwrap_or_else(|| panic!("{needle} not in the model file"))
}

fn debugger(model: &str) -> Debugger {
    match Debugger::new(&lib(), model, &Options::default()) {
        Ok(d) => d,
        Err(f) => panic!("{}: {}", f.stage, f.error),
    }
}

/// An alias found through a connection: the chain names the alias equations with their
/// components and file positions, then how the representative is computed.
#[test]
fn alias_chain_with_source_positions() {
    let d = debugger("Debug.RC");
    let t = d.trace_var("r.i").unwrap();
    assert_eq!(t.role, "alias");
    let alias_steps: Vec<_> = t.steps.iter().filter(|s| s.kind == "alias").collect();
    assert!(alias_steps.len() >= 2, "{}", t.to_text());
    // `i = p.i` of TwoPin, inherited by r: the equation's own line, component r.
    let s = alias_steps
        .iter()
        .find(|s| {
            s.source
                .as_ref()
                .is_some_and(|s| s.component.as_deref() == Some("r"))
        })
        .unwrap_or_else(|| panic!("{}", t.to_text()));
    let loc = s
        .source
        .as_ref()
        .unwrap()
        .loc
        .as_ref()
        .expect("a source position");
    assert!(loc.file.ends_with("debug.mo"));
    assert_eq!(loc.line, line_of("i = p.i;", 0));
    // A connection equation names its connectors.
    assert!(
        t.steps.iter().any(
            |s| s.source.as_ref().is_some_and(|s| s.origin == "connection"
                && s.component.as_deref().is_some_and(|c| c.contains("r.p")))
        ),
        "{}",
        t.to_text()
    );
    // The representative is solved from the resistor's law (after substitution).
    assert!(
        t.steps.iter().any(|s| s.kind == "assign"),
        "{}",
        t.to_text()
    );
    assert!(t.steps.iter().any(|s| s.kind == "source"
        && s.source.as_ref().unwrap().loc.as_ref().unwrap().line == line_of("v = R*i;", 0)));
}

/// A state: its derivative's equation, and the fixed start value from the modifier with
/// its position.
#[test]
fn state_trace_and_start_source() {
    let d = debugger("Debug.RC");
    let t = d.trace_var("c.v").unwrap();
    assert_eq!(t.role, "state");
    assert_eq!(t.start.fixed, Some(true));
    assert_eq!(
        t.start.written_in.as_deref(),
        Some(""),
        "set in the model itself"
    );
    assert_eq!(
        t.start.loc.as_ref().unwrap().line,
        line_of("Capacitor c(C = 0.1", 0)
    );
    assert!(t.steps.iter().any(|s| s.kind == "state"));
    assert!(
        t.steps.iter().any(
            |s| s.kind == "assign" && s.equation.as_deref().unwrap().starts_with("der(c.v) :=")
        )
    );
    assert!(
        t.steps
            .iter()
            .any(|s| s.kind == "initialization" && s.title.contains("fixed start value"))
    );
    // der(x) names the derivative.
    let dt = d.trace_var("der(c.v)").unwrap();
    assert_eq!(dt.role, "derivative");
}

/// Index reduction: the constraint of the pendulum is differentiated twice, constrains a
/// state set, and its second derivative is the residual of a torn loop whose tear is F.
#[test]
fn index_reduction_chain() {
    let d = debugger("Debug.Pendulum");
    let t = d.trace_var("F").unwrap();
    assert_eq!(t.tearing_role, Some("tear"), "{}", t.to_text());
    let kinds: Vec<&str> = t.steps.iter().map(|s| s.kind).collect();
    assert_eq!(
        kinds.iter().filter(|k| **k == "differentiated").count(),
        2,
        "{}",
        t.to_text()
    );
    assert!(kinds.contains(&"state-selection"), "{}", t.to_text());
    assert!(kinds.contains(&"tear"));
    let src = t.steps[0].source.as_ref().unwrap();
    assert_eq!(
        src.loc.as_ref().unwrap().line,
        line_of("x^2 + y^2 = L^2;", 0)
    );
    // One of x, y (and of vx, vy) became a dummy derivative.
    let dummies: Vec<_> = d
        .report
        .variables
        .iter()
        .filter(|v| v.role == "dummy derivative")
        .collect();
    assert!(!dummies.is_empty());
    let dt = d.trace_var(&dummies[0].name).unwrap();
    assert!(
        dt.steps.iter().any(|s| s.kind == "state-selection"),
        "{}",
        dt.to_text()
    );
    // Equation traces cover every equation of the structure report.
    for e in 0..d.report.equations.len() {
        let et = d.trace_eq(e).unwrap();
        assert!(!et.steps.is_empty());
    }
}

/// Tearing roles in a nonlinear loop: the tear variable and the inner variables.
#[test]
fn tearing_roles() {
    let d = debugger("Debug.DiodeLoop");
    let b = d
        .report
        .blocks
        .iter()
        .find(|b| b.kind == "nonlinear")
        .expect("a nonlinear loop");
    let tear = &b.tears[0];
    let t = d.trace_var(tear).unwrap();
    assert_eq!(t.tearing_role, Some("tear"));
    assert_eq!(t.block_kind, Some("nonlinear"));
    let inner = b.vars.iter().find(|v| !b.tears.contains(v)).unwrap();
    let ti = d.trace_var(inner).unwrap();
    assert_eq!(ti.tearing_role, Some("inner"), "{}", ti.to_text());
    assert!(ti.steps.iter().any(|s| s.kind == "inner"));
}

/// A failing initialization: the start values used (and the modifier that set the guess),
/// the failing block's residuals at the last iterate and the Newton updates.
#[test]
fn init_failure_report() {
    let r = init_report(&lib(), "Debug.InitFail", &Options::default(), None, &[]);
    assert!(!r.ok);
    assert_eq!(r.stage, "initialization");
    let s = r.unknowns.iter().find(|u| u.name == "s").unwrap();
    assert_eq!(s.start_use, "fixed");
    let x = r
        .unknowns
        .iter()
        .find(|u| u.name == "b.x" || u.name == "b.y")
        .unwrap();
    if x.name == "b.x" {
        assert_eq!(x.start_use, "guess");
        assert_eq!(x.start_value, Some(2.0), "the modifier b(x0 = 2)");
        assert_eq!(x.start.written_in.as_deref(), Some("b"));
        assert_eq!(
            x.start.loc.as_ref().unwrap().line,
            line_of("Real x(start = x0);", 0)
        );
    }
    let f = r.failure.as_ref().expect("a solver failure");
    assert_eq!(f.phase, "initialization");
    assert!(!f.linear);
    assert!(f.residual_norm > 1.0, "{}", r.to_text());
    assert!(!f.residuals.is_empty() && !f.updates.is_empty());
    // The residual that cannot vanish, with its source line.
    let top = &f.residuals[0];
    assert!(top.equation.contains("exp(b.x)"), "{}", r.to_text());
    assert_eq!(
        top.source.as_ref().unwrap().loc.as_ref().unwrap().line,
        line_of("exp(b.x) + b.y^2 = -1;", 0)
    );
    assert!(!f.hints.is_empty(), "{}", r.to_text());
    assert!(r.to_text().contains("largest Newton updates"));
}

#[test]
fn init_report_of_a_healthy_model() {
    let r = init_report(&lib(), "Debug.Pendulum", &Options::default(), None, &[]);
    assert!(r.ok, "{}", r.to_text());
    assert!(r.failure.is_none());
    let x = r.unknowns.iter().find(|u| u.name == "x").unwrap();
    assert_eq!(x.start_use, "fixed");
    assert_eq!(x.value, Some(1.0));
    // y has no fixed attribute: its start value closes the initial system or is a guess.
    assert!(r.relevant >= 2);
    assert!(r.unconnected.is_empty());
}

/// A nonlinear solve that fails during the simulation (no real root after t = 2.25).
#[test]
fn simulation_failure_report() {
    let lib = lib();
    let so = SimOptions {
        stop_time: 4.0,
        ..Default::default()
    };
    let f = simulation_failure(&lib, "Debug.SimFail", &Options::default(), so)
        .expect("a nonlinear failure");
    assert_eq!(f.phase, "simulation");
    assert!((f.time - 2.25).abs() < 0.05, "{}", f.to_text());
    assert_eq!(f.tears, vec!["x".to_string()]);
    assert!(f.residuals[0].source.is_some());
}

/// Underdetermined: the unknown no equation is left for, with its component.
#[test]
fn underdetermined_subset() {
    let r = init_report(
        &lib(),
        "Debug.Underdetermined",
        &Options::default(),
        None,
        &[],
    );
    assert_eq!(r.stage, "translation");
    let d = r.structural.as_ref().expect("structural diagnosis");
    assert!(d.overdetermined.is_empty());
    let sub = &d.underdetermined[0];
    assert_eq!(sub.unmatched, vec!["leak.w".to_string()]);
    assert_eq!(sub.components, vec!["leak".to_string()]);
    assert!(!d.hints.is_empty());
}

/// Overdetermined: the extra equation with its component, and the equation alias
/// elimination used for the same variable.
#[test]
fn overdetermined_subset() {
    let r = init_report(
        &lib(),
        "Debug.Overdetermined",
        &Options::default(),
        None,
        &[],
    );
    let d = r.structural.as_ref().expect("structural diagnosis");
    assert!(d.underdetermined.is_empty());
    let sub = &d.overdetermined[0];
    assert!(
        sub.components.contains(&"f".to_string()),
        "{:?}",
        sub.components
    );
    let lines: Vec<u32> = sub
        .equations
        .iter()
        .filter_map(|e| e.loc.as_ref().map(|l| l.line))
        .collect();
    assert!(lines.contains(&line_of("a = 1;", 0)), "{}", d.to_text());
    assert!(lines.contains(&line_of("f.a = 2;", 0)), "{}", d.to_text());
}

/// Unconnected connectors: an open pin and a floating component are warnings; the
/// healthy circuits have none.
#[test]
fn unconnected_connectors() {
    let r = init_report(&lib(), "Debug.OpenPin", &Options::default(), None, &[]);
    let w: Vec<&str> = r
        .unconnected
        .iter()
        .filter(|u| u.level == "warning")
        .map(|u| u.connector.as_str())
        .collect();
    assert_eq!(w, vec!["r2.n", "r3.n", "r3.p"], "{}", r.to_text());
    // The floating r3 also makes the model singular; the diagnosis points at it.
    let d = r.structural.as_ref().unwrap();
    assert!(d.hints.iter().any(|h| h.contains("r3")), "{}", d.to_text());
    for m in ["Debug.RC", "Debug.DiodeLoop"] {
        let r = init_report(&lib(), m, &Options::default(), None, &[]);
        assert!(r.ok, "{}", r.to_text());
        assert!(r.unconnected.is_empty(), "{m}: {:?}", r.unconnected);
    }
}

/// False-positive survey of the unconnected-connector check over the example models of
/// libraries (`DEBUG_SURVEY=dir:Prefix[,dir:Prefix]`); prints every warning.
#[test]
#[ignore]
fn unconnected_survey() {
    let Ok(spec) = std::env::var("DEBUG_SURVEY") else {
        return;
    };
    let mut lib = Library::new();
    let mut prefixes = Vec::new();
    for item in spec.split(',') {
        let (dir, prefix) = item.split_once(':').unwrap();
        lib.load_dir(std::path::Path::new(dir)).unwrap();
        prefixes.push(prefix.to_string());
    }
    let tree = lib.tree();
    let mut models = Vec::new();
    tree.walk(&mut |c| {
        let node = tree.node(c);
        if prefixes
            .iter()
            .any(|p| node.path.starts_with(&format!("{p}.")))
            && let Some(def) = node.def
            && !def.partial
            && matches!(
                def.class_type,
                rumoca_core::ClassType::Model | rumoca_core::ClassType::Block
            )
            && def.annotation.iter().any(|a| {
                matches!(a,
                rumoca_ir_ast::Expression::ClassModification { target, .. }
                    if target.parts.len() == 1 && &*target.parts[0].ident.text == "experiment")
            })
        {
            models.push(node.path.clone());
        }
    });
    drop(tree);
    let (mut n, mut warned, mut notes, mut warnings) = (0, 0, 0, 0);
    for m in &models {
        let Ok(Ok(flat)) =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| lib.flatten(m)))
        else {
            continue;
        };
        n += 1;
        let u = daedalus_core::debug::unconnected_connectors(&flat);
        let w: Vec<_> = u.iter().filter(|u| u.level == "warning").collect();
        notes += u.len() - w.len();
        if !w.is_empty() {
            warned += 1;
            warnings += w.len();
            for x in &w {
                println!("{m}: {} -- {}", x.connector, x.reason);
            }
        }
    }
    println!("{n} models flattened, {warned} with warnings ({warnings} warnings), {notes} notes");
}
