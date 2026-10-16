//! Editor intelligence: definition, references, rename, completion, hover, outline and
//! diagnostics over a small library, plus a sweep of the shipped library (no false
//! diagnostics).

use daedalus_core::frontend::Library;
use daedalus_core::ide::{Ide, byte_to_utf16, utf16_to_byte};

const LIB: &str = r#"package Lib "A test library"
  type Voltage = Real(unit = "V", quantity = "ElectricPotential") "An electric potential";
  type Mode = enumeration(off "switched off", on "switched on");
  connector Pin "Electrical pin"
    Voltage v "Potential at the pin";
    flow Real i(unit = "A") "Current into the pin";
  end Pin;
  partial model TwoPin "Component with two pins"
    Pin p "Positive pin";
    Pin n "Negative pin";
    Voltage u "Voltage drop";
  equation
    u = p.v - n.v;
  end TwoPin;
  model Resistor "Ideal resistor"
    extends TwoPin;
    parameter Real R(start = 1) = 10 "Resistance";
  equation
    u = R * p.i;
    p.i + n.i = 0;
  end Resistor;
  package Constants
    constant Real pi = 3.14159 "Circle constant";
  end Constants;
  model Circuit "Two resistors"
    import Lib.Constants.pi;
    Resistor r1(R = 2 * pi);
    Resistor r2(R = r1.R, u(start = 0));
    parameter Mode m = Mode.on;
    Real x[3];
  equation
    connect(r1.n, r2.p);
    r1.p.v = 1;
    r2.n.v = 0;
    for k in 1:3 loop
      x[k] = k * r1.u;
    end for;
  end Circuit;
end Lib;
"#;

fn setup() -> (Library, Ide) {
    let mut lib = Library::new();
    let mut ide = Ide::new();
    let d = ide.update(&mut lib, "Lib.mo", LIB);
    assert!(d.is_empty(), "unexpected diagnostics: {d:?}");
    (lib, ide)
}

/// Byte offset of the `nth` occurrence of `needle` (plus `delta`).
fn at(needle: &str, nth: usize, delta: usize) -> usize {
    LIB.match_indices(needle).nth(nth).expect(needle).0 + delta
}

fn line_of(off: usize) -> usize {
    LIB[..off].matches('\n').count()
}

#[test]
fn definition_of_types_components_and_dotted_names() {
    let (lib, ide) = setup();
    // `Resistor r1` -> model Resistor
    let d = ide.definition(&lib, "Lib.mo", at("Resistor r1", 0, 2));
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].start, at("model Resistor", 0, 6));
    assert_eq!(d[0].class.as_deref(), Some("Lib.Resistor"));
    // `r1.p.v` in an equation -> `v` declared in Pin
    let d = ide.definition(&lib, "Lib.mo", at("r1.p.v", 0, 5));
    assert_eq!(d[0].start, at("Voltage v", 0, 8));
    assert_eq!(d[0].class.as_deref(), Some("Lib.Pin"));
    // inherited component through extends: `u` in Resistor -> TwoPin.u
    let d = ide.definition(&lib, "Lib.mo", at("u = R", 0, 0));
    assert_eq!(d[0].start, at("Voltage u", 0, 8));
    // modifier name `R` in `r1(R = ...)` -> parameter R of Resistor
    let d = ide.definition(&lib, "Lib.mo", at("r1(R", 0, 3));
    assert_eq!(d[0].start, at("Real R(", 0, 5));
    // nested modifier `u(start = 0)`: u -> TwoPin.u
    let d = ide.definition(&lib, "Lib.mo", at("u(start = 0)", 0, 0));
    assert_eq!(d[0].start, at("Voltage u", 0, 8));
    // imported constant
    let d = ide.definition(&lib, "Lib.mo", at("2 * pi", 0, 4));
    assert_eq!(d[0].start, at("Real pi", 0, 5));
    // import path parts
    let d = ide.definition(&lib, "Lib.mo", at("Lib.Constants.pi;", 0, 4));
    assert_eq!(d[0].start, at("package Constants", 0, 8));
    // extends
    let d = ide.definition(&lib, "Lib.mo", at("extends TwoPin", 0, 9));
    assert_eq!(d[0].start, at("model TwoPin", 0, 6));
    // enumeration literal
    let d = ide.definition(&lib, "Lib.mo", at("Mode.on", 0, 5));
    assert_eq!(line_of(d[0].start), line_of(at("type Mode", 0, 0)));
    // for index
    let d = ide.definition(&lib, "Lib.mo", at("x[k]", 0, 2));
    assert_eq!(d[0].start, at("for k", 0, 4));
    // builtins have no definition
    assert!(
        ide.definition(&lib, "Lib.mo", at("Real x", 0, 1))
            .is_empty()
    );
}

