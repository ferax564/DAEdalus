//! WebAssembly bindings for the browser workbench.
//!
//! A [`Workspace`] owns the loaded sources and the last simulation result. Structured data
//! crosses the boundary as JSON strings; trajectories cross as `Float64Array`s so a plot of
//! a million points costs one copy.

use daedalus_core::frontend::{Library, inspect};
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
        to_json(&self.lib.set_document(uri, text))
    }

    #[wasm_bindgen(js_name = removeDocument)]
    pub fn remove_document(&mut self, uri: &str) {
        self.lib.remove_document(uri);
    }

    /// JSON: the class tree of everything loaded.
    #[wasm_bindgen(js_name = classTree)]
    pub fn class_tree(&self) -> String {
        to_json(&inspect::class_tree(&self.lib))
    }

    /// JSON: graphics, components, connections and documentation of one class (or null).
    #[wasm_bindgen(js_name = classInfo)]
    pub fn class_info(&self, path: &str) -> String {
        to_json(&inspect::class_info(&self.lib, path))
    }

    /// Source text of a document.
    #[wasm_bindgen(js_name = documentText)]
    pub fn document_text(&self, uri: &str) -> Option<String> {
        self.lib.docs.iter().find(|d| d.uri == uri).map(|d| d.text.to_string())
    }

    /// JSON: translation statistics, or `{"error": ...}`.
    pub fn check(&self, model: &str) -> String {
        match daedalus_core::translate(&self.lib, model, &Default::default()) {
            Ok(m) => to_json(&serde_json::json!({"ok": true, "stats": m.stats, "warnings": m.warnings, "states": m.states.len()})),
            Err(e) => to_json(&serde_json::json!({"ok": false, "error": format!("{e:#}")})),
        }
    }

    /// The flat model as Modelica-like text.
    pub fn flatten(&self, model: &str) -> String {
        match self.lib.flatten(model) {
            Ok(f) => f.dump(),
            Err(e) => format!("// error: {e:#}"),
        }
    }

    /// Translate and simulate. `options` is a JSON `SimOptions` subset (missing fields come
    /// from the experiment annotation). Returns a JSON summary; trajectories are fetched
    /// with [`Workspace::values`].
    pub fn simulate(&mut self, model: &str, options: &str) -> String {
        let t0 = now_ms();
        let m = match daedalus_core::translate(&self.lib, model, &Default::default()) {
            Ok(m) => m,
            Err(e) => {
                self.results = None;
                return to_json(&serde_json::json!({"ok": false, "error": format!("{e:#}"), "model": model}));
            }
        };
        let translate_ms = now_ms() - t0;
        let mut opts = SimOptions::from_model(&m);
        if let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(options) {
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
                VarSummary { name: &v.name, unit: &v.unit, description: &v.description, kind, value }
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
            stats,
            model_stats: Some(m.stats.clone()),
            experiment,
        })
    }

    /// Time axis of the last result.
    pub fn time(&self) -> Vec<f64> {
        self.results.as_ref().map(|r| r.time.clone()).unwrap_or_default()
    }

    /// Trajectory of one variable of the last result (parameters are expanded to a constant).
    pub fn values(&self, name: &str) -> Vec<f64> {
        self.results.as_ref().and_then(|r| r.values(name)).unwrap_or_default()
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
            let sel: Vec<String> = names.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            let _ = r.write_csv(&mut out, if sel.is_empty() { None } else { Some(&sel) });
        }
        String::from_utf8(out).unwrap_or_default()
    }
}
