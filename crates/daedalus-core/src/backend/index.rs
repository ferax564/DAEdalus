//! Derivative variables, Pantelides index reduction and dummy-derivative state selection.
//!
//! After [`create_derivative_vars`] every `der(v)` is an ordinary variable linked
//! through `der_of`/`base_of`. [`pantelides`] differentiates the minimal set of
//! equations that makes the system structurally index 1, and [`select_states`]
//! applies the Mattsson–Söderlind dummy-derivative method to pick the states,
//! honouring `stateSelect` and checking the choice numerically at start values.

use super::graph::{NONE, augment, augment_traced};
use super::{Eq, Work, eval};
use crate::expr::{Arena, ExprId, Node, VarId};
use crate::flat::{StateSelect, VarKind, VarType, Variable, WhenAction};
use anyhow::{Result, bail};
use rustc_hash::{FxHashMap, FxHashSet};

/// Total time derivative of `e` before derivative variables exist:
/// unknown `v` maps to `der(v)`, known quantities are piecewise constant.
pub fn total_derivative_raw(
    a: &mut Arena,
    e: ExprId,
    unknown: &[bool],
    _vars: &[Variable],
) -> ExprId {
    let mut memo = FxHashMap::default();
    let mut seed = |ar: &mut Arena, n: Node| -> Option<ExprId> {
        match n {
            Node::Var(v) if unknown[v as usize] => Some(ar.der(v)),
            Node::Time => Some(Arena::ONE),
            _ => None,
        }
    };
    a.forward_diff(e, &mut seed, &mut memo)
}

impl Work {
    /// The derivative variable of `v`, creating it if needed.
    pub fn der_var(&mut self, v: VarId) -> VarId {
        if let Some(d) = self.der_of[v as usize] {
            return d;
        }
        let base = &self.vars[v as usize];
        let mut dv = Variable::new(
            format!("der({})", base.name),
            VarKind::Continuous,
            VarType::Real,
        );
        dv.generated = self.base_of[v as usize].is_some() || base.generated;
        if !base.unit.is_empty() {
            dv.unit = format!("{}/s", base.unit);
        }
        dv.is_protected = base.is_protected;
        let d = self.add_var(dv, true);
        self.der_of[v as usize] = Some(d);
        self.base_of[d as usize] = Some(v);
        d
    }

    /// Total time derivative of `e` in terms of derivative variables.
    pub fn time_derivative(&mut self, e: ExprId) -> ExprId {
        let mut memo = FxHashMap::default();
        // Collect the unknowns first so derivative variables can be created up-front.
        let mut vs = Vec::new();
        self.unknowns_in(e, &mut vs);
        vs.retain(|v| !self.invariant.contains(v));
        let map: FxHashMap<VarId, ExprId> = vs
            .into_iter()
            .map(|v| {
                let d = self.der_var(v);
                (v, self.arena.var(d))
            })
            .collect();
        let mut seed = |_: &mut Arena, n: Node| -> Option<ExprId> {
            match n {
                Node::Var(v) => map.get(&v).copied(),
                Node::Time => Some(Arena::ONE),
                _ => None,
            }
        };
        self.arena.forward_diff(e, &mut seed, &mut memo)
    }

    /// Unknown variables of an equation (both sides).
    pub fn eq_unknowns(&self, e: &Eq) -> Vec<VarId> {
        let mut out = Vec::new();
        self.unknowns_in(e.lhs, &mut out);
        self.unknowns_in(e.rhs, &mut out);
        out
    }
}

