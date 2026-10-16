//! FMI 2.0 and FMI 3.0 runtime of FMUs exported by `daedalus fmu` (FMI 3.0 in [`fmi3`]).
//!
//! This shared library implements the `fmi2*` C functions for both Model Exchange
//! and Co-Simulation. It is model independent: at `fmi2Instantiate` it loads the
//! compiled model (the register tape of `daedalus-core`) that the exporter serialized
//! into the FMU's `resources/model.bin`.
//!
//! - Model Exchange exposes the selected states, their derivatives and one event
//!   indicator per relation (`lhs - rhs`); relations stay latched in continuous-time
//!   mode and are updated by the event iteration in `fmi2NewDiscreteStates`.
//! - Co-Simulation integrates with the model's own solver (BDF by default, or the
//!   experiment's algorithm) with zero-crossing and time events inside `fmi2DoStep`.
//!
//! Value references are variable indices (see `daedalus_core::fmi`).

#![allow(non_snake_case, non_camel_case_types, clippy::missing_safety_doc)]

use daedalus_core::codegen::Model;
use daedalus_core::flat::VarKind;
use daedalus_core::fmi::Bundle;
use daedalus_core::sim::{SimOptions, Simulation, Stepper};
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::panic::{AssertUnwindSafe, catch_unwind};

pub mod fmi3;

pub type fmi2Component = *mut c_void;
pub type fmi2Status = c_int;
pub type fmi2Boolean = c_int;
pub type fmi2Real = f64;
pub type fmi2Integer = c_int;
pub type fmi2ValueReference = u32;
pub type fmi2String = *const c_char;

const OK: fmi2Status = 0;
const WARNING: fmi2Status = 1;
const DISCARD: fmi2Status = 2;
const ERROR: fmi2Status = 3;
const FATAL: fmi2Status = 4;

const MODEL_EXCHANGE: c_int = 0;

type Logger =
    unsafe extern "C" fn(*mut c_void, fmi2String, fmi2Status, fmi2String, fmi2String, ...);

#[repr(C)]
pub struct fmi2CallbackFunctions {
    logger: Option<Logger>,
    allocate_memory: *const c_void,
    free_memory: *const c_void,
    step_finished: *const c_void,
    component_environment: *mut c_void,
}

#[repr(C)]
pub struct fmi2EventInfo {
    newDiscreteStatesNeeded: fmi2Boolean,
    terminateSimulation: fmi2Boolean,
    nominalsOfContinuousStatesChanged: fmi2Boolean,
    valuesOfContinuousStatesChanged: fmi2Boolean,
    nextEventTimeDefined: fmi2Boolean,
    nextEventTime: fmi2Real,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    Instantiated,
    Initialization,
    Event,
    Continuous,
    StepComplete,
    Terminated,
    Error,
}

/// The importer's logging callback: FMI 2 (variadic) or FMI 3.
#[derive(Clone, Copy)]
enum Log {
    V2(Logger),
    V3(fmi3::LogMessage),
}

struct Instance {
    name: CString,
    logger: Option<Log>,
    env: *mut c_void,
    logging: bool,
    model_exchange: bool,
    model: &'static Model,
    sim: Simulation<'static>,
    stepper: Option<Stepper>,
    opts: SimOptions,
    mode: Mode,
    /// Initialization must be (re)computed before values are read.
    init_dirty: bool,
    /// The algebraic variables must be recomputed before values are read.
    dirty: bool,
    /// Inputs changed since the last communication point (Co-Simulation).
    inputs_changed: bool,
    terminated: bool,
    last_time: f64,
    is_free_param: Vec<bool>,
    /// Largest |lhs - rhs| seen per relation (sets the event indicator offset).
    zc_scale: Vec<f64>,
}

impl Instance {
    fn log(&self, status: fmi2Status, category: &str, msg: &str) {
        if let Some(l) = self.logger
            && (self.logging || status >= WARNING)
        {
            let cat = CString::new(category).unwrap_or_default();
            let m = CString::new(msg.replace('\0', " ")).unwrap_or_default();
            let fmt = c"%s";
            match l {
                Log::V2(l) => unsafe {
                    l(
                        self.env,
                        self.name.as_ptr(),
                        status,
                        cat.as_ptr(),
                        fmt.as_ptr(),
                        m.as_ptr(),
                    )
                },
                Log::V3(l) => unsafe { l(self.env, status, cat.as_ptr(), m.as_ptr()) },
            }
        }
    }

    fn error(&mut self, msg: &str) -> fmi2Status {
        self.log(ERROR, "logStatusError", msg);
        ERROR
    }

    /// Run the initialization with the current parameter and input values.
    fn initialize(&mut self) -> fmi2Status {
        self.sim = Simulation::new(self.model, self.opts.clone());
        self.stepper = None;
        match self.sim.initialize() {
            Ok(()) => {
                let m = self.model;
                self.sim.rt.run(&m.outputs);
                self.init_dirty = false;
                self.dirty = false;
                self.last_time = self.sim.rt.time();
                OK
            }
            Err(e) => {
                self.mode = Mode::Error;
                self.error(&format!("initialization failed: {e}"))
            }
        }
    }

    /// Make the values in the registers consistent before they are read.
    fn update(&mut self) -> fmi2Status {
        if matches!(self.mode, Mode::Instantiated | Mode::Initialization) {
            if self.init_dirty {
                return self.initialize();
            }
            return OK;
        }
        if self.dirty {
            let m = self.model;
            self.sim.rt.run(&m.ode);
            self.sim.rt.run(&m.outputs);
            self.dirty = false;
            if let Some(e) = self.sim.rt.error.take() {
                // An integrator's trial point can be outside where the loop converges:
                // report it and return the best iterate (the integrator's error test
                // rejects the step if it matters), as the built-in solvers do.
                let names = m.newton[e.block].names.join(", ");
                self.log(
                    WARNING,
                    "logStatusWarning",
                    &format!(
                        "nonlinear system for [{names}] did not converge at t = {} (residual {:.3e})",
                        self.sim.rt.time(),
                        e.residual
                    ),
                );
                return WARNING;
            }
        }
        OK
    }

