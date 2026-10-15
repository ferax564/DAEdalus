//! Unit strings, display-unit conversion, and unit checking / inference on flat models.

use daedalus_core::frontend::Library;
use daedalus_core::units::{self, UnitReport, conversion, display_choices, parse_unit};

const SRC: &str = r#"
package U
  type Length = Real(unit = "m");
  type Time = Real(unit = "s");
  type Velocity = Real(unit = "m/s");
  type Mass = Real(unit = "kg");
  type Force = Real(unit = "N");
  type Angle = Real(unit = "rad", displayUnit = "deg");
  type AngularVelocity = Real(unit = "rad/s");
  type Torque = Real(unit = "N.m");
  type Power = Real(unit = "W");

  model Consistent
    parameter Mass m = 2;
    parameter Real c(unit = "N/m") = 100;
    parameter Real d(unit = "N.s/m") = 3;
    Length x(start = 0.1, fixed = true);
    Velocity v(start = 0, fixed = true);
    Force f;
    Real a "no unit: inferred m/s2";
    Real e "no unit: inferred J";
    Real k "no unit: inferred from f = k*x";
  equation
    der(x) = v;
    a = der(v);
    m*a = -c*x - d*v + f;
    f = 0.5*sin(6.28*time);
    e = m*v^2/2 + c*x^2/2;
    f = k*x + 1;
  end Consistent;

  model Inconsistent
    Length x(start = 1, fixed = true);
    Time t;
    Velocity v;
  equation
    der(x) = -x + t "m/s vs m vs s";
    t = time;
    v = x "m/s vs m";
  end Inconsistent;

  model Transcendental
    parameter Velocity c = 1;
    Length x;
    Real y;
    Real z "exp of a scaled length: polymorphic literal, no issue";
  equation
    x = c*time;
    y = sin(x);
    z = exp(2*x);
  end Transcendental;

  model Rotation
    Angle phi(start = 0, fixed = true);
    AngularVelocity w(start = 1, fixed = true);
    Torque tau;
    Power P;
    parameter Real J(unit = "kg.m2") = 1;
    parameter Time Modelica_s = 1;
  equation
    der(phi) = w;
    J*der(w) = tau;
    tau = -J*w/(1*Modelica_s);
    P = tau*w;
  end Rotation;

  model Gain "unit '1' parameters convert between units (MSL style)"
    parameter Real k(unit = "1") = 2;
    parameter Velocity v0 = 1;
    Length x;
    Velocity v;
  equation
    x = v0*time;
    v = k*x;
  end Gain;

  model IfEquation "each branch of an if-equation is its own equation"
    parameter Velocity v0 = 1;
    Length x;
    Force f;
    Boolean on = time > 0.5;
    Real s "path variable: a length in one branch, a force in the other";
  equation
    x = v0*time;
    if on then
      x = s;
    else
      f = s;
    end if;
    f = 1;
  end IfEquation;

  function kinetic "units of inputs and outputs, checked through inlining"
    input Mass m;
    input Velocity v;
    output Real e(unit = "J");
  algorithm
    e := m*v*v;
  end kinetic;

  model FunctionCall
    parameter Real a(unit = "m/s2") = 1;
    Mass m = 1;
    Velocity v = a*time;
    Real e;
    Force bad;
  equation
    e = kinetic(m, v);
    bad = kinetic(m, v);
  end FunctionCall;

  model Bindings
    parameter Length L = 2;
    parameter Time T = L "length bound to a time";
    parameter Real g = L/T^2 "inferred m/s2";
    Real y = g*time;
  end Bindings;
end U;
"#;

fn report(model: &str) -> (daedalus_core::flat::FlatModel, UnitReport) {
    let mut lib = Library::new();
    let errs = lib.set_document("units.mo", SRC);
    assert!(errs.is_empty(), "{errs:?}");
    let flat = lib.flatten(model).unwrap();
    let r = units::check(&flat);
    (flat, r)
}

