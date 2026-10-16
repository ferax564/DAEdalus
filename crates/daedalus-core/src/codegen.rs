//! Code generation: the causalised [`System`] is compiled into a flat register
//! bytecode ("tape") that the solvers execute. Common subexpressions are shared
//! across blocks, algebraic loops become Newton blocks with exact symbolic
//! Jacobians, and everything works on one contiguous `f64` register file, so the
//! same code runs natively and in WebAssembly without allocation in the hot loop.

use crate::backend::{AliasKind, Block, System};
use crate::expr::{Arena, Cmp, ExprId, Fn1, Fn2, Node, Table, Table2, VarId, eval_f1, eval_f2};
use crate::flat::{VarKind, Variable, WhenAction};
use rustc_hash::{FxHashMap, FxHashSet};

pub type Slot = u32;

/// One tape instruction. Operands are register indices.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize)]
pub enum Ins {
    Copy(Slot, Slot),
    Neg(Slot, Slot),
    Add(Slot, Slot, Slot),
    Sub(Slot, Slot, Slot),
    Mul(Slot, Slot, Slot),
    Div(Slot, Slot, Slot),
    Pow(Slot, Slot, Slot),
    PowI(Slot, Slot, i32),
    F1(Fn1, Slot, Slot),
    F2(Fn2, Slot, Slot, Slot),
    Cmp(Cmp, Slot, Slot, Slot),
    /// `d = if c != 0 { a } else { b }`
    Select(Slot, Slot, Slot, Slot),
    /// `d = tables[t].eval(kind, col, x, ne)` with `(t, col) = views[vk >> 2]`, `kind = vk & 3`
    Interp(Slot, u32, Slot, Slot),
    /// `d = tables2[tk >> 5].eval(tk & 31, u1, u2)` (value or partial derivative, see
    /// [`crate::table2::kind`])
    Interp2(Slot, u32, Slot, Slot),
    /// `d = history[k](time - r[delay])`
    Delay(Slot, u32, Slot),
    /// `d = output out of the loop program run at call site s`
    Call(Slot, u32, u32),
    /// `d = d output / d argument` of call site `s`, `(out << 16) | arg`
    CallD(Slot, u32, u32),
    /// Execute the impure external function of call site `s` (an effect of a fired
    /// when-clause); later [`Ins::Call`]s of the site read its results.
    Exec(u32),
    /// Solve Newton block `i` in place.
    Newton(u32),
    /// Skip the next `n` instructions when `c == 0`.
    Jz(Slot, u32),
    /// Skip the next `n` instructions.
    Jmp(u32),
}

/// A compiled run-time `while` loop: registers `0..n` hold the state.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ProgCode {
    pub n: usize,
    pub n_regs: usize,
    pub consts: Vec<(Slot, f64)>,
    pub cond: Vec<Ins>,
    pub cond_slot: Slot,
    pub next: Vec<Ins>,
    pub next_slots: Vec<Slot>,
    /// A native external function instead of a loop.
    pub native: Option<crate::external::NativeCall>,
}

/// A call of a loop program: the registers (of the calling tape) holding its arguments.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SiteCode {
    pub prog: u32,
    pub args: Vec<Slot>,
}

/// An algebraic loop: Newton iteration on the tear variables.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NewtonBlock {
    pub tears: Vec<Slot>,
    /// Computes inner (torn) variables and the residuals.
    pub body: Vec<Ins>,
    pub residuals: Vec<Slot>,
    /// Computes the Jacobian of the residuals w.r.t. the tears (row-major) after `body`.
    pub jac: Vec<Ins>,
    pub jac_slots: Vec<Slot>,
    pub linear: bool,
    /// Linear, and its code has no branches, selects or calls: a Newton step from a
    /// solution reproduces it exactly.
    pub plain_linear: bool,
    pub names: Vec<String>,
    /// Constant `min`/`max` bounds of the tears (Newton steps are clipped to them).
    pub bounds: Vec<(f64, f64)>,
    /// The block's unknowns (tears and inner variables) with a declared start value.
    pub started: Vec<Slot>,
    /// Residual equations as text, `0 = ...` (for the performance analyzer; not saved).
    #[serde(skip)]
    pub eqs: Vec<String>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WhenCode {
    /// Condition slots per branch.
    pub conds: Vec<Slot>,
    /// Actions per branch with the slot holding the new value.
    pub actions: Vec<Vec<Action>>,
    /// Branches with an action that reads a variable another action of the same branch
    /// assigns: their actions run in order, each seeing the values assigned before it.
    pub chained: Vec<bool>,
    /// Branches whose condition contains `initial()`: they fire at initialization even
    /// when another part of the condition (`{time >= t_next, initial()}`) is already true.
    pub at_initial: Vec<bool>,
    /// Per branch: code run once when the branch fires, before its actions are read:
    /// the effects (impure external calls) in order, then the values of actions that
    /// read their results.
    pub effects: Vec<Vec<Ins>>,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub enum Action {
    Assign(Slot, Slot),
    Reinit(Slot, Slot),
    Terminate(String),
    Assert(Slot, String),
}

/// Register file layout.
#[derive(Clone, Copy, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct Layout {
    pub nv: usize,
    pub pre0: usize,
    pub time: usize,
    pub initial: usize,
    pub terminal: usize,
    pub rel0: usize,
    pub n_rel: usize,
    pub n_regs: usize,
}

/// The compiled model.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Model {
    pub name: String,
    pub vars: Vec<Variable>,
    pub layout: Layout,
    pub consts: Vec<(Slot, f64)>,
    pub tables: Vec<Table>,
    /// (table, column) of the [`Ins::Interp`] views.
    pub views: Vec<(u32, u32)>,
    pub tables2: Vec<Table2>,
    pub newton: Vec<NewtonBlock>,
    /// Parameter bindings in dependency order, one segment per parameter so that
    /// individual parameters can be overridden without recompiling.
    pub params: Vec<(VarId, Vec<Ins>)>,
    /// Public parameters with a literal value: overriding them changes the result.
    /// Others are computed from other parameters (or protected) and are shown read-only.
    pub free_params: Vec<VarId>,
    /// Start values of non-parameters (guesses and discrete initial values).
    pub starts: Vec<Ins>,
    pub init: Vec<Ins>,
    pub ode: Vec<Ins>,
    /// Zero-crossing functions `lhs - rhs` and raw relation values.
    pub zc: Vec<Ins>,
    pub zc_g: Vec<Slot>,
    pub zc_raw: Vec<Slot>,
    pub rel_ops: Vec<Cmp>,
    /// Readable text of each relation (for diagnostics).
    pub rel_text: Vec<String>,
    /// Relations generated by `sample()`: start latched false so they fire at the start time.
    pub sample_rels: Vec<usize>,
    /// For each of `sample_rels`, the variable holding its next tick time.
    pub sample_next: Vec<u32>,
    pub when_prog: Vec<Ins>,
    pub whens: Vec<WhenCode>,
    /// When-clauses in data-dependency order: a clause reading (directly or through
    /// algebraic equations) a variable another clause assigns comes after it.
    pub when_order: Vec<usize>,
    /// Per clause, the clauses whose assignments it reads.
    pub when_deps: Vec<Vec<usize>>,
    /// Computes eliminated alias variables.
    pub outputs: Vec<Ins>,
    pub asserts_prog: Vec<Ins>,
    pub asserts: Vec<(Slot, String)>,
    pub states: Vec<Slot>,
    /// Evaluates the `nominal` attributes of the states (from the parameters).
    #[serde(default)]
    pub nominals: Vec<Ins>,
    /// Slot holding the nominal value of each state, if it has one.
    #[serde(default)]
    pub state_nominals: Vec<Option<Slot>>,
    pub derivs: Vec<Slot>,
    pub discretes: Vec<Slot>,
    /// `(v, p)`: after initialization `pre(v)` takes the value of `p`.
    pub pre_init: Vec<(VarId, VarId)>,
    /// Delayed variable of each `delay()` history buffer.
    pub delays: Vec<VarId>,
    pub progs: Vec<ProgCode>,
    pub sites: Vec<SiteCode>,
    pub param_vars: Vec<VarId>,
    pub inputs: Vec<VarId>,
    /// For each state derivative, the states it structurally depends on.
    pub jac_pattern: Vec<Vec<u32>>,
    pub aliases: Vec<(VarId, AliasKind)>,
    pub experiment: crate::flat::Experiment,
    /// FMU components evaluated outside the tape (see [`crate::external_component`]).
    #[serde(default)]
    pub externals: Vec<crate::external_component::ExternalComponent>,
    pub stats: crate::backend::Stats,
    pub warnings: Vec<String>,
    pub tape_len: usize,
}

struct Emitter<'a> {
    arena: &'a mut Arena,
    layout: Layout,
    next: u32,
    consts: FxHashMap<u64, Slot>,
    const_list: Vec<(Slot, f64)>,
    raw_rel: bool,
    sites: Vec<SiteCode>,
    /// Whether an expression contains a function call (worth branching around).
    has_call: FxHashMap<ExprId, bool>,
    has_impure: FxHashMap<ExprId, bool>,
    /// Nodes memoised inside the branches being emitted (forgotten when a branch ends).
    branch_log: Vec<Vec<ExprId>>,
    /// Result slots of branched `if` expressions (written in two places).
    branched: FxHashSet<Slot>,
    site_of: FxHashMap<(u32, Vec<Slot>), u32>,
}

impl Model {
    /// Evaluate a 1-D table lookup ([`Ins::Interp`]).
    #[inline]
    pub fn lookup(&self, vk: u32, x: f64, ne: f64) -> f64 {
        let (t, c) = self.views[(vk >> 2) as usize];
        let tb = &self.tables[t as usize];
        tb.eval(
            vk & 3,
            c as usize,
            x,
            if tb.time { ne } else { f64::INFINITY },
        )
    }
}

