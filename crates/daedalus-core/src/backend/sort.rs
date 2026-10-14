//! Causalisation: matching, BLT sorting, symbolic solving and tearing; plus the
//! initialization problem.

use super::graph::{Bipartite, NONE, augment, hopcroft_karp, tarjan_blt};
use super::{Block, Eq, Work};
use crate::expr::{Arena, ExprId, Node, VarId};
use anyhow::{Result, bail};
use rustc_hash::{FxHashMap, FxHashSet};

impl Work {
    fn contains_any(&self, e: ExprId, set: &FxHashSet<VarId>) -> bool {
        let mut found = false;
        self.arena.visit_leaves(e, &mut |n| {
            if let Node::Var(v) = n
                && set.contains(&v)
            {
                found = true;
            }
        });
        found
    }

    /// True if `e` contains a branch (if-expression or relation) that depends on a variable in `set`.
    fn has_branch_on(&self, e: ExprId, set: &FxHashSet<VarId>) -> bool {
        let mut stack = vec![e];
        let mut seen = FxHashSet::default();
        let mut ch = Vec::new();
        while let Some(x) = stack.pop() {
            if !seen.insert(x) {
                continue;
            }
            match self.arena.node(x) {
                Node::If(..) | Node::Cmp(..) => {
                    if self.contains_any(x, set) {
                        return true;
                    }
                }
                Node::Rel(r) => {
                    let rel = self.arena.relations[r as usize];
                    if self.contains_any(rel.lhs, set) || self.contains_any(rel.rhs, set) {
                        return true;
                    }
                }
                _ => {}
            }
            ch.clear();
            self.arena.children(x, &mut ch);
            stack.extend(ch.iter().copied());
        }
        false
    }

    /// Try to solve `lhs = rhs` explicitly for `v`. Returns the solution expression.
    pub fn solve_for(&mut self, eq: &Eq, v: VarId) -> Option<ExprId> {
        self.solve_class(eq, v).map(|(e, _)| e)
    }

    /// Like [`Work::solve_for`], also returning how safe the solution is:
    /// 0 = constant coefficient, 1 = coefficient depends on parameters only,
    /// 2 = division by a time-varying quantity (may vanish).
    pub fn solve_class(&mut self, eq: &Eq, v: VarId) -> Option<(ExprId, u8)> {
        let target = Node::Var(v);
        let a = &self.arena;
        if a.node(eq.lhs) == target && !a.contains(eq.rhs, target) {
            return Some((eq.rhs, 0));
        }
        if a.node(eq.rhs) == target && !a.contains(eq.lhs, target) {
            return Some((eq.lhs, 0));
        }
        let r = self.arena.sub(eq.lhs, eq.rhs);
        let only: FxHashSet<VarId> = [v].into_iter().collect();
        if self.has_branch_on(r, &only) {
            return None;
        }
        let mut memo = FxHashMap::default();
        let d = self.arena.diff(r, target, &mut memo);
        if self.arena.is_zero(d) || self.arena.contains(d, target) {
            return None;
        }
        // Linear: r = d*v + r0  ->  v = -r0/d
        let zero = Arena::ZERO;
        let r0 = self
            .arena
            .substitute(r, &|n| if n == target { Some(zero) } else { None });
        if self.arena.contains(r0, target) {
            return None;
        }
        let num = self.arena.neg(r0);
        let class = if self.arena.as_const(d).is_some() {
            0
        } else {
            let mut varying = false;
            self.arena.visit_leaves(d, &mut |n| match n {
                Node::Var(x) => {
                    if self.unknown[x as usize]
                        || self.vars[x as usize].kind != crate::flat::VarKind::Parameter
                            && self.vars[x as usize].kind != crate::flat::VarKind::Constant
                    {
                        varying = true;
                    }
                }
                Node::Time | Node::Pre(_) | Node::Der(_) => varying = true,
                _ => {}
            });
            if varying { 2 } else { 1 }
        };
        Some((self.arena.div(num, d), class))
    }

    /// The coefficient of `v` in `eq` is non-zero at the guess point `pt`.
    fn coefficient_nonzero(&mut self, eq: &Eq, v: VarId, pt: &[f64]) -> bool {
        let r = self.arena.sub(eq.lhs, eq.rhs);
        let mut memo = FxHashMap::default();
        let d = self.arena.diff(r, Node::Var(v), &mut memo);
        let vals = super::eval::Values {
            var: &|x| pt[x as usize],
            pre: &|x| pt[x as usize],
            time: 0.0,
            initial: false,
            rel: None,
        };
        let x = super::eval::eval(&self.arena, d, &vals, &mut FxHashMap::default());
        x.is_finite() && x.abs() > 1e-12
    }