fn inferred<'a>(r: &'a UnitReport, name: &str) -> Option<&'a str> {
    r.inferred
        .iter()
        .find(|i| i.name == name)
        .map(|i| i.unit.as_str())
}

#[track_caller]
fn close(a: f64, b: f64) {
    assert!((a - b).abs() <= 1e-9 * (1.0 + b.abs()), "{a} vs {b}");
}

#[test]
fn parses_unit_strings() {
    let n = parse_unit("N").unwrap();
    let nm = parse_unit("N.m").unwrap();
    let j = parse_unit("J").unwrap();
    assert_eq!(nm.dim, j.dim);
    assert_eq!(parse_unit("kg.m/s2").unwrap().dim, n.dim);
    assert_eq!(parse_unit("kg.m.s-2").unwrap().dim, n.dim);
    assert_eq!(
        parse_unit("W/(m2.K)").unwrap().dim,
        parse_unit("kg/(s3.K)").unwrap().dim
    );
    // lenient: J/kg.K reads as J/(kg.K)
    assert_eq!(
        parse_unit("J/kg.K").unwrap().dim,
        parse_unit("J/(kg.K)").unwrap().dim
    );
    assert!(parse_unit("1").unwrap().dim.is_one());
    assert!(
        parse_unit("rad").unwrap().dim.is_one(),
        "angles are dimensionless"
    );
    assert!(
        parse_unit("rad/s")
            .unwrap()
            .dim
            .same(parse_unit("Hz").unwrap().dim)
    );
    assert_eq!(
        parse_unit("1/s").unwrap().dim,
        parse_unit("s-1").unwrap().dim
    );
    close(parse_unit("mm2").unwrap().scale, 1e-6);
    close(parse_unit("km/h").unwrap().scale, 1.0 / 3.6);
    close(parse_unit("kg").unwrap().scale, 1.0);
    close(parse_unit("g").unwrap().scale, 1e-3);
    close(parse_unit("degC").unwrap().offset, 273.15);
    close(parse_unit("mol/(m3.s)").unwrap().scale, 1.0);
    assert!(parse_unit("").is_err());
    assert!(parse_unit("furlong").is_err());
    assert!(parse_unit("m/(s").is_err());
    assert_eq!(units::format_dim(nm.dim), "J");
    assert_eq!(units::format_dim(parse_unit("kg.m2/s3").unwrap().dim), "W");
    assert_eq!(units::format_dim(parse_unit("m/s2").unwrap().dim), "m/s2");
}

#[test]
fn converts_display_units() {
    let (s, o) = conversion("rad/s", "rpm").unwrap();
    close(s, 60.0 / (2.0 * std::f64::consts::PI));
    close(o, 0.0);
    let (s, o) = conversion("K", "degC").unwrap();
    close(300.0 * s + o, 26.85);
    let (s, o) = conversion("K", "degF").unwrap();
    close(373.15 * s + o, 212.0);
    let (s, o) = conversion("degC", "degF").unwrap();
    close(100.0 * s + o, 212.0);
    close(conversion("Pa", "psi").unwrap().0 * 6894.757293168, 1.0);
    close(conversion("Pa", "bar").unwrap().0, 1e-5);
    close(conversion("m/s", "mph").unwrap().0 * 0.44704, 1.0);
    close(conversion("m/s", "km/h").unwrap().0, 3.6);
    close(conversion("N", "lbf").unwrap().0 * 4.4482216152605, 1.0);
    close(conversion("W", "hp").unwrap().0 * 745.69987158227, 1.0);
    close(conversion("m", "ft").unwrap().0, 1.0 / 0.3048);
    close(conversion("m", "in").unwrap().0, 1.0 / 0.0254);
    close(
        conversion("N.m", "lbf.ft").unwrap().0,
        1.0 / (4.4482216152605 * 0.3048),
    );
    // the angle exponent keeps rad/s and Hz apart for display
    assert!(conversion("rad/s", "Hz").is_none());
    assert!(conversion("m", "s").is_none());

    let c = display_choices("rad", "deg").unwrap();
    assert_eq!(c.si_default, "deg");
    let c = display_choices("Pa", "").unwrap();
    assert_eq!(c.si_default, "bar");
    assert_eq!(c.imperial_default, "psi");
    assert!(c.options.iter().any(|o| o.unit == "psi" && o.imperial));
    let c = display_choices("m/s", "").unwrap();
    assert_eq!(c.si_default, "m/s");
    assert_eq!(c.imperial_default, "mph");
    let c = display_choices("K", "").unwrap();
    assert_eq!(c.imperial_default, "degF");
    // a displayUnit that does not convert is ignored
    let c = display_choices("m", "s").unwrap();
    assert_eq!(c.si_default, "m");
    assert!(display_choices("", "").is_none());
}

