//! Parameter sweeps: many simulations of one model with parameter overrides.
//!
//! A sweep is translated once. Free parameters (public, literal value, Real) are changed
//! at run time through [`SimOptions::params`], so every run shares the compiled model;
//! other parameters (computed, Integer/Boolean, possibly structural) are applied as a
//! modifier on a small wrapper class and translated again for each distinct combination,
//! which is slower. Runs execute on native threads, one [`crate::sim::Simulation`] each.

use crate::codegen::Model;
use crate::flat::{VarKind, VarType};
use crate::frontend::Library;
use crate::results::{ResultKind, ResultVar, Results};
use crate::sim::{SimOptions, SimStats};
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// Parameter values by name.
pub type Overrides = Vec<(String, f64)>;

/// One swept parameter and its values.
#[derive(Clone, Debug, PartialEq)]
pub struct SweepParam {
    pub name: String,
    pub values: Vec<f64>,
}

/// How the values of several parameters are combined into runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Combine {
    /// Full factorial: every combination (the last parameter varies fastest).
    Grid,
    /// The n-th value of every parameter together (all lists have the same length),
    /// like Dymola's grouped sweeps.
    Grouped,
}

/// Parse a value list: `1,2,3`, `lin:START:STOP:N` (N evenly spaced values) or
/// `log:START:STOP:N` (N logarithmically spaced values, START and STOP > 0).
pub fn parse_values(spec: &str) -> Result<Vec<f64>, String> {
    let spec = spec.trim();
    let num = |s: &str| -> Result<f64, String> {
        s.trim()
            .parse::<f64>()
            .map_err(|_| format!("'{}' is not a number", s.trim()))
    };
    let range = |rest: &str, kind: &str| -> Result<(f64, f64, usize), String> {
        let p: Vec<&str> = rest.split(':').collect();
        if p.len() != 3 {
            return Err(format!("expected {kind}:START:STOP:N, got '{spec}'"));
        }
        let n: usize = p[2]
            .trim()
            .parse()
            .map_err(|_| format!("'{}' is not a count", p[2].trim()))?;
        if n == 0 {
            return Err(format!("{kind}: N must be at least 1"));
        }
        Ok((num(p[0])?, num(p[1])?, n))
    };
    let at = |a: f64, b: f64, n: usize, i: usize| {
        if n == 1 {
            a
        } else if i == n - 1 {
            b
        } else {
            a + (b - a) * i as f64 / (n - 1) as f64
        }
    };
    if let Some(rest) = spec.strip_prefix("lin:") {
        let (a, b, n) = range(rest, "lin")?;
        Ok((0..n).map(|i| at(a, b, n, i)).collect())
    } else if let Some(rest) = spec.strip_prefix("log:") {
        let (a, b, n) = range(rest, "log")?;
        if !(a > 0.0 && b > 0.0) {
            return Err(format!(
                "log: START and STOP must be positive, got '{spec}'"
            ));
        }
        let (la, lb) = (a.log10(), b.log10());
        Ok((0..n)
            .map(|i| match i {
                0 => a,
                _ if i == n - 1 => b,
                _ => 10f64.powf(at(la, lb, n, i)),
            })
            .collect())
    } else {
        let v: Vec<f64> = spec
            .split(',')
            .filter(|s| !s.trim().is_empty())
            .map(num)
            .collect::<Result<_, _>>()?;
        if v.is_empty() {
            return Err("empty value list".into());
        }
        Ok(v)
    }
}

/// Parse `NAME=VALUES` (see [`parse_values`]).
pub fn parse_param(spec: &str) -> Result<SweepParam, String> {
    let (name, values) = spec
        .split_once('=')
        .ok_or_else(|| format!("expected NAME=VALUES, got '{spec}'"))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(format!("missing parameter name in '{spec}'"));
    }
    Ok(SweepParam {
        name: name.to_string(),
        values: parse_values(values)?,
    })
}

