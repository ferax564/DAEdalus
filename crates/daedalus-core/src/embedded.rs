//! Embedded C code generation: the compiled [`Model`] (its tape programs) is written out
//! as portable, allocation-free C99 that advances the model with a fixed step. The
//! generated code is meant for microcontrollers (AVR, ESP32, ARM Cortex-M) and for
//! software-in-the-loop tests on the host:
//!
//! - one `struct` holds all state, so several instances can run side by side;
//! - no heap, no recursion, no I/O; every loop has a fixed bound;
//! - `float` or `double` arithmetic (`real_t`), chosen at generation time;
//! - continuous states are integrated with explicit Euler or classic RK4, relations
//!   are latched during a step and events (`when`, `sample`, `reinit`, relations
//!   switching) are handled at the end of each step with the same event iteration
//!   as the simulator; algebraic loops are solved with damped Newton on the exact
//!   symbolic Jacobian.
//!
//! Register file compaction: a tape register that only one program reads and writes
//! becomes a local variable of that program's C function, constants become literals,
//! so the persistent state is only what has to survive between calls (states,
//! discrete values and their `pre`, relations, inputs, outputs, Newton tears).

use crate::codegen::{Action, Ins, Model, Slot};
use crate::expr::{Cmp, Fn1, Fn2, Table, VarId};
use crate::flat::VarKind;
use anyhow::{Result, bail};
use rustc_hash::{FxHashMap, FxHashSet};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RealType {
    Float,
    Double,
}

impl RealType {
    pub fn parse(s: &str) -> Option<RealType> {
        match s {
            "float" | "f32" | "single" => Some(RealType::Float),
            "double" | "f64" => Some(RealType::Double),
            _ => None,
        }
    }
    pub fn c_name(self) -> &'static str {
        match self {
            RealType::Float => "float",
            RealType::Double => "double",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Integrator {
    Euler,
    Rk4,
}

impl Integrator {
    pub fn parse(s: &str) -> Option<Integrator> {
        match s {
            "euler" => Some(Integrator::Euler),
            "rk4" => Some(Integrator::Rk4),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Options {
    /// C identifier prefix (default: the last part of the model name).
    pub prefix: Option<String>,
    /// Fixed step size in seconds (the controller's sample time).
    pub step: f64,
    pub real: RealType,
    pub integrator: Integrator,
    /// Parameter values baked into the code (overriding the model's).
    pub params: Vec<(String, f64)>,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            prefix: None,
            step: 1e-3,
            real: RealType::Float,
            integrator: Integrator::Rk4,
            params: Vec::new(),
        }
    }
}

/// One input or output of the generated code.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Port {
    pub name: String,
    /// The C macro naming its index (`PID_IN_r`).
    pub macro_name: String,
    pub index: usize,
    pub unit: String,
    pub start: f64,
}

/// What the generated code exposes; written next to it as `<prefix>.json`, read by
/// `daedalus hil` and by the HIL firmware shells.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Manifest {
    pub model: String,
    pub prefix: String,
    pub step: f64,
    pub start_time: f64,
    pub real: RealType,
    pub integrator: Integrator,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    pub n_states: usize,
    pub n_newton: usize,
    /// Persistent registers (`real_t r[]` in the instance struct).
    pub registers: usize,
    /// Size of the instance struct in bytes (estimate: registers plus bookkeeping).
    pub instance_bytes: usize,
}

pub struct Generated {
    pub manifest: Manifest,
    pub header: String,
    pub source: String,
}

impl Generated {
    pub fn header_name(&self) -> String {
        format!("{}.h", self.manifest.prefix)
    }
    pub fn source_name(&self) -> String {
        format!("{}.c", self.manifest.prefix)
    }
}

/// Programs (C functions) of the generated code; `Rt` is the hand-written runtime part
/// (integrator, event iteration, Newton driver), which references registers by index.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
enum Ctx {
    Params,
    Starts,
    Init,
    Ode,
    Zc,
    When,
    Outputs,
    Body(u32),
    Jac(u32),
    Rt,
}

/// Where a register lives in the generated code.
#[derive(Clone, Copy, Debug)]
enum Place {
    /// `R[i]`, persistent.
    Global(usize),
    /// A local variable of the one function using it.
    Local,
    /// A literal.
    Const(f64),
}

fn c_ident(s: &str) -> String {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    while out.contains("__") {
        out = out.replace("__", "_");
    }
    let out = out.trim_matches('_').to_string();
    if out.is_empty() || out.chars().next().unwrap().is_ascii_digit() {
        format!("v_{out}")
    } else {
        out
    }
}

fn lit(v: f64) -> String {
    if v.is_nan() {
        "DL_NAN".into()
    } else if v == f64::INFINITY {
        "DL_INF".into()
    } else if v == f64::NEG_INFINITY {
        "(-DL_INF)".into()
    } else if v == v.trunc() && v.abs() < 1e15 {
        format!("RC({v:.1})")
    } else {
        format!("RC({v:e})")
    }
}

fn reads_writes(ins: &Ins, reads: &mut Vec<Slot>, writes: &mut Vec<Slot>) {
    match *ins {
        Ins::Copy(d, a) | Ins::Neg(d, a) | Ins::PowI(d, a, _) | Ins::F1(_, d, a) => {
            writes.push(d);
            reads.push(a);
        }
        Ins::Add(d, a, b)
        | Ins::Sub(d, a, b)
        | Ins::Mul(d, a, b)
        | Ins::Div(d, a, b)
        | Ins::Pow(d, a, b)
        | Ins::F2(_, d, a, b)
        | Ins::Cmp(_, d, a, b) => {
            writes.push(d);
            reads.push(a);
            reads.push(b);
        }
        Ins::Select(d, c, a, b) => {
            writes.push(d);
            reads.extend([c, a, b]);
        }
        Ins::Interp(d, _, _, x) | Ins::InterpDer(d, _, _, x) | Ins::Delay(d, _, x) => {
            writes.push(d);
            reads.push(x);
        }
        Ins::Call(d, ..) | Ins::CallD(d, ..) => writes.push(d),
        Ins::Jz(c, _) => reads.push(c),
        Ins::Jmp(_) | Ins::Newton(_) => {}
    }
}

/// Fail early with a clear message on what the C generator cannot express.
fn check_supported(m: &Model, prog: &[Ins], what: &str) -> Result<()> {
    for ins in prog {
        match ins {
            Ins::Delay(..) => bail!(
                "{what}: delay() is not supported in embedded code (it needs a history buffer)"
            ),
            Ins::Call(..) | Ins::CallD(..) => {
                bail!("{what}: functions with while-loops are not supported in embedded code yet")
            }
            Ins::Newton(i) => {
                let b = &m.newton[*i as usize];
                check_supported(m, &b.body, what)?;
                check_supported(m, &b.jac, what)?;
            }
            _ => {}
        }
    }
    Ok(())
}

struct Gen<'m> {
    m: &'m Model,
    opts: &'m Options,
    prefix: String,
    place: Vec<Place>,
    n_global: usize,
    /// Global index -> what it holds (for comments).
    global_names: Vec<String>,
    tables_used: FxHashSet<u32>,
}

impl<'m> Gen<'m> {
    fn operand(&self, s: Slot) -> String {
        match self.place[s as usize] {
            Place::Global(i) => format!("R[{i}]"),
            Place::Local => format!("t{s}"),
            Place::Const(v) => lit(v),
        }
    }

    fn global(&self, s: Slot) -> usize {
        match self.place[s as usize] {
            Place::Global(i) => i,
            p => panic!("slot {s} is not global ({p:?})"),
        }
    }