    fn set_override(&mut self, name: &str, v: f64) {
        match self.opts.params.iter_mut().find(|(n, _)| n == name) {
            Some(p) => p.1 = v,
            None => self.opts.params.push((name.to_string(), v)),
        }
    }

    fn set_value(&mut self, vr: u32, v: f64) -> fmi2Status {
        let m = self.model;
        let Some(var) = m.vars.get(vr as usize) else {
            return self.error(&format!("unknown value reference {vr}"));
        };
        let before_init = matches!(self.mode, Mode::Instantiated | Mode::Initialization);
        match var.kind {
            VarKind::Input => {
                if before_init {
                    self.set_override(&var.name, v);
                    self.init_dirty = true;
                } else {
                    if self.sim.rt.r[vr as usize] != v {
                        self.inputs_changed = true;
                    }
                    self.sim.rt.r[vr as usize] = v;
                    self.dirty = true;
                }
                OK
            }
            VarKind::Parameter if self.is_free_param[vr as usize] && before_init => {
                self.set_override(&var.name, v);
                self.init_dirty = true;
                OK
            }
            _ => self.error(&format!(
                "variable '{}' (value reference {vr}) cannot be set{}",
                var.name,
                if before_init {
                    ""
                } else {
                    " after initialization"
                }
            )),
        }
    }

    fn get_value(&mut self, vr: u32) -> Option<f64> {
        self.sim.rt.r[..self.model.vars.len()]
            .get(vr as usize)
            .copied()
    }

    fn exit_initialization(&mut self) -> fmi2Status {
        if self.init_dirty
            || self.mode == Mode::Instantiated
            || self.sim.rt.time() != self.opts.start_time
        {
            let s = self.initialize();
            if s != OK {
                return s;
            }
        }
        if self.model_exchange {
            self.mode = Mode::Event;
        } else {
            self.stepper = Some(self.sim.stepper());
            self.mode = Mode::StepComplete;
        }
        OK
    }

    fn do_step(&mut self, t: f64, h: f64) -> fmi2Status {
        if self.mode != Mode::StepComplete {
            return self.error(&format!("fmi2DoStep called in mode {:?}", self.mode));
        }
        let Some(mut st) = self.stepper.take() else {
            return self.error("fmi2DoStep: not initialized");
        };
        let tol = 1e-9 * t.abs().max(h.abs()).max(1.0);
        if (st.t - t).abs() > tol {
            self.log(
                WARNING,
                "logStatusWarning",
                &format!(
                    "fmi2DoStep at t = {t} but the FMU is at t = {}; continuing from there",
                    st.t
                ),
            );
        }
        if self.inputs_changed || self.dirty {
            // Inputs jumped: algebraics, then restart the integrator (a discontinuity);
            // relations they switch are handled as events at the start of the step.
            let m = self.model;
            self.sim.rt.set_time(st.t);
            self.sim.rt.run(&m.ode);
            self.sim.restart(&mut st);
            self.inputs_changed = false;
            self.dirty = false;
        }
        let r = self.sim.advance_to(&mut st, t + h);
        self.last_time = st.t;
        self.stepper = Some(st);
        match r {
            Err(e) => {
                self.mode = Mode::Error;
                self.error(&format!("fmi2DoStep failed: {e}"))
            }
            Ok(()) => {
                if let Some(msg) = self.sim.terminated() {
                    let msg = msg.to_string();
                    self.terminated = true;
                    self.log(DISCARD, "logStatusDiscard", &format!("terminate(): {msg}"));
                    DISCARD
                } else {
                    OK
                }
            }
        }
    }
}

impl Instance {
    /// Offset of event indicator `k`: the indicator is `lhs - rhs` shifted by this, so
    /// that its root lies just past the switching point of the relation, where the
    /// relation has its new value (the integrator's root may sit exactly on the
    /// switching point or on its old side). Changing the latched value flips the
    /// offset: a band of twice its width separates switching back.
    fn zc_offset(&self, k: usize) -> f64 {
        use daedalus_core::expr::Cmp;
        let m = self.model;
        let latched = self.sim.rt.r[m.layout.rel0 + k] != 0.0;
        let delta = 1e-10 * (1.0 + self.zc_scale[k]);
        let side = match m.rel_ops[k] {
            Cmp::Gt | Cmp::Ge => {
                if latched {
                    1.0
                } else {
                    -1.0
                }
            }
            Cmp::Lt | Cmp::Le => {
                if latched {
                    -1.0
                } else {
                    1.0
                }
            }
            _ => 0.0,
        };
        side * delta
    }
}

fn read_bundle(resource_location: &str) -> Result<Bundle, String> {
    let path = uri_to_path(resource_location);
    let file = std::path::Path::new(&path).join("model.bin");
    let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    bincode::deserialize(&bytes).map_err(|e| format!("{}: {e}", file.display()))
}

