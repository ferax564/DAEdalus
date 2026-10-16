//! WebAssembly bindings for the browser workbench.
//!
//! A [`Workspace`] owns the loaded sources and the last simulation result. Structured data
//! crosses the boundary as JSON strings; trajectories cross as `Float64Array`s so a plot of
//! a million points costs one copy.

use daedalus_core::frontend::{Library, edit, inspect, lazy};
use daedalus_core::ide::{self, Ide};
use daedalus_core::results::Results;
use daedalus_core::sim::{SimOptions, simulate, simulate_profiled};
use serde::Serialize;
use std::collections::HashMap;
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
    /// Editor intelligence state (the last parsing version of each edited document).
    ide: Ide,
    results: Option<Results>,
    /// Classes whose registered library files are parsed already: (class, full).
    ensured: std::collections::HashSet<(String, bool)>,
    /// Translation reused by [`Workspace::simulate_run`] (sweeps): (model and state
    /// overrides, compiled).
    compiled: Option<(String, daedalus_core::codegen::Model)>,
    /// Translation reused by [`Workspace::simulate`] when Simulate is pressed again with
    /// only option or parameter changes: (model and state overrides, compiled, units).
    sim_cache: Option<(String, daedalus_core::codegen::Model, UnitMap)>,
    /// Performance profile of the last simulation (JSON), when it was asked for.
    profile: Option<String>,
    /// The running analysis (see `analysis.rs`).
    job: Option<Box<dyn daedalus_core::uq::Job>>,
    /// The model last translated for linear analysis (dropped when sources change).
    lin_model: Option<(String, daedalus_core::codegen::Model)>,
    /// The run being advanced in slices (live plotting, dashboards).
    live: Option<LiveRun>,
    /// Non-Modelica files of the loaded libraries and packages (meshes, tables, images),
    /// by path: what `modelica://` resource URIs resolve to.
    resources: std::collections::BTreeMap<String, Vec<u8>>,
}

/// Options of [`Workspace::linear_analysis`]: the linearization point and parameter
/// overrides, plus the analysis options.
#[derive(serde::Deserialize, Default)]
#[serde(default)]
struct LinearRequest {
    at: Option<f64>,
    params: Vec<(String, f64)>,
    #[serde(flatten)]
    analysis: daedalus_core::lti::LinearOptions,
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
    /// Display-unit choices keyed by "unit|displayUnit" (see `units::display_choices`).
    display: serde_json::Map<String, serde_json::Value>,
    /// The translation of the previous Simulate was reused (`translate_ms` is then 0).
    cached: bool,
}

