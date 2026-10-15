//! Direct (uncompiled) numeric evaluation of arena expressions.
//! Used for parameter evaluation and numeric checks during compilation; the
//! simulation hot path uses the compiled tape instead.

use crate::expr::{Arena, ExprId, Node, eval_f1, eval_f2};
use rustc_hash::FxHashMap;

pub struct Values<'a> {
    pub var: &'a dyn Fn(u32) -> f64,
    pub pre: &'a dyn Fn(u32) -> f64,
    pub time: f64,
    pub initial: bool,
    /// Value of an event relation (latched); `None` evaluates the relation directly.
    pub rel: Option<&'a dyn Fn(u32) -> f64>,
}

pub fn eval(a: &Arena, root: ExprId, vals: &Values, memo: &mut FxHashMap<ExprId, f64>) -> f64 {
    let mut stack: Vec<(ExprId, bool)> = vec![(root, false)];
    let mut ch = Vec::new();
    while let Some((id, expanded)) = stack.pop() {
        if memo.contains_key(&id) {
            continue;
        }
        let n = a.node(id);
        let leaf = match n {
            Node::Const(b) => Some(f64::from_bits(b)),
            Node::Var(v) | Node::Der(v) => Some((vals.var)(v)),
            Node::Pre(v) => Some((vals.pre)(v)),
            Node::Time => Some(vals.time),
            Node::Initial => Some(if vals.initial { 1.0 } else { 0.0 }),
            Node::Rel(r) if vals.rel.is_some() => Some((vals.rel.unwrap())(r)),
            _ => None,
        };
        if let Some(x) = leaf {
            memo.insert(id, x);
            continue;
        }
        ch.clear();
        a.children(id, &mut ch);
        if !expanded {
            stack.push((id, true));
            for &c in ch.iter().rev() {
                if !memo.contains_key(&c) {
                    stack.push((c, false));
                }
            }
            continue;
        }
        let g = |i: usize| memo[&ch[i]];
        let r = match n {
            Node::Neg(_) => -g(0),
            Node::Add(..) => g(0) + g(1),
            Node::Sub(..) => g(0) - g(1),
            Node::Mul(..) => g(0) * g(1),
            Node::Div(..) => g(0) / g(1),
            Node::Pow(..) => g(0).powf(g(1)),
            Node::F1(f, _) => eval_f1(f, g(0)),
            Node::F2(f, ..) => eval_f2(f, g(0), g(1)),
            Node::Cmp(op, ..) => f64::from(op.eval(g(0), g(1))),
            Node::Rel(r) => f64::from(a.relations[r as usize].op.eval(g(0), g(1))),
            Node::If(..) => {
                if g(0) != 0.0 {
                    g(1)
                } else {
                    g(2)
                }
            }
            Node::Interp(t, c, _) => a.tables[t as usize].interp(c as usize, g(0)),
            Node::InterpDer(t, c, _) => a.tables[t as usize].slope(c as usize, g(0)),
            // no history here: the undelayed value
            Node::Delay(k, _) => (vals.var)(a.delays[k as usize]),
            _ => unreachable!(),
        };
        memo.insert(id, r);
    }
    memo[&root]
}