    /// The C statements of `prog` (registers named per `place`), with `goto` for jumps.
    fn emit_prog(&mut self, prog: &[Ins], out: &mut String, indent: &str) {
        // Jump targets.
        let mut labels: FxHashSet<usize> = FxHashSet::default();
        for (pc, ins) in prog.iter().enumerate() {
            match *ins {
                Ins::Jz(_, k) | Ins::Jmp(k) => {
                    labels.insert(pc + 1 + k as usize);
                }
                _ => {}
            }
        }
        let mut used_tables: Vec<u32> = Vec::new();
        for (pc, ins) in prog.iter().enumerate() {
            if labels.contains(&pc) {
                let _ = writeln!(out, "L{pc}:;");
            }
            let o = |s: Slot| self.operand(s);
            let line = match *ins {
                Ins::Copy(d, a) => format!("{} = {};", o(d), o(a)),
                Ins::Neg(d, a) => format!("{} = -{};", o(d), o(a)),
                Ins::Add(d, a, b) => format!("{} = {} + {};", o(d), o(a), o(b)),
                Ins::Sub(d, a, b) => format!("{} = {} - {};", o(d), o(a), o(b)),
                Ins::Mul(d, a, b) => format!("{} = {} * {};", o(d), o(a), o(b)),
                Ins::Div(d, a, b) => format!("{} = {} / {};", o(d), o(a), o(b)),
                Ins::Pow(d, a, b) => format!("{} = DL_POW({}, {});", o(d), o(a), o(b)),
                Ins::PowI(d, a, k) => format!("{} = dl_powi({}, {k});", o(d), o(a)),
                Ins::F1(f, d, a) => {
                    let x = o(a);
                    let e = match f {
                        Fn1::Sin => format!("DL_SIN({x})"),
                        Fn1::Cos => format!("DL_COS({x})"),
                        Fn1::Tan => format!("DL_TAN({x})"),
                        Fn1::Asin => format!("DL_ASIN({x})"),
                        Fn1::Acos => format!("DL_ACOS({x})"),
                        Fn1::Atan => format!("DL_ATAN({x})"),
                        Fn1::Sinh => format!("DL_SINH({x})"),
                        Fn1::Cosh => format!("DL_COSH({x})"),
                        Fn1::Tanh => format!("DL_TANH({x})"),
                        Fn1::Exp => format!("DL_EXP({x})"),
                        Fn1::Log => format!("DL_LOG({x})"),
                        Fn1::Log10 => format!("DL_LOG10({x})"),
                        Fn1::Sqrt => format!("DL_SQRT({x})"),
                        Fn1::Abs => format!("DL_FABS({x})"),
                        Fn1::Sign => format!("dl_sign({x})"),
                        Fn1::Floor => format!("DL_FLOOR({x})"),
                        Fn1::Ceil => format!("DL_CEIL({x})"),
                        Fn1::Not => format!("(({x}) == RC(0.0) ? RC(1.0) : RC(0.0))"),
                    };
                    format!("{} = {e};", o(d))
                }
                Ins::F2(f, d, a, b) => {
                    let (x, y) = (o(a), o(b));
                    let e = match f {
                        Fn2::Atan2 => format!("dl_atan2({x}, {y})"),
                        Fn2::Min => format!("dl_min({x}, {y})"),
                        Fn2::Max => format!("dl_max({x}, {y})"),
                        Fn2::Div => format!("DL_TRUNC({x} / {y})"),
                        Fn2::Mod => format!("dl_mod({x}, {y})"),
                        Fn2::Rem => format!("dl_rem({x}, {y})"),
                        Fn2::And => {
                            format!("(({x}) != RC(0.0) && ({y}) != RC(0.0) ? RC(1.0) : RC(0.0))")
                        }
                        Fn2::Or => {
                            format!("(({x}) != RC(0.0) || ({y}) != RC(0.0) ? RC(1.0) : RC(0.0))")
                        }
                    };
                    format!("{} = {e};", o(d))
                }
                Ins::Cmp(op, d, a, b) => {
                    let c = match op {
                        Cmp::Lt => "<",
                        Cmp::Le => "<=",
                        Cmp::Gt => ">",
                        Cmp::Ge => ">=",
                        Cmp::Eq => "==",
                        Cmp::Ne => "!=",
                    };
                    format!("{} = ({} {c} {}) ? RC(1.0) : RC(0.0);", o(d), o(a), o(b))
                }
                Ins::Select(d, c, a, b) => {
                    format!("{} = ({} != RC(0.0)) ? {} : {};", o(d), o(c), o(a), o(b))
                }
                Ins::Interp(d, t, c, x) => {
                    used_tables.push(t);
                    if c == Table::NEXT_EVENT {
                        format!("{} = dl_next_event(&TBL{t}, {});", o(d), o(x))
                    } else {
                        format!("{} = dl_interp(&TBL{t}, {c}, {});", o(d), o(x))
                    }
                }
                Ins::InterpDer(d, t, c, x) => {
                    used_tables.push(t);
                    format!("{} = dl_slope(&TBL{t}, {c}, {});", o(d), o(x))
                }
                Ins::Jz(c, k) => format!("if ({} == RC(0.0)) goto L{};", o(c), pc + 1 + k as usize),
                Ins::Jmp(k) => format!("goto L{};", pc + 1 + k as usize),
                Ins::Newton(i) => format!("dl_newton(m, &NB{i});"),
                Ins::Delay(..) | Ins::Call(..) | Ins::CallD(..) => {
                    unreachable!("rejected by check_supported")
                }
            };
            let _ = writeln!(out, "{indent}{line}");
        }
        if labels.contains(&prog.len()) {
            let _ = writeln!(out, "L{}:;", prog.len());
        }
        self.tables_used.extend(used_tables);
    }

