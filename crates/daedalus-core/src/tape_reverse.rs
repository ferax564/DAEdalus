//! Reverse-mode (adjoint) differentiation of the compiled tape.
//!
//! [`run_reverse`] propagates adjoints `adj` backwards through a program that has just
//! been run forward ([`Runtime::run`]): on entry `adj` holds `w`, the weights of the
//! program's outputs; on return the adjoints of its inputs have been increased by
//! `w^T d(outputs)/d(inputs)` and the adjoints of the registers it writes are zero. One
//! reverse pass gives the gradient of a scalar with respect to every input (states,
//! parameters, time) at the cost of a few forward passes, where forward-mode tangents
//! ([`Runtime::run_tangent`]) need one pass per input direction.
//!
//! It relies on two properties of the code generator: every register is written at most
//! once on the path a program takes (temporaries are never reused; the two writes of a
//! branched `if` are on exclusive paths), and the values in the register file are those
//! of the forward run. Branches (`Jz`/`Jmp`) are replayed from the condition registers.
//!
//! Algebraic loops (`Newton`) are differentiated implicitly at their solution, the
//! transpose of the tangent rule `J dtears = -dF`: with `a` the adjoint of the tears
//! (directly and through the block's inner variables), `J^T l = -a`, and `l` is pushed
//! back through the residuals with the tears held. Calls of loop programs use their
//! exact partial derivatives (one tangent run per argument). Relations, `delay()` and
//! discrete values have zero derivative, as in forward mode.

use crate::codegen::{Ins, Runtime};
use crate::expr::{Fn1, Fn2, eval_f1};

