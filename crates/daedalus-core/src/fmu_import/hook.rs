//! The [`Hook`] that evaluates the FMU components of a model in the simulator.

use super::Fmu;
use super::binding::Instance;
use super::md::Ty;
use super::wrapper::{Port, interface};
use crate::backend::AliasKind;
use crate::codegen::{Model, Runtime};
use crate::expr::VarId;
use crate::external::{DERIVATIVES, ExternalComponent, FmiKind, Hook, STATES, STEP};
use rustc_hash::FxHashMap;
use std::sync::Arc;

/// Where a value the FMU reads lives in the model's registers.
#[derive(Clone, Copy, Debug)]
struct Src {
    reg: usize,
    sign: f64,
}

/// Variables of one type, exchanged in one call.
struct Group<T> {
    ty: Ty,
    vrs: Vec<u32>,
    at: Vec<T>,
    buf: Vec<f64>,
}

fn groups<T: Copy>(items: Vec<(Ty, u32, T)>) -> Vec<Group<T>> {
    let mut out: Vec<Group<T>> = Vec::new();
    for (ty, vr, at) in items {
        match out.iter_mut().find(|g| g.ty == ty) {
            Some(g) => {
                g.vrs.push(vr);
                g.at.push(at);
                g.buf.push(0.0);
            }
            None => out.push(Group {
                ty,
                vrs: vec![vr],
                at: vec![at],
                buf: vec![0.0],
            }),
        }
    }
    out
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Mode {
    Instantiated,
    Event,
    Continuous,
    Step,
}

struct Comp {
    name: String,
    inst: Instance,
    kind: FmiKind,
    mode: Mode,
    params: Vec<Group<Src>>,
    inputs: Vec<Group<Src>>,
    outputs: Vec<Group<usize>>,
    /// Registers of the states `fmu_x`, their derivatives `fmu_dx`, and the model's
    /// derivative slots of those states.
    x_regs: Vec<usize>,
    dx_regs: Vec<usize>,
    d_slots: Vec<usize>,
    x: Vec<f64>,
    dx: Vec<f64>,
    nz: usize,
    feedthrough: bool,
    /// Input values last passed to the FMU (in group order).
    last_u: Vec<f64>,
    u_valid: bool,
    /// An event began (or initialization ended): the next update must run.
    event_pending: bool,
    next_time: Option<f64>,
    step_src: Option<Src>,
    h: f64,
    /// Co-Simulation: start time and number of steps taken (communication points
    /// `t0 + k h`, exactly on the grid).
    t0: f64,
    steps: u64,
    /// Co-Simulation: outputs at the end of the step in flight, `(t_end, values)`.
    pending: Option<(f64, Vec<f64>)>,
    terminated: Option<String>,
}

pub struct FmuHook {
    comps: Vec<Comp>,
    /// Inputs read from alias expressions: run `m.outputs` before reading them.
    needs_outputs: bool,
    error: Option<String>,
}

fn find(m: &Model, by_name: &FxHashMap<&str, VarId>, name: &str) -> Option<(Src, bool)> {
    let &id = by_name.get(name)?;
    for (v, k) in &m.aliases {
        if *v == id {
            return Some(match k {
                AliasKind::Var { of, sign } => (
                    Src {
                        reg: *of as usize,
                        sign: *sign,
                    },
                    false,
                ),
                AliasKind::Expr => (
                    Src {
                        reg: id as usize,
                        sign: 1.0,
                    },
                    true,
                ),
            });
        }
    }
    Some((
        Src {
            reg: id as usize,
            sign: 1.0,
        },
        false,
    ))
}

/// Create the hook for the FMU components of `m` (instantiates every FMU).
pub fn create(m: &Model) -> Result<Box<dyn Hook>, String> {
    let by_name: FxHashMap<&str, VarId> = m
        .vars
        .iter()
        .enumerate()
        .map(|(i, v)| (v.name.as_str(), i as VarId))
        .collect();
    let mut comps = Vec::new();
    let mut needs_outputs = false;
    for e in &m.externals {
        let (c, n) = Comp::new(m, e, &by_name)?;
        needs_outputs |= n;
        comps.push(c);
    }
    Ok(Box::new(FmuHook {
        comps,
        needs_outputs,
        error: None,
    }))
}

impl Comp {
    fn new(
        m: &Model,
        e: &ExternalComponent,
        by_name: &FxHashMap<&str, VarId>,
    ) -> Result<(Comp, bool), String> {
        let label = if e.prefix.is_empty() {
            m.name.clone()
        } else {
            e.prefix.clone()
        };
        let fmu = Fmu::open(std::path::Path::new(&e.file))
            .map_err(|err| format!("FMU component '{label}': {err:#}"))?;
        let fmu = Arc::new(fmu);
        let md = &fmu.md;
        let itf = interface(md);
        let stale = |what: &str| {
            format!(
                "FMU component '{label}' ({}): the wrapper {} has no {what}; regenerate it \
                 with `daedalus import-fmu`",
                e.file, e.class
            )
        };
        let mut needs_outputs = false;
        let mut params = Vec::new();
        for p in &itf.params {
            let v = &md.vars[p.var];
            if let Some((src, n)) = find(m, by_name, &e.member(&p.ident)) {
                needs_outputs |= n;
                params.push((v.ty, v.vr, src));
            }
        }
        let mut inputs = Vec::new();
        for p in &itf.inputs {
            let v = &md.vars[p.var];
            let (src, n) = find(m, by_name, &e.member(&p.ident))
                .ok_or_else(|| stale(&format!("input {}", p.ident)))?;
            needs_outputs |= n;
            inputs.push((v.ty, v.vr, src));
        }
        let mut outputs = Vec::new();
        for Port { var, ident } in itf.outputs.iter().chain(&itf.locals) {
            let v = &md.vars[*var];
            if let Some(&id) = by_name.get(e.member(ident).as_str()) {
                outputs.push((v.ty, v.vr, id as usize));
            }
        }
        let nx = md.states.len();
        let (mut x_regs, mut dx_regs, mut d_slots) = (Vec::new(), Vec::new(), Vec::new());
        if e.kind == FmiKind::ModelExchange {
            for k in 1..=nx {
                let xn = e.member(&format!("{STATES}[{k}]"));
                let dn = e.member(&format!("{DERIVATIVES}[{k}]"));
                let &xi = by_name
                    .get(xn.as_str())
                    .ok_or_else(|| stale(&format!("state {xn}")))?;
                let &di = by_name.get(dn.as_str()).ok_or_else(|| stale(&dn))?;
                let pos = m.states.iter().position(|&s| s == xi).ok_or_else(|| {
                    format!("FMU component '{label}': {xn} is not a state of the model")
                })?;
                x_regs.push(xi as usize);
                dx_regs.push(di as usize);
                d_slots.push(m.derivs[pos] as usize);
            }
        }
        let step_src = if e.kind == FmiKind::CoSimulation {
            Some(
                find(m, by_name, &e.member(STEP))
                    .ok_or_else(|| stale(STEP))?
                    .0,
            )
        } else {
            None
        };
        let inst = Instance::new(fmu.clone(), e.kind, &label)?;
        let n_u = inputs.len();
        let feedthrough = md.feedthrough();
        let nz = if e.kind == FmiKind::ModelExchange {
            md.n_event_indicators
        } else {
            0
        };
        Ok((
            Comp {
                name: label,
                inst,
                kind: e.kind,
                mode: Mode::Instantiated,
                params: groups(params),
                inputs: groups(inputs),
                outputs: groups(outputs),
                x_regs,
                dx_regs,
                d_slots,
                x: vec![0.0; nx],
                dx: vec![0.0; nx],
                nz,
                feedthrough,
                last_u: vec![f64::NAN; n_u],
                u_valid: false,
                event_pending: true,
                next_time: None,
                step_src,
                h: 0.0,
                pending: None,
                t0: 0.0,
                steps: 0,
                terminated: None,
            },
            needs_outputs,
        ))
    }

    fn read(rt: &Runtime, s: Src) -> f64 {
        rt.r[s.reg] * s.sign
    }

    /// Pass the model's input values to the FMU; returns whether any changed.
    fn set_inputs(&mut self, rt: &Runtime, force: bool) -> Result<bool, String> {
        let mut k = 0;
        let mut changed = false;
        for g in &mut self.inputs {
            let mut diff = force || !self.u_valid;
            for (j, &s) in g.at.iter().enumerate() {
                let v = Self::read(rt, s);
                g.buf[j] = v;
                if self.last_u[k + j].to_bits() != v.to_bits() {
                    diff = true;
                }
            }
            if diff {
                self.inst.set(g.ty, &g.vrs, &g.buf)?;
                self.last_u[k..k + g.buf.len()].copy_from_slice(&g.buf);
                changed = true;
            }
            k += g.buf.len();
        }
        self.u_valid = true;
        Ok(changed)
    }

    /// Read the FMU's outputs into the model's registers; returns whether any changed.
    fn get_outputs(&mut self, rt: &mut Runtime) -> Result<bool, String> {
        let mut changed = false;
        for g in &mut self.outputs {
            self.inst.get(g.ty, &g.vrs, &mut g.buf)?;
            for (j, &r) in g.at.iter().enumerate() {
                if rt.r[r].to_bits() != g.buf[j].to_bits() {
                    rt.r[r] = g.buf[j];
                    changed = true;
                }
            }
        }
        Ok(changed)
    }

    fn write_states(&mut self, rt: &mut Runtime) {
        for (k, &r) in self.x_regs.iter().enumerate() {
            rt.r[r] = self.x[k];
        }
    }

    fn write_derivatives(&self, rt: &mut Runtime) {
        for k in 0..self.dx.len() {
            rt.r[self.dx_regs[k]] = self.dx[k];
            rt.r[self.d_slots[k]] = self.dx[k];
        }
    }

    /// Before the model equations: time, states and outputs (Model Exchange).
    fn pre(&mut self, rt: &mut Runtime) -> Result<(), String> {
        if self.kind != FmiKind::ModelExchange {
            return Ok(());
        }
        if self.mode == Mode::Continuous {
            self.inst.set_time(rt.time())?;
            for (k, &r) in self.x_regs.iter().enumerate() {
                self.x[k] = rt.r[r];
            }
            if !self.x.is_empty() {
                self.inst.set_states(&self.x)?;
            }
        }
        self.get_outputs(rt)?;
        Ok(())
    }

    /// After the model equations: inputs, then derivatives. Returns whether the outputs
    /// changed with the new inputs (direct feedthrough: evaluate the model again).
    fn post(&mut self, rt: &mut Runtime) -> Result<bool, String> {
        if self.kind != FmiKind::ModelExchange {
            return Ok(false);
        }
        let changed = self.set_inputs(rt, false)?;
        let again = changed && self.feedthrough && self.get_outputs(rt)?;
        if !self.dx.is_empty() {
            self.inst.get_derivatives(&mut self.dx)?;
        }
        self.write_derivatives(rt);
        Ok(again)
    }

    fn initialize(
        &mut self,
        rt: &mut Runtime,
        start: f64,
        stop: f64,
        tol: f64,
    ) -> Result<(), String> {
        for g in &mut self.params {
            for (j, &s) in g.at.iter().enumerate() {
                g.buf[j] = Self::read(rt, s);
            }
            self.inst.set(g.ty, &g.vrs, &g.buf)?;
        }
        self.set_inputs(rt, true)?;
        let stop = (stop > start).then_some(stop);
        self.inst.enter_initialization(Some(tol), start, stop)?;
        self.set_inputs(rt, true)?;
        self.inst.exit_initialization()?;
        self.get_outputs(rt)?;
        match self.kind {
            FmiKind::ModelExchange => {
                self.mode = Mode::Event;
                if !self.x.is_empty() {
                    self.inst.get_states(&mut self.x)?;
                    self.write_states(rt);
                    self.inst.get_derivatives(&mut self.dx)?;
                    self.write_derivatives(rt);
                }
            }
            FmiKind::CoSimulation => {
                self.mode = Mode::Step;
                let h = self.step_src.map_or(0.0, |s| Self::read(rt, s));
                if !(h > 0.0) {
                    return Err(format!(
                        "FMU component '{}': {STEP} must be positive (is {h})",
                        self.name
                    ));
                }
                self.h = h;
                self.t0 = start;
                self.steps = 0;
            }
        }
        Ok(())
    }
}

impl FmuHook {
    fn fail(&mut self, e: String) {
        if self.error.is_none() {
            self.error = Some(e);
        }
    }

    fn check(&mut self) -> Result<(), String> {
        match self.error.take() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    fn model_outputs(&self, rt: &mut Runtime) {
        if self.needs_outputs {
            let m = rt.m;
            rt.run(&m.outputs);
        }
    }

    fn try_ode(&mut self, rt: &mut Runtime) -> Result<(), String> {
        let m = rt.m;
        for c in &mut self.comps {
            c.pre(rt)?;
        }
        rt.run(&m.ode);
        for _ in 0..20 {
            self.model_outputs(rt);
            let mut again = false;
            for c in &mut self.comps {
                again |= c.post(rt)?;
            }
            if !again {
                return Ok(());
            }
            rt.run(&m.ode);
        }
        Err("algebraic loop through FMU components with direct feedthrough did not converge".into())
    }
}

impl Hook for FmuHook {
    fn ode(&mut self, rt: &mut Runtime) {
        if let Err(e) = self.try_ode(rt) {
            self.fail(e);
            for c in &self.comps {
                for &r in &c.d_slots {
                    rt.r[r] = f64::NAN;
                }
            }
        }
    }

    fn init(&mut self, rt: &mut Runtime, start: f64, stop: f64, tol: f64) -> Result<(), String> {
        for c in &mut self.comps {
            c.initialize(rt, start, stop, tol)?;
        }
        Ok(())
    }

    fn n_indicators(&self) -> usize {
        self.comps.iter().map(|c| c.nz).sum()
    }

    fn indicators(&mut self, _rt: &mut Runtime, z: &mut [f64]) {
        let mut k = 0;
        let mut err = None;
        for c in &mut self.comps {
            if c.nz == 0 {
                continue;
            }
            if let Err(e) = c.inst.get_event_indicators(&mut z[k..k + c.nz]) {
                err = Some(e);
            }
            k += c.nz;
        }
        if let Some(e) = err {
            self.fail(e);
        }
    }

    fn next_time_event(&self) -> Option<f64> {
        self.comps
            .iter()
            .filter_map(|c| match c.kind {
                FmiKind::ModelExchange => c.next_time,
                FmiKind::CoSimulation => c.pending.as_ref().map(|p| p.0),
            })
            .min_by(|a, b| a.total_cmp(b))
    }

    fn begin_event(&mut self, rt: &mut Runtime) -> Result<(), String> {
        self.check()?;
        let t = rt.time();
        for c in &mut self.comps {
            match c.kind {
                FmiKind::ModelExchange => {
                    if c.mode == Mode::Continuous {
                        c.inst.enter_event_mode()?;
                        c.mode = Mode::Event;
                    }
                    c.event_pending = true;
                }
                FmiKind::CoSimulation => {
                    if let Some((te, _)) = &c.pending
                        && t >= *te - 1e-9 * te.abs().max(c.h)
                    {
                        let (_, vals) = c.pending.take().unwrap_or_default();
                        let mut k = 0;
                        for g in &c.outputs {
                            for &r in &g.at {
                                rt.r[r] = vals[k];
                                k += 1;
                            }
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn update_event(&mut self, rt: &mut Runtime) -> Result<bool, String> {
        self.check()?;
        self.model_outputs(rt);
        let mut changed = false;
        for c in &mut self.comps {
            if c.kind != FmiKind::ModelExchange || c.mode != Mode::Event {
                continue;
            }
            let fresh = std::mem::take(&mut c.event_pending);
            let u_changed = c.set_inputs(rt, false)?;
            if !(fresh || u_changed) {
                continue;
            }
            for _ in 0..1000 {
                let u = c.inst.update_discrete_states()?;
                if u.terminate {
                    c.terminated.get_or_insert_with(|| {
                        format!("FMU component '{}' terminated the simulation", c.name)
                    });
                }
                c.next_time = u.next_time;
                if !u.needs_update {
                    break;
                }
            }
            changed |= c.get_outputs(rt)?;
        }
        Ok(changed)
    }

    fn end_event(&mut self, rt: &mut Runtime) -> Result<(), String> {
        self.check()?;
        let t = rt.time();
        self.model_outputs(rt);
        for c in &mut self.comps {
            match c.kind {
                FmiKind::ModelExchange => {
                    if c.mode == Mode::Event {
                        c.inst.enter_continuous_time_mode()?;
                        c.mode = Mode::Continuous;
                    }
                    if !c.x.is_empty() {
                        c.inst.get_states(&mut c.x)?;
                        c.write_states(rt);
                    }
                }
                FmiKind::CoSimulation => {
                    if c.pending.is_some() || c.terminated.is_some() {
                        continue;
                    }
                    let tk = c.t0 + c.steps as f64 * c.h;
                    let t1 = c.t0 + (c.steps + 1) as f64 * c.h;
                    if (t - tk).abs() > 1e-9 * c.h.max(t.abs() * 1e-6) {
                        // Not at a communication point (the step in flight was cut short
                        // by termination): nothing to do.
                        continue;
                    }
                    c.set_inputs(rt, false)?;
                    if c.inst.do_step(tk, t1 - tk)? {
                        c.terminated = Some(format!(
                            "FMU component '{}' terminated the simulation at t = {t1}",
                            c.name
                        ));
                    }
                    c.steps += 1;
                    let mut vals = Vec::new();
                    for g in &mut c.outputs {
                        c.inst.get(g.ty, &g.vrs, &mut g.buf)?;
                        vals.extend_from_slice(&g.buf);
                    }
                    c.pending = Some((t1, vals));
                }
            }
        }
        Ok(())
    }

    fn completed_step(&mut self, _rt: &mut Runtime) -> Result<bool, String> {
        self.check()?;
        let mut event = false;
        for c in &mut self.comps {
            if c.kind == FmiKind::ModelExchange && c.mode == Mode::Continuous {
                let (enter, term) = c.inst.completed_integrator_step()?;
                if term {
                    c.terminated.get_or_insert_with(|| {
                        format!("FMU component '{}' terminated the simulation", c.name)
                    });
                }
                event |= enter;
            }
        }
        Ok(event)
    }

    fn terminated(&self) -> Option<String> {
        self.comps.iter().find_map(|c| c.terminated.clone())
    }
}

impl Drop for FmuHook {
    fn drop(&mut self) {
        for c in &mut self.comps {
            if c.mode != Mode::Instantiated {
                let _ = c.inst.terminate();
            }
        }
    }
}
