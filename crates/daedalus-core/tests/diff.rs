//! Structural and text diff of two versions of a class (`daedalus diff`).

use daedalus_core::diff::{diff_sources, text_diff, unified};

const OLD: &str = r#"within Demo;
model Plant "Mass on a spring"
  parameter Real k = 100 "Stiffness";
  parameter Real m = 1;
  Spring spring(c=k) annotation(Placement(transformation(extent={{-10,-10},{10,10}})));
  Mass mass(m=m) annotation(Placement(transformation(extent={{20,-10},{40,10}})));
  Fixed fixed annotation(Placement(transformation(extent={{-40,-10},{-20,10}})));
  Real e;
equation
  e = 0.5*k*spring.s^2;
  connect(fixed.flange, spring.flange_a) annotation(Line(points={{-20,0},{-10,0}}));
  connect(spring.flange_b, mass.flange_a) annotation(Line(points={{10,0},{20,0}}));
  annotation(experiment(StopTime=1));
end Plant;
"#;

const NEW: &str = r#"within Demo;
model Plant "Mass on a spring"
  parameter Real k = 120 "Stiffness";
  parameter Real m = 1;
  parameter Real d = 2 "Damping";
  Spring spring(c=k) annotation(Placement(transformation(extent={{-10,-10},{10,10}})));
  Mass mass(m=m, s(start=0.1)) annotation(Placement(transformation(extent={{40,-10},{60,10}})));
  Damper damper(d=d) annotation(Placement(transformation(extent={{-10,20},{10,40}})));
  Real e;
equation
  e = 0.5*k*spring.s^2 + 0.5*m*mass.v^2;
  connect(mass.flange_a, spring.flange_b) annotation(Line(points={{10,0},{40,0}}));
  connect(spring.flange_a, damper.flange_a);
  annotation(experiment(StopTime=2));
end Plant;
"#;

#[test]
fn structural_diff_finds_components_parameters_connections_equations() {
    let d = diff_sources(OLD, NEW, Some("Demo.Plant"), None).unwrap();
    assert_eq!(d.old_class, "Demo.Plant");
    let comp = |n: &str| d.components.iter().find(|c| c.name == n);

    // parameters
    let k = comp("k").expect("k changed");
    assert_eq!(k.change, "modified");
    assert!(k.parameter);
    assert!(k.details.iter().any(|x| x.contains("= 100 -> = 120")), "{:?}", k.details);
    let dd = comp("d").expect("d added");
    assert_eq!(dd.change, "added");
    assert!(dd.parameter);
    assert!(comp("m").is_none(), "m is unchanged");

    // components: added, removed, moved with a new modifier
    assert_eq!(comp("damper").unwrap().change, "added");
    assert_eq!(comp("fixed").unwrap().change, "removed");
    assert!(comp("fixed").unwrap().old_placement.is_some());
    let mass = comp("mass").unwrap();
    assert_eq!(mass.change, "modified");
    assert!(mass.moved);
    assert!(mass.details.iter().any(|x| x.starts_with("modifier s")), "{:?}", mass.details);
    assert!(comp("spring").is_none());

    // connections: the swapped spring-mass connection is the same one, rerouted
    let ch = |c: &str| d.connections.iter().filter(|x| x.change == c).count();
    assert_eq!(ch("removed"), 1, "{:?}", d.connections);
    assert_eq!(ch("added"), 1);
    assert_eq!(ch("rerouted"), 1);
    let removed = d.connections.iter().find(|c| c.change == "removed").unwrap();
    assert_eq!((removed.from.as_str(), removed.to.as_str()), ("fixed.flange", "spring.flange_a"));

    // equations
    assert_eq!(d.equations.iter().filter(|e| e.change == "removed").count(), 1);
    assert_eq!(d.equations.iter().filter(|e| e.change == "added").count(), 1);
    assert!(d.equations.iter().any(|e| e.text.contains("mass.v")));

    // other
    assert!(d.other.iter().any(|o| o.starts_with("experiment")));
    assert!(!d.is_empty());

    let r = d.report();
    assert!(r.contains("+ damper : Damper"), "{r}");
    assert!(r.contains("- connect(fixed.flange, spring.flange_a)"), "{r}");
}

#[test]
fn identical_classes_have_no_diff() {
    let d = diff_sources(OLD, OLD, None, None).unwrap();
    assert!(d.is_empty(), "{}", d.report());
    assert!(d.report().contains("no differences"));
}

#[test]
fn text_diff_and_unified_format() {
    let a = "a\nb\nc\nd\ne\nf\n";
    let b = "a\nb\nX\nd\ne\nf\ng\n";
    let lines = text_diff(a, b);
    let ops: String = lines.iter().map(|l| l.op).collect();
    assert_eq!(ops, "  -+   +");
    let u = unified(&lines, "a", "b", 1);
    assert!(u.starts_with("--- a\n+++ b\n@@ -2,3 +2,3 @@\n b\n-c\n+X\n d\n"), "{u}");
    assert!(u.contains("+g"));
    // line numbers in the diff are those of the files
    let d = diff_sources(OLD, NEW, Some("Plant"), None).unwrap();
    let first = d.text.iter().find(|l| l.op == '-').unwrap();
    assert_eq!(first.old, Some(3));
    assert!(first.text.contains("k = 100"));
}

#[test]
fn renamed_class_and_missing_class() {
    let renamed = NEW.replace("Plant", "Plant2");
    let d = diff_sources(OLD, &renamed, Some("Demo.Plant"), Some("Demo.Plant2")).unwrap();
    assert_eq!(d.new_class, "Demo.Plant2");
    assert!(diff_sources(OLD, NEW, Some("Demo.Nope"), None).is_err());
}
