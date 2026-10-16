//! Analyses (sensitivities, adjoint gradients, Monte Carlo / Sobol / Morris, calibration,
//! optimization, designs of experiments) as stepwise jobs: the worker calls
//! [`Workspace::analysis_step`] with a time budget in a loop, posting progress and
//! checking for a cancel between calls.
//!
//! Designs of experiments can also fan out over the worker pool: [`doe_plan`] lists the
//! runs, each pool worker simulates some and evaluates the quantities of interest with
//! [`Workspace::qoi_values`], and [`doe_analyze`] fits the surfaces.

use super::{Workspace, now_ms, to_json};
use daedalus_core::adjoint::{GradJob, GradSpec};
use daedalus_core::calibrate::{CalibSpec, Calibration};
use daedalus_core::doe::{DoeJob, DoePlan, DoeSpec};
use daedalus_core::optimize::{OptSpec, Optimization};
use daedalus_core::sensitivity::{SensJob, SensOptions};
use daedalus_core::sim::SimOptions;
use daedalus_core::uq::{GsaJob, GsaSpec, Job};
use wasm_bindgen::prelude::*;

/// `SimOptions` from the model's experiment annotation, overridden by a JSON subset.
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

fn make_job(
    m: daedalus_core::codegen::Model,
    o: SimOptions,
    kind: &str,
    spec: &str,
) -> Result<Box<dyn Job>, String> {
    let bad = |e: serde_json::Error| format!("bad {kind} specification: {e}");
    Ok(match kind {
        "sensitivity" => {
            let so: SensOptions = serde_json::from_str(spec).map_err(bad)?;
            Box::new(SensJob::new(m, o, so)?)
        }
        "gsa" => {
            let s: GsaSpec = serde_json::from_str(spec).map_err(bad)?;
            Box::new(GsaJob::new(m, o, s)?)
        }
        "calibrate" => {
            let s: CalibSpec = serde_json::from_str(spec).map_err(bad)?;
            Box::new(Calibration::new(m, o, s)?)
        }
        "optimize" => {
            let s: OptSpec = serde_json::from_str(spec).map_err(bad)?;
            Box::new(Optimization::new(m, o, s)?)
        }
        "adjoint" | "gradient" => {
            let s: GradSpec = serde_json::from_str(spec).map_err(bad)?;
            Box::new(GradJob::new(m, o, s)?)
        }
        "doe" => {
            let s: DoeSpec = serde_json::from_str(spec).map_err(bad)?;
            Box::new(DoeJob::new(m, o, s)?)
        }
        _ => return Err(format!("unknown analysis '{kind}'")),
    })
}

#[wasm_bindgen]
impl Workspace {
    /// Translate `model` and plan an analysis: `kind` is sensitivity | gsa | calibrate |
    /// optimize, `spec` its JSON specification (see `daedalus_core`), `options` a JSON
    /// `SimOptions` subset. Returns `{"ok", "total"}` or `{"ok": false, "error"}`.
    #[wasm_bindgen(js_name = analysisStart)]
    pub fn analysis_start(&mut self, model: &str, kind: &str, spec: &str, options: &str) -> String {
        self.job = None;
        self.ensure(model, Some(true));
        // with the state-selection overrides of the Structure view, like a plain simulation
        let m = match daedalus_core::translate(&self.lib, model, &crate::backend_options(options)) {
            Ok(m) => m,
            Err(e) => return to_json(&serde_json::json!({"ok": false, "error": format!("{e:#}")})),
        };
        let o = sim_options(&m, options);
        match make_job(m, o, kind, spec) {
            Ok(j) => {
                let (d, t) = j.progress();
                self.job = Some(j);
                to_json(&serde_json::json!({"ok": true, "done": d, "total": t}))
            }
            Err(e) => to_json(&serde_json::json!({"ok": false, "error": e})),
        }
    }

