//! Source-preserving graphical edits (`frontend::edit`): every edit re-parses, touches only
//! the text it must, and the inverse edit gives back the original document byte for byte.

use daedalus_core::frontend::edit::{
    EditOp, apply_op, duplicate_class, edit_class, new_model, parameters,
};
use daedalus_core::frontend::{Library, inspect, parse_document};
use daedalus_core::sim::{SimOptions, simulate};
use std::path::Path;

fn lib_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/Daedalus")
}

fn library() -> Library {
    let mut lib = Library::new();
    lib.load_dir(&lib_dir()).expect("library parses");
    lib
}

fn apply(text: &str, class: &str, op: EditOp) -> String {
    let out = apply_op(text, class, &op, &[])
        .unwrap_or_else(|e| panic!("{op:?}: {e:#}"))
        .text;
    let errs = parse_document("t.mo", &out).errors;
    assert!(
        errs.is_empty(),
        "{op:?} gives a document that does not parse: {errs:?}\n{out}"
    );
    out
}

/// The single changed region between two texts: (common prefix, removed, inserted).
fn diff<'a>(a: &'a str, b: &'a str) -> (usize, &'a str, &'a str) {
    let pre = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
    let suf = a[pre..]
        .bytes()
        .rev()
        .zip(b[pre..].bytes().rev())
        .take_while(|(x, y)| x == y)
        .count();
    (pre, &a[pre..a.len() - suf], &b[pre..b.len() - suf])
}

fn mv(name: &str, dx: f64, dy: f64) -> EditOp {
    EditOp::MoveComponent {
        name: name.into(),
        dx,
        dy,
    }
}
fn set(name: &str, m: &str, v: Option<&str>) -> EditOp {
    EditOp::SetModifier {
        name: name.into(),
        modifier: m.into(),
        value: v.map(str::to_string),
    }
}

const RC: &str = r#"within Lib.Circuits;
model RC "RC circuit" // trailing comment
  /* a block comment with ; and end RC; inside */
  Modelica.Electrical.Analog.Basic.Resistor R1(R = 100 /* ohm */) "resistor"
    annotation (Placement(transformation(
          extent={{-10,-10},{10,10}},
          rotation=270,
          origin={20,0})));
  Modelica.Electrical.Analog.Basic.Capacitor C1(C=1e-3, v(start=1, fixed=true)) annotation (
    Placement(transformation(extent={{40,-10},{60,10}})));
  Ground g;

  // ports
  Pin a, b(v(start = 0)) "two pins";
equation
  connect(R1.n, C1.p)
    annotation (Line(points={{20,-10},{20,-20},{50,-20},{50,-10}}, color={0,0,255}));
  connect(C1.n, g.p); // no annotation
  R1.v = if a.v > 0 then 1 else 2;
  annotation (experiment(StopTime=1), Documentation(info="<html>connect(a,b); end RC;</html>"));
end RC;
"#;

#[test]
fn modifiers_keep_comments_and_layout() {
    let t = apply(RC, "Lib.Circuits.RC", set("R1", "R", Some("200")));
    assert!(t.contains("R1(R = 200 /* ohm */)"), "{t}");
    assert_eq!(t, RC.replace("R = 100", "R = 200"));

    let t = apply(RC, "Lib.Circuits.RC", set("C1", "v.start", Some("2")));
    assert!(t.contains("C1(C=1e-3, v(start=2, fixed=true))"));
    let t = apply(&t, "Lib.Circuits.RC", set("C1", "v.fixed", None));
    assert!(t.contains("C1(C=1e-3, v(start=2))"), "{t}");
    let t = apply(&t, "Lib.Circuits.RC", set("C1", "v.start", None));
    assert!(t.contains("C1(C=1e-3) annotation"), "{t}");
    let t = apply(&t, "Lib.Circuits.RC", set("C1", "C", None));
    assert!(t.contains("Capacitor C1 annotation"), "{t}");

    // new parentheses, then removed again: the original text
    let t = apply(RC, "Lib.Circuits.RC", set("g", "x", Some("1")));
    assert!(t.contains("Ground g(x=1);"));
    assert_eq!(apply(&t, "Lib.Circuits.RC", set("g", "x", None)), RC);

    // nested modifier created on a component of a multi-declaration
    let t = apply(RC, "Lib.Circuits.RC", set("a", "v.start", Some("3")));
    assert!(t.contains("Pin a(v(start=3)), b(v(start = 0))"), "{t}");
    let t = apply(RC, "Lib.Circuits.RC", set("b", "v.start", Some("5")));
    assert!(t.contains("b(v(start = 5))"), "{t}");
}

