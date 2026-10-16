//! Zip archives: model packages with their resources, library archives, and the
//! library index, install specifications and `uses` resolution.
#![cfg(feature = "archive")]

use daedalus_core::archive::{self, find_libraries, modelica_uris, read_package, split_uri, write_zip};
use daedalus_core::frontend::Library;
use daedalus_core::libraries::{self, Available, Requirement, builtin_index, parse_spec, resolve};

fn mylib() -> Library {
    let mut l = Library::new();
    // a library registered the way the workbench and `-L` register them
    l.add_lazy_text("lib/MyLib/package.mo", "MyLib", "package MyLib\n  annotation(version=\"1.0.0\", uses(Modelica(version=\"4.0.0\")));\nend MyLib;\n");
    l.add_lazy_text(
        "lib/MyLib/Parts.mo",
        "MyLib.Parts",
        "within MyLib;\npackage Parts\n  model Tank\n    parameter String file = Modelica.Utilities.Files.loadResource(\"modelica://MyLib/Resources/Data/tank.txt\");\n    Real h(start=1, fixed=true);\n  equation\n    der(h) = -0.1*h;\n    annotation(Documentation(info=\"<html><img src=\\\"modelica://MyLib/Resources/Images/tank.png\\\"></html>\"));\n  end Tank;\n  model Unused\n  end Unused;\nend Parts;\n",
    );
    l.add_lazy_text("lib/MyLib/Other.mo", "MyLib.Other", "within MyLib;\nmodel Other\n  Real y = 1;\nend Other;\n");
    l.add_lazy_text("msl/Modelica/package.mo", "Modelica", "package Modelica\n  annotation(version=\"4.0.0\");\nend Modelica;\n");
    l.set_document("user/Plant.mo", "model Plant\n  MyLib.Parts.Tank tank;\n  annotation(experiment(StopTime=1));\nend Plant;\n");
    l
}

#[test]
fn zip_round_trip_and_safe_paths() {
    let files = vec![
        ("a/b.txt".to_string(), b"hello".to_vec()),
        ("c.bin".to_string(), vec![0u8, 1, 2, 255]),
    ];
    let z = write_zip(&files).unwrap();
    let back = archive::read_zip(&z).unwrap();
    assert_eq!(back, files);
    assert!(write_zip(&[("../evil".to_string(), vec![])]).is_err());
    assert!(archive::read_zip(b"not a zip").is_err());
}

#[test]
fn modelica_uris_are_found_and_split() {
    let t = r#"x = loadResource("modelica://MyLib/Resources/Data/a%20b.txt"); <a href="modelica://MyLib.Parts.Tank">Tank</a>"#;
    let u = modelica_uris(t);
    assert_eq!(u, vec!["modelica://MyLib/Resources/Data/a%20b.txt", "modelica://MyLib.Parts.Tank"]);
    assert_eq!(split_uri(&u[0]).unwrap(), ("MyLib".to_string(), "Resources/Data/a b.txt".to_string()));
    assert_eq!(split_uri(&u[1]).unwrap(), ("MyLib.Parts.Tank".to_string(), String::new()));
    assert!(split_uri("http://x").is_none());
}

