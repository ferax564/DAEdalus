//! Python extension module `daedalus._daedalus`.
//!
//! The thin native layer under the `daedalus` Python package (python/daedalus): it loads
//! sources the way the `daedalus` CLI does, translates a model once, and simulates,
//! linearizes and exports it on request. Trajectories and matrices cross as NumPy arrays;
//! statistics and metadata as plain dicts. The friendlier API (`Model`, `Result`,
//! `Linearization`) is written in Python on top of it.

use daedalus_core::backend::Options;
use daedalus_core::codegen::Model;
use daedalus_core::flat::{VarKind, VarType};
use daedalus_core::frontend::Library;
use daedalus_core::linearize::JacobianMethod;
use daedalus_core::results::{ResultKind, Results};
use daedalus_core::sim::{Method, SimOptions};
use numpy::{PyArray1, PyArray2, PyArrayMethods};
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyKeyError, PyValueError};
use pyo3::prelude::*;
use pyo3::types::{PyDict, PyList};
use std::path::Path;

create_exception!(
    _daedalus,
    DaedalusError,
    PyException,
    "Base class of DAEdalus errors."
);
create_exception!(
    _daedalus,
    TranslationError,
    DaedalusError,
    "Loading, flattening or compiling a model failed."
);
create_exception!(
    _daedalus,
    SimulationError,
    DaedalusError,
    "Initialization, integration or linearization failed."
);

/// A JSON value as Python objects (dict, list, float, int, str, bool, None).
fn json_to_py<'py>(py: Python<'py>, v: &serde_json::Value) -> PyResult<Bound<'py, PyAny>> {
    use serde_json::Value as V;
    Ok(match v {
        V::Null => py.None().into_bound(py),
        V::Bool(b) => b.into_pyobject(py)?.to_owned().into_any(),
        V::Number(n) => {
            if let Some(i) = n.as_i64() {
                i.into_pyobject(py)?.into_any()
            } else if let Some(u) = n.as_u64() {
                u.into_pyobject(py)?.into_any()
            } else {
                n.as_f64().unwrap_or(f64::NAN).into_pyobject(py)?.into_any()
            }
        }
        V::String(s) => s.into_pyobject(py)?.into_any(),
        V::Array(a) => {
            let l = PyList::empty(py);
            for x in a {
                l.append(json_to_py(py, x)?)?;
            }
            l.into_any()
        }
        V::Object(o) => {
            let d = PyDict::new(py);
            for (k, x) in o {
                d.set_item(k, json_to_py(py, x)?)?;
            }
            d.into_any()
        }
    })
}

fn to_py<'py, T: serde::Serialize>(py: Python<'py>, v: &T) -> PyResult<Bound<'py, PyAny>> {
    let j = serde_json::to_value(v).map_err(|e| PyValueError::new_err(e.to_string()))?;
    json_to_py(py, &j)
}

fn matrix<'py>(py: Python<'py>, rows: &[Vec<f64>], ncols: usize) -> Bound<'py, PyArray2<f64>> {
    let flat: Vec<f64> = rows.iter().flatten().copied().collect();
    PyArray1::from_vec(py, flat)
        .reshape([rows.len(), ncols])
        .expect("matrix shape")
}

/// Run `f`; if it fails while library files are still unparsed (a class the file scan
/// could not see), parse the rest and run it again (as the CLI does).
fn with_all_files<T>(
    lib: &mut Library,
    f: impl Fn(&Library) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let r = f(lib);
    if r.is_err() && lib.has_pending() {
        lib.load_all();
        return f(lib);
    }
    r
}

