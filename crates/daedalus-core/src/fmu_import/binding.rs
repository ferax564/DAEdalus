//! Calling an FMU's shared library: the FMI 2.0 and FMI 3.0 C APIs behind one
//! [`Instance`] type.

use super::Fmu;
use super::md::{FmiVersion, Ty};
use crate::external_component::FmiKind;
use std::ffi::{CStr, CString, c_char, c_int, c_void};
use std::sync::{Arc, Mutex};

type Status = c_int;
const OK: Status = 0;
const WARNING: Status = 1;
const DISCARD: Status = 2;

unsafe extern "C" {
    fn calloc(n: usize, size: usize) -> *mut c_void;
    fn free(p: *mut c_void);
}

// ----------------------------------------------------------------------------------------
// FMI 2.0
// ----------------------------------------------------------------------------------------

#[repr(C)]
struct CallbackFunctions2 {
    logger: unsafe extern "C" fn(*mut c_void, *const c_char, Status, *const c_char, *const c_char),
    allocate_memory: unsafe extern "C" fn(usize, usize) -> *mut c_void,
    free_memory: unsafe extern "C" fn(*mut c_void),
    step_finished: *const c_void,
    component_environment: *mut c_void,
}

#[repr(C)]
#[derive(Default)]
struct EventInfo2 {
    new_discrete_states_needed: c_int,
    terminate_simulation: c_int,
    nominals_changed: c_int,
    values_changed: c_int,
    next_event_time_defined: c_int,
    next_event_time: f64,
}

type Comp = *mut c_void;
type Vr = u32;

struct Fmi2 {
    instantiate: unsafe extern "C" fn(
        *const c_char,
        c_int,
        *const c_char,
        *const c_char,
        *const CallbackFunctions2,
        c_int,
        c_int,
    ) -> Comp,
    free_instance: unsafe extern "C" fn(Comp),
    setup_experiment: unsafe extern "C" fn(Comp, c_int, f64, f64, c_int, f64) -> Status,
    enter_init: unsafe extern "C" fn(Comp) -> Status,
    exit_init: unsafe extern "C" fn(Comp) -> Status,
    terminate: unsafe extern "C" fn(Comp) -> Status,
    get_real: unsafe extern "C" fn(Comp, *const Vr, usize, *mut f64) -> Status,
    get_integer: unsafe extern "C" fn(Comp, *const Vr, usize, *mut c_int) -> Status,
    get_boolean: unsafe extern "C" fn(Comp, *const Vr, usize, *mut c_int) -> Status,
    set_real: unsafe extern "C" fn(Comp, *const Vr, usize, *const f64) -> Status,
    set_integer: unsafe extern "C" fn(Comp, *const Vr, usize, *const c_int) -> Status,
    set_boolean: unsafe extern "C" fn(Comp, *const Vr, usize, *const c_int) -> Status,
    // Model Exchange
    enter_event_mode: Option<unsafe extern "C" fn(Comp) -> Status>,
    new_discrete_states: Option<unsafe extern "C" fn(Comp, *mut EventInfo2) -> Status>,
    enter_ct_mode: Option<unsafe extern "C" fn(Comp) -> Status>,
    completed_step: Option<unsafe extern "C" fn(Comp, c_int, *mut c_int, *mut c_int) -> Status>,
    set_time: Option<unsafe extern "C" fn(Comp, f64) -> Status>,
    set_states: Option<unsafe extern "C" fn(Comp, *const f64, usize) -> Status>,
    get_derivatives: Option<unsafe extern "C" fn(Comp, *mut f64, usize) -> Status>,
    get_indicators: Option<unsafe extern "C" fn(Comp, *mut f64, usize) -> Status>,
    get_states: Option<unsafe extern "C" fn(Comp, *mut f64, usize) -> Status>,
    // Co-Simulation
    do_step: Option<unsafe extern "C" fn(Comp, f64, f64, c_int) -> Status>,
    get_boolean_status: Option<unsafe extern "C" fn(Comp, c_int, *mut c_int) -> Status>,
}

// ----------------------------------------------------------------------------------------
// FMI 3.0
// ----------------------------------------------------------------------------------------

type Log3 = unsafe extern "C" fn(*mut c_void, Status, *const c_char, *const c_char);