    /// `prog` without instructions that only write locals nobody reads (jump offsets
    /// adjusted). `tail` is C code run after it, which may read locals too.
    fn dead_code(&self, prog: &[Ins], tail: &str) -> Vec<Ins> {
        let mut keep = vec![true; prog.len()];
        loop {
            let mut read: FxHashSet<Slot> = FxHashSet::default();
            let (mut r, mut w) = (Vec::new(), Vec::new());
            for (ins, _) in prog.iter().zip(&keep).filter(|(_, k)| **k) {
                r.clear();
                w.clear();
                reads_writes(ins, &mut r, &mut w);
                read.extend(r.iter().copied());
            }
            let mut changed = false;
            for (i, ins) in prog.iter().enumerate() {
                if !keep[i] || matches!(ins, Ins::Jz(..) | Ins::Jmp(..) | Ins::Newton(..)) {
                    continue;
                }
                r.clear();
                w.clear();
                reads_writes(ins, &mut r, &mut w);
                let dead = w.iter().all(|&d| {
                    matches!(self.place[d as usize], Place::Local)
                        && !read.contains(&d)
                        && !tail.contains(&format!("t{d};"))
                });
                if dead && !w.is_empty() {
                    keep[i] = false;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        // Rebuild with jump offsets counted over the kept instructions.
        let mut new_index = vec![0usize; prog.len() + 1];
        let mut n = 0;
        for i in 0..prog.len() {
            new_index[i] = n;
            if keep[i] {
                n += 1;
            }
        }
        new_index[prog.len()] = n;
        let mut out = Vec::with_capacity(n);
        for (i, ins) in prog.iter().enumerate() {
            if !keep[i] {
                continue;
            }
            let fix = |k: u32| -> u32 {
                let target = i + 1 + k as usize;
                (new_index[target] - new_index[i] - 1) as u32
            };
            out.push(match *ins {
                Ins::Jz(c, k) => Ins::Jz(c, fix(k)),
                Ins::Jmp(k) => Ins::Jmp(fix(k)),
                x => x,
            });
        }
        out
    }

    /// A `static void name(P_t *m)` function running `prog`, with its locals declared.
    fn function(&mut self, name: &str, prog: &[Ins], extra_args: &str, tail: &str) -> String {
        let prog = self.dead_code(prog, tail);
        let prog = prog.as_slice();
        let p = self.prefix.clone();
        let mut locals: Vec<Slot> = Vec::new();
        let mut seen: FxHashSet<Slot> = FxHashSet::default();
        let (mut r, mut w) = (Vec::new(), Vec::new());
        for ins in prog {
            r.clear();
            w.clear();
            reads_writes(ins, &mut r, &mut w);
            for &s in r.iter().chain(&w) {
                if matches!(self.place[s as usize], Place::Local) && seen.insert(s) {
                    locals.push(s);
                }
            }
        }
        locals.sort_unstable();
        let mut body = String::new();
        self.emit_prog(prog, &mut body, "    ");
        let mut f = String::new();
        let _ = writeln!(f, "static void {name}({p}_t *m{extra_args})\n{{");
        let _ = writeln!(f, "    real_t *const R = m->r;");
        for chunk in locals.chunks(8) {
            let names: Vec<String> = chunk.iter().map(|s| format!("t{s}")).collect();
            let _ = writeln!(f, "    real_t {};", names.join(", "));
        }
        f.push_str(&body);
        f.push_str(tail);
        let _ = writeln!(f, "    (void)R;\n}}\n");
        f
    }
}

/// Generate the C code of `m`.
pub fn generate(m: &Model, opts: &Options) -> Result<Generated> {
    if !(opts.step > 0.0) {
        bail!("the step size must be positive");
    }
    let what = "embedded code";
    for prog in [&m.starts, &m.init, &m.ode, &m.zc, &m.when_prog, &m.outputs] {
        check_supported(m, prog, what)?;
    }
    for (_, seg) in &m.params {
        check_supported(m, seg, what)?;
    }
    if !m.delays.is_empty() {
        bail!("{what}: delay() is not supported in embedded code");
    }
    let prefix = c_ident(
        opts.prefix
            .as_deref()
            .unwrap_or_else(|| m.name.rsplit('.').next().unwrap_or(&m.name)),
    );
    // Parameter overrides become literal assignments.
    let mut params: Vec<Vec<Ins>> = Vec::new();
    let mut overrides: FxHashMap<&str, f64> =
        opts.params.iter().map(|(n, v)| (n.as_str(), *v)).collect();
    let mut override_consts: Vec<(Slot, f64)> = Vec::new();
    let mut next_extra = m.layout.n_regs as Slot;
    for (p, seg) in &m.params {
        let name = m.vars[*p as usize].name.as_str();
        if let Some(v) = overrides.remove(name) {
            let s = next_extra;
            next_extra += 1;
            override_consts.push((s, v));
            params.push(vec![Ins::Copy(*p, s)]);
        } else {
            params.push(seg.clone());
        }
    }
    if let Some(n) = overrides.keys().next() {
        bail!("unknown parameter '{n}'");
    }
    let n_regs = next_extra as usize;
    let lay = m.layout;

    // Which program touches each register.
    let mut ctxs: Vec<FxHashSet<Ctx>> = vec![FxHashSet::default(); n_regs];
    let mut written: Vec<bool> = vec![false; n_regs];
    {
        let mut add = |ctx: Ctx, prog: &[Ins], ctxs: &mut Vec<FxHashSet<Ctx>>| {
            let (mut r, mut w) = (Vec::new(), Vec::new());
            for ins in prog {
                r.clear();
                w.clear();
                reads_writes(ins, &mut r, &mut w);
                for &s in &w {
                    written[s as usize] = true;
                }
                for &s in r.iter().chain(&w) {
                    ctxs[s as usize].insert(ctx);
                }
            }
        };
        for seg in &params {
            add(Ctx::Params, seg, &mut ctxs);
        }
        add(Ctx::Starts, &m.starts, &mut ctxs);
        add(Ctx::Init, &m.init, &mut ctxs);
        add(Ctx::Ode, &m.ode, &mut ctxs);
        add(Ctx::Zc, &m.zc, &mut ctxs);
        add(Ctx::When, &m.when_prog, &mut ctxs);
        add(Ctx::Outputs, &m.outputs, &mut ctxs);
        for (i, b) in m.newton.iter().enumerate() {
            add(Ctx::Body(i as u32), &b.body, &mut ctxs);
            add(Ctx::Jac(i as u32), &b.jac, &mut ctxs);
        }
    }
    // Registers the runtime part reads or writes by index.
    let mut rt: Vec<Slot> = Vec::new();
    rt.push(lay.time as Slot);
    rt.push(lay.initial as Slot);
    rt.extend(&m.states);
    rt.extend(&m.derivs);
    rt.extend(m.inputs.iter().map(|&v| v as Slot));
    let outputs: Vec<VarId> = (0..m.vars.len() as VarId)
        .filter(|&v| {
            let x = &m.vars[v as usize];
            x.is_output && !x.name.starts_with('$') && x.kind != VarKind::Input
        })
        .collect();
    rt.extend(outputs.iter().map(|&v| v as Slot));
    // Discrete variables, and every pre() read anywhere: `pre` is copied by the runtime.
    let mut pre_vars: Vec<VarId> = m.discretes.iter().map(|&d| d as VarId).collect();
    for s in lay.pre0..lay.pre0 + lay.nv {
        if !ctxs[s].is_empty() {
            pre_vars.push((s - lay.pre0) as VarId);
        }
    }
    for &(v, _) in &m.pre_init {
        pre_vars.push(v);
    }
    pre_vars.sort_unstable();
    pre_vars.dedup();
    for &v in &pre_vars {
        rt.push(v as Slot);
        rt.push((lay.pre0 + v as usize) as Slot);
    }
    for &(v, p) in &m.pre_init {
        rt.push(p as Slot);
        rt.push((lay.pre0 + v as usize) as Slot);
    }
    for i in 0..lay.n_rel {
        rt.push((lay.rel0 + i) as Slot);
        rt.push(m.zc_raw[i]);
    }
    for w in &m.whens {
        rt.extend(&w.conds);
        for acts in &w.actions {
            for a in acts {
                match a {
                    Action::Assign(v, s) | Action::Reinit(v, s) => rt.extend([*v, *s]),
                    Action::Assert(c, _) => rt.push(*c),
                    Action::Terminate(_) => {}
                }
            }
        }
    }
    for b in &m.newton {
        rt.extend(&b.tears);
        rt.extend(&b.residuals);
        rt.extend(&b.jac_slots);
    }
    for &s in &rt {
        ctxs[s as usize].insert(Ctx::Rt);
    }
    let constv: FxHashMap<Slot, f64> = m.consts.iter().copied().chain(override_consts).collect();
    // Places.
    let mut place = vec![Place::Local; n_regs];
    let mut n_global = 0usize;
    let mut global_names: Vec<String> = Vec::new();
    let mut const_globals: Vec<(usize, f64)> = Vec::new();
    for s in 0..n_regs {
        let c = &ctxs[s];
        if c.is_empty() {
            continue;
        }
        if let Some(&v) = constv.get(&(s as Slot))
            && !written[s]
        {
            if c.contains(&Ctx::Rt) {
                place[s] = Place::Global(n_global);
                const_globals.push((n_global, v));
                global_names.push(format!("constant {v}"));
                n_global += 1;
            } else {
                place[s] = Place::Const(v);
            }
            continue;
        }
        if c.len() > 1 || c.contains(&Ctx::Rt) {
            place[s] = Place::Global(n_global);
            let name = if s < lay.nv {
                m.vars[s].name.clone()
            } else if s >= lay.pre0 && s < lay.pre0 + lay.nv {
                format!("pre({})", m.vars[s - lay.pre0].name)
            } else if s == lay.time {
                "time".into()
            } else if s == lay.initial {
                "initial()".into()
            } else if s >= lay.rel0 && s < lay.rel0 + lay.n_rel {
                format!("relation {}", m.rel_text[s - lay.rel0])
            } else {
                String::new()
            };
            global_names.push(name);
            n_global += 1;
        }
    }
    // A local that is read before any write in its function would read garbage: such
    // registers (the tape keeps values between runs) must stay global.
    let mut g = Gen {
        m,
        opts,
        prefix: prefix.clone(),
        place,
        n_global,
        global_names,
        tables_used: FxHashSet::default(),
    };
    promote_read_before_write(&mut g, &params);

    let p = prefix.clone();
    let up = p.to_uppercase();
    let real = opts.real.c_name();
    let n_states = m.states.len();

    // ---- source -------------------------------------------------------------------
    let mut fns = String::new();
    // Parameters: one function, segments in dependency order.
    let all_params: Vec<Ins> = params.iter().flatten().copied().collect();
    fns.push_str(&g.function("f_params", &all_params, "", ""));
    fns.push_str(&g.function("f_starts", &m.starts, "", ""));
    for (i, b) in m.newton.iter().enumerate() {
        let mut tail = String::new();
        for (k, &s) in b.residuals.iter().enumerate() {
            let _ = writeln!(tail, "    F[{k}] = {};", g.operand(s));
        }
        fns.push_str(&g.function(&format!("nb{i}_body"), &b.body, ", real_t *F", &tail));
        let mut tail = String::new();
        for (k, &s) in b.jac_slots.iter().enumerate() {
            let _ = writeln!(tail, "    J[{k}] = {};", g.operand(s));
        }
        fns.push_str(&g.function(&format!("nb{i}_jac"), &b.jac, ", real_t *J", &tail));
    }
    fns.push_str(&g.function("f_init", &m.init, "", ""));
    fns.push_str(&g.function("f_ode", &m.ode, "", ""));
    fns.push_str(&g.function("f_zc", &m.zc, "", ""));
    fns.push_str(&g.function("f_when", &m.when_prog, "", ""));
    fns.push_str(&g.function("f_outputs", &m.outputs, "", ""));

    let mut src = String::new();
    let _ = writeln!(
        src,
        "/* {p}.c: embedded C code of the Modelica model {}.\n * Generated by DAEdalus (https://github.com/ferax564/DAEdalus). Do not edit.\n * Fixed step {} s, {real} arithmetic, {} integration.\n */",
        m.name,
        opts.step,
        match opts.integrator {
            Integrator::Euler => "explicit Euler",
            Integrator::Rk4 => "classic Runge-Kutta (RK4)",
        }
    );
    let _ = writeln!(
        src,
        "#include \"{p}.h\"\n#include <math.h>\n#include <string.h>\n"
    );
    src.push_str(&runtime_prelude(opts.real));
    // Tables.
    let mut tables: Vec<u32> = g.tables_used.iter().copied().collect();
    tables.sort_unstable();
    for t in &tables {
        let tb = &m.tables[*t as usize];
        let data: Vec<String> = tb.data.iter().map(|&v| lit(v)).collect();
        let slopes: Vec<String> = tb.slopes.iter().map(|&v| lit(v)).collect();
        let _ = writeln!(
            src,
            "DL_UNUSED static const real_t TBL{t}_data[] = {{{}}};",
            data.join(", ")
        );
        if !slopes.is_empty() {
            let _ = writeln!(
                src,
                "DL_UNUSED static const real_t TBL{t}_slopes[] = {{{}}};",
                slopes.join(", ")
            );
        }
        let _ = writeln!(
            src,
            "DL_UNUSED static const dl_table TBL{t} = {{TBL{t}_data, {}, {}, {}, {}, {}, {}, {}}};",
            if slopes.is_empty() {
                "0".to_string()
            } else {
                format!("TBL{t}_slopes")
            },
            tb.rows,
            tb.cols,
            tb.extrapolation,
            tb.smoothness,
            lit(tb.shift),
            lit(tb.start)
        );
    }
    if !tables.is_empty() {
        src.push('\n');
    }
    // Forward declarations of the Newton descriptors (programs call dl_newton).
    let max_n = m.newton.iter().map(|b| b.tears.len()).max().unwrap_or(0);
    if !m.newton.is_empty() {
        let _ = writeln!(src, "#define DL_NB_MAX {max_n}");
        let _ = writeln!(
            src,
            "typedef struct {{\n    uint8_t n;\n    uint8_t linear;\n    const uint16_t *tears;\n    const real_t *lo;\n    const real_t *hi;\n    void (*body)({p}_t *, real_t *);\n    void (*jac)({p}_t *, real_t *);\n}} dl_nblock;"
        );
        for i in 0..m.newton.len() {
            let _ = writeln!(src, "static void nb{i}_body({p}_t *m, real_t *F);");
            let _ = writeln!(src, "static void nb{i}_jac({p}_t *m, real_t *J);");
            let _ = writeln!(src, "static const dl_nblock NB{i};");
        }
        let _ = writeln!(
            src,
            "static void dl_newton({p}_t *m, const dl_nblock *b);\n"
        );
    }
    src.push_str(&fns);
    // Newton descriptors and driver.
    for (i, b) in m.newton.iter().enumerate() {
        let tears: Vec<String> = b.tears.iter().map(|&s| g.global(s).to_string()).collect();
        let lo: Vec<String> = b.bounds.iter().map(|&(l, _)| lit(l)).collect();
        let hi: Vec<String> = b.bounds.iter().map(|&(_, h)| lit(h)).collect();
        let _ = writeln!(
            src,
            "/* Algebraic loop {i}: {} */",
            b.names.join(", ").replace("*/", "* /")
        );
        let _ = writeln!(
            src,
            "DL_UNUSED static const uint16_t NB{i}_tears[] = {{{}}};",
            tears.join(", ")
        );
        let _ = writeln!(
            src,
            "DL_UNUSED static const real_t NB{i}_lo[] = {{{}}};",
            lo.join(", ")
        );
        let _ = writeln!(
            src,
            "DL_UNUSED static const real_t NB{i}_hi[] = {{{}}};",
            hi.join(", ")
        );
        let _ = writeln!(
            src,
            "static const dl_nblock NB{i} = {{{}, {}, NB{i}_tears, NB{i}_lo, NB{i}_hi, nb{i}_body, nb{i}_jac}};\n",
            b.tears.len(),
            u8::from(b.linear)
        );
    }
    if !m.newton.is_empty() {
        src.push_str(&newton_driver(&p));
    }
    // Index tables of the runtime part.
    let idx = |v: &[usize]| -> String {
        if v.is_empty() {
            "{0}".into()
        } else {
            format!(
                "{{{}}}",
                v.iter()
                    .map(|x| x.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    let states: Vec<usize> = m.states.iter().map(|&s| g.global(s)).collect();
    let derivs: Vec<usize> = m.derivs.iter().map(|&s| g.global(s)).collect();
    let _ = writeln!(src, "#define N_STATES {n_states}");
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t STATE_IDX[] = {};",
        idx(&states)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t DERIV_IDX[] = {};",
        idx(&derivs)
    );
    let pre_pairs: Vec<usize> = pre_vars
        .iter()
        .flat_map(|&v| {
            [
                g.global((lay.pre0 + v as usize) as Slot),
                g.global(v as Slot),
            ]
        })
        .collect();
    let _ = writeln!(src, "#define N_PRE {}", pre_vars.len());
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t PRE_PAIRS[] = {}; /* pre, var */",
        idx(&pre_pairs)
    );
    let disc: Vec<usize> = m
        .discretes
        .iter()
        .flat_map(|&d| [g.global(d), g.global((lay.pre0 + d as usize) as Slot)])
        .collect();
    let _ = writeln!(src, "#define N_DISC {}", m.discretes.len());
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t DISC_PAIRS[] = {}; /* var, pre */",
        idx(&disc)
    );
    let pre_init: Vec<usize> = m
        .pre_init
        .iter()
        .flat_map(|&(v, q)| {
            [
                g.global((lay.pre0 + v as usize) as Slot),
                g.global(q as Slot),
            ]
        })
        .collect();
    let _ = writeln!(src, "#define N_PRE_INIT {}", m.pre_init.len());
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t PRE_INIT[] = {};",
        idx(&pre_init)
    );
    let rels: Vec<usize> = (0..lay.n_rel)
        .flat_map(|i| [g.global((lay.rel0 + i) as Slot), g.global(m.zc_raw[i])])
        .collect();
    let _ = writeln!(src, "#define N_REL {}", lay.n_rel);
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t REL_PAIRS[] = {}; /* latched, raw */",
        idx(&rels)
    );
    let samp: Vec<usize> = m
        .sample_rels
        .iter()
        .map(|&r| g.global((lay.rel0 + r) as Slot))
        .collect();
    let _ = writeln!(src, "#define N_SAMPLE_REL {}", samp.len());
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t SAMPLE_REL[] = {};",
        idx(&samp)
    );
    // When-clauses: branches flattened; actions per branch.
    let mut br_cond = Vec::new();
    let mut br_clause = Vec::new();
    let mut br_flags = Vec::new();
    let mut br_act = vec![0usize];
    let mut act_kind = Vec::new();
    let mut act_a = Vec::new();
    let mut act_b = Vec::new();
    let mut clause_br0 = Vec::new();
    for (wi, w) in m.whens.iter().enumerate() {
        clause_br0.push(br_cond.len());
        for (bi, acts) in w.actions.iter().enumerate() {
            br_cond.push(g.global(w.conds[bi]));
            br_clause.push(wi);
            br_flags.push(usize::from(w.chained[bi]) | (usize::from(w.at_initial[bi]) << 1));
            for a in acts {
                let (k, x, y) = match a {
                    Action::Assign(v, s) | Action::Reinit(v, s) => (0, g.global(*v), g.global(*s)),
                    Action::Terminate(_) => (1, 0, 0),
                    Action::Assert(c, _) => (2, g.global(*c), 0),
                };
                act_kind.push(k);
                act_a.push(x);
                act_b.push(y);
            }
            br_act.push(act_kind.len());
        }
    }
    clause_br0.push(br_cond.len());
    let n_br = br_cond.len();
    let _ = writeln!(src, "#define N_CLAUSE {}", m.whens.len());
    let _ = writeln!(src, "#define N_BRANCH {n_br}");
    let _ = writeln!(src, "#define N_ACTION {}", act_kind.len());
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t CLAUSE_BR0[] = {};",
        idx(&clause_br0)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t BR_COND[] = {};",
        idx(&br_cond)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint8_t BR_FLAGS[] = {}; /* 1 chained, 2 initial() */",
        idx(&br_flags)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t BR_ACT[] = {};",
        idx(&br_act)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint8_t ACT_KIND[] = {}; /* 0 assign, 1 terminate, 2 assert */",
        idx(&act_kind)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t ACT_A[] = {};",
        idx(&act_a)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t ACT_B[] = {};",
        idx(&act_b)
    );
    let order: Vec<usize> = m.when_order.clone();
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t WHEN_ORDER[] = {};",
        idx(&order)
    );
    let mut dep0 = vec![0usize];
    let mut deps = Vec::new();
    for d in &m.when_deps {
        deps.extend(d.iter().copied());
        dep0.push(deps.len());
    }
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t DEP0[] = {};",
        idx(&dep0)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t DEPS[] = {};",
        idx(&deps)
    );
    let ins: Vec<usize> = m.inputs.iter().map(|&v| g.global(v as Slot)).collect();
    let outs: Vec<usize> = outputs.iter().map(|&v| g.global(v as Slot)).collect();
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t IN_IDX[] = {};",
        idx(&ins)
    );
    let _ = writeln!(
        src,
        "DL_UNUSED static const uint16_t OUT_IDX[] = {};\n",
        idx(&outs)
    );
    let _ = writeln!(src, "#define TIME_IDX {}", g.global(lay.time as Slot));
    let _ = writeln!(src, "#define INITIAL_IDX {}", g.global(lay.initial as Slot));
    let _ = writeln!(src, "#define STEP_SIZE RC({:e})", opts.step);
    let _ = writeln!(
        src,
        "#define T_START RC({:e})",
        m.experiment.start_time.unwrap_or(0.0)
    );
    let _ = writeln!(
        src,
        "#define RK4 {}\n",
        u8::from(opts.integrator == Integrator::Rk4)
    );
    let mut cg = String::from("static void const_globals(");
    let _ = writeln!(cg, "{p}_t *m)\n{{\n    real_t *const R = m->r;");
    for (i, v) in &const_globals {
        let _ = writeln!(cg, "    R[{i}] = {};", lit(*v));
    }
    cg.push_str("    (void)R;\n}\n\n");
    src.push_str(&cg);
    src.push_str(&runtime_core(&p, &up));

    // Register map, for debugging.
    let mut map = String::from("/* Persistent registers R[i]:\n");
    for (i, nme) in g.global_names.iter().enumerate() {
        if !nme.is_empty() {
            let _ = writeln!(map, " *  {i:5}  {}", nme.replace("*/", "* /"));
        }
    }
    map.push_str(" */\n");
    src.push_str(&map);

    // ---- header -------------------------------------------------------------------
    let port = |v: VarId, kind: &str, k: usize, vals: &[f64]| -> Port {
        let var = &m.vars[v as usize];
        Port {
            name: var.name.clone(),
            macro_name: format!("{up}_{kind}_{}", c_ident(&var.name)),
            index: k,
            unit: var.unit.clone(),
            start: vals.get(v as usize).copied().unwrap_or(0.0),
        }
    };
    let vals = crate::fmi::parameter_values(m);
    let in_ports: Vec<Port> = m
        .inputs
        .iter()
        .enumerate()
        .map(|(k, &v)| port(v, "IN", k, &vals))
        .collect();
    let out_ports: Vec<Port> = outputs
        .iter()
        .enumerate()
        .map(|(k, &v)| port(v, "OUT", k, &vals))
        .collect();
    let mut h = String::new();
    let guard = format!("{up}_H");
    let _ = writeln!(
        h,
        "/* {p}.h: embedded C code of the Modelica model {}.\n * Generated by DAEdalus. Do not edit.\n *\n * Usage:\n *   static {p}_t ctl;\n *   {p}_init(&ctl);                  parameters and default inputs\n *   {p}_set_input(&ctl, {up}_IN_..., value);\n *   {p}_start(&ctl);                 initial equations and events at the start time\n *   every {} s:\n *       {p}_set_input(...);  status = {p}_step(&ctl);  y = {p}_get_output(&ctl, {up}_OUT_...);\n */",
        m.name, opts.step
    );
    let _ = writeln!(
        h,
        "#ifndef {guard}\n#define {guard}\n\n#include <stdint.h>\n"
    );
    let _ = writeln!(h, "#ifdef __cplusplus\nextern \"C\" {{\n#endif\n");
    let _ = writeln!(
        h,
        "#ifndef DAEDALUS_REAL_T\n#define DAEDALUS_REAL_T\ntypedef {real} real_t;\n#endif\n"
    );
    let _ = writeln!(h, "#define {up}_STEP_SIZE {:e} /* s */", opts.step);
    let _ = writeln!(h, "#define {up}_N_INPUTS {}", in_ports.len());
    let _ = writeln!(h, "#define {up}_N_OUTPUTS {}", out_ports.len());
    let _ = writeln!(h, "#define {up}_N_STATES {n_states}");
    let _ = writeln!(h, "#define {up}_N_REGISTERS {}\n", g.n_global.max(1));
    for pt in &in_ports {
        let _ = writeln!(
            h,
            "#define {} {} /* {}{} */",
            pt.macro_name,
            pt.index,
            pt.name,
            unit_note(&pt.unit)
        );
    }
    for pt in &out_ports {
        let _ = writeln!(
            h,
            "#define {} {} /* {}{} */",
            pt.macro_name,
            pt.index,
            pt.name,
            unit_note(&pt.unit)
        );
    }
    let _ = writeln!(h, "\n/* Status bits returned by {p}_start and {p}_step. */");
    let _ = writeln!(
        h,
        "#define {up}_OK 0\n#define {up}_NONLINEAR_FAILED 1\n#define {up}_EVENTS_FAILED 2\n#define {up}_TERMINATED 4\n#define {up}_ASSERTION 8\n"
    );
    let _ = writeln!(
        h,
        "typedef struct {{\n    real_t r[{}];\n    uint32_t k;        /* steps taken since start */\n    uint8_t status;\n    uint8_t cond_prev[{}];\n}} {p}_t;\n",
        g.n_global.max(1),
        n_br.max(1)
    );
    let _ = writeln!(h, "void {p}_init({p}_t *m);");
    let _ = writeln!(h, "uint8_t {p}_start({p}_t *m);");
    let _ = writeln!(h, "uint8_t {p}_step({p}_t *m);");
    let _ = writeln!(h, "void {p}_set_input({p}_t *m, uint16_t i, real_t v);");
    let _ = writeln!(h, "real_t {p}_get_output(const {p}_t *m, uint16_t i);");
    let _ = writeln!(h, "real_t {p}_time(const {p}_t *m);");
    let _ = writeln!(h, "extern const char *const {p}_input_names[];");
    let _ = writeln!(h, "extern const char *const {p}_output_names[];\n");
    let _ = writeln!(h, "#ifdef __cplusplus\n}}\n#endif\n\n#endif /* {guard} */");
    let names = |ps: &[Port]| -> String {
        if ps.is_empty() {
            "{0}".into()
        } else {
            format!(
                "{{{}}}",
                ps.iter()
                    .map(|p| format!("\"{}\"", p.name.replace('"', "'")))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        }
    };
    let _ = writeln!(
        src,
        "const char *const {p}_input_names[] = {};",
        names(&in_ports)
    );
    let _ = writeln!(
        src,
        "const char *const {p}_output_names[] = {};",
        names(&out_ports)
    );

    let real_bytes = match opts.real {
        RealType::Float => 4,
        RealType::Double => 8,
    };
    let manifest = Manifest {
        model: m.name.clone(),
        prefix: p.clone(),
        step: opts.step,
        start_time: m.experiment.start_time.unwrap_or(0.0),
        real: opts.real,
        integrator: opts.integrator,
        inputs: in_ports,
        outputs: out_ports,
        n_states,
        n_newton: m.newton.len(),
        registers: g.n_global,
        instance_bytes: g.n_global.max(1) * real_bytes + 4 + 1 + n_br.max(1),
    };
    let _ = g.opts;
    Ok(Generated {
        manifest,
        header: h,
        source: src,
    })
}

