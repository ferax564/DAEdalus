//! The browser's way of loading the Modelica Standard Library: every file registered from
//! memory, outlined for the package browser, parsed only when a class needs it.
//!
//! Runs when `DAEDALUS_MSL` names an MSL checkout (the directory holding `Modelica/`).

use daedalus_wasm::Workspace;
use std::path::Path;
use std::time::Instant;

fn register(ws: &mut Workspace, root: &Path) -> usize {
    let mut n = 0;
    // As web/scripts/fetch-msl.mjs packs it: package directories only.
    let mut stack: Vec<String> = ["Modelica", "ModelicaServices", "Complex.mo"]
        .map(String::from)
        .to_vec();
    while let Some(rel) = stack.pop() {
        let p = root.join(&rel);
        if p.is_dir() {
            if !p.join("package.mo").is_file() {
                continue;
            }
            for e in std::fs::read_dir(&p).unwrap().flatten() {
                stack.push(format!("{rel}/{}", e.file_name().to_string_lossy()));
            }
        } else if rel.ends_with(".mo") || rel.ends_with("package.order") {
            ws.add_library_file("msl/", &rel, &std::fs::read_to_string(&p).unwrap());
            n += 1;
        }
    }
    n
}

fn find<'a>(tree: &'a serde_json::Value, path: &str) -> Option<&'a serde_json::Value> {
    let mut level = tree;
    let mut found = None;
    let mut cur = String::new();
    for part in path.split('.') {
        cur = if cur.is_empty() {
            part.to_string()
        } else {
            format!("{cur}.{part}")
        };
        let e = level
            .as_array()?
            .iter()
            .find(|e| e["path"] == cur.as_str())?;
        level = &e["children"];
        found = Some(e);
    }
    found
}

#[test]
fn msl_on_demand() {
    let Some(dir) = std::env::var_os("DAEDALUS_MSL") else {
        eprintln!("DAEDALUS_MSL not set; skipped");
        return;
    };
    let root = Path::new(&dir);
    let mut ws = Workspace::new();
    let t0 = Instant::now();
    let n = register(&mut ws, root);
    eprintln!("registered {n} files in {:?}", t0.elapsed());
    let t0 = Instant::now();
    let tree: serde_json::Value = serde_json::from_str(&ws.class_tree()).unwrap();
    eprintln!("class tree in {:?}", t0.elapsed());
    let stats: serde_json::Value = serde_json::from_str(&ws.library_stats()).unwrap();
    assert_eq!(stats["parsed"], 0, "nothing is parsed before it is needed");
    let names: Vec<&str> = tree
        .as_array()
        .unwrap()
        .iter()
        .map(|e| e["name"].as_str().unwrap())
        .collect();
    for top in ["Complex", "Modelica", "ModelicaServices"] {
        assert!(names.contains(&top), "{top} missing from {names:?}");
    }
    let ex = find(
        &tree,
        "Modelica.Mechanics.Rotational.Examples.CoupledClutches",
    )
    .expect("outlined");
    assert_eq!(ex["kind"], "model");
    assert_eq!(ex["runnable"], true);
    assert!(!ex["description"].as_str().unwrap().is_empty());
    // package.order puts the user's guide first.
    let modelica = find(&tree, "Modelica").unwrap();
    assert_eq!(modelica["children"][0]["name"], "UsersGuide");

    // A package opens with its own body only.
    let info: serde_json::Value = serde_json::from_str(&ws.class_info("Modelica.Blocks")).unwrap();
    assert_eq!(info["kind"], "package");
    let stats: serde_json::Value = serde_json::from_str(&ws.library_stats()).unwrap();
    assert!(stats["parsed"].as_u64().unwrap() < 20, "{stats}");

    for model in [
        "Modelica.Mechanics.Rotational.Examples.CoupledClutches",
        "Modelica.Electrical.Analog.Examples.ChuaCircuit",
        "Modelica.Blocks.Examples.PID_Controller",
    ] {
        let t0 = Instant::now();
        let info: serde_json::Value = serde_json::from_str(&ws.class_info(model)).unwrap();
        assert_eq!(info["path"], model);
        let opened = t0.elapsed();
        let r: serde_json::Value = serde_json::from_str(&ws.simulate(model, "{}")).unwrap();
        let stats: serde_json::Value = serde_json::from_str(&ws.library_stats()).unwrap();
        eprintln!(
            "{model}: open {opened:?}, simulate {:?}, ok {}, {} points, parsed {}",
            t0.elapsed() - opened,
            r["ok"],
            r["points"],
            stats["parsed"]
        );
        assert_eq!(r["ok"], true, "{model}: {}", r["error"]);
        assert!(r["points"].as_u64().unwrap() > 10);
    }
    // The tree still lists everything after parsing part of the library.
    let tree2: serde_json::Value = serde_json::from_str(&ws.class_tree()).unwrap();
    assert!(find(&tree2, "Modelica.Fluid.Examples").is_some());
    assert!(find(&tree2, "Modelica.Blocks.Examples.PID_Controller").is_some());
}
