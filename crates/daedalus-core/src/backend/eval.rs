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
    // Phase 0: unvisited; 1: children evaluated; 2: `if` condition evaluated;
    // 3: the taken `if` branch evaluated (only that branch is evaluated).
    let mut stack: Vec<(ExprId, u8)> = vec![(root, 0)];
    let mut ch = Vec::new();
    while let Some((id, phase)) = stack.pop() {
        if memo.contains_key(&id) {
            continue;
        }
        let n = a.node(id);
        if let Node::If(c, x, y) = n {
            match phase {
                0 => {
                    stack.push((id, 2));
                    stack.push((c, 0));
                }
                2 => {
                    let b = if memo[&c] != 0.0 { x } else { y };
                    stack.push((id, 3));
                    stack.push((b, 0));
                }
                _ => {
                    let b = if memo[&c] != 0.0 { x } else { y };
                    let v = memo[&b];
                    memo.insert(id, v);
                }
            }
            continue;
        }
        let expanded = phase == 1;
        let leaf = match n {
            Node::Const(b) => Some(f64::from_bits(b)),
            // `Local` only occurs inside loop programs, where `var` reads the loop state
            Node::Var(v) | Node::Der(v) | Node::Local(v) => Some((vals.var)(v)),
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
            stack.push((id, 1));
            for &c in ch.iter().rev() {
                if !memo.contains_key(&c) {
                    stack.push((c, 0));
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
            Node::Interp(t, c, _) => a.tables[t as usize].interp(c as usize, g(0)),
            Node::InterpDer(t, c, _) => a.tables[t as usize].slope(c as usize, g(0)),
            // no history here: the undelayed value
            Node::Delay(k, _) => (vals.var)(a.delays[k as usize]),
            Node::Call(c, out) => {
                let args: Vec<f64> = (0..ch.len()).map(g).collect();
                run_prog(a, a.calls[c as usize].prog, &args)[out as usize]
            }
            Node::CallD(c, k) => {
                let args: Vec<f64> = (0..ch.len()).map(g).collect();
                call_partial(a, a.calls[c as usize].prog, &args, k >> 16, k & 0xffff)
            }
            _ => unreachable!(),
        };
        memo.insert(id, r);
    }
    memo[&root]
}

/// Largest number of iterations of a run-time `while` loop.
pub const MAX_LOOP_ITERATIONS: usize = 10_000_000;

/// Run loop program `p` from the initial state `args`; returns the final state.
pub fn run_prog(a: &Arena, p: u32, args: &[f64]) -> Vec<f64> {
    let key = (p, args.iter().map(|x| x.to_bits()).collect::<Vec<u64>>());
    if let Some(v) = a.prog_cache.borrow().get(&key) {
        return v.to_vec();
    }
    let x = run_prog_uncached(a, p, args);
    let mut c = a.prog_cache.borrow_mut();
    if c.len() > 100_000 {
        c.clear();
    }
    c.insert(key, x.clone().into());
    x
}

fn run_prog_uncached(a: &Arena, p: u32, args: &[f64]) -> Vec<f64> {
    let prog = &a.progs[p as usize];
    let mut x = args.to_vec();
    for _ in 0..MAX_LOOP_ITERATIONS {
        let xs = x.clone();
        let get = |v: u32| xs[v as usize];
        let vals = Values {
            var: &get,
            pre: &get,
            time: 0.0,
            initial: false,
            rel: None,
        };
        let mut memo = FxHashMap::default();
        if eval(a, prog.cond, &vals, &mut memo) == 0.0 {
            break;
        }
        let next: Vec<f64> = prog
            .next
            .iter()
            .map(|&e| eval(a, e, &vals, &mut memo))
            .collect();
        x = next;
    }
    x
}

/// Finite-difference partial derivative of output `out` with respect to argument `k`.
pub fn call_partial(a: &Arena, p: u32, args: &[f64], out: u32, k: u32) -> f64 {
    let f0 = run_prog(a, p, args)[out as usize];
    let mut ap = args.to_vec();
    let h = f64::EPSILON.sqrt() * ap[k as usize].abs().max(1.0);
    ap[k as usize] += h;
    (run_prog(a, p, &ap)[out as usize] - f0) / h
}
