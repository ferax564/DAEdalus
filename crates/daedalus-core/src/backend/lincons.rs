//! Constraints between states hidden by linear equations with integer coefficients.
//!
//! Index reduction (Pantelides) works on the structure of the equations: it differentiates
//! a subset of equations only when it has more equations than algebraic unknowns. A linear
//! combination whose algebraic terms cancel numerically is invisible to it. The star point
//! of a polyphase winding left open gives `sum(i) = 0` by Kirchhoff, and its zero-sequence
//! inductor states `m*i0 = sum(i)`: together `m*i0 = 0`, so `i0` cannot be a state, yet
//! every equation has more unknowns than it needs structurally. Taking `i0` as a state
//! makes the algebraic system of the currents singular.
//!
//! This pass runs a fraction-free Gaussian elimination over the equations that are linear
//! with integer coefficients (connection equations, sums over phases), eliminating every
//! unknown that does not appear differentiated. A combination left with differentiated
//! variables only is a constraint between potential states; when the structure of the
//! equations it combines does not reveal it, it replaces one of them (an invertible change
//! of the equation set), and index reduction then treats it like any other constraint.

use super::{Eq, Work};
use crate::expr::{ExprId, Node, VarId};
use crate::flat::{VarKind, VarType};
use rustc_hash::{FxHashMap, FxHashSet};

/// A linear equation: integer coefficients of unknowns, plus known terms.
struct Lin {
    terms: FxHashMap<VarId, i128>,
    /// Known parts (no unknowns): `(expression, integer factor)`.
    rest: Vec<(ExprId, i128)>,
}

/// Coefficients beyond this are a sign of an elimination that does not belong here.
const LIMIT: i128 = 1 << 60;

impl Work {
    /// An integer-valued coefficient that cannot change during the simulation: a literal,
    /// a constant, or an Integer or final parameter.
    fn int_coef(&self, e: ExprId) -> Option<i128> {
        let v = self.coef_value(e)?;
        (v.is_finite() && v.fract() == 0.0 && v.abs() < 1e15).then_some(v as i128)
    }

    fn coef_value(&self, e: ExprId) -> Option<f64> {
        match self.arena.node(e) {
            Node::Const(_) => self.arena.as_const(e),
            Node::Var(v) => {
                let x = &self.vars[v as usize];
                let fixed = match x.kind {
                    VarKind::Constant => true,
                    VarKind::Parameter => x.ty == VarType::Integer || x.is_final,
                    _ => false,
                };
                (fixed && !self.init_only.contains(&v)).then(|| self.param_values[v as usize])
            }
            Node::Neg(a) => Some(-self.coef_value(a)?),
            Node::Mul(a, b) => Some(self.coef_value(a)? * self.coef_value(b)?),
            Node::Add(a, b) => Some(self.coef_value(a)? + self.coef_value(b)?),
            Node::Sub(a, b) => Some(self.coef_value(a)? - self.coef_value(b)?),
            _ => None,
        }
    }

    /// Whether `e` holds no unknown, derivative or event-dependent part.
    fn is_known_part(&self, e: ExprId) -> bool {
        let mut ok = true;
        self.arena.visit_all_leaves(e, &mut |n| match n {
            Node::Var(v) => ok &= !self.unknown[v as usize],
            Node::Der(_) | Node::Rel(_) | Node::Pre(_) => ok = false,
            _ => {}
        });
        ok
    }

    /// Collect `f * e` into `out`; false when `e` is not linear with integer coefficients.
    fn linearize(&self, e: ExprId, f: i128, out: &mut Lin) -> bool {
        if f.abs() > LIMIT {
            return false;
        }
        match self.arena.node(e) {
            Node::Var(v) if self.unknown[v as usize] => {
                *out.terms.entry(v).or_insert(0) += f;
                true
            }
            Node::Neg(a) => self.linearize(a, -f, out),
            Node::Add(a, b) => self.linearize(a, f, out) && self.linearize(b, f, out),
            Node::Sub(a, b) => self.linearize(a, f, out) && self.linearize(b, -f, out),
            Node::Mul(a, b) => {
                if let Some(c) = self.int_coef(a) {
                    self.linearize(b, f * c, out)
                } else if let Some(c) = self.int_coef(b) {
                    self.linearize(a, f * c, out)
                } else {
                    self.known_rest(e, f, out)
                }
            }
            Node::Div(a, b) => match self.int_coef(b) {
                Some(1) => self.linearize(a, f, out),
                Some(-1) => self.linearize(a, -f, out),
                _ => self.known_rest(e, f, out),
            },
            _ => self.known_rest(e, f, out),
        }
    }

