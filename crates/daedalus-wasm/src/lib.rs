//! WebAssembly bindings for the browser workbench.
//!
//! A [`Workspace`] owns the loaded sources and the last simulation result. Structured data
//! crosses the boundary as JSON strings; trajectories cross as `Float64Array`s so a plot of
//! a million points costs one copy.

use daedalus_core::frontend::{Library, edit, inspect, lazy};
use daedalus_core::results::Results;
use daedalus_core::sim::{SimOptions, simulate, simulate_profiled};
use serde::Serialize;
use wasm_bindgen::prelude::*;

mod analysis;

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
    /// Translation reused by [`Workspace::simulate_run`] (sweeps): (model and state
    /// overrides, compiled).
    compiled: Option<(String, daedalus_core::codegen::Model)>,
    /// Performance profile of the last simulation (JSON), when it was asked for.
    profile: Option<String>,
    /// The running analysis (see `analysis.rs`).
    job: Option<Box<dyn daedalus_core::uq::Job>>,
    /// Non-Modelica files of the loaded libraries and packages (meshes, tables, images),
    /// by path: what `modelica://` resource URIs resolve to.
    resources: std::collections::BTreeMap<String, Vec<u8>>,
}

fn err_json(e: impl std::fmt::Display) -> String {
    to_json(&serde_json::json!({"error": e.to_string()}))
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
        #[cfg(target_arch = "wasm32")]
        daedalus_core::profile::set_clock(|| perf_now() * 1e-3);
        Workspace::default()
    }

    /// Add or replace a source file; returns a JSON array of parse errors.
    #[wasm_bindgen(js_name = setDocument)]
    pub fn set_document(&mut self, uri: &str, text: &str) -> String {
        self.ensured.clear();
        self.compiled = None;
        to_json(&self.lib.set_document(uri, text))
    }

    #[wasm_bindgen(js_name = removeDocument)]
    pub fn remove_document(&mut self, uri: &str) {
        self.ensured.clear();
        self.compiled = None;
        self.lib.remove_document(uri);
    }

    /// Register a file of a library without parsing it. `rel` is its path below the
    /// directory holding the library's top package (`Modelica/Blocks/package.mo`), and
    /// the document is named `prefix + rel`. `.mo` files are outlined for the class tree
    /// and parsed when a class needs them; `package.order` files order the browser.
    #[wasm_bindgen(js_name = addLibraryFile)]
    pub fn add_library_file(&mut self, prefix: &str, rel: &str, text: &str) {
        self.ensured.clear();
        self.compiled = None;
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
        self.compiled = None;
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
        let (r, profile) = simulate_profiled(&m, opts);
        self.profile = profile.map(|p| {
            let mut v = serde_json::to_value(&p).unwrap_or_default();
            v["summary_text"] = p.summary_text().into();
            to_json(&v)
        });
        let (res, stats, error) = match r {
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

    /// JSON performance profile and event log of the last simulation (`null` unless it
    /// ran with `"profile": true`).
    pub fn profile(&self) -> String {
        self.profile.clone().unwrap_or_else(|| "null".into())
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
                j["text"] = r.to_text().into();
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

/// Sweeps and experiments: runs that reuse one translation.
#[wasm_bindgen]
impl Workspace {
    /// Simulate `model` like [`Workspace::simulate`], but keep the translation for the next
    /// call: run-time overrides (free Real parameters in `params`) reuse it, other overrides
    /// are applied as a modifier and translated again. Returns a small JSON summary (no
    /// variable list); trajectories are fetched with [`Workspace::values`].
    #[wasm_bindgen(js_name = simulateRun)]
    pub fn simulate_run(&mut self, model: &str, options: &str) -> String {
        let load_ms = self.ensure(model, Some(true));
        let t0 = now_ms();
        let mut cached = true;
        // State-selection overrides change the translation, so they are part of the key.
        let bopts = backend_options(options);
        let key = format!("{model}\0{:?}", bopts.state_overrides);
        if self.compiled.as_ref().is_none_or(|(n, _)| *n != key) {
            cached = false;
            match daedalus_core::translate(&self.lib, model, &bopts) {
                Ok(m) => self.compiled = Some((key, m)),
                Err(e) => {
                    self.results = None;
                    return to_json(
                        &serde_json::json!({"ok": false, "error": format!("{e:#}"), "model": model}),
                    );
                }
            }
        }
        let base = &self.compiled.as_ref().unwrap().1;
        let mut opts = SimOptions::from_model(base);
        if let Ok(serde_json::Value::Object(o)) = serde_json::from_str::<serde_json::Value>(options)
        {
            let mut b = serde_json::to_value(&opts).unwrap_or_default();
            if let serde_json::Value::Object(b) = &mut b {
                for (k, v) in o {
                    b.insert(k, v);
                }
            }
            if let Ok(o2) = serde_json::from_value(b) {
                opts = o2;
            }
        }
        if let Err(e) =
            daedalus_core::sweep::check_names(base, opts.params.iter().map(|(n, _)| n.as_str()))
        {
            self.results = None;
            return to_json(&serde_json::json!({"ok": false, "error": e, "model": model}));
        }
        let (runtime, structural) =
            daedalus_core::sweep::split_overrides(base, &opts.params, false);
        let modified;
        let m = if structural.is_empty() {
            base
        } else {
            let base = base.clone();
            match daedalus_core::sweep::translate_with_modifiers(
                &mut self.lib,
                model,
                &structural,
                Some(&base),
                &bopts,
            ) {
                Ok(m) => {
                    modified = m;
                    &modified
                }
                Err(e) => {
                    self.results = None;
                    return to_json(
                        &serde_json::json!({"ok": false, "error": format!("{e:#}"), "model": model}),
                    );
                }
            }
        };
        opts.params = runtime;
        let translate_ms = now_ms() - t0;
        let t1 = now_ms();
        let (res, stats, error) = match simulate(m, opts) {
            Ok((r, mut s)) => {
                s.init_ms = 0.0;
                s.sim_ms = now_ms() - t1;
                (r, Some(s), None)
            }
            Err(e) => (e.partial.unwrap_or_default(), None, Some(e.message)),
        };
        let out = serde_json::json!({
            "ok": error.is_none(),
            "error": error,
            "model": model,
            "points": res.time.len(),
            "log": res.log,
            "warnings": m.warnings,
            "translate_ms": translate_ms,
            "load_ms": load_ms,
            "cached": cached && structural.is_empty(),
            "retranslated": structural.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            "stats": stats,
        });
        self.results = Some(res);
        to_json(&out)
    }
}

fn edit_error(e: impl std::fmt::Display) -> String {
    to_json(&serde_json::json!({"ok": false, "error": e.to_string()}))
}

/// Graphical editing: source-preserving edits of a class (see `frontend::edit`).
#[wasm_bindgen]
impl Workspace {
    /// Apply a JSON array of edit ops to class `path` and load the edited document.
    /// Returns JSON `{"ok", "uri", "text", "added"}` or `{"ok": false, "error"}`.
    #[wasm_bindgen(js_name = editClass)]
    pub fn edit_class(&mut self, path: &str, ops: &str) -> String {
        let ops: Vec<edit::EditOp> = match serde_json::from_str(ops) {
            Ok(o) => o,
            Err(e) => return edit_error(format!("bad edit: {e}")),
        };
        self.ensure(path, None);
        match edit::edit_class(&self.lib, path, &ops) {
            Ok(r) => {
                self.set_document(&r.uri, &r.text);
                to_json(
                    &serde_json::json!({"ok": true, "uri": r.uri, "text": r.text, "added": r.added}),
                )
            }
            Err(e) => edit_error(format!("{e:#}")),
        }
    }

    /// Copy class `path` into package `package` of document `uri` (created if missing),
    /// with its class names written out in full. JSON `{"ok", "uri", "text", "path"}`.
    #[wasm_bindgen(js_name = duplicateClass)]
    pub fn duplicate_class(&mut self, path: &str, uri: &str, package: &str) -> String {
        self.ensure(path, Some(true));
        let target = self.document_text(uri);
        match edit::duplicate_class(&self.lib, path, target.as_deref(), package) {
            Ok((text, new_path)) => {
                self.set_document(uri, &text);
                to_json(
                    &serde_json::json!({"ok": true, "uri": uri, "text": text, "path": new_path}),
                )
            }
            Err(e) => edit_error(format!("{e:#}")),
        }
    }

    /// Add an empty model `name` to package `package` of document `uri` (created if
    /// missing). JSON `{"ok", "uri", "text", "path"}`.
    #[wasm_bindgen(js_name = newModel)]
    pub fn new_model(&mut self, uri: &str, package: &str, name: &str) -> String {
        let target = self.document_text(uri);
        match edit::new_model(target.as_deref(), package, name) {
            Ok((text, path)) => {
                self.set_document(uri, &text);
                to_json(&serde_json::json!({"ok": true, "uri": uri, "text": text, "path": path}))
            }
            Err(e) => edit_error(format!("{e:#}")),
        }
    }

    /// JSON: public parameters of a class with units, descriptions and defaults.
    pub fn parameters(&mut self, path: &str) -> String {
        self.ensure(path, None);
        to_json(&edit::parameters(&self.lib, path))
    }
}

/// Library management, documentation, parameter import, diff and model packages.
#[wasm_bindgen]
impl Workspace {
    /// Store a resource file (a mesh, table or image of a library) by path.
    #[wasm_bindgen(js_name = setResource)]
    pub fn set_resource(&mut self, path: &str, bytes: &[u8]) {
        self.resources.insert(path.to_string(), bytes.to_vec());
    }

    /// A stored resource file.
    pub fn resource(&self, path: &str) -> Option<Vec<u8>> {
        self.resources.get(path).cloned()
    }

    /// JSON: structural and text diff of a class between two source texts
    /// (`daedalus_core::diff::ClassDiff`), or `{"error": ...}`.
    #[wasm_bindgen(js_name = diffSources)]
    pub fn diff_sources(&self, old_text: &str, new_text: &str, old_class: &str, new_class: &str) -> String {
        let o = (!old_class.is_empty()).then_some(old_class);
        let n = (!new_class.is_empty()).then_some(new_class);
        match daedalus_core::diff::diff_sources(old_text, new_text, o, n.or(o)) {
            Ok(d) => to_json(&d),
            Err(e) => err_json(format!("{e:#}")),
        }
    }

    /// JSON `{uri, text}`: the document of `class` with its `__Daedalus_metadata`
    /// annotation set to `entries` (a JSON list of [key, value]).
    #[wasm_bindgen(js_name = setMetadata)]
    pub fn set_metadata(&mut self, class: &str, entries: &str) -> String {
        self.ensure(class, Some(false));
        let kv: Vec<(String, String)> = match serde_json::from_str(entries) {
            Ok(v) => v,
            Err(e) => return err_json(e),
        };
        match daedalus_core::edit::set_metadata(&self.lib, class, &kv) {
            Ok((uri, text)) => to_json(&serde_json::json!({"uri": uri, "text": text})),
            Err(e) => err_json(format!("{e:#}")),
        }
    }

    /// JSON: the parameter values of a CSV or SSV text, or `{"error": ...}`.
    #[wasm_bindgen(js_name = paramsParse)]
    pub fn params_parse(&self, text: &str, file_name: &str) -> String {
        match daedalus_core::params::parse(text, file_name) {
            Ok(p) => to_json(&p),
            Err(e) => err_json(format!("{e:#}")),
        }
    }

    /// JSON `{params, messages, modifier, record}`: the values checked against component
    /// `component` of `class` (or the class itself when empty), converted to the
    /// declared units, as a modifier and as a record named `record`.
    #[wasm_bindgen(js_name = paramsFor)]
    pub fn params_for(&mut self, class: &str, component: &str, params: &str, record: &str) -> String {
        self.ensure(class, Some(true));
        let p: Vec<daedalus_core::params::ParamValue> = match serde_json::from_str(params) {
            Ok(v) => v,
            Err(e) => return err_json(e),
        };
        let comp = (!component.is_empty()).then_some(component);
        let checked = match daedalus_core::params::check(&self.lib, class, comp, &p) {
            Ok(c) => c,
            Err(e) => return err_json(format!("{e:#}")),
        };
        let target = comp.unwrap_or_else(|| class.rsplit('.').next().unwrap_or(class));
        to_json(&serde_json::json!({
            "params": checked.params,
            "messages": checked.messages,
            "modifier": daedalus_core::params::modifier(target, &checked.params),
            "record": daedalus_core::params::record(if record.is_empty() { "Parameters" } else { record }, &checked.params),
        }))
    }

    /// JSON `{uri, text}`: the document of `class` with `params` (JSON list of
    /// ParamValue) merged into the modifier of its component `component`.
    #[wasm_bindgen(js_name = applyParams)]
    pub fn apply_params(&mut self, class: &str, component: &str, params: &str) -> String {
        self.ensure(class, Some(false));
        let p: Vec<daedalus_core::params::ParamValue> = match serde_json::from_str(params) {
            Ok(v) => v,
            Err(e) => return err_json(e),
        };
        let tree = daedalus_core::params::ModTree::from_params(&p);
        match daedalus_core::edit::apply_modifier(&self.lib, class, component, &tree) {
            Ok((uri, text)) => to_json(&serde_json::json!({"uri": uri, "text": text})),
            Err(e) => err_json(format!("{e:#}")),
        }
    }

    /// The bundled index of open libraries (JSON).
    #[wasm_bindgen(js_name = libraryIndex)]
    pub fn library_index(&self) -> String {
        daedalus_core::libraries::INDEX_JSON.to_string()
    }

    /// JSON: an install specification resolved against the index, with its archive and
    /// CDN URLs, or `{"error": ...}`.
    #[wasm_bindgen(js_name = parseSpec)]
    pub fn parse_spec(&self, spec: &str) -> String {
        let idx = daedalus_core::libraries::builtin_index();
        match daedalus_core::libraries::parse_spec(spec, &idx) {
            Ok(s) => {
                let (listing, base) = s.cdn_urls();
                to_json(&serde_json::json!({
                    "source": s, "archives": s.archive_urls(), "cdn_listing": listing, "cdn_base": base
                }))
            }
            Err(e) => err_json(format!("{e:#}")),
        }
    }

    /// JSON `{loaded, requirements, resolution}`: the libraries loaded with their
    /// versions, the `uses` they state, and how those resolve (conflicts, missing).
    #[wasm_bindgen(js_name = libraryReport)]
    pub fn library_report(&mut self) -> String {
        use daedalus_core::libraries as L;
        let loaded = L::loaded(&self.lib);
        let mut reqs = L::requirements(&self.lib);
        for f in &self.lib.lazy {
            if f.class.contains('.') || f.loaded {
                continue;
            }
            if let Some(t) = f.read() {
                for (n, v) in L::package_meta(&t).1.uses {
                    reqs.push(L::Requirement { by: f.class.clone(), name: n, version: v });
                }
            }
        }
        let res = L::resolve(&reqs, &loaded);
        to_json(&serde_json::json!({"loaded": loaded, "requirements": reqs, "resolution": res}))
    }

    /// Install the library packages of a library zip (a GitHub source archive): their
    /// files are registered for on-demand parsing under `libs/<Name> <version>/` and
    /// their resources stored. `packages` (JSON list) restricts which are taken.
    /// Returns JSON `[{name, version, files, resources, uri}]` or `{"error": ...}`.
    #[wasm_bindgen(js_name = installLibraryZip)]
    pub fn install_library_zip(&mut self, bytes: &[u8], packages: &str, version: &str) -> String {
        let packages: Vec<String> = serde_json::from_str(packages).unwrap_or_default();
        let files = match daedalus_core::archive::read_zip(bytes) {
            Ok(f) => f,
            Err(e) => return err_json(format!("{e:#}")),
        };
        let names: Vec<&str> = files.iter().map(|(n, _)| n.as_str()).collect();
        let libs: Vec<(String, String)> = daedalus_core::archive::find_libraries(&names)
            .into_iter()
            .filter(|(n, _)| packages.is_empty() || packages.iter().any(|p| p == n))
            .collect();
        if libs.is_empty() {
            return err_json("no Modelica library (a directory with package.mo) in the archive");
        }
        let present: std::collections::HashSet<String> = daedalus_core::libraries::loaded(&self.lib)
            .into_iter()
            .map(|a| a.name)
            .collect();
        self.ensured.clear();
        let mut out = Vec::new();
        for (name, prefix) in libs {
            let single = prefix.ends_with(".mo");
            let main = if single { prefix.clone() } else { format!("{prefix}package.mo") };
            let text = files
                .iter()
                .find(|(n, _)| *n == main)
                .map(|(_, d)| String::from_utf8_lossy(d).into_owned())
                .unwrap_or_default();
            let (top, meta) = daedalus_core::libraries::package_meta(&text);
            let name = if name.is_empty() { top.unwrap_or_default() } else { name };
            if present.contains(&name) {
                out.push(serde_json::json!({"name": name, "skipped": "already loaded"}));
                continue;
            }
            let ver = meta.version.clone().unwrap_or_else(|| version.to_string());
            let base = format!("libs/{name} {ver}/");
            let (mut nfiles, mut nres) = (0, 0);
            if single {
                self.lib.set_document(&format!("{base}{name}.mo"), &text);
                nfiles = 1;
            } else {
                for (n, d) in &files {
                    let Some(rel) = n.strip_prefix(prefix.as_str()) else { continue };
                    let rel = format!("{name}/{rel}");
                    if rel.ends_with(".mo") || rel.ends_with("package.order") {
                        self.add_library_file(&base, &rel, &String::from_utf8_lossy(d));
                        nfiles += 1;
                    } else {
                        self.resources.insert(format!("{base}{rel}"), d.clone());
                        nres += 1;
                    }
                }
            }
            out.push(serde_json::json!({
                "name": name, "version": ver, "files": nfiles, "resources": nres,
                "uses": meta.uses, "base": base
            }));
        }
        to_json(&out)
    }

    /// The library packages of a list of archive paths (JSON list): `[[name, prefix]]`.
    #[wasm_bindgen(js_name = findLibraries)]
    pub fn find_libraries(&self, paths: &str) -> String {
        let p: Vec<String> = serde_json::from_str(paths).unwrap_or_default();
        let refs: Vec<&str> = p.iter().map(String::as_str).collect();
        to_json(&daedalus_core::archive::find_libraries(&refs))
    }

    /// A zip of text files given as JSON `[[path, text]]`.
    #[wasm_bindgen(js_name = zipTexts)]
    pub fn zip_texts(&self, files: &str) -> Vec<u8> {
        let f: Vec<(String, String)> = serde_json::from_str(files).unwrap_or_default();
        let f: Vec<(String, Vec<u8>)> = f.into_iter().map(|(p, t)| (p, t.into_bytes())).collect();
        daedalus_core::archive::write_zip(&f).unwrap_or_default()
    }

    /// The model package of `model` (a zip with the files it needs and the resources
    /// they reference; the MSL is referred to, not included). Errors are thrown.
    #[wasm_bindgen(js_name = exportPackage)]
    pub fn export_package(&mut self, model: &str) -> Result<Vec<u8>, JsError> {
        let res = &self.resources;
        let list = |key: &str| -> Vec<String> {
            let dir = format!("{key}/");
            res.keys()
                .filter(|k| k.as_str() == key || k.starts_with(&dir))
                .cloned()
                .collect()
        };
        let plan = daedalus_core::archive::plan_package(
            &self.lib,
            model,
            daedalus_core::archive::STANDARD_LIBRARIES,
            &list,
        )
        .map_err(|e| JsError::new(&format!("{e:#}")))?;
        let lib = &self.lib;
        let text = |uri: &str| -> Option<String> {
            lib.docs
                .iter()
                .find(|d| d.uri == uri)
                .map(|d| d.text.to_string())
                .or_else(|| lib.lazy_text(uri).map(|t| t.to_string()))
        };
        let read = |key: &str| res.get(key).cloned();
        daedalus_core::archive::write_package(&plan, &text, &read)
            .map_err(|e| JsError::new(&format!("{e:#}")))
    }

    /// Open a model package (or a zip of Modelica files): its files are registered
    /// under `prefix` (libraries already loaded are skipped) and its resources stored.
    /// JSON `{manifest, files, resources: [paths], skipped}` or `{"error": ...}`.
    #[wasm_bindgen(js_name = openPackage)]
    pub fn open_package(&mut self, bytes: &[u8], prefix: &str) -> String {
        let pkg = match daedalus_core::archive::read_package(bytes) {
            Ok(p) => p,
            Err(e) => return err_json(format!("{e:#}")),
        };
        self.ensured.clear();
        let (n, skipped) = daedalus_core::archive::register_package(&mut self.lib, &pkg, prefix);
        let mut res = Vec::new();
        for (p, d) in &pkg.resources {
            let key = format!("{prefix}{p}");
            self.resources.insert(key.clone(), d.clone());
            res.push(key);
        }
        to_json(&serde_json::json!({
            "manifest": pkg.manifest, "files": n, "resources": res, "skipped": skipped
        }))
    }
}

