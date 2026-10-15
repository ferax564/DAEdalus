//! Parameter data import (CSV and SSV) to modifiers and records, unit conversion, and
//! the source edits it drives (applying a modifier, the metadata annotation).

use daedalus_core::frontend::{Library, inspect};
use daedalus_core::params::{self, ModTree, ParamValue};

const MODEL: &str = r#"package P
  type Length = Real(final unit="m");
  type Pressure = Real(final unit="Pa");
  model Valve
    parameter Length d = 0.01 "Diameter";
    parameter Pressure dp_nominal = 1e5;
    parameter Real zeta = 2;
    parameter Boolean open = true;
  end Valve;
  model System "Two valves"
    Valve v1(zeta=3, open=false) "first";
    Valve v2;
    Real x;
  equation
    x = v1.d + v2.d;
    annotation(experiment(StopTime=1));
  end System;
end P;
"#;

fn lib() -> Library {
    let mut l = Library::new();
    assert!(l.set_document("p.mo", MODEL).is_empty());
    l
}

#[test]
fn csv_formats() {
    let csv = "# valve data\nname;value;unit\nd;12;mm\ndp_nominal;1.5;bar\nzeta;2.5;\nopen;FALSE\nlabel;\"a; b\"\ntable;{1, 2; 3}\n";
    let p = params::parse(csv, "v.csv").unwrap();
    assert_eq!(p.len(), 6);
    assert_eq!(p[0], ParamValue { name: "d".into(), value: "12".into(), unit: Some("mm".into()) });
    assert_eq!(p[3].value, "false");
    assert_eq!(p[4].value, "\"a; b\"");
    assert_eq!(p[5].value, "{1, 2; 3}");
    // comma and tab separated, no header
    let p = params::parse_csv("v1.zeta,4\nv2.d,0.02,m\n").unwrap();
    assert_eq!(p[1].name, "v2.d");
    let p = params::parse_csv("a\t1\tkg\n").unwrap();
    assert_eq!(p[0].unit.as_deref(), Some("kg"));
    assert!(params::parse_csv("lonely\n").is_err());
}

#[test]
fn ssv_values() {
    let ssv = r#"<?xml version="1.0" encoding="UTF-8"?>
<ssv:ParameterSet xmlns:ssv="http://ssp-standard.org/SSP1/SystemStructureParameterValues" version="1.0" name="valves">
  <ssv:Parameters>
    <!-- a comment <ssv:Parameter name="ignored"> -->
    <ssv:Parameter name="v1.d"><ssv:Real value="15" unit="mm"/></ssv:Parameter>
    <ssv:Parameter name="v1.open"><ssv:Boolean value="true"/></ssv:Parameter>
    <ssv:Parameter name="v2.zeta"><ssv:Integer value="7"/></ssv:Parameter>
    <ssv:Parameter name="note"><ssv:String value="say &quot;hi&quot;"/></ssv:Parameter>
  </ssv:Parameters>
</ssv:ParameterSet>"#;
    let p = params::parse(ssv, "x.ssv").unwrap();
    assert_eq!(p.len(), 4);
    assert_eq!(p[0].unit.as_deref(), Some("mm"));
    assert_eq!(p[1].value, "true");
    assert_eq!(p[3].value, "\"say \\\"hi\\\"\"");
    assert!(params::parse_ssv("<a/>").is_err());
}

#[test]
fn modifier_and_record() {
    let p = params::parse_csv("v1.d,0.02\nv1.zeta,3\nv2.d,0.03\nk,5\n").unwrap();
    assert_eq!(params::modifier("sys", &p), "sys(v1(d = 0.02, zeta = 3), v2(d = 0.03), k = 5)");
    let r = params::record("Data", &params::parse_csv("a,1.5,m\nn,3\ns,hello\nv1.d,2\n").unwrap());
    assert!(r.contains("parameter Real a(unit=\"m\") = 1.5;"), "{r}");
    assert!(r.contains("parameter Integer n = 3;"));
    assert!(r.contains("parameter String s = \"hello\";"));
    assert!(r.contains("//   v1.d = 2"));
    assert!(r.ends_with("end Data;\n"));
}

