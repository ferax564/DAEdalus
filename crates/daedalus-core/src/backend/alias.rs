//! Alias and trivial-equation elimination.
//!
//! `a = b`, `a = -b`, `a + b = 0`, `a - b = 0` collapse variables into one
//! representative; `a = f(parameters)` turns `a` into a parameter-dependent
//! constant. Connector-heavy models typically lose well over half of their
//! equations here, before any structural analysis runs.

use super::{Alias, AliasKind, Work};
use crate::expr::{Arena, ExprId, Node, VarId};
use crate::flat::{StateSelect, VarKind, WhenAction};
use rustc_hash::{FxHashMap, FxHashSet};

enum Found {
    Pair(VarId, VarId, f64),
    Bound(VarId, ExprId),
}

fn var_of(a: &Arena, e: ExprId) -> Option<(VarId, f64)> {
    match a.node(e) {
        Node::Var(v) => Some((v, 1.0)),
        Node::Neg(x) => match a.node(x) {
            Node::Var(v) => Some((v, -1.0)),
            _ => None,
        },
        _ => None,
    }
}

impl Work {
    /// Known-only expression (no unknowns, no time, no derivatives)?
    fn is_known_expr(&self, e: ExprId) -> bool {
        let mut ok = true;
        self.arena.visit_leaves(e, &mut |n| match n {
            Node::Var(v) => {
                if self.unknown[v as usize] {
                    ok = false;
                }
            }
            Node::Der(_) | Node::Time | Node::Pre(_) => ok = false,
            _ => {}
        });
        // Relations make the value piecewise constant in time; keep them as equations.
        ok && !self.has_relation(e)
    }

    fn has_relation(&self, e: ExprId) -> bool {
        let mut stack = vec![e];
        let mut seen = FxHashSet::default();
        let mut ch = Vec::new();
        while let Some(x) = stack.pop() {
            if !seen.insert(x) {
                continue;
            }
            if matches!(
                self.arena.node(x),
                Node::Rel(_) | Node::Initial | Node::Terminal
            ) {
                return true;
            }
            ch.clear();
            self.arena.children(x, &mut ch);
            stack.extend(ch.iter().copied());
        }
        false
    }

    fn classify(&self, lhs: ExprId, rhs: ExprId) -> Option<Found> {
        let a = &self.arena;
        let unk = |v: VarId| self.unknown[v as usize];
        // var = var
        if let (Some((x, sx)), Some((y, sy))) = (var_of(a, lhs), var_of(a, rhs))
            && unk(x)
            && unk(y)
            && x != y
        {
            return self.compatible(x, y).then_some(Found::Pair(x, y, sx * sy));
        }
        // a + b = 0, a - b = 0 (either side zero)
        let (expr, other) = if a.is_zero(rhs) {
            (lhs, rhs)
        } else if a.is_zero(lhs) {
            (rhs, lhs)
        } else {
            (lhs, rhs)
        };
        if a.is_zero(other) {
            match a.node(expr) {
                Node::Add(p, q) => {
                    if let (Some((x, sx)), Some((y, sy))) = (var_of(a, p), var_of(a, q))
                        && unk(x)
                        && unk(y)
                        && x != y
                    {
                        // sx x + sy y = 0  ->  x = -(sy/sx) y
                        return self.compatible(x, y).then_some(Found::Pair(x, y, -sy * sx));
                    }
                }
                Node::Sub(p, q) => {
                    if let (Some((x, sx)), Some((y, sy))) = (var_of(a, p), var_of(a, q))
                        && unk(x)
                        && unk(y)
                        && x != y
                    {
                        return self.compatible(x, y).then_some(Found::Pair(x, y, sy * sx));
                    }
                }
                _ => {}
            }
        }
        // var = known expression
        if let Some((x, sx)) = var_of(a, lhs)
            && unk(x)
            && self.is_known_expr(rhs)
        {
            return Some(Found::Bound(
                x,
                if sx < 0.0 { usize::MAX as ExprId } else { rhs },
            ));
        }
        if let Some((x, sx)) = var_of(a, rhs)
            && unk(x)
            && self.is_known_expr(lhs)
        {
            return Some(Found::Bound(
                x,
                if sx < 0.0 { usize::MAX as ExprId } else { lhs },
            ));
        }
        None
    }