#[test]
fn references_and_rename() {
    let (lib, ide) = setup();
    // component R of Resistor: declaration, use in equation, two modifiers, r1.R
    let refs = ide.references(&lib, "Lib.mo", at("Real R(", 0, 5));
    let starts: Vec<usize> = refs.iter().map(|l| l.start).collect();
    for want in [
        at("Real R(", 0, 5),
        at("= R *", 0, 2),
        at("r1(R", 0, 3),
        at("r2(R", 0, 3),
        at("r1.R", 0, 3),
    ] {
        assert!(starts.contains(&want), "missing {want} in {starts:?}");
    }
    assert_eq!(refs.len(), 5, "{refs:?}");

    // rename the class Pin: header, end clause and both declarations in TwoPin
    let edits = ide
        .rename(&lib, "Lib.mo", at("Pin p", 0, 1), "Port", &|_| false)
        .unwrap();
    assert_eq!(edits.len(), 4, "{edits:?}");
    let mut text = LIB.to_string();
    let mut e = edits.clone();
    e.sort_by_key(|x| std::cmp::Reverse(x.start));
    for x in e {
        text.replace_range(x.start..x.end, &x.new_text);
    }
    assert!(text.contains("connector Port \"Electrical pin\""));
    assert!(text.contains("end Port;"));
    assert!(text.contains("Port p \"Positive pin\""));
    // the renamed library still checks clean
    let mut lib2 = Library::new();
    let mut ide2 = Ide::new();
    assert!(ide2.update(&mut lib2, "Lib.mo", &text).is_empty());

    // read-only documents refuse
    let err = ide
        .rename(&lib, "Lib.mo", at("Pin p", 0, 1), "Port", &|u| {
            u == "Lib.mo"
        })
        .unwrap_err();
    assert!(err.contains("read-only"), "{err}");
    // name clashes and keywords refuse
    assert!(
        ide.rename(&lib, "Lib.mo", at("Real R(", 0, 5), "u", &|_| false)
            .is_err()
    );
    assert!(
        ide.rename(&lib, "Lib.mo", at("Real R(", 0, 5), "model", &|_| false)
            .is_err()
    );
}

#[test]
fn references_across_documents() {
    let (mut lib, mut ide) = setup();
    let user =
        "model Use\n  Lib.Resistor res(R = 5);\nequation\n  res.p.v = res.n.v + 1;\nend Use;\n";
    assert!(ide.update(&mut lib, "Use.mo", user).is_empty());
    let refs = ide.references(&lib, "Lib.mo", at("model Resistor", 0, 6));
    let uris: Vec<&str> = refs.iter().map(|l| l.uri.as_str()).collect();
    assert!(uris.contains(&"Use.mo"), "{refs:?}");
    // definition from the user document into the library
    let off = user.find("R = 5").unwrap();
    let d = ide.definition(&lib, "Use.mo", off);
    assert_eq!(d[0].uri, "Lib.mo");
    assert_eq!(d[0].start, at("Real R(", 0, 5));
}