fn unit_note(u: &str) -> String {
    if u.is_empty() {
        String::new()
    } else {
        format!(" [{u}]")
    }
}

/// Registers that some program reads before writing them (in program order, ignoring
/// branches) rely on the value from an earlier run: they must persist.
fn promote_read_before_write(g: &mut Gen, params: &[Vec<Ins>]) {
    let m = g.m;
    let all_params: Vec<Ins> = params.iter().flatten().copied().collect();
    let mut progs: Vec<&[Ins]> = vec![
        &all_params,
        &m.starts,
        &m.init,
        &m.ode,
        &m.zc,
        &m.when_prog,
        &m.outputs,
    ];
    for b in &m.newton {
        progs.push(&b.body);
        progs.push(&b.jac);
    }
    let mut promote: Vec<Slot> = Vec::new();
    for prog in progs {
        let mut written: FxHashSet<Slot> = FxHashSet::default();
        let (mut r, mut w) = (Vec::new(), Vec::new());
        for ins in prog {
            r.clear();
            w.clear();
            reads_writes(ins, &mut r, &mut w);
            for &s in &r {
                if matches!(g.place[s as usize], Place::Local) && !written.contains(&s) {
                    promote.push(s);
                }
            }
            written.extend(w.iter().copied());
        }
    }
    promote.sort_unstable();
    promote.dedup();
    for s in promote {
        g.place[s as usize] = Place::Global(g.n_global);
        g.global_names.push(String::new());
        g.n_global += 1;
    }
}