#[test]
fn delete_component_takes_its_connections_and_lines() {
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::DeleteComponent { name: "R1".into() },
    );
    assert!(!t.contains("Resistor"));
    assert!(!t.contains("connect(R1.n"));
    assert!(!t.contains("{20,-20}"));
    // the comment block and the other connect remain
    assert!(t.contains("/* a block comment with ; and end RC; inside */\n  Modelica.Electrical.Analog.Basic.Capacitor"));
    assert!(t.contains("equation\n  connect(C1.n, g.p); // no annotation"));
    // a declarator of a multi-declaration
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::DeleteComponent { name: "a".into() },
    );
    assert!(t.contains("  Pin b(v(start = 0)) \"two pins\";"), "{t}");
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::DeleteComponent { name: "b".into() },
    );
    // the description belongs to the last declarator
    assert!(t.contains("  Pin a;\n"), "{t}");
}

#[test]
fn placement_edits() {
    // with an origin only the origin moves
    let t = apply(RC, "Lib.Circuits.RC", mv("R1", 10.0, -4.0));
    assert_eq!(diff(RC, &t).1, "20,0");
    assert!(t.contains("origin={30,-4}"));
    assert_eq!(apply(&t, "Lib.Circuits.RC", mv("R1", -10.0, 4.0)), RC);
    // without one the extent moves
    let t = apply(RC, "Lib.Circuits.RC", mv("C1", 2.0, 2.0));
    assert!(t.contains("extent={{42,-8},{62,12}}"));
    // rotating about the icon centre adds an origin; four quarter turns come back
    let mut t = RC.to_string();
    for _ in 0..4 {
        t = apply(
            &t,
            "Lib.Circuits.RC",
            EditOp::RotateComponent {
                name: "C1".into(),
                angle: 90.0,
            },
        );
    }
    assert!(
        t.contains("transformation(extent={{-10,-10},{10,10}}, origin={50,0}, rotation=0)"),
        "{t}"
    );
    // flipping twice is the identity on the simple form
    let f = |t: &str, h: bool| {
        apply(
            t,
            "Lib.Circuits.RC",
            EditOp::FlipComponent {
                name: "C1".into(),
                horizontal: h,
            },
        )
    };
    let t1 = f(RC, true);
    assert!(t1.contains("extent={{60,-10},{40,10}}"), "{t1}");
    assert_eq!(f(&t1, true), RC);
    let t2 = f(RC, false);
    assert!(t2.contains("extent={{40,10},{60,-10}}"));
    // no placement yet: one is added after the description
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::SetPlacement {
            name: "g".into(),
            origin: None,
            extent: [[0.0, -40.0], [20.0, -20.0]],
            rotation: None,
        },
    );
    assert!(
        t.contains("Ground g annotation (Placement(transformation(extent={{0,-40},{20,-20}})));")
    );
}