#[derive(Serialize)]
struct VarSummary<'a> {
    name: &'a str,
    unit: &'a str,
    /// the `displayUnit` attribute
    display_unit: &'a str,
    /// `unit` was inferred by the unit checker (none declared)
    unit_inferred: bool,
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
        self.sim_cache = None;
        self.lin_model = None;
        to_json(&self.lib.set_document(uri, text))
    }

    #[wasm_bindgen(js_name = removeDocument)]
    pub fn remove_document(&mut self, uri: &str) {
        self.ensured.clear();
        self.ide.forget(uri);
        self.compiled = None;
        self.sim_cache = None;
        self.lin_model = None;
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
        self.sim_cache = None;
        self.lin_model = None;
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
        self.sim_cache = None;
        self.lin_model = None;
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
        let flat = match self.lib.flatten(model) {
            Ok(f) => f,
            Err(e) => return to_json(&serde_json::json!({"ok": false, "error": format!("{e:#}")})),
        };
        // Unit checking runs on the flat model; its findings are warnings only.
        let units = daedalus_core::units::check(&flat);
        let unit_json = serde_json::json!({
            "issues": units.issues,
            "declared": units.declared,
            "inferred": units.inferred.len(),
            "unknown": units.unknown,
            "unparsed": units.unparsed,
        });
        match daedalus_core::backend::compile(flat, &Default::default()) {
            Ok(sys) => {
                let m = daedalus_core::codegen::compile(sys);
                to_json(
                    &serde_json::json!({"ok": true, "stats": m.stats, "warnings": m.warnings,
                    "states": m.states.len(), "units": unit_json}),
                )
            }
            Err(e) => to_json(
                &serde_json::json!({"ok": false, "error": format!("{e:#}"), "units": unit_json}),
            ),
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
        let key = translation_key(model, &bopts);
        let (compiled, cached) = match self.sim_cache.take() {
            Some((k, m, u)) if k == key => (Ok((m, u)), true),
            _ => (compile_with_units(&self.lib, model, &bopts), false),
        };
        let (m, units) = match compiled {
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
        let json = summary_json(
            &m,
            r,
            model,
            error,
            translate_ms,
            load_ms,
            stats,
            experiment,
            &units,
            cached,
        );
        self.sim_cache = Some((key, m, units));
        json
    }

    /// JSON performance profile and event log of the last simulation (`null` unless it
    /// ran with `"profile": true`).
    pub fn profile(&self) -> String {
        self.profile.clone().unwrap_or_else(|| "null".into())
    }

    /// Linearize `model` (at `at`, default the start time) and analyze it: poles,
    /// transmission zeros, Bode/Nyquist data with margins, step and impulse responses,
    /// PID suggestions, and optionally balanced truncation and LQR. `options` is a JSON
    /// [`LinearRequest`]. Returns `{"ok", "error"?, "analysis"?, "inputs", "outputs",
    /// "states", "candidates", "translate_ms", "analysis_ms"}`; `candidates` are the
    /// continuous variables that can be chosen as extra outputs.
    #[wasm_bindgen(js_name = linearAnalysis)]
    pub fn linear_analysis(&mut self, model: &str, options: &str) -> String {
        let req: LinearRequest = match serde_json::from_str(options) {
            Ok(r) => r,
            Err(e) => {
                return to_json(&serde_json::json!({"ok": false, "error": format!("options: {e}")}));
            }
        };
        self.ensure(model, Some(true));
        let t0 = now_ms();
        if self.lin_model.as_ref().is_none_or(|(n, _)| n != model) {
            match daedalus_core::translate(&self.lib, model, &Default::default()) {
                Ok(m) => self.lin_model = Some((model.to_string(), m)),
                Err(e) => {
                    return to_json(&serde_json::json!({"ok": false, "error": format!("{e:#}")}));
                }
            }
        }
        let translate_ms = now_ms() - t0;
        let m = &self.lin_model.as_ref().unwrap().1;
        let names = |ids: &[u32]| -> Vec<String> {
            ids.iter()
                .map(|&v| m.vars[v as usize].name.clone())
                .collect()
        };
        use daedalus_core::linearize as lz;
        let inputs = names(&lz::real_inputs(m));
        let outputs = names(&lz::real_outputs(m));
        let states: Vec<String> = m
            .states
            .iter()
            .map(|&s| m.vars[s as usize].name.clone())
            .collect();
        let mut candidates = names(&lz::output_candidates(m));
        candidates.sort();
        let t1 = now_ms();
        let mut o = SimOptions::from_model(m);
        o.params = req.params.clone();
        let res = lz::linearize_with_outputs(
            m,
            o,
            req.at,
            lz::JacobianMethod::Exact,
            &req.analysis.outputs,
        )
        .and_then(|lin| {
            if lin.inputs.is_empty() {
                return Err(format!(
                    "{model} has no top-level Real inputs: linear analysis needs `input Real` variables at the top level"
                ));
            }
            daedalus_core::lti::analyze(&lin, &req.analysis)
        });
        let analysis_ms = now_ms() - t1;
        let (ok, error, analysis) = match res {
            Ok(a) => (true, None, Some(a)),
            Err(e) => (false, Some(e), None),
        };
        to_json(&serde_json::json!({
            "ok": ok,
            "error": error,
            "analysis": analysis,
            "inputs": inputs,
            "outputs": outputs,
            "states": states,
            "candidates": candidates,
            "translate_ms": translate_ms,
            "analysis_ms": analysis_ms,
        }))
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

    /// Equation and initialization debugger (see `daedalus_core::debug`), JSON.
    /// `options`: `{"var": name}` (transformation chain of a variable), `{"equation": id}`
    /// (of a simulation equation, ids as in the structure report), `{"init": true}` (the
    /// initialization report, also for models that do not translate), plus
    /// `stateOverrides`, `params`, `stop_time`, `tolerance` as for `structure`.
    pub fn debug(&mut self, model: &str, options: &str) -> String {
        self.ensure(model, Some(true));
        let v: serde_json::Value = serde_json::from_str(options).unwrap_or_default();
        let bopts = backend_options(options);
        let t0 = now_ms();
        if v["init"].as_bool().unwrap_or(false) {
            let params: Vec<(String, f64)> =
                serde_json::from_value(v["params"].clone()).unwrap_or_default();
            let so = match daedalus_core::translate(&self.lib, model, &bopts) {
                Ok(m) => {
                    let mut o = SimOptions::from_model(&m);
                    if let Some(t) = v["stop_time"].as_f64() {
                        o.stop_time = t;
                    }
                    if let Some(t) = v["tolerance"].as_f64() {
                        o.tolerance = t;
                    }
                    Some(o)
                }
                Err(_) => None,
            };
            // After a failed run: the nonlinear block that stopped the simulation.
            let sim_failure = match (&so, v["simulation"].as_bool().unwrap_or(false)) {
                (Some(o), true) => {
                    let mut o = o.clone();
                    o.params = params.clone();
                    daedalus_core::debug::simulation_failure(&self.lib, model, &bopts, o)
                }
                _ => None,
            };
            let r = daedalus_core::debug::init_report(&self.lib, model, &bopts, so, &params);
            let mut j = serde_json::to_value(&r).unwrap_or_default();
            j["simulation_failure"] = serde_json::to_value(&sim_failure).unwrap_or_default();
            j["text"] = r.to_text().into();
            j["ms"] = (now_ms() - t0).into();
            return to_json(&j);
        }
        let d = match daedalus_core::debug::Debugger::new(&self.lib, model, &bopts) {
            Ok(d) => d,
            Err(f) => {
                return to_json(&serde_json::json!({
                    "ok": false,
                    "error": f.error,
                    "structural": f.structural,
                }));
            }
        };
        let r = if let Some(name) = v["var"].as_str() {
            d.trace_var(name)
                .map(|t| serde_json::json!({"ok": true, "var": t, "text": t.to_text()}))
        } else if let Some(id) = v["equation"].as_u64() {
            d.trace_eq(id as usize)
                .map(|t| serde_json::json!({"ok": true, "equation": t, "text": t.to_text()}))
        } else {
            Err("nothing to debug: give var, equation or init".to_string())
        };
        let mut j = r.unwrap_or_else(|e| serde_json::json!({"ok": false, "error": e}));
        j["ms"] = (now_ms() - t0).into();
        to_json(&j)
    }
}

/// The JSON summary of a simulation: variables (with which parameters are editable),
/// statistics and messages.
#[allow(clippy::too_many_arguments)]
fn summary_json(
    m: &daedalus_core::codegen::Model,
    r: &Results,
    model: &str,
    error: Option<String>,
    translate_ms: f64,
    load_ms: f64,
    stats: Option<daedalus_core::sim::SimStats>,
    experiment: Option<serde_json::Value>,
    units: &UnitMap,
    cached: bool,
) -> String {
    let free: std::collections::HashSet<&str> = m
        .free_params
        .iter()
        .map(|&p| m.vars[p as usize].name.as_str())
        .collect();
    let mut display = serde_json::Map::new();
    let vars: Vec<VarSummary> = r
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
            let (unit, display_unit, inferred) = match units.get(&v.name) {
                Some((u, du, inf)) => (u.as_str(), du.as_str(), *inf),
                None => (v.unit.as_str(), "", false),
            };
            let key = format!("{unit}|{display_unit}");
            if !unit.is_empty()
                && !display.contains_key(&key)
                && let Some(c) = daedalus_core::units::display_choices(unit, display_unit)
            {
                display.insert(key, serde_json::to_value(c).unwrap_or_default());
            }
            VarSummary {
                name: &v.name,
                unit,
                display_unit,
                unit_inferred: inferred,
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
        display,
        cached,
    })
}

/// Resolved units by variable name: (unit, display unit, inferred).
type UnitMap = HashMap<String, (String, String, bool)>;

/// Flatten, infer the units of variables declared without one, then compile.
fn compile_with_units(
    lib: &Library,
    model: &str,
    bopts: &daedalus_core::backend::Options,
) -> anyhow::Result<(daedalus_core::codegen::Model, UnitMap)> {
    let flat = lib.flatten(model)?;
    let report = daedalus_core::units::check(&flat);
    let mut units = UnitMap::new();
    let resolved = daedalus_core::units::resolved_units(&flat, &report);
    for (i, v) in flat.vars.iter().enumerate() {
        if let Some(u) = resolved.get(&(i as u32)) {
            units.insert(
                v.name.clone(),
                (u.clone(), v.display_unit.clone(), v.unit.is_empty()),
            );
        }
    }
    let sys = daedalus_core::backend::compile(flat, bopts)?;
    Ok((daedalus_core::codegen::compile(sys), units))
}

/// A run in slices (live plotting, dashboards). `inc` borrows the model behind `model`,
/// which this struct owns: the borrow is dropped before the model (see `Drop`). `inc` is
/// `None` only once [`Workspace::live_finish`] took it.
struct LiveRun {
    inc: Option<daedalus_core::sim::Incremental<'static>>,
    model: *mut daedalus_core::codegen::Model,
    name: String,
    translate_ms: f64,
    load_ms: f64,
    t_sim0: f64,
    experiment: Option<serde_json::Value>,
    units: UnitMap,
}

impl LiveRun {
    fn model(&self) -> &daedalus_core::codegen::Model {
        // SAFETY: `model` comes from `Box::into_raw` and is freed only in `drop`.
        unsafe { &*self.model }
    }
}

impl Drop for LiveRun {
    fn drop(&mut self) {
        self.inc = None;
        // SAFETY: nothing borrows the model any more; it came from `Box::into_raw`.
        unsafe { drop(Box::from_raw(self.model)) };
    }
}

/// Simulation options: the experiment annotation's, with the JSON fields in `options`
/// replacing them.
fn sim_options(m: &daedalus_core::codegen::Model, options: &str) -> SimOptions {
    let mut opts = SimOptions::from_model(m);
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
    opts
}

/// Live runs: a simulation advanced in slices, with its rows readable in between.
#[wasm_bindgen]
impl Workspace {
    /// Translate and initialize `model` for a run in slices ([`Workspace::live_advance`]).
    /// JSON `{"ok", "error", "start_time", "stop_time"}`.
    #[wasm_bindgen(js_name = liveStart)]
    pub fn live_start(&mut self, model: &str, options: &str) -> String {
        self.live = None;
        let load_ms = self.ensure(model, Some(true));
        let t0 = now_ms();
        let (m, units) = match compile_with_units(&self.lib, model, &backend_options(options)) {
            Ok(m) => m,
            Err(e) => {
                self.results = None;
                return to_json(
                    &serde_json::json!({"ok": false, "error": format!("{e:#}"), "model": model}),
                );
            }
        };
        let translate_ms = now_ms() - t0;
        let opts = sim_options(&m, options);
        let experiment = serde_json::to_value(&m.experiment).ok();
        let (start, stop) = (opts.start_time, opts.stop_time);
        let ptr = Box::into_raw(Box::new(m));
        // SAFETY: the model lives until `LiveRun::drop`, which drops the run first.
        let mref: &'static daedalus_core::codegen::Model = unsafe { &*ptr };
        let t1 = now_ms();
        match daedalus_core::sim::Incremental::new(mref, opts) {
            Ok(inc) => {
                self.live = Some(LiveRun {
                    inc: Some(inc),
                    model: ptr,
                    name: model.to_string(),
                    translate_ms,
                    load_ms,
                    t_sim0: t1,
                    experiment,
                    units,
                });
                to_json(&serde_json::json!({"ok": true, "start_time": start, "stop_time": stop}))
            }
            Err(e) => {
                // SAFETY: nothing borrows the model any more.
                drop(unsafe { Box::from_raw(ptr) });
                self.results = None;
                to_json(&serde_json::json!({"ok": false, "error": e.message, "model": model}))
            }
        }
    }

    /// Advance the live run to time `t` (at most its stop time) or for `budget_ms` of
    /// wall-clock time (0: no budget). JSON `{"t", "points", "done"}`; once done, the run
    /// becomes the workspace's result and `"summary"` holds what `simulate` returns.
    #[wasm_bindgen(js_name = liveAdvance)]
    pub fn live_advance(&mut self, t: f64, budget_ms: f64) -> String {
        let Some(run) = self.live.as_mut() else {
            return to_json(&serde_json::json!({"done": true, "error": "no live run"}));
        };
        let inc = run.inc.as_mut().unwrap();
        let r = inc.advance(t, budget_ms * 1e-3);
        let done = matches!(r, Ok(true) | Err(_));
        let out = serde_json::json!({
            "t": inc.time(),
            "points": inc.results().time.len(),
            "done": done,
        });
        if !done {
            return to_json(&out);
        }
        let summary = self.live_finish();
        let mut out = out;
        out["summary"] = serde_json::from_str(&summary).unwrap_or_default();
        to_json(&out)
    }

    /// End the live run where it is; it becomes the workspace's result. Returns the
    /// summary JSON (as `simulate`), or `null` without a live run.
    #[wasm_bindgen(js_name = liveFinish)]
    pub fn live_finish(&mut self) -> String {
        let Some(mut run) = self.live.take() else {
            return "null".into();
        };
        let inc = run.inc.take().unwrap();
        let (res, mut stats, error) = inc.finish();
        stats.init_ms = 0.0;
        stats.sim_ms = now_ms() - run.t_sim0;
        let experiment = run.experiment.take();
        let json = summary_json(
            run.model(),
            &res,
            &run.name,
            error,
            run.translate_ms,
            run.load_ms,
            Some(stats),
            experiment,
            &run.units,
            false,
        );
        self.results = Some(res);
        json
    }

    /// Time axis of the live run's rows from index `from` on.
    #[wasm_bindgen(js_name = liveTime)]
    pub fn live_time(&self, from: usize) -> Vec<f64> {
        self.live
            .as_ref()
            .and_then(|r| r.inc.as_ref())
            .map(|i| i.results().time.get(from..).unwrap_or_default().to_vec())
            .unwrap_or_default()
    }

    /// Values of one variable of the live run from row `from` on (empty if unknown).
    #[wasm_bindgen(js_name = liveValues)]
    pub fn live_values(&self, name: &str, from: usize) -> Vec<f64> {
        use daedalus_core::results::ResultKind as K;
        let Some(inc) = self.live.as_ref().and_then(|r| r.inc.as_ref()) else {
            return Vec::new();
        };
        let r = inc.results();
        let n = r.time.len();
        let Some(v) = r.find(name) else {
            return Vec::new();
        };
        let from = from.min(n);
        match v.kind {
            K::Series(c) => r.series[c][from..].to_vec(),
            K::Param(x) => vec![x; n - from],
            K::Alias { col, sign } => r.series[col][from..].iter().map(|x| sign * x).collect(),
        }
    }

    /// Change free Real parameters of the live run at the time reached (JSON
    /// `[[name, value], ...]`). JSON `{"ok", "error"}`.
    #[wasm_bindgen(js_name = liveSetParams)]
    pub fn live_set_params(&mut self, params: &str) -> String {
        let Some(inc) = self.live.as_mut().and_then(|r| r.inc.as_mut()) else {
            return to_json(&serde_json::json!({"ok": false, "error": "no live run"}));
        };
        let ps: Vec<(String, f64)> = match serde_json::from_str(params) {
            Ok(p) => p,
            Err(e) => return to_json(&serde_json::json!({"ok": false, "error": e.to_string()})),
        };
        match inc.set_params(&ps) {
            Ok(()) => to_json(&serde_json::json!({"ok": true, "t": inc.time()})),
            Err(e) => to_json(&serde_json::json!({"ok": false, "error": e})),
        }
    }
}

/// Cache key of a translation: the model and the options that change its structure.
fn translation_key(model: &str, bopts: &daedalus_core::backend::Options) -> String {
    format!("{model}\0{:?}", bopts.state_overrides)
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
        let key = translation_key(model, &bopts);
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

/// A place in a document with JavaScript (UTF-16) offsets.
#[derive(Serialize)]
struct Loc16 {
    uri: String,
    from: usize,
    to: usize,
    /// 1-based line.
    line: usize,
    class: Option<String>,
    preview: String,
}

impl Workspace {
    fn text_of(&self, uri: &str) -> Option<std::sync::Arc<str>> {
        self.lib
            .docs
            .iter()
            .find(|d| d.uri == uri)
            .map(|d| d.text.clone())
    }

    fn off(&self, uri: &str, u16: usize) -> usize {
        self.text_of(uri)
            .map(|t| ide::utf16_to_byte(&t, u16))
            .unwrap_or(u16)
    }

    fn u16(&self, uri: &str, b: usize) -> usize {
        self.text_of(uri)
            .map(|t| ide::byte_to_utf16(&t, b))
            .unwrap_or(b)
    }

    fn loc16(&self, l: ide::Location) -> Loc16 {
        Loc16 {
            from: self.u16(&l.uri, l.start),
            to: self.u16(&l.uri, l.end),
            line: l.line + 1,
            uri: l.uri,
            class: l.class,
            preview: l.preview,
        }
    }

    fn symbols16(&self, uri: &str, v: Vec<ide::DocSymbol>) -> Vec<serde_json::Value> {
        v.into_iter()
            .map(|s| {
                let children = self.symbols16(uri, s.children);
                serde_json::json!({
                    "name": s.name, "kind": s.kind, "detail": s.detail, "path": s.path,
                    "from": self.u16(uri, s.start), "to": self.u16(uri, s.end),
                    "selFrom": self.u16(uri, s.sel_start), "selTo": self.u16(uri, s.sel_end),
                    "children": children,
                })
            })
            .collect()
    }
}

/// Editor intelligence. Offsets are JavaScript string indices (UTF-16 code units).
#[wasm_bindgen]
impl Workspace {
    /// Set an edited document's text (re-parsing only that document) and return its
    /// diagnostics: JSON `[{from, to, severity, message}]`.
    #[wasm_bindgen(js_name = ideUpdate)]
    pub fn ide_update(&mut self, uri: &str, text: &str) -> String {
        let registered = self.lib.is_registered_file(uri);
        if self.ide.set_text(&mut self.lib, uri, text) {
            self.ensured.clear();
            self.compiled = None;
            self.sim_cache = None;
        self.sim_cache = None;
            // parse the library classes the document names, so that they resolve
            if self.lib.has_pending()
                && !registered
                && let Some((path, _)) = Ide::document_classes(text).into_iter().next()
            {
                self.lib.load_class(&path, true);
            }
        }
        // A library file opened from the on-demand bundle may name classes not parsed
        // yet: only its syntax is checked.
        let diags = if registered {
            self.ide.syntax_diagnostics(&self.lib, uri)
        } else {
            self.ide.diagnostics(&self.lib, uri)
        };
        let out: Vec<serde_json::Value> = diags
            .into_iter()
            .map(|d| {
                serde_json::json!({"from": self.u16(uri, d.start), "to": self.u16(uri, d.end),
                    "severity": d.severity, "message": d.message})
            })
            .collect();
        to_json(&out)
    }

    /// JSON `[Loc]`: where the name at `offset` is declared.
    #[wasm_bindgen(js_name = ideDefinition)]
    pub fn ide_definition(&mut self, uri: &str, offset: usize) -> String {
        let off = self.off(uri, offset);
        let v: Vec<Loc16> = self
            .ide
            .definition(&self.lib, uri, off)
            .into_iter()
            .map(|l| self.loc16(l))
            .collect();
        to_json(&v)
    }

    /// JSON `[Loc]`: every use of the name at `offset` in the loaded documents.
    #[wasm_bindgen(js_name = ideReferences)]
    pub fn ide_references(&mut self, uri: &str, offset: usize) -> String {
        let off = self.off(uri, offset);
        let v: Vec<Loc16> = self
            .ide
            .references(&self.lib, uri, off)
            .into_iter()
            .map(|l| self.loc16(l))
            .collect();
        to_json(&v)
    }

    /// JSON `{ok, edits: [{uri, from, to, text}], error}`. Library files registered for
    /// on-demand parsing (the MSL bundle) are read-only.
    #[wasm_bindgen(js_name = ideRename)]
    pub fn ide_rename(&mut self, uri: &str, offset: usize, new_name: &str) -> String {
        let off = self.off(uri, offset);
        let lib = &self.lib;
        match self
            .ide
            .rename(lib, uri, off, new_name, &|u| lib.is_registered_file(u))
        {
            Ok(edits) => {
                let e: Vec<serde_json::Value> = edits
                    .into_iter()
                    .map(|e| {
                        serde_json::json!({"uri": e.uri, "from": self.u16(&e.uri, e.start),
                            "to": self.u16(&e.uri, e.end), "text": e.new_text})
                    })
                    .collect();
                to_json(&serde_json::json!({"ok": true, "edits": e}))
            }
            Err(e) => to_json(&serde_json::json!({"ok": false, "error": e, "edits": []})),
        }
    }

    /// JSON `{from, items: [{label, kind, detail, info}]}`.
    #[wasm_bindgen(js_name = ideCompletion)]
    pub fn ide_completion(&mut self, uri: &str, offset: usize) -> String {
        let off = self.off(uri, offset);
        // `Modelica.Blocks.|`: parse that package first, so that its members are known
        if self.lib.has_pending()
            && let Some(t) = self.text_of(uri)
            && let Some(p) = Ide::dotted_prefix(&t, off)
            && self.lib.lazy_is_package(&p).is_some()
        {
            self.ensure(&p, Some(false));
        }
        let c = self.ide.completion(&self.lib, uri, off);
        to_json(&serde_json::json!({"from": self.u16(uri, c.from), "items": c.items}))
    }

    /// JSON `{from, to, markdown}` or null.
    #[wasm_bindgen(js_name = ideHover)]
    pub fn ide_hover(&mut self, uri: &str, offset: usize) -> String {
        let off = self.off(uri, offset);
        match self.ide.hover(&self.lib, uri, off) {
            Some(h) => to_json(&serde_json::json!({"from": self.u16(uri, h.start),
                "to": self.u16(uri, h.end), "markdown": h.markdown})),
            None => "null".into(),
        }
    }

    /// JSON outline: `[{name, kind, detail, path, from, to, selFrom, selTo, children}]`.
    #[wasm_bindgen(js_name = ideSymbols)]
    pub fn ide_symbols(&self, uri: &str) -> String {
        to_json(&self.symbols16(uri, self.ide.symbols(&self.lib, uri)))
    }
}