/// The parameter values of every run, in run order.
pub fn expand(params: &[SweepParam], combine: Combine) -> Result<Vec<Overrides>, String> {
    if params.is_empty() {
        return Ok(vec![Vec::new()]);
    }
    for (i, p) in params.iter().enumerate() {
        if params[..i].iter().any(|q| q.name == p.name) {
            return Err(format!("parameter '{}' is swept twice", p.name));
        }
    }
    match combine {
        Combine::Grouped => {
            let n = params[0].values.len();
            if let Some(p) = params.iter().find(|p| p.values.len() != n) {
                return Err(format!(
                    "grouped sweep: '{}' has {} values but '{}' has {n}",
                    p.name,
                    p.values.len(),
                    params[0].name
                ));
            }
            Ok((0..n)
                .map(|i| {
                    params
                        .iter()
                        .map(|p| (p.name.clone(), p.values[i]))
                        .collect()
                })
                .collect())
        }
        Combine::Grid => {
            let total: usize = params.iter().map(|p| p.values.len()).product();
            Ok((0..total)
                .map(|mut k| {
                    let mut point = vec![(String::new(), 0.0); params.len()];
                    for (j, p) in params.iter().enumerate().rev() {
                        let n = p.values.len();
                        point[j] = (p.name.clone(), p.values[k % n]);
                        k /= n;
                    }
                    point
                })
                .collect())
        }
    }
}

/// How an override reaches the simulation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamClass {
    /// A free Real parameter: set at run time on the shared compiled model.
    Runtime,
    /// Anything else: needs a translation with a modifier (slower).
    Structural,
}

/// Whether overriding `name` at run time is safe: it must be a free parameter (public,
/// literal binding) of type Real. Integer and Boolean parameters may size arrays or
/// enable components, and computed parameters would lose their binding, so they are
/// translated again with a modifier instead.
pub fn classify(m: &Model, name: &str) -> ParamClass {
    let free = m.free_params.iter().any(|&p| {
        let v = &m.vars[p as usize];
        v.name == name && v.kind == VarKind::Parameter && v.ty == VarType::Real
    });
    if free {
        ParamClass::Runtime
    } else {
        ParamClass::Structural
    }
}

/// Every name must be a parameter (or top-level input) of the translated model: a
/// modifier on a name that does not exist would otherwise be ignored silently.
pub fn check_names<'a>(m: &Model, names: impl IntoIterator<Item = &'a str>) -> Result<(), String> {
    for n in names {
        if !m
            .vars
            .iter()
            .any(|v| v.name == n && matches!(v.kind, VarKind::Parameter | VarKind::Input))
        {
            return Err(format!("unknown parameter '{n}' in {}", m.name));
        }
    }
    Ok(())
}

/// Split overrides into run-time ones and ones that need a translation.
pub fn split_overrides(
    m: &Model,
    params: &[(String, f64)],
    force_translate: bool,
) -> (Overrides, Overrides) {
    params
        .iter()
        .cloned()
        .partition(|(n, _)| !force_translate && classify(m, n) == ParamClass::Runtime)
}

/// Modelica literal for an override, given the parameter's type when known.
fn literal(v: f64, ty: Option<VarType>) -> String {
    match ty {
        Some(VarType::Boolean) => (if v != 0.0 { "true" } else { "false" }).to_string(),
        Some(VarType::Integer) => format!("{}", v.round() as i64),
        // `{:?}` keeps a decimal point or exponent: `1.0`, `2.5`, `1e-7`.
        _ => format!("{v:?}"),
    }
}

/// A class modification such as `a(b(c = 1.0, d = 2.0), k = 3.0)` for dotted names.
pub fn modification(mods: &[(String, f64)], ty: &dyn Fn(&str) -> Option<VarType>) -> String {
    #[derive(Default)]
    struct Node {
        value: Option<String>,
        kids: BTreeMap<String, Node>,
    }
    fn render(n: &Node) -> String {
        n.kids
            .iter()
            .map(|(k, c)| match (&c.value, c.kids.is_empty()) {
                (Some(v), true) => format!("{k} = {v}"),
                (Some(v), false) => format!("{k}({}) = {v}", render(c)),
                (None, _) => format!("{k}({})", render(c)),
            })
            .collect::<Vec<_>>()
            .join(", ")
    }
    let mut root = Node::default();
    for (name, v) in mods {
        let mut n = &mut root;
        for part in name.split('.') {
            n = n.kids.entry(part.to_string()).or_default();
        }
        n.value = Some(literal(*v, ty(name)));
    }
    render(&root)
}

const WRAPPER_URI: &str = "__daedalus_sweep__.mo";

