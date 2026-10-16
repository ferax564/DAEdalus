//! FMI 3.0 C API (Model Exchange and Co-Simulation) on the same instance as FMI 2.0.
//!
//! Value references are those of FMI 2.0 (variable indices), plus the independent
//! variable `time` and one `Float64` variable per event indicator
//! (`daedalus_core::fmi::{time_vr3, indicator_vr3}`). Directional derivatives come
//! from a forward-mode (tangent) run of the compiled tape, so they are exact.

use super::*;

pub type fmi3Instance = *mut c_void;
pub type fmi3Status = c_int;
pub type fmi3ValueReference = u32;
pub type fmi3String = *const c_char;
pub type LogMessage = unsafe extern "C" fn(*mut c_void, fmi3Status, fmi3String, fmi3String);

/// Value of an FMI 3.0 value reference: a variable, `time` or an event indicator.
fn get3(i: &mut Instance, vr: u32) -> Option<f64> {
    let m = i.model;
    let n = m.vars.len() as u32;
    if vr < n {
        return i.get_value(vr);
    }
    if vr == n {
        return Some(i.sim.rt.time());
    }
    let k = (vr - n - 1) as usize;
    if k >= m.layout.n_rel {
        return None;
    }
    i.sim.rt.eval_zc();
    let g = i.sim.rt.r[m.zc_g[k] as usize];
    Some(g + i.zc_offset(k))
}

unsafe fn get_values3<T>(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    value: *mut T,
    n_values: usize,
    conv: impl Fn(f64) -> T,
) -> fmi3Status {
    with(c, |i| {
        if nvr == 0 {
            return OK;
        }
        if n_values < nvr {
            return i.error("array variables are not supported: nValues < nValueReferences");
        }
        let (vr, value) = unsafe { (slice(vr, nvr), slice_mut(value, nvr)) };
        let s = i.update();
        if s > WARNING {
            return s;
        }
        for (k, &r) in vr.iter().enumerate() {
            match get3(i, r) {
                Some(v) => value[k] = conv(v),
                None => return i.error(&format!("unknown value reference {r}")),
            }
        }
        OK
    })
}