/// Type-dependent math macros and small helpers.
fn runtime_prelude(real: RealType) -> String {
    let f = match real {
        RealType::Float => "f",
        RealType::Double => "",
    };
    let mut s = String::new();
    let _ = writeln!(s, "#define RC(x) ((real_t)(x))");
    let _ = writeln!(
        s,
        "#if defined(__GNUC__) || defined(__clang__)\n#define DL_UNUSED __attribute__((unused))\n#else\n#define DL_UNUSED\n#endif"
    );
    let _ = writeln!(
        s,
        "#define DL_INF ((real_t)INFINITY)\n#define DL_NAN ((real_t)NAN)"
    );
    for (mac, fun) in [
        ("SIN", "sin"),
        ("COS", "cos"),
        ("TAN", "tan"),
        ("ASIN", "asin"),
        ("ACOS", "acos"),
        ("ATAN", "atan"),
        ("SINH", "sinh"),
        ("COSH", "cosh"),
        ("TANH", "tanh"),
        ("EXP", "exp"),
        ("LOG", "log"),
        ("LOG10", "log10"),
        ("SQRT", "sqrt"),
        ("FABS", "fabs"),
        ("FLOOR", "floor"),
        ("CEIL", "ceil"),
        ("TRUNC", "trunc"),
    ] {
        let _ = writeln!(s, "#define DL_{mac}(x) {fun}{f}(x)");
    }
    let _ = writeln!(s, "#define DL_POW(x, y) pow{f}((x), (y))");
    let _ = writeln!(s, "#define DL_ATAN2(x, y) atan2{f}((x), (y))");
    s.push_str(
        r#"
DL_UNUSED static real_t dl_powi(real_t x, int k)
{
    real_t r = RC(1.0);
    unsigned int n = (unsigned int)(k < 0 ? -k : k);
    while (n) {
        if (n & 1u) r *= x;
        x *= x;
        n >>= 1;
    }
    return k < 0 ? RC(1.0) / r : r;
}
DL_UNUSED static real_t dl_sign(real_t x) { return x > RC(0.0) ? RC(1.0) : (x < RC(0.0) ? RC(-1.0) : RC(0.0)); }
DL_UNUSED static real_t dl_min(real_t x, real_t y) { return (y < x || x != x) ? y : x; }
DL_UNUSED static real_t dl_max(real_t x, real_t y) { return (y > x || x != x) ? y : x; }
DL_UNUSED static real_t dl_mod(real_t x, real_t y) { return x - DL_FLOOR(x / y) * y; }
DL_UNUSED static real_t dl_rem(real_t x, real_t y) { return x - DL_TRUNC(x / y) * y; }
DL_UNUSED static real_t dl_atan2(real_t x, real_t y) { return (x == RC(0.0) && y == RC(0.0)) ? RC(0.0) : DL_ATAN2(x, y); }

typedef struct {
    const real_t *data;
    const real_t *slopes;
    uint16_t rows, cols;
    uint8_t extrapolation, smoothness;
    real_t shift, start;
} dl_table;

#define TB(t, r, c) ((t)->data[(uint32_t)(r) * (t)->cols + (c)])
DL_UNUSED static real_t dl_tmap(const dl_table *t, real_t x)
{
    x -= t->shift;
    if (t->extrapolation == 2 && t->rows >= 2) {
        real_t x0 = TB(t, 0, 0), p = TB(t, t->rows - 1, 0) - x0;
        if (p > RC(0.0)) {
            real_t q = (x - x0) - DL_FLOOR((x - x0) / p) * p;
            return x0 + q;
        }
    }
    return x;
}
DL_UNUSED static uint16_t dl_locate(const dl_table *t, real_t x, real_t *w)
{
    uint16_t n = t->rows, lo = 0, hi = (uint16_t)(n - 1);
    real_t x0, x1;
    if (x <= TB(t, 0, 0)) lo = 0;
    else if (x >= TB(t, n - 1, 0)) lo = (uint16_t)(n - 2);
    else {
        while (hi - lo > 1) {
            uint16_t mid = (uint16_t)((lo + hi) / 2);
            if (TB(t, mid, 0) <= x) lo = mid; else hi = mid;
        }
    }
    x0 = TB(t, lo, 0);
    x1 = TB(t, lo + 1, 0);
    *w = x1 > x0 ? (x - x0) / (x1 - x0) : (x >= x1 ? RC(1.0) : RC(0.0));
    return lo;
}
DL_UNUSED static real_t dl_interp(const dl_table *t, uint16_t col, real_t x)
{
    real_t w, y0, y1;
    uint16_t r;
    int inside;
    if (x < t->start || t->rows == 0) return RC(0.0);
    if (t->rows == 1) return TB(t, 0, col);
    x = dl_tmap(t, x);
    r = dl_locate(t, x, &w);
    y0 = TB(t, r, col);
    y1 = TB(t, r + 1, col);
    inside = w >= RC(0.0) && w <= RC(1.0);
    if (!inside && t->extrapolation == 0) return w < RC(0.0) ? y0 : y1;
    if (t->smoothness == 1) return w >= RC(1.0) ? y1 : y0;
    if (t->smoothness == 2) {
        real_t h = TB(t, r + 1, 0) - TB(t, r, 0);
        real_t s0 = t->slopes[(uint32_t)r * t->cols + col], s1 = t->slopes[(uint32_t)(r + 1) * t->cols + col];
        real_t w2 = w * w, w3 = w2 * w;
        if (!inside) return w < RC(0.0) ? y0 + s0 * w * h : y1 + s1 * (w - RC(1.0)) * h;
        return (RC(2.0) * w3 - RC(3.0) * w2 + RC(1.0)) * y0 + (w3 - RC(2.0) * w2 + w) * h * s0
             + (RC(-2.0) * w3 + RC(3.0) * w2) * y1 + (w3 - w2) * h * s1;
    }
    return y0 + w * (y1 - y0);
}
DL_UNUSED static real_t dl_slope(const dl_table *t, uint16_t col, real_t x)
{
    real_t w, x0, x1, h;
    uint16_t r;
    int inside;
    if (x < t->start || t->rows < 2) return RC(0.0);
    x = dl_tmap(t, x);
    r = dl_locate(t, x, &w);
    inside = w >= RC(0.0) && w <= RC(1.0);
    if ((!inside && t->extrapolation == 0) || t->smoothness == 1) return RC(0.0);
    x0 = TB(t, r, 0);
    x1 = TB(t, r + 1, 0);
    if (x1 <= x0) return RC(0.0);
    h = x1 - x0;
    if (t->smoothness == 2) {
        real_t s0 = t->slopes[(uint32_t)r * t->cols + col], s1 = t->slopes[(uint32_t)(r + 1) * t->cols + col];
        real_t y0 = TB(t, r, col), y1 = TB(t, r + 1, col), w2 = w * w;
        if (!inside) return w < RC(0.0) ? s0 : s1;
        return ((RC(6.0) * w2 - RC(6.0) * w) * y0 + (RC(3.0) * w2 - RC(4.0) * w + RC(1.0)) * h * s0
              + (RC(-6.0) * w2 + RC(6.0) * w) * y1 + (RC(3.0) * w2 - RC(2.0) * w) * h * s1) / h;
    }
    return (TB(t, r + 1, col) - TB(t, r, col)) / h;
}
DL_UNUSED static real_t dl_next_event(const dl_table *t, real_t x)
{
    real_t eps, xs;
    uint16_t r;
    if (x < t->start) return t->start;
    if (t->rows == 0) return DL_INF;
    eps = RC(1e-6) * (RC(1.0) + DL_FABS(x));
    xs = x - t->shift;
    if (t->extrapolation == 2 && t->rows >= 2) {
        real_t x0 = TB(t, 0, 0), x1 = TB(t, t->rows - 1, 0), p = x1 - x0;
        if (p > RC(0.0)) {
            real_t k = DL_FLOOR((xs - x0) / p), local = xs - k * p;
            for (r = 0; r < t->rows; r++)
                if (TB(t, r, 0) > local + eps) return TB(t, r, 0) + k * p + t->shift;
            return x1 + k * p + t->shift;
        }
    }
    for (r = 0; r < t->rows; r++)
        if (TB(t, r, 0) > xs + eps) return TB(t, r, 0) + t->shift;
    return DL_INF;
}

"#,
    );
    s
}

