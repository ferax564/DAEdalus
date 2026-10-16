//! State machines (MLS chapter 17).
//!
//! `transition(..)` and `initialState(..)` are recorded while equations are translated;
//! `activeState(s)` becomes a Boolean variable of state `s`, and `ticksInState()` /
//! `timeInState()` become placeholders that are bound per transition.
//! [`Flattener::lower_state_machines`] then replaces the state machines by the clocked
//! equations of MLS 17.3.4 before the clocked partitions are lowered:
//!
//! * per state machine `selectedState`, `selectedReset`, `activeState`, `activeReset`,
//!   `nextState`, `nextReset`, the pending resets of every state and the counters
//!   behind `ticksInState()` and `timeInState()`;
//! * an inner state machine is active when its enclosing state is, and is reset when
//!   the enclosing state is entered with a reset;
//! * the equations of a state are solved for the variable they define and merged
//!   over all states defining that variable (MLS 17.3.5):
//!   `v = if activeState(s1) then e1 elseif activeState(s2) then e2 else previous(v)`;
//! * `previous(x)` of a variable `x` declared in state `s` is `x.start` when `s` is
//!   entered with a reset.
//!
//! A state machine whose clock cannot be inferred runs on a default clock with a
//! period of 1 s.

use super::*;
use crate::expr::ClkOp;

pub(super) struct SmTransition {
    from: InstId,
    to: InstId,
    cond: ExprId,
    immediate: bool,
    reset: bool,
    synchronize: bool,
    priority: i64,
}

#[derive(Default)]
pub(super) struct SmLog {
    transitions: Vec<SmTransition>,
    /// `initialState(s)`: (instance holding the equation, state).
    initial: Vec<(InstId, InstId)>,
    /// The `activeState(s)` variable of each state.
    active_var: FxHashMap<InstId, VarId>,
    /// Placeholders for `ticksInState()` and `timeInState()`.
    ticks_var: Option<VarId>,
    time_var: Option<VarId>,
    /// Equations `[start, end)` of `model.equations` translated from each instance.
    pub(super) eq_ranges: Vec<(usize, usize, InstId)>,
    /// Variables of the lowered state machines (their partitions get a default clock).
    pub(super) vars: FxHashSet<VarId>,
}

impl SmLog {
    pub(super) fn is_used(&self) -> bool {
        !self.transitions.is_empty()
            || !self.initial.is_empty()
            || !self.active_var.is_empty()
            || self.ticks_var.is_some()
            || self.time_var.is_some()
    }
}

/// One state machine: its states (the initial state first) and transitions.
struct Machine {
    states: Vec<InstId>,
    transitions: Vec<usize>,
    /// The state enclosing this machine (for hierarchical machines).
    parent: Option<InstId>,
}

impl<'a, 't> Flattener<'a, 't> {
    fn sm_instance(
        &mut self,
        arg: &'a E,
        scope: &Scope,
        ctx: &mut Ctx,
        what: &str,
    ) -> Result<InstId> {
        match self.translate(arg, scope, ctx)? {
            Val::Inst(i) => Ok(i),
            _ => bail!(
                "{what}: argument '{}' is not a component instance",
                expr_string(arg)
            ),
        }
    }

    fn sm_new_var(&mut self, name: String, ty: VarType, start: f64, visible: bool) -> VarId {
        let mut v = Variable::new(name, VarKind::Discrete, ty);
        v.generated = true;
        v.is_protected = !visible;
        v.start = Some(self.model.arena.constant(start));
        let id = self.model.add_var(v);
        self.sm.vars.insert(id);
        id
    }

    /// The Boolean `activeState(s)` variable of state instance `s`.
    fn sm_active_var(&mut self, s: InstId) -> VarId {
        if let Some(&v) = self.sm.active_var.get(&s) {
            return v;
        }
        let name = format!("{}.$active", self.insts[s as usize].prefix);
        let v = self.sm_new_var(name, VarType::Boolean, 0.0, false);
        self.sm.active_var.insert(s, v);
        v
    }