struct Fmi3 {
    instantiate_me: Option<
        unsafe extern "C" fn(
            *const c_char,
            *const c_char,
            *const c_char,
            bool,
            bool,
            *mut c_void,
            Log3,
        ) -> Comp,
    >,
    instantiate_cs: Option<
        unsafe extern "C" fn(
            *const c_char,
            *const c_char,
            *const c_char,
            bool,
            bool,
            bool,
            bool,
            *const Vr,
            usize,
            *mut c_void,
            Log3,
            *const c_void,
        ) -> Comp,
    >,
    free_instance: unsafe extern "C" fn(Comp),
    enter_init: unsafe extern "C" fn(Comp, bool, f64, f64, bool, f64) -> Status,
    exit_init: unsafe extern "C" fn(Comp) -> Status,
    enter_event_mode: unsafe extern "C" fn(Comp) -> Status,
    terminate: unsafe extern "C" fn(Comp) -> Status,
    get_f64: unsafe extern "C" fn(Comp, *const Vr, usize, *mut f64, usize) -> Status,
    get_f32: unsafe extern "C" fn(Comp, *const Vr, usize, *mut f32, usize) -> Status,
    get_i32: unsafe extern "C" fn(Comp, *const Vr, usize, *mut i32, usize) -> Status,
    get_i64: unsafe extern "C" fn(Comp, *const Vr, usize, *mut i64, usize) -> Status,
    get_bool: unsafe extern "C" fn(Comp, *const Vr, usize, *mut bool, usize) -> Status,
    set_f64: unsafe extern "C" fn(Comp, *const Vr, usize, *const f64, usize) -> Status,
    set_f32: unsafe extern "C" fn(Comp, *const Vr, usize, *const f32, usize) -> Status,
    set_i32: unsafe extern "C" fn(Comp, *const Vr, usize, *const i32, usize) -> Status,
    set_i64: unsafe extern "C" fn(Comp, *const Vr, usize, *const i64, usize) -> Status,
    set_bool: unsafe extern "C" fn(Comp, *const Vr, usize, *const bool, usize) -> Status,
    update_discrete_states: unsafe extern "C" fn(
        Comp,
        *mut bool,
        *mut bool,
        *mut bool,
        *mut bool,
        *mut bool,
        *mut f64,
    ) -> Status,
    // Model Exchange
    enter_ct_mode: Option<unsafe extern "C" fn(Comp) -> Status>,
    completed_step: Option<unsafe extern "C" fn(Comp, bool, *mut bool, *mut bool) -> Status>,
    set_time: Option<unsafe extern "C" fn(Comp, f64) -> Status>,
    set_states: Option<unsafe extern "C" fn(Comp, *const f64, usize) -> Status>,
    get_derivatives: Option<unsafe extern "C" fn(Comp, *mut f64, usize) -> Status>,
    get_indicators: Option<unsafe extern "C" fn(Comp, *mut f64, usize) -> Status>,
    get_states: Option<unsafe extern "C" fn(Comp, *mut f64, usize) -> Status>,
    // Co-Simulation
    do_step: Option<
        unsafe extern "C" fn(
            Comp,
            f64,
            f64,
            bool,
            *mut bool,
            *mut bool,
            *mut bool,
            *mut f64,
        ) -> Status,
    >,
}

enum Api {
    V2(Fmi2),
    V3(Fmi3),
}

/// Messages logged by an FMU instance (shared with its logger callback).
type LogSink = Arc<Mutex<Vec<String>>>;

unsafe extern "C" fn logger2(
    env: *mut c_void,
    _name: *const c_char,
    status: Status,
    category: *const c_char,
    message: *const c_char,
) {
    // The FMI 2 logger is variadic; the message is used as is (unformatted).
    unsafe { log_message(env, status, category, message) }
}

unsafe extern "C" fn logger3(
    env: *mut c_void,
    status: Status,
    category: *const c_char,
    message: *const c_char,
) {
    unsafe { log_message(env, status, category, message) }
}

unsafe fn log_message(
    env: *mut c_void,
    status: Status,
    category: *const c_char,
    msg: *const c_char,
) {
    if env.is_null() || status < WARNING {
        return;
    }
    let s = |p: *const c_char| {
        if p.is_null() {
            String::new()
        } else {
            unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
        }
    };
    let sink = unsafe { &*(env as *const Mutex<Vec<String>>) };
    if let Ok(mut v) = sink.lock()
        && v.len() < 1000
    {
        v.push(format!("[{}] {}", s(category), s(msg)));
    }
}

