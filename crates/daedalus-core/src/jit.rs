//! Compiled tapes: the hot tapes of a model (equations, outputs, zero crossings, Newton
//! blocks) translated to machine code with Cranelift, so [`Runtime::run`] executes them
//! without the interpreter's per-instruction dispatch.
//!
//! Results are bit-identical to the interpreter: every instruction compiles to the same
//! IEEE operation in the same order (no fused multiply-add, no reassociation), and
//! everything else (transcendental functions, `min`/`max`, tables, delays, function calls,
//! Newton blocks) calls the very Rust code the interpreter runs.
//!
//! In WebAssembly the same tapes become a generated WebAssembly module per tape, which the
//! page instantiates against this module's memory and function table (see
//! [`set_wasm_hooks`]); the browser compiles it to machine code like any other module.
//!
//! On by default with the `jit` feature; `DAEDALUS_JIT=0` or [`set_enabled`] (the CLI's
//! `--no-jit`) turns it off. Without the feature tapes are always interpreted.

use crate::codegen::Model;
#[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
pub(crate) use imp::{Compiled, compile};
#[cfg(all(feature = "jit", target_arch = "wasm32"))]
pub(crate) use wimp::{Compiled, compile};

/// Where a model keeps its compiled tapes (built on first use, not saved or cloned).
#[derive(Default)]
pub struct JitSlot {
    #[cfg(feature = "jit")]
    cell: std::sync::OnceLock<Option<std::sync::Arc<Compiled>>>,
}

impl Clone for JitSlot {
    /// A clone has its own tapes (at other addresses): it compiles them again when used.
    fn clone(&self) -> Self {
        JitSlot::default()
    }
}

#[cfg(feature = "jit")]
mod flag {
    use std::sync::atomic::{AtomicU8, Ordering};
    /// 0: not decided yet (read `DAEDALUS_JIT`, on unless it is `0`), 1: off, 2: on.
    static STATE: AtomicU8 = AtomicU8::new(0);
    pub fn enabled() -> bool {
        match STATE.load(Ordering::Relaxed) {
            0 => {
                let on = std::env::var("DAEDALUS_JIT").map_or(true, |v| v != "0");
                STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
                on
            }
            s => s == 2,
        }
    }
    pub fn set(on: bool) {
        STATE.store(if on { 2 } else { 1 }, Ordering::Relaxed);
    }
}

/// Whether models compile their tapes (see the module documentation).
pub fn enabled() -> bool {
    #[cfg(feature = "jit")]
    return flag::enabled();
    #[cfg(not(feature = "jit"))]
    false
}

/// Turn compiled tapes on or off for models run from now on. Without the `jit` feature
/// (or in WebAssembly) this does nothing.
pub fn set_enabled(on: bool) {
    #[cfg(feature = "jit")]
    flag::set(on);
    #[cfg(not(feature = "jit"))]
    let _ = on;
}

/// How the page installs a generated tape module: `install(bytes)` instantiates it with
/// this module's memory and function table and returns the table index of its `run`
/// export; `remove(index)` frees the slot.
#[cfg(all(feature = "jit", target_arch = "wasm32"))]
pub type WasmHooks = (fn(&[u8]) -> Option<u32>, fn(u32));
#[cfg(all(feature = "jit", target_arch = "wasm32"))]
static HOOKS: std::sync::OnceLock<WasmHooks> = std::sync::OnceLock::new();

/// Register the page's installer of generated tape modules (WebAssembly only); without
/// it tapes are interpreted.
#[cfg(all(feature = "jit", target_arch = "wasm32"))]
pub fn set_wasm_hooks(install: fn(&[u8]) -> Option<u32>, remove: fn(u32)) {
    let _ = HOOKS.set((install, remove));
}

/// How many of the model's tapes run compiled (0 when compiled tapes are off or not
/// built yet).
pub fn compiled_tapes(m: &Model) -> usize {
    #[cfg(feature = "jit")]
    return m
        .jit
        .cell
        .get()
        .and_then(|c| c.as_ref())
        .map_or(0, |c| c.len());
    #[cfg(not(feature = "jit"))]
    {
        let _ = m;
        0
    }
}

#[cfg(feature = "jit")]
impl JitSlot {
    /// The model's compiled tapes, compiling them on first use; None when compiled tapes
    /// are off or Cranelift does not support this machine.
    pub(crate) fn get(&self, m: &Model) -> Option<std::sync::Arc<Compiled>> {
        if !enabled() {
            return None;
        }
        self.cell
            .get_or_init(|| compile(m).map(std::sync::Arc::new))
            .clone()
    }
}