    /// Translate `activeState`, `ticksInState` and `timeInState`. `None` if `name` is
    /// none of them.
    pub(super) fn sm_call(
        &mut self,
        name: &str,
        pos: &[&'a E],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Option<Result<Val>> {
        if !matches!(name, "activeState" | "ticksInState" | "timeInState")
            || self.lookup_class(&scope.env, &[name]).is_some()
        {
            return None;
        }
        self.uses_clocks = true;
        Some((|| {
            Ok(match name {
                "activeState" => {
                    let a = pos
                        .first()
                        .ok_or_else(|| anyhow!("activeState: missing argument"))?;
                    let s = self.sm_instance(a, scope, ctx, "activeState")?;
                    let v = self.sm_active_var(s);
                    Val::S(self.model.arena.var(v))
                }
                "ticksInState" => {
                    let v = match self.sm.ticks_var {
                        Some(v) => v,
                        None => {
                            let v = self.sm_new_var(
                                "$sm.ticksInState".into(),
                                VarType::Integer,
                                0.0,
                                false,
                            );
                            self.sm.ticks_var = Some(v);
                            v
                        }
                    };
                    Val::S(self.model.arena.var(v))
                }
                _ => {
                    let v = match self.sm.time_var {
                        Some(v) => v,
                        None => {
                            let v = self.sm_new_var(
                                "$sm.timeInState".into(),
                                VarType::Real,
                                0.0,
                                false,
                            );
                            self.sm.time_var = Some(v);
                            v
                        }
                    };
                    Val::S(self.model.arena.var(v))
                }
            })
        })())
    }

    /// Record `transition(..)` or `initialState(..)`. False if `name` is neither (or a
    /// user function of that name is visible).
    pub(super) fn sm_equation(
        &mut self,
        name: &str,
        args: &'a [E],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<bool> {
        if !matches!(name, "transition" | "initialState")
            || self.lookup_class(&scope.env, &[name]).is_some()
        {
            return Ok(false);
        }
        let Some(here) = scope.inst else {
            return Ok(false);
        };
        self.uses_clocks = true;
        let (pos, named) = split_args(args);
        let arg = |k: usize, n: &str| -> Option<&'a E> {
            pos.get(k)
                .copied()
                .or_else(|| named.iter().find(|(m, _)| *m == n).map(|(_, e)| *e))
        };
        if name == "initialState" {
            let a = arg(0, "state").ok_or_else(|| anyhow!("initialState: missing argument"))?;
            let s = self.sm_instance(a, scope, ctx, "initialState")?;
            self.sm.initial.push((here, s));
            return Ok(true);
        }
        let from = arg(0, "from").ok_or_else(|| anyhow!("transition: missing 'from'"))?;
        let to = arg(1, "to").ok_or_else(|| anyhow!("transition: missing 'to'"))?;
        let c = arg(2, "condition").ok_or_else(|| anyhow!("transition: missing 'condition'"))?;
        let from = self.sm_instance(from, scope, ctx, "transition")?;
        let to = self.sm_instance(to, scope, ctx, "transition")?;
        // Conditions are evaluated at clock ticks: plain comparisons, no crossing functions.
        let cond = self
            .translate(
                c,
                scope,
                &mut Ctx {
                    no_event: true,
                    ..ctx.clone()
                },
            )?
            .scalar()?;
        let mut flag = |k: usize, n: &str, default: f64| -> Result<f64> {
            match arg(k, n) {
                Some(e) => self.eval_structural(e, scope).with_context(|| {
                    format!("transition argument '{n}' must be a parameter expression")
                }),
                None => Ok(default),
            }
        };
        let immediate = flag(3, "immediate", 1.0)? != 0.0;
        let reset = flag(4, "reset", 1.0)? != 0.0;
        let synchronize = flag(5, "synchronize", 0.0)? != 0.0;
        let priority = flag(6, "priority", 1.0)?.round() as i64;
        if priority < 1 {
            bail!("transition priority must be at least 1");
        }
        self.sm.transitions.push(SmTransition {
            from,
            to,
            cond,
            immediate,
            reset,
            synchronize,
            priority,
        });
        Ok(true)
    }