/// Result of an event update (`fmi2NewDiscreteStates` / `fmi3UpdateDiscreteStates`).
#[derive(Clone, Copy, Debug, Default)]
pub struct EventUpdate {
    pub needs_update: bool,
    pub terminate: bool,
    pub values_changed: bool,
    pub next_time: Option<f64>,
}

/// One instantiated FMU.
pub struct Instance {
    api: Api,
    c: Comp,
    pub kind: FmiKind,
    pub name: String,
    log: LogSink,
    _callbacks: Option<Box<CallbackFunctions2>>,
    _fmu: Arc<Fmu>,
}

// The instance is used from one thread at a time.
unsafe impl Send for Instance {}

macro_rules! sym {
    ($lib:expr, $name:literal) => {
        *unsafe { $lib.get(concat!($name, "\0").as_bytes()) }
            .map_err(|e| format!("FMU binary has no {}: {e}", $name))?
    };
}
macro_rules! opt {
    ($lib:expr, $name:literal) => {
        unsafe { $lib.get(concat!($name, "\0").as_bytes()) }
            .ok()
            .map(|s| *s)
    };
}

fn load_api(lib: &libloading::Library, v: FmiVersion) -> Result<Api, String> {
    Ok(match v {
        FmiVersion::V2 => Api::V2(Fmi2 {
            instantiate: sym!(lib, "fmi2Instantiate"),
            free_instance: sym!(lib, "fmi2FreeInstance"),
            setup_experiment: sym!(lib, "fmi2SetupExperiment"),
            enter_init: sym!(lib, "fmi2EnterInitializationMode"),
            exit_init: sym!(lib, "fmi2ExitInitializationMode"),
            terminate: sym!(lib, "fmi2Terminate"),
            get_real: sym!(lib, "fmi2GetReal"),
            get_integer: sym!(lib, "fmi2GetInteger"),
            get_boolean: sym!(lib, "fmi2GetBoolean"),
            set_real: sym!(lib, "fmi2SetReal"),
            set_integer: sym!(lib, "fmi2SetInteger"),
            set_boolean: sym!(lib, "fmi2SetBoolean"),
            enter_event_mode: opt!(lib, "fmi2EnterEventMode"),
            new_discrete_states: opt!(lib, "fmi2NewDiscreteStates"),
            enter_ct_mode: opt!(lib, "fmi2EnterContinuousTimeMode"),
            completed_step: opt!(lib, "fmi2CompletedIntegratorStep"),
            set_time: opt!(lib, "fmi2SetTime"),
            set_states: opt!(lib, "fmi2SetContinuousStates"),
            get_derivatives: opt!(lib, "fmi2GetDerivatives"),
            get_indicators: opt!(lib, "fmi2GetEventIndicators"),
            get_states: opt!(lib, "fmi2GetContinuousStates"),
            do_step: opt!(lib, "fmi2DoStep"),
            get_boolean_status: opt!(lib, "fmi2GetBooleanStatus"),
        }),
        FmiVersion::V3 => Api::V3(Fmi3 {
            instantiate_me: opt!(lib, "fmi3InstantiateModelExchange"),
            instantiate_cs: opt!(lib, "fmi3InstantiateCoSimulation"),
            free_instance: sym!(lib, "fmi3FreeInstance"),
            enter_init: sym!(lib, "fmi3EnterInitializationMode"),
            exit_init: sym!(lib, "fmi3ExitInitializationMode"),
            enter_event_mode: sym!(lib, "fmi3EnterEventMode"),
            terminate: sym!(lib, "fmi3Terminate"),
            get_f64: sym!(lib, "fmi3GetFloat64"),
            get_f32: sym!(lib, "fmi3GetFloat32"),
            get_i32: sym!(lib, "fmi3GetInt32"),
            get_i64: sym!(lib, "fmi3GetInt64"),
            get_bool: sym!(lib, "fmi3GetBoolean"),
            set_f64: sym!(lib, "fmi3SetFloat64"),
            set_f32: sym!(lib, "fmi3SetFloat32"),
            set_i32: sym!(lib, "fmi3SetInt32"),
            set_i64: sym!(lib, "fmi3SetInt64"),
            set_bool: sym!(lib, "fmi3SetBoolean"),
            update_discrete_states: sym!(lib, "fmi3UpdateDiscreteStates"),
            enter_ct_mode: opt!(lib, "fmi3EnterContinuousTimeMode"),
            completed_step: opt!(lib, "fmi3CompletedIntegratorStep"),
            set_time: opt!(lib, "fmi3SetTime"),
            set_states: opt!(lib, "fmi3SetContinuousStates"),
            get_derivatives: opt!(lib, "fmi3GetContinuousStateDerivatives"),
            get_indicators: opt!(lib, "fmi3GetEventIndicators"),
            get_states: opt!(lib, "fmi3GetContinuousStates"),
            do_step: opt!(lib, "fmi3DoStep"),
        }),
    })
}