#[test]
fn model_package_holds_the_closure_and_resources() {
    let lib = mylib();
    let have = ["lib/MyLib/Resources/Data/tank.txt"];
    let list = |key: &str| -> Vec<String> {
        have.iter().filter(|h| **h == key || h.starts_with(&format!("{key}/"))).map(|h| h.to_string()).collect()
    };
    let plan = archive::plan_package(&lib, "Plant", archive::STANDARD_LIBRARIES, &list).unwrap();
    let rels: Vec<&str> = plan.files.iter().map(|(_, r)| r.as_str()).collect();
    assert!(rels.contains(&"Plant.mo"), "{rels:?}");
    assert!(rels.contains(&"MyLib/package.mo"));
    assert!(rels.contains(&"MyLib/Parts.mo"));
    assert!(!rels.contains(&"MyLib/Other.mo"), "a file the model does not need stays out");
    assert!(!rels.iter().any(|r| r.starts_with("Modelica")));
    assert_eq!(plan.uses.get("Modelica").map(String::as_str), Some("4.0.0"));
    assert_eq!(plan.resources, vec![("lib/MyLib/Resources/Data/tank.txt".to_string(), "MyLib/Resources/Data/tank.txt".to_string())]);
    assert_eq!(plan.missing, vec!["modelica://MyLib/Resources/Images/tank.png".to_string()]);

    let docs: std::collections::HashMap<String, String> =
        lib.docs.iter().map(|d| (d.uri.clone(), d.text.to_string())).collect();
    let text = |uri: &str| docs.get(uri).cloned().or_else(|| lib.lazy_text(uri).map(|t| t.to_string()));
    let read = |key: &str| (key == have[0]).then(|| b"0 1\n1 0.5\n".to_vec());
    let zip = archive::write_package(&plan, &text, &read).unwrap();

    // open it again: the manifest, the files and the resource
    let pkg = read_package(&zip).unwrap();
    let m = pkg.manifest.as_ref().unwrap();
    assert_eq!(m.model, "Plant");
    assert_eq!(m.resources, vec!["MyLib/Resources/Data/tank.txt"]);
    assert_eq!(pkg.resources[0].1, b"0 1\n1 0.5\n");
    assert_eq!(pkg.mo.len(), 3);

    // and the model translates from the package alone
    let mut l2 = Library::new();
    let (n, skipped) = archive::register_package(&mut l2, &pkg, "pkg/");
    assert_eq!(n, 3);
    assert!(skipped.is_empty());
    l2.load_for("Plant");
    daedalus_core::translate(&l2, "Plant", &Default::default()).unwrap();
}