    fn is_linear_in(&mut self, e: ExprId, set: &FxHashSet<VarId>) -> bool {
        if self.has_branch_on(e, set) {
            return false;
        }
        for &v in set {
            let mut memo = FxHashMap::default();
            let d = self.arena.diff(e, Node::Var(v), &mut memo);
            if self.contains_any(d, set) {
                return false;
            }
        }
        true
    }
}

/// Causalise `eqs` for the unknowns selected by `is_unknown` (among `w.unknown`).
pub fn causalize(
    w: &mut Work,
    eqs: &[Eq],
    is_unknown: &dyn Fn(VarId) -> bool,
    tearing: bool,
    label: &str,
) -> Result<Vec<Block>> {
    // Compact unknown indexing.
    let mut idx: FxHashMap<VarId, u32> = FxHashMap::default();
    let mut vars: Vec<VarId> = Vec::new();
    let mut adj: Vec<Vec<u32>> = Vec::with_capacity(eqs.len());
    for e in eqs {
        let mut row = Vec::new();
        for v in w.eq_unknowns(e) {
            if !is_unknown(v) {
                continue;
            }
            let i = *idx.entry(v).or_insert_with(|| {
                vars.push(v);
                (vars.len() - 1) as u32
            });
            row.push(i);
        }
        adj.push(row);
    }
    // Unknowns that appear in no equation.
    for v in 0..w.vars.len() as VarId {
        if w.unknown[v as usize] && is_unknown(v) && !idx.contains_key(&v) {
            idx.insert(v, vars.len() as u32);
            vars.push(v);
        }
    }
    let n_eq = eqs.len();
    let n_var = vars.len();
    let g = Bipartite {
        n_eq,
        n_var,
        adj: &adj,
    };
    let (eq_match, var_match) = hopcroft_karp(&g);
    let free_eqs: Vec<usize> = (0..n_eq).filter(|&e| eq_match[e] == NONE).collect();
    let free_vars: Vec<VarId> = (0..n_var)
        .filter(|&v| var_match[v] == NONE)
        .map(|v| vars[v])
        .collect();
    if !free_eqs.is_empty() || !free_vars.is_empty() {
        let mut msg = format!(
            "{label} problem is structurally singular ({n_eq} equations, {n_var} unknowns).\n"
        );
        if !free_vars.is_empty() {
            let names: Vec<&str> = free_vars.iter().take(30).map(|&v| w.name(v)).collect();
            msg.push_str(&format!(
                "  unknowns not determined by any equation: {}\n",
                names.join(", ")
            ));
        }
        for &e in free_eqs.iter().take(30) {
            msg.push_str(&format!(
                "  equation with no unknown left: {}\n",
                w.eq_text(&eqs[e])
            ));
        }
        bail!(msg);
    }
    let comps = tarjan_blt(&adj, &var_match);
    let mut blocks = Vec::with_capacity(comps.len());
    // Guess point propagated through the blocks in order (start values, then what the
    // explicit assignments give from them): tearing checks its pivots against it.
    let mut pt: Vec<f64> = (0..w.vars.len() as VarId)
        .map(|x| {
            if w.unknown[x as usize] {
                w.start_value(x)
            } else {
                w.param_values[x as usize]
            }
        })
        .collect();
    for comp in comps {
        if comp.len() == 1 {
            let e = comp[0] as usize;
            let v = vars[eq_match[e] as usize];
            match w.solve_for(&eqs[e], v) {
                Some(x) => {
                    let y = eval_at(w, x, &pt);
                    if y.is_finite() {
                        pt[v as usize] = y;
                    }
                    blocks.push(Block::Assign { var: v, expr: x })
                }
                None => {
                    let r = w.arena.sub(eqs[e].lhs, eqs[e].rhs);
                    let set: FxHashSet<VarId> = [v].into_iter().collect();
                    let linear = w.is_linear_in(r, &set);
                    blocks.push(Block::Solve {
                        tears: vec![v],
                        inner: vec![],
                        residuals: vec![r],
                        linear,
                    });
                }
            }
            continue;
        }
        let ceqs: Vec<Eq> = comp.iter().map(|&e| eqs[e as usize]).collect();
        let cvars: Vec<VarId> = comp
            .iter()
            .map(|&e| vars[eq_match[e as usize] as usize])
            .collect();
        let b = tear(w, &ceqs, &cvars, tearing, &pt);
        if let Block::Solve { inner, .. } = &b {
            for &(v, x) in inner {
                let y = eval_at(w, x, &pt);
                if y.is_finite() {
                    pt[v as usize] = y;
                }
            }
        }
        blocks.push(b);
    }
    Ok(blocks)
}