fn cstring(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

impl Instance {
    /// Load the FMU's binary for `kind` and instantiate it.
    pub fn new(fmu: Arc<Fmu>, kind: FmiKind, name: &str) -> Result<Instance, String> {
        let md = &fmu.md;
        let iface = match kind {
            FmiKind::ModelExchange => md.me.as_ref(),
            FmiKind::CoSimulation => md.cs.as_ref(),
        }
        .ok_or_else(|| format!("the FMU does not support {}", kind.as_str()))?;
        let lib = fmu.library(&iface.model_identifier)?;
        let api = load_api(lib, md.version)?;
        let log: LogSink = Arc::default();
        let env = Arc::as_ptr(&log) as *mut c_void;
        let cname = cstring(name);
        let token = cstring(&md.token);
        let (c, callbacks) = match &api {
            Api::V2(f) => {
                let res = cstring(&fmu.resource_uri());
                let cb = Box::new(CallbackFunctions2 {
                    logger: logger2,
                    allocate_memory: calloc,
                    free_memory: free,
                    step_finished: std::ptr::null(),
                    component_environment: env,
                });
                let t = match kind {
                    FmiKind::ModelExchange => 0,
                    FmiKind::CoSimulation => 1,
                };
                let c = unsafe {
                    (f.instantiate)(cname.as_ptr(), t, token.as_ptr(), res.as_ptr(), &*cb, 0, 0)
                };
                (c, Some(cb))
            }
            Api::V3(f) => {
                let res = cstring(&fmu.resource_path());
                let c = match kind {
                    FmiKind::ModelExchange => {
                        let inst = f
                            .instantiate_me
                            .ok_or("FMU binary has no fmi3InstantiateModelExchange")?;
                        unsafe {
                            inst(
                                cname.as_ptr(),
                                token.as_ptr(),
                                res.as_ptr(),
                                false,
                                false,
                                env,
                                logger3,
                            )
                        }
                    }
                    FmiKind::CoSimulation => {
                        let inst = f
                            .instantiate_cs
                            .ok_or("FMU binary has no fmi3InstantiateCoSimulation")?;
                        unsafe {
                            inst(
                                cname.as_ptr(),
                                token.as_ptr(),
                                res.as_ptr(),
                                false,
                                false,
                                false,
                                false,
                                std::ptr::null(),
                                0,
                                env,
                                logger3,
                                std::ptr::null(),
                            )
                        }
                    }
                };
                (c, None)
            }
        };
        let inst = Instance {
            api,
            c,
            kind,
            name: name.to_string(),
            log,
            _callbacks: callbacks,
            _fmu: fmu,
        };
        if c.is_null() {
            return Err(format!(
                "instantiating the FMU failed{}",
                inst.messages_suffix()
            ));
        }
        Ok(inst)
    }

    fn messages_suffix(&self) -> String {
        let v = self.log.lock().map(|v| v.clone()).unwrap_or_default();
        if v.is_empty() {
            String::new()
        } else {
            format!(": {}", v.join("; "))
        }
    }

    fn check(&self, what: &str, s: Status) -> Result<(), String> {
        if s <= WARNING {
            Ok(())
        } else {
            Err(format!(
                "FMU '{}': {what} failed (status {s}){}",
                self.name,
                self.messages_suffix()
            ))
        }
    }

    /// A communication step the FMU discarded without terminating: the importer has
    /// no step-size control to retry it with a smaller step.
    fn discarded(&self, what: &str, t: f64, h: f64) -> String {
        format!(
            "FMU '{}': {what} discarded the communication step from t = {t} (step size {h}) \
             without terminating; try a smaller communicationStepSize{}",
            self.name,
            self.messages_suffix()
        )
    }

    fn missing(&self, what: &str) -> String {
        format!("FMU '{}': the binary has no {what}", self.name)
    }

    /// Enter initialization mode (FMI 2: after `fmi2SetupExperiment`).
    pub fn enter_initialization(
        &mut self,
        tol: Option<f64>,
        start: f64,
        stop: Option<f64>,
    ) -> Result<(), String> {
        let s = match &self.api {
            Api::V2(f) => unsafe {
                let s = (f.setup_experiment)(
                    self.c,
                    tol.is_some() as c_int,
                    tol.unwrap_or(0.0),
                    start,
                    stop.is_some() as c_int,
                    stop.unwrap_or(0.0),
                );
                self.check("fmi2SetupExperiment", s)?;
                (f.enter_init)(self.c)
            },
            Api::V3(f) => unsafe {
                (f.enter_init)(
                    self.c,
                    tol.is_some(),
                    tol.unwrap_or(0.0),
                    start,
                    stop.is_some(),
                    stop.unwrap_or(0.0),
                )
            },
        };
        self.check("enter initialization mode", s)
    }

    pub fn exit_initialization(&mut self) -> Result<(), String> {
        let s = match &self.api {
            Api::V2(f) => unsafe { (f.exit_init)(self.c) },
            Api::V3(f) => unsafe { (f.exit_init)(self.c) },
        };
        self.check("exit initialization mode", s)
    }

    /// Set variables of type `ty` (values as f64).
    pub fn set(&mut self, ty: Ty, vrs: &[u32], vals: &[f64]) -> Result<(), String> {
        if vrs.is_empty() {
            return Ok(());
        }
        let n = vrs.len();
        let s = match &self.api {
            Api::V2(f) => unsafe {
                match ty {
                    Ty::Float64 | Ty::Float32 => {
                        (f.set_real)(self.c, vrs.as_ptr(), n, vals.as_ptr())
                    }
                    Ty::Int32 | Ty::Int64 => {
                        let v: Vec<c_int> = vals.iter().map(|x| x.round() as c_int).collect();
                        (f.set_integer)(self.c, vrs.as_ptr(), n, v.as_ptr())
                    }
                    Ty::Boolean => {
                        let v: Vec<c_int> = vals.iter().map(|&x| (x != 0.0) as c_int).collect();
                        (f.set_boolean)(self.c, vrs.as_ptr(), n, v.as_ptr())
                    }
                    Ty::Other => OK,
                }
            },
            Api::V3(f) => unsafe {
                match ty {
                    Ty::Float64 => (f.set_f64)(self.c, vrs.as_ptr(), n, vals.as_ptr(), n),
                    Ty::Float32 => {
                        let v: Vec<f32> = vals.iter().map(|&x| x as f32).collect();
                        (f.set_f32)(self.c, vrs.as_ptr(), n, v.as_ptr(), n)
                    }
                    Ty::Int32 => {
                        let v: Vec<i32> = vals.iter().map(|x| x.round() as i32).collect();
                        (f.set_i32)(self.c, vrs.as_ptr(), n, v.as_ptr(), n)
                    }
                    Ty::Int64 => {
                        let v: Vec<i64> = vals.iter().map(|x| x.round() as i64).collect();
                        (f.set_i64)(self.c, vrs.as_ptr(), n, v.as_ptr(), n)
                    }
                    Ty::Boolean => {
                        let v: Vec<bool> = vals.iter().map(|&x| x != 0.0).collect();
                        (f.set_bool)(self.c, vrs.as_ptr(), n, v.as_ptr(), n)
                    }
                    Ty::Other => OK,
                }
            },
        };
        self.check("set values", s)
    }

    /// Get variables of type `ty` (values as f64).
    pub fn get(&mut self, ty: Ty, vrs: &[u32], out: &mut [f64]) -> Result<(), String> {
        if vrs.is_empty() {
            return Ok(());
        }
        let n = vrs.len();
        let s = match &self.api {
            Api::V2(f) => unsafe {
                match ty {
                    Ty::Float64 | Ty::Float32 => {
                        (f.get_real)(self.c, vrs.as_ptr(), n, out.as_mut_ptr())
                    }
                    Ty::Int32 | Ty::Int64 | Ty::Boolean => {
                        let mut v = vec![0 as c_int; n];
                        let s = if ty == Ty::Boolean {
                            (f.get_boolean)(self.c, vrs.as_ptr(), n, v.as_mut_ptr())
                        } else {
                            (f.get_integer)(self.c, vrs.as_ptr(), n, v.as_mut_ptr())
                        };
                        for (o, x) in out.iter_mut().zip(&v) {
                            *o = if ty == Ty::Boolean {
                                (*x != 0) as i32 as f64
                            } else {
                                *x as f64
                            };
                        }
                        s
                    }
                    Ty::Other => OK,
                }
            },
            Api::V3(f) => unsafe {
                match ty {
                    Ty::Float64 => (f.get_f64)(self.c, vrs.as_ptr(), n, out.as_mut_ptr(), n),
                    Ty::Float32 => {
                        let mut v = vec![0f32; n];
                        let s = (f.get_f32)(self.c, vrs.as_ptr(), n, v.as_mut_ptr(), n);
                        for (o, x) in out.iter_mut().zip(&v) {
                            *o = *x as f64;
                        }
                        s
                    }
                    Ty::Int32 => {
                        let mut v = vec![0i32; n];
                        let s = (f.get_i32)(self.c, vrs.as_ptr(), n, v.as_mut_ptr(), n);
                        for (o, x) in out.iter_mut().zip(&v) {
                            *o = *x as f64;
                        }
                        s
                    }
                    Ty::Int64 => {
                        let mut v = vec![0i64; n];
                        let s = (f.get_i64)(self.c, vrs.as_ptr(), n, v.as_mut_ptr(), n);
                        for (o, x) in out.iter_mut().zip(&v) {
                            *o = *x as f64;
                        }
                        s
                    }
                    Ty::Boolean => {
                        let mut v = vec![false; n];
                        let s = (f.get_bool)(self.c, vrs.as_ptr(), n, v.as_mut_ptr(), n);
                        for (o, x) in out.iter_mut().zip(&v) {
                            *o = *x as i32 as f64;
                        }
                        s
                    }
                    Ty::Other => OK,
                }
            },
        };
        self.check("get values", s)
    }

    pub fn enter_event_mode(&mut self) -> Result<(), String> {
        let s = match &self.api {
            Api::V2(f) => unsafe {
                (f.enter_event_mode
                    .ok_or_else(|| self.missing("fmi2EnterEventMode"))?)(self.c)
            },
            Api::V3(f) => unsafe { (f.enter_event_mode)(self.c) },
        };
        self.check("enter event mode", s)
    }

    pub fn update_discrete_states(&mut self) -> Result<EventUpdate, String> {
        match &self.api {
            Api::V2(f) => {
                let g = f
                    .new_discrete_states
                    .ok_or_else(|| self.missing("fmi2NewDiscreteStates"))?;
                let mut e = EventInfo2::default();
                let s = unsafe { g(self.c, &mut e) };
                self.check("fmi2NewDiscreteStates", s)?;
                Ok(EventUpdate {
                    needs_update: e.new_discrete_states_needed != 0,
                    terminate: e.terminate_simulation != 0,
                    values_changed: e.values_changed != 0,
                    next_time: (e.next_event_time_defined != 0).then_some(e.next_event_time),
                })
            }
            Api::V3(f) => {
                let (mut a, mut b, mut c, mut d, mut e, mut t) =
                    (false, false, false, false, false, 0.0);
                let s = unsafe {
                    (f.update_discrete_states)(
                        self.c, &mut a, &mut b, &mut c, &mut d, &mut e, &mut t,
                    )
                };
                self.check("fmi3UpdateDiscreteStates", s)?;
                Ok(EventUpdate {
                    needs_update: a,
                    terminate: b,
                    values_changed: d,
                    next_time: e.then_some(t),
                })
            }
        }
    }

    pub fn enter_continuous_time_mode(&mut self) -> Result<(), String> {
        let g = match &self.api {
            Api::V2(f) => f.enter_ct_mode,
            Api::V3(f) => f.enter_ct_mode,
        }
        .ok_or_else(|| self.missing("EnterContinuousTimeMode"))?;
        let s = unsafe { g(self.c) };
        self.check("enter continuous-time mode", s)
    }

    /// `(enter event mode, terminate)`.
    pub fn completed_integrator_step(&mut self) -> Result<(bool, bool), String> {
        match &self.api {
            Api::V2(f) => {
                let g = f
                    .completed_step
                    .ok_or_else(|| self.missing("fmi2CompletedIntegratorStep"))?;
                let (mut a, mut b) = (0, 0);
                let s = unsafe { g(self.c, 1, &mut a, &mut b) };
                self.check("fmi2CompletedIntegratorStep", s)?;
                Ok((a != 0, b != 0))
            }
            Api::V3(f) => {
                let g = f
                    .completed_step
                    .ok_or_else(|| self.missing("fmi3CompletedIntegratorStep"))?;
                let (mut a, mut b) = (false, false);
                let s = unsafe { g(self.c, true, &mut a, &mut b) };
                self.check("fmi3CompletedIntegratorStep", s)?;
                Ok((a, b))
            }
        }
    }

    pub fn set_time(&mut self, t: f64) -> Result<(), String> {
        let g = match &self.api {
            Api::V2(f) => f.set_time,
            Api::V3(f) => f.set_time,
        }
        .ok_or_else(|| self.missing("SetTime"))?;
        let s = unsafe { g(self.c, t) };
        self.check("set time", s)
    }

    pub fn set_states(&mut self, x: &[f64]) -> Result<(), String> {
        let g = match &self.api {
            Api::V2(f) => f.set_states,
            Api::V3(f) => f.set_states,
        }
        .ok_or_else(|| self.missing("SetContinuousStates"))?;
        let s = unsafe { g(self.c, x.as_ptr(), x.len()) };
        self.check("set continuous states", s)
    }

    fn get_vec(
        &self,
        g: Option<unsafe extern "C" fn(Comp, *mut f64, usize) -> Status>,
        what: &str,
        out: &mut [f64],
    ) -> Result<(), String> {
        if out.is_empty() {
            return Ok(());
        }
        let g = g.ok_or_else(|| self.missing(what))?;
        let s = unsafe { g(self.c, out.as_mut_ptr(), out.len()) };
        self.check(what, s)
    }

    pub fn get_states(&mut self, x: &mut [f64]) -> Result<(), String> {
        let g = match &self.api {
            Api::V2(f) => f.get_states,
            Api::V3(f) => f.get_states,
        };
        self.get_vec(g, "GetContinuousStates", x)
    }

    pub fn get_derivatives(&mut self, dx: &mut [f64]) -> Result<(), String> {
        let g = match &self.api {
            Api::V2(f) => f.get_derivatives,
            Api::V3(f) => f.get_derivatives,
        };
        self.get_vec(g, "GetDerivatives", dx)
    }

    pub fn get_event_indicators(&mut self, z: &mut [f64]) -> Result<(), String> {
        let g = match &self.api {
            Api::V2(f) => f.get_indicators,
            Api::V3(f) => f.get_indicators,
        };
        self.get_vec(g, "GetEventIndicators", z)
    }

    /// One communication step; returns whether the FMU asked to terminate.
    pub fn do_step(&mut self, t: f64, h: f64) -> Result<bool, String> {
        match &self.api {
            Api::V2(f) => {
                let g = f.do_step.ok_or_else(|| self.missing("fmi2DoStep"))?;
                let s = unsafe { g(self.c, t, h, 1) };
                if s == DISCARD {
                    let mut term = 0;
                    if let Some(gs) = f.get_boolean_status {
                        // fmi2Terminated = 3
                        if unsafe { gs(self.c, 3, &mut term) } > WARNING {
                            term = 0;
                        }
                    }
                    if term != 0 {
                        return Ok(true);
                    }
                    return Err(self.discarded("fmi2DoStep", t, h));
                }
                self.check("fmi2DoStep", s)?;
                Ok(false)
            }
            Api::V3(f) => {
                let g = f.do_step.ok_or_else(|| self.missing("fmi3DoStep"))?;
                let (mut ev, mut term, mut early, mut last) = (false, false, false, 0.0);
                let s = unsafe {
                    g(
                        self.c, t, h, true, &mut ev, &mut term, &mut early, &mut last,
                    )
                };
                if s == DISCARD {
                    if term {
                        return Ok(true);
                    }
                    return Err(self.discarded("fmi3DoStep", t, h));
                }
                self.check("fmi3DoStep", s)?;
                Ok(term)
            }
        }
    }

    pub fn terminate(&mut self) -> Result<(), String> {
        let s = match &self.api {
            Api::V2(f) => unsafe { (f.terminate)(self.c) },
            Api::V3(f) => unsafe { (f.terminate)(self.c) },
        };
        self.check("terminate", s)
    }

    /// Warnings and errors the FMU logged so far.
    pub fn messages(&self) -> Vec<String> {
        self.log.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        unsafe {
            match &self.api {
                Api::V2(f) => (f.free_instance)(self.c),
                Api::V3(f) => (f.free_instance)(self.c),
            }
        }
    }
}