    fn compatible(&self, x: VarId, y: VarId) -> bool {
        let a = &self.vars[x as usize];
        let b = &self.vars[y as usize];
        a.ty == b.ty && a.kind == b.kind
    }
}

/// Run alias elimination to a fixed point. Returns the number of removed equations.
pub fn eliminate(w: &mut Work) -> usize {
    let mut removed_total = 0;
    // Variables appearing under der() should become representatives.
    let mut differentiated: FxHashSet<VarId> = FxHashSet::default();
    for e in w.eqs.iter().chain(w.init_eqs.iter()) {
        for x in [e.lhs, e.rhs] {
            w.arena.visit_leaves(x, &mut |n| {
                if let Node::Der(v) = n {
                    differentiated.insert(v);
                }
            });
        }
    }
    for _round in 0..20 {
        // parent pointer with sign: var = sign * parent
        let mut parent: FxHashMap<VarId, (VarId, f64)> = FxHashMap::default();
        fn find(parent: &mut FxHashMap<VarId, (VarId, f64)>, v: VarId) -> (VarId, f64) {
            let mut cur = v;
            let mut sign = 1.0;
            let mut path = Vec::new();
            while let Some(&(p, s)) = parent.get(&cur) {
                if p == cur {
                    break;
                }
                path.push(cur);
                sign *= s;
                cur = p;
            }
            // compress
            let root = cur;
            let mut acc = sign;
            for node in path {
                let (_, s) = parent[&node];
                parent.insert(node, (root, acc));
                acc *= s; // s applied: acc for next = acc / s, signs are +-1 so * works
            }
            (root, sign)
        }
        let mut bound: FxHashMap<VarId, ExprId> = FxHashMap::default();
        let mut used = vec![false; w.eqs.len()];
        let mut any = false;
        for (i, eq) in w.eqs.iter().enumerate() {
            match w.classify(eq.lhs, eq.rhs) {
                Some(Found::Pair(x, y, s)) => {
                    if bound.contains_key(&x) || bound.contains_key(&y) {
                        continue;
                    }
                    let (rx, sx) = find(&mut parent, x);
                    let (ry, sy) = find(&mut parent, y);
                    if rx == ry {
                        continue; // redundant cycle: keep as equation
                    }
                    // x = s*y ; x = sx*rx ; y = sy*ry  => rx = sx*s*sy*ry
                    parent.insert(rx, (ry, sx * s * sy));
                    used[i] = true;
                    any = true;
                }
                Some(Found::Bound(x, e)) => {
                    if e == usize::MAX as ExprId
                        || bound.contains_key(&x)
                        || parent.contains_key(&x)
                    {
                        continue;
                    }
                    // Don't bind variables that are in an alias class this round.
                    if parent.values().any(|&(p, _)| p == x) {
                        continue;
                    }
                    bound.insert(x, e);
                    used[i] = true;
                    any = true;
                }
                None => {}
            }
        }
        if !any {
            break;
        }
        // Group classes and pick representatives.
        let members: Vec<VarId> = parent.keys().copied().collect();
        let mut classes: FxHashMap<VarId, Vec<(VarId, f64)>> = FxHashMap::default();
        for v in members {
            let (r, s) = find(&mut parent, v);
            classes.entry(r).or_default().push((v, s));
        }
        for (r, list) in classes.iter_mut() {
            if !list.iter().any(|(v, _)| v == r) {
                list.push((*r, 1.0));
            }
        }
        let score = |w: &Work, v: VarId| -> i64 {
            let var = &w.vars[v as usize];
            let mut s = 0i64;
            if differentiated.contains(&v) {
                s += 1000;
            }
            s += match var.state_select {
                StateSelect::Always => 400,
                StateSelect::Prefer => 200,
                StateSelect::Avoid => -200,
                StateSelect::Never => -400,
                StateSelect::Default => 0,
            };
            if var.fixed == Some(true) {
                s += 50;
            }
            if var.start.is_some() {
                s += 10;
            }
            if !var.is_flow {
                s += 5;
            }
            if var.generated {
                s -= 100;
            }
            s -= var.name.matches('.').count() as i64 * 2;
            s
        };
        let mut subst: FxHashMap<VarId, ExprId> = FxHashMap::default();
        for (_, list) in classes {
            let (rep, rep_sign) = *list
                .iter()
                .max_by_key(|(v, _)| (score(w, *v), -(*v as i64)))
                .unwrap();
            // Merge start / fixed attributes into the representative.
            let mut best_start: Option<(VarId, f64)> = None;
            for &(v, s) in &list {
                if v == rep {
                    continue;
                }
                let var = &w.vars[v as usize];
                if var.start.is_some() && (var.fixed == Some(true) || best_start.is_none()) {
                    let rel = s * rep_sign; // v = rel * rep
                    if best_start.is_none_or(|(b, _)| w.vars[b as usize].fixed != Some(true)) {
                        best_start = Some((v, rel));
                    }
                }
            }
            let rv = &w.vars[rep as usize];
            if let Some((v, rel)) = best_start
                && (rv.start.is_none()
                    || (rv.fixed != Some(true) && w.vars[v as usize].fixed == Some(true)))
            {
                let st = w.vars[v as usize].start.unwrap();
                let st = if rel < 0.0 { w.arena.neg(st) } else { st };
                let fixed = w.vars[v as usize].fixed;
                let rvm = &mut w.vars[rep as usize];
                rvm.start = Some(st);
                if fixed == Some(true) {
                    rvm.fixed = Some(true);
                }
            }
            let rep_e = w.arena.var(rep);
            for &(v, s) in &list {
                if v == rep {
                    continue;
                }
                let rel = s * rep_sign;
                let e = if rel < 0.0 { w.arena.neg(rep_e) } else { rep_e };
                subst.insert(v, e);
                w.aliases.push(Alias {
                    var: v,
                    kind: AliasKind::Var { of: rep, sign: rel },
                    expr: e,
                });
                w.unknown[v as usize] = false;
            }
        }
        for (&v, &e) in &bound {
            subst.insert(v, e);
            w.aliases.push(Alias {
                var: v,
                kind: AliasKind::Expr,
                expr: e,
            });
            w.unknown[v as usize] = false;
        }
        // Remove used equations, substitute everywhere.
        let mut keep = Vec::with_capacity(w.eqs.len());
        for (i, eq) in w.eqs.iter().enumerate() {
            if !used[i] {
                keep.push(*eq);
            } else {
                removed_total += 1;
            }
        }
        w.eqs = keep;
        apply_substitution(w, &subst);
        // Drop trivially satisfied equations.
        let mut keep = Vec::with_capacity(w.eqs.len());
        for eq in std::mem::take(&mut w.eqs) {
            if eq.lhs == eq.rhs {
                w.warnings
                    .push(format!("redundant equation removed: {}", w.eq_text(&eq)));
                removed_total += 1;
                continue;
            }
            if let (Some(a), Some(b)) = (w.arena.as_const(eq.lhs), w.arena.as_const(eq.rhs)) {
                if (a - b).abs() > 1e-12 * (1.0 + a.abs()) {
                    w.warnings
                        .push(format!("inconsistent equation {} = {}", a, b));
                    keep.push(eq);
                } else {
                    removed_total += 1;
                }
                continue;
            }
            keep.push(eq);
        }
        w.eqs = keep;
    }
    // Resolve alias chains so each alias expression refers to non-alias variables.
    let alias_map: FxHashMap<VarId, ExprId> = w.aliases.iter().map(|a| (a.var, a.expr)).collect();
    for i in 0..w.aliases.len() {
        let mut e = w.aliases[i].expr;
        for _ in 0..50 {
            let e2 = w.arena.substitute(e, &|n| match n {
                Node::Var(v) => alias_map.get(&v).copied(),
                _ => None,
            });
            if e2 == e {
                break;
            }
            e = e2;
        }
        w.aliases[i].expr = e;
        if let (AliasKind::Var { .. }, Some((v, s))) = (&w.aliases[i].kind, var_of(&w.arena, e)) {
            w.aliases[i].kind = AliasKind::Var { of: v, sign: s };
        } else if !matches!(w.arena.node(e), Node::Var(_)) && var_of(&w.arena, e).is_none() {
            w.aliases[i].kind = AliasKind::Expr;
        }
    }
    removed_total
}