/// What compiled code calls back into: the interpreter's own functions, so both give the
/// same results.
#[cfg(feature = "jit")]
mod helpers {
    use super::Model;
    use crate::codegen::{Ins, Runtime};
    use crate::expr::{Fn1, Fn2, eval_f1, eval_f2};

    // Helpers called from compiled code: the interpreter's own functions.
    macro_rules! f1_helpers {
        ($($name:ident => $f:ident),* $(,)?) => {
            $(extern "C" fn $name(x: f64) -> f64 { eval_f1(Fn1::$f, x) })*
            pub fn f1_helper(f: Fn1) -> Option<usize> {
                match f {
                    $(Fn1::$f => Some($name as *const () as usize),)*
                    _ => None,
                }
            }
        };
    }
    f1_helpers!(
        h_sin => Sin, h_cos => Cos, h_tan => Tan, h_asin => Asin, h_acos => Acos,
        h_atan => Atan, h_sinh => Sinh, h_cosh => Cosh, h_tanh => Tanh, h_exp => Exp,
        h_log => Log, h_log10 => Log10, h_floor => Floor, h_ceil => Ceil,
    );
    macro_rules! f2_helpers {
        ($($name:ident => $f:ident),* $(,)?) => {
            $(extern "C" fn $name(x: f64, y: f64) -> f64 { eval_f2(Fn2::$f, x, y) })*
            pub fn f2_helper(f: Fn2) -> Option<usize> {
                match f {
                    $(Fn2::$f => Some($name as *const () as usize),)*
                    _ => None,
                }
            }
        };
    }
    f2_helpers!(h_atan2 => Atan2, h_min => Min, h_max => Max, h_div => Div, h_mod => Mod, h_rem => Rem);
    pub extern "C" fn h_pow(x: f64, y: f64) -> f64 {
        x.powf(y)
    }
    pub extern "C" fn h_powi(x: f64, k: i32) -> f64 {
        x.powi(k)
    }
    /// Execute one table, delay, call or Newton instruction; returns the register file's
    /// (possibly moved) address.
    pub extern "C" fn h_slow(rt: *mut u8, ins: *const Ins) -> *mut f64 {
        // SAFETY: `rt` is the runtime running this tape (see `Runtime::run`) and `ins` an
        // instruction of a tape of its model.
        let rt = unsafe { &mut *(rt as *mut Runtime<'static>) };
        // A panic cannot unwind through compiled code: it is kept, the rest of the tape
        // runs without effects, and `Runtime::run` resumes it once the tape returns.
        if rt.jit_panic.is_none() {
            let ins = unsafe { &*ins };
            if let Err(p) =
                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| rt.exec_slow(ins)))
            {
                rt.jit_panic = Some(p);
            }
        }
        rt.r.as_mut_ptr()
    }

    /// The tapes worth compiling: those run at every step or Newton iteration. The
    /// event code (`when_prog`) is left to the interpreter: it runs only at events, and
    /// compiled it is a large stretch of machine code that is cold whenever it runs,
    /// slower than interpreting it (Rectifier12pulseFFT: 1.3 s slower over 4,600 events).
    pub fn hot_tapes(m: &Model) -> Vec<&[Ins]> {
        let mut v: Vec<&[Ins]> = vec![&m.ode, &m.outputs, &m.zc];
        for b in &m.newton {
            v.push(&b.body);
            v.push(&b.jac);
        }
        v.retain(|t| !t.is_empty());
        v
    }
}

#[cfg(all(feature = "jit", not(target_arch = "wasm32")))]
mod imp {
    use super::Model;
    use super::helpers::*;
    use crate::codegen::Ins;
    use crate::expr::{Cmp, Fn1, Fn2};
    use cranelift_codegen::ir::condcodes::FloatCC;
    use cranelift_codegen::ir::{
        AbiParam, Block, InstBuilder, MemFlagsData, SigRef, Signature, UserFuncName, Value, types,
    };
    use cranelift_codegen::settings::{self, Configurable};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
    use cranelift_jit::{JITBuilder, JITModule};
    use cranelift_module::{Linkage, Module, default_libcall_names};
    use rustc_hash::FxHashMap;

    /// A compiled tape: `f(runtime, registers)`.
    pub type TapeFn = unsafe extern "C" fn(*mut u8, *mut f64);

    pub struct Compiled {
        module: Option<JITModule>,
        /// By the tape's address and length.
        fns: FxHashMap<(usize, usize), TapeFn>,
    }