#[test]
fn connections() {
    let add = EditOp::AddConnection {
        from: "R1.p".into(),
        to: "g.p".into(),
        points: vec![[20.0, 10.0], [20.0, 30.0], [-40.5, 30.0]],
        color: Some([0, 0, 255]),
    };
    let t = apply(RC, "Lib.Circuits.RC", add.clone());
    // after the last connect equation, same indentation
    assert!(t.contains("connect(C1.n, g.p); // no annotation\n  connect(R1.p, g.p) annotation (Line(points={{20,10},{20,30},{-40.5,30}}, color={0,0,255}));\n  R1.v"), "{t}");
    assert!(
        apply_op(&t, "Lib.Circuits.RC", &add, &[]).is_err(),
        "duplicate connection"
    );
    let back = apply(
        &t,
        "Lib.Circuits.RC",
        EditOp::DeleteConnection {
            from: "g.p".into(),
            to: "R1.p".into(),
        },
    );
    assert_eq!(back, RC);

    // rewrite the points of an annotated and an unannotated connection
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::SetConnectionPoints {
            from: "C1.p".into(),
            to: "R1.n".into(),
            points: vec![[1.0, 1.0], [2.0, 2.0]],
        },
    );
    assert!(
        t.contains("Line(points={{2,2},{1,1}}, color={0,0,255})"),
        "{t}"
    );
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::SetConnectionPoints {
            from: "C1.n".into(),
            to: "g.p".into(),
            points: vec![[1.0, 1.0], [2.0, 2.0]],
        },
    );
    assert!(t.contains("connect(C1.n, g.p) annotation (Line(points={{1,1},{2,2}}, color={0,0,255})); // no annotation"));

    // a class without equations gets an equation section before its annotation
    let src = "model M\n  A a;\n  B b;\n  annotation (Icon());\nend M;\n";
    let t = apply(
        src,
        "M",
        EditOp::AddConnection {
            from: "a.y".into(),
            to: "b.u".into(),
            points: vec![],
            color: None,
        },
    );
    assert_eq!(
        t,
        "model M\n  A a;\n  B b;\nequation\n  connect(a.y, b.u);\n  annotation (Icon());\nend M;\n"
    );
    let t = apply(
        &t,
        "M",
        EditOp::AddConnection {
            from: "b.y".into(),
            to: "a.u".into(),
            points: vec![],
            color: None,
        },
    );
    assert!(t.contains("  connect(a.y, b.u);\n  connect(b.y, a.u);\n"));
}

#[test]
fn add_and_rename_components() {
    let add = EditOp::AddComponent {
        type_name: "Modelica.Electrical.Analog.Basic.Inductor".into(),
        name: None,
        extent: [[-60.0, -10.0], [-40.0, 10.0]],
        rotation: None,
        modifiers: vec![("L".into(), "0.1".into())],
    };
    let a = apply_op(RC, "Lib.Circuits.RC", &add, &["inductor1".to_string()]).unwrap();
    assert_eq!(a.name.as_deref(), Some("inductor2"));
    assert!(a.text.contains("  Pin a, b(v(start = 0)) \"two pins\";\n  Modelica.Electrical.Analog.Basic.Inductor inductor2(L=0.1) annotation (Placement(transformation(extent={{-60,-10},{-40,10}})));\nequation"), "{}", a.text);
    let back = apply(
        &a.text,
        "Lib.Circuits.RC",
        EditOp::DeleteComponent {
            name: "inductor2".into(),
        },
    );
    assert_eq!(back, RC);

    // rotated placement is written about the icon centre
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::AddComponent {
            type_name: "X".into(),
            name: Some("x".into()),
            extent: [[0.0, 0.0], [20.0, 20.0]],
            rotation: Some(90.0),
            modifiers: vec![],
        },
    );
    assert!(t.contains("X x annotation (Placement(transformation(origin={10,10}, extent={{-10,-10},{10,10}}, rotation=90)));"));
    assert!(
        apply_op(
            RC,
            "Lib.Circuits.RC",
            &EditOp::AddComponent {
                type_name: "X".into(),
                name: Some("g".into()),
                extent: [[0.0, 0.0], [1.0, 1.0]],
                rotation: None,
                modifiers: vec![]
            },
            &[]
        )
        .is_err()
    );

    // rename: declaration and every equation, not the comment or the documentation string
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::RenameComponent {
            name: "a".into(),
            new_name: "pa".into(),
        },
    );
    assert!(t.contains("Pin pa, b"));
    assert!(t.contains("if pa.v > 0"));
    assert!(t.contains("connect(a,b); end RC;</html>"));
    let t = apply(
        RC,
        "Lib.Circuits.RC",
        EditOp::RenameComponent {
            name: "C1".into(),
            new_name: "Cap".into(),
        },
    );
    assert!(
        t.contains("connect(R1.n, Cap.p)")
            && t.contains("connect(Cap.n, g.p)")
            && t.contains("Capacitor Cap(C=1e-3")
    );
    assert!(
        apply_op(
            RC,
            "Lib.Circuits.RC",
            &EditOp::RenameComponent {
                name: "a".into(),
                new_name: "g".into()
            },
            &[]
        )
        .is_err()
    );
}