fn pre_of(a: &mut Arena, e: ExprId) -> ExprId {
    a.substitute(e, &|_| None);
    let mut memo = FxHashMap::default();
    a.rewrite(e, &mut memo, &mut |ar, n| match n {
        Node::Var(v) => Some(ar.pre(v)),
        _ => None,
    })
}

pub fn apply_substitution(w: &mut Work, subst: &FxHashMap<VarId, ExprId>) {
    if subst.is_empty() {
        return;
    }
    let mut pre_cache: FxHashMap<VarId, ExprId> = FxHashMap::default();
    for (&v, &e) in subst {
        let p = pre_of(&mut w.arena, e);
        pre_cache.insert(v, p);
    }
    let mut memo = FxHashMap::default();
    let mut der_cache: FxHashMap<VarId, ExprId> = FxHashMap::default();
    for (&v, &e) in subst {
        let d = super::index::total_derivative_raw(&mut w.arena, e, &w.unknown, &w.vars);
        der_cache.insert(v, d);
        let _ = v;
    }
    let mut f = |ar: &mut Arena, n: Node| -> Option<ExprId> {
        match n {
            Node::Var(v) => subst.get(&v).copied(),
            Node::Pre(v) => pre_cache.get(&v).copied(),
            Node::Der(v) => der_cache.get(&v).copied(),
            _ => {
                let _ = ar;
                None
            }
        }
    };
    let mut sub = |ar: &mut Arena, e: ExprId, memo: &mut FxHashMap<ExprId, ExprId>| {
        ar.rewrite(e, memo, &mut f)
    };
    for eq in w.eqs.iter_mut().chain(w.init_eqs.iter_mut()) {
        eq.lhs = sub(&mut w.arena, eq.lhs, &mut memo);
        eq.rhs = sub(&mut w.arena, eq.rhs, &mut memo);
    }
    for wc in w.whens.iter_mut() {
        for (c, acts) in wc.branches.iter_mut() {
            *c = sub(&mut w.arena, *c, &mut memo);
            for a in acts.iter_mut() {
                match a {
                    WhenAction::Assign(_, e)
                    | WhenAction::Reinit(_, e)
                    | WhenAction::Assert(e, _) => {
                        *e = sub(&mut w.arena, *e, &mut memo);
                    }
                    WhenAction::Effect(c, e) => {
                        *c = sub(&mut w.arena, *c, &mut memo);
                        *e = sub(&mut w.arena, *e, &mut memo);
                    }
                    WhenAction::Terminate(_) => {}
                }
                if let WhenAction::Reinit(v, e) = a
                    && let Some(&t) = subst.get(v)
                    && let Some((rep, s)) = var_of(&w.arena, t)
                {
                    *v = rep;
                    if s < 0.0 {
                        *e = w.arena.neg(*e);
                    }
                }
            }
        }
    }
    for (c, _) in w.asserts.iter_mut() {
        *c = sub(&mut w.arena, *c, &mut memo);
    }
    // Relations reference expressions too: rewrite their operands (relations are
    // re-interned, so update via the arena relation table).
    let nrel = w.arena.relations.len();
    for r in 0..nrel {
        let rel = w.arena.relations[r];
        let l = sub(&mut w.arena, rel.lhs, &mut memo);
        let rr = sub(&mut w.arena, rel.rhs, &mut memo);
        w.arena.relations[r].lhs = l;
        w.arena.relations[r].rhs = rr;
    }
    // Start/min/max/nominal attributes may reference aliased variables (rare): leave as is.
    let _ = VarKind::Continuous;
}