    // SAFETY: after `finalize_definitions` the module is only kept to free its memory on
    // drop; the code it holds is immutable and reentrant.
    unsafe impl Send for Compiled {}
    unsafe impl Sync for Compiled {}

    impl Drop for Compiled {
        fn drop(&mut self) {
            if let Some(m) = self.module.take() {
                // SAFETY: the functions are only reachable through `fns`, dropped with us.
                unsafe { m.free_memory() };
            }
        }
    }

    impl Compiled {
        pub fn len(&self) -> usize {
            self.fns.len()
        }

        #[inline]
        pub fn get(&self, prog: &[Ins]) -> Option<TapeFn> {
            if prog.is_empty() {
                return None;
            }
            self.fns.get(&(prog.as_ptr() as usize, prog.len())).copied()
        }
    }

    pub fn compile(m: &Model) -> Option<Compiled> {
        let t0 = std::time::Instant::now();
        let r = compile_inner(m);
        if std::env::var_os("DAEDALUS_JIT_TIMING").is_some() {
            let n: usize = hot_tapes(m).iter().map(|t| t.len()).sum();
            eprintln!(
                "jit: {n} instructions compiled in {:.1} ms",
                t0.elapsed().as_secs_f64() * 1e3
            );
        }
        r
    }

    fn compile_inner(m: &Model) -> Option<Compiled> {
        let mut flags = settings::builder();
        flags.set("opt_level", "speed").ok()?;
        flags.set("is_pic", "false").ok()?;
        flags.set("use_colocated_libcalls", "false").ok()?;
        let isa = cranelift_native::builder()
            .ok()?
            .finish(settings::Flags::new(flags))
            .ok()?;
        let mut module = JITModule::new(JITBuilder::with_isa(isa, default_libcall_names()));
        let ptr = module.target_config().pointer_type();
        let mut ctx = module.make_context();
        let mut fctx = FunctionBuilderContext::new();
        let mut ids = Vec::new();
        for (k, tape) in hot_tapes(m).into_iter().enumerate() {
            ctx.func.signature.params = vec![AbiParam::new(ptr), AbiParam::new(ptr)];
            ctx.func.name = UserFuncName::user(0, k as u32);
            emit(
                FunctionBuilder::new(&mut ctx.func, &mut fctx),
                tape,
                ptr,
                module.target_config(),
            );
            let id = module
                .declare_function(&format!("tape{k}"), Linkage::Local, &ctx.func.signature)
                .ok()?;
            module.define_function(id, &mut ctx).ok()?;
            module.clear_context(&mut ctx);
            ids.push((tape, id));
        }
        module.finalize_definitions().ok()?;
        let mut fns = FxHashMap::default();
        for (tape, id) in ids {
            let code = module.get_finalized_function(id);
            // SAFETY: compiled with the `TapeFn` signature above.
            let f: TapeFn = unsafe { std::mem::transmute(code) };
            fns.insert((tape.as_ptr() as usize, tape.len()), f);
        }
        Some(Compiled {
            module: Some(module),
            fns,
        })
    }

    fn float_cc(c: Cmp) -> FloatCC {
        match c {
            Cmp::Lt => FloatCC::LessThan,
            Cmp::Le => FloatCC::LessThanOrEqual,
            Cmp::Gt => FloatCC::GreaterThan,
            Cmp::Ge => FloatCC::GreaterThanOrEqual,
            Cmp::Eq => FloatCC::Equal,
            // unordered or not equal: Rust's `!=`
            Cmp::Ne => FloatCC::NotEqual,
        }
    }

    struct Em<'a, 'b> {
        b: &'a mut FunctionBuilder<'b>,
        base: Variable,
        rt: Value,
        ptr: types::Type,
        sig1: Option<SigRef>,
        sig2: Option<SigRef>,
        sig_powi: Option<SigRef>,
        sig_slow: Option<SigRef>,
    }