#[test]
fn consistent_model_and_inference() {
    let (_, r) = report("U.Consistent");
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
    assert_eq!(inferred(&r, "a"), Some("m/s2"));
    assert_eq!(inferred(&r, "e"), Some("J"));
    assert_eq!(inferred(&r, "k"), Some("N/m"));
}

#[test]
fn reports_mismatches_with_equation_and_units() {
    let (_, r) = report("U.Inconsistent");
    assert_eq!(r.issues.len(), 2, "{:#?}", r.issues);
    let m = &r.issues[0];
    assert!(m.origin.contains("equation"), "{m:?}");
    assert!(m.equation.contains("der(x)"), "{m:?}");
    let units = [m.left_unit.as_str(), m.right_unit.as_str()];
    assert!(units.contains(&"m/s"), "{m:?}");
    assert!(m.message.contains("unit mismatch"));
    let m = &r.issues[1];
    assert!(m.equation.contains("v = x"), "{m:?}");
    assert!(
        [m.left_unit.as_str(), m.right_unit.as_str()].contains(&"m"),
        "{m:?}"
    );
    assert!(
        r.warnings(1).len() == 2,
        "a count line follows the shown issues"
    );
}

#[test]
fn transcendental_argument_must_be_dimensionless() {
    let (_, r) = report("U.Transcendental");
    assert_eq!(r.issues.len(), 1, "{:#?}", r.issues);
    assert!(
        r.issues[0].message.contains("argument of sin"),
        "{:?}",
        r.issues[0]
    );
    assert_eq!(inferred(&r, "y"), Some("1"));
}

#[test]
fn rotation_der_and_power() {
    let (_, r) = report("U.Rotation");
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
}

#[test]
fn unit_one_parameters_and_if_equations_are_polymorphic() {
    let (_, r) = report("U.Gain");
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
    let (_, r) = report("U.IfEquation");
    assert!(r.issues.is_empty(), "{:#?}", r.issues);
    assert_eq!(
        inferred(&r, "s"),
        None,
        "no inference inside if-equation branches"
    );
}

#[test]
fn inlined_functions_carry_units() {
    let (_, r) = report("U.FunctionCall");
    assert_eq!(r.issues.len(), 1, "{:#?}", r.issues);
    let i = &r.issues[0];
    assert!(
        [i.left_unit.as_str(), i.right_unit.as_str()].contains(&"N"),
        "{i:?}"
    );
    assert!(
        [i.left_unit.as_str(), i.right_unit.as_str()].contains(&"J"),
        "{i:?}"
    );
    assert_eq!(inferred(&r, "e"), Some("J"));
}

#[test]
fn parameter_bindings() {
    let (_, r) = report("U.Bindings");
    assert_eq!(r.issues.len(), 1, "{:#?}", r.issues);
    assert!(
        r.issues[0].origin.contains("binding of T"),
        "{:?}",
        r.issues[0]
    );
    assert_eq!(inferred(&r, "g"), Some("m/s2"));
    assert_eq!(inferred(&r, "y"), Some("m/s"));
}