    /// Advance the analysis for about `budget_ms` (at least one step). Returns
    /// `{"done": bool, "progress": [done, total]}` or `{"error"}`.
    #[wasm_bindgen(js_name = analysisStep)]
    pub fn analysis_step(&mut self, budget_ms: f64) -> String {
        let Some(job) = self.job.as_mut() else {
            return to_json(&serde_json::json!({"error": "no analysis running"}));
        };
        let t0 = now_ms();
        loop {
            match job.step() {
                Ok(true) => {
                    let (d, t) = job.progress();
                    return to_json(&serde_json::json!({"done": true, "progress": [d, t]}));
                }
                Ok(false) => {
                    if now_ms() - t0 >= budget_ms {
                        let (d, t) = job.progress();
                        return to_json(&serde_json::json!({"done": false, "progress": [d, t]}));
                    }
                }
                Err(e) => {
                    self.job = None;
                    return to_json(&serde_json::json!({"error": e}));
                }
            }
        }
    }

    /// JSON result of the analysis (complete once a step reported done), with its text
    /// report under `"report"`.
    #[wasm_bindgen(js_name = analysisResult)]
    pub fn analysis_result(&self) -> String {
        match &self.job {
            Some(j) => {
                let mut v = j.result_json();
                if let serde_json::Value::Object(o) = &mut v {
                    o.insert("report".into(), j.report().into());
                }
                // NaN and infinities become null in JSON.
                v.to_string()
            }
            None => "null".into(),
        }
    }

    /// Drop the running analysis.
    #[wasm_bindgen(js_name = analysisCancel)]
    pub fn analysis_cancel(&mut self) {
        self.job = None;
    }

    /// The quantities of interest `qois` (JSON array of `max(x)`-style strings) on the
    /// last simulation result: a JSON array of numbers (null where not finite or the
    /// variable is missing).
    #[wasm_bindgen(js_name = qoiValues)]
    pub fn qoi_values(&self, qois: &str) -> String {
        let qs: Vec<String> = serde_json::from_str(qois).unwrap_or_default();
        let vals: Vec<Option<f64>> = qs
            .iter()
            .map(|q| {
                let q = daedalus_core::uq::Qoi::parse(q).ok()?;
                let r = self.results.as_ref()?;
                let y = r.values(&q.var)?;
                let v = q.eval(&r.time, &y);
                v.is_finite().then_some(v)
            })
            .collect();
        to_json(&vals)
    }
}

/// Plan a design of experiments: `spec` is a JSON [`DoeSpec`]. Returns the JSON
/// [`DoePlan`] or `{"error"}`.
#[wasm_bindgen(js_name = doePlan)]
pub fn doe_plan(spec: &str) -> String {
    let r = serde_json::from_str::<DoeSpec>(spec)
        .map_err(|e| format!("bad design specification: {e}"))
        .and_then(|s| daedalus_core::doe::plan(&s));
    match r {
        Ok(p) => to_json(&p),
        Err(e) => to_json(&serde_json::json!({ "error": e })),
    }
}

/// Fit and summarize a design run elsewhere: `plan` from [`doe_plan`], `responses`
/// a JSON array per run of the quantity values (null for failed runs), `errors` a JSON
/// array of messages. Returns the JSON result with its text report under `"report"`.
#[wasm_bindgen(js_name = doeAnalyze)]
pub fn doe_analyze(model: &str, spec: &str, plan: &str, responses: &str, errors: &str) -> String {
    let run = || -> Result<serde_json::Value, String> {
        let spec: DoeSpec =
            serde_json::from_str(spec).map_err(|e| format!("bad design specification: {e}"))?;
        let plan: DoePlan = serde_json::from_str(plan).map_err(|e| format!("bad plan: {e}"))?;
        let resp: Vec<Option<Vec<Option<f64>>>> =
            serde_json::from_str(responses).map_err(|e| format!("bad responses: {e}"))?;
        let nq = spec.qois.len();
        let resp: Vec<Vec<f64>> = resp
            .into_iter()
            .map(|r| match r {
                Some(v) => v.into_iter().map(|x| x.unwrap_or(f64::NAN)).collect(),
                None => vec![f64::NAN; nq],
            })
            .collect();
        let errors: Vec<String> = serde_json::from_str(errors).unwrap_or_default();
        let r = daedalus_core::doe::analyze(model, &spec, &plan, &resp, errors)?;
        let mut v = serde_json::to_value(&r).map_err(|e| e.to_string())?;
        if let serde_json::Value::Object(o) = &mut v {
            o.insert("report".into(), r.report().into());
        }
        Ok(v)
    };
    match run() {
        Ok(v) => v.to_string(),
        Err(e) => to_json(&serde_json::json!({ "error": e })),
    }
}