    impl Em<'_, '_> {
        fn ld(&mut self, s: u32) -> Value {
            let base = self.b.use_var(self.base);
            self.b
                .ins()
                .load(types::F64, MemFlagsData::trusted(), base, (8 * s) as i32)
        }
        fn st(&mut self, s: u32, v: Value) {
            let base = self.b.use_var(self.base);
            self.b
                .ins()
                .store(MemFlagsData::trusted(), v, base, (8 * s) as i32);
        }
        fn bool_f64(&mut self, c: Value) -> Value {
            let one = self.b.ins().f64const(1.0);
            let zero = self.b.ins().f64const(0.0);
            self.b.ins().select(c, one, zero)
        }
        fn sig(&mut self, params: &[types::Type], ret: Option<types::Type>) -> SigRef {
            let mut s = Signature::new(self.b.func.signature.call_conv);
            s.params = params.iter().map(|&t| AbiParam::new(t)).collect();
            s.returns = ret.into_iter().map(AbiParam::new).collect();
            self.b.import_signature(s)
        }
        fn call(&mut self, sig: SigRef, addr: usize, args: &[Value]) -> Value {
            let f = self.b.ins().iconst(self.ptr, addr as i64);
            let c = self.b.ins().call_indirect(sig, f, args);
            self.b.inst_results(c)[0]
        }
        fn call1(&mut self, addr: usize, x: Value) -> Value {
            let sig = match self.sig1 {
                Some(s) => s,
                None => {
                    let s = self.sig(&[types::F64], Some(types::F64));
                    self.sig1 = Some(s);
                    s
                }
            };
            self.call(sig, addr, &[x])
        }
        fn call2(&mut self, addr: usize, x: Value, y: Value) -> Value {
            let sig = match self.sig2 {
                Some(s) => s,
                None => {
                    let s = self.sig(&[types::F64, types::F64], Some(types::F64));
                    self.sig2 = Some(s);
                    s
                }
            };
            self.call(sig, addr, &[x, y])
        }
    }