/// `file:///a/b%20c` -> `/a/b c` (also `file:/a/b` and plain paths).
fn uri_to_path(uri: &str) -> String {
    let rest = if let Some(r) = uri.strip_prefix("file://") {
        // file://host/path: drop a host name (localhost)
        match r.find('/') {
            Some(0) => r,
            Some(i) => &r[i..],
            None => r,
        }
    } else if let Some(r) = uri.strip_prefix("file:") {
        r
    } else {
        uri
    };
    let b = rest.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && i + 2 < b.len()
            && let Ok(v) =
                u8::from_str_radix(std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or(""), 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

unsafe fn cstr<'a>(p: fmi2String) -> &'a str {
    if p.is_null() {
        ""
    } else {
        unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
    }
}

/// Run `f` on the instance behind `c`, turning a panic into fmi2Fatal.
fn with(c: fmi2Component, f: impl FnOnce(&mut Instance) -> fmi2Status) -> fmi2Status {
    if c.is_null() {
        return ERROR;
    }
    let inst = unsafe { &mut *(c as *mut Instance) };
    match catch_unwind(AssertUnwindSafe(|| f(inst))) {
        Ok(s) => s,
        Err(p) => {
            let msg = p
                .downcast_ref::<String>()
                .map(|s| s.as_str())
                .or_else(|| p.downcast_ref::<&str>().copied())
                .unwrap_or("panic");
            inst.mode = Mode::Error;
            inst.log(FATAL, "logStatusFatal", &format!("internal error: {msg}"));
            FATAL
        }
    }
}

unsafe fn slice<'a, T>(p: *const T, n: usize) -> &'a [T] {
    if n == 0 || p.is_null() {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(p, n) }
    }
}

unsafe fn slice_mut<'a, T>(p: *mut T, n: usize) -> &'a mut [T] {
    if n == 0 || p.is_null() {
        &mut []
    } else {
        unsafe { std::slice::from_raw_parts_mut(p, n) }
    }
}

// ---------------------------------------------------------------------------------------
// Common functions
// ---------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn fmi2GetTypesPlatform() -> *const c_char {
    c"default".as_ptr()
}