/// Translate `model` with structural overrides: a wrapper class `extends model(mods)` is
/// added to the library, translated and removed again. The returned model carries the
/// original name. `base` (a translation of the unmodified model) supplies the types of
/// the overridden parameters.
pub fn translate_with_modifiers(
    lib: &mut Library,
    model: &str,
    mods: &[(String, f64)],
    base: Option<&Model>,
    opts: &crate::backend::Options,
) -> anyhow::Result<Model> {
    if mods.is_empty() {
        return crate::translate(lib, model, opts);
    }
    let ty = |n: &str| -> Option<VarType> {
        base.and_then(|m| m.vars.iter().find(|v| v.name == n).map(|v| v.ty))
    };
    let class = "DaedalusSweepRun";
    let text = format!(
        "model {class}\n  extends {model}({});\nend {class};\n",
        modification(mods, &ty)
    );
    let errors = lib.set_document(WRAPPER_URI, &text);
    let r = if errors.is_empty() {
        crate::translate(lib, class, opts)
    } else {
        Err(anyhow::anyhow!("modifier: {}", errors.join("; ")))
    };
    lib.remove_document(WRAPPER_URI);
    let mut m = r?;
    m.name = model.to_string();
    Ok(m)
}

/// Run `f(0..n)` on up to `threads` native threads, calling `progress(i, &result)` as each
/// run finishes (from the worker thread). Results come back in index order. Setting
/// `cancel` stops handing out new runs; runs not started are returned as `None`.
pub fn run_parallel<T, F, P>(
    n: usize,
    threads: usize,
    cancel: Option<&AtomicBool>,
    f: F,
    progress: P,
) -> Vec<Option<T>>
where
    T: Send,
    F: Fn(usize) -> T + Sync,
    P: Fn(usize, &T) + Sync,
{
    let next = AtomicUsize::new(0);
    let threads = threads.clamp(1, n.max(1));
    let mut out: Vec<Option<T>> = (0..n).map(|_| None).collect();
    let done: Vec<(usize, T)> = std::thread::scope(|sc| {
        let handles: Vec<_> = (0..threads)
            .map(|_| {
                sc.spawn(|| {
                    let mut mine = Vec::new();
                    loop {
                        if cancel.is_some_and(|c| c.load(Ordering::Relaxed)) {
                            break;
                        }
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        if i >= n {
                            break;
                        }
                        let r = f(i);
                        progress(i, &r);
                        mine.push((i, r));
                    }
                    mine
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|h| h.join().unwrap_or_default())
            .collect()
    });
    for (i, r) in done {
        out[i] = Some(r);
    }
    out
}

/// A scalar characteristic of a trajectory, for the summary table.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Stat {
    Initial,
    Final,
    Min,
    Max,
    /// Time average (trapezoidal).
    Mean,
    /// Value at a time (linear interpolation).
    At(f64),
}

impl Stat {
    /// `initial | final | min | max | mean | at=T`.
    pub fn parse(s: &str) -> Result<Stat, String> {
        Ok(match s.trim() {
            "initial" | "start" => Stat::Initial,
            "final" | "end" => Stat::Final,
            "min" => Stat::Min,
            "max" => Stat::Max,
            "mean" | "avg" => Stat::Mean,
            t => match t.strip_prefix("at=").or_else(|| t.strip_prefix("at:")) {
                Some(x) => Stat::At(x.parse().map_err(|_| format!("'{x}' is not a time"))?),
                None => {
                    return Err(format!(
                        "unknown statistic '{t}' (initial | final | min | max | mean | at=T)"
                    ));
                }
            },
        })
    }

    pub fn label(&self) -> String {
        match self {
            Stat::Initial => "initial".into(),
            Stat::Final => "final".into(),
            Stat::Min => "min".into(),
            Stat::Max => "max".into(),
            Stat::Mean => "mean".into(),
            Stat::At(t) => format!("at={t}"),
        }
    }

    /// The statistic of variable `name` in `res`.
    pub fn eval(&self, res: &Results, name: &str) -> Option<f64> {
        let y = res.values(name)?;
        if y.is_empty() {
            return None;
        }
        Some(match self {
            Stat::Initial => y[0],
            Stat::Final => *y.last().unwrap(),
            Stat::Min => y.iter().copied().fold(f64::INFINITY, f64::min),
            Stat::Max => y.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            Stat::Mean => {
                let t = &res.time;
                let span = t[t.len() - 1] - t[0];
                if span <= 0.0 {
                    y[0]
                } else {
                    let s: f64 = (1..t.len())
                        .map(|i| 0.5 * (y[i] + y[i - 1]) * (t[i] - t[i - 1]))
                        .sum();
                    s / span
                }
            }
            Stat::At(t) => res.at(name, *t)?,
        })
    }
}

/// A copy of `res` keeping only the named variables (in that order) as series.
pub fn subset(res: &Results, names: &[String]) -> Results {
    let mut out = Results {
        model: res.model.clone(),
        time: res.time.clone(),
        log: res.log.clone(),
        ..Default::default()
    };
    for n in names {
        if let Some(v) = res.find(n) {
            out.vars.push(ResultVar {
                name: v.name.clone(),
                description: v.description.clone(),
                unit: v.unit.clone(),
                kind: ResultKind::Series(out.series.len()),
            });
            out.series.push(res.values_of(v));
        }
    }
    out
}

/// One finished run of a sweep.
#[derive(Clone, Debug)]
pub struct RunRecord {
    pub index: usize,
    pub params: Vec<(String, f64)>,
    /// The run was translated again for its structural overrides.
    pub retranslated: bool,
    /// The kept variables (see [`subset`]); partial results on failure.
    pub results: Results,
    pub error: Option<String>,
    pub stats: Option<SimStats>,
    pub wall_ms: f64,
}

/// Simulate one run: `model` with run-time overrides `params` on top of `base`.
pub fn simulate_run(
    model: &Model,
    base: &SimOptions,
    params: &[(String, f64)],
) -> Result<(Results, SimStats), Box<crate::sim::SimError>> {
    let mut o = base.clone();
    for (n, v) in params {
        o.params.retain(|(m, _)| m != n);
        o.params.push((n.clone(), *v));
    }
    crate::sim::simulate(model, o).map_err(Box::new)
}

/// The summary table as CSV: run, the swept parameters, status, then `stat(var)` for every
/// kept variable and statistic, and the error message of failed runs.
pub fn write_summary_csv(
    out: &mut dyn Write,
    runs: &[RunRecord],
    vars: &[String],
    stats: &[Stat],
) -> io::Result<()> {
    let pnames: Vec<&str> = runs
        .first()
        .map(|r| r.params.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();
    write!(out, "run")?;
    for p in &pnames {
        write!(out, ",\"{p}\"")?;
    }
    write!(out, ",status")?;
    for v in vars {
        for s in stats {
            write!(out, ",\"{}({v})\"", s.label())?;
        }
    }
    writeln!(out, ",error")?;
    for r in runs {
        write!(out, "{}", r.index + 1)?;
        for (_, v) in &r.params {
            write!(out, ",{v}")?;
        }
        write!(out, ",{}", if r.error.is_none() { "ok" } else { "fail" })?;
        for v in vars {
            for s in stats {
                match r.error.is_none().then(|| s.eval(&r.results, v)).flatten() {
                    Some(x) => write!(out, ",{x}")?,
                    None => write!(out, ",")?,
                }
            }
        }
        let err = r
            .error
            .as_deref()
            .unwrap_or("")
            .lines()
            .next()
            .unwrap_or("");
        writeln!(out, ",\"{}\"", err.replace('"', "'"))?;
    }
    Ok(())
}

/// All runs in one CSV, one row per run and time point: `run, params..., time, vars...`.
pub fn write_combined_csv(
    out: &mut dyn Write,
    runs: &[RunRecord],
    vars: &[String],
) -> io::Result<()> {
    let pnames: Vec<&str> = runs
        .first()
        .map(|r| r.params.iter().map(|(n, _)| n.as_str()).collect())
        .unwrap_or_default();
    write!(out, "run")?;
    for p in &pnames {
        write!(out, ",\"{p}\"")?;
    }
    write!(out, ",time")?;
    for v in vars {
        write!(out, ",\"{v}\"")?;
    }
    writeln!(out)?;
    for r in runs {
        let cols: Vec<Option<Vec<f64>>> = vars.iter().map(|v| r.results.values(v)).collect();
        let prefix: String = std::iter::once((r.index + 1).to_string())
            .chain(r.params.iter().map(|(_, v)| v.to_string()))
            .collect::<Vec<_>>()
            .join(",");
        for (i, t) in r.results.time.iter().enumerate() {
            write!(out, "{prefix},{t}")?;
            for c in &cols {
                match c {
                    Some(c) => write!(out, ",{}", c[i])?,
                    None => write!(out, ",")?,
                }
            }
            writeln!(out)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_is_shareable_between_threads() {
        fn shareable<T: Send + Sync>() {}
        shareable::<Model>();
    }

    #[test]
    fn modification_nests_dotted_names() {
        let ty = |n: &str| (n == "n").then_some(VarType::Integer);
        let m = modification(
            &[
                ("a.b.c".into(), 1.0),
                ("a.b.d".into(), 2.5),
                ("k".into(), 3.0),
                ("n".into(), 4.0),
            ],
            &ty,
        );
        assert_eq!(m, "a(b(c = 1.0, d = 2.5)), k = 3.0, n = 4");
    }
}