/// Load `paths` (files or library directories) and translate `name`. Parse errors are
/// returned alongside so they can be reported with a failed translation.
fn load_and_translate(
    paths: &[String],
    name: &str,
    opts: &Options,
) -> (anyhow::Result<Model>, Vec<String>) {
    let mut lib = Library::new();
    let mut parse_errors = Vec::new();
    for s in paths {
        let p = Path::new(s);
        if p.is_dir() {
            if let Err(e) = lib.add_dir_lazy(p) {
                return (Err(anyhow::anyhow!("{s}: {e:#}")), parse_errors);
            }
        } else {
            match std::fs::read_to_string(p) {
                Ok(text) => parse_errors.extend(lib.set_document(s, &text)),
                Err(e) => return (Err(anyhow::anyhow!("{s}: {e}")), parse_errors),
            }
        }
    }
    if lib.has_pending() {
        let before = lib.docs.len();
        lib.load_for(name);
        for d in &lib.docs[before..] {
            parse_errors.extend(d.errors.iter().take(3).cloned());
        }
    }
    let r = with_all_files(&mut lib, |l| daedalus_core::translate(l, name, opts));
    (r, parse_errors)
}

/// Simulation options: the experiment annotation, then the given overrides (mirrors the
/// `daedalus simulate` flags).
#[allow(clippy::too_many_arguments)]
fn sim_options(
    m: &Model,
    start: Option<f64>,
    stop: Option<f64>,
    n: Option<usize>,
    interval: Option<f64>,
    tol: Option<f64>,
    method: Option<&str>,
    step: Option<f64>,
    params: Vec<(String, f64)>,
    store_events: Option<bool>,
    max_wall_time: Option<f64>,
) -> PyResult<SimOptions> {
    let mut o = SimOptions::from_model(m);
    if let Some(t) = start {
        o.start_time = t;
    }
    if let Some(t) = stop {
        o.stop_time = t;
    }
    if let Some(t) = tol {
        o.tolerance = t;
    }
    if let Some(s) = method {
        o.method = Method::parse(s).ok_or_else(|| {
            PyValueError::new_err(format!("unknown method '{s}' (bdf | dopri5 | rk4 | euler)"))
        })?;
    }
    match (interval, n) {
        (Some(dt), _) => o.interval = dt,
        // An explicit interval count replaces the experiment's Interval.
        (None, Some(n)) => {
            o.interval = 0.0;
            o.intervals = n;
        }
        (None, None) => {}
    }
    if let Some(h) = step {
        o.fixed_step = h;
    }
    if let Some(b) = store_events {
        o.store_events = b;
    }
    if let Some(t) = max_wall_time {
        o.max_wall_time = t;
    }
    o.params = params;
    Ok(o)
}

fn kind_name(k: VarKind) -> &'static str {
    match k {
        VarKind::Constant => "constant",
        VarKind::Parameter => "parameter",
        VarKind::Input => "input",
        VarKind::Discrete => "discrete",
        VarKind::Continuous => "continuous",
    }
}

fn type_name(t: VarType) -> &'static str {
    match t {
        VarType::Real => "Real",
        VarType::Integer => "Integer",
        VarType::Boolean => "Boolean",
        VarType::Enumeration => "enumeration",
    }
}

/// A translated model (native part of `daedalus.Model`).
#[pyclass(module = "daedalus._daedalus", frozen)]
struct CompiledModel {
    model: Model,
    parse_errors: Vec<String>,
}

#[pymethods]
impl CompiledModel {
    /// Load `paths` (.mo files and library directories) and translate model `name`.
    #[new]
    #[pyo3(signature = (paths, name, index_reduction = true, tearing = true))]
    fn new(
        py: Python<'_>,
        paths: Vec<String>,
        name: String,
        index_reduction: bool,
        tearing: bool,
    ) -> PyResult<Self> {
        let opts = Options {
            index_reduction,
            tearing,
        };
        let (r, parse_errors) = py.detach(|| load_and_translate(&paths, &name, &opts));
        match r {
            Ok(model) => Ok(CompiledModel {
                model,
                parse_errors,
            }),
            Err(e) => {
                let mut msg = format!("{e:#}");
                if !parse_errors.is_empty() {
                    msg.push_str("\nparse errors:\n");
                    msg.push_str(&parse_errors.join("\n"));
                }
                Err(TranslationError::new_err(msg))
            }
        }
    }

    #[getter]
    fn name(&self) -> &str {
        &self.model.name
    }

    #[getter]
    fn warnings(&self) -> Vec<String> {
        self.model.warnings.clone()
    }