impl Emitter<'_> {
    fn contains_call(&mut self, root: ExprId) -> bool {
        if let Some(&b) = self.has_call.get(&root) {
            return b;
        }
        let mut stack = vec![(root, false)];
        let mut ch = Vec::new();
        while let Some((id, expanded)) = stack.pop() {
            if self.has_call.contains_key(&id) {
                continue;
            }
            ch.clear();
            self.arena.children(id, &mut ch);
            if !expanded {
                stack.push((id, true));
                for &c in &ch {
                    if !self.has_call.contains_key(&c) {
                        stack.push((c, false));
                    }
                }
                continue;
            }
            let own = matches!(self.arena.node(id), Node::Call(..) | Node::CallD(..));
            let b = own || ch.iter().any(|c| self.has_call[c]);
            self.has_call.insert(id, b);
        }
        self.has_call[&root]
    }

    /// The call site of call `c` with arguments in `args`: the outputs and partials of
    /// one call share it, so the function runs once for all of them.
    fn site(&mut self, c: u32, args: Vec<Slot>) -> u32 {
        let key = (c, args);
        if let Some(&k) = self.site_of.get(&key) {
            return k;
        }
        let prog = self.arena.calls[c as usize].prog;
        self.sites.push(SiteCode {
            prog,
            args: key.1.clone(),
        });
        let k = (self.sites.len() - 1) as u32;
        self.site_of.insert(key, k);
        k
    }

    fn remember(&mut self, memo: &mut FxHashMap<ExprId, Slot>, id: ExprId, s: Slot) {
        memo.insert(id, s);
        if let Some(log) = self.branch_log.last_mut() {
            log.push(id);
        }
    }

    /// Emit `d := e` as one branch: what it computes is forgotten afterwards.
    fn emit_branch(
        &mut self,
        d: Slot,
        e: ExprId,
        memo: &mut FxHashMap<ExprId, Slot>,
        out: &mut Vec<Ins>,
    ) {
        self.branch_log.push(Vec::new());
        let s = self.emit(e, memo, out);
        out.push(Ins::Copy(d, s));
        for x in self.branch_log.pop().unwrap() {
            memo.remove(&x);
        }
    }

    /// Several `if c then a_i else b_i` with one condition and function calls in their
    /// branches: one pair of branches computes them all, so what they share is computed
    /// once (the outputs of one call taken under an algorithm `if`).
    fn emit_branched_multi(
        &mut self,
        c: ExprId,
        ids: &[ExprId],
        memo: &mut FxHashMap<ExprId, Slot>,
        out: &mut Vec<Ins>,
    ) {
        let sc = self.emit(c, memo, out);
        let ds: Vec<Slot> = ids.iter().map(|_| self.temp()).collect();
        let jz = out.len();
        out.push(Ins::Jz(sc, 0));
        self.emit_sides(ids, &ds, true, memo, out);
        let jmp = out.len();
        out.push(Ins::Jmp(0));
        out[jz] = Ins::Jz(sc, (out.len() - jz - 1) as u32);
        self.emit_sides(ids, &ds, false, memo, out);
        out[jmp] = Ins::Jmp((out.len() - jmp - 1) as u32);
        for (&id, &d) in ids.iter().zip(&ds) {
            self.branched.insert(d);
            self.remember(memo, id, d);
        }
    }

    /// One branch of [`Emitter::emit_branched_multi`]: `d_i := a_i` (or `b_i`).
    fn emit_sides(
        &mut self,
        ids: &[ExprId],
        ds: &[Slot],
        then: bool,
        memo: &mut FxHashMap<ExprId, Slot>,
        out: &mut Vec<Ins>,
    ) {
        self.branch_log.push(Vec::new());
        for (&id, &d) in ids.iter().zip(ds) {
            let Node::If(_, a, b) = self.arena.node(id) else {
                unreachable!("not an if-expression")
            };
            let s = self.emit(if then { a } else { b }, memo, out);
            out.push(Ins::Copy(d, s));
        }
        for x in self.branch_log.pop().unwrap() {
            memo.remove(&x);
        }
    }

    /// `if c then a else b` with a function call in a branch: only the taken branch runs.
    fn emit_branched(
        &mut self,
        id: ExprId,
        c: ExprId,
        a: ExprId,
        b: ExprId,
        memo: &mut FxHashMap<ExprId, Slot>,
        out: &mut Vec<Ins>,
    ) -> Slot {
        let sc = self.emit(c, memo, out);
        let d = self.temp();
        let jz = out.len();
        out.push(Ins::Jz(sc, 0));
        self.emit_branch(d, a, memo, out);
        let jmp = out.len();
        out.push(Ins::Jmp(0));
        out[jz] = Ins::Jz(sc, (out.len() - jz - 1) as u32);
        self.emit_branch(d, b, memo, out);
        out[jmp] = Ins::Jmp((out.len() - jmp - 1) as u32);
        self.branched.insert(d);
        self.remember(memo, id, d);
        d
    }

    fn temp(&mut self) -> Slot {
        let s = self.next;
        self.next += 1;
        s
    }

    fn constant(&mut self, v: f64) -> Slot {
        if let Some(&s) = self.consts.get(&v.to_bits()) {
            return s;
        }
        let s = self.temp();
        self.consts.insert(v.to_bits(), s);
        self.const_list.push((s, v));
        s
    }

    fn contains_impure(&mut self, root: ExprId) -> bool {
        if !self.contains_call(root) {
            return false;
        }
        if let Some(&b) = self.has_impure.get(&root) {
            return b;
        }
        let mut stack = vec![(root, false)];
        let mut ch = Vec::new();
        while let Some((id, expanded)) = stack.pop() {
            if self.has_impure.contains_key(&id) {
                continue;
            }
            ch.clear();
            self.arena.children(id, &mut ch);
            if !expanded {
                stack.push((id, true));
                for &c in &ch {
                    if !self.has_impure.contains_key(&c) {
                        stack.push((c, false));
                    }
                }
                continue;
            }
            let own = match self.arena.node(id) {
                Node::Call(c, _) => {
                    self.arena.progs[self.arena.calls[c as usize].prog as usize].is_impure()
                }
                _ => false,
            };
            let b = own || ch.iter().any(|c| self.has_impure[c]);
            self.has_impure.insert(id, b);
        }
        self.has_impure[&root]
    }

    /// Emit an effect: execute the impure call `call` when `cond` holds. The arguments
    /// are computed unconditionally, so that the actions reading the results find the
    /// same call site.
    fn emit_effect(
        &mut self,
        cond: ExprId,
        call: ExprId,
        memo: &mut FxHashMap<ExprId, Slot>,
        out: &mut Vec<Ins>,
    ) {
        let Node::Call(c, _) = self.arena.node(call) else {
            return;
        };
        let args: Vec<ExprId> = self.arena.calls[c as usize].args.clone();
        let slots: Vec<Slot> = args.iter().map(|&a| self.emit(a, memo, out)).collect();
        let site = self.site(c, slots);
        if self.arena.as_const(cond) == Some(0.0) {
            return;
        }
        if cond != Arena::ONE {
            let sc = self.emit(cond, memo, out);
            out.push(Ins::Jz(sc, 1));
        }
        out.push(Ins::Exec(site));
    }

    /// Emit code computing `root`; returns its slot. `memo` caches already computed nodes.
    fn emit(
        &mut self,
        root: ExprId,
        memo: &mut FxHashMap<ExprId, Slot>,
        out: &mut Vec<Ins>,
    ) -> Slot {
        let mut stack = vec![(root, false)];
        let mut ch = Vec::new();
        while let Some((id, expanded)) = stack.pop() {
            if memo.contains_key(&id) {
                continue;
            }
            let n = self.arena.node(id);
            let leaf = match n {
                Node::Const(b) => Some(self.constant(f64::from_bits(b))),
                Node::Var(v) | Node::Local(v) => Some(v),
                Node::Pre(v) => Some((self.layout.pre0 + v as usize) as Slot),
                Node::Time => Some(self.layout.time as Slot),
                Node::Initial => Some(self.layout.initial as Slot),
                Node::Terminal => Some(self.layout.terminal as Slot),
                Node::Der(v) => panic!("unexpected der({v}) in code generation"),
                Node::Rel(r) if !self.raw_rel => Some((self.layout.rel0 + r as usize) as Slot),
                _ => None,
            };
            if let Some(s) = leaf {
                memo.insert(id, s);
                continue;
            }
            if let Node::If(c, a, b) = n
                && (self.contains_call(a) || self.contains_call(b))
            {
                self.emit_branched(id, c, a, b, memo, out);
                continue;
            }
            ch.clear();
            self.arena.children(id, &mut ch);
            if !expanded {
                stack.push((id, true));
                for &c in ch.iter().rev() {
                    if !memo.contains_key(&c) {
                        stack.push((c, false));
                    }
                }
                continue;
            }
            let s: Vec<Slot> = ch.iter().map(|c| memo[c]).collect();
            let d = self.temp();
            let ins = match n {
                Node::Neg(_) => Ins::Neg(d, s[0]),
                Node::Add(..) => Ins::Add(d, s[0], s[1]),
                Node::Sub(..) => Ins::Sub(d, s[0], s[1]),
                Node::Mul(..) => Ins::Mul(d, s[0], s[1]),
                Node::Div(..) => Ins::Div(d, s[0], s[1]),
                Node::Pow(_, b) => match self.arena.as_const(b) {
                    Some(k) if k == k.round() && k.abs() <= 64.0 => {
                        if k == 2.0 {
                            Ins::Mul(d, s[0], s[0])
                        } else {
                            Ins::PowI(d, s[0], k as i32)
                        }
                    }
                    Some(k) if k == 0.5 => Ins::F1(Fn1::Sqrt, d, s[0]),
                    _ => Ins::Pow(d, s[0], s[1]),
                },
                Node::F1(f, _) => Ins::F1(f, d, s[0]),
                Node::F2(f, ..) => Ins::F2(f, d, s[0], s[1]),
                Node::Cmp(op, ..) => Ins::Cmp(op, d, s[0], s[1]),
                Node::Rel(r) => Ins::Cmp(self.arena.relations[r as usize].op, d, s[0], s[1]),
                Node::If(..) => Ins::Select(d, s[0], s[1], s[2]),
                Node::Interp(vk, ..) => Ins::Interp(d, vk, s[0], s[1]),
                Node::Interp2(tk, ..) => Ins::Interp2(d, tk, s[0], s[1]),
                Node::Delay(k, _) => Ins::Delay(d, k, s[0]),
                Node::Call(c, out) => Ins::Call(d, self.site(c, s), out),
                Node::CallD(c, k) => Ins::CallD(d, self.site(c, s), k),
                other => unreachable!("node {other:?} in code generation"),
            };
            out.push(ins);
            self.remember(memo, id, d);
        }
        memo[&root]
    }

    /// Emit `dst := expr`, writing directly into `dst` when possible.
    fn assign(
        &mut self,
        dst: Slot,
        e: ExprId,
        memo: &mut FxHashMap<ExprId, Slot>,
        out: &mut Vec<Ins>,
    ) {
        let before_len = out.len();
        let had = memo.contains_key(&e);
        let s = self.emit(e, memo, out);
        if s == dst {
            return;
        }
        if !had
            && out.len() > before_len
            && s >= self.first_temp()
            && !self.branched.contains(&s)
            && retarget(out.last_mut().unwrap(), s, dst)
        {
            memo.insert(e, dst);
            return;
        }
        out.push(Ins::Copy(dst, s));
    }

    fn first_temp(&self) -> Slot {
        (self.layout.rel0 + self.layout.n_rel) as Slot
    }
}

fn retarget(ins: &mut Ins, from: Slot, to: Slot) -> bool {
    let d = match ins {
        Ins::Copy(d, _)
        | Ins::Neg(d, _)
        | Ins::Add(d, ..)
        | Ins::Sub(d, ..)
        | Ins::Mul(d, ..)
        | Ins::Div(d, ..)
        | Ins::Pow(d, ..)
        | Ins::PowI(d, ..)
        | Ins::F1(_, d, _)
        | Ins::F2(_, d, ..)
        | Ins::Cmp(_, d, ..)
        | Ins::Select(d, ..)
        | Ins::Interp(d, ..)
        | Ins::Interp2(d, ..)
        | Ins::Delay(d, ..)
        | Ins::Call(d, ..)
        | Ins::CallD(d, ..) => d,
        Ins::Newton(_) | Ins::Exec(_) | Ins::Jz(..) | Ins::Jmp(_) => return false,
    };
    if *d == from {
        *d = to;
        true
    } else {
        false
    }
}