/// Indices of the instructions of `prog` executed for the current register values
/// (jumps resolved and left out).
fn trace(rt: &Runtime, prog: &[Ins], out: &mut Vec<u32>) {
    let mut pc = 0;
    while pc < prog.len() {
        let ins = &prog[pc];
        match *ins {
            Ins::Jz(c, k) => {
                pc += 1;
                if rt.r[c as usize] == 0.0 {
                    pc += k as usize;
                }
            }
            Ins::Jmp(k) => pc += 1 + k as usize,
            _ => {
                out.push(pc as u32);
                pc += 1;
            }
        }
    }
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

/// Reverse pass of `prog` (see the module documentation). `adj` has one entry per
/// register.
pub fn run_reverse(rt: &mut Runtime, prog: &[Ins], adj: &mut [f64]) {
    let mut path = Vec::new();
    trace(rt, prog, &mut path);
    for &pc in path.iter().rev() {
        reverse_ins(rt, &prog[pc as usize], adj);
    }
}

#[inline]
fn take(adj: &mut [f64], d: u32) -> f64 {
    std::mem::take(&mut adj[d as usize])
}

fn reverse_ins(rt: &mut Runtime, ins: &Ins, adj: &mut [f64]) {
    let r = &rt.r;
    match *ins {
        Ins::Copy(d, a) => {
            let g = take(adj, d);
            adj[a as usize] += g;
        }
        Ins::Neg(d, a) => {
            let g = take(adj, d);
            adj[a as usize] -= g;
        }
        Ins::Add(d, a, b) => {
            let g = take(adj, d);
            adj[a as usize] += g;
            adj[b as usize] += g;
        }
        Ins::Sub(d, a, b) => {
            let g = take(adj, d);
            adj[a as usize] += g;
            adj[b as usize] -= g;
        }
        Ins::Mul(d, a, b) => {
            let g = take(adj, d);
            if g != 0.0 {
                let (x, y) = (r[a as usize], r[b as usize]);
                adj[a as usize] += g * y;
                adj[b as usize] += g * x;
            }
        }
        Ins::Div(d, a, b) => {
            let g = take(adj, d);
            if g != 0.0 {
                let y = r[b as usize];
                let q = r[a as usize] / y;
                adj[a as usize] += g / y;
                adj[b as usize] -= g * q / y;
            }
        }
        Ins::Pow(d, a, b) => {
            let g = take(adj, d);
            if g != 0.0 {
                let (x, y) = (r[a as usize], r[b as usize]);
                let v = x.powf(y);
                // As in forward mode, a partial that does not exist (0^y with y < 1 for
                // the base, log of a non-positive base for the exponent) only matters
                // when its direction is used: it is left out rather than spreading NaN
                // into constant registers.
                let pa = y * x.powf(y - 1.0);
                if pa.is_finite() {
                    adj[a as usize] += g * pa;
                }
                if x > 0.0 {
                    adj[b as usize] += g * v * x.ln();
                }
            }
        }
        Ins::PowI(d, a, k) => {
            let g = take(adj, d);
            if g != 0.0 && k != 0 {
                adj[a as usize] += g * f64::from(k) * r[a as usize].powi(k - 1);
            }
        }
        Ins::F1(f, d, a) => {
            let g = take(adj, d);
            if g != 0.0 {
                adj[a as usize] += g * d_f1(f, r[a as usize]);
            }
        }
        Ins::F2(f, d, a, b) => {
            let g = take(adj, d);
            if g != 0.0 {
                let (x, y) = (r[a as usize], r[b as usize]);
                match f {
                    Fn2::Atan2 => {
                        let den = x * x + y * y;
                        adj[a as usize] += g * y / den;
                        adj[b as usize] -= g * x / den;
                    }
                    Fn2::Min => adj[if x <= y { a } else { b } as usize] += g,
                    Fn2::Max => adj[if x >= y { a } else { b } as usize] += g,
                    Fn2::Mod => {
                        adj[a as usize] += g;
                        adj[b as usize] -= g * (x / y).floor();
                    }
                    Fn2::Rem => {
                        adj[a as usize] += g;
                        adj[b as usize] -= g * (x / y).trunc();
                    }
                    Fn2::Div | Fn2::And | Fn2::Or => {}
                }
            }
        }
        Ins::Cmp(_, d, ..) | Ins::CallD(d, ..) | Ins::Delay(d, ..) => {
            adj[d as usize] = 0.0;
        }
        Ins::Select(d, c, a, b) => {
            let g = take(adj, d);
            let k = if r[c as usize] != 0.0 { a } else { b };
            adj[k as usize] += g;
        }
        // Table lookups and calls: exact partials along each distinct argument register,
        // by forward mode on the one instruction.
        Ins::Interp(d, _, x, ne) => partials(rt, ins, d, vec![x, ne], adj),
        Ins::Interp2(d, _, u1, u2) => partials(rt, ins, d, vec![u1, u2], adj),
        Ins::Call(d, site, _) => {
            let args = rt.m.sites[site as usize].args.clone();
            partials(rt, ins, d, args, adj)
        }
        // Effects of impure external functions: no derivative.
        Ins::Exec(_) => {}
        Ins::Newton(bi) => reverse_newton(rt, bi as usize, adj),
        Ins::Jz(..) | Ins::Jmp(..) => {}
    }
}

/// Add `g * d(ins)/d(arg)` to the adjoint of every argument register of `ins`, where `g`
/// is the adjoint of its result `d`.
fn partials(rt: &mut Runtime, ins: &Ins, d: u32, mut args: Vec<u32>, adj: &mut [f64]) {
    let g = take(adj, d);
    if g == 0.0 {
        return;
    }
    args.sort_unstable();
    args.dedup();
    let mut dr = vec![0.0; rt.r.len()];
    for s in args {
        dr.iter_mut().for_each(|x| *x = 0.0);
        dr[s as usize] = 1.0;
        rt.run_tangent(std::slice::from_ref(ins), &mut dr);
        let p = dr[d as usize];
        if p != 0.0 && p.is_finite() {
            adj[s as usize] += g * p;
        }
    }
}

fn reverse_newton(rt: &mut Runtime, bi: usize, adj: &mut [f64]) {
    let m = rt.m;
    let b = &m.newton[bi];
    let n = b.tears.len();
    // The second body pass of the tangent rule: inner variables from the tears (with
    // their tangents) and the block's inputs.
    run_reverse(rt, &b.body, adj);
    let mut l: Vec<f64> = b
        .tears
        .iter()
        .map(|&t| -std::mem::take(&mut adj[t as usize]))
        .collect();
    if l.iter().all(|&x| x == 0.0) {
        for &s in &b.residuals {
            adj[s as usize] = 0.0;
        }
        return;
    }
    // J^T l = -a, J row-major (residual, tear).
    rt.run(&b.jac);
    let mut jt = vec![0.0; n * n];
    for i in 0..n {
        for j in 0..n {
            jt[j * n + i] = rt.r[b.jac_slots[i * n + j] as usize];
        }
    }
    let mut piv = vec![0usize; n];
    if crate::linalg::lu_factor(&mut jt, n, &mut piv) {
        crate::linalg::lu_solve(&jt, n, &piv, &mut l);
    } else {
        l.iter_mut().for_each(|x| *x = f64::NAN);
    }
    // The first body pass: residuals with the tears held.
    for (k, &s) in b.residuals.iter().enumerate() {
        adj[s as usize] += l[k];
    }
    run_reverse(rt, &b.body, adj);
    for &t in &b.tears {
        adj[t as usize] = 0.0;
    }
}