    #[getter]
    fn parse_errors(&self) -> Vec<String> {
        self.parse_errors.clone()
    }

    /// The `experiment` annotation: start_time, stop_time, tolerance, interval, algorithm.
    #[getter]
    fn experiment<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        to_py(py, &self.model.experiment)
    }

    /// Translation statistics (as printed by `daedalus check`).
    #[getter]
    fn stats<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyAny>> {
        let d = to_py(py, &self.model.stats)?;
        d.set_item("tape_len", self.model.tape_len)?;
        Ok(d)
    }

    /// Names of the selected states.
    #[getter]
    fn states(&self) -> Vec<String> {
        let m = &self.model;
        m.states
            .iter()
            .map(|&s| m.vars[s as usize].name.clone())
            .collect()
    }

    /// One dict per model variable (compiler-generated `$` variables left out).
    fn variables<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyList>> {
        let m = &self.model;
        let mut is_state = vec![false; m.vars.len()];
        for &s in &m.states {
            is_state[s as usize] = true;
        }
        let mut free = vec![false; m.vars.len()];
        for &p in &m.free_params {
            free[p as usize] = true;
        }
        let values = daedalus_core::fmi::parameter_values(m);
        let out = PyList::empty(py);
        for (i, v) in m.vars.iter().enumerate() {
            if v.name.starts_with('$') {
                continue;
            }
            let d = PyDict::new(py);
            d.set_item("name", &v.name)?;
            d.set_item("kind", kind_name(v.kind))?;
            d.set_item("type", type_name(v.ty))?;
            d.set_item("unit", &v.unit)?;
            d.set_item("display_unit", &v.display_unit)?;
            d.set_item("description", &v.description)?;
            d.set_item("state", is_state[i])?;
            d.set_item("output", v.is_output)?;
            d.set_item("protected", v.is_protected)?;
            d.set_item("final", v.is_final)?;
            d.set_item("free", free[i])?;
            let value = matches!(
                v.kind,
                VarKind::Parameter | VarKind::Constant | VarKind::Input
            )
            .then(|| values.get(i).copied())
            .flatten();
            d.set_item("value", value)?;
            out.append(d)?;
        }
        Ok(out)
    }

    /// Default values of the parameters (bindings evaluated) as a dict.
    fn parameter_values<'py>(&self, py: Python<'py>) -> PyResult<Bound<'py, PyDict>> {
        let m = &self.model;
        let values = daedalus_core::fmi::parameter_values(m);
        let d = PyDict::new(py);
        for &p in &m.param_vars {
            let v = &m.vars[p as usize];
            if !v.name.starts_with('$') {
                d.set_item(&v.name, values[p as usize])?;
            }
        }
        Ok(d)
    }

    /// Simulate (the GIL is released meanwhile). Returns `(results, stats, error)`:
    /// on failure `stats` is None, `error` the message and `results` the partial result
    /// (or None).
    #[pyo3(signature = (start=None, stop=None, n=None, interval=None, tol=None, method=None,
                        step=None, params=Vec::new(), store_events=None, max_wall_time=None))]
    #[allow(clippy::too_many_arguments)]
    fn simulate<'py>(
        &self,
        py: Python<'py>,
        start: Option<f64>,
        stop: Option<f64>,
        n: Option<usize>,
        interval: Option<f64>,
        tol: Option<f64>,
        method: Option<String>,
        step: Option<f64>,
        params: Vec<(String, f64)>,
        store_events: Option<bool>,
        max_wall_time: Option<f64>,
    ) -> PyResult<(Option<SimResult>, Bound<'py, PyAny>, Option<String>)> {
        let o = sim_options(
            &self.model,
            start,
            stop,
            n,
            interval,
            tol,
            method.as_deref(),
            step,
            params,
            store_events,
            max_wall_time,
        )?;
        let m = &self.model;
        let r = py.detach(|| daedalus_core::sim::simulate(m, o).map_err(Box::new));
        Ok(match r {
            Ok((res, stats)) => (Some(SimResult { res }), to_py(py, &stats)?, None),
            Err(e) => (
                e.partial.map(|res| SimResult { res }),
                py.None().into_bound(py),
                Some(e.message),
            ),
        })
    }

    /// Linearize after initialization or at time `at`; returns a dict with A, B, C, D
    /// (NumPy arrays) and the state, input and output names.
    #[pyo3(signature = (at=None, start=None, tol=None, method=None, params=Vec::new(),
                        jacobian="exact"))]
    #[allow(clippy::too_many_arguments)]
    fn linearize<'py>(
        &self,
        py: Python<'py>,
        at: Option<f64>,
        start: Option<f64>,
        tol: Option<f64>,
        method: Option<String>,
        params: Vec<(String, f64)>,
        jacobian: &str,
    ) -> PyResult<Bound<'py, PyDict>> {
        let jac = match jacobian {
            "exact" => JacobianMethod::Exact,
            "fd" | "cd" | "central" => JacobianMethod::CentralDifference,
            j => {
                return Err(PyValueError::new_err(format!(
                    "unknown jacobian method '{j}' (exact | fd)"
                )));
            }
        };
        let o = sim_options(
            &self.model,
            start,
            None,
            None,
            None,
            tol,
            method.as_deref(),
            None,
            params,
            None,
            None,
        )?;
        let m = &self.model;
        let lin = py
            .detach(|| daedalus_core::linearize::linearize(m, o, at, jac))
            .map_err(SimulationError::new_err)?;
        let (nx, nu) = (lin.states.len(), lin.inputs.len());
        let d = PyDict::new(py);
        d.set_item("model", &lin.model)?;
        d.set_item("time", lin.time)?;
        d.set_item("states", &lin.states)?;
        d.set_item("inputs", &lin.inputs)?;
        d.set_item("outputs", &lin.outputs)?;
        d.set_item("A", matrix(py, &lin.a, nx))?;
        d.set_item("B", matrix(py, &lin.b, nu))?;
        d.set_item("C", matrix(py, &lin.c, nx))?;
        d.set_item("D", matrix(py, &lin.d, nu))?;
        d.set_item("x0", PyArray1::from_slice(py, &lin.x0))?;
        d.set_item("u0", PyArray1::from_slice(py, &lin.u0))?;
        d.set_item("y0", PyArray1::from_slice(py, &lin.y0))?;
        d.set_item("dx0", PyArray1::from_slice(py, &lin.dx0))?;
        d.set_item(
            "jacobian",
            match lin.method {
                JacobianMethod::Exact => "exact",
                JacobianMethod::CentralDifference => "fd",
            },
        )?;
        d.set_item("report", lin.report())?;
        d.set_item("modelica", lin.to_modelica())?;
        Ok(d)
    }

    /// Write an FMI 2.0 FMU (Model Exchange and Co-Simulation) to `path`, embedding the
    /// FMI runtime library at `runtime`. Returns a summary dict.
    fn export_fmu<'py>(
        &self,
        py: Python<'py>,
        path: std::path::PathBuf,
        runtime: std::path::PathBuf,
    ) -> PyResult<Bound<'py, PyDict>> {
        let bytes = std::fs::read(&runtime).map_err(|e| {
            PyValueError::new_err(format!("FMI runtime {}: {e}", runtime.display()))
        })?;
        let m = &self.model;
        let desc = py
            .detach(|| {
                daedalus_core::fmu::write_fmu(
                    m,
                    &path,
                    &bytes,
                    &format!("DAEdalus {} (Python)", env!("CARGO_PKG_VERSION")),
                )
            })
            .map_err(|e| DaedalusError::new_err(format!("{e:#}")))?;
        let d = PyDict::new(py);
        d.set_item(
            "model_identifier",
            daedalus_core::fmi::model_identifier(&m.name),
        )?;
        d.set_item("variables", desc.vars.len())?;
        d.set_item("states", desc.n_states)?;
        d.set_item("event_indicators", desc.n_event_indicators)?;
        d.set_item(
            "inputs",
            desc.vars
                .iter()
                .filter(|v| v.causality == daedalus_core::fmi::Causality::Input)
                .count(),
        )?;
        d.set_item("outputs", desc.outputs.len())?;
        Ok(d)
    }
}