fn compile_blocks(
    em: &mut Emitter,
    blocks: &[Block],
    names: &[Variable],
    newton: &mut Vec<NewtonBlock>,
    out: &mut Vec<Ins>,
) {
    let mut memo: FxHashMap<ExprId, Slot> = FxHashMap::default();
    for b in blocks {
        match b {
            Block::Assign { var, expr } => em.assign(*var, *expr, &mut memo, out),
            Block::Solve {
                tears,
                inner,
                residuals,
                linear,
            } => {
                let mut local = memo.clone();
                // Forget anything that depends on the block's own unknowns (cannot be in memo yet).
                let mut body = Vec::new();
                for (v, e) in inner {
                    em.assign(*v, *e, &mut local, &mut body);
                }
                let res: Vec<Slot> = residuals
                    .iter()
                    .map(|&r| em.emit(r, &mut local, &mut body))
                    .collect();
                // Jacobian by forward differentiation through the inner assignments.
                let mut jac_exprs = vec![Arena::ZERO; residuals.len() * tears.len()];
                for (j, &t) in tears.iter().enumerate() {
                    let mut seeds: FxHashMap<VarId, ExprId> = FxHashMap::default();
                    seeds.insert(t, Arena::ONE);
                    for (v, e) in inner {
                        let mut m = FxHashMap::default();
                        let sd = seeds.clone();
                        let d = em.arena.forward_diff(
                            *e,
                            &mut |_, n| match n {
                                Node::Var(x) => sd.get(&x).copied(),
                                _ => None,
                            },
                            &mut m,
                        );
                        if !em.arena.is_zero(d) {
                            seeds.insert(*v, d);
                        }
                    }
                    for (i, &r) in residuals.iter().enumerate() {
                        let mut m = FxHashMap::default();
                        let d = em.arena.forward_diff(
                            r,
                            &mut |_, n| match n {
                                Node::Var(x) => seeds.get(&x).copied(),
                                _ => None,
                            },
                            &mut m,
                        );
                        jac_exprs[i * tears.len() + j] = d;
                    }
                }
                let mut jac = Vec::new();
                let mut jlocal = local.clone();
                let jac_slots: Vec<Slot> = jac_exprs
                    .iter()
                    .map(|&e| em.emit(e, &mut jlocal, &mut jac))
                    .collect();
                let mut nm: Vec<String> = tears
                    .iter()
                    .map(|&t| names[t as usize].name.clone())
                    .collect();
                nm.extend(inner.iter().map(|(v, _)| names[*v as usize].name.clone()));
                let bounds = tears
                    .iter()
                    .map(|&t| {
                        let v = &names[t as usize];
                        let lo = v
                            .min
                            .and_then(|e| em.arena.as_const(e))
                            .unwrap_or(f64::NEG_INFINITY);
                        let hi = v
                            .max
                            .and_then(|e| em.arena.as_const(e))
                            .unwrap_or(f64::INFINITY);
                        (lo, hi)
                    })
                    .collect();
                let plain_linear = *linear
                    && !body.iter().chain(jac.iter()).any(|i| {
                        matches!(
                            i,
                            Ins::Select(..)
                                | Ins::Jz(..)
                                | Ins::Jmp(_)
                                | Ins::Exec(_)
                                | Ins::Cmp(..)
                                | Ins::Call(..)
                                | Ins::CallD(..)
                        )
                    });
                let started = tears
                    .iter()
                    .copied()
                    .chain(inner.iter().map(|(v, _)| *v))
                    .filter(|&v| names[v as usize].start.is_some())
                    .map(|v| v as Slot)
                    .collect();
                let eqs = residuals
                    .iter()
                    .map(|&r| {
                        let mut t = em.arena.render(r, &|v| names[v as usize].name.clone());
                        if t.len() > 160 {
                            let mut k = 160;
                            while !t.is_char_boundary(k) {
                                k -= 1;
                            }
                            t.truncate(k);
                            t.push('…');
                        }
                        format!("0 = {t}")
                    })
                    .collect();
                newton.push(NewtonBlock {
                    eqs,
                    started,
                    plain_linear,
                    bounds,
                    tears: tears.clone(),
                    body,
                    residuals: res,
                    jac,
                    jac_slots,
                    linear: *linear,
                    names: nm,
                });
                out.push(Ins::Newton((newton.len() - 1) as u32));
            }
        }
    }
}

/// Compile a system into a tape model.
/// Compile loop program `p`: its condition and next-state expressions over the state
/// registers.
fn compile_prog(arena: &mut Arena, p: usize, sites: &mut Vec<SiteCode>) -> ProgCode {
    let prog = arena.progs[p].clone();
    let n = prog.n;
    if let Some(nc) = prog.native {
        return ProgCode {
            n,
            n_regs: 0,
            consts: Vec::new(),
            cond: Vec::new(),
            cond_slot: 0,
            next: Vec::new(),
            next_slots: Vec::new(),
            native: Some(nc),
        };
    }
    let layout = Layout {
        nv: n,
        pre0: n,
        time: n,
        initial: n,
        terminal: n,
        rel0: n,
        n_rel: 0,
        n_regs: 0,
    };
    let mut em = Emitter {
        arena,
        layout,
        next: n as Slot,
        consts: FxHashMap::default(),
        const_list: Vec::new(),
        raw_rel: true,
        sites: std::mem::take(sites),
        has_call: FxHashMap::default(),
        has_impure: FxHashMap::default(),
        branch_log: Vec::new(),
        branched: FxHashSet::default(),
        site_of: FxHashMap::default(),
    };
    let mut memo = FxHashMap::default();
    let mut cond = Vec::new();
    let cond_slot = em.emit(prog.cond, &mut memo, &mut cond);
    let mut next = Vec::new();
    let next_slots = prog
        .next
        .iter()
        .map(|&e| em.emit(e, &mut memo, &mut next))
        .collect();
    *sites = std::mem::take(&mut em.sites);
    ProgCode {
        n,
        n_regs: em.next as usize + n,
        consts: em.const_list,
        cond,
        cond_slot,
        next,
        next_slots,
        native: None,
    }
}

pub fn compile(mut sys: System) -> Model {
    let nv = sys.vars.len();
    let n_rel = sys.arena.relations.len();
    let (when_order, when_deps) = when_dependencies(&sys);
    let layout = Layout {
        nv,
        pre0: nv,
        time: 2 * nv,
        initial: 2 * nv + 1,
        terminal: 2 * nv + 2,
        rel0: 2 * nv + 3,
        n_rel,
        n_regs: 0,
    };
    let tables = sys.arena.tables.clone();
    let views = sys.arena.views.clone();
    let tables2 = sys.arena.tables2.clone();
    let mut sites = Vec::new();
    let progs: Vec<ProgCode> = (0..sys.arena.progs.len())
        .map(|p| compile_prog(&mut sys.arena, p, &mut sites))
        .collect();
    let mut em = Emitter {
        arena: &mut sys.arena,
        layout,
        next: (layout.rel0 + n_rel) as Slot,
        consts: FxHashMap::default(),
        const_list: Vec::new(),
        raw_rel: true,
        sites,
        has_call: FxHashMap::default(),
        has_impure: FxHashMap::default(),
        branch_log: Vec::new(),
        branched: FxHashSet::default(),
        site_of: FxHashMap::default(),
    };
    let mut newton = Vec::new();
    // Parameters.
    let mut params = Vec::new();
    let mut free_params = Vec::new();
    for &p in &sys.params {
        let v = &sys.vars[p as usize];
        let e = if v.kind == VarKind::Input {
            v.start
        } else {
            v.binding.or(v.start)
        };
        if !v.is_protected && e.is_none_or(|e| em.arena.vars_in(&[e]).is_empty()) {
            free_params.push(p);
        }
        let mut seg = Vec::new();
        if let Some(e) = e {
            em.assign(p, e, &mut FxHashMap::default(), &mut seg);
        }
        params.push((p, seg));
    }
    let param_set: FxHashSet<VarId> = sys.params.iter().copied().collect();
    // Start values (guesses), then pre := value for all variables.
    let mut starts = Vec::new();
    {
        let mut memo = FxHashMap::default();
        for (i, v) in sys.vars.iter().enumerate() {
            if param_set.contains(&(i as VarId)) {
                continue;
            }
            if let Some(s) = v.start {
                em.assign(i as Slot, s, &mut memo, &mut starts);
            }
        }
    }
    // Initialization (relations evaluated directly).
    let mut init = Vec::new();
    compile_blocks(&mut em, &sys.init_blocks, &sys.vars, &mut newton, &mut init);
    // Simulation (latched relations).
    em.raw_rel = false;
    let mut ode = Vec::new();
    compile_blocks(&mut em, &sys.blocks, &sys.vars, &mut newton, &mut ode);
    // Zero crossings.
    let mut zc = Vec::new();
    let mut zc_g = Vec::new();
    let mut zc_raw = Vec::new();
    let mut rel_ops = Vec::new();
    let mut rel_text = Vec::new();
    let mut sample_rels = Vec::new();
    let mut sample_next = Vec::new();
    for r in 0..n_rel {
        let rel = em.arena.relations[r];
        if let (Node::Time, Node::Var(v)) = (em.arena.node(rel.lhs), em.arena.node(rel.rhs))
            && sys.vars[v as usize].name.starts_with("$sample")
        {
            sample_rels.push(r);
            sample_next.push(v);
        }
    }
    {
        let mut memo = FxHashMap::default();
        for r in 0..n_rel {
            let rel = em.arena.relations[r];
            let l = em.emit(rel.lhs, &mut memo, &mut zc);
            let rr = em.emit(rel.rhs, &mut memo, &mut zc);
            let g = em.temp();
            zc.push(Ins::Sub(g, l, rr));
            let raw = em.temp();
            zc.push(Ins::Cmp(rel.op, raw, l, rr));
            zc_g.push(g);
            zc_raw.push(raw);
            rel_ops.push(rel.op);
            let name = |v: VarId| sys.vars[v as usize].name.clone();
            let mut t = em.arena.render(rel.lhs, &name);
            t.push_str(&format!(" {:?} ", rel.op));
            t.push_str(&em.arena.render(rel.rhs, &name));
            if t.len() > 160 {
                let mut k = 160;
                while !t.is_char_boundary(k) {
                    k -= 1;
                }
                t.truncate(k);
                t.push('…');
            }
            rel_text.push(t);
        }
    }
    // When clauses.
    let mut when_prog = Vec::new();
    let mut whens = Vec::new();
    {
        let mut memo = FxHashMap::default();
        for wc in &sys.whens {
            let mut conds = Vec::new();
            let mut actions = Vec::new();
            let mut at_initial = Vec::new();
            let mut chained = Vec::new();
            let mut effects = Vec::new();
            for (c, acts) in &wc.branches {
                at_initial.push(em.arena.mentions_initial(*c));
                conds.push(em.emit(*c, &mut memo, &mut when_prog));
                let (order, chain) = order_actions(em.arena, acts);
                chained.push(chain);
                // Impure external calls run once when the branch fires: first the
                // effects in order, then the actions reading their results.
                let mut eprog = Vec::new();
                let mut ememo = None;
                for a in acts {
                    if let WhenAction::Effect(cnd, call) = a {
                        let m = ememo.get_or_insert_with(|| memo.clone());
                        em.emit_effect(*cnd, *call, m, &mut eprog);
                    }
                }
                // Values under one condition with calls in their branches share them.
                let mut groups: Vec<(ExprId, Vec<ExprId>)> = Vec::new();
                for a in acts {
                    let (WhenAction::Assign(_, e) | WhenAction::Reinit(_, e)) = a else {
                        continue;
                    };
                    let e = *e;
                    if let Node::If(c, x, y) = em.arena.node(e)
                        && !memo.contains_key(&e)
                        && (em.contains_call(x) || em.contains_call(y))
                        && !em.contains_impure(e)
                    {
                        match groups.iter_mut().find(|g| g.0 == c) {
                            Some(g) => {
                                if !g.1.contains(&e) {
                                    g.1.push(e)
                                }
                            }
                            None => groups.push((c, vec![e])),
                        }
                    }
                }
                for (c, es) in &groups {
                    if es.len() > 1 {
                        em.emit_branched_multi(*c, es, &mut memo, &mut when_prog);
                    }
                }
                let mut list = Vec::new();
                for a in order.into_iter().map(|k| &acts[k]) {
                    let mut value = |em: &mut Emitter, e: ExprId| {
                        if em.contains_impure(e) {
                            let m = ememo.get_or_insert_with(|| memo.clone());
                            em.emit(e, m, &mut eprog)
                        } else {
                            em.emit(e, &mut memo, &mut when_prog)
                        }
                    };
                    list.push(match a {
                        WhenAction::Assign(v, e) => Action::Assign(*v, value(&mut em, *e)),
                        WhenAction::Reinit(v, e) => Action::Reinit(*v, value(&mut em, *e)),
                        WhenAction::Terminate(m) => Action::Terminate(m.clone()),
                        WhenAction::Assert(e, m) => Action::Assert(value(&mut em, *e), m.clone()),
                        WhenAction::Effect(..) => continue,
                    });
                }
                actions.push(list);
                effects.push(eprog);
            }
            whens.push(WhenCode {
                conds,
                actions,
                chained,
                at_initial,
                effects,
            });
        }
    }
    // Aliases.
    let mut outputs = Vec::new();
    {
        let mut memo = FxHashMap::default();
        for a in &sys.aliases {
            em.assign(a.var, a.expr, &mut memo, &mut outputs);
        }
    }
    let mut asserts_prog = Vec::new();
    let mut asserts = Vec::new();
    {
        let mut memo = FxHashMap::default();
        for (c, m) in &sys.asserts {
            asserts.push((em.emit(*c, &mut memo, &mut asserts_prog), m.clone()));
        }
    }
    // Nominal values of the states (scale the solver's absolute tolerance).
    let mut nominals = Vec::new();
    let state_nominals: Vec<Option<Slot>> = {
        let mut memo = FxHashMap::default();
        sys.states
            .iter()
            .map(|&x| {
                sys.vars[x as usize]
                    .nominal
                    .map(|e| em.emit(e, &mut memo, &mut nominals))
            })
            .collect()
    };
    let mut layout = layout;
    layout.n_regs = em.next as usize;
    let consts = em.const_list.clone();
    let sites = std::mem::take(&mut em.sites);
    let jac_pattern = dependency_pattern(&sys);
    let tape_len = params.iter().map(|p| p.1.len()).sum::<usize>()
        + init.len()
        + ode.len()
        + newton
            .iter()
            .map(|b| b.body.len() + b.jac.len())
            .sum::<usize>();
    Model {
        name: sys.name,
        layout,
        consts,
        tables,
        views,
        tables2,
        newton,
        params,
        free_params,
        starts,
        init,
        ode,
        zc,
        zc_g,
        zc_raw,
        rel_ops,
        rel_text,
        sample_rels,
        sample_next,
        when_prog,
        whens,
        when_order,
        when_deps,
        outputs,
        asserts_prog,
        asserts,
        states: sys.states.clone(),
        nominals,
        state_nominals,
        derivs: sys.state_derivs.clone(),
        discretes: sys.discretes.clone(),
        pre_init: sys.pre_init.clone(),
        delays: sys.arena.delays.clone(),
        progs,
        sites,
        param_vars: sys.params.clone(),
        inputs: sys.inputs.clone(),
        jac_pattern,
        aliases: sys
            .aliases
            .iter()
            .map(|a| (a.var, a.kind.clone()))
            .collect(),
        experiment: sys.experiment,
        externals: sys.externals,
        stats: sys.stats,
        warnings: sys.warnings,
        vars: sys.vars,
        tape_len,
    }
}