    fn emit(
        mut fb: FunctionBuilder,
        tape: &[Ins],
        ptr: types::Type,
        config: cranelift_codegen::isa::TargetFrontendConfig,
    ) {
        let b = &mut fb;
        let n = tape.len();
        // Blocks where a jump lands (index n: the end).
        let mut targets: FxHashMap<usize, Block> = FxHashMap::default();
        for (i, ins) in tape.iter().enumerate() {
            if let Ins::Jz(_, k) | Ins::Jmp(k) = *ins {
                let t = i + 1 + k as usize;
                targets.entry(t.min(n)).or_insert_with(|| b.create_block());
            }
        }
        let entry = b.create_block();
        b.append_block_params_for_function_params(entry);
        b.switch_to_block(entry);
        let base = b.declare_var(ptr);
        let rt = b.block_params(entry)[0];
        let r0 = b.block_params(entry)[1];
        b.def_var(base, r0);
        let mut e = Em {
            b,
            base,
            rt,
            ptr,
            sig1: None,
            sig2: None,
            sig_powi: None,
            sig_slow: None,
        };
        // Whether the current block already ended with a jump.
        let mut ended = false;
        for (i, ins) in tape.iter().enumerate() {
            if let Some(&blk) = targets.get(&i) {
                if !ended {
                    e.b.ins().jump(blk, &[]);
                }
                e.b.switch_to_block(blk);
                ended = false;
            } else if ended {
                // Code after an unconditional jump that nothing jumps to.
                let blk = e.b.create_block();
                e.b.switch_to_block(blk);
                ended = false;
            }
            match *ins {
                Ins::Jz(c, k) => {
                    let x = e.ld(c);
                    let zero = e.b.ins().f64const(0.0);
                    let is0 = e.b.ins().fcmp(FloatCC::Equal, x, zero);
                    let next = e.b.create_block();
                    let to = targets[&(i + 1 + k as usize).min(n)];
                    e.b.ins().brif(is0, to, &[], next, &[]);
                    e.b.switch_to_block(next);
                }
                Ins::Jmp(k) => {
                    let to = targets[&(i + 1 + k as usize).min(n)];
                    e.b.ins().jump(to, &[]);
                    ended = true;
                }
                Ins::Copy(d, a) => {
                    let x = e.ld(a);
                    e.st(d, x);
                }
                Ins::Neg(d, a) => {
                    let x = e.ld(a);
                    let v = e.b.ins().fneg(x);
                    e.st(d, v);
                }
                Ins::Add(d, a, c) | Ins::Sub(d, a, c) | Ins::Mul(d, a, c) | Ins::Div(d, a, c) => {
                    let x = e.ld(a);
                    let y = e.ld(c);
                    let v = match *ins {
                        Ins::Add(..) => e.b.ins().fadd(x, y),
                        Ins::Sub(..) => e.b.ins().fsub(x, y),
                        Ins::Mul(..) => e.b.ins().fmul(x, y),
                        _ => e.b.ins().fdiv(x, y),
                    };
                    e.st(d, v);
                }
                Ins::Pow(d, a, c) => {
                    let x = e.ld(a);
                    let y = e.ld(c);
                    let v = e.call2(h_pow as *const () as usize, x, y);
                    e.st(d, v);
                }
                Ins::PowI(d, a, k) => {
                    let x = e.ld(a);
                    let kk = e.b.ins().iconst(types::I32, k as i64);
                    let sig = match e.sig_powi {
                        Some(s) => s,
                        None => {
                            let s = e.sig(&[types::F64, types::I32], Some(types::F64));
                            e.sig_powi = Some(s);
                            s
                        }
                    };
                    let v = e.call(sig, h_powi as *const () as usize, &[x, kk]);
                    e.st(d, v);
                }
                Ins::F1(f, d, a) => {
                    let x = e.ld(a);
                    let v = match f {
                        Fn1::Abs => e.b.ins().fabs(x),
                        Fn1::Sqrt => e.b.ins().sqrt(x),
                        Fn1::Sign => {
                            let zero = e.b.ins().f64const(0.0);
                            let one = e.b.ins().f64const(1.0);
                            let m1 = e.b.ins().f64const(-1.0);
                            let gt = e.b.ins().fcmp(FloatCC::GreaterThan, x, zero);
                            let lt = e.b.ins().fcmp(FloatCC::LessThan, x, zero);
                            let neg = e.b.ins().select(lt, m1, zero);
                            e.b.ins().select(gt, one, neg)
                        }
                        Fn1::Not => {
                            let zero = e.b.ins().f64const(0.0);
                            let c = e.b.ins().fcmp(FloatCC::Equal, x, zero);
                            e.bool_f64(c)
                        }
                        _ => e.call1(f1_helper(f).expect("Fn1 helper"), x),
                    };
                    e.st(d, v);
                }
                Ins::F2(f, d, a, c) => {
                    let x = e.ld(a);
                    let y = e.ld(c);
                    let v = match f {
                        Fn2::And | Fn2::Or => {
                            let zero = e.b.ins().f64const(0.0);
                            let xa = e.b.ins().fcmp(FloatCC::NotEqual, x, zero);
                            let ya = e.b.ins().fcmp(FloatCC::NotEqual, y, zero);
                            let c = if f == Fn2::And {
                                e.b.ins().band(xa, ya)
                            } else {
                                e.b.ins().bor(xa, ya)
                            };
                            e.bool_f64(c)
                        }
                        _ => e.call2(f2_helper(f).expect("Fn2 helper"), x, y),
                    };
                    e.st(d, v);
                }
                Ins::Cmp(op, d, a, c) => {
                    let x = e.ld(a);
                    let y = e.ld(c);
                    let cc = e.b.ins().fcmp(float_cc(op), x, y);
                    let v = e.bool_f64(cc);
                    e.st(d, v);
                }
                Ins::Select(d, c, a, bb) => {
                    let cv = e.ld(c);
                    let x = e.ld(a);
                    let y = e.ld(bb);
                    let zero = e.b.ins().f64const(0.0);
                    let nz = e.b.ins().fcmp(FloatCC::NotEqual, cv, zero);
                    let v = e.b.ins().select(nz, x, y);
                    e.st(d, v);
                }
                _ => {
                    let sig = match e.sig_slow {
                        Some(s) => s,
                        None => {
                            let s = e.sig(&[ptr, ptr], Some(ptr));
                            e.sig_slow = Some(s);
                            s
                        }
                    };
                    let insp = e.b.ins().iconst(ptr, ins as *const Ins as i64);
                    let rt = e.rt;
                    let nb = e.call(sig, h_slow as *const () as usize, &[rt, insp]);
                    e.b.def_var(e.base, nb);
                }
            }
        }
        if let Some(&blk) = targets.get(&n) {
            if !ended {
                e.b.ins().jump(blk, &[]);
            }
            e.b.switch_to_block(blk);
        } else if ended {
            let blk = e.b.create_block();
            e.b.switch_to_block(blk);
        }
        e.b.ins().return_(&[]);
        e.b.seal_all_blocks();
        fb.finalize(config);
    }
}

#[cfg(all(feature = "jit", target_arch = "wasm32"))]
mod wimp {
    use super::Model;
    use super::helpers::*;
    use crate::codegen::Ins;
    use crate::expr::{Cmp, Fn1, Fn2};
    use rustc_hash::FxHashMap;

    /// A compiled tape: `f(runtime, registers)` (a function table index).
    pub type TapeFn = unsafe extern "C" fn(*mut u8, *mut f64);

    pub struct Compiled {
        fns: FxHashMap<(usize, usize), TapeFn>,
        slots: Vec<u32>,
    }