/// A simulation result (native part of `daedalus.Result`).
#[pyclass(module = "daedalus._daedalus", frozen)]
struct SimResult {
    res: Results,
}

#[pymethods]
impl SimResult {
    #[getter]
    fn model(&self) -> &str {
        &self.res.model
    }

    /// Variable names in result order (compiler-generated `$` variables left out).
    fn names(&self) -> Vec<String> {
        self.res
            .vars
            .iter()
            .filter(|v| !v.name.starts_with('$'))
            .map(|v| v.name.clone())
            .collect()
    }

    /// `(name, kind, unit, description)` per variable; kind is "series", "param" or
    /// "alias".
    fn info(&self) -> Vec<(String, &'static str, String, String)> {
        self.res
            .vars
            .iter()
            .filter(|v| !v.name.starts_with('$'))
            .map(|v| {
                let k = match v.kind {
                    ResultKind::Series(_) => "series",
                    ResultKind::Param(_) => "param",
                    ResultKind::Alias { .. } => "alias",
                };
                (v.name.clone(), k, v.unit.clone(), v.description.clone())
            })
            .collect()
    }

    fn time<'py>(&self, py: Python<'py>) -> Bound<'py, PyArray1<f64>> {
        PyArray1::from_slice(py, &self.res.time)
    }

    fn __len__(&self) -> usize {
        self.res.time.len()
    }

    fn __contains__(&self, name: &str) -> bool {
        self.res.find(name).is_some()
    }

    /// Trajectory of one variable on the result time grid (parameters as a constant).
    fn values<'py>(&self, py: Python<'py>, name: &str) -> PyResult<Bound<'py, PyArray1<f64>>> {
        let v = self
            .res
            .find(name)
            .ok_or_else(|| PyKeyError::new_err(name.to_string()))?;
        Ok(PyArray1::from_vec(py, self.res.values_of(v)))
    }

    /// Final value of a variable.
    fn final_value(&self, name: &str) -> PyResult<f64> {
        self.res
            .final_value(name)
            .ok_or_else(|| PyKeyError::new_err(name.to_string()))
    }

    /// Linear interpolation of a variable at time `t`.
    fn at(&self, name: &str, t: f64) -> PyResult<f64> {
        self.res
            .at(name, t)
            .ok_or_else(|| PyKeyError::new_err(name.to_string()))
    }

    #[getter]
    fn log(&self) -> Vec<String> {
        self.res.log.clone()
    }

    /// Write a CSV file (all variables, or `names`).
    #[pyo3(signature = (path, names=None))]
    fn write_csv(&self, path: std::path::PathBuf, names: Option<Vec<String>>) -> PyResult<()> {
        let f = std::fs::File::create(&path)?;
        let mut w = std::io::BufWriter::new(f);
        self.res.write_csv(&mut w, names.as_deref())?;
        Ok(())
    }

    /// Write a Dymola-compatible `.mat` result file.
    fn write_mat(&self, path: std::path::PathBuf) -> PyResult<()> {
        let f = std::fs::File::create(&path)?;
        let mut w = std::io::BufWriter::new(f);
        self.res.write_mat(&mut w)?;
        Ok(())
    }
}

/// Read a Dymola/DAEdalus `.mat` result file.
#[pyfunction]
fn read_mat(path: std::path::PathBuf) -> PyResult<SimResult> {
    let bytes = std::fs::read(&path)?;
    let res = daedalus_core::results::read_mat(&bytes).map_err(PyValueError::new_err)?;
    Ok(SimResult { res })
}

#[pymodule]
fn _daedalus(m: &Bound<'_, PyModule>) -> PyResult<()> {
    let py = m.py();
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("DaedalusError", py.get_type::<DaedalusError>())?;
    m.add("TranslationError", py.get_type::<TranslationError>())?;
    m.add("SimulationError", py.get_type::<SimulationError>())?;
    m.add_class::<CompiledModel>()?;
    m.add_class::<SimResult>()?;
    m.add_function(wrap_pyfunction!(read_mat, m)?)?;
    Ok(())
}