#[test]
fn completion_contexts() {
    let (mut lib, mut ide) = setup();
    // members after a dot, while the document does not parse
    let text = LIB.replace("    r2.n.v = 0;\n", "    r2.n.v = 0;\n    r1.p.\n");
    let diags = ide.update(&mut lib, "Lib.mo", &text);
    assert!(diags.iter().any(|d| d.severity == "error"));
    let off = text.find("r1.p.\n").unwrap() + 5;
    let c = ide.completion(&lib, "Lib.mo", off);
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"v") && labels.contains(&"i"), "{labels:?}");
    assert_eq!(c.from, off);

    // modifier names inside `Resistor r3(|`
    let text = LIB.replace("    Real x[3];\n", "    Real x[3];\n    Resistor r3(\n");
    ide.update(&mut lib, "Lib.mo", &text);
    let off = text.find("r3(").unwrap() + 3;
    let c = ide.completion(&lib, "Lib.mo", off);
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"R") && labels.contains(&"u") && labels.contains(&"p"),
        "{labels:?}"
    );
    assert!(!labels.contains(&"Resistor"));

    // attributes of a builtin type: `Real y(st|`
    let text = LIB.replace("    Real x[3];\n", "    Real x[3];\n    Real y(st\n");
    ide.update(&mut lib, "Lib.mo", &text);
    let off = text.find("y(st").unwrap() + 4;
    let c = ide.completion(&lib, "Lib.mo", off);
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(labels, ["start", "stateSelect"]);

    // names in scope with a prefix: components, classes, keywords
    let text = LIB.replace("    r2.n.v = 0;\n", "    r2.n.v = 0;\n    Re\n");
    ide.update(&mut lib, "Lib.mo", &text);
    let off = text.find("    Re\n").unwrap() + 6;
    let c = ide.completion(&lib, "Lib.mo", off);
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"Resistor") && labels.contains(&"Real") && labels.contains(&"redeclare"),
        "{labels:?}"
    );
    let text = LIB.replace("    r2.n.v = 0;\n", "    r2.n.v = 0;\n    r\n");
    ide.update(&mut lib, "Lib.mo", &text);
    let off = text.find("    r\n").unwrap() + 5;
    let c = ide.completion(&lib, "Lib.mo", off);
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"r1") && labels.contains(&"r2"),
        "{labels:?}"
    );

    // classes of a package
    let text = LIB.replace("    Real x[3];\n", "    Real x[3];\n    Lib.\n");
    ide.update(&mut lib, "Lib.mo", &text);
    let off = text.find("Lib.\n").unwrap() + 4;
    let c = ide.completion(&lib, "Lib.mo", off);
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.contains(&"Resistor") && labels.contains(&"Constants") && labels.contains(&"Pin"),
        "{labels:?}"
    );

    // nothing inside comments and strings
    let off = at("\"Ideal resistor", 0, 3);
    assert!(ide.completion(&lib, "Lib.mo", off).items.is_empty());
}

#[test]
fn hover_shows_declaration_description_and_unit() {
    let (lib, ide) = setup();
    let h = ide.hover(&lib, "Lib.mo", at("r1.p.v", 0, 5)).unwrap();
    assert!(h.markdown.contains("Voltage v"), "{}", h.markdown);
    assert!(h.markdown.contains("Potential at the pin"));
    assert!(h.markdown.contains("Unit: `V`"), "{}", h.markdown);
    let h = ide.hover(&lib, "Lib.mo", at("Resistor r1", 0, 0)).unwrap();
    assert!(h.markdown.contains("model Lib.Resistor"));
    assert!(h.markdown.contains("Ideal resistor"));
    assert!(h.markdown.contains("Lib.TwoPin"));
    let h = ide.hover(&lib, "Lib.mo", at("Voltage u", 0, 1)).unwrap();
    assert!(
        h.markdown.contains("type Voltage = Real(unit = \"V\""),
        "{}",
        h.markdown
    );
    let h = ide.hover(&lib, "Lib.mo", at("i(unit", 0, 0)).unwrap();
    assert!(h.markdown.contains("Unit: `A`"), "{}", h.markdown);
}

#[test]
fn outline() {
    let (lib, ide) = setup();
    let s = ide.symbols(&lib, "Lib.mo");
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].name, "Lib");
    let names: Vec<&str> = s[0].children.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Voltage",
            "Mode",
            "Pin",
            "TwoPin",
            "Resistor",
            "Constants",
            "Circuit"
        ]
    );
    let r = &s[0].children[4];
    assert_eq!(r.kind, "model");
    let comps: Vec<(&str, &str)> = r
        .children
        .iter()
        .map(|c| (c.name.as_str(), c.kind.as_str()))
        .collect();
    assert_eq!(comps, [("R", "parameter")]);
    let circuit = &s[0].children[6];
    assert!(
        circuit
            .children
            .iter()
            .any(|c| c.name == "r1" && c.detail == "Resistor")
    );
}

#[test]
fn diagnostics_with_ranges() {
    let (mut lib, mut ide) = setup();
    // syntax error
    let bad = LIB.replace("u = p.v - n.v;", "u = p.v - ;");
    let d = ide.update(&mut lib, "Lib.mo", &bad);
    assert!(!d.is_empty() && d[0].severity == "error");
    let semi = bad.find("- ;").unwrap() + 2;
    assert!(
        d.iter().any(|x| x.start <= semi && semi <= x.end + 1),
        "{d:?}"
    );
    // unknown class, unknown member, unknown name, unknown modifier
    let bad = LIB
        .replace("Resistor r1(R = 2 * pi);", "Resistr r1(R = 2 * pi);")
        .replace("r2.n.v = 0;", "r2.n.v = zz;")
        .replace("Resistor r2(R = r1.R, ", "Resistor r2(Rx = r1.R, ")
        .replace("import Lib.Constants.pi;", "import Lib.Constant.pi;");
    let d = ide.update(&mut lib, "Lib.mo", &bad);
    let msgs: Vec<&str> = d.iter().map(|x| x.message.as_str()).collect();
    assert!(msgs.contains(&"unknown class `Resistr`"), "{msgs:?}");
    assert!(msgs.contains(&"unknown name `zz`"), "{msgs:?}");
    assert!(
        msgs.contains(&"`Lib.Resistor` has no element `Rx`"),
        "{msgs:?}"
    );
    assert!(msgs.contains(&"`Lib` has no member `Constant`"), "{msgs:?}");
    let r = d.iter().find(|x| x.message.contains("Resistr")).unwrap();
    assert_eq!(&bad[r.start..r.end], "Resistr");
}