#[test]
fn unusual_formatting_one_line_and_tabs() {
    let src = "package P model M A a; B b(k=1); equation connect(a.y,b.u); end M; end P;";
    let t = apply(
        src,
        "P.M",
        EditOp::AddComponent {
            type_name: "C".into(),
            name: None,
            extent: [[0.0, 0.0], [20.0, 20.0]],
            rotation: None,
            modifiers: vec![],
        },
    );
    assert!(t.contains("B b(k=1); C c1 annotation"), "{t}");
    let t = apply(&t, "P.M", EditOp::DeleteComponent { name: "a".into() });
    assert!(!t.contains("connect(a.y"));
    assert!(t.contains("model M B b(k=1);"), "{t}");

    let src = "model T\n\tA a;\nequation\n\tconnect(a.y, a.u);\nend T;\n";
    let t = apply(
        src,
        "T",
        EditOp::AddComponent {
            type_name: "B".into(),
            name: Some("b".into()),
            extent: [[0.0, 0.0], [20.0, 20.0]],
            rotation: None,
            modifiers: vec![],
        },
    );
    assert!(t.contains("\tA a;\n\tB b annotation"), "{t}");
    let t = apply(
        &t,
        "T",
        EditOp::AddConnection {
            from: "b.y".into(),
            to: "a.u".into(),
            points: vec![],
            color: None,
        },
    );
    assert!(
        t.contains("\tconnect(a.y, a.u);\n\tconnect(b.y, a.u);"),
        "{t}"
    );

    // nested classes, a protected section and an initial equation
    let src = "model O\n  model I\n    Real x;\n  equation\n    x = 1;\n  end I;\n  I i;\nprotected\n  Real z;\ninitial equation\n  z = 0;\nequation\n  der(z) = 1;\nend O;\n";
    let t = apply(
        src,
        "O",
        EditOp::AddComponent {
            type_name: "I".into(),
            name: None,
            extent: [[0.0, 0.0], [20.0, 20.0]],
            rotation: None,
            modifiers: vec![],
        },
    );
    assert!(t.contains("  I i;\n  I i1 annotation"), "{t}");
    let t = apply(
        src,
        "O",
        EditOp::AddConnection {
            from: "i.a".into(),
            to: "i.b".into(),
            points: vec![],
            color: None,
        },
    );
    assert!(
        t.contains("  der(z) = 1;\n  connect(i.a, i.b);\nend O;"),
        "{t}"
    );
    // the inner class is editable by its path
    let t = apply(
        src,
        "O.I",
        EditOp::RenameComponent {
            name: "x".into(),
            new_name: "y".into(),
        },
    );
    assert!(t.contains("Real y;\n  equation\n    y = 1;"), "{t}");
}

