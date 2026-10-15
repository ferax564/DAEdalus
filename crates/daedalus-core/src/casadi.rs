//! Export of the causalised system for CasADi (optimal control, MPC, estimation).
//!
//! The export is a JSON document that the `daedalus_casadi` Python package turns
//! into CasADi symbolic expressions. It carries the system after alias elimination,
//! index reduction and tearing, as a semi-explicit index-1 DAE
//!
//! ```text
//!     der(x) = f(t, x, z, u, p)
//!          0 = g(t, x, z, u, p)
//! ```
//!
//! - `x`: the selected states,
//! - `z`: the tearing variables of algebraic loops (no `z` when the model has none),
//! - `u`: top-level `input` variables,
//! - `p`: public parameters with a literal value (the ones a user can change).
//!
//! Every other variable is an expression of those, so a cost or constraint can
//! refer to any model variable by name. Event relations become plain comparisons
//! (`if_else` in CasADi); when-clauses are not part of the continuous problem, so
//! discrete variables keep their initial values.
//!
//! The document also holds the values of all variables after DAEdalus'
//! own initialization at the start time: consistent initial states and guesses.

use crate::backend::{Block, System, eval};
use crate::codegen;
use crate::expr::{Arena, ExprId, Node, VarId, fn1_name, fn2_name};
use crate::flat::VarKind;
use crate::sim::{SimOptions, Simulation};
use anyhow::{Result, bail};
use rustc_hash::FxHashMap;
use serde_json::{Value, json};

/// Format tag and version of the exported document.
pub const FORMAT: &str = "daedalus-casadi";
pub const VERSION: u32 = 1;

/// How a variable is defined in the exported problem.
enum Def {
    State(usize),
    Alg(usize),
    Input(usize),
    Param(usize),
    Expr(ExprId),
    /// Fixed numeric value (discrete variables, unused unknowns).
    Value,
}

/// Compact, topologically ordered copy of the expressions the export needs.
struct Nodes<'a> {
    arena: &'a Arena,
    der_of: &'a [Option<VarId>],
    map: FxHashMap<ExprId, usize>,
    out: Vec<Value>,
}

impl Nodes<'_> {
    /// Add `root` and everything below it; returns its index in the node list.
    fn add(&mut self, root: ExprId) -> Result<usize> {
        if let Some(&i) = self.map.get(&root) {
            return Ok(i);
        }
        let mut stack = vec![(root, false)];
        let mut ch = Vec::new();
        while let Some((id, expanded)) = stack.pop() {
            if self.map.contains_key(&id) {
                continue;
            }
            ch.clear();
            self.arena.children(id, &mut ch);
            if !expanded {
                stack.push((id, true));
                for &c in ch.iter().rev() {
                    if !self.map.contains_key(&c) {
                        stack.push((c, false));
                    }
                }
                continue;
            }
            let k = |c: ExprId| self.map[&c];
            let node = match self.arena.node(id) {
                Node::Const(b) => json!(["const", f64_json(f64::from_bits(b))]),
                Node::Var(v) | Node::Pre(v) => json!(["var", v]),
                Node::Der(v) => match self.der_of.get(v as usize).copied().flatten() {
                    Some(d) => json!(["var", d]),
                    None => bail!("der() of a variable without a derivative variable"),
                },
                Node::Time => json!(["time"]),
                Node::Initial => json!(["const", 0.0]),
                Node::Neg(a) => json!(["neg", k(a)]),
                Node::Add(a, b) => json!(["add", k(a), k(b)]),
                Node::Sub(a, b) => json!(["sub", k(a), k(b)]),
                Node::Mul(a, b) => json!(["mul", k(a), k(b)]),
                Node::Div(a, b) => json!(["div", k(a), k(b)]),
                Node::Pow(a, b) => json!(["pow", k(a), k(b)]),
                Node::F1(f, a) => json!(["f1", fn1_name(f), k(a)]),
                Node::F2(f, a, b) => json!(["f2", fn2_name(f), k(a), k(b)]),
                Node::Cmp(op, a, b) => json!(["cmp", op.symbol(), k(a), k(b)]),
                Node::Rel(r) => {
                    let rel = self.arena.relations[r as usize];
                    json!(["cmp", rel.op.symbol(), k(rel.lhs), k(rel.rhs)])
                }
                Node::If(c, a, b) => json!(["if", k(c), k(a), k(b)]),
                Node::Interp(t, c, a) => json!(["interp", t, c, k(a)]),
                Node::InterpDer(t, c, a) => json!(["interp_der", t, c, k(a)]),
                Node::Delay(..) => bail!("delay() is not supported by the CasADi export"),
                Node::Call(..) | Node::CallD(..) | Node::Local(_) => {
                    bail!("functions with while-loops are not supported by the CasADi export")
                }
                Node::Clk(..) => bail!("clocked equations are not supported by the CasADi export"),
            };
            self.map.insert(id, self.out.len());
            self.out.push(node);
        }
        Ok(self.map[&root])
    }
}

fn f64_json(x: f64) -> Value {
    if x.is_finite() {
        json!(x)
    } else if x.is_nan() {
        json!("nan")
    } else if x > 0.0 {
        json!("inf")
    } else {
        json!("-inf")
    }
}