/// Structural dependency of each state derivative on the states.
fn dependency_pattern(sys: &System) -> Vec<Vec<u32>> {
    let state_index: FxHashMap<VarId, u32> = sys
        .states
        .iter()
        .enumerate()
        .map(|(i, &s)| (s, i as u32))
        .collect();
    let mut deps: FxHashMap<VarId, Vec<u32>> = FxHashMap::default();
    let collect = |e: ExprId, deps: &FxHashMap<VarId, Vec<u32>>, acc: &mut FxHashSet<u32>| {
        let mut push = |v: VarId| {
            if let Some(&i) = state_index.get(&v) {
                acc.insert(i);
            } else if let Some(d) = deps.get(&v) {
                acc.extend(d.iter().copied());
            }
        };
        sys.arena.visit_leaves(e, &mut |n| {
            if let Node::Var(v) = n {
                push(v);
            }
        });
        // Relations inside the expression: operands too (latched, but they do vary).
    };
    for b in &sys.blocks {
        match b {
            Block::Assign { var, expr } => {
                let mut acc = FxHashSet::default();
                collect(*expr, &deps, &mut acc);
                let mut v: Vec<u32> = acc.into_iter().collect();
                v.sort_unstable();
                deps.insert(*var, v);
            }
            Block::Solve {
                tears,
                inner,
                residuals,
                ..
            } => {
                let mut acc = FxHashSet::default();
                for (_, e) in inner {
                    collect(*e, &deps, &mut acc);
                }
                for &r in residuals {
                    collect(r, &deps, &mut acc);
                }
                let mut v: Vec<u32> = acc.into_iter().collect();
                v.sort_unstable();
                for &t in tears.iter().chain(inner.iter().map(|(v, _)| v)) {
                    deps.insert(t, v.clone());
                }
            }
        }
    }
    sys.state_derivs
        .iter()
        .map(|d| deps.get(d).cloned().unwrap_or_default())
        .collect()
}

// ----------------------------------------------------------------------------
// Interpreter
// ----------------------------------------------------------------------------

/// Statistics of the Newton solver.
#[derive(Clone, Copy, Debug, Default)]
pub struct NewtonStats {
    pub solves: u64,
    pub iterations: u64,
    pub failures: u64,
}

#[derive(Debug, Clone)]
pub struct NewtonError {
    pub block: usize,
    pub residual: f64,
}

/// History of one `delay()` argument: accepted solution points, with both the left and
/// the right value at events.
#[derive(Clone, Debug, Default)]
pub struct DelayBuf {
    pub t: Vec<f64>,
    pub v: Vec<f64>,
    /// Delay time at the last evaluation, and the largest one seen.
    pub d: f64,
    pub d_max: f64,
}

impl DelayBuf {
    /// Value at `tq` (linear between recorded points, constant outside them).
    /// At a recorded jump (both values stored at one instant, give or take rounding of
    /// `time - delayTime`) this is the left value, or the right one when `right`.
    pub fn at(&self, tq: f64, right: bool) -> Option<f64> {
        let n = self.t.len();
        if n == 0 {
            return None;
        }
        let tol = 1e-12 * tq.abs().max(1.0);
        let lo = self.t.partition_point(|&t| t < tq - tol);
        let hi = self.t.partition_point(|&t| t <= tq + tol);
        if hi > lo + 1 {
            return Some(if right { self.v[hi - 1] } else { self.v[lo] });
        }
        let i = self.t.partition_point(|&t| t <= tq);
        if i == 0 {
            return Some(self.v[0]);
        }
        if i == n {
            return Some(self.v[n - 1]);
        }
        let (t0, t1) = (self.t[i - 1], self.t[i]);
        let w = (tq - t0) / (t1 - t0);
        Some(self.v[i - 1] + w * (self.v[i] - self.v[i - 1]))
    }

    /// Append a point; drops points no delay can reach any more.
    pub fn push(&mut self, t: f64, v: f64) {
        if let (Some(&lt), Some(&lv)) = (self.t.last(), self.v.last())
            && lt == t
            && (lv == v || (lv.is_nan() && v.is_nan()))
        {
            return;
        }
        self.t.push(t);
        self.v.push(v);
        if self.t.len() > 8192 {
            let cutoff = t - 2.0 * self.d_max.max(0.0);
            let j = self.t.partition_point(|&x| x < cutoff);
            if j > 4096 {
                self.t.drain(..j - 1);
                self.v.drain(..j - 1);
            }
        }
    }
}

/// Registers of the loop programs and the last result of every call site.
#[derive(Default)]
pub struct LoopState {
    regs: Vec<Vec<f64>>,
    /// Per call site: last arguments, final state, valid.
    cache: Vec<(Vec<f64>, Vec<f64>, bool)>,
    /// Per call site: arguments and, per argument, the tangent of the final state along
    /// that argument (empty until needed).
    dcache: Vec<(Vec<f64>, Vec<Vec<f64>>)>,
    /// Per call site: arguments, argument tangents, final state and its tangent.
    tcache: Vec<TangentEntry>,
    /// Per loop program: value and tangent registers for forward-mode runs.
    tregs: Vec<(Vec<f64>, Vec<f64>)>,
    /// State of the impure external functions (random generators, printed lines).
    pub ext: crate::external::ExtState,
}

#[derive(Clone, Default)]
struct TangentEntry {
    args: Vec<f64>,
    dargs: Vec<f64>,
    st: Vec<f64>,
    dst: Vec<f64>,
    ok: bool,
}

/// Output `out` of call site `site`, whose arguments are in the caller's registers `r`.
fn site_value(m: &Model, ls: &mut LoopState, site: usize, out: usize, r: &[f64]) -> f64 {
    let sc = &m.sites[site];
    if let Some(nc) = &m.progs[sc.prog as usize].native
        && nc.f.impure()
    {
        // The results of the last execution by an effect ([`Ins::Exec`]) of a fired
        // when-clause; a call site no effect executes runs at every evaluation.
        if ls.cache[site].2 {
            return ls.cache[site].1.get(out).copied().unwrap_or(f64::NAN);
        }
        let args: Vec<f64> = sc.args.iter().map(|&s| r[s as usize]).collect();
        let mut o = Vec::new();
        run_native(nc, &args, &mut o, &mut ls.ext);
        return o.get(out).copied().unwrap_or(f64::NAN);
    }
    {
        let (a, st, ok) = &ls.cache[site];
        if *ok
            && a.len() == sc.args.len()
            && sc.args.iter().zip(a).all(|(&s, &x)| r[s as usize] == x)
        {
            return st[out];
        }
    }
    let mut e = std::mem::take(&mut ls.cache[site]);
    e.0.clear();
    e.0.extend(sc.args.iter().map(|&s| r[s as usize]));
    run_loop(m, ls, sc.prog as usize, &e.0, &mut e.1);
    e.2 = true;
    let v = e.1[out];
    ls.cache[site] = e;
    v
}

/// Partial derivative of a call site output, `k = (out << 16) | arg`, by a forward-mode
/// run of the loop program along that argument (exact, unlike differences, so outer
/// difference Jacobians of it stay accurate).
fn site_partial(m: &Model, ls: &mut LoopState, site: usize, k: u32, r: &[f64]) -> f64 {
    let (out, arg) = ((k >> 16) as usize, (k & 0xffff) as usize);
    let sc = &m.sites[site];
    if m.progs[sc.prog as usize]
        .native
        .as_ref()
        .is_some_and(|c| c.f.impure())
    {
        return 0.0;
    }
    let mut d = std::mem::take(&mut ls.dcache[site]);
    if d.0.len() != sc.args.len() || sc.args.iter().zip(&d.0).any(|(&s, &x)| r[s as usize] != x) {
        d.0.clear();
        d.0.extend(sc.args.iter().map(|&s| r[s as usize]));
        d.1.resize(sc.args.len(), Vec::new());
        for st in &mut d.1 {
            st.clear();
        }
    }
    if d.1[arg].is_empty() {
        let mut dargs = vec![0.0; d.0.len()];
        dargs[arg] = 1.0;
        let mut st = Vec::new();
        let mut dst = std::mem::take(&mut d.1[arg]);
        run_loop_tangent(m, ls, sc.prog as usize, &d.0, &dargs, &mut st, &mut dst);
        d.1[arg] = dst;
    }
    let v = d.1[arg][out];
    ls.dcache[site] = d;
    v
}

/// Value and tangent of output `out` of call site `site` in a forward-mode run.
fn site_tangent(
    m: &Model,
    ls: &mut LoopState,
    site: usize,
    out: usize,
    r: &[f64],
    dr: &[f64],
) -> (f64, f64) {
    let sc = &m.sites[site];
    if m.progs[sc.prog as usize]
        .native
        .as_ref()
        .is_some_and(|c| c.f.impure())
    {
        return (site_value(m, ls, site, out, r), 0.0);
    }
    {
        let e = &ls.tcache[site];
        if e.ok
            && e.args.len() == sc.args.len()
            && sc
                .args
                .iter()
                .enumerate()
                .all(|(i, &s)| r[s as usize] == e.args[i] && dr[s as usize] == e.dargs[i])
        {
            return (e.st[out], e.dst[out]);
        }
    }
    let mut e = std::mem::take(&mut ls.tcache[site]);
    e.args.clear();
    e.args.extend(sc.args.iter().map(|&s| r[s as usize]));
    e.dargs.clear();
    e.dargs.extend(sc.args.iter().map(|&s| dr[s as usize]));
    if e.dargs.iter().all(|&x| x == 0.0) {
        let v = site_value(m, ls, site, out, r);
        let n = e.args.len().max(out + 1);
        e.st = ls.cache[site].1.clone();
        e.dst.clear();
        e.dst.resize(n.max(e.st.len()), 0.0);
        e.ok = true;
        ls.tcache[site] = e;
        return (v, 0.0);
    }
    run_loop_tangent(
        m,
        ls,
        sc.prog as usize,
        &e.args,
        &e.dargs,
        &mut e.st,
        &mut e.dst,
    );
    e.ok = true;
    let res = (e.st[out], e.dst[out]);
    ls.tcache[site] = e;
    res
}