/// Every model of the Daedalus examples: inverse edits restore the file exactly, and
/// each edit changes one region of it.
#[test]
fn round_trips_on_library_models() {
    let lib = library();
    let mut checked = 0;
    for doc in &lib.docs {
        if !doc.uri.contains("Examples") {
            continue;
        }
        let text: &str = &doc.text;
        for path in model_paths(&lib, &doc.uri) {
            let info = inspect::class_info(&lib, &path).unwrap();
            let own: Vec<_> = info
                .components
                .iter()
                .filter(|c| c.declared_in == path)
                .collect();
            let Some(first) = own
                .iter()
                .find(|c| c.type_kind == "model" || c.type_kind == "block")
            else {
                continue;
            };
            // add + delete
            let add = EditOp::AddComponent {
                type_name: "Daedalus.Blocks.Sources.Constant".into(),
                name: None,
                extent: [[-90.0, 70.0], [-70.0, 90.0]],
                rotation: None,
                modifiers: vec![("k".into(), "2".into())],
            };
            let a = apply_op(text, &path, &add, &[]).unwrap();
            assert!(parse_document("x.mo", &a.text).errors.is_empty(), "{path}");
            let (_, removed, inserted) = diff(text, &a.text);
            assert!(
                removed.is_empty() && inserted.contains("Constant"),
                "{path}: {inserted}"
            );
            let back = apply(
                &a.text,
                &path,
                EditOp::DeleteComponent {
                    name: a.name.clone().unwrap(),
                },
            );
            assert_eq!(back, text, "{path}: add/delete");
            // a modifier the component does not have
            let t = apply(text, &path, set(&first.name, "zzTest", Some("1.5")));
            assert_eq!(
                apply(&t, &path, set(&first.name, "zzTest", None)),
                text,
                "{path}: modifier"
            );
            // a connection added and deleted
            let t = apply(
                text,
                &path,
                EditOp::AddConnection {
                    from: format!("{}.zz", first.name),
                    to: "zz.p".into(),
                    points: vec![[0.0, 0.0], [10.0, 0.0]],
                    color: None,
                },
            );
            let back = apply(
                &t,
                &path,
                EditOp::DeleteConnection {
                    from: "zz.p".into(),
                    to: format!("{}.zz", first.name),
                },
            );
            if back != text {
                let (at, a, b) = diff(text, &back);
                panic!(
                    "{path}: connection at {at}: {a:?} -> {b:?}\n{}",
                    &t[at.saturating_sub(300)..(at + 300).min(t.len())]
                );
            }
            // moving there and back
            if first.placement.is_some() {
                let t = apply(text, &path, mv(&first.name, 6.0, -8.0));
                assert_eq!(
                    apply(&t, &path, mv(&first.name, -6.0, 8.0)),
                    text,
                    "{path}: move"
                );
            }
            // rename there and back
            let t = apply(
                text,
                &path,
                EditOp::RenameComponent {
                    name: first.name.clone(),
                    new_name: "zzRenamed".into(),
                },
            );
            let back = apply(
                &t,
                &path,
                EditOp::RenameComponent {
                    name: "zzRenamed".into(),
                    new_name: first.name.clone(),
                },
            );
            assert_eq!(back, text, "{path}: rename");
            // deleting each component leaves a document that parses
            for c in &own {
                apply(
                    text,
                    &path,
                    EditOp::DeleteComponent {
                        name: c.name.clone(),
                    },
                );
            }
            checked += 1;
        }
    }
    assert!(checked >= 12, "only {checked} models checked");
}

fn model_paths(lib: &Library, uri: &str) -> Vec<String> {
    fn walk(e: &inspect::ClassEntry, out: &mut Vec<String>) {
        if e.kind == "model" {
            out.push(e.path.clone());
        }
        for c in &e.children {
            walk(c, out);
        }
    }
    let mut all = Vec::new();
    for e in inspect::class_tree(lib) {
        walk(&e, &mut all);
    }
    all.into_iter()
        .filter(|p| {
            inspect::class_info(lib, p)
                .and_then(|i| i.source)
                .is_some_and(|s| s.uri == uri)
        })
        .collect()
}