#[test]
fn utf16_offsets() {
    let t = "// µ°\nmodel M end M;";
    let b = t.find("model").unwrap();
    let u = byte_to_utf16(t, b);
    assert_eq!(u, 6);
    assert_eq!(utf16_to_byte(t, u), b);
}

/// Every document of the shipped library resolves without diagnostics (no false
/// positives), and the requests stay fast.
#[test]
fn shipped_library_has_no_diagnostics() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../library/Daedalus");
    let mut lib = Library::new();
    lib.load_dir(&root).unwrap();
    if let Some(msl) = std::env::var_os("DAEDALUS_MSL") {
        lib.load_dir(std::path::Path::new(&msl)).unwrap();
    }
    let ide = Ide::new();
    let mut bad = Vec::new();
    let t0 = std::time::Instant::now();
    let uris: Vec<String> = lib.docs.iter().map(|d| d.uri.clone()).collect();
    for u in &uris {
        for d in ide.diagnostics(&lib, u) {
            let text = &lib.docs.iter().find(|x| &x.uri == u).unwrap().text;
            let line = text[..d.start].matches('\n').count() + 1;
            bad.push(format!("{u}:{line}: {} {}", d.severity, d.message));
        }
    }
    eprintln!("{} documents in {:?}", uris.len(), t0.elapsed());
    let mut per_file: std::collections::BTreeMap<String, usize> = Default::default();
    for b in &bad {
        let f = b.split(':').next().unwrap().to_string();
        let n = per_file.entry(f).or_default();
        *n += 1;
        if *n <= 3 {
            eprintln!("{b}");
        }
    }
    assert!(bad.is_empty(), "{} diagnostics", bad.len());
}

/// Per-keystroke and per-request latency with the whole MSL loaded (`DAEDALUS_MSL`).
#[test]
fn msl_latency() {
    let Some(msl) = std::env::var_os("DAEDALUS_MSL") else {
        return;
    };
    let mut lib = Library::new();
    lib.load_dir(std::path::Path::new(&msl)).unwrap();
    let mut ide = Ide::new();
    let user = "model Test\n  Modelica.Blocks.Sources.Step step(height = 2);\n  Modelica.Blocks.Continuous.FirstOrder pt1(T = 0.1);\nequation\n  connect(step.y, pt1.u);\nend Test;\n";
    let t0 = std::time::Instant::now();
    let d = ide.update(&mut lib, "Test.mo", user);
    assert!(d.is_empty(), "{d:?}");
    let t_update = t0.elapsed();
    let typed = user.replace("pt1.u);", "pt1.u);\n  step.");
    let t0 = std::time::Instant::now();
    ide.update(&mut lib, "Test.mo", &typed);
    let c = ide.completion(&lib, "Test.mo", typed.find("step.\n").unwrap() + 5);
    let t_complete = t0.elapsed();
    let labels: Vec<&str> = c.items.iter().map(|i| i.label.as_str()).collect();
    assert!(labels.contains(&"height"), "{labels:?}");
    ide.update(&mut lib, "Test.mo", user);
    let t0 = std::time::Instant::now();
    let refs = ide.references(&lib, "Test.mo", user.find("Step step").unwrap() + 2);
    let t_refs = t0.elapsed();
    assert!(refs.len() > 10, "{}", refs.len());
    let t0 = std::time::Instant::now();
    let h = ide
        .hover(&lib, "Test.mo", user.find("pt1.u").unwrap() + 4)
        .unwrap();
    let t_hover = t0.elapsed();
    assert!(h.markdown.contains("RealInput"), "{}", h.markdown);
    eprintln!(
        "MSL ({} docs): update {t_update:?}, edit+complete {t_complete:?}, references {t_refs:?} ({} found), hover {t_hover:?}",
        lib.docs.len(),
        refs.len()
    );
    assert!(t_complete.as_millis() < 500);
}