/// Forward-mode run of loop program `p`: final state and its tangent.
fn run_loop_tangent(
    m: &Model,
    ls: &mut LoopState,
    p: usize,
    args: &[f64],
    dargs: &[f64],
    out: &mut Vec<f64>,
    dout: &mut Vec<f64>,
) {
    let pc = &m.progs[p];
    if let Some(nc) = &pc.native {
        // Directional derivative by a forward difference (none for impure functions).
        run_native(nc, args, out, &mut ls.ext);
        dout.clear();
        dout.resize(out.len(), 0.0);
        if !nc.f.impure() && dargs.iter().any(|&d| d != 0.0) {
            let norm = args.iter().fold(1.0f64, |a, &x| a.max(x.abs()));
            let dn = dargs.iter().fold(0.0f64, |a, &x| a.max(x.abs()));
            let h = f64::EPSILON.sqrt() * norm / dn;
            let ap: Vec<f64> = args.iter().zip(dargs).map(|(&a, &d)| a + h * d).collect();
            let mut o2 = Vec::new();
            run_native(nc, &ap, &mut o2, &mut ls.ext);
            for (k, d) in dout.iter_mut().enumerate() {
                *d = (o2[k] - out[k]) / h;
            }
        }
        return;
    }
    let (mut r, mut dr) = std::mem::take(&mut ls.tregs[p]);
    if r.len() != pc.n_regs {
        r = vec![0.0; pc.n_regs];
        for &(s, v) in &pc.consts {
            r[s as usize] = v;
        }
        dr = vec![0.0; pc.n_regs];
    }
    let n = pc.n;
    r[..n].copy_from_slice(args);
    dr[..n].copy_from_slice(dargs);
    let base = pc.n_regs - n;
    for _ in 0..crate::backend::eval::MAX_LOOP_ITERATIONS {
        run_sub_tangent(m, ls, &pc.cond, &mut r, &mut dr);
        if r[pc.cond_slot as usize] == 0.0 {
            break;
        }
        run_sub_tangent(m, ls, &pc.next, &mut r, &mut dr);
        for k in 0..n {
            r[base + k] = r[pc.next_slots[k] as usize];
            dr[base + k] = dr[pc.next_slots[k] as usize];
        }
        r.copy_within(base..base + n, 0);
        dr.copy_within(base..base + n, 0);
    }
    out.clear();
    out.extend_from_slice(&r[..n]);
    dout.clear();
    dout.extend_from_slice(&dr[..n]);
    ls.tregs[p] = (r, dr);
}

fn d_f1(f: Fn1, x: f64) -> f64 {
    match f {
        Fn1::Sin => x.cos(),
        Fn1::Cos => -x.sin(),
        Fn1::Tan => 1.0 / (x.cos() * x.cos()),
        Fn1::Asin => 1.0 / (1.0 - x * x).sqrt(),
        Fn1::Acos => -1.0 / (1.0 - x * x).sqrt(),
        Fn1::Atan => 1.0 / (1.0 + x * x),
        Fn1::Sinh => x.cosh(),
        Fn1::Cosh => x.sinh(),
        Fn1::Tanh => 1.0 - x.tanh() * x.tanh(),
        Fn1::Exp => x.exp(),
        Fn1::Log => 1.0 / x,
        Fn1::Log10 => 1.0 / (x * std::f64::consts::LN_10),
        Fn1::Sqrt => 0.5 / x.sqrt(),
        Fn1::Abs => eval_f1(Fn1::Sign, x),
        Fn1::Sign | Fn1::Floor | Fn1::Ceil | Fn1::Not => 0.0,
    }
}

/// Forward-mode tape interpreter for loop programs: `dr` carries the tangents of `r`.
fn run_sub_tangent(m: &Model, ls: &mut LoopState, prog: &[Ins], r: &mut [f64], dr: &mut [f64]) {
    let mut pc = 0;
    while pc < prog.len() {
        let ins = &prog[pc];
        pc += 1;
        match *ins {
            Ins::Jz(c, k) => {
                if r[c as usize] == 0.0 {
                    pc += k as usize;
                }
            }
            Ins::Jmp(k) => pc += k as usize,
            Ins::Copy(d, a) => {
                r[d as usize] = r[a as usize];
                dr[d as usize] = dr[a as usize];
            }
            Ins::Neg(d, a) => {
                r[d as usize] = -r[a as usize];
                dr[d as usize] = -dr[a as usize];
            }
            Ins::Add(d, a, b) => {
                r[d as usize] = r[a as usize] + r[b as usize];
                dr[d as usize] = dr[a as usize] + dr[b as usize];
            }
            Ins::Sub(d, a, b) => {
                r[d as usize] = r[a as usize] - r[b as usize];
                dr[d as usize] = dr[a as usize] - dr[b as usize];
            }
            Ins::Mul(d, a, b) => {
                let (x, y) = (r[a as usize], r[b as usize]);
                dr[d as usize] = dr[a as usize] * y + x * dr[b as usize];
                r[d as usize] = x * y;
            }
            Ins::Div(d, a, b) => {
                let (x, y) = (r[a as usize], r[b as usize]);
                let q = x / y;
                dr[d as usize] = (dr[a as usize] - q * dr[b as usize]) / y;
                r[d as usize] = q;
            }
            Ins::Pow(d, a, b) => {
                let (x, y) = (r[a as usize], r[b as usize]);
                let (dx, dy) = (dr[a as usize], dr[b as usize]);
                let v = x.powf(y);
                let mut t = if dx != 0.0 {
                    y * x.powf(y - 1.0) * dx
                } else {
                    0.0
                };
                if dy != 0.0 {
                    t += v * x.ln() * dy;
                }
                r[d as usize] = v;
                dr[d as usize] = t;
            }
            Ins::PowI(d, a, k) => {
                let x = r[a as usize];
                dr[d as usize] = if k == 0 {
                    0.0
                } else {
                    f64::from(k) * x.powi(k - 1) * dr[a as usize]
                };
                r[d as usize] = x.powi(k);
            }
            Ins::F1(f, d, a) => {
                let x = r[a as usize];
                let dx = dr[a as usize];
                dr[d as usize] = if dx != 0.0 { d_f1(f, x) * dx } else { 0.0 };
                r[d as usize] = eval_f1(f, x);
            }
            Ins::F2(f, d, a, b) => {
                let (x, y) = (r[a as usize], r[b as usize]);
                let (dx, dy) = (dr[a as usize], dr[b as usize]);
                dr[d as usize] = match f {
                    Fn2::Atan2 => (y * dx - x * dy) / (x * x + y * y),
                    Fn2::Min => {
                        if x <= y {
                            dx
                        } else {
                            dy
                        }
                    }
                    Fn2::Max => {
                        if x >= y {
                            dx
                        } else {
                            dy
                        }
                    }
                    Fn2::Mod => dx - (x / y).floor() * dy,
                    Fn2::Rem => dx - (x / y).trunc() * dy,
                    Fn2::Div | Fn2::And | Fn2::Or => 0.0,
                };
                r[d as usize] = eval_f2(f, x, y);
            }
            Ins::Cmp(op, d, a, b) => {
                r[d as usize] = f64::from(op.eval(r[a as usize], r[b as usize]));
                dr[d as usize] = 0.0;
            }
            Ins::Select(d, c, a, b) => {
                let k = if r[c as usize] != 0.0 { a } else { b };
                r[d as usize] = r[k as usize];
                dr[d as usize] = dr[k as usize];
            }
            Ins::Interp(d, vk, x, ne) => {
                let (xv, nv) = (r[x as usize], r[ne as usize]);
                // d(lookup of kind k)/dx is the lookup of kind k + 1 (the third
                // derivative for k = 2, piecewise constant for the cubic splines);
                // the next time event does not depend on x.
                dr[d as usize] = match vk & 3 {
                    3 => 0.0,
                    k => {
                        let tb = &m.tables[m.views[(vk >> 2) as usize].0 as usize];
                        let ne = if tb.time { nv } else { f64::INFINITY };
                        let c = m.views[(vk >> 2) as usize].1 as usize;
                        tb.eval(k + 1, c, xv, ne) * dr[x as usize]
                    }
                };
                r[d as usize] = m.lookup(vk, xv, nv);
            }
            Ins::Interp2(d, tk, a, b) => {
                let tb = &m.tables2[(tk >> 5) as usize];
                let (k, u1, u2) = (tk & 31, r[a as usize], r[b as usize]);
                let (du1, du2) = (dr[a as usize], dr[b as usize]);
                // one-sided on grid lines, as the symbolic derivative (see `Arena::forward_diff`)
                let sides = if k & 7 == 0 {
                    crate::table2::kind::sides(du1, du2)
                } else {
                    0
                };
                let mut dv = 0.0;
                for (dim, du) in [(0, du1), (1, du2)] {
                    if du != 0.0
                        && let Some(pk) = crate::table2::kind::partial(k | sides, dim)
                    {
                        dv += tb.eval(pk, u1, u2) * du;
                    }
                }
                dr[d as usize] = dv;
                r[d as usize] = tb.eval(k, u1, u2);
            }
            Ins::Call(d, site, out) => {
                let (v, dv) = site_tangent(m, ls, site as usize, out as usize, r, dr);
                r[d as usize] = v;
                dr[d as usize] = dv;
            }
            Ins::CallD(d, site, k) => {
                r[d as usize] = site_partial(m, ls, site as usize, k, r);
                dr[d as usize] = 0.0;
            }
            Ins::Delay(d, ..) => {
                r[d as usize] = f64::NAN;
                dr[d as usize] = 0.0;
            }
            Ins::Newton(_) => {}
            Ins::Exec(site) => exec_site(m, ls, site as usize, r),
        }
    }
}

/// Execute the impure external function of call site `site` and keep its results for
/// the [`Ins::Call`]s that read them.
fn exec_site(m: &Model, ls: &mut LoopState, site: usize, r: &[f64]) {
    let sc = &m.sites[site];
    let Some(nc) = &m.progs[sc.prog as usize].native else {
        return;
    };
    let args: Vec<f64> = sc.args.iter().map(|&s| r[s as usize]).collect();
    let mut o = std::mem::take(&mut ls.cache[site].1);
    run_native(nc, &args, &mut o, &mut ls.ext);
    ls.cache[site] = (args, o, true);
}

/// Evaluate a native external function; an error is logged and gives NaN results.
fn run_native(
    nc: &crate::external::NativeCall,
    args: &[f64],
    out: &mut Vec<f64>,
    st: &mut crate::external::ExtState,
) {
    if let Err(e) = crate::external::eval(nc, args, out, st) {
        st.log.push(format!("error in {}: {e}", nc.f.name()));
        out.clear();
        out.resize(nc.n_out, f64::NAN);
    }
}

/// Run loop program `p` from state `args`; the final state goes to `out`.
fn run_loop(m: &Model, ls: &mut LoopState, p: usize, args: &[f64], out: &mut Vec<f64>) {
    let pc = &m.progs[p];
    if let Some(nc) = &pc.native {
        run_native(nc, args, out, &mut ls.ext);
        return;
    }
    let mut r = std::mem::take(&mut ls.regs[p]);
    if r.len() != pc.n_regs {
        r = vec![0.0; pc.n_regs];
        for &(s, v) in &pc.consts {
            r[s as usize] = v;
        }
    }
    let n = pc.n;
    r[..n].copy_from_slice(args);
    let base = pc.n_regs - n;
    for _ in 0..crate::backend::eval::MAX_LOOP_ITERATIONS {
        run_sub(m, ls, &pc.cond, &mut r);
        if r[pc.cond_slot as usize] == 0.0 {
            break;
        }
        run_sub(m, ls, &pc.next, &mut r);
        for k in 0..n {
            r[base + k] = r[pc.next_slots[k] as usize];
        }
        r.copy_within(base..base + n, 0);
    }
    out.clear();
    out.extend_from_slice(&r[..n]);
    ls.regs[p] = r;
}