/// Build Constant -> FirstOrder from scratch as the diagram editor does, and check the
/// simulated response against y = k (1 - exp(-t/T)).
#[test]
fn build_and_simulate_a_model_from_scratch() {
    let mut lib = library();
    let (text, path) = new_model(None, "MyModels", "Lag").unwrap();
    assert_eq!(path, "MyModels.Lag");
    assert!(lib.set_document("user/MyModels.mo", &text).is_empty());
    let ops = vec![
        EditOp::AddComponent {
            type_name: "Daedalus.Blocks.Sources.Constant".into(),
            name: None,
            extent: [[-60.0, -10.0], [-40.0, 10.0]],
            rotation: None,
            modifiers: vec![],
        },
        EditOp::AddComponent {
            type_name: "Daedalus.Blocks.Continuous.FirstOrder".into(),
            name: None,
            extent: [[0.0, -10.0], [20.0, 10.0]],
            rotation: None,
            modifiers: vec![],
        },
    ];
    let r = edit_class(&lib, &path, &ops).unwrap();
    assert_eq!(r.added, ["constant1", "firstOrder1"]);
    lib.set_document(&r.uri, &r.text);
    let ops = vec![
        EditOp::AddConnection {
            from: "constant1.y".into(),
            to: "firstOrder1.u".into(),
            points: vec![[-39.0, 0.0], [-2.0, 0.0]],
            color: Some([0, 0, 127]),
        },
        set("constant1", "k", Some("2")),
        set("firstOrder1", "T", Some("0.5")),
    ];
    let r = edit_class(&lib, &path, &ops).unwrap();
    lib.set_document(&r.uri, &r.text);
    let info = inspect::class_info(&lib, &path).unwrap();
    assert_eq!(info.connections.len(), 1);
    assert_eq!(
        info.connections[0].line.as_ref().unwrap()["color"],
        serde_json::json!([0, 0, 127])
    );

    let m = daedalus_core::translate(&lib, &path, &Default::default()).unwrap();
    let opts = SimOptions::from_model(&m);
    let (res, _) = simulate(&m, opts).map_err(|e| e.message).unwrap();
    let y = res.values("firstOrder1.y").unwrap();
    let mut worst: f64 = 0.0;
    for (t, y) in res.time.iter().zip(&y) {
        worst = worst.max((y - 2.0 * (1.0 - (-t / 0.5).exp())).abs());
    }
    assert!(worst < 1e-3, "max error {worst}");
    assert!((res.time.last().unwrap() - 1.0).abs() < 1e-12);

    // the parameter dialog's view of the block
    let ps = parameters(&lib, "Daedalus.Blocks.Continuous.FirstOrder");
    let t = ps.iter().find(|p| p.name == "T").unwrap();
    assert_eq!(t.unit, "s");
    assert_eq!(t.default.as_deref(), Some("1"));
    assert_eq!(t.description, "Time constant");
}

/// A library model copied into MyModels behaves exactly like the original.
#[test]
fn duplicated_class_simulates_like_the_original() {
    let mut lib = library();
    let (text, path) = duplicate_class(
        &lib,
        "Daedalus.Examples.TorqueConverter.StallTest",
        None,
        "MyModels",
    )
    .unwrap();
    assert_eq!(path, "MyModels.StallTest");
    // names written relative to the library are qualified
    assert!(
        text.contains("Daedalus.Mechanics.Rotational.Components.TorqueConverter converter;"),
        "{text}"
    );
    assert!(text.contains("extends Daedalus.Icons.Example;"));
    assert!(lib.set_document("user/MyModels.mo", &text).is_empty());
    // a second copy gets a new name
    let (text2, path2) = duplicate_class(
        &lib,
        "Daedalus.Examples.TorqueConverter.StallTest",
        Some(&text),
        "MyModels",
    )
    .unwrap();
    assert_eq!(path2, "MyModels.StallTest2");
    assert!(text2.contains("end StallTest2;"));

    let run = |lib: &Library, p: &str| {
        let m = daedalus_core::translate(lib, p, &Default::default()).unwrap();
        let (r, _) = simulate(&m, SimOptions::from_model(&m))
            .map_err(|e| e.message)
            .unwrap();
        r.final_value("rpm_engine").unwrap()
    };
    let a = run(&lib, "Daedalus.Examples.TorqueConverter.StallTest");
    let b = run(&lib, &path);
    assert!((a - b).abs() < 1e-9, "{a} vs {b}");
}