/// Damped Newton with the block's exact Jacobian and partial-pivoting LU.
fn newton_driver(p: &str) -> String {
    format!(
        r#"#define DL_NEWTON_TOL (sizeof(real_t) == 4 ? RC(1e-5) : RC(1e-10))
static uint8_t dl_lu_solve(real_t *A, real_t *b, uint8_t n)
{{
    uint8_t i, j, k, p;
    for (k = 0; k < n; k++) {{
        real_t best = DL_FABS(A[k * n + k]);
        p = k;
        for (i = (uint8_t)(k + 1); i < n; i++)
            if (DL_FABS(A[i * n + k]) > best) {{ best = DL_FABS(A[i * n + k]); p = i; }}
        if (!(best > RC(0.0))) return 0;
        if (p != k) {{
            real_t t;
            for (j = 0; j < n; j++) {{ t = A[k * n + j]; A[k * n + j] = A[p * n + j]; A[p * n + j] = t; }}
            t = b[k]; b[k] = b[p]; b[p] = t;
        }}
        for (i = (uint8_t)(k + 1); i < n; i++) {{
            real_t f = A[i * n + k] / A[k * n + k];
            for (j = k; j < n; j++) A[i * n + j] -= f * A[k * n + j];
            b[i] -= f * b[k];
        }}
    }}
    for (k = n; k-- > 0;) {{
        real_t s = b[k];
        for (j = (uint8_t)(k + 1); j < n; j++) s -= A[k * n + j] * b[j];
        b[k] = s / A[k * n + k];
    }}
    return 1;
}}
DL_UNUSED static real_t dl_norm(const real_t *F, uint8_t n)
{{
    real_t r = RC(0.0);
    uint8_t i;
    for (i = 0; i < n; i++) if (!(DL_FABS(F[i]) <= r)) r = DL_FABS(F[i]);
    return r;
}}
static void dl_newton({p}_t *m, const dl_nblock *b)
{{
    real_t *const R = m->r;
    real_t F[DL_NB_MAX], J[DL_NB_MAX * DL_NB_MAX], x0[DL_NB_MAX], dx[DL_NB_MAX], entry[DL_NB_MAX];
    uint8_t i, it, n = b->n;
    real_t norm = RC(0.0), scale;
    for (i = 0; i < n; i++) entry[i] = R[b->tears[i]];
    for (it = 0; it < 50; it++) {{
        real_t lambda = RC(1.0), step = RC(0.0);
        b->body(m, F);
        norm = dl_norm(F, n);
        scale = RC(1.0);
        for (i = 0; i < n; i++) if (DL_FABS(R[b->tears[i]]) > scale) scale = DL_FABS(R[b->tears[i]]);
        if (norm <= DL_NEWTON_TOL * scale) return;
        b->jac(m, J);
        for (i = 0; i < n; i++) {{ dx[i] = -F[i]; x0[i] = R[b->tears[i]]; }}
        if (!dl_lu_solve(J, dx, n)) break;
        for (;;) {{
            for (i = 0; i < n; i++) {{
                real_t x = x0[i] + lambda * dx[i];
                if (x < b->lo[i] && x0[i] >= b->lo[i]) x = x0[i] + (b->lo[i] - x0[i]) * RC(0.5);
                else if (x > b->hi[i] && x0[i] <= b->hi[i]) x = x0[i] + (b->hi[i] - x0[i]) * RC(0.5);
                R[b->tears[i]] = x;
            }}
            if (b->linear || lambda < RC(1e-3)) break;
            b->body(m, F);
            if (dl_norm(F, n) < norm * (RC(1.0) - RC(1e-4) * lambda)) break;
            lambda *= RC(0.5);
        }}
        for (i = 0; i < n; i++) {{
            real_t s = DL_FABS(lambda * dx[i]) / (RC(1.0) + DL_FABS(x0[i]));
            if (s > step) step = s;
        }}
        if (step < (sizeof(real_t) == 4 ? RC(1e-7) : RC(1e-13))) {{ b->body(m, F); return; }}
    }}
    b->body(m, F);
    norm = dl_norm(F, n);
    scale = RC(1.0);
    for (i = 0; i < n; i++) if (DL_FABS(R[b->tears[i]]) > scale) scale = DL_FABS(R[b->tears[i]]);
    if (!(norm <= (sizeof(real_t) == 4 ? RC(1e-3) : RC(1e-6)) * scale)) {{
        for (i = 0; i < n; i++) R[b->tears[i]] = entry[i];
        b->body(m, F);
        m->status |= 1u;
    }}
}}

"#
    )
}

