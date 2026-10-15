//! The Modelica wrapper block of an imported FMU.
//!
//! Parameters, inputs and outputs of the FMU become parameters, input and output
//! connectors of a block, with Modelica identifiers made from the FMU names
//! ([`interface`]; `engine.J` becomes `engine_J`). The simulator binds the block's
//! variables back to the FMU's value references with the same function, so the
//! mapping needs no table in the generated code. A Model Exchange wrapper also has the
//! FMU's continuous states `fmu_x[n]` with `der(fmu_x) = fmu_dx`, so the model's own
//! integrator integrates them; a Co-Simulation wrapper has the parameter
//! `communicationStepSize`.

use super::md::{Causality, ModelDescription, Ty, Variability};
use crate::external::{DERIVATIVES, FMU_ANNOTATION, FmiKind, STATES, STEP};
use std::collections::HashSet;
use std::fmt::Write;

/// A variable of the FMU exposed by the wrapper.
#[derive(Clone, Debug)]
pub struct Port {
    /// Index into the model description's variables.
    pub var: usize,
    /// Modelica identifier in the wrapper.
    pub ident: String,
}

#[derive(Clone, Debug, Default)]
pub struct Interface {
    pub params: Vec<Port>,
    pub inputs: Vec<Port>,
    pub outputs: Vec<Port>,
    /// Scalar local variables and calculated parameters, which a wrapper may expose as
    /// (unconnected) outputs to observe the FMU's internals; identifiers are allocated
    /// after the ports, so they never change the ports' names.
    pub locals: Vec<Port>,
    /// Variables that could not be exposed (strings, arrays, ...), by FMU name.
    pub skipped: Vec<String>,
}

const KEYWORDS: &[&str] = &[
    "algorithm",
    "and",
    "annotation",
    "block",
    "break",
    "class",
    "connect",
    "connector",
    "constant",
    "constrainedby",
    "der",
    "discrete",
    "each",
    "else",
    "elseif",
    "elsewhen",
    "encapsulated",
    "end",
    "enumeration",
    "equation",
    "expandable",
    "extends",
    "external",
    "false",
    "final",
    "flow",
    "for",
    "function",
    "if",
    "import",
    "impure",
    "in",
    "initial",
    "inner",
    "input",
    "loop",
    "model",
    "not",
    "operator",
    "or",
    "outer",
    "output",
    "package",
    "parameter",
    "partial",
    "protected",
    "public",
    "pure",
    "record",
    "redeclare",
    "replaceable",
    "return",
    "stream",
    "then",
    "true",
    "type",
    "when",
    "while",
    "within",
    "time",
    "Real",
    "Integer",
    "Boolean",
    "String",
];

const RESERVED: &[&str] = &[
    STATES,
    DERIVATIVES,
    STEP,
    "RealInput",
    "RealOutput",
    "IntegerInput",
    "IntegerOutput",
    "BooleanInput",
    "BooleanOutput",
];

/// A Modelica identifier for an FMU variable name.
pub fn ident(name: &str) -> String {
    let mut s = String::with_capacity(name.len());
    let mut last_us = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            s.push(c);
            last_us = false;
        } else if !last_us {
            s.push('_');
            last_us = true;
        }
    }
    let s = s.trim_end_matches('_').to_string();
    let mut s = if s.is_empty() { "v".to_string() } else { s };
    if s.starts_with(|c: char| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    if KEYWORDS.contains(&s.as_str()) {
        s.push('_');
    }
    s
}

/// The variables the wrapper of `md` exposes, with their identifiers.
pub fn interface(md: &ModelDescription) -> Interface {
    let mut used: HashSet<String> = RESERVED.iter().map(|s| s.to_string()).collect();
    let mut out = Interface::default();
    for (k, v) in md.vars.iter().enumerate() {
        let list = match v.causality {
            Causality::Parameter
                if matches!(v.variability, Variability::Fixed | Variability::Tunable) =>
            {
                &mut out.params
            }
            Causality::Input => &mut out.inputs,
            Causality::Output => &mut out.outputs,
            _ => continue,
        };
        if v.ty == Ty::Other || v.is_array {
            out.skipped.push(v.name.clone());
            continue;
        }
        let base = ident(&v.name);
        let mut id = base.clone();
        let mut n = 2;
        while used.contains(&id) {
            id = format!("{base}_{n}");
            n += 1;
        }
        used.insert(id.clone());
        list.push(Port { var: k, ident: id });
    }
    for (k, v) in md.vars.iter().enumerate() {
        if !matches!(
            v.causality,
            Causality::Local | Causality::CalculatedParameter
        ) || v.ty == Ty::Other
            || v.is_array
        {
            continue;
        }
        let base = ident(&v.name);
        let mut id = base.clone();
        let mut n = 2;
        while used.contains(&id) {
            id = format!("{base}_{n}");
            n += 1;
        }
        used.insert(id.clone());
        out.locals.push(Port { var: k, ident: id });
    }
    out
}