unsafe fn set_values3<T: Copy>(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    value: *const T,
    n_values: usize,
    conv: impl Fn(T) -> f64,
) -> fmi3Status {
    with(c, |i| {
        if nvr == 0 {
            return OK;
        }
        if n_values < nvr {
            return i.error("array variables are not supported: nValues < nValueReferences");
        }
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

fn unsupported(c: fmi3Instance, what: &str) -> fmi3Status {
    with(c, |i| i.error(&format!("{what} is not supported")))
}

#[unsafe(no_mangle)]
pub extern "C" fn fmi3GetVersion() -> *const c_char {
    c"3.0".as_ptr()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetDebugLogging(
    c: fmi3Instance,
    logging_on: bool,
    _n: usize,
    _categories: *const fmi3String,
) -> fmi3Status {
    with(c, |i| {
        i.logging = logging_on;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3InstantiateModelExchange(
    instance_name: fmi3String,
    token: fmi3String,
    resource_path: fmi3String,
    _visible: bool,
    logging_on: bool,
    env: *mut c_void,
    log: Option<LogMessage>,
) -> fmi3Instance {
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        new_instance(
            cstr(instance_name),
            true,
            cstr(token),
            cstr(resource_path),
            log.map(Log::V3),
            env,
            logging_on,
        )
    }));
    r.unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3InstantiateCoSimulation(
    instance_name: fmi3String,
    token: fmi3String,
    resource_path: fmi3String,
    _visible: bool,
    logging_on: bool,
    _event_mode_used: bool,
    _early_return_allowed: bool,
    _required_intermediate: *const fmi3ValueReference,
    _n_required_intermediate: usize,
    env: *mut c_void,
    log: Option<LogMessage>,
    _intermediate_update: *const c_void,
) -> fmi3Instance {
    let r = catch_unwind(AssertUnwindSafe(|| unsafe {
        new_instance(
            cstr(instance_name),
            false,
            cstr(token),
            cstr(resource_path),
            log.map(Log::V3),
            env,
            logging_on,
        )
    }));
    r.unwrap_or(std::ptr::null_mut())
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3InstantiateScheduledExecution(
    _instance_name: fmi3String,
    _token: fmi3String,
    _resource_path: fmi3String,
) -> fmi3Instance {
    std::ptr::null_mut()
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3FreeInstance(c: fmi3Instance) {
    unsafe { fmi2FreeInstance(c) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3EnterInitializationMode(
    c: fmi3Instance,
    tolerance_defined: bool,
    tolerance: f64,
    start_time: f64,
    stop_time_defined: bool,
    stop_time: f64,
) -> fmi3Status {
    let s = unsafe {
        fmi2SetupExperiment(
            c,
            tolerance_defined as c_int,
            tolerance,
            start_time,
            stop_time_defined as c_int,
            stop_time,
        )
    };
    if s != OK {
        return s;
    }
    unsafe { fmi2EnterInitializationMode(c) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3ExitInitializationMode(c: fmi3Instance) -> fmi3Status {
    unsafe { fmi2ExitInitializationMode(c) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3EnterEventMode(c: fmi3Instance) -> fmi3Status {
    with(c, |i| {
        if !i.model_exchange {
            return i.error("fmi3EnterEventMode: Co-Simulation without event mode");
        }
        i.mode = Mode::Event;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3Terminate(c: fmi3Instance) -> fmi3Status {
    unsafe { fmi2Terminate(c) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3Reset(c: fmi3Instance) -> fmi3Status {
    unsafe { fmi2Reset(c) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetFloat64(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    values: *mut f64,
    n_values: usize,
) -> fmi3Status {
    unsafe { get_values3(c, vr, nvr, values, n_values, |v| v) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetInt32(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    values: *mut i32,
    n_values: usize,
) -> fmi3Status {
    unsafe { get_values3(c, vr, nvr, values, n_values, |v| v.round() as i32) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetBoolean(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    values: *mut bool,
    n_values: usize,
) -> fmi3Status {
    unsafe { get_values3(c, vr, nvr, values, n_values, |v| v != 0.0) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetFloat64(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    values: *const f64,
    n_values: usize,
) -> fmi3Status {
    unsafe { set_values3(c, vr, nvr, values, n_values, |v| v) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetInt32(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    values: *const i32,
    n_values: usize,
) -> fmi3Status {
    unsafe { set_values3(c, vr, nvr, values, n_values, |v| v as f64) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetBoolean(
    c: fmi3Instance,
    vr: *const fmi3ValueReference,
    nvr: usize,
    values: *const bool,
    n_values: usize,
) -> fmi3Status {
    unsafe { set_values3(c, vr, nvr, values, n_values, |v| if v { 1.0 } else { 0.0 }) }
}

/// Getters and setters of types the model never has: fine for zero variables.
macro_rules! no_such_type {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(
            c: fmi3Instance,
            _vr: *const fmi3ValueReference,
            nvr: usize,
            _values: *mut c_void,
            _n_values: usize,
        ) -> fmi3Status {
            with(c, |i| {
                if nvr == 0 {
                    OK
                } else {
                    i.error(concat!(stringify!($name), ": the model has no variables of this type"))
                }
            })
        }
    )*};
}

no_such_type!(
    fmi3GetFloat32,
    fmi3GetInt8,
    fmi3GetUInt8,
    fmi3GetInt16,
    fmi3GetUInt16,
    fmi3GetUInt32,
    fmi3GetInt64,
    fmi3GetUInt64,
    fmi3GetString,
    fmi3SetFloat32,
    fmi3SetInt8,
    fmi3SetUInt8,
    fmi3SetInt16,
    fmi3SetUInt16,
    fmi3SetUInt32,
    fmi3SetInt64,
    fmi3SetUInt64,
    fmi3SetString
);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetBinary(
    c: fmi3Instance,
    _vr: *const fmi3ValueReference,
    nvr: usize,
    _sizes: *mut usize,
    _values: *mut c_void,
    _n_values: usize,
) -> fmi3Status {
    with(c, |i| {
        if nvr == 0 {
            OK
        } else {
            i.error("the model has no Binary variables")
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetBinary(
    c: fmi3Instance,
    _vr: *const fmi3ValueReference,
    nvr: usize,
    _sizes: *const usize,
    _values: *const c_void,
    _n_values: usize,
) -> fmi3Status {
    with(c, |i| {
        if nvr == 0 {
            OK
        } else {
            i.error("the model has no Binary variables")
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetClock(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3GetClock (the model has no clocks)")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetClock(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3SetClock (the model has no clocks)")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetNumberOfVariableDependencies(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3GetNumberOfVariableDependencies")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetVariableDependencies(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3GetVariableDependencies")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetFMUState(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3GetFMUState")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetFMUState(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3SetFMUState")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3FreeFMUState(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3FreeFMUState")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SerializedFMUStateSize(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "FMU state serialization")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SerializeFMUState(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "FMU state serialization")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3DeserializeFMUState(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "FMU state serialization")
}

/// Exact directional derivative `sensitivity = d unknowns / d knowns * seed` at the
/// current point: a tangent run of the model equations.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetDirectionalDerivative(
    c: fmi3Instance,
    unknowns: *const fmi3ValueReference,
    n_unknowns: usize,
    knowns: *const fmi3ValueReference,
    n_knowns: usize,
    seed: *const f64,
    n_seed: usize,
    sensitivity: *mut f64,
    n_sensitivity: usize,
) -> fmi3Status {
    with(c, |i| {
        if n_seed != n_knowns || n_sensitivity != n_unknowns {
            return i.error("fmi3GetDirectionalDerivative: array sizes do not match");
        }
        let (unknowns, knowns, seed, sens) = unsafe {
            (
                slice(unknowns, n_unknowns),
                slice(knowns, n_knowns),
                slice(seed, n_seed),
                slice_mut(sensitivity, n_sensitivity),
            )
        };
        let s = i.update();
        if s > WARNING {
            return s;
        }
        let m = i.model;
        let nv = m.vars.len() as u32;
        if let Some(&bad) = unknowns.iter().chain(knowns).find(|&&v| v >= nv) {
            return i.error(&format!(
                "fmi3GetDirectionalDerivative: value reference {bad} is not a model variable"
            ));
        }
        let mut dr = vec![0.0; i.sim.rt.r.len()];
        for (k, &v) in knowns.iter().enumerate() {
            dr[v as usize] = seed[k];
        }
        if matches!(i.mode, Mode::Instantiated | Mode::Initialization) {
            // Initialization: the parameters and inputs reach the unknowns through the
            // parameter bindings too, which the tangent of the equations does not see.
            return i.error("fmi3GetDirectionalDerivative is only available after initialization");
        }
        i.sim.rt.run_tangent(&m.ode, &mut dr);
        i.sim.rt.run_tangent(&m.outputs, &mut dr);
        i.sim.rt.error = None;
        for (k, &v) in unknowns.iter().enumerate() {
            sens[k] = dr[v as usize];
        }
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetAdjointDerivative(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3GetAdjointDerivative")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3EnterConfigurationMode(c: fmi3Instance) -> fmi3Status {
    unsupported(
        c,
        "configuration mode (the model has no structural parameters)",
    )
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3ExitConfigurationMode(c: fmi3Instance) -> fmi3Status {
    unsupported(
        c,
        "configuration mode (the model has no structural parameters)",
    )
}

macro_rules! no_clocks {
    ($($name:ident),*) => {$(
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $name(c: fmi3Instance) -> fmi3Status {
            unsupported(c, concat!(stringify!($name), " (the model has no clocks)"))
        }
    )*};
}

no_clocks!(
    fmi3GetIntervalDecimal,
    fmi3GetIntervalFraction,
    fmi3GetShiftDecimal,
    fmi3GetShiftFraction,
    fmi3SetIntervalDecimal,
    fmi3SetIntervalFraction,
    fmi3SetShiftDecimal,
    fmi3SetShiftFraction,
    fmi3ActivateModelPartition
);

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3EvaluateDiscreteStates(c: fmi3Instance) -> fmi3Status {
    unsupported(c, "fmi3EvaluateDiscreteStates")
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3UpdateDiscreteStates(
    c: fmi3Instance,
    discrete_states_need_update: *mut bool,
    terminate_simulation: *mut bool,
    nominals_changed: *mut bool,
    values_changed: *mut bool,
    next_event_time_defined: *mut bool,
    next_event_time: *mut f64,
) -> fmi3Status {
    let mut info = fmi2EventInfo {
        newDiscreteStatesNeeded: 0,
        terminateSimulation: 0,
        nominalsOfContinuousStatesChanged: 0,
        valuesOfContinuousStatesChanged: 0,
        nextEventTimeDefined: 0,
        nextEventTime: 0.0,
    };
    let s = unsafe { fmi2NewDiscreteStates(c, &mut info) };
    unsafe {
        let put = |p: *mut bool, v: c_int| {
            if !p.is_null() {
                *p = v != 0;
            }
        };
        put(discrete_states_need_update, info.newDiscreteStatesNeeded);
        put(terminate_simulation, info.terminateSimulation);
        put(nominals_changed, info.nominalsOfContinuousStatesChanged);
        put(values_changed, info.valuesOfContinuousStatesChanged);
        put(next_event_time_defined, info.nextEventTimeDefined);
        if !next_event_time.is_null() {
            *next_event_time = info.nextEventTime;
        }
    }
    s
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3EnterContinuousTimeMode(c: fmi3Instance) -> fmi3Status {
    unsafe { fmi2EnterContinuousTimeMode(c) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3CompletedIntegratorStep(
    c: fmi3Instance,
    no_set_prior: bool,
    enter_event_mode: *mut bool,
    terminate: *mut bool,
) -> fmi3Status {
    let (mut e, mut t) = (0, 0);
    let s = unsafe { fmi2CompletedIntegratorStep(c, no_set_prior as c_int, &mut e, &mut t) };
    unsafe {
        if !enter_event_mode.is_null() {
            *enter_event_mode = e != 0;
        }
        if !terminate.is_null() {
            *terminate = t != 0;
        }
    }
    s
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetTime(c: fmi3Instance, time: f64) -> fmi3Status {
    unsafe { fmi2SetTime(c, time) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3SetContinuousStates(
    c: fmi3Instance,
    x: *const f64,
    nx: usize,
) -> fmi3Status {
    unsafe { fmi2SetContinuousStates(c, x, nx) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetContinuousStateDerivatives(
    c: fmi3Instance,
    dx: *mut f64,
    nx: usize,
) -> fmi3Status {
    unsafe { fmi2GetDerivatives(c, dx, nx) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetEventIndicators(
    c: fmi3Instance,
    z: *mut f64,
    n: usize,
) -> fmi3Status {
    unsafe { fmi2GetEventIndicators(c, z, n) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetContinuousStates(
    c: fmi3Instance,
    x: *mut f64,
    nx: usize,
) -> fmi3Status {
    unsafe { fmi2GetContinuousStates(c, x, nx) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetNominalsOfContinuousStates(
    c: fmi3Instance,
    x: *mut f64,
    nx: usize,
) -> fmi3Status {
    unsafe { fmi2GetNominalsOfContinuousStates(c, x, nx) }
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetNumberOfEventIndicators(
    c: fmi3Instance,
    n: *mut usize,
) -> fmi3Status {
    with(c, |i| {
        if !n.is_null() {
            unsafe { *n = i.model.layout.n_rel };
        }
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetNumberOfContinuousStates(
    c: fmi3Instance,
    n: *mut usize,
) -> fmi3Status {
    with(c, |i| {
        if !n.is_null() {
            unsafe { *n = i.model.states.len() };
        }
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3EnterStepMode(c: fmi3Instance) -> fmi3Status {
    with(c, |i| {
        if i.model_exchange {
            return i.error("fmi3EnterStepMode: not a Co-Simulation instance");
        }
        i.mode = Mode::StepComplete;
        OK
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3GetOutputDerivatives(
    c: fmi3Instance,
    _vr: *const fmi3ValueReference,
    nvr: usize,
    _orders: *const i32,
    _values: *mut f64,
    _n_values: usize,
) -> fmi3Status {
    with(c, |i| {
        if nvr == 0 {
            OK
        } else {
            i.error("output derivatives are not supported")
        }
    })
}

#[unsafe(no_mangle)]
pub unsafe extern "C" fn fmi3DoStep(
    c: fmi3Instance,
    current_communication_point: f64,
    communication_step_size: f64,
    _no_set_prior: bool,
    event_handling_needed: *mut bool,
    terminate_simulation: *mut bool,
    early_return: *mut bool,
    last_successful_time: *mut f64,
) -> fmi3Status {
    with(c, |i| {
        let s = i.do_step(current_communication_point, communication_step_size);
        unsafe {
            if !event_handling_needed.is_null() {
                *event_handling_needed = false;
            }
            if !terminate_simulation.is_null() {
                *terminate_simulation = i.terminated;
            }
            if !early_return.is_null() {
                *early_return = false;
            }
            if !last_successful_time.is_null() {
                *last_successful_time = i.last_time;
            }
        }
        // FMI 3: a terminate() inside the step is reported through terminateSimulation.
        if s == DISCARD && i.terminated { OK } else { s }
    })
}