/// Tape interpreter for loop programs (no Newton blocks, delays or relations there).
fn run_sub(m: &Model, ls: &mut LoopState, prog: &[Ins], r: &mut [f64]) {
    let mut pc = 0;
    while pc < prog.len() {
        let ins = &prog[pc];
        pc += 1;
        match *ins {
            Ins::Jz(c, k) => {
                if r[c as usize] == 0.0 {
                    pc += k as usize;
                }
            }
            Ins::Jmp(k) => pc += k as usize,
            Ins::Copy(d, a) => r[d as usize] = r[a as usize],
            Ins::Neg(d, a) => r[d as usize] = -r[a as usize],
            Ins::Add(d, a, b) => r[d as usize] = r[a as usize] + r[b as usize],
            Ins::Sub(d, a, b) => r[d as usize] = r[a as usize] - r[b as usize],
            Ins::Mul(d, a, b) => r[d as usize] = r[a as usize] * r[b as usize],
            Ins::Div(d, a, b) => r[d as usize] = r[a as usize] / r[b as usize],
            Ins::Pow(d, a, b) => r[d as usize] = r[a as usize].powf(r[b as usize]),
            Ins::PowI(d, a, k) => r[d as usize] = r[a as usize].powi(k),
            Ins::F1(f, d, a) => r[d as usize] = eval_f1(f, r[a as usize]),
            Ins::F2(f, d, a, b) => r[d as usize] = eval_f2(f, r[a as usize], r[b as usize]),
            Ins::Cmp(op, d, a, b) => {
                r[d as usize] = f64::from(op.eval(r[a as usize], r[b as usize]))
            }
            Ins::Select(d, c, a, b) => {
                r[d as usize] = if r[c as usize] != 0.0 {
                    r[a as usize]
                } else {
                    r[b as usize]
                }
            }
            Ins::Interp(d, vk, x, ne) => {
                r[d as usize] = m.lookup(vk, r[x as usize], r[ne as usize])
            }
            Ins::Interp2(d, tk, a, b) => {
                r[d as usize] =
                    m.tables2[(tk >> 5) as usize].eval(tk & 31, r[a as usize], r[b as usize])
            }
            Ins::Call(d, site, out) => {
                r[d as usize] = site_value(m, ls, site as usize, out as usize, r)
            }
            Ins::CallD(d, site, k) => r[d as usize] = site_partial(m, ls, site as usize, k, r),
            Ins::Delay(d, ..) => r[d as usize] = f64::NAN,
            Ins::Newton(_) => {}
            Ins::Exec(site) => exec_site(m, ls, site as usize, r),
        }
    }
}

pub struct Runtime<'m> {
    pub m: &'m Model,
    pub r: Vec<f64>,
    pub loops: LoopState,
    pub delays: Vec<DelayBuf>,
    pub newton_stats: NewtonStats,
    work: Vec<f64>,
    jsave: Vec<f64>,
    weights: Vec<f64>,
    /// Per Newton block: residual row weights from its last Jacobian,
    /// `1 / max_j |J_ij| * nominal_j`, with the tears' largest magnitude seen so far as
    /// their nominal size (a tear passing through zero must not inflate the weight).
    row_w: Vec<Vec<f64>>,
    tear_nom: Vec<Vec<f64>>,
    best_x: Vec<f64>,
    entry: Vec<f64>,
    piv: Vec<usize>,
    pub error: Option<NewtonError>,
    /// Set while the initial equations are solved (see `newton_init`).
    pub init_mode: bool,
    /// Largest Newton step relative to `1 + |tear|` (0: unlimited).
    step_cap: f64,
    /// Newton steps may leave the tears' declared `min`/`max` (a retry after a solve
    /// that stalled on a bound).
    ignore_bounds: bool,
    /// `delay()` reads the right value at a delayed jump (set while events are handled).
    pub delay_right: bool,
    /// External components (imported FMUs): evaluates `m.ode` in [`Runtime::run_ode`].
    pub ext: Option<Box<dyn crate::external_component::Hook>>,
    /// Block timing of the performance analyzer (None: not profiling).
    pub prof: Option<Box<crate::profile::BlockProf>>,
    /// Set during an event iteration: a `sample()` relation that became true stays true
    /// for the rest of the event instant (see `held_samples`).
    pub hold_samples: bool,
    /// The `sample()` relations held true in the current event instant.
    pub held_samples: Vec<usize>,
}

impl<'m> Runtime<'m> {
    pub fn new(m: &'m Model) -> Self {
        let mut r = vec![0.0; m.layout.n_regs];
        for &(s, v) in &m.consts {
            r[s as usize] = v;
        }
        Runtime {
            m,
            r,
            delays: vec![DelayBuf::default(); m.delays.len()],
            loops: LoopState {
                regs: vec![Vec::new(); m.progs.len()],
                cache: vec![Default::default(); m.sites.len()],
                dcache: vec![Default::default(); m.sites.len()],
                tcache: vec![Default::default(); m.sites.len()],
                tregs: vec![Default::default(); m.progs.len()],
                ext: Default::default(),
            },
            newton_stats: NewtonStats::default(),
            work: Vec::new(),
            jsave: Vec::new(),
            weights: Vec::new(),
            row_w: vec![Vec::new(); m.newton.len()],
            tear_nom: vec![Vec::new(); m.newton.len()],
            best_x: Vec::new(),
            entry: Vec::new(),
            piv: Vec::new(),
            error: None,
            init_mode: false,
            step_cap: 0.0,
            ignore_bounds: false,
            delay_right: true,
            ext: None,
            prof: None,
            hold_samples: false,
            held_samples: Vec::new(),
        }
    }

    #[inline]
    pub fn run(&mut self, prog: &[Ins]) {
        let mut pc = 0;
        while pc < prog.len() {
            let ins = &prog[pc];
            pc += 1;
            let r = &mut self.r;
            match *ins {
                Ins::Jz(c, k) => {
                    if r[c as usize] == 0.0 {
                        pc += k as usize;
                    }
                }
                Ins::Jmp(k) => pc += k as usize,
                Ins::Copy(d, a) => r[d as usize] = r[a as usize],
                Ins::Neg(d, a) => r[d as usize] = -r[a as usize],
                Ins::Add(d, a, b) => r[d as usize] = r[a as usize] + r[b as usize],
                Ins::Sub(d, a, b) => r[d as usize] = r[a as usize] - r[b as usize],
                Ins::Mul(d, a, b) => r[d as usize] = r[a as usize] * r[b as usize],
                Ins::Div(d, a, b) => r[d as usize] = r[a as usize] / r[b as usize],
                Ins::Pow(d, a, b) => r[d as usize] = r[a as usize].powf(r[b as usize]),
                Ins::PowI(d, a, k) => r[d as usize] = r[a as usize].powi(k),
                Ins::F1(f, d, a) => r[d as usize] = eval_f1(f, r[a as usize]),
                Ins::F2(f, d, a, b) => r[d as usize] = eval_f2(f, r[a as usize], r[b as usize]),
                Ins::Cmp(op, d, a, b) => {
                    r[d as usize] = f64::from(op.eval(r[a as usize], r[b as usize]))
                }
                Ins::Select(d, c, a, b) => {
                    r[d as usize] = if r[c as usize] != 0.0 {
                        r[a as usize]
                    } else {
                        r[b as usize]
                    }
                }
                Ins::Interp(d, vk, x, ne) => {
                    r[d as usize] = self.m.lookup(vk, r[x as usize], r[ne as usize])
                }
                Ins::Interp2(d, tk, a, b) => {
                    r[d as usize] = self.m.tables2[(tk >> 5) as usize].eval(
                        tk & 31,
                        r[a as usize],
                        r[b as usize],
                    )
                }
                Ins::Delay(d, k, ds) => {
                    let dl = r[ds as usize];
                    let b = &mut self.delays[k as usize];
                    b.d = dl;
                    if dl > b.d_max {
                        b.d_max = dl;
                    }
                    let tq = r[self.m.layout.time] - dl;
                    r[d as usize] = b
                        .at(tq, self.delay_right)
                        .unwrap_or(r[self.m.delays[k as usize] as usize]);
                }
                Ins::Call(d, site, out) => {
                    let t0 = self.prof.as_mut().map(|p| p.enter());
                    let v = site_value(
                        self.m,
                        &mut self.loops,
                        site as usize,
                        out as usize,
                        &self.r,
                    );
                    self.r[d as usize] = v;
                    if let (Some(t0), Some(p)) = (t0, self.prof.as_mut()) {
                        p.leave_call(site as usize, t0);
                    }
                }
                Ins::CallD(d, site, k) => {
                    let v = site_partial(self.m, &mut self.loops, site as usize, k, &self.r);
                    self.r[d as usize] = v;
                }
                Ins::Newton(i) => self.newton(i as usize),
                Ins::Exec(site) => exec_site(self.m, &mut self.loops, site as usize, &self.r),
            }
        }
    }

    /// Record the current values of the delayed variables (after a solution of the model
    /// at the current time). Returns the times at which a recorded jump comes out of a
    /// delay, which must become time events.
    pub fn record_delays(&mut self, jumps: &mut Vec<f64>) {
        if self.m.delays.is_empty() {
            return;
        }
        let m = self.m;
        self.run(&m.outputs);
        let t = self.r[m.layout.time];
        for (k, &x) in m.delays.iter().enumerate() {
            let v = self.r[x as usize];
            let b = &mut self.delays[k];
            let jump =
                matches!((b.t.last(), b.v.last()), (Some(&lt), Some(&lv)) if lt == t && lv != v);
            b.push(t, v);
            if jump && b.d > 0.0 {
                jumps.push(t + b.d);
            }
        }
    }

    fn newton(&mut self, bi: usize) {
        let t0 = self.prof.as_mut().map(|p| p.enter());
        let s0 = self.newton_stats;
        let mut work = std::mem::take(&mut self.work);
        let mut piv = std::mem::take(&mut self.piv);
        self.newton_impl(bi, &mut work, &mut piv);
        self.work = work;
        self.piv = piv;
        if let (Some(t0), Some(p)) = (t0, self.prof.as_mut()) {
            p.leave_newton(bi, t0, s0, self.newton_stats);
        }
    }

    fn newton_impl(&mut self, bi: usize, work: &mut Vec<f64>, piv: &mut Vec<usize>) {
        let m = self.m;
        let b = &m.newton[bi];
        let n = b.tears.len();
        self.newton_stats.solves += 1;
        work.resize(n * n + 3 * n, 0.0);
        // scratch buffers kept in the runtime: no allocation per solve
        let mut weights = std::mem::take(&mut self.weights);
        let mut best_x = std::mem::take(&mut self.best_x);
        let mut entry = std::mem::take(&mut self.entry);
        best_x.clear();
        entry.clear();
        entry.extend(b.tears.iter().map(|&t| self.r[t as usize]));
        if self.init_mode && !b.linear {
            self.newton_init(bi, work, piv, &mut weights, &mut best_x, &entry);
        } else {
            self.newton_iterate(bi, work, piv, &mut weights, &mut best_x, &entry);
        }
        self.weights = weights;
        self.best_x = best_x;
        self.entry = entry;
    }

    /// Initial equations: a nonlinear block whose Newton solution lands far from the
    /// start values of its tears (a suspension arm folded over instead of the design
    /// position: start positions of a mechanism's loop are often all zero, and full
    /// Newton steps from there can jump to another assembly branch) is solved again with
    /// the steps limited to a fraction of each tear's size, which follows the residual
    /// down to a nearer root. The root nearest the start values is kept.
    fn newton_init(
        &mut self,
        bi: usize,
        work: &mut [f64],
        piv: &mut Vec<usize>,
        weights: &mut Vec<f64>,
        best_x: &mut Vec<f64>,
        entry: &[f64],
    ) {
        let m = self.m;
        let b = &m.newton[bi];
        let (err0, fail0) = (self.error.clone(), self.newton_stats.failures);
        // Values on entry: the start values, as nothing has solved the block yet.
        let x0: Vec<f64> = b.started.iter().map(|&s| self.r[s as usize]).collect();
        let dist = |rt: &Runtime| {
            b.started
                .iter()
                .zip(&x0)
                .map(|(&s, &x)| (rt.r[s as usize] - x).abs() / (1.0 + x.abs()))
                .fold(0.0f64, f64::max)
        };
        self.newton_iterate(bi, work, piv, weights, best_x, entry);
        let ok = self.newton_stats.failures == fail0;
        let d = dist(self);
        if ok && d <= 0.5 {
            return;
        }
        let mut best: Option<(f64, Vec<f64>)> =
            ok.then(|| (d, b.tears.iter().map(|&t| self.r[t as usize]).collect()));
        for cap in [0.2, 0.05] {
            for (i, &t) in b.tears.iter().enumerate() {
                self.r[t as usize] = entry[i];
            }
            let fails = self.newton_stats.failures;
            self.step_cap = cap;
            self.newton_iterate(bi, work, piv, weights, best_x, entry);
            self.step_cap = 0.0;
            if self.newton_stats.failures == fails {
                let d = dist(self);
                if best.as_ref().is_none_or(|(bd, _)| d < *bd) {
                    best = Some((d, b.tears.iter().map(|&t| self.r[t as usize]).collect()));
                }
            }
        }
        self.newton_stats.failures = fail0;
        self.error = err0;
        match best {
            Some((_, x)) => {
                for (i, &t) in b.tears.iter().enumerate() {
                    self.r[t as usize] = x[i];
                }
                self.run(&b.body);
            }
            None => {
                for (i, &t) in b.tears.iter().enumerate() {
                    self.r[t as usize] = entry[i];
                }
                self.newton_iterate(bi, work, piv, weights, best_x, entry);
            }
        }
    }

