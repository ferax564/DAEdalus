//! WebAssembly bindings for the browser workbench.
//!
//! A [`Workspace`] owns the loaded sources and the last simulation result. Structured data
//! crosses the boundary as JSON strings; trajectories cross as `Float64Array`s so a plot of
//! a million points costs one copy.

use daedalus_core::frontend::{Library, inspect, lazy};
use daedalus_core::results::Results;
use daedalus_core::sim::{SimOptions, simulate};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
extern "C" {
    #[wasm_bindgen(js_namespace = performance, js_name = now)]
    fn perf_now() -> f64;
}

#[cfg(target_arch = "wasm32")]
fn now_ms() -> f64 {
    perf_now()
}
#[cfg(not(target_arch = "wasm32"))]
fn now_ms() -> f64 {
    0.0
}

#[wasm_bindgen]
pub fn version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}

#[wasm_bindgen]
#[derive(Default)]
pub struct Workspace {
    lib: Library,
    results: Option<Results>,
    /// Classes whose registered library files are parsed already: (class, full).
    ensured: std::collections::HashSet<(String, bool)>,
}

fn to_json<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| format!("{{\"error\":{:?}}}", e.to_string()))
}

#[derive(Serialize)]
struct SimSummary<'a> {
    ok: bool,
    error: Option<String>,
    model: &'a str,
    vars: Vec<VarSummary<'a>>,
    points: usize,
    log: &'a [String],
    warnings: Vec<String>,
    translate_ms: f64,
    /// Time spent parsing the registered library files the model needs.
    load_ms: f64,
    stats: Option<daedalus_core::sim::SimStats>,
    model_stats: Option<daedalus_core::backend::Stats>,
    experiment: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct VarSummary<'a> {
    name: &'a str,
    unit: &'a str,
    description: &'a str,
    /// "series" | "param" | "alias"
    kind: &'static str,
    /// constant value of parameters
    value: Option<f64>,
    /// a parameter whose override changes the result (public, literal value)
    editable: bool,
}

#[wasm_bindgen]
impl Workspace {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Workspace {
        Workspace::default()
    }

    /// Add or replace a source file; returns a JSON array of parse errors.
    #[wasm_bindgen(js_name = setDocument)]
    pub fn set_document(&mut self, uri: &str, text: &str) -> String {
        self.ensured.clear();
        to_json(&self.lib.set_document(uri, text))
    }

    #[wasm_bindgen(js_name = removeDocument)]
    pub fn remove_document(&mut self, uri: &str) {
        self.ensured.clear();
        self.lib.remove_document(uri);
    }

    /// Register a file of a library without parsing it. `rel` is its path below the
    /// directory holding the library's top package (`Modelica/Blocks/package.mo`), and
    /// the document is named `prefix + rel`. `.mo` files are outlined for the class tree
    /// and parsed when a class needs them; `package.order` files order the browser.
    #[wasm_bindgen(js_name = addLibraryFile)]
    pub fn add_library_file(&mut self, prefix: &str, rel: &str, text: &str) {
        self.ensured.clear();
        if let Some(dir) = rel.strip_suffix("package.order") {
            let class = lazy::class_for_path(&format!("{dir}package.mo"));
            self.lib.add_package_order(&class, text);
        } else if rel.ends_with(".mo") {
            let class = lazy::class_for_path(rel);
            self.lib
                .add_lazy_text(&format!("{prefix}{rel}"), &class, text);
        }
    }

    /// JSON `{"files": registered library files, "parsed": how many are parsed}`.
    #[wasm_bindgen(js_name = libraryStats)]
    pub fn library_stats(&self) -> String {
        let parsed = self.lib.lazy.iter().filter(|f| f.loaded).count();
        to_json(&serde_json::json!({"files": self.lib.lazy.len(), "parsed": parsed}))
    }

    /// Parse every registered library file (the slow way out when the on-demand
    /// closure missed a class).
    #[wasm_bindgen(js_name = loadAll)]
    pub fn load_all(&mut self) {
        self.ensured.clear();
        self.lib.load_all();
    }

