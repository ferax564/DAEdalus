//! `modelDescription.xml` of FMI 2.0 and FMI 3.0 FMUs: the parts an importer needs.

use anyhow::{Context, Result, anyhow, bail};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FmiVersion {
    V2,
    V3,
}

/// Value type of a variable, as the C API reads and writes it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ty {
    /// FMI 2 `Real`, FMI 3 `Float64`.
    Float64,
    /// FMI 3 `Float32`.
    Float32,
    /// FMI 2 `Integer` / `Enumeration`, FMI 3 `Int32`.
    Int32,
    /// FMI 3 `Int64` / `Enumeration`.
    Int64,
    Boolean,
    /// Strings, binaries, clocks, other integer widths: not exchanged.
    Other,
}

impl Ty {
    pub fn is_real(self) -> bool {
        matches!(self, Ty::Float64 | Ty::Float32)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Causality {
    Parameter,
    CalculatedParameter,
    Input,
    Output,
    Local,
    Independent,
    StructuralParameter,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variability {
    Constant,
    Fixed,
    Tunable,
    Discrete,
    Continuous,
}

#[derive(Clone, Debug)]
pub struct Var {
    pub name: String,
    pub vr: u32,
    pub ty: Ty,
    pub causality: Causality,
    pub variability: Variability,
    pub start: Option<f64>,
    pub description: String,
    pub unit: String,
    /// The variable is an array (FMI 3 `<Dimension>`): not exchanged.
    pub is_array: bool,
}

#[derive(Clone, Debug)]
pub struct Interface {
    pub model_identifier: String,
    pub can_handle_variable_step: bool,
}

#[derive(Clone, Debug, Default)]
pub struct DefaultExperiment {
    pub start: Option<f64>,
    pub stop: Option<f64>,
    pub tolerance: Option<f64>,
    pub step: Option<f64>,
}

#[derive(Clone, Debug)]
pub struct ModelDescription {
    pub version: FmiVersion,
    pub model_name: String,
    pub description: String,
    /// FMI 2 `guid`, FMI 3 `instantiationToken`.
    pub token: String,
    pub generation_tool: String,
    pub me: Option<Interface>,
    pub cs: Option<Interface>,
    pub experiment: DefaultExperiment,
    pub vars: Vec<Var>,
    /// Continuous states, in state-vector order (indices into `vars`).
    pub states: Vec<usize>,
    /// Their derivatives, same order.
    pub derivatives: Vec<usize>,
    pub n_event_indicators: usize,
    /// Outputs (indices into `vars`) with their dependencies (indices into `vars`;
    /// `None` when not given: dependent on everything).
    pub outputs: Vec<(usize, Option<Vec<usize>>)>,
}

impl ModelDescription {
    pub fn parse(xml: &str) -> Result<ModelDescription> {
        let doc = roxmltree::Document::parse(xml).context("modelDescription.xml")?;
        let root = doc.root_element();
        if root.tag_name().name() != "fmiModelDescription" {
            bail!("not an FMI model description");
        }
        let ver = root.attribute("fmiVersion").unwrap_or("");
        let version = if ver.starts_with("2.") {
            FmiVersion::V2
        } else if ver.starts_with("3.") {
            FmiVersion::V3
        } else {
            bail!("unsupported FMI version '{ver}' (2.0 and 3.0 are supported)")
        };
        let token = match version {
            FmiVersion::V2 => root.attribute("guid"),
            FmiVersion::V3 => root.attribute("instantiationToken"),
        }
        .unwrap_or("")
        .to_string();
        let mut md = ModelDescription {
            version,
            model_name: root.attribute("modelName").unwrap_or("").to_string(),
            description: root.attribute("description").unwrap_or("").to_string(),
            token,
            generation_tool: root.attribute("generationTool").unwrap_or("").to_string(),
            me: None,
            cs: None,
            experiment: DefaultExperiment::default(),
            vars: Vec::new(),
            states: Vec::new(),
            derivatives: Vec::new(),
            n_event_indicators: root
                .attribute("numberOfEventIndicators")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            outputs: Vec::new(),
        };
        let num =
            |n: roxmltree::Node, a: &str| n.attribute(a).and_then(|s| s.trim().parse::<f64>().ok());
        let iface = |n: roxmltree::Node| Interface {
            model_identifier: n.attribute("modelIdentifier").unwrap_or("").to_string(),
            can_handle_variable_step: n.attribute("canHandleVariableCommunicationStepSize")
                == Some("true"),
        };
        let mut derivative_of: Vec<(usize, String)> = Vec::new();
        let mut model_vars = None;
        let mut structure = None;
        for n in root.children().filter(|n| n.is_element()) {
            match n.tag_name().name() {
                "ModelExchange" => md.me = Some(iface(n)),
                "CoSimulation" => md.cs = Some(iface(n)),
                "DefaultExperiment" => {
                    md.experiment = DefaultExperiment {
                        start: num(n, "startTime"),
                        stop: num(n, "stopTime"),
                        tolerance: num(n, "tolerance"),
                        step: num(n, "stepSize"),
                    }
                }
                "ModelVariables" => model_vars = Some(n),
                "ModelStructure" => structure = Some(n),
                _ => {}
            }
        }
        let Some(mv) = model_vars else {
            bail!("modelDescription.xml has no ModelVariables")
        };
        for v in mv.children().filter(|n| n.is_element()) {
            let (tyname, typed) = match version {
                FmiVersion::V2 => {
                    if v.tag_name().name() != "ScalarVariable" {
                        continue;
                    }
                    let Some(t) = v.children().find(|c| c.is_element()) else {
                        continue;
                    };
                    (t.tag_name().name(), t)
                }
                FmiVersion::V3 => (v.tag_name().name(), v),
            };
            let ty = match (version, tyname) {
                (FmiVersion::V2, "Real") | (FmiVersion::V3, "Float64") => Ty::Float64,
                (FmiVersion::V3, "Float32") => Ty::Float32,
                (FmiVersion::V2, "Integer" | "Enumeration") | (FmiVersion::V3, "Int32") => {
                    Ty::Int32
                }
                (FmiVersion::V3, "Int64" | "Enumeration") => Ty::Int64,
                (_, "Boolean") => Ty::Boolean,
                _ => Ty::Other,
            };
            let causality = match v.attribute("causality").unwrap_or("local") {
                "parameter" => Causality::Parameter,
                "calculatedParameter" => Causality::CalculatedParameter,
                "input" => Causality::Input,
                "output" => Causality::Output,
                "independent" => Causality::Independent,
                "structuralParameter" => Causality::StructuralParameter,
                _ => Causality::Local,
            };
            let variability = match v.attribute("variability") {
                Some("constant") => Variability::Constant,
                Some("fixed") => Variability::Fixed,
                Some("tunable") => Variability::Tunable,
                Some("discrete") => Variability::Discrete,
                Some("continuous") => Variability::Continuous,
                _ if ty.is_real() => Variability::Continuous,
                _ => Variability::Discrete,
            };
            let start = typed.attribute("start").and_then(|s| {
                let s = s.trim();
                match s {
                    "true" => Some(1.0),
                    "false" => Some(0.0),
                    _ => s.split_whitespace().next()?.parse().ok(),
                }
            });
            let name = v.attribute("name").unwrap_or("").to_string();
            let vr = v
                .attribute("valueReference")
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| anyhow!("variable '{name}' has no valueReference"))?;
            if let Some(d) = typed.attribute("derivative") {
                derivative_of.push((md.vars.len(), d.to_string()));
            }
            md.vars.push(Var {
                name,
                vr,
                ty,
                causality,
                variability,
                start,
                description: v.attribute("description").unwrap_or("").to_string(),
                unit: typed.attribute("unit").unwrap_or("").to_string(),
                is_array: typed.children().any(|c| c.tag_name().name() == "Dimension"),
            });
        }
        // References in the model structure: FMI 2 uses 1-based variable indices, FMI 3
        // value references (of Float64 etc. variables; take the first match).
        let by_ref = |s: &str| -> Option<usize> {
            match version {
                FmiVersion::V2 => s
                    .parse::<usize>()
                    .ok()
                    .and_then(|k| k.checked_sub(1))
                    .filter(|&k| k < md.vars.len()),
                FmiVersion::V3 => {
                    let vr: u32 = s.parse().ok()?;
                    md.vars.iter().position(|v| v.vr == vr)
                }
            }
        };
        let deps = |n: roxmltree::Node| -> Option<Vec<usize>> {
            n.attribute("dependencies")
                .map(|d| d.split_whitespace().filter_map(by_ref).collect())
        };
        if let Some(st) = structure {
            let key = match version {
                FmiVersion::V2 => "index",
                FmiVersion::V3 => "valueReference",
            };
            let push = |n: roxmltree::Node, out: &mut Vec<(usize, Option<Vec<usize>>)>| {
                if let Some(k) = n.attribute(key).and_then(by_ref) {
                    out.push((k, deps(n)));
                }
            };
            let mut outputs = Vec::new();
            let mut derivs = Vec::new();
            let mut indicators = Vec::new();
            match version {
                FmiVersion::V2 => {
                    for g in st.children().filter(|n| n.is_element()) {
                        let list = match g.tag_name().name() {
                            "Outputs" => &mut outputs,
                            "Derivatives" => &mut derivs,
                            _ => continue,
                        };
                        for u in g.children().filter(|n| n.has_tag_name("Unknown")) {
                            push(u, list);
                        }
                    }
                }
                FmiVersion::V3 => {
                    for u in st.children().filter(|n| n.is_element()) {
                        match u.tag_name().name() {
                            "Output" => push(u, &mut outputs),
                            "ContinuousStateDerivative" => push(u, &mut derivs),
                            "EventIndicator" => push(u, &mut indicators),
                            _ => {}
                        }
                    }
                    md.n_event_indicators = indicators.len();
                }
            }
            md.outputs = outputs;
            for (d, _) in derivs {
                let Some((_, of)) = derivative_of.iter().find(|(k, _)| *k == d) else {
                    bail!(
                        "derivative '{}' has no derivative attribute",
                        md.vars[d].name
                    );
                };
                let s =
                    by_ref(of).ok_or_else(|| anyhow!("derivative of unknown variable '{of}'"))?;
                md.derivatives.push(d);
                md.states.push(s);
            }
        }
        if md.me.is_none() && md.cs.is_none() {
            bail!("the FMU supports neither Model Exchange nor Co-Simulation");
        }
        Ok(md)
    }

    /// Whether any output depends directly on an input.
    pub fn feedthrough(&self) -> bool {
        let has_inputs = self.vars.iter().any(|v| v.causality == Causality::Input);
        has_inputs
            && self.outputs.iter().any(|(_, d)| match d {
                None => true,
                Some(d) => d
                    .iter()
                    .any(|&k| self.vars[k].causality == Causality::Input),
            })
    }

    pub fn version_str(&self) -> &'static str {
        match self.version {
            FmiVersion::V2 => "2.0",
            FmiVersion::V3 => "3.0",
        }
    }
}