/// Integrator, event iteration and the public API.
fn runtime_core(p: &str, up: &str) -> String {
    format!(
        r#"static void save_pre({p}_t *m)
{{
    int i;
    for (i = 0; i < N_PRE; i++) m->r[PRE_PAIRS[2 * i]] = m->r[PRE_PAIRS[2 * i + 1]];
}}

/* Latch the relations at the current point; the time is nudged forward by a small
 * fraction of the step so that sample instants falling on this step fire here despite
 * rounding. Returns whether any relation changed. */
static uint8_t latch({p}_t *m)
{{
    real_t *const R = m->r;
    real_t t = R[TIME_IDX];
    uint8_t changed = 0;
    int i;
    R[TIME_IDX] = t + RC(0.05) * STEP_SIZE;
    f_zc(m);
    R[TIME_IDX] = t;
    for (i = 0; i < N_REL; i++) {{
        real_t v = R[REL_PAIRS[2 * i + 1]];
        if (R[REL_PAIRS[2 * i]] != v) {{ R[REL_PAIRS[2 * i]] = v; changed = 1; }}
    }}
    return changed;
}}

static void when_conds({p}_t *m, uint8_t *c)
{{
    int i;
    f_when(m);
    for (i = 0; i < N_BRANCH; i++) c[i] = m->r[BR_COND[i]] != RC(0.0);
}}

/* Execute the when-clauses whose condition became true (simultaneous semantics: new
 * values are computed first, then assigned). Returns whether any clause fired. */
static uint8_t fire({p}_t *m, const uint8_t *cond, const uint8_t *prev)
{{
    real_t *const R = m->r;
    uint16_t upd_var[N_ACTION + 1];
    real_t upd_val[N_ACTION + 1];
    uint8_t fired[N_CLAUSE + 1];
    int n_upd = 0, o, k, a;
    uint8_t any = 0;
    memset(fired, 0, sizeof fired);
    for (o = 0; o < N_CLAUSE; o++) {{
        int wi = WHEN_ORDER[o], bi;
        for (bi = CLAUSE_BR0[wi]; bi < CLAUSE_BR0[wi + 1]; bi++) {{
            if (cond[bi] && !prev[bi]) {{
                uint8_t chained = BR_FLAGS[bi] & 1u;
                any = 1;
                if (n_upd) {{
                    uint8_t dep = 0;
                    for (k = DEP0[wi]; k < DEP0[wi + 1]; k++) if (fired[DEPS[k]]) dep = 1;
                    if (dep) {{
                        for (k = 0; k < n_upd; k++) R[upd_var[k]] = upd_val[k];
                        n_upd = 0;
                        f_ode(m);
                        f_when(m);
                    }}
                }}
                fired[wi] = 1;
                for (a = BR_ACT[bi]; a < BR_ACT[bi + 1]; a++) {{
                    if (ACT_KIND[a] == 0) {{
                        if (chained) {{
                            if (a > BR_ACT[bi]) f_when(m);
                            R[ACT_A[a]] = R[ACT_B[a]];
                        }} else {{
                            upd_var[n_upd] = ACT_A[a];
                            upd_val[n_upd] = R[ACT_B[a]];
                            n_upd++;
                        }}
                    }} else if (ACT_KIND[a] == 1) {{
                        m->status |= 4u;
                    }} else if (R[ACT_A[a]] == RC(0.0)) {{
                        m->status |= 8u;
                    }}
                }}
                break;
            }}
            if (cond[bi]) break;
        }}
    }}
    for (k = 0; k < n_upd; k++) R[upd_var[k]] = upd_val[k];
    (void)upd_var; (void)upd_val;
    return any;
}}

/* Event iteration at the current time until the discrete values settle. */
static uint8_t events({p}_t *m)
{{
    real_t *const R = m->r;
    uint8_t conds[N_BRANCH + 1], seen[N_BRANCH + 1], now[N_BRANCH + 1];
    uint8_t iter, j, rel_changed, fired, disc_changed;
    int i;
    for (iter = 0; iter < 100; iter++) {{
        save_pre(m);
        rel_changed = latch(m);
        f_ode(m);
        when_conds(m, conds);
        fired = fire(m, conds, m->cond_prev);
        if (fired) {{
            for (j = 0; j < 100; j++) {{
                uint8_t more;
                f_ode(m);
                rel_changed |= latch(m);
                f_ode(m);
                when_conds(m, now);
                for (i = 0; i < N_BRANCH; i++) seen[i] = conds[i] || m->cond_prev[i];
                more = fire(m, now, seen);
                memcpy(conds, now, N_BRANCH);
                if (!more) break;
            }}
        }}
        memcpy(m->cond_prev, conds, N_BRANCH);
        f_ode(m);
        disc_changed = 0;
        for (i = 0; i < N_DISC; i++) {{
            real_t a = R[DISC_PAIRS[2 * i]], b = R[DISC_PAIRS[2 * i + 1]];
            if (a != b && !(a != a && b != b)) disc_changed = 1;
        }}
        if (!rel_changed && !fired && !disc_changed) {{
            save_pre(m);
            return 0;
        }}
    }}
    (void)seen; (void)now;
    return 2u;
}}

void {p}_init({p}_t *m)
{{
    memset(m, 0, sizeof *m);
    const_globals(m);
    f_params(m);
}}

uint8_t {p}_start({p}_t *m)
{{
    real_t *const R = m->r;
    uint8_t c1[N_BRANCH + 1], c0[N_BRANCH + 1], edge[N_BRANCH + 1];
    int i;
    m->status = 0;
    m->k = 0;
    R[TIME_IDX] = T_START;
    R[INITIAL_IDX] = RC(1.0);
    f_starts(m);
    save_pre(m);
    f_init(m);
    for (i = 0; i < N_PRE_INIT; i++) R[PRE_INIT[2 * i]] = R[PRE_INIT[2 * i + 1]];
    latch(m);
    for (i = 0; i < N_SAMPLE_REL; i++) R[SAMPLE_REL[i]] = RC(0.0);
    f_ode(m);
    when_conds(m, c1);
    R[INITIAL_IDX] = RC(0.0);
    when_conds(m, c0);
    R[INITIAL_IDX] = RC(1.0);
    for (i = 0; i < N_BRANCH; i++) edge[i] = c0[i] && !(BR_FLAGS[i] & 2u);
    fire(m, c1, edge);
    memcpy(m->cond_prev, c0, N_BRANCH);
    R[INITIAL_IDX] = RC(0.0);
    m->status |= events(m);
    f_outputs(m);
    (void)c1; (void)c0; (void)edge;
    return m->status;
}}

uint8_t {p}_step({p}_t *m)
{{
    real_t *const R = m->r;
    real_t t0 = R[TIME_IDX], h = STEP_SIZE;
    int i;
#if N_STATES > 0
    real_t x0[N_STATES], acc[N_STATES];
    f_ode(m);
    for (i = 0; i < N_STATES; i++) {{ x0[i] = R[STATE_IDX[i]]; acc[i] = R[DERIV_IDX[i]]; }}
#if RK4
    {{
        static const real_t c[3] = {{RC(0.5), RC(0.5), RC(1.0)}};
        static const real_t w[3] = {{RC(2.0), RC(2.0), RC(1.0)}};
        real_t k[N_STATES];
        uint8_t s;
        for (i = 0; i < N_STATES; i++) k[i] = acc[i];
        for (s = 0; s < 3; s++) {{
            R[TIME_IDX] = t0 + c[s] * h;
            for (i = 0; i < N_STATES; i++) R[STATE_IDX[i]] = x0[i] + c[s] * h * k[i];
            f_ode(m);
            for (i = 0; i < N_STATES; i++) {{ k[i] = R[DERIV_IDX[i]]; acc[i] += w[s] * k[i]; }}
        }}
        for (i = 0; i < N_STATES; i++) acc[i] /= RC(6.0);
    }}
#endif
    m->k++;
    R[TIME_IDX] = T_START + (real_t)m->k * h;
    for (i = 0; i < N_STATES; i++) R[STATE_IDX[i]] = x0[i] + h * acc[i];
#else
    m->k++;
    R[TIME_IDX] = T_START + (real_t)m->k * h;
#endif
    (void)t0;
    (void)i;
    f_ode(m);
    m->status |= events(m);
    f_outputs(m);
    return m->status;
}}

void {p}_set_input({p}_t *m, uint16_t i, real_t v)
{{
#if {up}_N_INPUTS > 0
    if (i < {up}_N_INPUTS) m->r[IN_IDX[i]] = v;
#else
    (void)m; (void)i; (void)v;
#endif
}}

real_t {p}_get_output(const {p}_t *m, uint16_t i)
{{
#if {up}_N_OUTPUTS > 0
    return i < {up}_N_OUTPUTS ? m->r[OUT_IDX[i]] : RC(0.0);
#else
    (void)m; (void)i;
    return RC(0.0);
#endif
}}

real_t {p}_time(const {p}_t *m)
{{
    return m->r[TIME_IDX];
}}

"#
    )
}