    /// Parse the registered library files a class needs: in full for a model (`full`
    /// None decides by kind: a package's own body only). Returns the time taken in ms.
    fn ensure(&mut self, path: &str, full: Option<bool>) -> f64 {
        if !self.lib.has_pending() {
            return 0.0;
        }
        let t0 = now_ms();
        let full = full.unwrap_or_else(|| !self.lib.lazy_is_package(path).unwrap_or(false));
        if self.ensured.contains(&(path.to_string(), full))
            || (!full && self.ensured.contains(&(path.to_string(), true)))
        {
            return 0.0;
        }
        self.lib.load_class(path, full);
        self.ensured.insert((path.to_string(), full));
        now_ms() - t0
    }

    /// JSON: the class tree of everything loaded.
    #[wasm_bindgen(js_name = classTree)]
    pub fn class_tree(&self) -> String {
        to_json(&inspect::class_tree(&self.lib))
    }

    /// JSON: graphics, components, connections and documentation of one class (or null).
    #[wasm_bindgen(js_name = classInfo)]
    pub fn class_info(&mut self, path: &str) -> String {
        self.ensure(path, None);
        to_json(&inspect::class_info(&self.lib, path))
    }

    /// Source text of a document.
    #[wasm_bindgen(js_name = documentText)]
    pub fn document_text(&self, uri: &str) -> Option<String> {
        self.lib
            .docs
            .iter()
            .find(|d| d.uri == uri)
            .map(|d| d.text.to_string())
            .or_else(|| self.lib.lazy_text(uri).map(|t| t.to_string()))
    }

    /// JSON: translation statistics, or `{"error": ...}`.
    pub fn check(&mut self, model: &str) -> String {
        self.ensure(model, Some(true));
        match daedalus_core::translate(&self.lib, model, &Default::default()) {
            Ok(m) => to_json(
                &serde_json::json!({"ok": true, "stats": m.stats, "warnings": m.warnings, "states": m.states.len()}),
            ),
            Err(e) => to_json(&serde_json::json!({"ok": false, "error": format!("{e:#}")})),
        }
    }

    /// The flat model as Modelica-like text.
    pub fn flatten(&mut self, model: &str) -> String {
        self.ensure(model, Some(true));
        match self.lib.flatten(model) {
            Ok(f) => f.dump(),
            Err(e) => format!("// error: {e:#}"),
        }
    }

    /// Translate and simulate. `options` is a JSON `SimOptions` subset (missing fields come
    /// from the experiment annotation). Returns a JSON summary; trajectories are fetched
    /// with [`Workspace::values`].
    pub fn simulate(&mut self, model: &str, options: &str) -> String {
        let load_ms = self.ensure(model, Some(true));
        let t0 = now_ms();
        let bopts = backend_options(options);
        let m = match daedalus_core::translate(&self.lib, model, &bopts) {
            Ok(m) => m,
            Err(e) => {
                self.results = None;
                return to_json(
                    &serde_json::json!({"ok": false, "error": format!("{e:#}"), "model": model}),
                );
            }
        };
        let translate_ms = now_ms() - t0;
        let mut opts = SimOptions::from_model(&m);
        if let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(options)
        {
            let mut base = serde_json::to_value(&opts).unwrap_or_default();
            if let serde_json::Value::Object(b) = &mut base {
                for (k, v) in o {
                    b.insert(k, v);
                }
            }
            if let Ok(o2) = serde_json::from_value(base) {
                opts = o2;
            }
        }
        let experiment = serde_json::to_value(&m.experiment).ok();
        let t1 = now_ms();
        let (res, stats, error) = match simulate(&m, opts) {
            Ok((r, mut s)) => {
                s.init_ms = 0.0;
                s.sim_ms = now_ms() - t1;
                (r, Some(s), None)
            }
            Err(e) => (e.partial.unwrap_or_default(), None, Some(e.message)),
        };
        self.results = Some(res);
        let r = self.results.as_ref().unwrap();
        let free: std::collections::HashSet<&str> = m
            .free_params
            .iter()
            .map(|&p| m.vars[p as usize].name.as_str())
            .collect();
        let vars = r
            .vars
            .iter()
            .filter(|v| !v.name.starts_with('$'))
            .map(|v| {
                use daedalus_core::results::ResultKind as K;
                let (kind, value) = match v.kind {
                    K::Series(_) => ("series", None),
                    K::Param(x) => ("param", Some(x)),
                    K::Alias { .. } => ("alias", None),
                };
                VarSummary {
                    name: &v.name,
                    unit: &v.unit,
                    description: &v.description,
                    kind,
                    value,
                    editable: kind == "param" && free.contains(v.name.as_str()),
                }
            })
            .collect();
        to_json(&SimSummary {
            ok: error.is_none(),
            error,
            model,
            vars,
            points: r.time.len(),
            log: &r.log,
            warnings: m.warnings.clone(),
            translate_ms,
            load_ms,
            stats,
            model_stats: Some(m.stats.clone()),
            experiment,
        })
    }