    fn known_rest(&self, e: ExprId, f: i128, out: &mut Lin) -> bool {
        if !self.is_known_part(e) {
            return false;
        }
        if !self.arena.is_zero(e) {
            out.rest.push((e, f));
        }
        true
    }
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a
}

/// One row of the elimination: a linear combination of source equations.
struct Row {
    terms: FxHashMap<VarId, i128>,
    rest: Vec<(ExprId, i128)>,
    /// `(equation index, multiplier)`.
    combo: Vec<(usize, i128)>,
}

impl Row {
    /// `self = a*self - b*other`, then divided by the gcd of everything. None on overflow.
    fn combine(&mut self, a: i128, other: &Row, b: i128) -> Option<()> {
        let mul = |x: i128, y: i128| x.checked_mul(y).filter(|v| v.abs() <= LIMIT);
        for v in self.terms.values_mut() {
            *v = mul(*v, a)?;
        }
        for (_, c) in &mut self.rest {
            *c = mul(*c, a)?;
        }
        for (_, c) in &mut self.combo {
            *c = mul(*c, a)?;
        }
        for (&v, &c) in &other.terms {
            let e = self.terms.entry(v).or_insert(0);
            *e = e.checked_sub(mul(c, b)?)?;
        }
        self.terms.retain(|_, c| *c != 0);
        for &(e, c) in &other.rest {
            self.rest.push((e, -mul(c, b)?));
        }
        for &(k, c) in &other.combo {
            match self.combo.iter_mut().find(|(j, _)| *j == k) {
                Some((_, x)) => *x = x.checked_sub(mul(c, b)?)?,
                None => self.combo.push((k, -mul(c, b)?)),
            }
        }
        self.combo.retain(|&(_, c)| c != 0);
        // Known terms over the same expression add up.
        let mut merged: Vec<(ExprId, i128)> = Vec::new();
        for &(e, c) in &self.rest {
            match merged.iter_mut().find(|(x, _)| *x == e) {
                Some((_, y)) => *y = y.checked_add(c)?,
                None => merged.push((e, c)),
            }
        }
        merged.retain(|&(_, c)| c != 0);
        self.rest = merged;
        let g = self
            .terms
            .values()
            .chain(self.rest.iter().map(|(_, c)| c))
            .chain(self.combo.iter().map(|(_, c)| c))
            .fold(0, |g, &c| gcd(g, c));
        if g > 1 {
            self.terms.values_mut().for_each(|c| *c /= g);
            self.rest.iter_mut().for_each(|(_, c)| *c /= g);
            self.combo.iter_mut().for_each(|(_, c)| *c /= g);
        }
        Some(())
    }
}

/// Whether the equations `eqs` can be matched to distinct unknowns among the
/// non-differentiated ones they contain: then their structure does not show that a
/// combination of them constrains the states, and index reduction would miss it.
fn structurally_hidden(w: &Work, eqs: &[usize], eliminable: &dyn Fn(VarId) -> bool) -> bool {
    let cols: Vec<Vec<VarId>> = eqs
        .iter()
        .map(|&k| {
            let mut u = w.eq_unknowns(&w.eqs[k]);
            u.retain(|&v| eliminable(v));
            u.sort_unstable();
            u.dedup();
            u
        })
        .collect();
    let mut owner: FxHashMap<VarId, usize> = FxHashMap::default();
    fn augment(
        i: usize,
        cols: &[Vec<VarId>],
        owner: &mut FxHashMap<VarId, usize>,
        seen: &mut FxHashSet<VarId>,
    ) -> bool {
        for &v in &cols[i] {
            if !seen.insert(v) {
                continue;
            }
            let free = match owner.get(&v) {
                None => true,
                Some(&j) => augment(j, cols, owner, seen),
            };
            if free {
                owner.insert(v, i);
                return true;
            }
        }
        false
    }
    (0..eqs.len()).all(|i| augment(i, &cols, &mut owner, &mut FxHashSet::default()))
}