    // SAFETY: WebAssembly here is single-threaded; the table slots are only read.
    unsafe impl Send for Compiled {}
    unsafe impl Sync for Compiled {}

    impl Drop for Compiled {
        fn drop(&mut self) {
            if let Some(&(_, remove)) = super::HOOKS.get() {
                for &s in &self.slots {
                    remove(s);
                }
            }
        }
    }

    impl Compiled {
        pub fn len(&self) -> usize {
            self.fns.len()
        }

        #[inline]
        pub fn get(&self, prog: &[Ins]) -> Option<TapeFn> {
            if prog.is_empty() {
                return None;
            }
            self.fns.get(&(prog.as_ptr() as usize, prog.len())).copied()
        }
    }

    pub fn compile(m: &Model) -> Option<Compiled> {
        let &(install, _) = super::HOOKS.get()?;
        let mut c = Compiled {
            fns: FxHashMap::default(),
            slots: Vec::new(),
        };
        for tape in hot_tapes(m) {
            // A tape too large for one WebAssembly function stays interpreted.
            let Some(bytes) = module(tape) else { continue };
            let Some(slot) = install(&bytes) else {
                continue;
            };
            c.slots.push(slot);
            // SAFETY: in WebAssembly a function pointer is its table index; the slot holds
            // the tape's `run(rt: i32, r: i32)`, the `TapeFn` signature.
            let f: TapeFn = unsafe { std::mem::transmute(slot as usize) };
            c.fns.insert((tape.as_ptr() as usize, tape.len()), f);
        }
        (!c.fns.is_empty()).then_some(c)
    }

    // Types of the generated module.
    const T_RUN: u8 = 0; // (rt, r) -> ()
    const T_F1: u8 = 1; // f64 -> f64
    const T_F2: u8 = 2; // (f64, f64) -> f64
    const T_POWI: u8 = 3; // (f64, i32) -> f64
    const T_SLOW: u8 = 4; // (rt, ins) -> r

