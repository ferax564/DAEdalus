//! FMI 2.0 model description of a compiled model: which variables an FMU exposes,
//! their value references, causality, variability and `initial`, and the
//! `modelDescription.xml` text. The FMU binary (`daedalus-fmi`) and the exporter
//! (`daedalus fmu`) both derive the variable table from here, so they agree.
//!
//! The value reference of a variable is its index in [`Model::vars`]; every variable
//! has its own register, so there are no aliases in the FMI sense.

use crate::codegen::{Model, Runtime};
use crate::expr::VarId;
use crate::flat::{VarKind, VarType};
use std::fmt::Write;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Causality {
    Parameter,
    CalculatedParameter,
    Input,
    Output,
    Local,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variability {
    Fixed,
    Discrete,
    Continuous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Initial {
    Exact,
    Calculated,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BaseType {
    Real,
    Integer,
    Boolean,
}

#[derive(Clone, Debug)]
pub struct ScalarVariable {
    pub vr: u32,
    pub name: String,
    pub description: String,
    pub unit: String,
    pub causality: Causality,
    pub variability: Variability,
    pub initial: Option<Initial>,
    pub ty: BaseType,
    pub start: Option<f64>,
    /// 1-based index (in the variable list) of the state this is the derivative of.
    pub derivative: Option<usize>,
}

/// The variables of an FMU and its model structure (indices are 1-based positions in
/// `vars`, as in `modelDescription.xml`).
#[derive(Clone, Debug)]
pub struct Description {
    pub vars: Vec<ScalarVariable>,
    pub outputs: Vec<usize>,
    pub derivatives: Vec<usize>,
    pub initial_unknowns: Vec<usize>,
    pub n_states: usize,
    pub n_event_indicators: usize,
}

fn base_type(ty: VarType) -> BaseType {
    match ty {
        VarType::Real => BaseType::Real,
        VarType::Integer | VarType::Enumeration => BaseType::Integer,
        VarType::Boolean => BaseType::Boolean,
    }
}

/// Values of the parameters and inputs (their start values) as evaluated by the model.
pub fn parameter_values(m: &Model) -> Vec<f64> {
    let mut rt = Runtime::new(m);
    for (_, seg) in &m.params {
        rt.run(seg);
    }
    rt.r[..m.vars.len()].to_vec()
}

/// Build the FMI variable table of `m`.
pub fn describe(m: &Model) -> Description {
    let values = parameter_values(m);
    let is_param: Vec<bool> = {
        let mut p = vec![false; m.vars.len()];
        for &v in &m.param_vars {
            p[v as usize] = true;
        }
        p
    };
    let free: Vec<bool> = {
        let mut p = vec![false; m.vars.len()];
        for &v in &m.free_params {
            p[v as usize] = true;
        }
        p
    };
    let state_of_deriv: rustc_hash::FxHashMap<VarId, usize> =
        m.derivs.iter().enumerate().map(|(i, &d)| (d, i)).collect();
    let mut vars = Vec::new();
    let mut index_of: rustc_hash::FxHashMap<VarId, usize> = Default::default();
    for (i, v) in m.vars.iter().enumerate() {
        let id = i as VarId;
        let hidden = v.name.starts_with('$');
        if v.kind == VarKind::Constant || (hidden && !state_of_deriv.contains_key(&id)) {
            continue;
        }
        let ty = base_type(v.ty);
        let (causality, variability, initial, start) = match v.kind {
            VarKind::Parameter if free[i] && !v.is_final && is_param[i] => (
                Causality::Parameter,
                Variability::Fixed,
                Some(Initial::Exact),
                Some(values[i]),
            ),
            VarKind::Parameter => (
                Causality::CalculatedParameter,
                Variability::Fixed,
                Some(Initial::Calculated),
                None,
            ),
            VarKind::Input => (
                Causality::Input,
                if ty == BaseType::Real {
                    Variability::Continuous
                } else {
                    Variability::Discrete
                },
                None,
                Some(values[i]),
            ),
            _ => (
                if v.is_output {
                    Causality::Output
                } else {
                    Causality::Local
                },
                if ty == BaseType::Real && v.kind == VarKind::Continuous {
                    Variability::Continuous
                } else {
                    Variability::Discrete
                },
                Some(Initial::Calculated),
                None,
            ),
        };
        let name = if hidden {
            let s = state_of_deriv[&id];
            format!("der({})", m.vars[m.states[s] as usize].name)
        } else {
            v.name.clone()
        };
        index_of.insert(id, vars.len() + 1);
        vars.push(ScalarVariable {
            vr: id,
            name,
            description: v.description.clone(),
            unit: if ty == BaseType::Real {
                v.unit.clone()
            } else {
                String::new()
            },
            causality,
            variability,
            initial,
            ty,
            start,
            derivative: None,
        });
    }
    let mut derivatives = Vec::new();
    for (s, &d) in m.derivs.iter().enumerate() {
        let state = index_of[&m.states[s]];
        let k = index_of[&d];
        vars[k - 1].derivative = Some(state);
        derivatives.push(k);
    }
    let outputs: Vec<usize> = vars
        .iter()
        .enumerate()
        .filter(|(_, v)| v.causality == Causality::Output)
        .map(|(k, _)| k + 1)
        .collect();
    let mut initial_unknowns: Vec<usize> = outputs.clone();
    initial_unknowns.extend(
        vars.iter()
            .enumerate()
            .filter(|(_, v)| v.causality == Causality::CalculatedParameter)
            .map(|(k, _)| k + 1),
    );
    for (s, &k) in derivatives.iter().enumerate() {
        initial_unknowns.push(k);
        initial_unknowns.push(index_of[&m.states[s]]);
    }
    initial_unknowns.sort_unstable();
    initial_unknowns.dedup();
    Description {
        vars,
        outputs,
        derivatives,
        initial_unknowns,
        n_states: m.states.len(),
        n_event_indicators: m.layout.n_rel,
    }
}

fn xml_escape(s: &str) -> String {
    let mut o = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => o.push_str("&amp;"),
            '<' => o.push_str("&lt;"),
            '>' => o.push_str("&gt;"),
            '"' => o.push_str("&quot;"),
            '\'' => o.push_str("&apos;"),
            c if (c as u32) < 0x20 && c != '\t' => o.push(' '),
            c => o.push(c),
        }
    }
    o
}

fn real(v: f64) -> String {
    if v.is_finite() {
        format!("{v:?}")
    } else if v.is_nan() {
        "0.0".into()
    } else if v > 0.0 {
        "1.7976931348623157e308".into()
    } else {
        "-1.7976931348623157e308".into()
    }
}

/// `modelDescription.xml` (FMI 2.0, Model Exchange and Co-Simulation).
pub fn model_description_xml(
    m: &Model,
    d: &Description,
    model_identifier: &str,
    guid: &str,
    generation_tool: &str,
) -> String {
    let mut s = String::new();
    let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        s,
        r#"<fmiModelDescription fmiVersion="2.0" modelName="{}" guid="{}" description="{}" generationTool="{}" variableNamingConvention="flat" numberOfEventIndicators="{}">"#,
        xml_escape(&m.name),
        xml_escape(guid),
        xml_escape(&format!("{} (DAEdalus)", m.name)),
        xml_escape(generation_tool),
        d.n_event_indicators
    );
    let _ = writeln!(
        s,
        r#"  <ModelExchange modelIdentifier="{model_identifier}" canNotUseMemoryManagementFunctions="true" canGetAndSetFMUstate="false" canSerializeFMUstate="false" providesDirectionalDerivative="false"/>"#
    );
    let _ = writeln!(
        s,
        r#"  <CoSimulation modelIdentifier="{model_identifier}" canHandleVariableCommunicationStepSize="true" canInterpolateInputs="false" maxOutputDerivativeOrder="0" canRunAsynchronuously="false" canBeInstantiatedOnlyOncePerProcess="false" canNotUseMemoryManagementFunctions="true" canGetAndSetFMUstate="false" canSerializeFMUstate="false" providesDirectionalDerivative="false"/>"#
    );
    let mut units: Vec<&str> = d
        .vars
        .iter()
        .map(|v| v.unit.as_str())
        .filter(|u| !u.is_empty())
        .collect();
    units.sort_unstable();
    units.dedup();
    if !units.is_empty() {
        let _ = writeln!(s, "  <UnitDefinitions>");
        for u in units {
            let _ = writeln!(s, r#"    <Unit name="{}"/>"#, xml_escape(u));
        }
        let _ = writeln!(s, "  </UnitDefinitions>");
    }
    let e = &m.experiment;
    let mut exp = String::new();
    if let Some(t) = e.start_time {
        let _ = write!(exp, r#" startTime="{}""#, real(t));
    }
    if let Some(t) = e.stop_time {
        let _ = write!(exp, r#" stopTime="{}""#, real(t));
    }
    if let Some(t) = e.tolerance {
        let _ = write!(exp, r#" tolerance="{}""#, real(t));
    }
    if let Some(t) = e.interval {
        let _ = write!(exp, r#" stepSize="{}""#, real(t));
    }
    if !exp.is_empty() {
        let _ = writeln!(s, "  <DefaultExperiment{exp}/>");
    }
    let _ = writeln!(s, "  <ModelVariables>");
    for (k, v) in d.vars.iter().enumerate() {
        let _ = writeln!(s, "    <!-- {} -->", k + 1);
        let _ = write!(
            s,
            r#"    <ScalarVariable name="{}" valueReference="{}""#,
            xml_escape(&v.name),
            v.vr
        );
        if !v.description.is_empty() {
            let _ = write!(s, r#" description="{}""#, xml_escape(&v.description));
        }
        let _ = write!(
            s,
            r#" causality="{}" variability="{}""#,
            match v.causality {
                Causality::Parameter => "parameter",
                Causality::CalculatedParameter => "calculatedParameter",
                Causality::Input => "input",
                Causality::Output => "output",
                Causality::Local => "local",
            },
            match v.variability {
                Variability::Fixed => "fixed",
                Variability::Discrete => "discrete",
                Variability::Continuous => "continuous",
            }
        );
        if let Some(i) = v.initial {
            let _ = write!(
                s,
                r#" initial="{}""#,
                match i {
                    Initial::Exact => "exact",
                    Initial::Calculated => "calculated",
                }
            );
        }
        let _ = writeln!(s, ">");
        let mut attrs = String::new();
        if let Some(x) = v.start {
            let _ = write!(
                attrs,
                r#" start="{}""#,
                match v.ty {
                    BaseType::Real => real(x),
                    BaseType::Integer => format!("{}", x.round() as i64),
                    BaseType::Boolean => (x != 0.0).to_string(),
                }
            );
        }
        if !v.unit.is_empty() {
            let _ = write!(attrs, r#" unit="{}""#, xml_escape(&v.unit));
        }
        if let Some(st) = v.derivative {
            let _ = write!(attrs, r#" derivative="{st}""#);
        }
        let _ = writeln!(
            s,
            "      <{}{attrs}/>",
            match v.ty {
                BaseType::Real => "Real",
                BaseType::Integer => "Integer",
                BaseType::Boolean => "Boolean",
            }
        );
        let _ = writeln!(s, "    </ScalarVariable>");
    }
    let _ = writeln!(s, "  </ModelVariables>");
    let _ = writeln!(s, "  <ModelStructure>");
    for (tag, list) in [
        ("Outputs", &d.outputs),
        ("Derivatives", &d.derivatives),
        ("InitialUnknowns", &d.initial_unknowns),
    ] {
        if list.is_empty() {
            continue;
        }
        let _ = writeln!(s, "    <{tag}>");
        for k in list {
            let _ = writeln!(s, r#"      <Unknown index="{k}"/>"#);
        }
        let _ = writeln!(s, "    </{tag}>");
    }
    let _ = writeln!(s, "  </ModelStructure>");
    let _ = writeln!(s, "</fmiModelDescription>");
    s
}

/// A stable identifier for a model: 128 bits of FNV-1a over `bytes`, formatted as a GUID.
pub fn guid_of(bytes: &[u8]) -> String {
    let mut h1: u64 = 0xcbf29ce484222325;
    let mut h2: u64 = 0x84222325cbf29ce4;
    for &b in bytes {
        h1 = (h1 ^ b as u64).wrapping_mul(0x100000001b3);
        h2 = (h2 ^ (b as u64).rotate_left(3)).wrapping_mul(0x100000001b3 ^ 0x5bd1e995);
    }
    format!(
        "{{{:08x}-{:04x}-{:04x}-{:04x}-{:012x}}}",
        h1 >> 32,
        (h1 >> 16) & 0xffff,
        h1 & 0xffff,
        h2 >> 48,
        h2 & 0xffff_ffff_ffff
    )
}

/// What an FMU carries in `resources/model.bin` (bincode): the compiled model and the
/// GUID of its model description.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct Bundle {
    pub guid: String,
    pub model: Model,
}

/// A C identifier made from a model name (`A.B.C` -> `A_B_C`).
pub fn model_identifier(name: &str) -> String {
    let mut s: String = name
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect();
    if s.chars().next().is_none_or(|c| c.is_ascii_digit()) {
        s.insert(0, '_');
    }
    s
}