#[test]
fn library_archives_are_found_and_installed() {
    let paths = [
        "repo-1.0/README.md",
        "repo-1.0/Lib/package.mo",
        "repo-1.0/Lib/Sub/package.mo",
        "repo-1.0/Lib/Sub/A.mo",
        "repo-1.0/Complex.mo",
        "repo-1.0/ModelicaServices/package.mo",
        "repo-1.0/test/Test.mo",
    ];
    let found = find_libraries(&paths);
    assert_eq!(
        found,
        vec![
            ("Complex".to_string(), "repo-1.0/Complex.mo".to_string()),
            ("Lib".to_string(), "repo-1.0/Lib/".to_string()),
            ("ModelicaServices".to_string(), "repo-1.0/ModelicaServices/".to_string()),
        ]
    );

    let zip = write_zip(&[
        ("x-2.0/Lib/package.mo".to_string(), b"package Lib\n annotation(version=\"2.0.0\", uses(Base(version=\"1.1\")));\nend Lib;\n".to_vec()),
        ("x-2.0/Lib/M.mo".to_string(), b"within Lib;\nmodel M\nend M;\n".to_vec()),
        ("x-2.0/Lib/Resources/r.txt".to_string(), b"r".to_vec()),
    ])
    .unwrap();
    let dir = std::env::temp_dir().join(format!("daedalus-test-libs-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let got = archive::extract_libraries(&zip, &dir, &[], None).unwrap();
    assert_eq!(got.len(), 1);
    assert_eq!(got[0].version.as_deref(), Some("2.0.0"));
    assert!(dir.join("Lib 2.0.0/M.mo").is_file());
    assert!(dir.join("Lib 2.0.0/Resources/r.txt").is_file());
    let inst = libraries::scan_installed(&dir);
    assert_eq!(inst[0].name, "Lib");
    assert_eq!(inst[0].version.as_deref(), Some("2.0.0"));
    // a versioned directory loads as package Lib
    let mut l = Library::new();
    l.add_dir_lazy(&dir.join("Lib 2.0.0")).unwrap();
    l.load_for("Lib.M");
    assert!(daedalus_core::frontend::inspect::class_info(&l, "Lib.M").is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn index_and_install_specs() {
    let idx = builtin_index();
    for name in ["Buildings", "ThermoSysPro", "OpenIPSL", "VehicleInterfaces", "ExternData", "Modelica"] {
        assert!(idx.find(name).is_some(), "{name} in the index");
    }
    let s = parse_spec("Buildings@11.1.0", &idx).unwrap();
    assert_eq!(s.repo, "lbl-srg/modelica-buildings");
    assert_eq!(s.git_ref, "v11.1.0");
    assert_eq!(s.version.as_deref(), Some("11.1.0"));
    // latest when no version is given
    let s = parse_spec("buildings", &idx).unwrap();
    assert_eq!(s.version.as_deref(), Some("12.1.0"));
    let s = parse_spec("https://github.com/modelica/ModelicaStandardLibrary/tree/v4.0.0", &idx).unwrap();
    assert_eq!((s.git_ref.as_str(), s.version.as_deref()), ("v4.0.0", Some("4.0.0")));
    assert_eq!(s.packages, vec!["Modelica", "ModelicaServices", "Complex"]);
    let s = parse_spec("https://github.com/someone/MyLib@main", &idx).unwrap();
    assert_eq!((s.repo.as_str(), s.git_ref.as_str(), s.version.as_deref()), ("someone/MyLib", "main", None));
    assert!(s.archive_urls()[0].starts_with("https://codeload.github.com/someone/MyLib/zip/refs/tags/main"));
    let s = parse_spec("someone/MyLib@v1.2", &idx).unwrap();
    assert_eq!(s.version.as_deref(), Some("1.2"));
    assert!(s.cdn_urls().1.starts_with("https://cdn.jsdelivr.net/gh/someone/MyLib@v1.2"));
    assert!(parse_spec("NoSuchLibrary", &idx).is_err());
}

#[test]
fn uses_resolution_and_conflicts() {
    let av = |n: &str, v: &str, loaded: bool| Available {
        name: n.into(),
        version: Some(v.into()),
        origin: format!("{n} {v}"),
        loaded,
    };
    let rq = |by: &str, n: &str, v: &str| Requirement { by: by.into(), name: n.into(), version: v.into() };
    let avail = vec![av("Modelica", "3.2.3", false), av("Modelica", "4.0.0", false), av("Modelica", "4.1.0", false), av("Lib", "1.0", false)];

    // exact version
    let r = resolve(&[rq("M", "Modelica", "4.0.0")], &avail);
    assert_eq!(r.selected[0].version.as_deref(), Some("4.0.0"));
    assert!(r.conflicts.is_empty() && r.notes.is_empty());
    // same major, newest, with a note
    let r = resolve(&[rq("M", "Modelica", "4.0.1")], &avail);
    assert_eq!(r.selected[0].version.as_deref(), Some("4.1.0"));
    assert_eq!(r.notes.len(), 1);
    // two requirers of different majors: a conflict
    let r = resolve(&[rq("A", "Modelica", "3.2.3"), rq("B", "Modelica", "4.0.0")], &avail);
    assert_eq!(r.conflicts.len(), 1, "{r:?}");
    assert!(r.conflicts[0].contains("3.2.3 (by A)") && r.conflicts[0].contains("4.0.0 (by B)"));
    // an explicitly loaded library wins, and a major mismatch is a conflict
    let mut a2 = avail.clone();
    a2.push(av("Modelica", "4.1.0", true));
    let r = resolve(&[rq("Old", "Modelica", "3.2.3")], &a2);
    assert!(r.selected[0].loaded);
    assert_eq!(r.conflicts.len(), 1);
    // nothing installed
    let r = resolve(&[rq("M", "Missing", "1.0")], &avail);
    assert_eq!(r.missing.len(), 1);
    // only another major installed
    let r = resolve(&[rq("M", "Lib", "2.0")], &avail);
    assert!(r.selected.is_empty());
    assert_eq!(r.conflicts.len(), 1);

    assert_eq!(libraries::compare_versions("4.10.0", "4.9.1"), std::cmp::Ordering::Greater);
    assert_eq!(libraries::compare_versions("4.1.0-rc.1", "4.1.0"), std::cmp::Ordering::Less);
}

#[test]
fn requirements_of_loaded_documents() {
    let mut l = Library::new();
    l.set_document("m.mo", "model M\n annotation(uses(Modelica(version=\"4.0.0\"), Buildings(version=\"12.1.0\")));\nend M;\n");
    let r = libraries::requirements(&l);
    assert_eq!(r.len(), 2);
    assert_eq!(r[1].name, "Buildings");
    assert_eq!(r[1].by, "M");
}