fn eval_at(w: &Work, e: ExprId, pt: &[f64]) -> f64 {
    let vals = super::eval::Values {
        var: &|x| pt[x as usize],
        pre: &|x| pt[x as usize],
        time: 0.0,
        initial: true,
        rel: None,
    };
    super::eval::eval(&w.arena, e, &vals, &mut FxHashMap::default())
}

/// Tear an algebraic loop: greedily assign equations with one remaining unknown that can be
/// solved explicitly, otherwise pick the variable occurring most often as tear variable.
fn tear(w: &mut Work, eqs: &[Eq], vars: &[VarId], tearing: bool, pt: &[f64]) -> Block {
    let set: FxHashSet<VarId> = vars.iter().copied().collect();
    let residual_all: Vec<ExprId> = eqs.iter().map(|e| w.arena.sub(e.lhs, e.rhs)).collect();
    let linear = residual_all.iter().all(|&r| w.is_linear_in(r, &set));
    if !tearing {
        return Block::Solve {
            tears: vars.to_vec(),
            inner: vec![],
            residuals: residual_all,
            linear,
        };
    }
    let occ: Vec<Vec<VarId>> = eqs
        .iter()
        .map(|e| {
            w.eq_unknowns(e)
                .into_iter()
                .filter(|v| set.contains(v))
                .collect()
        })
        .collect();
    let mut known: FxHashSet<VarId> = FxHashSet::default();
    let mut used = vec![false; eqs.len()];
    let mut inner: Vec<(VarId, ExprId)> = Vec::new();
    let mut tears: Vec<VarId> = Vec::new();
    // Cache of failed solve attempts: (eq, var).
    let mut unsolvable: FxHashSet<(usize, VarId)> = FxHashSet::default();
    loop {
        // Propagate: repeatedly solve equations with one remaining unknown, safest first.
        loop {
            let mut best: Option<(usize, VarId, ExprId, u8)> = None;
            for i in 0..eqs.len() {
                if used[i] {
                    continue;
                }
                let mut rem = occ[i].iter().copied().filter(|v| !known.contains(v));
                let (Some(v), None) = (rem.next(), rem.next()) else {
                    continue;
                };
                if unsolvable.contains(&(i, v)) {
                    continue;
                }
                // Inside a loop only solve through coefficients that are non-zero at the
                // parameter and start values; a vanishing one (I[1,3] = 0, R.T[2,3] of an
                // unrotated frame) stays a residual for the Newton/linear solve.
                let sc = w
                    .solve_class(&eqs[i], v)
                    .filter(|&(_, c)| c == 0 || w.coefficient_nonzero(&eqs[i], v, pt));
                match sc {
                    Some((x, c)) => {
                        if best.is_none_or(|b| c < b.3) {
                            best = Some((i, v, x, c));
                            if c == 0 {
                                break;
                            }
                        }
                    }
                    None => {
                        unsolvable.insert((i, v));
                    }
                }
            }
            let Some((i, v, x, _)) = best else { break };
            inner.push((v, x));
            known.insert(v);
            used[i] = true;
        }
        if known.len() == vars.len() {
            break;
        }
        // Pick a tear variable: most occurrences in unused equations, ties broken by
        // occurrences in equations that would become solvable.
        let mut count: FxHashMap<VarId, usize> = FxHashMap::default();
        for i in 0..eqs.len() {
            if used[i] {
                continue;
            }
            let rem: Vec<VarId> = occ[i]
                .iter()
                .copied()
                .filter(|v| !known.contains(v))
                .collect();
            for &v in &rem {
                *count.entry(v).or_default() += if rem.len() == 2 { 3 } else { 1 };
            }
        }
        let t = vars
            .iter()
            .copied()
            .filter(|v| !known.contains(v))
            .max_by_key(|v| (count.get(v).copied().unwrap_or(0), std::cmp::Reverse(*v)))
            .unwrap();
        tears.push(t);
        known.insert(t);
    }
    let residuals: Vec<ExprId> = (0..eqs.len())
        .filter(|&i| !used[i])
        .map(|i| residual_all[i])
        .collect();
    debug_assert_eq!(residuals.len(), tears.len());
    Block::Solve {
        tears,
        inner,
        residuals,
        linear,
    }
}