    fn uleb(out: &mut Vec<u8>, mut v: u64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if v == 0 {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn sleb(out: &mut Vec<u8>, mut v: i64) {
        loop {
            let b = (v & 0x7f) as u8;
            v >>= 7;
            if (v == 0 && b & 0x40 == 0) || (v == -1 && b & 0x40 != 0) {
                out.push(b);
                return;
            }
            out.push(b | 0x80);
        }
    }

    fn section(out: &mut Vec<u8>, id: u8, body: &[u8]) {
        out.push(id);
        uleb(out, body.len() as u64);
        out.extend_from_slice(body);
    }

    fn name(out: &mut Vec<u8>, s: &str) {
        uleb(out, s.len() as u64);
        out.extend_from_slice(s.as_bytes());
    }

    /// Function body emitter: `local 0` is the runtime, `local 1` the register file.
    struct Body(Vec<u8>);

    impl Body {
        fn op(&mut self, b: u8) -> &mut Self {
            self.0.push(b);
            self
        }
        fn addr(&mut self) -> &mut Self {
            self.0.extend_from_slice(&[0x20, 1]);
            self
        }
        fn ld(&mut self, slot: u32) -> &mut Self {
            self.addr();
            self.0.extend_from_slice(&[0x2b, 3]);
            uleb(&mut self.0, 8 * slot as u64);
            self
        }
        fn st(&mut self, slot: u32) -> &mut Self {
            self.0.extend_from_slice(&[0x39, 3]);
            uleb(&mut self.0, 8 * slot as u64);
            self
        }
        fn f64c(&mut self, v: f64) -> &mut Self {
            self.0.push(0x44);
            self.0.extend_from_slice(&v.to_le_bytes());
            self
        }
        fn i32c(&mut self, v: i32) -> &mut Self {
            self.0.push(0x41);
            sleb(&mut self.0, v as i64);
            self
        }
        fn call(&mut self, ty: u8, f: usize) -> &mut Self {
            self.i32c(f as u32 as i32);
            self.0.extend_from_slice(&[0x11, ty, 0]);
            self
        }
        /// `bool` (i32) to 1.0 / 0.0.
        fn to_f64(&mut self) -> &mut Self {
            self.op(0xb8)
        }
    }

    const F64_EQ: u8 = 0x61;
    const F64_NE: u8 = 0x62;
    const F64_LT: u8 = 0x63;
    const F64_GT: u8 = 0x64;
    const F64_LE: u8 = 0x65;
    const F64_GE: u8 = 0x66;

    fn cmp_op(c: Cmp) -> u8 {
        match c {
            Cmp::Lt => F64_LT,
            Cmp::Le => F64_LE,
            Cmp::Gt => F64_GT,
            Cmp::Ge => F64_GE,
            Cmp::Eq => F64_EQ,
            // true for NaN, like Rust's `!=`
            Cmp::Ne => F64_NE,
        }
    }

    /// The tape as a WebAssembly module importing `env.memory` and `env.table` and
    /// exporting `run`. None when it is too large for one function.
    pub fn module(tape: &[Ins]) -> Option<Vec<u8>> {
        // Split the tape into functions of about CHUNK instructions at points no jump
        // crosses: browsers optimise huge functions very slowly (a 20,000-instruction
        // tape could take minutes to reach optimised code).
        const CHUNK: usize = 1000;
        let n = tape.len();
        let mut cover = vec![0i32; n + 1];
        for (i, ins) in tape.iter().enumerate() {
            if let Ins::Jz(_, k) | Ins::Jmp(k) = *ins {
                let t = (i + 1 + k as usize).min(n);
                // positions i+1 .. t-1 lie inside the jump
                if i + 1 < t {
                    cover[i + 1] += 1;
                    cover[t] -= 1;
                }
            }
        }
        let mut cuts = vec![0];
        let mut depth = 0;
        for (p, c) in cover.iter().enumerate().take(n) {
            depth += c;
            if depth == 0 && p - cuts.last().unwrap() >= CHUNK {
                cuts.push(p);
            }
        }
        cuts.push(n);
        let mut bodies = Vec::new();
        for w in cuts.windows(2) {
            bodies.push(chunk(&tape[w[0]..w[1]])?);
        }
        let mut run = Body(vec![0]);
        for k in 0..bodies.len() {
            run.0.extend_from_slice(&[0x20, 0, 0x20, 1, 0x10]);
            uleb(&mut run.0, 1 + k as u64);
            run.0.extend_from_slice(&[0x21, 1]);
        }
        run.op(0x0b);

        let mut m = vec![0x00, 0x61, 0x73, 0x6d, 1, 0, 0, 0];
        let (i32_, f64_) = (0x7f, 0x7c);
        let types: [(&[u8], &[u8]); 5] = [
            (&[i32_, i32_], &[]),
            (&[f64_], &[f64_]),
            (&[f64_, f64_], &[f64_]),
            (&[f64_, i32_], &[f64_]),
            (&[i32_, i32_], &[i32_]),
        ];
        debug_assert_eq!(T_RUN, 0);
        let mut sec = vec![types.len() as u8];
        for (p, r) in types {
            sec.push(0x60);
            sec.push(p.len() as u8);
            sec.extend_from_slice(p);
            sec.push(r.len() as u8);
            sec.extend_from_slice(r);
        }
        section(&mut m, 1, &sec);
        let mut sec = vec![2];
        name(&mut sec, "env");
        name(&mut sec, "memory");
        sec.extend_from_slice(&[0x02, 0x00, 0x00]); // memory, min 0
        name(&mut sec, "env");
        name(&mut sec, "table");
        sec.extend_from_slice(&[0x01, 0x70, 0x00, 0x00]); // funcref table, min 0
        section(&mut m, 2, &sec);
        // `run`, then the chunks: (rt, r) -> r, the same type as the slow helper
        let mut sec = Vec::new();
        uleb(&mut sec, 1 + bodies.len() as u64);
        sec.push(T_RUN);
        sec.extend(std::iter::repeat_n(T_SLOW, bodies.len()));
        section(&mut m, 3, &sec);
        let mut sec = vec![1];
        name(&mut sec, "run");
        sec.extend_from_slice(&[0x00, 0x00]);
        section(&mut m, 7, &sec);
        let mut sec = Vec::new();
        uleb(&mut sec, 1 + bodies.len() as u64);
        for body in std::iter::once(&run.0).chain(&bodies) {
            uleb(&mut sec, body.len() as u64);
            sec.extend_from_slice(body);
        }
        section(&mut m, 10, &sec);
        Some(m)
    }

    /// One function of a tape: `(rt, r) -> r` (the register file may move in a call).
    fn chunk(tape: &[Ins]) -> Option<Vec<u8>> {
        let n = tape.len();
        // Forward jumps become nested blocks all opened at the start, the one ending at
        // the furthest target outermost; a jump to target t breaks out of the blocks
        // still open below it.
        let mut targets: Vec<usize> = tape
            .iter()
            .enumerate()
            .filter_map(|(i, ins)| match *ins {
                Ins::Jz(_, k) | Ins::Jmp(k) => Some((i + 1 + k as usize).min(n)),
                _ => None,
            })
            .collect();
        targets.sort_unstable();
        targets.dedup();
        let mut b = Body(Vec::with_capacity(16 * n));
        b.0.push(0); // no locals
        for _ in &targets {
            b.0.extend_from_slice(&[0x02, 0x40]);
        }
        let mut lo = 0; // first open target
        let slow = h_slow as *const () as usize;
        for (i, ins) in tape.iter().enumerate() {
            while lo < targets.len() && targets[lo] == i {
                b.op(0x0b);
                lo += 1;
            }
            let depth = |t: usize| -> u64 {
                let t = t.min(n);
                (targets.binary_search(&t).expect("jump target") - lo) as u64
            };
            match *ins {
                Ins::Jz(c, k) => {
                    b.ld(c).f64c(0.0).op(F64_EQ).op(0x0d);
                    uleb(&mut b.0, depth(i + 1 + k as usize));
                }
                Ins::Jmp(k) => {
                    b.op(0x0c);
                    uleb(&mut b.0, depth(i + 1 + k as usize));
                }
                Ins::Copy(d, a) => {
                    b.addr().ld(a).st(d);
                }
                Ins::Neg(d, a) => {
                    b.addr().ld(a).op(0x9a).st(d);
                }
                Ins::Add(d, x, y) => {
                    b.addr().ld(x).ld(y).op(0xa0).st(d);
                }
                Ins::Sub(d, x, y) => {
                    b.addr().ld(x).ld(y).op(0xa1).st(d);
                }
                Ins::Mul(d, x, y) => {
                    b.addr().ld(x).ld(y).op(0xa2).st(d);
                }
                Ins::Div(d, x, y) => {
                    b.addr().ld(x).ld(y).op(0xa3).st(d);
                }
                Ins::Pow(d, x, y) => {
                    b.addr()
                        .ld(x)
                        .ld(y)
                        .call(T_F2, h_pow as *const () as usize)
                        .st(d);
                }
                Ins::PowI(d, x, k) => {
                    b.addr()
                        .ld(x)
                        .i32c(k)
                        .call(T_POWI, h_powi as *const () as usize)
                        .st(d);
                }
                Ins::F1(f, d, x) => {
                    b.addr();
                    match f {
                        Fn1::Abs => {
                            b.ld(x).op(0x99);
                        }
                        Fn1::Sqrt => {
                            b.ld(x).op(0x9f);
                        }
                        Fn1::Sign => {
                            // x > 0 ? 1 : (x < 0 ? -1 : 0)
                            b.f64c(1.0).f64c(-1.0).f64c(0.0);
                            b.ld(x).f64c(0.0).op(F64_LT).op(0x1b);
                            b.ld(x).f64c(0.0).op(F64_GT).op(0x1b);
                        }
                        Fn1::Not => {
                            b.ld(x).f64c(0.0).op(F64_EQ).to_f64();
                        }
                        _ => {
                            b.ld(x).call(T_F1, f1_helper(f)?);
                        }
                    }
                    b.st(d);
                }
                Ins::F2(f, d, x, y) => {
                    b.addr();
                    match f {
                        Fn2::And | Fn2::Or => {
                            b.ld(x).f64c(0.0).op(F64_NE);
                            b.ld(y).f64c(0.0).op(F64_NE);
                            b.op(if f == Fn2::And { 0x71 } else { 0x72 }).to_f64();
                        }
                        _ => {
                            b.ld(x).ld(y).call(T_F2, f2_helper(f)?);
                        }
                    }
                    b.st(d);
                }
                Ins::Cmp(op, d, x, y) => {
                    b.addr().ld(x).ld(y).op(cmp_op(op)).to_f64().st(d);
                }
                Ins::Select(d, c, x, y) => {
                    b.addr()
                        .ld(x)
                        .ld(y)
                        .ld(c)
                        .f64c(0.0)
                        .op(F64_NE)
                        .op(0x1b)
                        .st(d);
                }
                _ => {
                    b.0.extend_from_slice(&[0x20, 0]);
                    b.i32c(ins as *const Ins as usize as u32 as i32);
                    b.call(T_SLOW, slow);
                    b.0.extend_from_slice(&[0x21, 1]);
                }
            }
        }
        while lo < targets.len() {
            b.op(0x0b);
            lo += 1;
        }
        b.0.extend_from_slice(&[0x20, 1, 0x0b]);
        // V8 rejects function bodies over 7,654,321 bytes.
        (b.0.len() <= 7_000_000).then_some(b.0)
    }
}