fn real(x: f64) -> String {
    if x.is_finite() {
        let s = format!("{x:?}");
        s
    } else if x > 0.0 {
        "1e300".into()
    } else if x < 0.0 {
        "-1e300".into()
    } else {
        "0.0".into()
    }
}

fn mo_string(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

fn ty_name(t: Ty) -> &'static str {
    match t {
        Ty::Float64 | Ty::Float32 => "Real",
        Ty::Int32 | Ty::Int64 => "Integer",
        Ty::Boolean => "Boolean",
        Ty::Other => "Real",
    }
}

fn value(t: Ty, x: f64) -> String {
    match t {
        Ty::Boolean => (x != 0.0).to_string(),
        Ty::Int32 | Ty::Int64 => format!("{}", x.round() as i64),
        _ => real(x),
    }
}

/// The default communication step of a Co-Simulation wrapper.
pub fn default_step(md: &ModelDescription) -> f64 {
    let e = &md.experiment;
    e.step.filter(|&h| h > 0.0).unwrap_or_else(|| {
        let span = e.stop.unwrap_or(1.0) - e.start.unwrap_or(0.0);
        if span > 0.0 { span / 500.0 } else { 1e-3 }
    })
}

/// Options of [`wrapper_text`].
#[derive(Clone, Debug, Default)]
pub struct WrapperOptions {
    /// `within` clause.
    pub within: Option<String>,
    /// Add an `experiment` annotation from the FMU's default experiment.
    pub experiment: bool,
    /// Communication step size of a Co-Simulation wrapper (default from the FMU).
    pub step: Option<f64>,
    /// Also expose the FMU's local variables as outputs (see [`Interface::locals`]).
    pub locals: bool,
}