    /// Replace all state machines by clocked equations (MLS 17.3).
    pub(super) fn lower_state_machines(&mut self) -> Result<()> {
        if !self.sm.is_used() {
            return Ok(());
        }
        let transitions = std::mem::take(&mut self.sm.transitions);
        let initial = std::mem::take(&mut self.sm.initial);
        // States and their machines (connected by transitions).
        let mut states: Vec<InstId> = Vec::new();
        let mut index: FxHashMap<InstId, usize> = FxHashMap::default();
        let mut add = |s: InstId, states: &mut Vec<InstId>| {
            *index.entry(s).or_insert_with(|| {
                states.push(s);
                states.len() - 1
            })
        };
        for &(_, s) in &initial {
            add(s, &mut states);
        }
        for t in &transitions {
            add(t.from, &mut states);
            add(t.to, &mut states);
        }
        let mut parent: Vec<usize> = (0..states.len()).collect();
        fn find(p: &mut [usize], mut x: usize) -> usize {
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for t in &transitions {
            let (a, b) = (
                find(&mut parent, index[&t.from]),
                find(&mut parent, index[&t.to]),
            );
            if a != b {
                parent[a] = b;
            }
        }
        let mut machines: Vec<Machine> = Vec::new();
        let mut machine_of_root: FxHashMap<usize, usize> = FxHashMap::default();
        for &(here, s) in &initial {
            let r = find(&mut parent, index[&s]);
            if machine_of_root.contains_key(&r) {
                bail!(
                    "state machine has more than one initial state ('{}')",
                    self.insts[s as usize].prefix
                );
            }
            machine_of_root.insert(r, machines.len());
            machines.push(Machine {
                states: vec![s],
                transitions: Vec::new(),
                parent: self.sm_state_of_inst(here, &states),
            });
        }
        for (k, &s) in states.iter().enumerate() {
            let r = find(&mut parent, k);
            let Some(&m) = machine_of_root.get(&r) else {
                bail!(
                    "state '{}' belongs to a state machine without initialState()",
                    self.insts[s as usize].prefix
                );
            };
            if !machines[m].states.contains(&s) {
                machines[m].states.push(s);
            }
        }
        for (k, t) in transitions.iter().enumerate() {
            let m = machine_of_root[&find(&mut parent, index[&t.from])];
            machines[m].transitions.push(k);
        }
        // Higher priority (lower number) first; declaration order among equals.
        for m in &mut machines {
            m.transitions.sort_by_key(|&k| transitions[k].priority);
            for (i, &a) in m.transitions.iter().enumerate() {
                for &b in &m.transitions[i + 1..] {
                    let (ta, tb) = (&transitions[a], &transitions[b]);
                    if ta.from == tb.from && ta.priority == tb.priority {
                        bail!(
                            "transitions from state '{}' must have different priorities",
                            self.insts[ta.from as usize].prefix
                        );
                    }
                }
            }
        }
        let prefixes: Vec<String> = states
            .iter()
            .map(|&s| self.insts[s as usize].prefix.clone())
            .collect();
        let active: Vec<VarId> = states.iter().map(|&s| self.sm_active_var(s)).collect();
        for (&s, &v) in &self.sm.active_var {
            if !index.contains_key(&s) {
                bail!(
                    "activeState({}): '{}' is not a state of a state machine",
                    self.insts[s as usize].prefix,
                    self.model.var_name(v)
                );
            }
        }
        // Machines are lowered outside-in: an inner machine reads the reset of its
        // enclosing state.
        let depth = |m: &Machine| m.parent.map(|p| self.insts[p as usize].prefix.len());
        let mut order: Vec<usize> = (0..machines.len()).collect();
        order.sort_by_key(|&m| depth(&machines[m]));
        let mut reset_of: FxHashMap<InstId, ExprId> = FxHashMap::default();
        // `activeState` of each machine, for `synchronize`.
        let mut machine_act: Vec<Option<VarId>> = vec![None; machines.len()];
        let mut sm_eqs: Vec<Equation> = Vec::new();
        let o = self.model.origin("state machine");
        let push = |eqs: &mut Vec<Equation>, a: &mut Arena, v: VarId, rhs: ExprId| {
            let lhs = a.var(v);
            eqs.push(Equation {
                lhs,
                rhs,
                origin: o,
            });
        };
        // Inner machines first for synchronize (their final-state test); their own
        // equations are built in the outside-in pass below, so create variables up front.
        let mut mvars: Vec<FxHashMap<&'static str, VarId>> = Vec::new();
        for m in &machines {
            let base = format!("{}.$sm", self.insts[m.states[0] as usize].prefix);
            let mut vs = FxHashMap::default();
            for (f, ty, start, vis) in [
                ("selectedState", VarType::Integer, 1.0, false),
                ("selectedReset", VarType::Boolean, 1.0, false),
                ("activeState", VarType::Integer, 1.0, true),
                ("activeReset", VarType::Boolean, 1.0, false),
                ("nextState", VarType::Integer, 1.0, false),
                ("nextReset", VarType::Boolean, 1.0, false),
                ("active", VarType::Boolean, 0.0, false),
                ("delayedFired", VarType::Boolean, 0.0, false),
                ("ticks", VarType::Integer, 0.0, false),
                ("entryTime", VarType::Real, 0.0, false),
            ] {
                vs.insert(f, self.sm_new_var(format!("{base}.{f}"), ty, start, vis));
            }
            mvars.push(vs);
        }
        for (k, vs) in mvars.iter().enumerate() {
            machine_act[k] = Some(vs["activeState"]);
        }
        let ticks_ph = self.sm.ticks_var;
        let time_ph = self.sm.time_var;
        for &mi in &order {
            let m = &machines[mi];
            let vs = &mvars[mi];
            let a = &mut self.model.arena;
            let prev = |a: &mut Arena, v: VarId| {
                let x = a.var(v);
                a.clk(ClkOp::Previous, x, Arena::ZERO)
            };
            let (act_in, reset_in) = match m.parent {
                Some(p) => (
                    a.var(active[index[&p]]),
                    *reset_of.get(&p).unwrap_or(&Arena::ZERO),
                ),
                None => (Arena::ONE, Arena::ZERO),
            };
            let sidx = |s: InstId| m.states.iter().position(|&x| x == s).unwrap() + 1;
            let c = |a: &mut Arena, x: usize| a.constant(x as f64);
            let (sel, sel_r, act, act_r, next, next_r, mact, dfired, ticks, tent) = (
                vs["selectedState"],
                vs["selectedReset"],
                vs["activeState"],
                vs["activeReset"],
                vs["nextState"],
                vs["nextReset"],
                vs["active"],
                vs["delayedFired"],
                vs["ticks"],
                vs["entryTime"],
            );
            // selectedState = if reset then 1 else previous(nextState)
            let p = prev(a, next);
            let rhs = a.if_(reset_in, Arena::ONE, p);
            push(&mut sm_eqs, a, sel, rhs);
            let p = prev(a, next_r);
            let rhs = a.if_(reset_in, Arena::ONE, p);
            push(&mut sm_eqs, a, sel_r, rhs);
            // The machine's own activity, so that the first tick after an activation
            // restarts the counters.
            push(&mut sm_eqs, a, mact, act_in);
            // Uninterrupted continuation of the selected state from the previous tick.
            let pa = prev(a, mact);
            let pd = prev(a, dfired);
            let npd = a.not(pd);
            let nr = a.not(reset_in);
            let cont = a.f2(Fn2::And, pa, npd);
            let cont = a.f2(Fn2::And, cont, nr);
            let pt = prev(a, ticks);
            let pt1 = a.add(pt, Arena::ONE);
            let ticks_sel = a.if_(cont, pt1, Arena::ONE);
            let now = a.time();
            let pe = prev(a, tent);
            let tent_sel = a.if_(cont, pe, now);
            let sel_v = a.var(sel);
            let act_v = a.var(act);
            let ticks_v = a.var(ticks);
            let tent_v = a.var(tent);
            let time_sel = a.sub(now, tent_sel);
            let time_act = a.sub(now, tent_v);
            // Transition conditions with ticksInState()/timeInState() bound.
            let mut imm: Vec<(ExprId, usize, bool)> = Vec::new();
            let mut del: Vec<(ExprId, usize, bool)> = Vec::new();
            for &ti in &m.transitions {
                let t = &transitions[ti];
                let (tk, tm, cur) = if t.immediate {
                    (ticks_sel, time_sel, sel_v)
                } else {
                    (ticks_v, time_act, act_v)
                };
                let mut cond = self.model.arena.substitute(t.cond, &|nd| match nd {
                    Node::Var(v) if Some(v) == ticks_ph => Some(tk),
                    Node::Var(v) if Some(v) == time_ph => Some(tm),
                    _ => None,
                });
                if t.synchronize {
                    // Enabled only when every machine of the from-state is in a final state.
                    for (k2, m2) in machines.iter().enumerate() {
                        if m2.parent != Some(t.from) {
                            continue;
                        }
                        let a2 = machine_act[k2].unwrap();
                        let mut fin = Arena::ZERO;
                        for (si, &s2) in m2.states.iter().enumerate() {
                            if m2.transitions.iter().any(|&x| transitions[x].from == s2) {
                                continue;
                            }
                            let a = &mut self.model.arena;
                            let av = a.var(a2);
                            let k = a.constant((si + 1) as f64);
                            let is = a.cmp(Cmp::Eq, av, k);
                            fin = a.f2(Fn2::Or, fin, is);
                        }
                        cond = self.model.arena.f2(Fn2::And, cond, fin);
                    }
                }
                let a = &mut self.model.arena;
                let fk = c(a, sidx(t.from));
                let here = a.cmp(Cmp::Eq, cur, fk);
                let fire = a.f2(Fn2::And, here, cond);
                let entry = (fire, sidx(t.to), t.reset);
                if t.immediate {
                    imm.push(entry);
                } else {
                    del.push(entry);
                }
            }
            let a = &mut self.model.arena;
            // activeState / activeReset: the first enabled immediate transition.
            let mut st = sel_v;
            let sel_rv = a.var(sel_r);
            let mut rs = sel_rv;
            let mut fired = Arena::ZERO;
            for &(f, to, r) in imm.iter().rev() {
                let tk = c(a, to);
                st = a.if_(f, tk, st);
                let rk = if r { Arena::ONE } else { Arena::ZERO };
                rs = a.if_(f, rk, rs);
            }
            for &(f, ..) in &imm {
                fired = a.f2(Fn2::Or, fired, f);
            }
            push(&mut sm_eqs, a, act, st);
            push(&mut sm_eqs, a, act_r, rs);
            // nextState / nextReset: the first enabled delayed transition.
            let mut st = act_v;
            let mut rs = Arena::ZERO;
            let mut dfire = Arena::ZERO;
            for &(f, to, r) in del.iter().rev() {
                let tk = c(a, to);
                st = a.if_(f, tk, st);
                let rk = if r { Arena::ONE } else { Arena::ZERO };
                rs = a.if_(f, rk, rs);
            }
            for &(f, ..) in &del {
                dfire = a.f2(Fn2::Or, dfire, f);
            }
            let p = prev(a, next);
            let rhs = a.if_(act_in, st, p);
            push(&mut sm_eqs, a, next, rhs);
            let p = prev(a, next_r);
            let rhs = a.if_(act_in, rs, p);
            push(&mut sm_eqs, a, next_r, rhs);
            let rhs = a.f2(Fn2::And, act_in, dfire);
            push(&mut sm_eqs, a, dfired, rhs);
            // Ticks and entry time of the active state.
            let t_in = a.if_(fired, Arena::ONE, ticks_sel);
            let rhs = a.if_(act_in, t_in, pt);
            push(&mut sm_eqs, a, ticks, rhs);
            let e_in = a.if_(fired, now, tent_sel);
            let rhs = a.if_(act_in, e_in, pe);
            push(&mut sm_eqs, a, tent, rhs);
            // Per state: activity, pending resets and the reset on entry.
            let act_rv = a.var(act_r);
            let base = format!("{}.$sm", self.insts[m.states[0] as usize].prefix);
            for (si, &s) in m.states.iter().enumerate() {
                let pend = {
                    let name = format!("{base}.resetPending[{}]", si + 1);
                    self.sm_new_var(name, VarType::Boolean, 1.0, false)
                };
                let npend = {
                    let name = format!("{base}.nextResetPending[{}]", si + 1);
                    self.sm_new_var(name, VarType::Boolean, 1.0, false)
                };
                let a = &mut self.model.arena;
                let k = c(a, si + 1);
                let is = a.cmp(Cmp::Eq, act_v, k);
                let on = a.f2(Fn2::And, act_in, is);
                push(&mut sm_eqs, a, active[index[&s]], on);
                let p = prev(a, npend);
                let rhs = a.if_(reset_in, Arena::ONE, p);
                push(&mut sm_eqs, a, pend, rhs);
                let pv = a.var(pend);
                let cleared = a.if_(is, Arena::ZERO, pv);
                let p = prev(a, npend);
                let rhs = a.if_(act_in, cleared, p);
                push(&mut sm_eqs, a, npend, rhs);
                let av = a.var(active[index[&s]]);
                let r = a.f2(Fn2::Or, act_rv, pv);
                let r = a.f2(Fn2::And, av, r);
                reset_of.insert(s, r);
            }
        }
        // Equations of the states.
        let ranges = std::mem::take(&mut self.sm.eq_ranges);
        let mut eq_inst: Vec<Option<InstId>> = vec![None; self.model.equations.len()];
        for (s0, s1, i) in ranges {
            for k in s0..s1.min(eq_inst.len()) {
                eq_inst[k] = Some(i);
            }
        }
        let eqs = std::mem::take(&mut self.model.equations);
        let mut keep = Vec::with_capacity(eqs.len());
        // Definitions of each variable: (state, value, origin), in equation order.
        let mut defs: IndexMap<VarId, Vec<(usize, ExprId, u32)>> = IndexMap::new();
        for (k, e) in eqs.into_iter().enumerate() {
            let owner = match eq_inst[k] {
                Some(i) => self.sm_state_of_inst(i, &states).map(|s| index[&s]),
                None => match self.model.arena.node(e.lhs) {
                    Node::Var(v) => self.sm_state_of_var(v, &prefixes),
                    _ => None,
                },
            };
            let Some(si) = owner else {
                keep.push(e);
                continue;
            };
            let lhs = self.sm_state_expr(e.lhs, &prefixes, &states, &reset_of);
            let rhs = self.sm_state_expr(e.rhs, &prefixes, &states, &reset_of);
            let (v, val) = self.sm_explicit(lhs, rhs, si, &prefixes).ok_or_else(|| {
                anyhow!(
                    "equation '{} = {}' in state '{}' cannot be solved for the variable it defines",
                    self.model.show(e.lhs),
                    self.model.show(e.rhs),
                    prefixes[si]
                )
            })?;
            defs.entry(v).or_default().push((si, val, e.origin));
        }
        // `inner outer output v` in a state: the merged inner value defines the outer one.
        for (si, &s) in states.iter().enumerate() {
            let pairs: Vec<(String, Comp)> = self.insts[s as usize]
                .inner_of
                .iter()
                .map(|(k, c)| (k.clone(), c.clone()))
                .collect();
            for (name, inner) in pairs {
                if let (Comp::Var(x), Some(Comp::Var(y))) =
                    (inner, self.insts[s as usize].comps.get(&name).cloned())
                    && defs.contains_key(&x)
                {
                    let xv = self.model.arena.var(x);
                    defs.entry(y).or_default().push((si, xv, o));
                }
            }
        }
        for (v, ds) in defs {
            // Held value; a reset of the state declaring `v` restores its start value
            // even while the states defining it are inactive.
            let reset = self
                .sm_state_of_var(v, &prefixes)
                .and_then(|si| reset_of.get(&states[si]).copied());
            let start = self.model.vars[v as usize].start.unwrap_or(Arena::ZERO);
            let a = &mut self.model.arena;
            let mut rhs = a.pre(v);
            if let Some(r) = reset {
                rhs = a.if_(r, start, rhs);
            }
            for &(si, val, _) in ds.iter().rev() {
                let on = a.var(active[si]);
                rhs = a.if_(on, val, rhs);
            }
            let lhs = a.var(v);
            keep.push(Equation {
                lhs,
                rhs,
                origin: ds[0].2,
            });
        }
        keep.extend(sm_eqs);
        self.model.equations = keep;
        // ticksInState()/timeInState() outside transition conditions.
        for ph in [ticks_ph, time_ph].into_iter().flatten() {
            let used = self.model.equations.iter().any(|e| {
                self.model.arena.contains(e.lhs, Node::Var(ph))
                    || self.model.arena.contains(e.rhs, Node::Var(ph))
            });
            if used {
                bail!(
                    "ticksInState() and timeInState() are only supported in transition conditions"
                );
            }
            let var = &mut self.model.vars[ph as usize];
            var.kind = VarKind::Constant;
            var.binding = Some(Arena::ZERO);
            self.sm.vars.remove(&ph);
        }
        self.uses_clocks = true;
        Ok(())
    }

    /// The innermost state containing instance `i` (or `i` itself).
    fn sm_state_of_inst(&self, i: InstId, states: &[InstId]) -> Option<InstId> {
        let p = &self.insts[i as usize].prefix;
        states
            .iter()
            .copied()
            .filter(|&s| within(p, &self.insts[s as usize].prefix))
            .max_by_key(|&s| self.insts[s as usize].prefix.len())
    }

    /// Index of the innermost state declaring variable `v`.
    fn sm_state_of_var(&self, v: VarId, prefixes: &[String]) -> Option<usize> {
        let n = &self.model.vars[v as usize].name;
        (0..prefixes.len())
            .filter(|&k| n.len() > prefixes[k].len() && within(n, &prefixes[k]))
            .max_by_key(|&k| prefixes[k].len())
    }

    /// A side of a state's equation with `previous(x)` as `pre(x)`, or as `x.start` when
    /// the state declaring `x` is entered with a reset.
    fn sm_state_expr(
        &mut self,
        e: ExprId,
        prefixes: &[String],
        states: &[InstId],
        reset_of: &FxHashMap<InstId, ExprId>,
    ) -> ExprId {
        let mut owner: FxHashMap<VarId, Option<usize>> = FxHashMap::default();
        let mut vars = Vec::new();
        self.model.arena.visit_leaves(e, &mut |n| {
            if let Node::Var(v) = n {
                vars.push(v);
            }
        });
        for v in vars {
            let s = self.sm_state_of_var(v, prefixes);
            owner.insert(v, s);
        }
        let starts: FxHashMap<VarId, ExprId> = owner
            .keys()
            .map(|&v| (v, self.model.vars[v as usize].start.unwrap_or(Arena::ZERO)))
            .collect();
        let mut memo = FxHashMap::default();
        self.model.arena.rewrite(e, &mut memo, &mut |a, n| match n {
            Node::Clk(ClkOp::Previous, x, _) => {
                Some(
                    a.rewrite(x, &mut FxHashMap::default(), &mut |a2, n| match n {
                        Node::Var(v) => {
                            let p = a2.pre(v);
                            Some(match owner.get(&v).copied().flatten() {
                                Some(si) => match reset_of.get(&states[si]) {
                                    Some(&r) => a2.if_(r, starts[&v], p),
                                    None => p,
                                },
                                None => p,
                            })
                        }
                        _ => None,
                    }),
                )
            }
            _ => None,
        })
    }

    /// `lhs = rhs` of state `si` as `v = value`.
    fn sm_explicit(
        &mut self,
        lhs: ExprId,
        rhs: ExprId,
        si: usize,
        prefixes: &[String],
    ) -> Option<(VarId, ExprId)> {
        let a = &self.model.arena;
        if let Node::Var(v) = a.node(lhs)
            && !a.contains(rhs, Node::Var(v))
        {
            return Some((v, rhs));
        }
        if let Node::Var(v) = a.node(rhs)
            && !a.contains(lhs, Node::Var(v))
        {
            return Some((v, lhs));
        }
        let r = self.model.arena.sub(lhs, rhs);
        let mut cands: Vec<VarId> = Vec::new();
        self.model.arena.visit_leaves(r, &mut |n| {
            if let Node::Var(v) = n
                && !cands.contains(&v)
            {
                cands.push(v);
            }
        });
        cands.retain(|&v| {
            matches!(
                self.model.vars[v as usize].kind,
                VarKind::Discrete | VarKind::Continuous
            )
        });
        let local: Vec<VarId> = cands
            .iter()
            .copied()
            .filter(|&v| self.sm_state_of_var(v, prefixes) == Some(si))
            .collect();
        let pick = if local.len() == 1 {
            local[0]
        } else if cands.len() == 1 {
            cands[0]
        } else {
            return None;
        };
        self.solve_for(r, pick).map(|x| (pick, x))
    }
}

/// `name` is `prefix` or lies below it.
fn within(name: &str, prefix: &str) -> bool {
    if prefix.is_empty() {
        return false;
    }
    name == prefix
        || (name.starts_with(prefix) && matches!(name.as_bytes().get(prefix.len()), Some(b'.')))
}