/// A host test driver (`<prefix>_main.c`): reads input rows `time,u1,u2...` from stdin
/// (the first row's inputs initialise the model) and prints `time,y1,y2...` per step.
pub fn host_main(g: &Manifest) -> String {
    let p = &g.prefix;
    let up = p.to_uppercase();
    format!(
        r#"/* Software-in-the-loop driver for {p}: one line of inputs per step on stdin,
 * one line of outputs per step on stdout (CSV). Generated by DAEdalus. */
#include <stdio.h>
#include <stdlib.h>
#include "{p}.h"

int main(int argc, char **argv)
{{
    static {p}_t m;
    char line[4096];
    long steps = argc > 1 ? atol(argv[1]) : -1;
    int first = 1;
    int i;
    {p}_init(&m);
    printf("time");
    for (i = 0; i < {up}_N_OUTPUTS; i++) printf(",%s", {p}_output_names[i]);
    printf("\n");
    for (;;) {{
        char *s;
        uint8_t st;
        if (steps >= 0 && (long)m.k >= steps && !first) break;
        if (!fgets(line, sizeof line, stdin)) break;
        s = line;
        for (i = 0; i < {up}_N_INPUTS; i++) {{
            char *e;
            double v = strtod(s, &e);
            if (e == s) break;
            {p}_set_input(&m, (uint16_t)i, (real_t)v);
            s = e;
            while (*s == ',' || *s == ' ' || *s == '\t') s++;
        }}
        st = first ? {p}_start(&m) : {p}_step(&m);
        first = 0;
        printf("%.9g", (double){p}_time(&m));
        for (i = 0; i < {up}_N_OUTPUTS; i++) printf(",%.9g", (double){p}_get_output(&m, (uint16_t)i));
        printf("\n");
        if (st & ~{up}_ASSERTION) {{
            fprintf(stderr, "status %u at t = %g\n", st, (double){p}_time(&m));
            if (st & ({up}_NONLINEAR_FAILED | {up}_EVENTS_FAILED)) return 2;
            if (st & {up}_TERMINATED) break;
        }}
    }}
    return 0;
}}
"#
    )
}