/// Find the hidden constraints between potential states and put each in place of one of
/// the equations it combines. Returns the number of equations replaced.
pub fn expose_hidden_constraints(w: &mut Work) -> usize {
    let differentiated = |w: &Work, v: VarId| w.der_of[v as usize].is_some();
    let mut rows: Vec<Row> = Vec::new();
    for (k, eq) in w.eqs.iter().enumerate() {
        if w.invariant_eqs.contains(&k) || eq.derived.is_some() || eq.parent.is_some() {
            continue;
        }
        let mut lin = Lin {
            terms: FxHashMap::default(),
            rest: Vec::new(),
        };
        if !(w.linearize(eq.lhs, 1, &mut lin) && w.linearize(eq.rhs, -1, &mut lin)) {
            continue;
        }
        lin.terms.retain(|_, c| *c != 0);
        // Only equations that involve both kinds can combine into a constraint on states
        // through others; pure algebraic ones are the elimination's pivots.
        if lin.terms.is_empty() {
            continue;
        }
        rows.push(Row {
            terms: lin.terms,
            rest: lin.rest,
            combo: vec![(k, 1)],
        });
    }
    // Rows touching a potential state, else nothing to find.
    if !rows
        .iter()
        .any(|r| r.terms.keys().any(|&v| differentiated(w, v)))
    {
        return 0;
    }
    // Eliminate the non-differentiated unknowns, the sparsest column first.
    let mut active: Vec<bool> = vec![true; rows.len()];
    let mut by_col: FxHashMap<VarId, Vec<usize>> = FxHashMap::default();
    for (i, r) in rows.iter().enumerate() {
        for &v in r.terms.keys() {
            if !differentiated(w, v) {
                by_col.entry(v).or_default().push(i);
            }
        }
    }
    loop {
        // The column with the fewest active rows.
        let mut best: Option<(usize, VarId)> = None;
        for (&v, list) in &by_col {
            let n = list
                .iter()
                .filter(|&&i| active[i] && rows[i].terms.contains_key(&v))
                .count();
            if n > 0 && best.is_none_or(|(m, u)| n < m || (n == m && v < u)) {
                best = Some((n, v));
            }
        }
        let Some((_, c)) = best else { break };
        let mut users: Vec<usize> = by_col[&c]
            .iter()
            .copied()
            .filter(|&i| active[i] && rows[i].terms.contains_key(&c))
            .collect();
        users.sort_unstable();
        users.dedup();
        let p = *users
            .iter()
            .min_by_key(|&&i| (rows[i].terms.len(), i))
            .unwrap();
        active[p] = false;
        let pivot = std::mem::replace(
            &mut rows[p],
            Row {
                terms: FxHashMap::default(),
                rest: Vec::new(),
                combo: Vec::new(),
            },
        );
        let pc = pivot.terms[&c];
        for &i in &users {
            if i == p {
                continue;
            }
            let rc = rows[i].terms[&c];
            let g = gcd(pc, rc);
            if rows[i].combine(pc / g, &pivot, rc / g).is_none() {
                return 0;
            }
            for &v in rows[i].terms.keys() {
                if !differentiated(w, v) {
                    by_col.entry(v).or_default().push(i);
                }
            }
        }
        rows[p] = pivot;
        by_col.remove(&c);
    }
    let eliminable = |v: VarId| w.der_of[v as usize].is_none();
    let mut replaced: FxHashSet<usize> = FxHashSet::default();
    let mut new_eqs: Vec<(usize, Row)> = Vec::new();
    for (i, r) in rows.into_iter().enumerate() {
        if !active[i] || r.terms.is_empty() || r.terms.keys().any(|&v| eliminable(v)) {
            continue;
        }
        let eqs: Vec<usize> = r.combo.iter().map(|&(k, _)| k).collect();
        if eqs.len() < 2 || !structurally_hidden(w, &eqs, &eliminable) {
            continue;
        }
        // Replace an equation of the combination that holds one of the states (the
        // equation that defined the dependent state), not yet replaced.
        let target = r
            .combo
            .iter()
            .map(|&(k, _)| k)
            .filter(|k| !replaced.contains(k))
            .max_by_key(|&k| {
                let u = w.eq_unknowns(&w.eqs[k]);
                (
                    u.iter().any(|&v| r.terms.contains_key(&v)),
                    std::cmp::Reverse(k),
                )
            });
        let Some(k) = target else { continue };
        replaced.insert(k);
        new_eqs.push((k, r));
    }
    let n = new_eqs.len();
    for (k, r) in new_eqs {
        let mut terms: Vec<(VarId, i128)> = r.terms.into_iter().collect();
        terms.sort_unstable();
        let mut lhs = crate::expr::Arena::ZERO;
        for (v, c) in terms {
            let x = w.arena.var(v);
            let t = if c == 1 {
                x
            } else {
                let cc = w.arena.constant(c as f64);
                w.arena.mul(cc, x)
            };
            lhs = w.arena.add(lhs, t);
        }
        for (e, c) in r.rest {
            let t = if c == 1 {
                e
            } else {
                let cc = w.arena.constant(c as f64);
                w.arena.mul(cc, e)
            };
            lhs = w.arena.add(lhs, t);
        }
        let old = w.eqs[k];
        w.eqs[k] = Eq {
            lhs,
            rhs: crate::expr::Arena::ZERO,
            ..old
        };
    }
    n
}