    /// Time axis of the last result.
    pub fn time(&self) -> Vec<f64> {
        self.results
            .as_ref()
            .map(|r| r.time.clone())
            .unwrap_or_default()
    }

    /// Trajectory of one variable of the last result (parameters are expanded to a constant).
    pub fn values(&self, name: &str) -> Vec<f64> {
        self.results
            .as_ref()
            .and_then(|r| r.values(name))
            .unwrap_or_default()
    }

    /// The last result as a Dymola-compatible `.mat` file.
    #[wasm_bindgen(js_name = resultMat)]
    pub fn result_mat(&self) -> Vec<u8> {
        let mut out = Vec::new();
        if let Some(r) = &self.results {
            let _ = r.write_mat(&mut out);
        }
        out
    }

    /// The last result as CSV (all variables or a comma-separated selection).
    #[wasm_bindgen(js_name = resultCsv)]
    pub fn result_csv(&self, names: &str) -> String {
        let mut out = Vec::new();
        if let Some(r) = &self.results {
            let sel: Vec<String> = names
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
            let _ = r.write_csv(&mut out, if sel.is_empty() { None } else { Some(&sel) });
        }
        String::from_utf8(out).unwrap_or_default()
    }

    /// JSON structure report (see `daedalus_core::structure`), or `{"error": ...}`.
    /// `options`: `{"jacobian": bool, "at": T, "stop_time", "tolerance", "params",
    /// "stateOverrides": [[name, tier], ...]}`.
    pub fn structure(&mut self, model: &str, options: &str) -> String {
        self.ensure(model, Some(true));
        let v: serde_json::Value = serde_json::from_str(options).unwrap_or_default();
        let opts = daedalus_core::structure::StructureOptions {
            jacobian: v["jacobian"].as_bool().unwrap_or(true),
            at: v["at"].as_f64(),
            stop_time: v["stop_time"].as_f64(),
            tolerance: v["tolerance"].as_f64(),
            params: serde_json::from_value(v["params"].clone()).unwrap_or_default(),
            ..Default::default()
        };
        let t0 = now_ms();
        match daedalus_core::structure::structure(
            &self.lib,
            model,
            &backend_options(options),
            &opts,
        ) {
            Ok(r) => {
                let mut j = serde_json::to_value(&r).unwrap_or_default();
                j["ok"] = true.into();
                j["ms"] = (now_ms() - t0).into();
                to_json(&j)
            }
            Err(e) => to_json(&serde_json::json!({"ok": false, "error": format!("{e:#}")})),
        }
    }
}

/// Backend options from a JSON options object: `"stateOverrides": [[name, tier], ...]`.
fn backend_options(options: &str) -> daedalus_core::backend::Options {
    use daedalus_core::backend::state_select::{StateOverride, parse_tier};
    let mut o = daedalus_core::backend::Options::default();
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(options)
        && let Some(list) = v["stateOverrides"].as_array()
    {
        for item in list {
            if let (Some(name), Some(tier)) =
                (item[0].as_str(), item[1].as_str().and_then(parse_tier))
            {
                o.state_overrides.push(StateOverride {
                    pattern: name.to_string(),
                    tier,
                });
            }
        }
    }
    o
}