/// Find unknowns that are fixed by equations over parameters (and other such unknowns)
/// only, like the switching times of a point-to-point path planner. Their derivative is
/// zero, so index reduction does not differentiate the equations that define them.
pub fn find_invariant(w: &mut Work) {
    let mut invariant: FxHashSet<VarId> = FxHashSet::default();
    // A variable the model differentiates itself (an inductor current forced to zero by
    // an open terminal) is left to index reduction.
    let mut differentiated: FxHashSet<VarId> = FxHashSet::default();
    for eq in &w.eqs {
        for root in [eq.lhs, eq.rhs] {
            w.arena.visit_all_leaves(root, &mut |n| {
                if let Node::Der(v) = n {
                    differentiated.insert(v);
                }
            });
        }
    }
    let mut done = vec![false; w.eqs.len()];
    let mut defining = Vec::new();
    loop {
        let mut changed = false;
        for (k, eq) in w.eqs.iter().enumerate() {
            if done[k] {
                continue;
            }
            let mut varying = false;
            let mut free: Option<VarId> = None;
            let mut count = 0;
            for root in [eq.lhs, eq.rhs] {
                w.arena.visit_all_leaves(root, &mut |n| match n {
                    Node::Var(v) => {
                        let var = &w.vars[v as usize];
                        if matches!(var.kind, VarKind::Parameter | VarKind::Constant)
                            || invariant.contains(&v)
                        {
                            return;
                        }
                        if !w.unknown[v as usize]
                            || !matches!(var.kind, VarKind::Continuous | VarKind::Discrete)
                            || w.when_assigned.contains(&v)
                            || differentiated.contains(&v)
                        {
                            varying = true;
                        } else if free != Some(v) {
                            free = Some(v);
                            count += 1;
                        }
                    }
                    Node::Const(_) => {}
                    _ => varying = true,
                });
            }
            if varying || count > 1 {
                continue;
            }
            done[k] = true;
            if let Some(v) = free {
                invariant.insert(v);
                defining.push(k);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    w.invariant = invariant;
    w.invariant_eqs = defining.into_iter().collect();
}

/// Replace every `der(v)` leaf by a derivative variable.
pub fn create_derivative_vars(w: &mut Work) {
    // Discover all der() leaves first.
    let mut ders: Vec<VarId> = Vec::new();
    let mut seen = FxHashSet::default();
    let mut roots: Vec<ExprId> = Vec::new();
    for e in w.eqs.iter().chain(w.init_eqs.iter()) {
        roots.push(e.lhs);
        roots.push(e.rhs);
    }
    for wc in &w.whens {
        for (c, acts) in &wc.branches {
            roots.push(*c);
            for a in acts {
                match a {
                    WhenAction::Assign(_, e)
                    | WhenAction::Reinit(_, e)
                    | WhenAction::Assert(e, _) => roots.push(*e),
                    WhenAction::Terminate(_) => {}
                }
            }
        }
    }
    for (c, _) in &w.asserts {
        roots.push(*c);
    }
    for r in &w.arena.relations {
        roots.push(r.lhs);
        roots.push(r.rhs);
    }
    for &r in &roots {
        w.arena.visit_leaves(r, &mut |n| {
            if let Node::Der(v) = n
                && seen.insert(v)
            {
                ders.push(v);
            }
        });
    }
    ders.sort();
    let mut map: FxHashMap<VarId, ExprId> = FxHashMap::default();
    for v in ders {
        if w.unknown[v as usize] {
            let d = w.der_var(v);
            let e = w.arena.var(d);
            map.insert(v, e);
        } else {
            map.insert(v, Arena::ZERO);
        }
    }
    if map.is_empty() {
        return;
    }
    let mut memo = FxHashMap::default();
    let mut f = |_: &mut Arena, n: Node| -> Option<ExprId> {
        match n {
            Node::Der(v) => map.get(&v).copied(),
            _ => None,
        }
    };
    for eq in w.eqs.iter_mut().chain(w.init_eqs.iter_mut()) {
        eq.lhs = w.arena.rewrite(eq.lhs, &mut memo, &mut f);
        eq.rhs = w.arena.rewrite(eq.rhs, &mut memo, &mut f);
    }
    for wc in w.whens.iter_mut() {
        for (c, acts) in wc.branches.iter_mut() {
            *c = w.arena.rewrite(*c, &mut memo, &mut f);
            for a in acts.iter_mut() {
                match a {
                    WhenAction::Assign(_, e)
                    | WhenAction::Reinit(_, e)
                    | WhenAction::Assert(e, _) => {
                        *e = w.arena.rewrite(*e, &mut memo, &mut f);
                    }
                    WhenAction::Terminate(_) => {}
                }
            }
        }
    }
    for (c, _) in w.asserts.iter_mut() {
        *c = w.arena.rewrite(*c, &mut memo, &mut f);
    }
    for r in 0..w.arena.relations.len() {
        let rel = w.arena.relations[r];
        let l = w.arena.rewrite(rel.lhs, &mut memo, &mut f);
        let rr = w.arena.rewrite(rel.rhs, &mut memo, &mut f);
        w.arena.relations[r].lhs = l;
        w.arena.relations[r].rhs = rr;
    }
}

/// Check that the number of equations matches the number of unknowns
/// (unknowns whose derivative appears count as determined by integration).
fn check_balance(w: &Work) -> Result<()> {
    let n_unknown = w.unknown.iter().filter(|&&u| u).count();
    let n_diff = (0..w.vars.len())
        .filter(|&v| w.unknown[v] && w.der_of[v].is_some())
        .count();
    let n_eq = w.eqs.len();
    let need = n_unknown - n_diff;
    if n_eq != need {
        let what = if n_eq > need {
            "overdetermined"
        } else {
            "underdetermined"
        };
        bail!(
            "model is {what}: {n_eq} equations for {need} unknowns ({n_unknown} variables, {n_diff} of them appear differentiated){}",
            unmatched_hint(w)
        );
    }
    Ok(())
}

/// Name a few unknowns (or equations) left over by a maximum matching, the way a
/// structural error is usually tracked down.
fn unmatched_hint(w: &Work) -> String {
    let adj: Vec<Vec<u32>> = w.eqs.iter().map(|e| w.eq_unknowns(e)).collect();
    let mut eq_match = vec![NONE; w.eqs.len()];
    let mut var_match = vec![NONE; w.vars.len()];
    let mut visited_eq = vec![0u32; w.eqs.len()];
    let mut visited_var = vec![0u32; w.vars.len()];
    let active = |v: u32| w.der_of[v as usize].is_none();
    let mut free_eqs = Vec::new();
    for e in 0..w.eqs.len() {
        let stamp = e as u32 + 1;
        if !augment(
            e,
            &adj,
            &mut eq_match,
            &mut var_match,
            &mut visited_eq,
            &mut visited_var,
            stamp,
            &active,
        ) {
            free_eqs.push(e);
        }
    }
    let free_vars: Vec<&str> = (0..w.vars.len())
        .filter(|&v| w.unknown[v] && w.der_of[v].is_none() && var_match[v] == NONE)
        .map(|v| w.name(v as VarId))
        .collect();
    let list = |items: Vec<String>| {
        let n = items.len();
        let mut s = items.into_iter().take(8).collect::<Vec<_>>().join(", ");
        if n > 8 {
            s.push_str(&format!(" and {} more", n - 8));
        }
        s
    };
    let mut out = String::new();
    if !free_vars.is_empty() {
        out.push_str(&format!(
            "; no equation for: {}",
            list(free_vars.iter().map(|s| s.to_string()).collect())
        ));
    }
    if !free_eqs.is_empty() {
        out.push_str(&format!(
            "; redundant: {}",
            list(free_eqs.iter().map(|&e| w.eq_text(&w.eqs[e])).collect())
        ));
    }
    out
}

/// Pantelides' algorithm. Differentiated equations are appended to `w.eqs`
/// with `parent`/`derived` links; new derivative variables are created on demand.
pub fn pantelides(w: &mut Work) -> Result<()> {
    check_balance(w)?;
    prefer_state_change(w);
    let n_orig = w.eqs.len();
    // Time-invariant unknowns and their defining equations are solved like parameters:
    // they take no part in index reduction.
    let invariant = w.invariant.clone();
    let invariant_eqs = w.invariant_eqs.clone();
    let unknowns = |w: &Work, e: &Eq| -> Vec<u32> {
        let mut u = w.eq_unknowns(e);
        u.retain(|v| !invariant.contains(v));
        u
    };
    let mut adj: Vec<Vec<u32>> = w
        .eqs
        .iter()
        .enumerate()
        .map(|(k, e)| {
            if invariant_eqs.contains(&k) {
                Vec::new()
            } else {
                unknowns(w, e)
            }
        })
        .collect();
    let mut eq_match: Vec<u32> = vec![NONE; w.eqs.len()];
    let mut var_match: Vec<u32> = vec![NONE; w.vars.len()];
    let mut visited_eq: Vec<u32> = vec![0; w.eqs.len()];
    let mut visited_var: Vec<u32> = vec![0; w.vars.len()];
    let mut stamp = 0u32;
    let max_eqs = 8 * n_orig.max(16) + 1000;
    for k in 0..n_orig {
        if invariant_eqs.contains(&k) || w.eqs[k].parent.is_some() {
            continue;
        }
        // Equations already differentiated (by the state change) start at their
        // highest derivative.
        let mut i = k;
        while let Some(d) = w.eqs[i].derived {
            i = d;
        }
        let mut level = 0;
        loop {
            stamp += 1;
            let der_of = &w.der_of;
            let active = |v: u32| der_of[v as usize].is_none();
            let (mut colored_eqs, mut colored_vars) = (Vec::new(), Vec::new());
            if augment_traced(
                i,
                &adj,
                &mut eq_match,
                &mut var_match,
                &mut visited_eq,
                &mut visited_var,
                stamp,
                &active,
                Some((&mut colored_eqs, &mut colored_vars)),
            ) {
                break;
            }
            level += 1;
            colored_vars.retain(|&v| active(v));
            colored_vars.sort_unstable();
            colored_eqs.sort_unstable();
            let no_unknowns = colored_eqs.iter().all(|&e| adj[e].is_empty());
            if no_unknowns || level > 10 || w.eqs.len() > max_eqs {
                let mut msg = String::from(
                    "structurally singular system; the following equations cannot be solved:\n",
                );
                for &e in colored_eqs.iter().take(20) {
                    msg.push_str("  ");
                    msg.push_str(&w.eq_text(&w.eqs[e]));
                    msg.push('\n');
                }
                if no_unknowns {
                    msg.push_str("(no unknown is left for them: too many equations or a redundant constraint)");
                } else {
                    let names: Vec<&str> = colored_vars.iter().map(|&v| w.name(v)).collect();
                    msg.push_str(&format!("involving: {}", names.join(", ")));
                }
                bail!(msg);
            }
            // Differentiate the colored variables (create derivative vars).
            for &v in &colored_vars {
                w.der_var(v);
            }
            // Differentiate the colored equations.
            for &l in &colored_eqs {
                let eq = w.eqs[l];
                let lhs = w.time_derivative(eq.lhs);
                let rhs = w.time_derivative(eq.rhs);
                let new = Eq {
                    lhs,
                    rhs,
                    origin: eq.origin,
                    parent: Some(l),
                    derived: None,
                };
                w.eqs.push(new);
                let ni = w.eqs.len() - 1;
                w.eqs[l].derived = Some(ni);
                adj.push(unknowns(w, &new));
                eq_match.push(NONE);
                visited_eq.push(0);
            }
            var_match.resize(w.vars.len(), NONE);
            visited_var.resize(w.vars.len(), 0);
            // Transfer the matching to the derivatives.
            for &v in &colored_vars {
                let e = var_match[v as usize];
                if e != NONE {
                    let dv = w.der_of[v as usize].unwrap();
                    let de = w.eqs[e as usize].derived.unwrap();
                    var_match[dv as usize] = de as u32;
                    eq_match[de] = dv;
                }
            }
            i = w.eqs[i].derived.unwrap();
        }
    }
    Ok(())
}

/// Make unknowns with `stateSelect = prefer/always` that do not appear differentiated
/// candidates for states, as Dymola does: the algebraic equations that compute them from
/// the natural states (`m = V*d(p, h)`, `U = m*u(p, h)` for a medium volume with mass and
/// energy balances) are differentiated, so the dummy-derivative selection can pick the
/// preferred variables (`p`, `h`) as states and turn the natural ones into algebraic
/// variables. The states then enter the medium functions directly instead of through a
/// nonlinear system solved at every step.
fn prefer_state_change(w: &mut Work) {
    let nv = w.vars.len();
    let preferred: Vec<VarId> = (0..nv as VarId)
        .filter(|&v| {
            let x = &w.vars[v as usize];
            w.unknown[v as usize]
                && w.der_of[v as usize].is_none()
                && w.base_of[v as usize].is_none()
                && !w.invariant.contains(&v)
                && x.ty == VarType::Real
                && x.kind == VarKind::Continuous
                && matches!(x.state_select, StateSelect::Prefer | StateSelect::Always)
        })
        .collect();
    if preferred.is_empty() {
        return;
    }
    // Algebraic part: equations without derivative variables, over unknowns that are
    // neither natural states nor derivatives.
    let is_alg = |w: &Work, v: VarId| {
        w.der_of[v as usize].is_none()
            && w.base_of[v as usize].is_none()
            && !w.invariant.contains(&v)
    };
    let mut alg_eqs: Vec<usize> = Vec::new();
    let mut adj: Vec<Vec<u32>> = Vec::new();
    let mut touches_state: Vec<bool> = Vec::new();
    for (k, e) in w.eqs.iter().enumerate() {
        if w.invariant_eqs.contains(&k) || e.derived.is_some() || e.parent.is_some() {
            continue;
        }
        let u = w.eq_unknowns(e);
        if u.iter().any(|&v| w.base_of[v as usize].is_some()) {
            continue;
        }
        let a: Vec<u32> = u.iter().copied().filter(|&v| is_alg(w, v)).collect();
        if a.is_empty() {
            // a constraint between states: index reduction proper handles it
            return;
        }
        touches_state.push(u.iter().any(|&v| w.der_of[v as usize].is_some()));
        alg_eqs.push(k);
        adj.push(a);
    }
    let mut eq_match = vec![NONE; alg_eqs.len()];
    let mut var_match = vec![NONE; nv];
    let mut visited_eq = vec![0u32; alg_eqs.len()];
    let mut visited_var = vec![0u32; nv];
    let active = |_: u32| true;
    for e in 0..alg_eqs.len() {
        augment(
            e,
            &adj,
            &mut eq_match,
            &mut var_match,
            &mut visited_eq,
            &mut visited_var,
            e as u32 + 1,
            &active,
        );
    }
    // For each preferred variable, the equations it depends on (transitively).
    let mut chosen: Vec<bool> = vec![false; alg_eqs.len()];
    for &p in &preferred {
        if var_match[p as usize] == NONE {
            continue;
        }
        let mut seen_eq: FxHashSet<u32> = FxHashSet::default();
        let mut stack = vec![p];
        let mut seen_var: FxHashSet<VarId> = FxHashSet::default();
        seen_var.insert(p);
        let mut reaches_state = false;
        let mut complete = true;
        while let Some(v) = stack.pop() {
            let e = var_match[v as usize];
            if e == NONE {
                complete = false;
                break;
            }
            if !seen_eq.insert(e) {
                continue;
            }
            reaches_state |= touches_state[e as usize];
            for &u in &adj[e as usize] {
                if seen_var.insert(u) {
                    stack.push(u);
                }
            }
        }
        if complete && reaches_state {
            for e in seen_eq {
                chosen[e as usize] = true;
            }
        }
    }
    let todo: Vec<usize> = (0..alg_eqs.len())
        .filter(|&e| chosen[e])
        .map(|e| alg_eqs[e])
        .collect();
    for l in todo {
        let eq = w.eqs[l];
        for v in w.eq_unknowns(&eq) {
            if !w.invariant.contains(&v) {
                w.der_var(v);
            }
        }
        let lhs = w.time_derivative(eq.lhs);
        let rhs = w.time_derivative(eq.rhs);
        w.eqs.push(Eq {
            lhs,
            rhs,
            origin: eq.origin,
            parent: Some(l),
            derived: None,
        });
        let ni = w.eqs.len() - 1;
        w.eqs[l].derived = Some(ni);
    }
}

/// Structural pseudo-random perturbation used for numeric rank checks.
fn perturb(v: VarId) -> f64 {
    let mut x = (v as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    x ^= x >> 29;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 32;
    0.05 + 0.1 * ((x % 10_000) as f64 / 10_000.0)
}

/// Preference for a derivative variable to become a dummy (higher = more willing).
fn dummy_preference(w: &Work, z: VarId) -> i64 {
    // Making `z` a dummy derivative removes its base from the state vector.
    let base = w.base_of[z as usize].unwrap();
    let var = &w.vars[base as usize];
    let mut s = match var.state_select {
        StateSelect::Never => 4000,
        StateSelect::Avoid => 2000,
        StateSelect::Default => 0,
        StateSelect::Prefer => -2000,
        StateSelect::Always => -4000,
    };
    // Higher derivative order bases are less natural states.
    let mut order = 0;
    let mut b = base;
    while let Some(p) = w.base_of[b as usize] {
        order += 1;
        b = p;
    }
    s += 100 * order;
    if var.generated {
        s += 50;
    }
    if var.fixed == Some(true) {
        s -= 20;
    }
    // Later declared variables first (mimics Dymola's tie-breaking).
    s * 1_000_000 + base as i64
}

/// Select the states with the dummy-derivative method. Returns the state variables.
pub fn select_states(w: &mut Work) -> Result<Vec<VarId>> {
    // Chains of differentiated equations.
    let roots: Vec<usize> = (0..w.eqs.len())
        .filter(|&e| w.eqs[e].parent.is_none())
        .collect();
    let mut chains: Vec<Vec<usize>> = Vec::new();
    for &r in &roots {
        let mut c = vec![r];
        let mut e = r;
        while let Some(d) = w.eqs[e].derived {
            c.push(d);
            e = d;
        }
        if c.len() > 1 {
            chains.push(c);
        }
    }
    if !chains.is_empty() {
        // Evaluation points: start values, and start values + perturbation (to break ties
        // where the start point is structurally degenerate, e.g. all zeros).
        let mut point0: Vec<f64> = vec![0.0; w.vars.len()];
        let mut point1: Vec<f64> = vec![0.0; w.vars.len()];
        for v in 0..w.vars.len() as VarId {
            if w.unknown[v as usize] {
                let s = w.start_value(v);
                point0[v as usize] = s;
                point1[v as usize] = s + perturb(v) * (1.0 + s.abs());
            } else {
                point0[v as usize] = w.param_values[v as usize];
                point1[v as usize] = w.param_values[v as usize];
            }
        }
        let max_c = chains.iter().map(|c| c.len() - 1).max().unwrap();
        // Candidates for step 1: highest derivatives that are derivative variables.
        let mut candidates: Option<Vec<VarId>> = None;
        for k in 1..=max_c {
            let g: Vec<usize> = chains
                .iter()
                .filter(|c| c.len() - 1 >= k)
                .map(|c| c[c.len() - k])
                .collect();
            if g.is_empty() {
                break;
            }
            let cols: Vec<VarId> = match &candidates {
                None => {
                    let mut set = Vec::new();
                    let mut seen = FxHashSet::default();
                    for &e in &g {
                        for v in w.eq_unknowns(&w.eqs[e]) {
                            if w.base_of[v as usize].is_some()
                                && w.der_of[v as usize].is_none()
                                && seen.insert(v)
                            {
                                set.push(v);
                            }
                        }
                    }
                    set
                }
                Some(prev) => prev
                    .iter()
                    .filter_map(|&z| w.base_of[z as usize])
                    .filter(|&b| w.base_of[b as usize].is_some())
                    .collect(),
            };
            // Order columns by preference (most willing to become dummy first).
            let mut cols = cols;
            cols.sort_by_key(|&z| -dummy_preference(w, z));
            let tiers: Vec<i64> = cols.iter().map(|&z| dummy_tier(w, z)).collect();
            // Jacobian H = dg/dz evaluated numerically, stored sparsely: only columns that
            // occur in an equation can have a nonzero derivative.
            let col_of: FxHashMap<VarId, usize> =
                cols.iter().enumerate().map(|(i, &z)| (z, i)).collect();
            let mut entries: Vec<Vec<(usize, f64)>> = vec![Vec::new(); g.len()];
            for (ri, &e) in g.iter().enumerate() {
                let eq = w.eqs[e];
                let r = w.arena.sub(eq.lhs, eq.rhs);
                let mut occ: Vec<usize> = w
                    .eq_unknowns(&eq)
                    .into_iter()
                    .filter_map(|v| col_of.get(&v).copied())
                    .collect();
                occ.sort_unstable();
                occ.dedup();
                for ci in occ {
                    let z = cols[ci];
                    let mut memo = FxHashMap::default();
                    let d = w.arena.diff(r, Node::Var(z), &mut memo);
                    if w.arena.is_zero(d) {
                        continue;
                    }
                    let mut val = 0.0;
                    for (pt, wgt) in [(&point0, 1.0), (&point1, 1e-4)] {
                        let vals = eval::Values {
                            var: &|v| pt[v as usize],
                            pre: &|v| pt[v as usize],
                            time: 0.0,
                            initial: false,
                            rel: None,
                        };
                        let x = eval::eval(&w.arena, d, &vals, &mut FxHashMap::default());
                        if x.is_finite() {
                            val += wgt * x;
                        }
                    }
                    entries[ri].push((ci, val));
                }
            }
            // H is block diagonal after permutation (one block per group of equations that
            // share candidate columns, e.g. one per spring in a chain); select per block so
            // the cost stays linear in the number of blocks.
            let nr = g.len();
            let mut parent: Vec<usize> = (0..nr + cols.len()).collect();
            fn find(p: &mut [usize], mut x: usize) -> usize {
                while p[x] != x {
                    p[x] = p[p[x]];
                    x = p[x];
                }
                x
            }
            for (ri, row) in entries.iter().enumerate() {
                for &(ci, _) in row {
                    let (a, b) = (find(&mut parent, ri), find(&mut parent, nr + ci));
                    if a != b {
                        parent[a] = b;
                    }
                }
            }
            let mut block_rows: FxHashMap<usize, Vec<usize>> = FxHashMap::default();
            let mut block_cols: FxHashMap<usize, Vec<usize>> = FxHashMap::default();
            for ri in 0..nr {
                let r = find(&mut parent, ri);
                block_rows.entry(r).or_default().push(ri);
            }
            for ci in 0..cols.len() {
                let r = find(&mut parent, nr + ci);
                block_cols.entry(r).or_default().push(ci);
            }
            let mut chosen: Vec<usize> = Vec::new();
            let mut keys: Vec<usize> = block_rows.keys().copied().collect();
            keys.sort_unstable();
            for key in keys {
                let rows = &block_rows[&key];
                let bcols = block_cols.get(&key).cloned().unwrap_or_default();
                let local: FxHashMap<usize, usize> =
                    bcols.iter().enumerate().map(|(i, &c)| (c, i)).collect();
                let btiers: Vec<i64> = bcols.iter().map(|&c| tiers[c]).collect();
                let pick = if rows.len() <= DENSE_SELECT_MAX {
                    let mut h = vec![vec![0.0; bcols.len()]; rows.len()];
                    for (lr, &ri) in rows.iter().enumerate() {
                        for &(ci, v) in &entries[ri] {
                            h[lr][local[&ci]] = v;
                        }
                    }
                    select_columns(&h, &btiers, rows.len())
                } else {
                    let h: Vec<Vec<(usize, f64)>> = rows
                        .iter()
                        .map(|&ri| entries[ri].iter().map(|&(ci, v)| (local[&ci], v)).collect())
                        .collect();
                    select_columns_sparse(&h, bcols.len(), rows.len())
                };
                if pick.len() < rows.len() {
                    let names: Vec<&str> = bcols.iter().map(|&c| w.name(cols[c])).collect();
                    let eqs: Vec<String> = rows
                        .iter()
                        .take(3)
                        .map(|&ri| w.eq_text(&w.eqs[g[ri]]))
                        .collect();
                    bail!(
                        "index reduction failed: cannot select {} dummy derivatives among [{}] (singular Jacobian at start values) for: {}",
                        rows.len(),
                        names.join(", "),
                        eqs.join("; ")
                    );
                }
                chosen.extend(pick.into_iter().map(|l| bcols[l]));
            }
            chosen.sort_unstable();
            let sel: Vec<VarId> = chosen.iter().map(|&c| cols[c]).collect();
            for &z in &sel {
                w.dummy.insert(z);
            }
            candidates = Some(sel);
        }
    }
    let mut states = Vec::new();
    for v in 0..w.vars.len() as VarId {
        if !w.unknown[v as usize] {
            continue;
        }
        if let Some(d) = w.der_of[v as usize]
            && !w.dummy.contains(&d)
        {
            states.push(v);
        }
    }
    for &s in &states {
        if w.vars[s as usize].state_select == StateSelect::Never {
            w.warnings.push(format!(
                "'{}' has stateSelect=never but had to be selected as a state",
                w.name(s)
            ));
        }
    }
    let _ = &w.dummy;
    Ok(states)
}

/// Blocks with more rows than this use the sparse greedy selection instead of the dense
/// Gram-Schmidt one (which is cubic in the block size).
const DENSE_SELECT_MAX: usize = 64;

/// Sparse column selection for large blocks: Gaussian elimination over the columns in
/// preference order, accepting a column when its reduced entries are not negligible and
/// pivoting on the row with the largest one. This is the greedy matroid basis, so the
/// preferred columns are taken whenever they are independent of the ones already taken.
fn select_columns_sparse(h: &[Vec<(usize, f64)>], ncols: usize, need: usize) -> Vec<usize> {
    let mut rows: Vec<FxHashMap<usize, f64>> =
        h.iter().map(|r| r.iter().copied().collect()).collect();
    let mut col_rows: Vec<FxHashSet<usize>> = vec![FxHashSet::default(); ncols];
    let mut norm0 = vec![0.0f64; ncols];
    for (ri, r) in h.iter().enumerate() {
        for &(c, v) in r {
            col_rows[c].insert(ri);
            norm0[c] = norm0[c].max(v.abs());
        }
    }
    let scale = norm0.iter().fold(0.0f64, |m, &x| m.max(x)).max(1e-300);
    let mut done = vec![false; rows.len()];
    let mut chosen = Vec::new();
    for c in 0..ncols {
        if chosen.len() == need {
            break;
        }
        if norm0[c] <= 1e-10 * scale {
            continue;
        }
        let mut best: Option<(usize, f64)> = None;
        for &r in &col_rows[c] {
            if done[r] {
                continue;
            }
            let v = rows[r].get(&c).copied().unwrap_or(0.0).abs();
            if v > 1e-8 * norm0[c] && best.is_none_or(|(br, bv)| v > bv || (v == bv && r < br)) {
                best = Some((r, v));
            }
        }
        let Some((p, _)) = best else { continue };
        done[p] = true;
        chosen.push(c);
        let prow: Vec<(usize, f64)> = rows[p].iter().map(|(&k, &v)| (k, v)).collect();
        let pv = rows[p][&c];
        let mut targets: Vec<usize> = col_rows[c].iter().copied().filter(|&r| !done[r]).collect();
        targets.sort_unstable();
        for r in targets {
            let Some(&rv) = rows[r].get(&c) else { continue };
            let f = rv / pv;
            for &(k, v) in &prow {
                let e = rows[r].entry(k).or_insert(0.0);
                *e -= f * v;
                col_rows[k].insert(r);
            }
            rows[r].remove(&c);
        }
    }
    chosen
}

/// Coarse preference tier (stateSelect of the base variable); pivoting happens within a tier.
fn dummy_tier(w: &Work, z: VarId) -> i64 {
    let base = w.base_of[z as usize].unwrap();
    match w.vars[base as usize].state_select {
        StateSelect::Never => 4,
        StateSelect::Avoid => 3,
        StateSelect::Default => 2,
        StateSelect::Prefer => 1,
        StateSelect::Always => 0,
    }
}

/// Column selection by Gram-Schmidt with column pivoting: columns are taken tier by tier
/// (`tiers` is non-increasing along the column order); within a tier the column with the
/// largest component orthogonal to the already selected ones is chosen first, so the
/// dummy-derivative Jacobian is as well conditioned as possible at the start point.
fn select_columns(h: &[Vec<f64>], tiers: &[i64], need: usize) -> Vec<usize> {
    let rows = h.len();
    if rows == 0 {
        return Vec::new();
    }
    let ncols = h[0].len();
    let scale = h
        .iter()
        .flatten()
        .fold(0.0f64, |m, x| m.max(x.abs()))
        .max(1e-300);
    let mut cols: Vec<Vec<f64>> = (0..ncols)
        .map(|c| (0..rows).map(|r| h[r][c]).collect())
        .collect();
    let norm0: Vec<f64> = cols
        .iter()
        .map(|c| c.iter().map(|x| x * x).sum::<f64>().sqrt())
        .collect();
    let mut taken = vec![false; ncols];
    let mut chosen = Vec::new();
    let mut start = 0;
    while start < ncols && chosen.len() < need {
        let mut end = start;
        while end < ncols && tiers[end] == tiers[start] {
            end += 1;
        }
        loop {
            if chosen.len() == need {
                break;
            }
            let mut best = None;
            let mut best_n = 0.0;
            for c in start..end {
                if taken[c] || norm0[c] <= 1e-10 * scale {
                    continue;
                }
                let n = cols[c].iter().map(|x| x * x).sum::<f64>().sqrt();
                if n > 1e-8 * norm0[c] && n > best_n * (1.0 + 1e-9) {
                    best_n = n;
                    best = Some(c);
                }
            }
            let Some(c) = best else { break };
            taken[c] = true;
            chosen.push(c);
            let q: Vec<f64> = cols[c].iter().map(|x| x / best_n).collect();
            for (k, col) in cols.iter_mut().enumerate() {
                if taken[k] {
                    continue;
                }
                let dot: f64 = col.iter().zip(&q).map(|(x, y)| x * y).sum();
                for (x, y) in col.iter_mut().zip(&q) {
                    *x -= dot * y;
                }
            }
        }
        start = end;
    }
    chosen
}