/// The same round trips over the Modelica Standard Library, whose files use every layout
/// Dymola and OMEdit write. Runs when `DAEDALUS_MSL_JSON` names the unpacked library
/// bundle of the workbench (`gzip -dc web/public/msl/msl-*.json.gz > msl.json`).
#[test]
fn round_trips_on_msl() {
    let Ok(path) = std::env::var("DAEDALUS_MSL_JSON") else {
        eprintln!("DAEDALUS_MSL_JSON not set: skipped");
        return;
    };
    let bundle: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let files = bundle["files"].as_array().unwrap();
    let (mut classes, mut failures) = (0, Vec::new());
    for f in files {
        let rel = f[0].as_str().unwrap();
        if !rel.ends_with(".mo") || !(rel.contains("Examples") || rel.contains("Electrical/Analog"))
        {
            continue;
        }
        let text = f[1].as_str().unwrap();
        let doc = parse_document(rel, text);
        if !doc.errors.is_empty() {
            continue;
        }
        let within = doc
            .ast
            .within
            .as_ref()
            .map(|w| w.to_string())
            .unwrap_or_default();
        let mut stack: Vec<(String, &rumoca_ir_ast::ClassDef)> = doc
            .ast
            .classes
            .iter()
            .map(|(n, c)| {
                (
                    if within.is_empty() {
                        n.clone()
                    } else {
                        format!("{within}.{n}")
                    },
                    c,
                )
            })
            .collect();
        while let Some((cpath, c)) = stack.pop() {
            stack.extend(c.classes.iter().map(|(n, k)| (format!("{cpath}.{n}"), k)));
            let Some((name, comp)) = c.components.iter().find(|(_, k)| !k.annotation.is_empty())
            else {
                continue;
            };
            classes += 1;
            let mut check = |what: &str, ops: Vec<EditOp>| {
                let mut t = text.to_string();
                for op in &ops {
                    match apply_op(&t, &cpath, op, &[]) {
                        Ok(a) => t = a.text,
                        Err(e) => {
                            failures.push(format!("{cpath} {what}: {e:#}"));
                            return;
                        }
                    }
                    if !parse_document(rel, &t).errors.is_empty() {
                        failures.push(format!("{cpath} {what}: {op:?} does not parse"));
                        return;
                    }
                }
                if t != text {
                    let (at, a, b) = diff(text, &t);
                    failures.push(format!("{cpath} {what}: changed at {at}: {a:?} -> {b:?}"));
                }
            };
            check(
                "add/delete",
                vec![
                    EditOp::AddComponent {
                        type_name: "Modelica.Blocks.Sources.Constant".into(),
                        name: Some("zzNew".into()),
                        extent: [[0.0, 0.0], [20.0, 20.0]],
                        rotation: None,
                        modifiers: vec![],
                    },
                    EditOp::DeleteComponent {
                        name: "zzNew".into(),
                    },
                ],
            );
            check(
                "modifier",
                vec![set(name, "zzMod", Some("1")), set(name, "zzMod", None)],
            );
            use rumoca_core::ClassType as CT;
            if matches!(c.class_type, CT::Model | CT::Block | CT::Class) {
                check(
                    "connection",
                    vec![
                        EditOp::AddConnection {
                            from: format!("{name}.zz"),
                            to: "zz.p".into(),
                            points: vec![[0.0, 0.0], [5.0, 0.0]],
                            color: None,
                        },
                        EditOp::DeleteConnection {
                            from: format!("{name}.zz"),
                            to: "zz.p".into(),
                        },
                    ],
                );
            }
            check(
                "rename",
                vec![
                    EditOp::RenameComponent {
                        name: name.clone(),
                        new_name: "zzRenamed".into(),
                    },
                    EditOp::RenameComponent {
                        name: "zzRenamed".into(),
                        new_name: name.clone(),
                    },
                ],
            );
            if format!("{:?}", comp.annotation).contains("Placement") {
                check("move", vec![mv(name, 4.0, -2.0), mv(name, -4.0, 2.0)]);
            }
        }
    }
    eprintln!("{classes} MSL classes edited, {} failures", failures.len());
    for f in failures.iter().take(40) {
        eprintln!("{f}");
    }
    assert!(classes > 300, "{classes}");
    assert!(failures.is_empty(), "{} failures", failures.len());
}