#[test]
fn check_converts_units_and_reports_unknown_names() {
    let l = lib();
    let p = params::parse_csv("d,12,mm\ndp_nominal,1.5,bar\nzeta,4\nbogus,1\n").unwrap();
    let c = params::check(&l, "P.System", Some("v1"), &p).unwrap();
    assert_eq!(c.params.len(), 3, "{:?}", c.messages);
    assert_eq!(c.params[0].value, "0.012");
    assert_eq!(c.params[1].value, "150000");
    assert!(c.messages.iter().any(|m| m.contains("bogus")));
    // dotted names through components of the model
    let c = params::check(&l, "P.System", None, &params::parse_csv("v2.d,10,cm\n").unwrap()).unwrap();
    assert_eq!(c.params[0].value, "0.1");
    assert!(params::check(&l, "P.System", Some("nope"), &p).is_err());
    assert_eq!(params::convert(20.0, "degC", "K"), Some(293.15));
    assert_eq!(params::convert(1.0, "m", "kg"), None);
}

#[test]
fn apply_modifier_merges_into_the_declaration() {
    let l = lib();
    let p = params::parse_csv("zeta,5\nd,0.02\n").unwrap();
    let (uri, text) = daedalus_core::edit::apply_modifier(&l, "P.System", "v1", &ModTree::from_params(&p)).unwrap();
    assert_eq!(uri, "p.mo");
    assert!(text.contains("Valve v1(zeta = 5, open = false, d = 0.02) \"first\";"), "{text}");
    let (_, text) = daedalus_core::edit::apply_modifier(&l, "P.System", "v2", &ModTree::from_params(&p)).unwrap();
    assert!(text.contains("Valve v2(zeta = 5, d = 0.02);"), "{text}");
    // the edited text still parses, and the new values are there
    let mut l2 = Library::new();
    assert!(l2.set_document("p.mo", &text).is_empty());
    let info = inspect::class_info(&l2, "P.System").unwrap();
    let v2 = info.components.iter().find(|c| c.name == "v2").unwrap();
    assert_eq!(v2.modifiers.len(), 2);
    assert!(daedalus_core::edit::apply_modifier(&l, "P.System", "x9", &ModTree::default()).is_err());
}

#[test]
fn metadata_annotation_is_read_and_edited() {
    let l = lib();
    let kv = vec![("owner".to_string(), "Ada".to_string()), ("review".to_string(), "2026-10-01".to_string())];
    // a class with an annotation
    let (_, t1) = daedalus_core::edit::set_metadata(&l, "P.System", &kv).unwrap();
    let mut l1 = Library::new();
    assert!(l1.set_document("p.mo", &t1).is_empty(), "{t1}");
    let info = inspect::class_info(&l1, "P.System").unwrap();
    assert_eq!(info.metadata, kv);
    assert!(info.experiment.is_some(), "the experiment annotation is kept");
    // replace, then remove
    let kv2 = vec![("owner".to_string(), "Grace \"G\"".to_string())];
    let (_, t2) = daedalus_core::edit::set_metadata(&l1, "P.System", &kv2).unwrap();
    let mut l2 = Library::new();
    l2.set_document("p.mo", &t2);
    assert_eq!(inspect::class_info(&l2, "P.System").unwrap().metadata, kv2);
    let (_, t3) = daedalus_core::edit::set_metadata(&l2, "P.System", &[]).unwrap();
    assert!(!t3.contains("__Daedalus_metadata"));
    let mut l3 = Library::new();
    assert!(l3.set_document("p.mo", &t3).is_empty(), "{t3}");
    // a class without an annotation gets one
    let (_, t4) = daedalus_core::edit::set_metadata(&l, "P.Valve", &kv).unwrap();
    let mut l4 = Library::new();
    assert!(l4.set_document("p.mo", &t4).is_empty(), "{t4}");
    assert_eq!(inspect::class_info(&l4, "P.Valve").unwrap().metadata, kv);
    assert!(daedalus_core::edit::set_metadata(&l, "P.Valve", &[("bad key".into(), "x".into())]).is_err());
}

#[test]
fn documentation_revisions_version_and_uses() {
    let src = r#"package Lib
  model M
    annotation(Documentation(info="<html><p>Info</p></html>", revisions="<html>r1</html>"));
  end M;
  annotation(version="2.1.0", uses(Modelica(version="4.0.0"), Other(version="1.0")));
end Lib;
"#;
    let mut l = Library::new();
    l.set_document("lib.mo", src);
    let m = inspect::class_info(&l, "Lib.M").unwrap();
    assert_eq!(m.documentation.as_deref(), Some("<html><p>Info</p></html>"));
    assert_eq!(m.revisions.as_deref(), Some("<html>r1</html>"));
    let p = inspect::class_info(&l, "Lib").unwrap();
    assert_eq!(p.version.as_deref(), Some("2.1.0"));
    assert_eq!(p.uses, vec![("Modelica".into(), "4.0.0".into()), ("Other".into(), "1.0".into())]);
}