    fn newton_iterate(
        &mut self,
        bi: usize,
        work: &mut [f64],
        piv: &mut Vec<usize>,
        weights: &mut Vec<f64>,
        best_x: &mut Vec<f64>,
        entry: &[f64],
    ) {
        let m = self.m;
        let b = &m.newton[bi];
        let n = b.tears.len();
        let mut best = f64::INFINITY;
        // One tear: points on either side of the root, for a bracketing fallback when
        // Newton cycles on a piecewise-linear residual (a saturating amplifier).
        let (mut neg, mut pos): (Option<(f64, f64)>, Option<(f64, f64)>) = (None, None);
        let mut prev_abs = f64::INFINITY;
        let mut n_forced = 0u32;
        let mut n_stalled = 0u32;
        let mut n_kicks = 0u32;
        let mut hit_bound = false;
        // A linear block normally converges in one step; if it does not (an ill-conditioned
        // or switching system), it continues as a damped Newton iteration.
        let max_iter = if self.step_cap > 0.0 { 400 } else { 50 };
        for it in 0..max_iter {
            self.run(&b.body);
            let norm = b
                .residuals
                .iter()
                .map(|&s| self.r[s as usize].abs())
                .fold(0.0, f64::max);
            let scale = b
                .tears
                .iter()
                .map(|&s| self.r[s as usize].abs())
                .fold(1.0, f64::max);
            let linear = b.linear && it < 3;
            // The starting guess (the last solution) is accepted only when its residuals
            // are also small relative to how strongly the tears move them: an absolute
            // test alone misreads rows with tiny coefficients (`iS + Ccs*der(vcs)`,
            // Ccs ~ 1e-12) as solved and returns stale tears, which blanks those entries
            // of the integrator's finite-difference Jacobian. In a switching block a stale
            // tear also moves relations that hold identically in the current mode: the
            // quenched arc `i = v*Goff` with `i ~ 1e-8` passes the absolute test with `v`
            // off by 1e-4 relative, and `abs(i) <= abs(v)*Goff` then chatters on it.
            let fresh_ok = it > 0 || {
                let rw = &self.row_w[bi];
                rw.len() == n
                    && b.residuals
                        .iter()
                        .zip(rw)
                        .all(|(&s, w)| (self.r[s as usize] * w).abs() <= 1e-10)
            };
            if (norm <= 1e-12 * scale && fresh_ok) || (it > 0 && linear && norm <= 1e-9 * scale) {
                return;
            }
            if !norm.is_finite() && it > 0 {
                break;
            }
            if norm < best {
                best = norm;
                best_x.clear();
                best_x.extend(b.tears.iter().map(|&t| self.r[t as usize]));
            }
            self.run(&b.jac);
            self.newton_stats.iterations += 1;
            // Assemble J and -F.
            let (jm, rest) = work.split_at_mut(n * n);
            let (rhs, old) = rest.split_at_mut(n);
            for (k, &s) in b.jac_slots.iter().enumerate() {
                jm[k] = self.r[s as usize];
            }
            for (i, &s) in b.residuals.iter().enumerate() {
                rhs[i] = -self.r[s as usize];
            }
            let f_entry = -rhs[0];
            // Row weights for the damping test: residuals of very different scale
            // (1/tau next to a fraction) are compared relative to their sensitivity.
            weights.clear();
            for i in 0..n {
                let mut r = 0.0f64;
                for (j, &t) in b.tears.iter().enumerate() {
                    r = r.max((jm[i * n + j] * (self.r[t as usize].abs() + 1e-10)).abs());
                }
                weights.push(if r > 0.0 && r.is_finite() {
                    1.0 / r
                } else {
                    1.0
                });
            }
            {
                let nom = &mut self.tear_nom[bi];
                nom.resize(n, 0.0);
                for (j, &t) in b.tears.iter().enumerate() {
                    nom[j] = nom[j].max(self.r[t as usize].abs());
                }
                let rw = &mut self.row_w[bi];
                rw.clear();
                for i in 0..n {
                    let mut r = 0.0f64;
                    for j in 0..n {
                        r = r.max((jm[i * n + j] * nom[j].max(1e-10)).abs());
                    }
                    rw.push(if r > 0.0 && r.is_finite() {
                        1.0 / r
                    } else {
                        0.0
                    });
                }
            }
            let wnorm0 = b
                .residuals
                .iter()
                .zip(weights.iter())
                .map(|(&s, w)| (self.r[s as usize] * w).abs())
                .fold(0.0, f64::max);
            piv.resize(n, 0);
            self.jsave.clear();
            self.jsave.extend_from_slice(jm);
            if crate::linalg::lu_factor(jm, n, piv) {
                crate::linalg::lu_solve(jm, n, piv, rhs);
            } else {
                // Singular Jacobian (a mode in which some unknowns are not determined, such
                // as the torque split between stuck friction elements in series): take the
                // regularised least-squares step (J'J + mu I) dx = -J'F.
                let j = &self.jsave;
                let mut dmax = 0.0f64;
                for p in 0..n {
                    for q in 0..n {
                        let mut a = 0.0;
                        for k in 0..n {
                            a += j[k * n + p] * j[k * n + q];
                        }
                        jm[p * n + q] = a;
                    }
                    dmax = dmax.max(jm[p * n + p]);
                    let mut g = 0.0;
                    for k in 0..n {
                        g += j[k * n + p] * rhs[k];
                    }
                    old[p] = g;
                }
                let mu = 1e-12 * dmax.max(1e-300);
                for p in 0..n {
                    jm[p * n + p] += mu;
                }
                if !crate::linalg::lu_factor(jm, n, piv) {
                    // No direction at all (the residuals do not depend on the tears here).
                    // When they already vanish, the tears are not determined by them: keep
                    // the values they have (the start values `x(start = 0)` of an
                    // initialization variable that the equations leave free).
                    if norm <= 1e-12 * scale {
                        return;
                    }
                    // perturb the tears slightly and retry
                    for &t in &b.tears {
                        let x = &mut self.r[t as usize];
                        *x += 1e-6 * (1.0 + x.abs());
                    }
                    continue;
                }
                crate::linalg::lu_solve(jm, n, piv, &mut old[..n]);
                rhs.copy_from_slice(&old[..n]);
            }
            for (i, &t) in b.tears.iter().enumerate() {
                old[i] = self.r[t as usize];
            }
            let mut forced = false;
            if n == 1 {
                let f = self.r[b.residuals[0] as usize];
                let x = old[0];
                if f < 0.0 {
                    neg = Some((x, f));
                } else if f > 0.0 {
                    pos = Some((x, f));
                }
                if let (Some((xn, fneg)), Some((xp, fpos))) = (neg, pos) {
                    let cand = x + rhs[0];
                    let (lo, hi) = (xn.min(xp), xn.max(xp));
                    let slow = f.abs() > 0.5 * prev_abs;
                    if !(cand > lo && cand < hi) || slow {
                        let rf = xn - fneg * (xp - xn) / (fpos - fneg);
                        let c = if n_forced % 2 == 0 && rf > lo && rf < hi {
                            rf
                        } else {
                            0.5 * (lo + hi)
                        };
                        rhs[0] = c - x;
                        forced = true;
                        n_forced += 1;
                    }
                }
                prev_abs = f.abs();
            }
            // Damped update: halve the step while the residual grows.
            let mut lambda = 1.0;
            if self.step_cap > 0.0 {
                let r = (0..n)
                    .map(|i| rhs[i].abs() / (self.step_cap * (1.0 + old[i].abs())))
                    .fold(0.0, f64::max);
                if r > 1.0 {
                    lambda = 1.0 / r;
                }
            }
            loop {
                for (i, &t) in b.tears.iter().enumerate() {
                    let (lo, hi) = b.bounds[i];
                    let x = old[i] + lambda * rhs[i];
                    // Stay inside the declared range; a step that leaves it is cut short.
                    self.r[t as usize] = if self.ignore_bounds {
                        x
                    } else if x < lo && old[i] >= lo {
                        hit_bound = true;
                        old[i] + (lo - old[i]) * 0.5_f64.max(0.0)
                    } else if x > hi && old[i] <= hi {
                        hit_bound = true;
                        old[i] + (hi - old[i]) * 0.5
                    } else {
                        x
                    };
                }
                if linear || forced || lambda < 1e-3 {
                    break;
                }
                self.run(&b.body);
                let nn = b
                    .residuals
                    .iter()
                    .zip(weights.iter())
                    .map(|(&s, w)| (self.r[s as usize] * w).abs())
                    .fold(0.0, f64::max);
                let nu = b
                    .residuals
                    .iter()
                    .map(|&s| self.r[s as usize].abs())
                    .fold(0.0, f64::max);
                // either measure decreasing is progress
                if (nn.is_finite() && nn < wnorm0 * (1.0 - 1e-4 * lambda) + 1e-300)
                    || (nu.is_finite() && nu < norm * (1.0 - 1e-4 * lambda) + 1e-300)
                {
                    break;
                }
                lambda *= 0.5;
            }
            // One tear whose damped Newton step stalled (the residual first grows along
            // the step: a fold of a piecewise-linear characteristic, as in a Schmitt
            // trigger whose branch ends) and no sign change known yet: search outwards
            // along the Newton direction, then against it, for a point of opposite sign,
            // so that the bracketing above can take over.
            if lambda < 1e-3 && !forced {
                n_stalled += 1;
            } else {
                n_stalled = 0;
            }
            if n == 1 && n_stalled >= 2 && !self.init_mode && (neg.is_none() || pos.is_none()) {
                let x0 = old[0];
                let f0 = f_entry;
                let (lo, hi) = b.bounds[0];
                let t = b.tears[0] as usize;
                let keep = self.r[t];
                let mut found = None;
                if f0 != 0.0 && f0.is_finite() {
                    'dirs: for dir in [rhs[0].signum(), -rhs[0].signum()] {
                        let mut d = rhs[0].abs().max(1e-6 * (1.0 + x0.abs()));
                        for _ in 0..64 {
                            let x = x0 + dir * d;
                            if !x.is_finite() || x < lo || x > hi {
                                break;
                            }
                            self.r[t] = x;
                            self.run(&b.body);
                            let f = self.r[b.residuals[0] as usize];
                            if f.is_finite() && f != 0.0 && (f < 0.0) != (f0 < 0.0) {
                                found = Some((x, f));
                                break 'dirs;
                            }
                            d *= 2.0;
                        }
                    }
                }
                match found {
                    Some((x, f)) => {
                        if f < 0.0 {
                            neg = Some((x, f));
                        } else {
                            pos = Some((x, f));
                        }
                        self.r[t] = x;
                        continue;
                    }
                    None => self.r[t] = keep,
                }
            }
            let step = (0..n)
                .map(|i| (lambda * rhs[i]).abs() / (1.0 + old[i].abs()))
                .fold(0.0, f64::max);
            if step < 1e-13 {
                self.run(&b.body);
                // Stuck where the residual is flat in some tear (`dp = k*m_flow^2` at
                // `m_flow = 0`) but not solved: move off that point and continue.
                let nn = b
                    .residuals
                    .iter()
                    .map(|&s| self.r[s as usize].abs())
                    .fold(0.0, f64::max);
                let scale = b
                    .tears
                    .iter()
                    .map(|&s| self.r[s as usize].abs())
                    .fold(1.0, f64::max);
                if nn <= 1e-6 * scale || n_kicks >= 2 {
                    return;
                }
                n_kicks += 1;
                for (i, &t) in b.tears.iter().enumerate() {
                    let x = &mut self.r[t as usize];
                    *x = old[i] + 1e-6 * (1.0 + old[i].abs());
                }
            }
        }
        self.run(&b.body);
        let mut norm = b
            .residuals
            .iter()
            .map(|&s| self.r[s as usize].abs())
            .fold(0.0, f64::max);
        if !(norm <= best) && best_x.len() == n {
            // the iteration wandered off: keep the best point it visited
            for (i, &t) in b.tears.iter().enumerate() {
                self.r[t as usize] = best_x[i];
            }
            self.run(&b.body);
            norm = best;
        }
        let scale = b
            .tears
            .iter()
            .map(|&s| self.r[s as usize].abs())
            .fold(1.0, f64::max);
        if !(norm <= 1e-6 * scale) && hit_bound && !self.ignore_bounds {
            // Stalled against a tear's `min` or `max` (the solution lies just outside it,
            // as a small negative flow through a port declared `min = 0`): the attributes
            // are checks, not constraints of the equations, so solve again without them.
            for (i, &t) in b.tears.iter().enumerate() {
                self.r[t as usize] = entry[i];
            }
            self.ignore_bounds = true;
            self.newton_iterate(bi, work, piv, weights, best_x, entry);
            self.ignore_bounds = false;
            return;
        }
        if !(norm <= 1e-6 * scale) {
            // A failed solve leaves the tears where they were, so that the retry (with a
            // smaller step) starts from the last good point and not from a wild iterate.
            for (i, &t) in b.tears.iter().enumerate() {
                self.r[t as usize] = entry[i];
            }
            self.newton_stats.failures += 1;
            if self.error.is_none() {
                self.error = Some(NewtonError {
                    block: bi,
                    residual: norm.min(best),
                });
            }
        }
    }

    #[inline]
    pub fn var(&self, v: VarId) -> f64 {
        self.r[v as usize]
    }

    pub fn set_time(&mut self, t: f64) {
        self.r[self.m.layout.time] = t;
    }

    pub fn time(&self) -> f64 {
        self.r[self.m.layout.time]
    }

    pub fn set_states(&mut self, x: &[f64]) {
        for (i, &s) in self.m.states.iter().enumerate() {
            self.r[s as usize] = x[i];
        }
    }

    pub fn get_states(&self, x: &mut [f64]) {
        for (i, &s) in self.m.states.iter().enumerate() {
            x[i] = self.r[s as usize];
        }
    }

    pub fn get_derivs(&self, dx: &mut [f64]) {
        for (i, &s) in self.m.derivs.iter().enumerate() {
            dx[i] = self.r[s as usize];
        }
    }

    /// Evaluate the right-hand side `dx = f(t, x)`.
    pub fn rhs(&mut self, t: f64, x: &[f64], dx: &mut [f64]) {
        let t0 = self.prof.as_mut().map(|p| p.enter_rhs());
        self.set_time(t);
        self.set_states(x);
        self.run_ode();
        self.get_derivs(dx);
        if let (Some(t0), Some(p)) = (t0, self.prof.as_mut()) {
            p.leave_rhs(t0);
        }
    }

    /// Run `m.ode`, through the external components' hook when there is one.
    pub fn run_ode(&mut self) {
        match self.ext.take() {
            Some(mut h) => {
                h.ode(self);
                self.ext = Some(h);
            }
            None => {
                let m = self.m;
                self.run(&m.ode);
            }
        }
    }

    /// Forward-mode (tangent) run of `prog`: recomputes the values in `r` as [`run`]
    /// does and propagates the directional derivatives `dr` alongside, exactly (no
    /// differences). Algebraic loops are differentiated implicitly at their solution
    /// (`J dtears = -dF/dseed` with the block's symbolic Jacobian `J`), so the tears in
    /// `r` must already hold a converged solution (run `prog` first). Relations, when
    /// conditions, `delay()` outputs and discrete values have zero tangent.
    ///
    /// [`run`]: Runtime::run
    pub fn run_tangent(&mut self, prog: &[Ins], dr: &mut [f64]) {
        let m = self.m;
        let mut pc = 0;
        while pc < prog.len() {
            let ins = &prog[pc];
            pc += 1;
            match *ins {
                Ins::Jz(c, k) => {
                    if self.r[c as usize] == 0.0 {
                        pc += k as usize;
                    }
                }
                Ins::Jmp(k) => pc += k as usize,
                Ins::Delay(d, ..) => {
                    self.run(std::slice::from_ref(ins));
                    dr[d as usize] = 0.0;
                }
                Ins::Newton(bi) => {
                    let b = &m.newton[bi as usize];
                    let n = b.tears.len();
                    for &t in &b.tears {
                        dr[t as usize] = 0.0;
                    }
                    // Tangent of the residuals with the tears held: -J dt = dF.
                    self.run_tangent(&b.body, dr);
                    let mut rhs: Vec<f64> = b.residuals.iter().map(|&s| -dr[s as usize]).collect();
                    self.run(&b.jac);
                    let mut jm: Vec<f64> =
                        b.jac_slots.iter().map(|&s| self.r[s as usize]).collect();
                    let mut piv = vec![0usize; n];
                    if crate::linalg::lu_factor(&mut jm, n, &mut piv) {
                        crate::linalg::lu_solve(&jm, n, &piv, &mut rhs);
                    } else {
                        rhs.iter_mut().for_each(|x| *x = f64::NAN);
                    }
                    for (k, &t) in b.tears.iter().enumerate() {
                        dr[t as usize] = rhs[k];
                    }
                    // Inner variables with the tears' tangents.
                    self.run_tangent(&b.body, dr);
                }
                _ => run_sub_tangent(
                    m,
                    &mut self.loops,
                    std::slice::from_ref(ins),
                    &mut self.r,
                    dr,
                ),
            }
        }
    }

    /// Copy current values into `pre` slots.
    pub fn save_pre(&mut self) {
        let nv = self.m.layout.nv;
        let (a, b) = self.r.split_at_mut(nv);
        b[..nv].copy_from_slice(a);
    }

    pub fn eval_zc(&mut self) {
        let m = self.m;
        self.run(&m.zc);
    }

    /// Latch relations from their raw values; returns whether any changed.
    pub fn latch_relations(&mut self) -> bool {
        let m = self.m;
        self.run(&m.zc);
        let mut changed = false;
        for (i, &raw) in m.zc_raw.iter().enumerate() {
            let s = m.layout.rel0 + i;
            let v = self.r[raw as usize];
            // sample() is true during the whole event instant of a tick, not only until
            // its own when-clause has advanced the tick counter.
            if self.hold_samples && m.sample_rels.contains(&i) {
                if self.held_samples.contains(&i) {
                    continue;
                }
                if v != 0.0 {
                    self.held_samples.push(i);
                }
            }
            if self.r[s] != v {
                self.r[s] = v;
                changed = true;
            }
        }
        changed
    }
}

