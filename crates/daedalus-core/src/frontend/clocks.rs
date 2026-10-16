//! Synchronous language elements (MLS chapter 16).
//!
//! Translation turns `Clock(..)`, `sample`, `hold`, `previous`, `subSample`,
//! `superSample`, `shiftSample`, `backSample`, `noClock`, `interval` and `firstTick`
//! into [`Node::Clk`] nodes. After flattening, [`Flattener::lower_clocks`]:
//!
//! 1. solves the clock equations (`c = Clock(0.1)`, `c2 = subSample(c1, 2)`,
//!    connections between clock connectors) into base clocks with exact rational
//!    periods and shifts;
//! 2. splits the variables into partitions: equations link the variables they
//!    mention, except across `sample`, `hold`, the sub/super/shift/back operators
//!    and `noClock`, which are the partition boundaries;
//! 3. infers the clock of every clocked partition from `sample(u, c)`, clocked
//!    `when` clauses and the rate relations between partitions;
//! 4. lowers each base clock to one `when` clause driven by a tick counter on the
//!    finest common rate. Each partition's equations are matched, solved for their
//!    unknowns and run, in data-dependency order, on the ticks where that partition
//!    is active. Held values are the discrete variables themselves.
//!
//! Partitions with `der()` on a solver clock are discretised with explicit or implicit Euler.

use super::*;
use crate::expr::ClkOp;

/// Exact fraction `n / d` with `d > 0`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Rat(i64, i64);

fn gcd(a: i64, b: i64) -> i64 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

impl Rat {
    fn new(n: i64, d: i64) -> Rat {
        let (n, d) = if d < 0 { (-n, -d) } else { (n, d) };
        let g = gcd(n, d);
        Rat(n / g, d / g)
    }
    fn mul(self, o: Rat) -> Rat {
        Rat::new(self.0 * o.0, self.1 * o.1)
    }
    fn div(self, o: Rat) -> Rat {
        Rat::new(self.0 * o.1, self.1 * o.0)
    }
    fn add(self, o: Rat) -> Rat {
        Rat::new(self.0 * o.1 + o.0 * self.1, self.1 * o.1)
    }
    fn neg(self) -> Rat {
        Rat(-self.0, self.1)
    }
}

#[derive(Debug, Clone, Copy)]
enum BaseKind {
    /// Periodic with this interval (seconds).
    Periodic(ExprId),
    /// Ticks on the rising edge of this condition.
    Event(ExprId),
}

#[derive(Debug, Clone)]
struct Base {
    kind: BaseKind,
    /// `Clock(c, solverMethod = ..)`: continuous equations are discretised.
    solver: bool,
    /// The solver method is implicit (`ImplicitEuler`, `ImplicitTrapezoid`).
    implicit: bool,
}

/// A clock relative to its base: ticks every `period` base intervals, offset by `shift`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ClockVal {
    base: usize,
    period: Rat,
    shift: Rat,
}

/// Rate relation `clock(to) = op(clock(from), factor)` between two partitions.
struct RateRel {
    to: usize,
    from: usize,
    op: ClkOp,
    /// `None` when the factor is inferred (`superSample(u)`).
    factor: Option<Rat>,
}

#[derive(Default)]
struct ClockSolve {
    bases: Vec<Base>,
    base_of_expr: FxHashMap<ExprId, usize>,
    /// Clock variables: union-find parent and the defining expression of each class.
    parent: FxHashMap<VarId, VarId>,
    def: FxHashMap<VarId, ExprId>,
    memo: FxHashMap<VarId, Option<ClockVal>>,
    busy: FxHashSet<VarId>,
    rational_base: Option<usize>,
}

fn uf_find(p: &mut FxHashMap<VarId, VarId>, x: VarId) -> VarId {
    let mut r = x;
    while let Some(&q) = p.get(&r) {
        if q == r {
            break;
        }
        r = q;
    }
    let mut c = x;
    while let Some(&q) = p.get(&c) {
        if q == r {
            break;
        }
        p.insert(c, r);
        c = q;
    }
    r
}

fn ix_find(p: &mut [usize], mut x: usize) -> usize {
    while p[x] != x {
        p[x] = p[p[x]];
        x = p[x];
    }
    x
}

impl<'a, 't> Flattener<'a, 't> {
    /// True if `e` denotes a clock (rather than a clocked value).
    pub(super) fn is_clock_expr(&self, e: ExprId) -> bool {
        match self.model.arena.node(e) {
            Node::Var(v) => self.clock_vars.contains(&v),
            Node::Clk(op, a, _) => match op {
                ClkOp::Inferred | ClkOp::Periodic | ClkOp::Event | ClkOp::Solver => true,
                ClkOp::Sub | ClkOp::Super | ClkOp::Shift | ClkOp::Back => self.is_clock_expr(a),
                _ => false,
            },
            _ => false,
        }
    }

    fn is_boolean_expr(&self, e: ExprId) -> bool {
        match self.model.arena.node(e) {
            Node::Rel(_) | Node::Cmp(..) | Node::F1(Fn1::Not, _) => true,
            Node::F2(Fn2::And | Fn2::Or, ..) => true,
            Node::Var(v) | Node::Pre(v) => self.model.vars[v as usize].ty == VarType::Boolean,
            Node::If(_, a, b) => self.is_boolean_expr(a) && self.is_boolean_expr(b),
            _ => false,
        }
    }