/// Export `sys` for CasADi. `opts` supplies the start time and parameter overrides
/// used for the initialization whose values the document carries.
pub fn export(sys: System, opts: &SimOptions) -> Result<Value> {
    let n = sys.vars.len();
    let mut def: Vec<Def> = (0..n).map(|_| Def::Value).collect();
    let mut warnings: Vec<String> = sys.warnings.clone();

    for (i, &s) in sys.states.iter().enumerate() {
        def[s as usize] = Def::State(i);
    }
    let mut algs = Vec::new();
    let mut residuals = Vec::new();
    for b in &sys.blocks {
        match b {
            Block::Assign { var, expr } => def[*var as usize] = Def::Expr(*expr),
            Block::Solve {
                tears,
                inner,
                residuals: res,
                ..
            } => {
                for &t in tears {
                    def[t as usize] = Def::Alg(algs.len());
                    algs.push(t);
                }
                for &(v, e) in inner {
                    def[v as usize] = Def::Expr(e);
                }
                residuals.extend_from_slice(res);
            }
        }
    }
    for a in &sys.aliases {
        def[a.var as usize] = Def::Expr(a.expr);
    }
    if !sys.whens.is_empty() {
        warnings.push(format!(
            "{} when-clause(s) are left out: discrete variables keep their initial values",
            sys.whens.len()
        ));
    }

    let mut nodes = Nodes {
        arena: &sys.arena,
        der_of: &sys.der_of,
        map: FxHashMap::default(),
        out: Vec::new(),
    };

    let params = sys.params.clone();
    let vars = sys.vars.clone();

    // Parameters: free ones (same rule as the code generator: public, with a value
    // that refers to no variable) are symbols, the rest are expressions of them.
    let mut p_list = Vec::new();
    let mut u_list = Vec::new();
    for &p in &params {
        let v = &vars[p as usize];
        if v.kind == VarKind::Input {
            def[p as usize] = Def::Input(u_list.len());
            u_list.push(p);
            continue;
        }
        let e = v.binding.or(v.start);
        if v.kind == VarKind::Parameter
            && !v.is_protected
            && e.is_none_or(|e| sys.arena.vars_in(&[e]).is_empty())
        {
            def[p as usize] = Def::Param(p_list.len());
            p_list.push(p);
        } else if let Some(e) = e {
            def[p as usize] = Def::Expr(e);
        }
    }

    let mut var_json = Vec::with_capacity(n);
    let mut def_json: Vec<Value> = Vec::with_capacity(n);
    for d in &def {
        let dj = match d {
            Def::State(k) => json!(["x", k]),
            Def::Alg(k) => json!(["z", k]),
            Def::Input(k) => json!(["u", k]),
            Def::Param(k) => json!(["p", k]),
            Def::Expr(e) => json!(["expr", nodes.add(*e)?]),
            Def::Value => json!(["value"]),
        };
        def_json.push(dj);
    }
    let mut res_json = Vec::new();
    for &r in &residuals {
        res_json.push(nodes.add(r)?);
    }
    let node_list = std::mem::take(&mut nodes.out);
    let derivs = sys.state_derivs.clone();
    let arena = sys.arena.clone();
    let states = sys.states.clone();
    let experiment = sys.experiment.clone();
    let name = sys.name.clone();

    // Initialize with DAEdalus itself: consistent states, guesses and parameter values.
    let model = codegen::compile(sys);
    let mut sim = Simulation::new(&model, opts.clone());
    let init_error = sim.initialize().err();
    sim.rt.run(&model.outputs);
    let r = &sim.rt.r;
    let value = |v: VarId| r[v as usize];
    let attr = |e: Option<ExprId>| -> Option<Value> {
        let vals = eval::Values {
            var: &|v| r[v as usize],
            pre: &|v| r[v as usize],
            time: opts.start_time,
            initial: true,
            rel: None,
        };
        let x = eval::eval(&arena, e?, &vals, &mut FxHashMap::default());
        x.is_finite().then(|| json!(x))
    };

    for (i, v) in vars.iter().enumerate() {
        let kind = match &def[i] {
            Def::State(_) => "state",
            Def::Alg(_) => "algebraic",
            Def::Input(_) => "input",
            Def::Param(_) => "parameter",
            Def::Expr(_) => match v.kind {
                VarKind::Parameter => "dependent_parameter",
                VarKind::Constant => "constant",
                _ => "computed",
            },
            Def::Value => match v.kind {
                VarKind::Parameter | VarKind::Constant => "constant",
                VarKind::Discrete => "discrete",
                _ => "fixed",
            },
        };
        let mut o = json!({
            "name": v.name,
            "kind": kind,
            "def": def_json[i],
            "value": f64_json(value(i as VarId)),
        });
        if !v.unit.is_empty() {
            o["unit"] = json!(v.unit);
        }
        if !v.description.is_empty() {
            o["description"] = json!(v.description);
        }
        if v.is_protected {
            o["protected"] = json!(true);
        }
        if v.generated {
            o["generated"] = json!(true);
        }
        for (key, e) in [("min", v.min), ("max", v.max), ("nominal", v.nominal)] {
            if let Some(x) = attr(e) {
                o[key] = x;
            }
        }
        var_json.push(o);
    }

    Ok(json!({
        "format": FORMAT,
        "version": VERSION,
        "model": name,
        "experiment": experiment,
        "start_time": opts.start_time,
        "initialized": init_error.is_none(),
        "init_error": init_error,
        "states": states,
        "derivatives": derivs,
        "algebraics": algs,
        "inputs": u_list,
        "parameters": p_list,
        "residuals": res_json,
        "variables": var_json,
        "nodes": node_list,
        "tables": arena.tables,
        "warnings": warnings,
    }))
}