/// Modelica source of the wrapper block `class_name` for the FMU `md`, found at
/// `file_name` (as written into the annotation: relative to the `.mo` file, or
/// absolute).
pub fn wrapper_text(
    md: &ModelDescription,
    class_name: &str,
    file_name: &str,
    kind: FmiKind,
    opts: &WrapperOptions,
) -> String {
    let itf = interface(md);
    let mut s = String::new();
    if let Some(w) = &opts.within {
        let _ = writeln!(s, "within {w};");
    }
    let desc = if md.description.is_empty() {
        format!("FMU {} ({})", md.model_name, kind.as_str())
    } else {
        format!("{} (FMU, {})", md.description, kind.as_str())
    };
    let _ = writeln!(s, "block {class_name} \"{}\"", mo_string(&desc));
    let blue = "{0,0,127}";
    let pink = "{255,127,0}";
    let purple = "{255,0,255}";
    for (t, color) in [("Real", blue), ("Integer", pink), ("Boolean", purple)] {
        let used_in = itf.inputs.iter().any(|p| ty_name(md.vars[p.var].ty) == t);
        let used_out = itf.outputs.iter().any(|p| ty_name(md.vars[p.var].ty) == t);
        if used_in {
            let _ = writeln!(
                s,
                "  connector {t}Input = input {t} annotation(Icon(graphics={{Polygon(points={{{{-100,100}},{{100,0}},{{-100,-100}},{{-100,100}}}}, lineColor={color}, fillColor={color}, fillPattern=FillPattern.Solid)}}));"
            );
        }
        if used_out {
            let _ = writeln!(
                s,
                "  connector {t}Output = output {t} annotation(Icon(graphics={{Polygon(points={{{{-100,100}},{{100,0}},{{-100,-100}},{{-100,100}}}}, lineColor={color}, fillColor={{255,255,255}}, fillPattern=FillPattern.Solid)}}));"
            );
        }
    }
    for p in &itf.params {
        let v = &md.vars[p.var];
        let mut attrs = Vec::new();
        if v.ty.is_real() && !v.unit.is_empty() {
            attrs.push(format!("unit = \"{}\"", mo_string(&v.unit)));
        }
        let a = if attrs.is_empty() {
            String::new()
        } else {
            format!("({})", attrs.join(", "))
        };
        let _ = writeln!(
            s,
            "  parameter {} {}{a} = {} \"{}\";",
            ty_name(v.ty),
            p.ident,
            value(v.ty, v.start.unwrap_or(0.0)),
            mo_string(&describe(&v.name, &v.description))
        );
    }
    let place = |k: usize, n: usize, x: i32| -> String {
        let y = if n <= 1 {
            0.0
        } else {
            80.0 - 160.0 * k as f64 / (n - 1) as f64
        };
        let y = y.round() as i32;
        format!(
            "annotation(Placement(transformation(extent={{{{{},{}}},{{{},{}}}}})))",
            x - 20,
            y - 20,
            x + 20,
            y + 20
        )
    };
    let n_in = itf.inputs.len();
    for (k, p) in itf.inputs.iter().enumerate() {
        let v = &md.vars[p.var];
        let mut attrs = Vec::new();
        if let Some(x) = v.start {
            attrs.push(format!("start = {}", value(v.ty, x)));
        }
        if v.ty.is_real() && !v.unit.is_empty() {
            attrs.push(format!("unit = \"{}\"", mo_string(&v.unit)));
        }
        let a = if attrs.is_empty() {
            String::new()
        } else {
            format!("({})", attrs.join(", "))
        };
        let _ = writeln!(
            s,
            "  {}Input {}{a} \"{}\" {};",
            ty_name(v.ty),
            p.ident,
            mo_string(&describe(&v.name, &v.description)),
            place(k, n_in, -120)
        );
    }
    let n_out = itf.outputs.len();
    for (k, p) in itf.outputs.iter().enumerate() {
        let v = &md.vars[p.var];
        let a = if v.ty.is_real() && !v.unit.is_empty() {
            format!("(unit = \"{}\")", mo_string(&v.unit))
        } else {
            String::new()
        };
        let _ = writeln!(
            s,
            "  {}Output {}{a} \"{}\" {};",
            ty_name(v.ty),
            p.ident,
            mo_string(&describe(&v.name, &v.description)),
            place(k, n_out, 120)
        );
    }
    if opts.locals && !itf.locals.is_empty() {
        let _ = writeln!(
            s,
            "  // Local variables of the FMU, read after every evaluation"
        );
        for p in &itf.locals {
            let v = &md.vars[p.var];
            let a = if v.ty.is_real() && !v.unit.is_empty() {
                format!("(unit = \"{}\")", mo_string(&v.unit))
            } else {
                String::new()
            };
            let _ = writeln!(
                s,
                "  output {} {}{a} \"{}\";",
                ty_name(v.ty),
                p.ident,
                mo_string(&describe(&v.name, &v.description))
            );
        }
    }
    let nx = md.states.len();
    match kind {
        FmiKind::ModelExchange if nx > 0 => {
            let starts: Vec<String> = md
                .states
                .iter()
                .map(|&k| real(md.vars[k].start.unwrap_or(0.0)))
                .collect();
            let names: Vec<&str> = md
                .states
                .iter()
                .map(|&k| md.vars[k].name.as_str())
                .collect();
            let _ = writeln!(
                s,
                "  Real {STATES}[{nx}](start = {{{}}}, each fixed = true) \"Continuous states of the FMU: {}\";",
                starts.join(", "),
                mo_string(&names.join(", "))
            );
            let _ = writeln!(s, "protected");
            let _ = writeln!(
                s,
                "  Real {DERIVATIVES}[{nx}] \"Derivatives of the FMU states (computed by the FMU)\";"
            );
            let _ = writeln!(s, "equation");
            let _ = writeln!(s, "  der({STATES}) = {DERIVATIVES};");
        }
        FmiKind::ModelExchange => {}
        FmiKind::CoSimulation => {
            let h = opts.step.unwrap_or_else(|| default_step(md));
            let _ = writeln!(
                s,
                "  parameter Real {STEP}(unit = \"s\", min = 0) = {} \"Communication step size (inputs are held over a step)\";",
                real(h)
            );
        }
    }
    let mut ann = format!(
        "{FMU_ANNOTATION}(fileName = \"{}\", fmiType = \"{}\", fmiVersion = \"{}\", modelName = \"{}\", guid = \"{}\")",
        mo_string(file_name),
        kind.as_str(),
        md.version_str(),
        mo_string(&md.model_name),
        mo_string(&md.token)
    );
    let _ = write!(
        ann,
        ",\n    Icon(graphics={{Rectangle(extent={{{{-100,-100}},{{100,100}}}}, lineColor={{0,0,127}}, fillColor={{255,255,255}}, fillPattern=FillPattern.Solid), Text(extent={{{{-90,60}},{{90,20}}}}, textString=\"FMU\", textColor={{0,0,127}}), Text(extent={{{{-90,0}},{{90,-40}}}}, textString=\"{}\", textColor={{0,0,0}}), Text(extent={{{{-150,150}},{{150,110}}}}, textString=\"%name\", textColor={{0,0,255}})}})",
        if kind == FmiKind::ModelExchange {
            "ME"
        } else {
            "CS"
        }
    );
    if opts.experiment {
        let e = &md.experiment;
        let mut parts = Vec::new();
        if let Some(t) = e.start {
            parts.push(format!("StartTime = {}", real(t)));
        }
        if let Some(t) = e.stop {
            parts.push(format!("StopTime = {}", real(t)));
        }
        if let Some(t) = e.tolerance {
            parts.push(format!("Tolerance = {}", real(t)));
        }
        if let Some(t) = e.step {
            parts.push(format!("Interval = {}", real(t)));
        }
        let _ = write!(ann, ",\n    experiment({})", parts.join(", "));
    }
    let _ = writeln!(s, "  annotation({ann});");
    let _ = writeln!(s, "end {class_name};");
    s
}

fn describe(name: &str, description: &str) -> String {
    if description.is_empty() {
        name.to_string()
    } else {
        format!("{description} ({name})")
    }
}