/// Build and causalise the initialization problem: all unknowns (states included) from the
/// simulation equations, initial equations, `fixed=true` start values and, where still
/// underdetermined, start values of states (then of other variables) as needed.
pub fn initialization(w: &mut Work, states: &[VarId], tearing: bool) -> Result<Vec<Block>> {
    let mut mandatory: Vec<Eq> = w.eqs.clone();
    mandatory.extend(w.init_eqs.iter().copied());
    let state_set: FxHashSet<VarId> = states.iter().copied().collect();
    let start_eq = |w: &mut Work, v: VarId| -> Eq {
        let rhs = match w.vars[v as usize].start {
            Some(s) => s,
            None => Arena::ZERO,
        };
        let lhs = w.arena.var(v);
        let origin = w.origins.len() as u32;
        w.origins.push(format!("start value of {}", w.name(v)));
        Eq {
            lhs,
            rhs,
            origin,
            parent: None,
            derived: None,
        }
    };
    for v in 0..w.vars.len() as VarId {
        if w.unknown[v as usize] && w.vars[v as usize].fixed == Some(true) {
            let e = start_eq(w, v);
            mandatory.push(e);
        }
    }
    // Optional equations by priority: states without explicit fixed, then other variables.
    let mut optional: Vec<VarId> = Vec::new();
    for &s in states {
        if w.vars[s as usize].fixed.is_none() {
            optional.push(s);
        }
    }
    let n_states_optional = optional.len();
    for v in 0..w.vars.len() as VarId {
        let fixed = w.vars[v as usize].fixed;
        if w.unknown[v as usize]
            && fixed != Some(true)
            && !(state_set.contains(&v) && fixed.is_none())
        {
            optional.push(v);
        }
    }
    let nv = w.vars.len();
    let mut adj: Vec<Vec<u32>> = mandatory.iter().map(|e| w.eq_unknowns(e)).collect();
    let g = Bipartite {
        n_eq: adj.len(),
        n_var: nv,
        adj: &adj,
    };
    let (em, vm) = hopcroft_karp(&g);
    let mut eq_match = em;
    let mut var_match = vm;
    // Drop redundant initial / fixed equations that cannot be matched.
    let n_sim = w.eqs.len();
    let mut keep: Vec<bool> = vec![true; mandatory.len()];
    for e in 0..mandatory.len() {
        if eq_match[e] == NONE {
            if e < n_sim {
                bail!(
                    "initialization problem is overdetermined at {}",
                    w.eq_text(&mandatory[e])
                );
            }
            w.warnings.push(format!(
                "initialization: redundant equation ignored: {}",
                w.eq_text(&mandatory[e])
            ));
            keep[e] = false;
        }
    }
    let mut selected: Vec<Eq> = mandatory
        .iter()
        .zip(&keep)
        .filter(|(_, k)| **k)
        .map(|(e, _)| *e)
        .collect();
    let mut visited_eq: Vec<u32> = vec![0; mandatory.len() + optional.len()];
    let mut visited_var: Vec<u32> = vec![0; nv];
    let mut stamp = 0;
    let mut defaulted: Vec<String> = Vec::new();
    let n_unknown = w.unknown.iter().filter(|&&u| u).count();
    let mut matched = eq_match.iter().filter(|&&m| m != NONE).count();
    for (k, &v) in optional.iter().enumerate() {
        if matched == n_unknown {
            break;
        }
        let e = start_eq(w, v);
        adj.push(w.eq_unknowns(&e));
        eq_match.push(NONE);
        let ei = adj.len() - 1;
        stamp += 1;
        let ok = augment(
            ei,
            &adj,
            &mut eq_match,
            &mut var_match,
            &mut visited_eq,
            &mut visited_var,
            stamp,
            &|_| true,
        );
        if ok {
            matched += 1;
            selected.push(e);
            if k >= n_states_optional {
                defaulted.push(w.name(v).to_string());
            }
        }
    }
    if !defaulted.is_empty() {
        w.warnings.push(format!(
            "initialization underdetermined; using start values for: {}",
            defaulted
                .iter()
                .take(20)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    let unknown = w.unknown.clone();
    causalize(
        w,
        &selected,
        &|v| unknown[v as usize],
        tearing,
        "initialization",
    )
}