    /// Translate a call to a synchronous builtin. `None` if `name` is not one.
    pub(super) fn clock_call(
        &mut self,
        name: &str,
        pos: &[&'a E],
        named: &[(&str, &'a E)],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Option<Result<Val>> {
        let ops = [
            "Clock",
            "sample",
            "hold",
            "previous",
            "subSample",
            "superSample",
            "shiftSample",
            "backSample",
            "noClock",
            "interval",
            "firstTick",
        ];
        if !ops.contains(&name) {
            return None;
        }
        Some(self.clock_call_inner(name, pos, named, scope, ctx))
    }

    fn clock_call_inner(
        &mut self,
        name: &str,
        pos: &[&'a E],
        named: &[(&str, &'a E)],
        scope: &Scope,
        ctx: &mut Ctx,
    ) -> Result<Val> {
        let mut args = Vec::new();
        for p in pos {
            args.push(self.translate(p, scope, ctx)?);
        }
        let mut nargs = Vec::new();
        for (n, e) in named {
            nargs.push((n.to_string(), self.translate(e, scope, ctx)?));
        }
        let named_arg = |k: &str| nargs.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let z = Arena::ZERO;
        let sc = |v: &Val| v.clone().scalar();
        if name != "sample" || args.len() != 2 {
            self.uses_clocks = true;
        }
        // Second operand of a solver clock: 1 for an implicit method.
        let implicit = match named_arg("solverMethod") {
            Some(Val::Str(m)) if m.starts_with("Implicit") => Arena::ONE,
            _ => z,
        };
        match name {
            "Clock" => {
                let a = &mut self.model.arena;
                let e = match args.len() {
                    0 => a.clk(ClkOp::Inferred, z, z),
                    _ => {
                        let x = sc(&args[0])?;
                        if self.is_clock_expr(x) {
                            // Clock(c, solverMethod = ..)
                            self.model.arena.clk(ClkOp::Solver, x, implicit)
                        } else if self.is_boolean_expr(x) {
                            self.model.arena.clk(ClkOp::Event, x, z)
                        } else if args.len() >= 2 || named_arg("resolution").is_some() {
                            let r = match args.get(1) {
                                Some(v) => sc(v)?,
                                None => sc(&named_arg("resolution").unwrap())?,
                            };
                            let q = self.model.arena.clk(ClkOp::Rational, x, r);
                            self.model.arena.clk(ClkOp::Periodic, q, z)
                        } else {
                            self.model.arena.clk(ClkOp::Periodic, x, z)
                        }
                    }
                };
                let e = if named_arg("solverMethod").is_some()
                    && !matches!(self.model.arena.node(e), Node::Clk(ClkOp::Solver, ..))
                {
                    self.model.arena.clk(ClkOp::Solver, e, implicit)
                } else {
                    e
                };
                Ok(Val::S(e))
            }
            "sample" => {
                if args.len() == 2 {
                    let c = sc(&args[1])?;
                    if !self.is_clock_expr(c) {
                        let start = sc(&args[0])?;
                        return Ok(Val::S(self.sample(start, c)));
                    }
                    self.uses_clocks = true;
                    let u = args[0].clone();
                    return self.map1(u, &mut |a, x| a.clk(ClkOp::Sample, x, c));
                }
                let u = args
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow!("sample: missing argument"))?;
                self.map1(u, &mut |a, x| {
                    let inf = a.clk(ClkOp::Inferred, z, z);
                    a.clk(ClkOp::Sample, x, inf)
                })
            }
            "hold" | "previous" | "noClock" => {
                let op = match name {
                    "hold" => ClkOp::Hold,
                    "previous" => ClkOp::Previous,
                    _ => ClkOp::NoClock,
                };
                let u = args
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow!("{name}: missing argument"))?;
                self.map1(u, &mut |a, x| a.clk(op, x, z))
            }
            "interval" | "firstTick" => {
                let op = if name == "interval" {
                    ClkOp::Interval
                } else {
                    ClkOp::FirstTick
                };
                let u = match args.first() {
                    Some(v) => v.clone().elems()?.1.first().copied().unwrap_or(z),
                    None => z,
                };
                Ok(Val::S(self.model.arena.clk(op, u, z)))
            }
            "subSample" | "superSample" => {
                let op = if name == "subSample" {
                    ClkOp::Sub
                } else {
                    ClkOp::Super
                };
                let f = match args.get(1).cloned().or_else(|| named_arg("factor")) {
                    Some(v) => sc(&v)?,
                    None => z,
                };
                let u = args
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow!("{name}: missing argument"))?;
                self.map1(u, &mut |a, x| a.clk(op, x, f))
            }
            "shiftSample" | "backSample" => {
                let op = if name == "shiftSample" {
                    ClkOp::Shift
                } else {
                    ClkOp::Back
                };
                let k = match args
                    .get(1)
                    .cloned()
                    .or_else(|| named_arg("shiftCounter").or_else(|| named_arg("backCounter")))
                {
                    Some(v) => sc(&v)?,
                    None => z,
                };
                let r = match args.get(2).cloned().or_else(|| named_arg("resolution")) {
                    Some(v) => sc(&v)?,
                    None => Arena::ONE,
                };
                let q = self.model.arena.clk(ClkOp::Rational, k, r);
                let u = args
                    .first()
                    .cloned()
                    .ok_or_else(|| anyhow!("{name}: missing argument"))?;
                self.map1(u, &mut |a, x| a.clk(op, x, q))
            }
            _ => unreachable!(),
        }
    }

    fn eval_int(&mut self, e: ExprId, what: &str) -> Result<i64> {
        let v = self.eval_param_expr(e).ok_or_else(|| {
            anyhow!(
                "{what} '{}' must be a parameter expression",
                self.model.show(e)
            )
        })?;
        if (v - v.round()).abs() > 1e-9 {
            bail!("{what} must be an integer, got {v}");
        }
        Ok(v.round() as i64)
    }

    /// Exact value of a factor: an integer expression or a `Rational` node.
    fn eval_rat(&mut self, e: ExprId, what: &str) -> Result<Rat> {
        if let Node::Clk(ClkOp::Rational, n, d) = self.model.arena.node(e) {
            let n = self.eval_int(n, what)?;
            let d = self.eval_int(d, what)?;
            if d == 0 {
                bail!("{what}: zero resolution");
            }
            return Ok(Rat::new(n, d));
        }
        Ok(Rat::new(self.eval_int(e, what)?, 1))
    }

    fn eval_clock(&mut self, cs: &mut ClockSolve, e: ExprId) -> Result<Option<ClockVal>> {
        match self.model.arena.node(e) {
            Node::Var(v) if self.clock_vars.contains(&v) => {
                let r = uf_find(&mut cs.parent, v);
                if let Some(c) = cs.memo.get(&r) {
                    return Ok(*c);
                }
                if !cs.busy.insert(r) {
                    bail!("cyclic clock definition at '{}'", self.model.var_name(v));
                }
                let val = match cs.def.get(&r).copied() {
                    Some(d) => self.eval_clock(cs, d)?,
                    None => None,
                };
                cs.busy.remove(&r);
                cs.memo.insert(r, val);
                Ok(val)
            }
            Node::Clk(op, a, b) => match op {
                ClkOp::Inferred => Ok(None),
                ClkOp::Periodic
                    if let Node::Clk(ClkOp::Rational, n, d) = self.model.arena.node(a)
                        && let (Some(n), Some(d)) =
                            (self.eval_param_expr(n), self.eval_param_expr(d))
                        && n.fract() == 0.0
                        && d.fract() == 0.0
                        && d != 0.0 =>
                {
                    // Rational clocks share one exact time base of 1 s, so their
                    // relative rates are known (MLS 16.3).
                    let base = match cs.rational_base {
                        Some(b) => b,
                        None => {
                            cs.bases.push(Base {
                                kind: BaseKind::Periodic(Arena::ONE),
                                solver: false,
                                implicit: false,
                            });
                            cs.rational_base = Some(cs.bases.len() - 1);
                            cs.bases.len() - 1
                        }
                    };
                    Ok(Some(ClockVal {
                        base,
                        period: Rat::new(n as i64, d as i64),
                        shift: Rat(0, 1),
                    }))
                }
                ClkOp::Periodic | ClkOp::Event => {
                    let base = match cs.base_of_expr.get(&e) {
                        Some(&b) => b,
                        None => {
                            let kind = if op == ClkOp::Event {
                                BaseKind::Event(a)
                            } else if let Node::Clk(ClkOp::Rational, n, d) =
                                self.model.arena.node(a)
                            {
                                BaseKind::Periodic(self.model.arena.div(n, d))
                            } else {
                                BaseKind::Periodic(a)
                            };
                            cs.bases.push(Base {
                                kind,
                                solver: false,
                                implicit: false,
                            });
                            cs.base_of_expr.insert(e, cs.bases.len() - 1);
                            cs.bases.len() - 1
                        }
                    };
                    Ok(Some(ClockVal {
                        base,
                        period: Rat(1, 1),
                        shift: Rat(0, 1),
                    }))
                }
                ClkOp::Solver => {
                    let c = self.eval_clock(cs, a)?;
                    if let Some(c) = c {
                        cs.bases[c.base].solver = true;
                        cs.bases[c.base].implicit |= b == Arena::ONE;
                    }
                    Ok(c)
                }
                ClkOp::Sub | ClkOp::Super | ClkOp::Shift | ClkOp::Back => {
                    let Some(c) = self.eval_clock(cs, a)? else {
                        return Ok(None);
                    };
                    let f = self.eval_rat(b, "clock factor")?;
                    Ok(Some(apply_rate(c, op, f)?))
                }
                _ => bail!("'{}' is not a clock", self.model.show(e)),
            },
            _ => bail!("'{}' is not a clock", self.model.show(e)),
        }
    }

    /// Lower all clocked partitions to event-driven discrete equations.
    pub(super) fn lower_clocks(&mut self) -> Result<()> {
        if !self.uses_clocks && self.clock_vars.is_empty() {
            return Ok(());
        }
        let mut cs = ClockSolve::default();
        // 1. Clock equations.
        let eqs = std::mem::take(&mut self.model.equations);
        let mut rest = Vec::with_capacity(eqs.len());
        let mut clock_eqs = Vec::new();
        for e in eqs {
            let is_clock = self.is_clock_expr(e.lhs) || self.is_clock_expr(e.rhs);
            if is_clock {
                clock_eqs.push(e);
            } else {
                rest.push(e);
            }
        }
        for &v in &self.clock_vars {
            cs.parent.insert(v, v);
        }
        for e in &clock_eqs {
            let (l, r) = (self.model.arena.node(e.lhs), self.model.arena.node(e.rhs));
            match (l, r) {
                (Node::Var(a), Node::Var(b))
                    if self.clock_vars.contains(&a) && self.clock_vars.contains(&b) =>
                {
                    let (ra, rb) = (uf_find(&mut cs.parent, a), uf_find(&mut cs.parent, b));
                    if ra != rb {
                        cs.parent.insert(ra, rb);
                        if let Some(d) = cs.def.remove(&ra) {
                            cs.def.entry(rb).or_insert(d);
                        }
                    }
                }
                (Node::Var(a), _) if self.clock_vars.contains(&a) => {
                    let ra = uf_find(&mut cs.parent, a);
                    cs.def.entry(ra).or_insert(e.rhs);
                }
                (_, Node::Var(b)) if self.clock_vars.contains(&b) => {
                    let rb = uf_find(&mut cs.parent, b);
                    cs.def.entry(rb).or_insert(e.lhs);
                }
                _ => {}
            }
        }
        // 2. Partitions over all equations, clocked when-clauses included.
        let mut all: Vec<(Equation, Option<ExprId>)> =
            rest.into_iter().map(|e| (e, None)).collect();
        // Equations of one clocked when-clause form one partition.
        let mut groups: Vec<std::ops::Range<usize>> = Vec::new();
        for (c, es) in std::mem::take(&mut self.clocked_whens) {
            let s0 = all.len();
            for e in es {
                all.push((e, Some(c)));
            }
            groups.push(s0..all.len());
        }
        let nvars = self.model.vars.len();
        let candidate = |s: &Self, v: VarId| {
            matches!(
                s.model.vars[v as usize].kind,
                VarKind::Continuous | VarKind::Discrete
            ) && !s.clock_vars.contains(&v)
        };
        let mut part: Vec<usize> = (0..nvars).collect();
        struct EqInfo {
            link: Vec<VarId>,
            seed: bool,
            /// (op, operand, factor/clock) of each boundary operator in the equation.
            cross: Vec<(ClkOp, ExprId, ExprId)>,
            ders: bool,
        }
        let mut infos = Vec::with_capacity(all.len());
        for (e, _) in &all {
            let mut info = EqInfo {
                link: Vec::new(),
                seed: false,
                cross: Vec::new(),
                ders: false,
            };
            let mut seen = FxHashSet::default();
            let mut stack = vec![e.lhs, e.rhs];
            let mut ch = Vec::new();
            while let Some(id) = stack.pop() {
                if !seen.insert(id) {
                    continue;
                }
                match self.model.arena.node(id) {
                    Node::Var(v) | Node::Pre(v) => {
                        if candidate(self, v) {
                            info.link.push(v);
                        }
                        continue;
                    }
                    Node::Der(v) => {
                        info.ders = true;
                        if candidate(self, v) {
                            info.link.push(v);
                        }
                        continue;
                    }
                    Node::Clk(op, a, b) => {
                        match op {
                            ClkOp::Previous => {
                                info.seed = true;
                                stack.push(a);
                            }
                            ClkOp::Interval | ClkOp::FirstTick => {
                                info.seed = true;
                                stack.push(a);
                            }
                            ClkOp::Sample => {
                                info.seed = true;
                                info.cross.push((op, a, b));
                            }
                            ClkOp::Sub | ClkOp::Super | ClkOp::Shift | ClkOp::Back => {
                                if !self.is_clock_expr(a) {
                                    info.seed = true;
                                    info.cross.push((op, a, b));
                                }
                            }
                            ClkOp::Hold | ClkOp::NoClock => info.cross.push((op, a, b)),
                            _ => {}
                        }
                        continue;
                    }
                    _ => {}
                }
                ch.clear();
                self.model.arena.children(id, &mut ch);
                stack.extend(ch.iter().copied());
            }
            for w in info.link.windows(2) {
                let (x, y) = (
                    ix_find(&mut part, w[0] as usize),
                    ix_find(&mut part, w[1] as usize),
                );
                if x != y {
                    part[x] = y;
                }
            }
            infos.push(info);
        }
        for g in &groups {
            let mut root: Option<usize> = None;
            for k in g.clone() {
                if let Some(&v) = infos[k].link.first() {
                    let x = ix_find(&mut part, v as usize);
                    match root {
                        None => root = Some(x),
                        Some(r) => {
                            let r = ix_find(&mut part, r);
                            if r != x {
                                part[x] = r;
                            }
                        }
                    }
                }
            }
        }
        let first_var = |s: &Self, e: ExprId| -> Option<VarId> {
            s.model
                .arena
                .vars_in(&[e])
                .into_iter()
                .filter(|&v| candidate(s, v))
                .min()
        };
        let mut clocked: FxHashSet<usize> = FxHashSet::default();
        let mut clock_of: FxHashMap<usize, ClockVal> = FxHashMap::default();
        let mut solver_part: FxHashSet<usize> = FxHashSet::default();
        let mut rels: Vec<RateRel> = Vec::new();
        for (k, (_, when_clock)) in all.iter().enumerate() {
            let info = &infos[k];
            let me = info.link.first().map(|&v| ix_find(&mut part, v as usize));
            if (info.seed || when_clock.is_some())
                && let Some(p) = me
            {
                clocked.insert(p);
            }
            if let (Some(c), Some(p)) = (when_clock, me)
                && let Some(cv) = self.eval_clock(&mut cs, *c)?
            {
                clock_of.insert(p, cv);
            }
            for &(op, a, b) in &info.cross.clone() {
                let src = first_var(self, a).map(|v| ix_find(&mut part, v as usize));
                match op {
                    ClkOp::Sample => {
                        if let Some(p) = me
                            && let Some(cv) = self.eval_clock(&mut cs, b)?
                        {
                            clock_of.insert(p, cv);
                        }
                    }
                    ClkOp::Hold | ClkOp::NoClock => {
                        if let Some(s) = src {
                            clocked.insert(s);
                        }
                    }
                    _ => {
                        if let Some(s) = src {
                            clocked.insert(s);
                        }
                        let factor = if op == ClkOp::Sub || op == ClkOp::Super {
                            if self.model.arena.is_zero(b) {
                                None
                            } else {
                                Some(self.eval_rat(b, "sampling factor")?)
                            }
                        } else {
                            Some(self.eval_rat(b, "shift")?)
                        };
                        if let (Some(p), Some(s)) = (me, src) {
                            rels.push(RateRel {
                                to: p,
                                from: s,
                                op,
                                factor,
                            });
                        }
                    }
                }
            }
        }
        if std::env::var("DAEDALUS_DEBUG_CLOCKS").is_ok() {
            for (k, (e, w)) in all.iter().enumerate() {
                if infos[k].seed || w.is_some() || !infos[k].cross.is_empty() {
                    eprintln!(
                        "eq {} = {}  when={:?} link={:?}",
                        self.model.show(e.lhs),
                        self.model.show(e.rhs),
                        w.map(|c| self.model.show(c)),
                        infos[k]
                            .link
                            .iter()
                            .map(|&v| ix_find(&mut part, v as usize))
                            .collect::<Vec<_>>()
                    );
                }
            }
            eprintln!(
                "clock_of {:?}\nrels {:?}",
                clock_of,
                rels.iter()
                    .map(|r| (r.to, r.from, r.op, r.factor))
                    .collect::<Vec<_>>()
            );
        }
        // 3. Clock inference over the rate relations.
        let mut default_base: Option<usize> = None;
        loop {
            let mut changed = false;
            for r in &rels {
                let Some(f) = r.factor else { continue };
                match (clock_of.get(&r.from).copied(), clock_of.get(&r.to).copied()) {
                    (Some(c), None) => {
                        clock_of.insert(r.to, apply_rate(c, r.op, f)?);
                        changed = true;
                    }
                    (None, Some(c)) => {
                        clock_of.insert(r.from, invert_rate(c, r.op, f)?);
                        changed = true;
                    }
                    _ => {}
                }
            }
            // A state machine whose clock is not inferred runs on a default clock of 1 s.
            if !changed {
                let mut sm_parts: Vec<usize> = self
                    .sm
                    .vars
                    .iter()
                    .map(|&v| ix_find(&mut part, v as usize))
                    .filter(|p| clocked.contains(p) && !clock_of.contains_key(p))
                    .collect();
                sm_parts.sort();
                if let Some(&p) = sm_parts.first() {
                    let base = match default_base {
                        Some(b) => b,
                        None => {
                            cs.bases.push(Base {
                                kind: BaseKind::Periodic(Arena::ONE),
                                solver: false,
                                implicit: false,
                            });
                            default_base = Some(cs.bases.len() - 1);
                            cs.bases.len() - 1
                        }
                    };
                    clock_of.insert(
                        p,
                        ClockVal {
                            base,
                            period: Rat(1, 1),
                            shift: Rat(0, 1),
                        },
                    );
                    continue;
                }
            }
            // An inferred factor still ties the two partitions to the same base.
            if !changed {
                for r in &rels {
                    if r.factor.is_some() {
                        continue;
                    }
                    if let (Some(a), Some(b)) = (clock_of.get(&r.from), clock_of.get(&r.to))
                        && a.base != b.base
                    {
                        bail!("sub/super-sampled partitions must share their base clock");
                    }
                }
                break;
            }
        }
        for &p in &clocked {
            if !clock_of.contains_key(&p) {
                let name = (0..nvars)
                    .find(|&v| ix_find(&mut part, v) == p)
                    .map(|v| self.model.vars[v].name.clone())
                    .unwrap_or_default();
                bail!("cannot infer the clock of the clocked partition containing '{name}'");
            }
        }
        // 4. Lower each base clock.
        let mut by_base: FxHashMap<usize, Vec<usize>> = FxHashMap::default();
        for &p in &clocked {
            by_base.entry(clock_of[&p].base).or_default().push(p);
        }
        let mut part_eqs: FxHashMap<usize, Vec<Equation>> = FxHashMap::default();
        let mut cont = Vec::new();
        for (k, (e, _)) in all.into_iter().enumerate() {
            let p = infos[k]
                .link
                .first()
                .map(|&v| ix_find(&mut part, v as usize));
            match p {
                Some(p) if clocked.contains(&p) => {
                    if infos[k].ders {
                        solver_part.insert(p);
                    }
                    part_eqs.entry(p).or_default().push(e)
                }
                _ => cont.push(e),
            }
        }
        // Continuous side: `hold(u)` is the held discrete value itself.
        for e in &mut cont {
            e.lhs = self.strip_clk(e.lhs, None);
            e.rhs = self.strip_clk(e.rhs, None);
        }
        self.model.equations = cont;
        let mut part_vars: FxHashMap<usize, Vec<VarId>> = FxHashMap::default();
        for v in 0..nvars as VarId {
            if !candidate(self, v) {
                continue;
            }
            let p = ix_find(&mut part, v as usize);
            if clocked.contains(&p) {
                part_vars.entry(p).or_default().push(v);
            }
        }
        let mut bases: Vec<usize> = by_base.keys().copied().collect();
        bases.sort();
        for b in bases {
            let mut parts = by_base[&b].clone();
            parts.sort();
            let mut q: i64 = 1;
            for p in &parts {
                let c = clock_of[p];
                q = lcm(q, c.period.1);
                q = lcm(q, c.shift.1);
            }
            let ticks = |r: Rat| r.0 * (q / r.1);
            let kmin = parts
                .iter()
                .map(|p| ticks(clock_of[p].shift))
                .min()
                .unwrap_or(0)
                .max(0);
            let base = cs.bases[b].clone();
            // The finest common tick: the gcd of all intervals and offsets.
            let mut g = 0i64;
            if matches!(base.kind, BaseKind::Periodic(_)) {
                for p in &parts {
                    let c = clock_of[p];
                    g = gcd(g, ticks(c.period));
                    g = gcd(g, ticks(c.shift) - kmin);
                }
            } else if q != 1 {
                bail!("an event clock cannot be super-sampled");
            }
            let g = g.max(1);
            let (cond, idx, mut acts, h, now) = match base.kind {
                BaseKind::Periodic(t) => {
                    let a = &mut self.model.arena;
                    let qe = a.constant(q as f64 / g as f64);
                    let h = a.div(t, qe);
                    let ke = a.constant(kmin as f64 / q as f64);
                    let start = a.mul(ke, t);
                    let (cond, n) = self.sample_counter(start, h);
                    let a = &mut self.model.arena;
                    let idx = a.pre(n);
                    // The nominal instant of the tick, free of the round-off of locating
                    // the event.
                    let ih = a.mul(idx, h);
                    let now = a.add(start, ih);
                    (cond, idx, Vec::new(), Some(h), now)
                }
                BaseKind::Event(c) => {
                    let k = self.sample_count;
                    self.sample_count += 1;
                    let mut v =
                        Variable::new(format!("$clock{k}.n"), VarKind::Discrete, VarType::Integer);
                    v.generated = true;
                    v.is_protected = true;
                    v.start = Some(Arena::ZERO);
                    let n = self.model.add_var(v);
                    self.when_assigned.insert(n);
                    // The time of the last tick, so that `interval()` is the time
                    // since the previous tick (zero at the first one).
                    let mut tv =
                        Variable::new(format!("$clock{k}.t"), VarKind::Discrete, VarType::Real);
                    tv.generated = true;
                    tv.is_protected = true;
                    tv.start = Some(Arena::ZERO);
                    let tl = self.model.add_var(tv);
                    self.when_assigned.insert(tl);
                    let a = &mut self.model.arena;
                    let idx = a.pre(n);
                    let inc = a.add(idx, Arena::ONE);
                    let now = a.time();
                    let last = a.pre(tl);
                    let since = a.sub(now, last);
                    let first = a.cmp(Cmp::Eq, idx, Arena::ZERO);
                    let h = a.if_(first, Arena::ZERO, since);
                    let acts = vec![WhenAction::Assign(n, inc), WhenAction::Assign(tl, now)];
                    // An event clock whose condition already holds at initialization
                    // ticks then (as the reference tool does), and afterwards whenever the
                    // condition becomes true: `{initial() and c, c}`.
                    let init = a.initial();
                    let at_init = a.f2(Fn2::And, init, c);
                    let cond = a.f2(Fn2::Or, at_init, c);
                    (cond, idx, acts, Some(h), now)
                }
            };
            for p in parts {
                let c = clock_of[&p];
                let m = ticks(c.period) / g;
                let k = (ticks(c.shift) - kmin) / g;
                if m <= 0 {
                    bail!("clock with a non-positive interval");
                }
                let a = &mut self.model.arena;
                let ke = a.constant(k as f64);
                let rel = a.sub(idx, ke);
                let first = a.cmp(Cmp::Eq, idx, ke);
                let active = if m == 1 && k == 0 {
                    Arena::ONE
                } else {
                    let me = a.constant(m as f64);
                    let md = a.f2(Fn2::Mod, rel, me);
                    let on = a.cmp(Cmp::Eq, md, Arena::ZERO);
                    let started = a.cmp(Cmp::Ge, idx, ke);
                    a.f2(Fn2::And, on, started)
                };
                let interval = match h {
                    Some(h) => {
                        let me = a.constant(m as f64);
                        a.mul(h, me)
                    }
                    None => Arena::ZERO,
                };
                let info = PartInfo {
                    interval,
                    first_tick: first,
                    now,
                };
                let mut es = part_eqs.remove(&p).unwrap_or_default();
                let mut vars = part_vars.remove(&p).unwrap_or_default();
                if solver_part.contains(&p) || base.solver {
                    self.discretize(&mut es, &mut vars, &info, base.implicit)?;
                }
                for e in &mut es {
                    e.lhs = self.strip_clk(e.lhs, Some(&info));
                    e.rhs = self.strip_clk(e.rhs, Some(&info));
                }
                let sols = self.solve_partition(&es, &vars)?;
                if vars.iter().any(|v| self.sm.vars.contains(v)) {
                    self.check_acyclic(&sols)?;
                }
                for (v, s) in sols {
                    let a = &mut self.model.arena;
                    let val = if active == Arena::ONE {
                        s
                    } else {
                        let pv = a.pre(v);
                        a.if_(active, s, pv)
                    };
                    acts.push(WhenAction::Assign(v, val));
                    self.when_assigned.insert(v);
                }
            }
            let o = self.model.origin("clocked partition");
            self.model.whens.push(WhenClause {
                branches: vec![(cond, acts)],
                origin: o,
            });
        }
        // Relations translated inside clocked equations were replaced by direct
        // comparisons; disarm their zero-crossing entries.
        for r in 0..self.model.arena.relations.len() {
            let rel = self.model.arena.relations[r];
            if self.has_clk(rel.lhs) || self.has_clk(rel.rhs) {
                self.model.arena.relations[r] = crate::expr::Relation {
                    op: Cmp::Gt,
                    lhs: Arena::ONE,
                    rhs: Arena::ZERO,
                };
            }
        }
        // Clocks themselves are compile-time objects.
        for &v in &self.clock_vars {
            let var = &mut self.model.vars[v as usize];
            var.kind = VarKind::Constant;
            var.binding = Some(Arena::ZERO);
            var.start = Some(Arena::ZERO);
            var.is_protected = true;
            var.generated = true;
        }
        Ok(())
    }

    fn has_clk(&self, e: ExprId) -> bool {
        let mut seen = FxHashSet::default();
        let mut stack = vec![e];
        let mut ch = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if matches!(self.model.arena.node(id), Node::Clk(..)) {
                return true;
            }
            ch.clear();
            self.model.arena.children(id, &mut ch);
            stack.extend(ch.iter().copied());
        }
        false
    }

    /// Replace the synchronous operators of a lowered equation by plain expressions.
    fn strip_clk(&mut self, e: ExprId, info: Option<&PartInfo>) -> ExprId {
        let mut cur = e;
        for _ in 0..64 {
            let mut found = false;
            let (interval, first, now) = info
                .map(|i| (i.interval, i.first_tick, i.now))
                .unwrap_or((Arena::ZERO, Arena::ZERO, Arena::ZERO));
            let mut memo = FxHashMap::default();
            let in_part = info.is_some();
            let next = self
                .model
                .arena
                .rewrite(cur, &mut memo, &mut |a, n| match n {
                    Node::Clk(op, x, _) => {
                        found = true;
                        Some(match op {
                            ClkOp::Previous => {
                                a.rewrite(x, &mut FxHashMap::default(), &mut |a2, n| {
                                    if let Node::Var(v) = n {
                                        Some(a2.pre(v))
                                    } else {
                                        None
                                    }
                                })
                            }
                            ClkOp::Interval => interval,
                            ClkOp::FirstTick => first,
                            // `sample(u)` reads the left limit of u at the tick: the
                            // value before this instant's events changed it.
                            ClkOp::Sample if in_part => {
                                a.rewrite(x, &mut FxHashMap::default(), &mut |a2, n| {
                                    if let Node::Var(v) = n {
                                        Some(a2.pre(v))
                                    } else {
                                        None
                                    }
                                })
                            }
                            _ => x,
                        })
                    }
                    Node::Time if in_part => Some(now),
                    // Inside a partition, comparisons are evaluated at the tick, not latched.
                    Node::Rel(r) if in_part => {
                        let rel = a.relations[r as usize];
                        found = true;
                        Some(a.cmp(rel.op, rel.lhs, rel.rhs))
                    }
                    _ => None,
                });
            cur = next;
            if !found {
                break;
            }
        }
        cur
    }

    /// Match each equation of a partition to one unknown and solve it symbolically.
    fn solve_partition(&mut self, es: &[Equation], vars: &[VarId]) -> Result<Vec<(VarId, ExprId)>> {
        let index: FxHashMap<VarId, usize> =
            vars.iter().enumerate().map(|(i, &v)| (v, i)).collect();
        let mut adj: Vec<Vec<usize>> = Vec::with_capacity(es.len());
        for e in es {
            let mut row = Vec::new();
            self.model.arena.visit_leaves(e.lhs, &mut |n| {
                if let Node::Var(v) = n
                    && let Some(&i) = index.get(&v)
                {
                    row.push(i);
                }
            });
            self.model.arena.visit_leaves(e.rhs, &mut |n| {
                if let Node::Var(v) = n
                    && let Some(&i) = index.get(&v)
                {
                    row.push(i);
                }
            });
            row.sort();
            row.dedup();
            adj.push(row);
        }
        if es.len() != vars.len() {
            let names: Vec<&str> = vars
                .iter()
                .take(6)
                .map(|&v| self.model.var_name(v))
                .collect();
            bail!(
                "clocked partition has {} equations for {} unknowns ({}{})",
                es.len(),
                vars.len(),
                names.join(", "),
                if vars.len() > 6 { ", ..." } else { "" }
            );
        }
        let mut owner: Vec<Option<usize>> = vec![None; vars.len()];
        fn augment(
            e: usize,
            adj: &[Vec<usize>],
            owner: &mut [Option<usize>],
            seen: &mut [bool],
        ) -> bool {
            for &v in &adj[e] {
                if seen[v] {
                    continue;
                }
                seen[v] = true;
                if owner[v].is_none() || augment(owner[v].unwrap(), adj, owner, seen) {
                    owner[v] = Some(e);
                    return true;
                }
            }
            false
        }
        for e in 0..es.len() {
            let mut seen = vec![false; vars.len()];
            if !augment(e, &adj, &mut owner, &mut seen) {
                if std::env::var("DAEDALUS_DEBUG_CLOCKS").is_ok() {
                    for q in es {
                        eprintln!("  {} = {}", self.model.show(q.lhs), self.model.show(q.rhs));
                    }
                    let names: Vec<&str> = vars.iter().map(|&v| self.model.var_name(v)).collect();
                    eprintln!("  unknowns: {}", names.join(", "));
                }
                let shown: Vec<String> = es
                    .iter()
                    .map(|q| format!("{} = {}", self.model.show(q.lhs), self.model.show(q.rhs)))
                    .collect();
                // `x = pre(x) + h*pre(der(x))` (the ControlledMixingUnit filter step) is
                // not a generic singular system: the previous derivative is not an
                // unknown of this tick, and matching it as one reports "singular".
                if shown.iter().any(|s| s.contains("pre(der(")) {
                    bail!(
                        "unsupported: a clocked state is updated from its own pre(der(x)) ({}); \
                         that previous derivative is not solved as an unknown of the same tick",
                        shown
                            .iter()
                            .find(|s| s.contains("pre(der("))
                            .map(|s| s.as_str())
                            .unwrap_or("")
                    );
                }
                bail!(
                    "clocked partition is singular at '{} = {}'",
                    self.model.show(es[e].lhs),
                    self.model.show(es[e].rhs)
                );
            }
        }
        // Rows of an if-equation with a time-varying condition are solved branch by
        // branch for the unknowns matched to them, then joined with the conditions.
        let mut groups: FxHashMap<Vec<ExprId>, Vec<usize>> = FxHashMap::default();
        let mut split: Vec<Option<Vec<ExprId>>> = vec![None; es.len()];
        for (k, e) in es.iter().enumerate() {
            let r = self.model.arena.sub(e.lhs, e.rhs);
            let (conds, branches) = self.if_chain(r);
            if !conds.is_empty() {
                split[k] = Some(branches);
                groups.entry(conds).or_default().push(k);
            }
        }
        let mut out = Vec::with_capacity(vars.len());
        let mut done = vec![false; es.len()];
        let row_var: FxHashMap<usize, VarId> = owner
            .iter()
            .enumerate()
            .map(|(vi, e)| (e.unwrap(), vars[vi]))
            .collect();
        let mut keys: Vec<Vec<ExprId>> = groups.keys().cloned().collect();
        keys.sort();
        for conds in keys {
            let rows = &groups[&conds];
            let unknowns: Vec<VarId> = rows.iter().map(|k| row_var[k]).collect();
            let nb = conds.len() + 1;
            let mut per_branch: Vec<FxHashMap<VarId, ExprId>> = Vec::new();
            for bi in 0..nb {
                let res: Vec<ExprId> = rows
                    .iter()
                    .map(|&k| split[k].as_ref().unwrap()[bi])
                    .collect();
                per_branch.push(self.solve_sequential(&res, &unknowns)?);
            }
            for &v in &unknowns {
                let a = &mut self.model.arena;
                let mut x = per_branch[nb - 1][&v];
                for bi in (0..conds.len()).rev() {
                    x = a.if_(conds[bi], per_branch[bi][&v], x);
                }
                out.push((v, x));
            }
            for &k in rows {
                done[k] = true;
            }
        }
        for (vi, eo) in owner.iter().enumerate() {
            if done[eo.unwrap()] {
                continue;
            }
            let e = es[eo.unwrap()];
            let v = vars[vi];
            let r = self.model.arena.sub(e.lhs, e.rhs);
            let s = self.solve_for(r, v).ok_or_else(|| {
                anyhow!(
                    "clocked equation '{} = {}' cannot be solved for '{}'",
                    self.model.show(e.lhs),
                    self.model.show(e.rhs),
                    self.model.var_name(v)
                )
            })?;
            out.push((v, s));
        }
        Ok(out)
    }

    /// Reject a cycle among the solved equations of a partition (an algebraic loop
    /// between discrete variables, such as an immediate transition whose condition
    /// reads a variable its from-state computes in the same tick).
    fn check_acyclic(&self, sols: &[(VarId, ExprId)]) -> Result<()> {
        let index: FxHashMap<VarId, usize> =
            sols.iter().enumerate().map(|(i, &(v, _))| (v, i)).collect();
        let deps: Vec<Vec<usize>> = sols
            .iter()
            .map(|&(_, e)| {
                let mut d = Vec::new();
                self.model.arena.visit_leaves(e, &mut |n| {
                    if let Node::Var(w) = n
                        && let Some(&j) = index.get(&w)
                    {
                        d.push(j);
                    }
                });
                d
            })
            .collect();
        // Iterative depth-first search with colours: 1 = on the stack, 2 = done.
        let mut state = vec![0u8; sols.len()];
        for root in 0..sols.len() {
            if state[root] != 0 {
                continue;
            }
            let mut stack: Vec<(usize, usize)> = vec![(root, 0)];
            state[root] = 1;
            while let Some(&mut (k, ref mut next)) = stack.last_mut() {
                if let Some(&j) = deps[k].get(*next) {
                    *next += 1;
                    match state[j] {
                        0 => {
                            state[j] = 1;
                            stack.push((j, 0));
                        }
                        1 => {
                            let at = stack.iter().position(|&(x, _)| x == j).unwrap_or(0);
                            let names: Vec<&str> = stack[at..]
                                .iter()
                                .map(|&(x, _)| self.model.var_name(sols[x].0))
                                .filter(|n| !n.contains('$'))
                                .take(6)
                                .collect();
                            bail!(
                                "algebraic loop between clocked variables of a state machine{}{} (an immediate \
                                 transition whose condition depends on its from-state?)",
                                if names.is_empty() { "" } else { ": " },
                                names.join(", ")
                            );
                        }
                        _ => {}
                    }
                } else {
                    state[k] = 2;
                    stack.pop();
                }
            }
        }
        Ok(())
    }

    /// `if c1 then r1 elseif c2 then r2 else r3` as `([c1, c2], [r1, r2, r3])`.
    fn if_chain(&self, r: ExprId) -> (Vec<ExprId>, Vec<ExprId>) {
        let mut conds = Vec::new();
        let mut branches = Vec::new();
        let mut cur = r;
        while let Node::If(c, a, b) = self.model.arena.node(cur) {
            if self.model.arena.as_const(c).is_some() {
                break;
            }
            conds.push(c);
            branches.push(a);
            cur = b;
        }
        branches.push(cur);
        (conds, branches)
    }

    /// Residual `r` solved for `v` when `r` is linear in `v`.
    pub(super) fn solve_for(&mut self, r: ExprId, v: VarId) -> Option<ExprId> {
        let a = &mut self.model.arena;
        let mut memo = FxHashMap::default();
        let d = a.diff(r, Node::Var(v), &mut memo);
        if a.contains(d, Node::Var(v)) || a.is_zero(d) {
            return None;
        }
        let r0 = a.substitute(r, &|n| match n {
            Node::Var(w) if w == v => Some(Arena::ZERO),
            _ => None,
        });
        let nr = a.neg(r0);
        Some(if a.is_one(d) { nr } else { a.div(nr, d) })
    }

    /// Solve residuals for `unknowns` one at a time, always picking a residual with a
    /// single unknown left and substituting each solution into the rest.
    fn solve_sequential(
        &mut self,
        res: &[ExprId],
        unknowns: &[VarId],
    ) -> Result<FxHashMap<VarId, ExprId>> {
        let mut res = res.to_vec();
        let mut used = vec![false; res.len()];
        let mut sol: FxHashMap<VarId, ExprId> = FxHashMap::default();
        while sol.len() < unknowns.len() {
            let mut progress = false;
            for k in 0..res.len() {
                if used[k] {
                    continue;
                }
                let left: Vec<VarId> = unknowns
                    .iter()
                    .copied()
                    .filter(|&v| {
                        !sol.contains_key(&v) && self.model.arena.contains(res[k], Node::Var(v))
                    })
                    .collect();
                if left.len() != 1 {
                    continue;
                }
                let v = left[0];
                let Some(x) = self.solve_for(res[k], v) else {
                    continue;
                };
                used[k] = true;
                sol.insert(v, x);
                for j in 0..res.len() {
                    if !used[j] {
                        res[j] = self.model.arena.substitute(res[j], &|n| match n {
                            Node::Var(w) if w == v => Some(x),
                            _ => None,
                        });
                    }
                }
                progress = true;
            }
            if !progress {
                let names: Vec<&str> = unknowns.iter().map(|&v| self.model.var_name(v)).collect();
                bail!(
                    "cannot solve the if-equation branches for {}",
                    names.join(", ")
                );
            }
        }
        Ok(sol)
    }

    /// Euler for a clocked partition with `der()`: `x = previous(x) +
    /// interval()*previous(der(x))` (explicit) or `x = previous(x) + interval()*der(x)`
    /// (implicit), with `der(x)` a new clocked unknown.
    fn discretize(
        &mut self,
        es: &mut Vec<Equation>,
        vars: &mut Vec<VarId>,
        info: &PartInfo,
        implicit: bool,
    ) -> Result<()> {
        let mut states: Vec<VarId> = Vec::new();
        for e in es.iter() {
            for side in [e.lhs, e.rhs] {
                self.model.arena.visit_leaves(side, &mut |n| {
                    if let Node::Der(v) = n
                        && !states.contains(&v)
                    {
                        states.push(v);
                    }
                });
            }
        }
        let mut dvar: FxHashMap<VarId, VarId> = FxHashMap::default();
        for &x in &states {
            let mut v = Variable::new(
                format!("der({})", self.model.var_name(x)),
                VarKind::Discrete,
                VarType::Real,
            );
            v.generated = true;
            v.start = Some(Arena::ZERO);
            let d = self.model.add_var(v);
            dvar.insert(x, d);
            vars.push(d);
        }
        for e in es.iter_mut() {
            let m = &dvar;
            let a = &mut self.model.arena;
            let f = |n: Node| match n {
                Node::Der(v) => m.get(&v).copied(),
                _ => None,
            };
            let mut vmap: FxHashMap<VarId, ExprId> = FxHashMap::default();
            for (&x, &d) in m {
                let _ = x;
                vmap.insert(d, a.var(d));
            }
            e.lhs = a.substitute(e.lhs, &|n| f(n).map(|d| vmap[&d]));
            e.rhs = a.substitute(e.rhs, &|n| f(n).map(|d| vmap[&d]));
        }
        let o = self.model.origin("clocked discretisation");
        for &x in &states {
            let d = dvar[&x];
            let a = &mut self.model.arena;
            let px = a.pre(x);
            let pd = if implicit { a.var(d) } else { a.pre(d) };
            let step = a.mul(info.interval, pd);
            let rhs = a.add(px, step);
            let xv = a.var(x);
            es.push(Equation {
                lhs: xv,
                rhs,
                origin: o,
            });
        }
        Ok(())
    }

    /// `sample(start, interval)` that also returns its tick counter.
    fn sample_counter(&mut self, start: ExprId, interval: ExprId) -> (ExprId, VarId) {
        let cond = self.sample(start, interval);
        let n = self
            .model
            .vars
            .iter()
            .rposition(|v| v.name.ends_with(".n") && v.name.starts_with("$sample"))
            .unwrap() as VarId;
        (cond, n)
    }
}

struct PartInfo {
    interval: ExprId,
    first_tick: ExprId,
    /// `time` at the tick.
    now: ExprId,
}

fn lcm(a: i64, b: i64) -> i64 {
    a / gcd(a, b) * b
}

fn apply_rate(c: ClockVal, op: ClkOp, f: Rat) -> Result<ClockVal> {
    Ok(match op {
        ClkOp::Sub => ClockVal {
            period: c.period.mul(f),
            ..c
        },
        ClkOp::Super => {
            if f.0 == 0 {
                bail!("superSample factor must be positive");
            }
            ClockVal {
                period: c.period.div(f),
                ..c
            }
        }
        ClkOp::Shift => ClockVal {
            shift: c.shift.add(c.period.mul(f)),
            ..c
        },
        ClkOp::Back => ClockVal {
            shift: c.shift.add(c.period.mul(f).neg()),
            ..c
        },
        _ => c,
    })
}

fn invert_rate(c: ClockVal, op: ClkOp, f: Rat) -> Result<ClockVal> {
    Ok(match op {
        ClkOp::Sub => ClockVal {
            period: c.period.div(f),
            ..c
        },
        ClkOp::Super => ClockVal {
            period: c.period.mul(f),
            ..c
        },
        // shift(u) = s + P f  =>  s = shift - P f (same period)
        ClkOp::Shift => ClockVal {
            shift: c.shift.add(c.period.mul(f).neg()),
            ..c
        },
        ClkOp::Back => ClockVal {
            shift: c.shift.add(c.period.mul(f)),
            ..c
        },
        _ => c,
    })
}