#[unsafe(no_mangle)]
pub extern "C" fn fmi2GetVersion() -> *const c_char {
    c"2.0".as_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetDebugLogging(
    c: fmi2Component,
    logging_on: fmi2Boolean,
    _n: usize,
    _categories: *const fmi2String,
) -> fmi2Status {
    with(c, |i| {
        i.logging = logging_on != 0;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2Instantiate(
    instance_name: fmi2String,
    fmu_type: c_int,
    guid: fmi2String,
    resource_location: fmi2String,
    functions: *const fmi2CallbackFunctions,
    _visible: fmi2Boolean,
    logging_on: fmi2Boolean,
) -> fmi2Component {
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        let (logger, env) = if functions.is_null() {
            (None, std::ptr::null_mut())
        } else {
            (
                (*functions).logger.map(Log::V2),
                (*functions).component_environment,
            )
        };
        new_instance(
            cstr(instance_name),
            fmu_type == MODEL_EXCHANGE,
            cstr(guid),
            cstr(resource_location),
            logger,
            env,
            logging_on != 0,
        )
    }));
    r.unwrap_or(std::ptr::null_mut())
}

/// Load the model of an FMU and create an instance (shared by FMI 2 and FMI 3).
fn new_instance(
    name: &str,
    model_exchange: bool,
    guid: &str,
    resource_location: &str,
    logger: Option<Log>,
    env: *mut c_void,
    logging: bool,
) -> *mut c_void {
    let report = |msg: &str| {
        let n = CString::new(name).unwrap_or_default();
        let m = CString::new(msg).unwrap_or_default();
        match logger {
            Some(Log::V2(l)) => unsafe {
                l(
                    env,
                    n.as_ptr(),
                    ERROR,
                    c"logStatusError".as_ptr(),
                    c"%s".as_ptr(),
                    m.as_ptr(),
                )
            },
            Some(Log::V3(l)) => unsafe { l(env, ERROR, c"logStatusError".as_ptr(), m.as_ptr()) },
            None => {}
        }
    };
    let bundle = match read_bundle(resource_location) {
        Ok(b) => b,
        Err(e) => {
            report(&format!("cannot load the model: {e}"));
            return std::ptr::null_mut();
        }
    };
    if !guid.is_empty() && guid != bundle.guid {
        report(&format!(
            "GUID mismatch: {guid} given, the FMU's model is {}",
            bundle.guid
        ));
        return std::ptr::null_mut();
    }
    let model: &'static Model = Box::leak(Box::new(bundle.model));
    let opts = SimOptions {
        store_events: false,
        ..SimOptions::from_model(model)
    };
    let mut is_free_param = vec![false; model.vars.len()];
    for &p in &model.free_params {
        if model.vars[p as usize].kind == VarKind::Parameter && !model.vars[p as usize].is_final {
            is_free_param[p as usize] = true;
        }
    }
    let inst = Box::new(Instance {
        name: CString::new(name).unwrap_or_default(),
        logger,
        env,
        logging,
        model_exchange,
        model,
        sim: Simulation::new(model, opts.clone()),
        stepper: None,
        opts,
        mode: Mode::Instantiated,
        init_dirty: true,
        dirty: false,
        inputs_changed: false,
        terminated: false,
        last_time: 0.0,
        is_free_param,
        zc_scale: vec![0.0; model.layout.n_rel],
    });
    Box::into_raw(inst) as *mut c_void
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2FreeInstance(c: fmi2Component) {
    if c.is_null() {
        return;
    }
    let _ = catch_unwind(AssertUnwindSafe(|| unsafe {
        let inst = Box::from_raw(c as *mut Instance);
        let model = inst.model as *const Model as *mut Model;
        drop(inst);
        drop(Box::from_raw(model));
    }));
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetupExperiment(
    c: fmi2Component,
    tolerance_defined: fmi2Boolean,
    tolerance: fmi2Real,
    start_time: fmi2Real,
    stop_time_defined: fmi2Boolean,
    stop_time: fmi2Real,
) -> fmi2Status {
    with(c, |i| {
        if i.mode != Mode::Instantiated {
            return i.error("fmi2SetupExperiment must be called before initialization");
        }
        if tolerance_defined != 0 && tolerance > 0.0 {
            i.opts.tolerance = tolerance;
        }
        let span = i.opts.stop_time - i.opts.start_time;
        i.opts.start_time = start_time;
        i.opts.stop_time = if stop_time_defined != 0 && stop_time > start_time {
            stop_time
        } else {
            start_time + span.max(1.0)
        };
        i.init_dirty = true;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2EnterInitializationMode(c: fmi2Component) -> fmi2Status {
    with(c, |i| {
        if i.mode != Mode::Instantiated {
            return i.error("fmi2EnterInitializationMode called twice");
        }
        i.mode = Mode::Initialization;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2ExitInitializationMode(c: fmi2Component) -> fmi2Status {
    with(c, |i| {
        if i.mode != Mode::Initialization {
            return i.error("fmi2ExitInitializationMode outside initialization mode");
        }
        i.exit_initialization()
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2Terminate(c: fmi2Component) -> fmi2Status {
    with(c, |i| {
        i.mode = Mode::Terminated;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2Reset(c: fmi2Component) -> fmi2Status {
    with(c, |i| {
        i.opts = SimOptions {
            store_events: false,
            ..SimOptions::from_model(i.model)
        };
        i.sim = Simulation::new(i.model, i.opts.clone());
        i.stepper = None;
        i.mode = Mode::Instantiated;
        i.init_dirty = true;
        i.dirty = false;
        i.inputs_changed = false;
        i.terminated = false;
        OK
    })
}

unsafe fn get_values<T>(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *mut T,
    conv: impl Fn(f64) -> T,
) -> fmi2Status {
    with(c, |i| {
        let (vr, value) = unsafe { (slice(vr, nvr), slice_mut(value, nvr)) };
        if nvr == 0 {
            return OK;
        }
        let s = i.update();
        if s > WARNING {
            return s;
        }
        for (k, &r) in vr.iter().enumerate() {
            match i.get_value(r) {
                Some(v) => value[k] = conv(v),
                None => return i.error(&format!("unknown value reference {r}")),
            }
        }
        OK
    })
}

unsafe fn set_values<T: Copy>(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *const T,
    conv: impl Fn(T) -> f64,
) -> fmi2Status {
    with(c, |i| {
        let (vr, value) = unsafe { (slice(vr, nvr), slice(value, nvr)) };
        for (k, &r) in vr.iter().enumerate() {
            let s = i.set_value(r, conv(value[k]));
            if s != OK {
                return s;
            }
        }
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetReal(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *mut fmi2Real,
) -> fmi2Status {
    unsafe { get_values(c, vr, nvr, value, |v| v) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetInteger(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *mut fmi2Integer,
) -> fmi2Status {
    unsafe { get_values(c, vr, nvr, value, |v| v.round() as fmi2Integer) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetBoolean(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *mut fmi2Boolean,
) -> fmi2Status {
    unsafe { get_values(c, vr, nvr, value, |v| (v != 0.0) as fmi2Boolean) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetString(
    c: fmi2Component,
    _vr: *const fmi2ValueReference,
    nvr: usize,
    _value: *mut fmi2String,
) -> fmi2Status {
    with(c, |i| {
        if nvr == 0 {
            OK
        } else {
            i.error("the model has no String variables")
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetReal(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *const fmi2Real,
) -> fmi2Status {
    unsafe { set_values(c, vr, nvr, value, |v| v) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetInteger(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *const fmi2Integer,
) -> fmi2Status {
    unsafe { set_values(c, vr, nvr, value, |v| v as f64) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetBoolean(
    c: fmi2Component,
    vr: *const fmi2ValueReference,
    nvr: usize,
    value: *const fmi2Boolean,
) -> fmi2Status {
    unsafe { set_values(c, vr, nvr, value, |v| if v != 0 { 1.0 } else { 0.0 }) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetString(
    c: fmi2Component,
    _vr: *const fmi2ValueReference,
    nvr: usize,
    _value: *const fmi2String,
) -> fmi2Status {
    with(c, |i| {
        if nvr == 0 {
            OK
        } else {
            i.error("the model has no String variables")
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetFMUstate(c: fmi2Component, _s: *mut *mut c_void) -> fmi2Status {
    with(c, |i| i.error("fmi2GetFMUstate is not supported"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetFMUstate(c: fmi2Component, _s: *mut c_void) -> fmi2Status {
    with(c, |i| i.error("fmi2SetFMUstate is not supported"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2FreeFMUstate(c: fmi2Component, _s: *mut *mut c_void) -> fmi2Status {
    with(c, |i| i.error("fmi2FreeFMUstate is not supported"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SerializedFMUstateSize(
    c: fmi2Component,
    _s: *mut c_void,
    _size: *mut usize,
) -> fmi2Status {
    with(c, |i| i.error("FMU state serialization is not supported"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SerializeFMUstate(
    c: fmi2Component,
    _s: *mut c_void,
    _bytes: *mut c_char,
    _size: usize,
) -> fmi2Status {
    with(c, |i| i.error("FMU state serialization is not supported"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2DeSerializeFMUstate(
    c: fmi2Component,
    _bytes: *const c_char,
    _size: usize,
    _s: *mut *mut c_void,
) -> fmi2Status {
    with(c, |i| i.error("FMU state serialization is not supported"))
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetDirectionalDerivative(
    c: fmi2Component,
    _unknown: *const fmi2ValueReference,
    _n_unknown: usize,
    _known: *const fmi2ValueReference,
    _n_known: usize,
    _dv_known: *const fmi2Real,
    _dv_unknown: *mut fmi2Real,
) -> fmi2Status {
    with(c, |i| {
        i.error("fmi2GetDirectionalDerivative is not supported")
    })
}

// ---------------------------------------------------------------------------------------
// Model Exchange
// ---------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2EnterEventMode(c: fmi2Component) -> fmi2Status {
    with(c, |i| {
        i.mode = Mode::Event;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2NewDiscreteStates(
    c: fmi2Component,
    info: *mut fmi2EventInfo,
) -> fmi2Status {
    with(c, |i| {
        if info.is_null() {
            return ERROR;
        }
        let s = i.update();
        if s > WARNING {
            return s;
        }
        let m = i.model;
        let mut x0 = vec![0.0; m.states.len()];
        i.sim.rt.get_states(&mut x0);
        if let Err(e) = i.sim.handle_event() {
            i.mode = Mode::Error;
            return i.error(&format!("event iteration failed: {e}"));
        }
        i.sim.rt.run(&m.outputs);
        i.dirty = false;
        let mut x1 = vec![0.0; m.states.len()];
        i.sim.rt.get_states(&mut x1);
        let next = i.sim.consume_time_events();
        let terminated = i.sim.terminated().is_some();
        i.terminated |= terminated;
        unsafe {
            *info = fmi2EventInfo {
                newDiscreteStatesNeeded: 0,
                terminateSimulation: terminated as fmi2Boolean,
                nominalsOfContinuousStatesChanged: 0,
                valuesOfContinuousStatesChanged: (x0 != x1) as fmi2Boolean,
                nextEventTimeDefined: next.is_some() as fmi2Boolean,
                nextEventTime: next.unwrap_or(0.0),
            };
        }
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2EnterContinuousTimeMode(c: fmi2Component) -> fmi2Status {
    with(c, |i| {
        i.mode = Mode::Continuous;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2CompletedIntegratorStep(
    c: fmi2Component,
    _no_set_prior: fmi2Boolean,
    enter_event_mode: *mut fmi2Boolean,
    terminate: *mut fmi2Boolean,
) -> fmi2Status {
    with(c, |i| {
        let s = i.update();
        if s > WARNING {
            return s;
        }
        i.sim.completed_step();
        // A relation that changed without a sign change the integrator saw (it stopped
        // just short of the root): a step event updates it.
        let m = i.model;
        i.sim.rt.eval_zc();
        let stale = (0..m.layout.n_rel).any(|k| {
            let raw = i.sim.rt.r[m.zc_raw[k] as usize];
            let lat = i.sim.rt.r[m.layout.rel0 + k];
            let g = i.sim.rt.r[m.zc_g[k] as usize];
            raw != lat && g.abs() > i.zc_offset(k).abs()
        });
        unsafe {
            if !enter_event_mode.is_null() {
                *enter_event_mode = stale as fmi2Boolean;
            }
            if !terminate.is_null() {
                *terminate = 0;
            }
        }
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetTime(c: fmi2Component, time: fmi2Real) -> fmi2Status {
    with(c, |i| {
        i.sim.rt.set_time(time);
        i.dirty = true;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetContinuousStates(
    c: fmi2Component,
    x: *const fmi2Real,
    nx: usize,
) -> fmi2Status {
    with(c, |i| {
        if nx != i.model.states.len() {
            return i.error(&format!(
                "fmi2SetContinuousStates: {nx} states given, the model has {}",
                i.model.states.len()
            ));
        }
        let x = unsafe { slice(x, nx) };
        i.sim.rt.set_states(x);
        i.dirty = true;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetDerivatives(
    c: fmi2Component,
    dx: *mut fmi2Real,
    nx: usize,
) -> fmi2Status {
    with(c, |i| {
        if nx != i.model.states.len() {
            return i.error("fmi2GetDerivatives: wrong number of states");
        }
        let s = i.update();
        if s > WARNING {
            return s;
        }
        let dx = unsafe { slice_mut(dx, nx) };
        i.sim.rt.get_derivs(dx);
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetEventIndicators(
    c: fmi2Component,
    z: *mut fmi2Real,
    ni: usize,
) -> fmi2Status {
    with(c, |i| {
        let m = i.model;
        if ni != m.layout.n_rel {
            return i.error("fmi2GetEventIndicators: wrong number of event indicators");
        }
        let s = i.update();
        if s > WARNING {
            return s;
        }
        i.sim.rt.eval_zc();
        let z = unsafe { slice_mut(z, ni) };
        for (k, zk) in z.iter_mut().enumerate() {
            let g = i.sim.rt.r[m.zc_g[k] as usize];
            if g.is_finite() && g.abs() > i.zc_scale[k] {
                i.zc_scale[k] = g.abs();
            }
            *zk = g + i.zc_offset(k);
        }
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetContinuousStates(
    c: fmi2Component,
    x: *mut fmi2Real,
    nx: usize,
) -> fmi2Status {
    with(c, |i| {
        if nx != i.model.states.len() {
            return i.error("fmi2GetContinuousStates: wrong number of states");
        }
        if matches!(i.mode, Mode::Instantiated | Mode::Initialization) {
            let s = i.update();
            if s > WARNING {
                return s;
            }
        }
        let x = unsafe { slice_mut(x, nx) };
        i.sim.rt.get_states(x);
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetNominalsOfContinuousStates(
    c: fmi2Component,
    x: *mut fmi2Real,
    nx: usize,
) -> fmi2Status {
    with(c, |_| {
        for v in unsafe { slice_mut(x, nx) } {
            *v = 1.0;
        }
        OK
    })
}

// ---------------------------------------------------------------------------------------
// Co-Simulation
// ---------------------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2SetRealInputDerivatives(
    c: fmi2Component,
    _vr: *const fmi2ValueReference,
    nvr: usize,
    _order: *const fmi2Integer,
    _value: *const fmi2Real,
) -> fmi2Status {
    with(c, |i| {
        if nvr == 0 {
            OK
        } else {
            i.error("input derivatives are not supported (canInterpolateInputs=false)")
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetRealOutputDerivatives(
    c: fmi2Component,
    _vr: *const fmi2ValueReference,
    nvr: usize,
    _order: *const fmi2Integer,
    _value: *mut fmi2Real,
) -> fmi2Status {
    with(c, |i| {
        if nvr == 0 {
            OK
        } else {
            i.error("output derivatives are not supported (maxOutputDerivativeOrder=0)")
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2DoStep(
    c: fmi2Component,
    current_communication_point: fmi2Real,
    communication_step_size: fmi2Real,
    _no_set_prior: fmi2Boolean,
) -> fmi2Status {
    with(c, |i| {
        i.do_step(current_communication_point, communication_step_size)
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2CancelStep(c: fmi2Component) -> fmi2Status {
    with(c, |i| {
        i.error("fmi2CancelStep: steps are never asynchronous")
    })
}

const STATUS_DO_STEP: c_int = 0;
const STATUS_LAST_SUCCESSFUL_TIME: c_int = 2;
const STATUS_TERMINATED: c_int = 3;

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetStatus(
    c: fmi2Component,
    kind: c_int,
    value: *mut fmi2Status,
) -> fmi2Status {
    with(c, |i| {
        if kind != STATUS_DO_STEP || value.is_null() {
            return DISCARD;
        }
        unsafe { *value = if i.mode == Mode::Error { ERROR } else { OK } };
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetRealStatus(
    c: fmi2Component,
    kind: c_int,
    value: *mut fmi2Real,
) -> fmi2Status {
    with(c, |i| {
        if kind != STATUS_LAST_SUCCESSFUL_TIME || value.is_null() {
            return DISCARD;
        }
        unsafe { *value = i.last_time };
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetIntegerStatus(
    c: fmi2Component,
    _kind: c_int,
    _value: *mut fmi2Integer,
) -> fmi2Status {
    with(c, |_| DISCARD)
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetBooleanStatus(
    c: fmi2Component,
    kind: c_int,
    value: *mut fmi2Boolean,
) -> fmi2Status {
    with(c, |i| {
        if kind != STATUS_TERMINATED || value.is_null() {
            return DISCARD;
        }
        unsafe { *value = i.terminated as fmi2Boolean };
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi2GetStringStatus(
    c: fmi2Component,
    _kind: c_int,
    _value: *mut fmi2String,
) -> fmi2Status {
    with(c, |_| DISCARD)
}

#[cfg(test)]
mod tests {
    use super::*;
    use daedalus_core::fmi::{Bundle, describe, guid_of};
    use daedalus_core::frontend::Library;

    #[test]
    fn uri() {
        assert_eq!(uri_to_path("file:///tmp/a%20b/res"), "/tmp/a b/res");
        assert_eq!(uri_to_path("file:/tmp/x"), "/tmp/x");
        assert_eq!(uri_to_path("file://localhost/tmp/x"), "/tmp/x");
        assert_eq!(uri_to_path("/tmp/x"), "/tmp/x");
    }

    const BALL: &str = r#"
model Ball
  input Real gain(start = 1);
  output Real height;
  parameter Real e = 0.8;
  Real h(start = 1, fixed = true);
  Real v(start = 0, fixed = true);
equation
  der(h) = v;
  der(v) = -9.81 * gain;
  when h < 0 then
    reinit(v, -e * pre(v));
  end when;
  height = h;
end Ball;
"#;

    struct Fmu {
        dir: std::path::PathBuf,
        guid: CString,
        model: Model,
    }

    fn export(name: &str) -> Fmu {
        let mut lib = Library::new();
        assert!(lib.set_document("Ball.mo", BALL).is_empty());
        let m = daedalus_core::translate(&lib, "Ball", &Default::default()).unwrap();
        let guid = guid_of(&bincode::serialize(&m).unwrap());
        let dir =
            std::env::temp_dir().join(format!("daedalus-fmi-test-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let bytes = bincode::serialize(&Bundle {
            guid: guid.clone(),
            model: m.clone(),
        })
        .unwrap();
        std::fs::write(dir.join("model.bin"), bytes).unwrap();
        Fmu {
            dir,
            guid: CString::new(guid).unwrap(),
            model: m,
        }
    }

    fn vr(m: &Model, name: &str) -> u32 {
        describe(m)
            .vars
            .iter()
            .find(|v| v.name == name)
            .unwrap_or_else(|| panic!("no {name}"))
            .vr
    }

    fn instantiate(f: &Fmu, kind: c_int, guid: &CStr) -> fmi2Component {
        let loc = CString::new(format!("file://{}", f.dir.display())).unwrap();
        unsafe {
            fmi2Instantiate(
                c"ball".as_ptr(),
                kind,
                guid.as_ptr(),
                loc.as_ptr(),
                std::ptr::null(),
                0,
                0,
            )
        }
    }

    /// Reference: the built-in simulation with the same settings.
    fn reference(m: &Model, e: f64, t: f64) -> f64 {
        let opts = SimOptions {
            stop_time: t,
            params: vec![("e".into(), e)],
            ..SimOptions::from_model(m)
        };
        let (r, _) = daedalus_core::sim::simulate(m, opts).unwrap();
        r.final_value("height").unwrap()
    }

    #[test]
    fn guid_mismatch_is_refused() {
        let f = export("guid");
        let c = instantiate(&f, 1, c"{wrong}");
        assert!(c.is_null());
    }

    #[test]
    fn co_simulation_matches_the_simulator() {
        let f = export("cs");
        let m = &f.model;
        let c = instantiate(&f, 1, &f.guid);
        assert!(!c.is_null());
        let (ve, vh) = (vr(m, "e"), vr(m, "height"));
        unsafe {
            assert_eq!(fmi2SetupExperiment(c, 1, 1e-8, 0.0, 1, 2.0), OK);
            assert_eq!(fmi2SetReal(c, &ve, 1, &0.7), OK);
            assert_eq!(fmi2EnterInitializationMode(c), OK);
            assert_eq!(fmi2ExitInitializationMode(c), OK);
            // parameters are fixed after initialization
            assert_eq!(fmi2SetReal(c, &ve, 1, &0.5), ERROR);
            let mut t = 0.0;
            let h = 0.01;
            for _ in 0..200 {
                assert_eq!(fmi2DoStep(c, t, h, 1), OK);
                t += h;
            }
            let mut y = 0.0;
            assert_eq!(fmi2GetReal(c, &vh, 1, &mut y), OK);
            let want = {
                let opts = SimOptions {
                    stop_time: 2.0,
                    tolerance: 1e-8,
                    params: vec![("e".into(), 0.7)],
                    ..SimOptions::from_model(m)
                };
                daedalus_core::sim::simulate(m, opts)
                    .unwrap()
                    .0
                    .final_value("height")
                    .unwrap()
            };
            assert!((y - want).abs() < 1e-5, "{y} vs {want}");
            let mut last = 0.0;
            assert_eq!(
                fmi2GetRealStatus(c, STATUS_LAST_SUCCESSFUL_TIME, &mut last),
                OK
            );
            assert!((last - 2.0).abs() < 1e-9);
            assert_eq!(fmi2Terminate(c), OK);
            fmi2FreeInstance(c);
        }
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    #[test]
    fn model_exchange_with_events() {
        let f = export("me");
        let m = &f.model;
        let c = instantiate(&f, MODEL_EXCHANGE, &f.guid);
        assert!(!c.is_null());
        let vh = vr(m, "height");
        let nx = m.states.len();
        let nz = m.layout.n_rel;
        assert_eq!((nx, nz), (2, 1));
        unsafe {
            assert_eq!(fmi2SetupExperiment(c, 0, 0.0, 0.0, 0, 0.0), OK);
            assert_eq!(fmi2EnterInitializationMode(c), OK);
            assert_eq!(fmi2ExitInitializationMode(c), OK);
            let mut info = fmi2EventInfo {
                newDiscreteStatesNeeded: 1,
                terminateSimulation: 0,
                nominalsOfContinuousStatesChanged: 0,
                valuesOfContinuousStatesChanged: 0,
                nextEventTimeDefined: 0,
                nextEventTime: 0.0,
            };
            assert_eq!(fmi2NewDiscreteStates(c, &mut info), OK);
            assert_eq!(fmi2EnterContinuousTimeMode(c), OK);
            // RK4 with a fixed step; events located by bisection on the indicator.
            let mut x = vec![0.0; nx];
            let mut z = vec![0.0; nz];
            assert_eq!(fmi2GetContinuousStates(c, x.as_mut_ptr(), nx), OK);
            assert_eq!(fmi2GetEventIndicators(c, z.as_mut_ptr(), nz), OK);
            let deriv = |t: f64, x: &[f64]| -> Vec<f64> {
                let mut dx = vec![0.0; nx];
                fmi2SetTime(c, t);
                fmi2SetContinuousStates(c, x.as_ptr(), nx);
                assert_eq!(fmi2GetDerivatives(c, dx.as_mut_ptr(), nx), OK);
                dx
            };
            let rk4 = |t: f64, x: &[f64], h: f64| -> Vec<f64> {
                let k1 = deriv(t, x);
                let a: Vec<f64> = (0..nx).map(|i| x[i] + 0.5 * h * k1[i]).collect();
                let k2 = deriv(t + 0.5 * h, &a);
                let b: Vec<f64> = (0..nx).map(|i| x[i] + 0.5 * h * k2[i]).collect();
                let k3 = deriv(t + 0.5 * h, &b);
                let d: Vec<f64> = (0..nx).map(|i| x[i] + h * k3[i]).collect();
                let k4 = deriv(t + h, &d);
                (0..nx)
                    .map(|i| x[i] + h / 6.0 * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]))
                    .collect()
            };
            let indicator = |t: f64, x: &[f64]| -> f64 {
                let mut z = vec![0.0; nz];
                fmi2SetTime(c, t);
                fmi2SetContinuousStates(c, x.as_ptr(), nx);
                assert_eq!(fmi2GetEventIndicators(c, z.as_mut_ptr(), nz), OK);
                z[0]
            };
            let (mut t, h, t_end) = (0.0f64, 1e-3f64, 2.0f64);
            let mut events = 0;
            while t < t_end - 1e-12 {
                let hs = h.min(t_end - t);
                let xn = rk4(t, &x, hs);
                let z0 = indicator(t, &x);
                let z1 = indicator(t + hs, &xn);
                if z0 * z1 < 0.0 {
                    // bisection for the crossing within the step
                    let (mut a, mut b) = (0.0, hs);
                    for _ in 0..60 {
                        let mid = 0.5 * (a + b);
                        let zm = indicator(t + mid, &rk4(t, &x, mid));
                        if zm * z0 < 0.0 { b = mid } else { a = mid }
                    }
                    x = rk4(t, &x, b);
                    t += b;
                    fmi2SetTime(c, t);
                    fmi2SetContinuousStates(c, x.as_ptr(), nx);
                    assert_eq!(fmi2EnterEventMode(c), OK);
                    assert_eq!(fmi2NewDiscreteStates(c, &mut info), OK);
                    assert_eq!(info.newDiscreteStatesNeeded, 0);
                    assert_eq!(fmi2EnterContinuousTimeMode(c), OK);
                    assert_eq!(fmi2GetContinuousStates(c, x.as_mut_ptr(), nx), OK);
                    // every other event is the ball leaving the ground (h < 0 turns
                    // false, nothing fires); bounces reinit v
                    if info.valuesOfContinuousStatesChanged != 0 {
                        assert!(x[1] > 0.0);
                        events += 1;
                    }
                } else {
                    x = xn;
                    t += hs;
                    let (mut enter, mut term) = (0, 0);
                    fmi2SetTime(c, t);
                    fmi2SetContinuousStates(c, x.as_ptr(), nx);
                    assert_eq!(fmi2CompletedIntegratorStep(c, 1, &mut enter, &mut term), OK);
                    assert_eq!(enter, 0);
                }
            }
            let mut y = 0.0;
            assert_eq!(fmi2GetReal(c, &vh, 1, &mut y), OK);
            let want = reference(m, 0.8, 2.0);
            assert!(events >= 3, "{events} bounces");
            assert!((y - want).abs() < 1e-4, "{y} vs {want}");
            fmi2FreeInstance(c);
        }
        let _ = std::fs::remove_dir_all(&f.dir);
    }

    /// FMI 3.0 Model Exchange: initialization, and directional derivatives (exact
    /// tangents) against finite differences of the derivatives and outputs.
    #[test]
    fn fmi3_directional_derivatives() {
        use crate::fmi3::*;
        let f = export("fmi3");
        let m = &f.model;
        let loc = CString::new(format!("{}/", f.dir.display())).unwrap();
        let (vg, vh, vv) = (vr(m, "gain"), vr(m, "h"), vr(m, "v"));
        let (vdh, vdv, vy) = (vr(m, "der(h)"), vr(m, "der(v)"), vr(m, "height"));
        unsafe {
            let c = fmi3InstantiateModelExchange(
                c"ball".as_ptr(),
                f.guid.as_ptr(),
                loc.as_ptr(),
                false,
                false,
                std::ptr::null_mut(),
                None,
            );
            assert!(!c.is_null());
            assert_eq!(fmi3SetFloat64(c, &vg, 1, &1.5, 1), OK);
            assert_eq!(
                fmi3EnterInitializationMode(c, false, 0.0, 0.0, false, 0.0),
                OK
            );
            assert_eq!(fmi3ExitInitializationMode(c), OK);
            let (mut a, mut b, mut cc, mut d, mut e, mut t) =
                (false, false, false, false, false, 0.0);
            assert_eq!(
                fmi3UpdateDiscreteStates(c, &mut a, &mut b, &mut cc, &mut d, &mut e, &mut t),
                OK
            );
            assert_eq!(fmi3EnterContinuousTimeMode(c), OK);
            let x = [0.7, -1.2];
            assert_eq!(fmi3SetContinuousStates(c, x.as_ptr(), 2), OK);
            let unknowns = [vdh, vdv, vy];
            let knowns = [vh, vv, vg];
            // exact Jacobian, column by column
            let mut jac = [[0.0; 3]; 3];
            for (k, _) in knowns.iter().enumerate() {
                let mut seed = [0.0; 3];
                seed[k] = 1.0;
                let mut out = [0.0; 3];
                assert_eq!(
                    fmi3GetDirectionalDerivative(
                        c,
                        unknowns.as_ptr(),
                        3,
                        knowns.as_ptr(),
                        3,
                        seed.as_ptr(),
                        3,
                        out.as_mut_ptr(),
                        3
                    ),
                    OK
                );
                for (i, o) in out.iter().enumerate() {
                    jac[i][k] = *o;
                }
            }
            // finite differences
            let eval = |x: [f64; 2], g: f64| -> [f64; 3] {
                assert_eq!(fmi3SetFloat64(c, &vg, 1, &g, 1), OK);
                assert_eq!(fmi3SetContinuousStates(c, x.as_ptr(), 2), OK);
                let mut dx = [0.0; 2];
                assert_eq!(fmi3GetContinuousStateDerivatives(c, dx.as_mut_ptr(), 2), OK);
                let mut y = 0.0;
                assert_eq!(fmi3GetFloat64(c, &vy, 1, &mut y, 1), OK);
                [dx[0], dx[1], y]
            };
            let eps = 1e-6;
            let base = eval(x, 1.5);
            for k in 0..3 {
                let mut xp = x;
                let mut g = 1.5;
                if k < 2 {
                    xp[k] += eps;
                } else {
                    g += eps;
                }
                let p = eval(xp, g);
                for i in 0..3 {
                    let fd = (p[i] - base[i]) / eps;
                    assert!(
                        (fd - jac[i][k]).abs() < 1e-6,
                        "d{i}/d{k}: {} vs {fd}",
                        jac[i][k]
                    );
                }
            }
            assert_eq!(jac[1][2], -9.81);
            assert_eq!(jac[0][1], 1.0);
            assert_eq!(jac[2][0], 1.0);
            fmi3FreeInstance(c);
        }
        let _ = std::fs::remove_dir_all(&f.dir);
    }
}