/// Data dependencies between when-clauses (see `Model::when_order`). Simultaneous
/// when-equations see each other's new values: an AD converter's output read by a DA
/// converter triggered by the same clock edge is the value converted at that edge.
fn when_dependencies(sys: &System) -> (Vec<usize>, Vec<Vec<usize>>) {
    let nw = sys.whens.len();
    let identity = || ((0..nw).collect(), vec![Vec::new(); nw]);
    if nw < 2 {
        return identity();
    }
    // Variables each algebraic block variable reads.
    let mut reads_of: FxHashMap<VarId, Vec<VarId>> = FxHashMap::default();
    for b in &sys.blocks {
        match b {
            Block::Assign { var, expr } => {
                reads_of.insert(*var, sys.arena.vars_in(&[*expr]).into_iter().collect());
            }
            Block::Solve {
                tears,
                inner,
                residuals,
                ..
            } => {
                let mut es: Vec<ExprId> = inner.iter().map(|(_, e)| *e).collect();
                es.extend(residuals.iter().copied());
                let r: Vec<VarId> = sys.arena.vars_in(&es).into_iter().collect();
                for &t in tears.iter().chain(inner.iter().map(|(v, _)| v)) {
                    reads_of.insert(t, r.clone());
                }
            }
        }
    }
    let mut writer: FxHashMap<VarId, Vec<usize>> = FxHashMap::default();
    for (wi, wc) in sys.whens.iter().enumerate() {
        for (_, acts) in &wc.branches {
            for a in acts {
                if let WhenAction::Assign(v, _) | WhenAction::Reinit(v, _) = a {
                    writer.entry(*v).or_default().push(wi);
                }
            }
        }
    }
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); nw];
    for (wj, wc) in sys.whens.iter().enumerate() {
        let mut es = Vec::new();
        for (_, acts) in &wc.branches {
            for a in acts {
                match a {
                    WhenAction::Assign(_, e)
                    | WhenAction::Reinit(_, e)
                    | WhenAction::Assert(e, _) => es.push(*e),
                    WhenAction::Effect(c, e) => {
                        es.push(*c);
                        es.push(*e);
                    }
                    WhenAction::Terminate(_) => {}
                }
            }
        }
        let mut stack: Vec<VarId> = sys.arena.vars_in(&es).into_iter().collect();
        let mut seen: FxHashSet<VarId> = stack.iter().copied().collect();
        while let Some(v) = stack.pop() {
            if let Some(ws) = writer.get(&v) {
                for &wi in ws {
                    if wi != wj && !deps[wj].contains(&wi) {
                        deps[wj].push(wi);
                    }
                }
            }
            if let Some(rs) = reads_of.get(&v) {
                for &r in rs {
                    if seen.insert(r) {
                        stack.push(r);
                    }
                }
            }
        }
    }
    if deps.iter().all(|d| d.is_empty()) {
        return identity();
    }
    // Kahn's algorithm, stable by clause index; clauses on a cycle keep their order.
    let mut indeg: Vec<usize> = deps.iter().map(|d| d.len()).collect();
    let mut users: Vec<Vec<usize>> = vec![Vec::new(); nw];
    for (j, d) in deps.iter().enumerate() {
        for &i in d {
            users[i].push(j);
        }
    }
    let mut done = vec![false; nw];
    let mut order = Vec::with_capacity(nw);
    loop {
        let Some(k) = (0..nw).find(|&k| !done[k] && indeg[k] == 0) else {
            break;
        };
        done[k] = true;
        order.push(k);
        for &u in &users[k] {
            indeg[u] -= 1;
        }
    }
    order.extend((0..nw).filter(|&k| !done[k]));
    (order, deps)
}

/// Order the actions of one when-branch so that an assignment comes before the actions
/// that read its variable (`y = f(x); x = g(..)` runs `x` first). Returns the order and
/// whether any such dependency exists.
fn order_actions(arena: &Arena, acts: &[WhenAction]) -> (Vec<usize>, bool) {
    let target = |a: &WhenAction| match a {
        WhenAction::Assign(v, _) | WhenAction::Reinit(v, _) => Some(*v),
        _ => None,
    };
    let assigned: FxHashMap<VarId, usize> = acts
        .iter()
        .enumerate()
        .filter_map(|(k, a)| target(a).map(|v| (v, k)))
        .collect();
    let mut deps: Vec<Vec<usize>> = vec![Vec::new(); acts.len()];
    let mut chained = false;
    for (k, a) in acts.iter().enumerate() {
        let e = match a {
            WhenAction::Assign(_, e)
            | WhenAction::Reinit(_, e)
            | WhenAction::Assert(e, _)
            | WhenAction::Effect(_, e) => *e,
            WhenAction::Terminate(_) => continue,
        };
        arena.visit_leaves(e, &mut |n| {
            if let Node::Var(v) = n
                && let Some(&j) = assigned.get(&v)
                && j != k
            {
                deps[k].push(j);
                chained = true;
            }
        });
    }
    if !chained {
        return ((0..acts.len()).collect(), false);
    }
    // Depth-first topological order; a cycle keeps the declared order for its members.
    let mut state = vec![0u8; acts.len()];
    let mut order = Vec::with_capacity(acts.len());
    fn visit(k: usize, deps: &[Vec<usize>], state: &mut [u8], order: &mut Vec<usize>) {
        if state[k] != 0 {
            return;
        }
        state[k] = 1;
        for &j in &deps[k] {
            visit(j, deps, state, order);
        }
        state[k] = 2;
        order.push(k);
    }
    for k in 0..acts.len() {
        visit(k, &deps, &mut state, &mut order);
    }
    (order, true)
}
