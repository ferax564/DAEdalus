//! SSP (System Structure and Parameterization 1.0) import and export.
//!
//! An `.ssp` archive holds `SystemStructure.ssd` (components, their connectors, the
//! connections and parameter bindings) and the FMUs of the components. Import turns
//! it into one Modelica model: a wrapper block per FMU (Co-Simulation by default, so
//! the simulator acts as a co-simulation master with a fixed communication step), an
//! instance per component with its parameter values, and a `connect` per connection.
//! Export does the reverse for a model composed of FMU wrapper instances.

use super::Fmu;
use super::md::{Causality, ModelDescription, Ty};
use super::wrapper::{self, Port, WrapperOptions, interface, wrapper_text};
use crate::external::FmiKind;
use anyhow::{Context, Result, anyhow, bail};
use std::collections::{HashMap, HashSet};
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const SSD_NS: &str = "http://ssp-standard.org/SSP1/SystemStructureDescription";
const SSC_NS: &str = "http://ssp-standard.org/SSP1/SystemStructureCommon";
const SSV_NS: &str = "http://ssp-standard.org/SSP1/SystemStructureParameterValues";

#[derive(Clone, Debug, Default)]
pub struct SsdComponent {
    pub name: String,
    /// FMU path inside the archive (`resources/A.fmu`).
    pub source: String,
    /// Parameter values by FMU variable name.
    pub params: Vec<(String, f64)>,
    /// `ElementGeometry` (x1, y1, x2, y2).
    pub geometry: Option<[f64; 4]>,
}

#[derive(Clone, Debug)]
pub struct SsdConnection {
    /// `None`: a connector of the system itself.
    pub start_element: Option<String>,
    pub start_connector: String,
    pub end_element: Option<String>,
    pub end_connector: String,
}

#[derive(Clone, Debug, Default)]
pub struct Ssd {
    pub name: String,
    pub components: Vec<SsdComponent>,
    pub connections: Vec<SsdConnection>,
    /// System-level connectors: (name, kind) with kind `input`, `output`, ...
    pub connectors: Vec<(String, String)>,
    pub start_time: Option<f64>,
    pub stop_time: Option<f64>,
}

fn child<'a, 'i>(n: roxmltree::Node<'a, 'i>, name: &str) -> Option<roxmltree::Node<'a, 'i>> {
    n.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn children<'a, 'i>(
    n: roxmltree::Node<'a, 'i>,
    name: &'static str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'i>> {
    n.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

/// Parameter values of an `ssv:ParameterSet` (inline or file contents).
fn parameter_set(set: roxmltree::Node) -> Vec<(String, f64)> {
    let mut out = Vec::new();
    let Some(ps) = child(set, "Parameters") else {
        return out;
    };
    for p in children(ps, "Parameter") {
        let name = p.attribute("name").unwrap_or("").to_string();
        let Some(v) = p.children().find(|c| c.is_element()) else {
            continue;
        };
        let val = v.attribute("value").and_then(|s| match s.trim() {
            "true" => Some(1.0),
            "false" => Some(0.0),
            s => s.parse::<f64>().ok(),
        });
        if let Some(x) = val {
            out.push((name, x));
        }
    }
    out
}

/// Parameter bindings under `n` (`ssd:ParameterBindings`); `dir` resolves `.ssv` files.
fn bindings(n: roxmltree::Node, dir: &Path) -> Result<Vec<(String, f64)>> {
    let mut out = Vec::new();
    let Some(pb) = child(n, "ParameterBindings") else {
        return Ok(out);
    };
    for b in children(pb, "ParameterBinding") {
        let prefix = b.attribute("prefix").unwrap_or("");
        let mut vals = Vec::new();
        if let Some(src) = b.attribute("source") {
            let path = dir.join(src);
            let text = std::fs::read_to_string(&path)
                .with_context(|| format!("parameter values {}", path.display()))?;
            let doc = roxmltree::Document::parse(&text)
                .with_context(|| format!("parameter values {}", path.display()))?;
            vals = parameter_set(doc.root_element());
        } else if let Some(pv) = child(b, "ParameterValues")
            && let Some(set) = child(pv, "ParameterSet")
        {
            vals = parameter_set(set);
        }
        for (k, v) in vals {
            let k = k.strip_prefix(prefix).unwrap_or(&k).to_string();
            out.push((k, v));
        }
    }
    Ok(out)
}

/// Read `SystemStructure.ssd`; `dir` is the unpacked archive (for `.ssv` files).
pub fn read_ssd(xml: &str, dir: &Path) -> Result<Ssd> {
    let doc = roxmltree::Document::parse(xml).context("SystemStructure.ssd")?;
    let root = doc.root_element();
    if root.tag_name().name() != "SystemStructureDescription" {
        bail!("not an SSP system structure description");
    }
    let sys = child(root, "System").ok_or_else(|| anyhow!("the SSD has no System"))?;
    let mut ssd = Ssd {
        name: root
            .attribute("name")
            .or_else(|| sys.attribute("name"))
            .unwrap_or("System")
            .to_string(),
        ..Default::default()
    };
    if let Some(e) = child(root, "DefaultExperiment") {
        ssd.start_time = e.attribute("startTime").and_then(|s| s.parse().ok());
        ssd.stop_time = e.attribute("stopTime").and_then(|s| s.parse().ok());
    }
    if let Some(cs) = child(sys, "Connectors") {
        for c in children(cs, "Connector") {
            ssd.connectors.push((
                c.attribute("name").unwrap_or("").to_string(),
                c.attribute("kind").unwrap_or("").to_string(),
            ));
        }
    }
    // System-level bindings `comp.param`.
    let sys_params = bindings(sys, dir)?;
    if let Some(els) = child(sys, "Elements") {
        for c in els.children().filter(|c| c.is_element()) {
            match c.tag_name().name() {
                "Component" => {}
                "System" => bail!("nested sub-systems are not supported"),
                _ => continue,
            }
            let ty = c
                .attribute("type")
                .unwrap_or("application/x-fmu-sharedlibrary");
            if ty != "application/x-fmu-sharedlibrary" {
                bail!(
                    "component '{}' has type {ty}: only FMU components are supported",
                    c.attribute("name").unwrap_or("")
                );
            }
            let name = c.attribute("name").unwrap_or("").to_string();
            let mut params = bindings(c, dir)?;
            let pre = format!("{name}.");
            for (k, v) in &sys_params {
                if let Some(rest) = k.strip_prefix(&pre) {
                    params.push((rest.to_string(), *v));
                }
            }
            let geometry = child(c, "ElementGeometry").and_then(|g| {
                let f = |a| g.attribute(a).and_then(|s: &str| s.parse::<f64>().ok());
                Some([f("x1")?, f("y1")?, f("x2")?, f("y2")?])
            });
            ssd.components.push(SsdComponent {
                name,
                source: c.attribute("source").unwrap_or("").to_string(),
                params,
                geometry,
            });
        }
    }
    if let Some(cs) = child(sys, "Connections") {
        for c in children(cs, "Connection") {
            ssd.connections.push(SsdConnection {
                start_element: c.attribute("startElement").map(str::to_string),
                start_connector: c.attribute("startConnector").unwrap_or("").to_string(),
                end_element: c.attribute("endElement").map(str::to_string),
                end_connector: c.attribute("endConnector").unwrap_or("").to_string(),
            });
        }
    }
    Ok(ssd)
}

/// An imported SSP: the generated Modelica model and the directory it was unpacked
/// into (removed when dropped, unless kept).
pub struct ImportedSsp {
    pub model: String,
    pub text: String,
    pub dir: PathBuf,
    /// FMU files referenced by the model, relative to `dir` as in the archive.
    pub fmus: Vec<String>,
    keep: bool,
}

impl ImportedSsp {
    /// Keep the unpacked directory after this value is dropped.
    pub fn keep(&mut self) {
        self.keep = true;
    }
}

impl Drop for ImportedSsp {
    fn drop(&mut self) {
        if !self.keep {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct SspImportOptions {
    /// FMI type for the components (default Co-Simulation where the FMU supports it).
    pub kind: Option<FmiKind>,
    /// Communication step of Co-Simulation components (default: span / 500).
    pub step: Option<f64>,
    /// FMU files are referenced relative to the generated `.mo` file (as in the
    /// archive) instead of by absolute path into the unpacked directory.
    pub relative: bool,
}

fn mo_string(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn num(x: f64) -> String {
    format!("{x:?}")
}

/// Unpack `ssp` and generate the Modelica model of its system.
pub fn import(ssp: &Path, opts: &SspImportOptions) -> Result<ImportedSsp> {
    let stem = ssp.file_stem().and_then(|s| s.to_str()).unwrap_or("ssp");
    let dir = super::temp_dir(&format!("ssp-{stem}"))?;
    let mut out = ImportedSsp {
        model: String::new(),
        text: String::new(),
        dir: dir.clone(),
        fmus: Vec::new(),
        keep: false,
    };
    super::unzip(ssp, &dir)?;
    let xml = std::fs::read_to_string(dir.join("SystemStructure.ssd"))
        .with_context(|| format!("{}: no SystemStructure.ssd", ssp.display()))?;
    let ssd = read_ssd(&xml, &dir)?;
    let model = wrapper::ident(&ssd.name);
    let span = match (ssd.start_time, ssd.stop_time) {
        (Some(a), Some(b)) if b > a => b - a,
        (None, Some(b)) if b > 0.0 => b,
        _ => 1.0,
    };
    let step = opts.step.unwrap_or(span / 500.0);
    // One wrapper class per FMU file.
    let mut classes: HashMap<String, (String, ModelDescription, FmiKind)> = HashMap::new();
    let mut class_names: HashSet<String> = HashSet::new();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "model {model} \"SSP system {} (imported from {})\"",
        mo_string(&ssd.name),
        mo_string(&ssp.file_name().unwrap_or_default().to_string_lossy())
    );
    for c in &ssd.components {
        if classes.contains_key(&c.source) {
            continue;
        }
        let path = dir.join(&c.source);
        let fmu =
            Fmu::open(&path).with_context(|| format!("component '{}' ({})", c.name, c.source))?;
        let md = fmu.md.clone();
        let kind = match opts.kind {
            Some(k) => k,
            None if md.cs.is_some() => FmiKind::CoSimulation,
            None => FmiKind::ModelExchange,
        };
        let base = wrapper::ident(
            Path::new(&c.source)
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("FMU"),
        );
        let mut cname = format!("{base}_FMU");
        let mut n = 2;
        while class_names.contains(&cname) {
            cname = format!("{base}_FMU{n}");
            n += 1;
        }
        class_names.insert(cname.clone());
        let file = if opts.relative {
            c.source.clone()
        } else {
            path.to_string_lossy().into_owned()
        };
        let text = wrapper_text(
            &md,
            &cname,
            &file,
            kind,
            &WrapperOptions {
                step: Some(step),
                ..Default::default()
            },
        );
        for l in text.lines() {
            let _ = writeln!(s, "  {l}");
        }
        out.fmus.push(c.source.clone());
        classes.insert(c.source.clone(), (cname, md, kind));
    }
    // Instance names and port lookup.
    let mut inst_of: HashMap<&str, String> = HashMap::new();
    let mut used: HashSet<String> = class_names.clone();
    used.insert(model.clone());
    for c in &ssd.components {
        let base = wrapper::ident(&c.name);
        let mut id = base.clone();
        let mut n = 2;
        while used.contains(&id) {
            id = format!("{base}_{n}");
            n += 1;
        }
        used.insert(id.clone());
        inst_of.insert(c.name.as_str(), id);
    }
    let comp_of: HashMap<&str, &SsdComponent> = ssd
        .components
        .iter()
        .map(|c| (c.name.as_str(), c))
        .collect();
    let ports = |src: &str| -> (Vec<Port>, Vec<Port>, Vec<Port>) {
        let md = &classes[src].1;
        let i = interface(md);
        (i.params, i.inputs, i.outputs)
    };
    let port_ident = |el: &str, conn: &str| -> Result<(String, bool)> {
        let c = comp_of
            .get(el)
            .ok_or_else(|| anyhow!("connection to unknown element '{el}'"))?;
        let md = &classes[&c.source].1;
        let (_, ins, outs) = ports(&c.source);
        for (list, is_out) in [(&outs, true), (&ins, false)] {
            if let Some(p) = list.iter().find(|p| md.vars[p.var].name == conn) {
                return Ok((format!("{}.{}", inst_of[el], p.ident), is_out));
            }
        }
        bail!("element '{el}' has no input or output '{conn}'")
    };
    let mut sys_vars: HashMap<&str, String> = HashMap::new();
    for (name, kind) in &ssd.connectors {
        let id = wrapper::ident(name);
        let dir_kw = match kind.as_str() {
            "input" => "input",
            "output" => "output",
            _ => continue,
        };
        let _ = writeln!(s, "  {dir_kw} Real {id} \"{}\";", mo_string(name));
        sys_vars.insert(name.as_str(), id);
    }
    let mut eqs = Vec::new();
    let mut connected: HashSet<String> = HashSet::new();
    for c in &ssd.connections {
        let a = match &c.start_element {
            Some(e) => port_ident(e, &c.start_connector)?,
            None => (
                sys_vars
                    .get(c.start_connector.as_str())
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown system connector '{}'", c.start_connector))?,
                false,
            ),
        };
        let b = match &c.end_element {
            Some(e) => port_ident(e, &c.end_connector)?,
            None => (
                sys_vars
                    .get(c.end_connector.as_str())
                    .cloned()
                    .ok_or_else(|| anyhow!("unknown system connector '{}'", c.end_connector))?,
                false,
            ),
        };
        if c.start_element.is_some() && c.end_element.is_some() {
            let (src, dst) = if a.1 { (&a.0, &b.0) } else { (&b.0, &a.0) };
            connected.insert(dst.clone());
            eqs.push(format!("connect({src}, {dst});"));
        } else if c.start_element.is_none() {
            // system input -> component input
            connected.insert(b.0.clone());
            eqs.push(format!("{} = {};", b.0, a.0));
        } else {
            eqs.push(format!("{} = {};", b.0, a.0));
        }
    }
    for c in &ssd.components {
        let (cname, md, _) = &classes[&c.source];
        let (params, ins, _) = ports(&c.source);
        let inst = &inst_of[c.name.as_str()];
        let mut mods = Vec::new();
        for (k, v) in &c.params {
            if let Some(p) = params.iter().find(|p| &md.vars[p.var].name == k) {
                mods.push(format!("{} = {}", p.ident, value(md.vars[p.var].ty, *v)));
            }
        }
        // Inputs nothing drives keep their start values.
        for p in &ins {
            if !connected.contains(&format!("{inst}.{}", p.ident)) {
                let v = &md.vars[p.var];
                mods.push(format!(
                    "{} = {}",
                    p.ident,
                    value(v.ty, v.start.unwrap_or(0.0))
                ));
            }
        }
        let m = if mods.is_empty() {
            String::new()
        } else {
            format!("({})", mods.join(", "))
        };
        let place = c
            .geometry
            .map(|[x1, y1, x2, y2]| {
                format!(
                    " annotation(Placement(transformation(extent={{{{{},{}}},{{{},{}}}}})))",
                    num(x1),
                    num(-y2),
                    num(x2),
                    num(-y1)
                )
            })
            .unwrap_or_default();
        let _ = writeln!(s, "  {cname} {inst}{m} \"{}\"{place};", mo_string(&c.name));
    }
    let _ = writeln!(s, "equation");
    for e in eqs {
        let _ = writeln!(s, "  {e}");
    }
    let mut exp = Vec::new();
    if let Some(t) = ssd.start_time {
        exp.push(format!("StartTime = {}", num(t)));
    }
    if let Some(t) = ssd.stop_time {
        exp.push(format!("StopTime = {}", num(t)));
    }
    exp.push(format!("Interval = {}", num(step)));
    let _ = writeln!(s, "  annotation(experiment({}));", exp.join(", "));
    let _ = writeln!(s, "end {model};");
    out.model = model;
    out.text = s;
    Ok(out)
}

fn value(t: Ty, x: f64) -> String {
    match t {
        Ty::Boolean => (x != 0.0).to_string(),
        Ty::Int32 | Ty::Int64 => format!("{}", x.round() as i64),
        _ => num(x),
    }
}

/// A component of a model to export: an FMU wrapper instance.
pub struct ExportComponent {
    pub name: String,
    pub fmu: PathBuf,
    /// Parameter values by wrapper identifier.
    pub params: HashMap<String, f64>,
    pub placement: Option<[f64; 4]>,
}

/// One connection `from -> to` between wrapper ports (`comp.port`).
pub struct ExportConnection {
    pub from: String,
    pub to: String,
}

fn ssc_type(t: Ty) -> &'static str {
    match t {
        Ty::Int32 | Ty::Int64 => "ssc:Integer",
        Ty::Boolean => "ssc:Boolean",
        _ => "ssc:Real",
    }
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Write the SSP of a system of FMU components to `out`.
pub fn export(
    name: &str,
    comps: &[ExportComponent],
    conns: &[ExportConnection],
    start: Option<f64>,
    stop: Option<f64>,
    out: &Path,
) -> Result<()> {
    let mut mds: Vec<ModelDescription> = Vec::new();
    let mut files: Vec<(String, PathBuf)> = Vec::new();
    let mut s = String::new();
    let _ = writeln!(s, r#"<?xml version="1.0" encoding="UTF-8"?>"#);
    let _ = writeln!(
        s,
        r#"<ssd:SystemStructureDescription xmlns:ssc="{SSC_NS}" xmlns:ssd="{SSD_NS}" xmlns:ssv="{SSV_NS}" version="1.0" name="{}" generationTool="DAEdalus {}">"#,
        xml(name),
        env!("CARGO_PKG_VERSION")
    );
    let _ = writeln!(s, r#"  <ssd:System name="{}">"#, xml(name));
    let _ = writeln!(s, "    <ssd:Elements>");
    for c in comps {
        let fmu = Fmu::open(&c.fmu).with_context(|| format!("component '{}'", c.name))?;
        let md = fmu.md.clone();
        let file_name = c
            .fmu
            .file_name()
            .map(|f| f.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("{}.fmu", c.name));
        let src = match files.iter().find(|(_, p)| p == &c.fmu) {
            Some((s, _)) => s.clone(),
            None => {
                let mut s = format!("resources/{file_name}");
                let mut n = 2;
                while files.iter().any(|(x, _)| x == &s) {
                    s = format!("resources/{n}_{file_name}");
                    n += 1;
                }
                files.push((s.clone(), c.fmu.clone()));
                s
            }
        };
        let _ = writeln!(
            s,
            r#"      <ssd:Component name="{}" type="application/x-fmu-sharedlibrary" source="{}">"#,
            xml(&c.name),
            xml(&src)
        );
        let itf = interface(&md);
        let _ = writeln!(s, "        <ssd:Connectors>");
        for (list, kind) in [
            (&itf.inputs, "input"),
            (&itf.outputs, "output"),
            (&itf.params, "parameter"),
        ] {
            for p in list.iter() {
                let v = &md.vars[p.var];
                let unit = if v.ty.is_real() && !v.unit.is_empty() {
                    format!(r#" unit="{}""#, xml(&v.unit))
                } else {
                    String::new()
                };
                let _ = writeln!(
                    s,
                    r#"          <ssd:Connector name="{}" kind="{kind}"><{}{unit}/></ssd:Connector>"#,
                    xml(&v.name),
                    ssc_type(v.ty)
                );
            }
        }
        let _ = writeln!(s, "        </ssd:Connectors>");
        if let Some([x1, y1, x2, y2]) = c.placement {
            let _ = writeln!(
                s,
                r#"        <ssd:ElementGeometry x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
                num(x1.min(x2)),
                num(-y1.max(y2)),
                num(x1.max(x2)),
                num(-y1.min(y2))
            );
        }
        let vals: Vec<(&Port, f64)> = itf
            .params
            .iter()
            .filter_map(|p| c.params.get(&p.ident).map(|&v| (p, v)))
            .collect();
        if !vals.is_empty() {
            let _ = writeln!(s, "        <ssd:ParameterBindings>");
            let _ = writeln!(s, "          <ssd:ParameterBinding>");
            let _ = writeln!(s, "            <ssd:ParameterValues>");
            let _ = writeln!(
                s,
                r#"              <ssv:ParameterSet version="1.0" name="{}">"#,
                xml(&c.name)
            );
            let _ = writeln!(s, "                <ssv:Parameters>");
            for (p, v) in vals {
                let var = &md.vars[p.var];
                let (tag, val) = match var.ty {
                    Ty::Boolean => ("ssv:Boolean", (v != 0.0).to_string()),
                    Ty::Int32 | Ty::Int64 => ("ssv:Integer", format!("{}", v.round() as i64)),
                    _ => ("ssv:Real", num(v)),
                };
                let _ = writeln!(
                    s,
                    r#"                  <ssv:Parameter name="{}"><{tag} value="{val}"/></ssv:Parameter>"#,
                    xml(&var.name)
                );
            }
            let _ = writeln!(s, "                </ssv:Parameters>");
            let _ = writeln!(s, "              </ssv:ParameterSet>");
            let _ = writeln!(s, "            </ssd:ParameterValues>");
            let _ = writeln!(s, "          </ssd:ParameterBinding>");
            let _ = writeln!(s, "        </ssd:ParameterBindings>");
        }
        let _ = writeln!(s, "      </ssd:Component>");
        mds.push(md);
    }
    let _ = writeln!(s, "    </ssd:Elements>");
    // Map `comp.ident` to (component, FMU variable name, is output).
    let resolve = |port: &str| -> Result<(String, String, bool)> {
        let (comp, id) = port
            .split_once('.')
            .ok_or_else(|| anyhow!("connection '{port}' is not between FMU components"))?;
        let k = comps
            .iter()
            .position(|c| c.name == comp)
            .ok_or_else(|| anyhow!("'{comp}' in connection '{port}' is not an FMU component"))?;
        let md = &mds[k];
        let itf = interface(md);
        for (list, out) in [(&itf.outputs, true), (&itf.inputs, false)] {
            if let Some(p) = list.iter().find(|p| p.ident == id) {
                return Ok((comp.to_string(), md.vars[p.var].name.clone(), out));
            }
        }
        bail!("'{port}' is not an input or output of the FMU")
    };
    if !conns.is_empty() {
        let _ = writeln!(s, "    <ssd:Connections>");
        for c in conns {
            let a = resolve(&c.from)?;
            let b = resolve(&c.to)?;
            let (src, dst) = if a.2 || !b.2 { (a, b) } else { (b, a) };
            let _ = writeln!(
                s,
                r#"      <ssd:Connection startElement="{}" startConnector="{}" endElement="{}" endConnector="{}"/>"#,
                xml(&src.0),
                xml(&src.1),
                xml(&dst.0),
                xml(&dst.1)
            );
        }
        let _ = writeln!(s, "    </ssd:Connections>");
    }
    let _ = writeln!(s, "  </ssd:System>");
    let mut exp = String::new();
    if let Some(t) = start {
        let _ = write!(exp, r#" startTime="{}""#, num(t));
    }
    if let Some(t) = stop {
        let _ = write!(exp, r#" stopTime="{}""#, num(t));
    }
    if !exp.is_empty() {
        let _ = writeln!(s, "  <ssd:DefaultExperiment{exp}/>");
    }
    let _ = writeln!(s, "</ssd:SystemStructureDescription>");
    let f = std::fs::File::create(out).with_context(|| out.display().to_string())?;
    let mut z = zip::ZipWriter::new(std::io::BufWriter::new(f));
    let o = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o644);
    z.start_file("SystemStructure.ssd", o)?;
    z.write_all(s.as_bytes())?;
    z.add_directory("resources/", o)?;
    for (name, path) in &files {
        let bytes = std::fs::read(path).with_context(|| path.display().to_string())?;
        // FMUs are zip files already: store them.
        z.start_file(
            name.as_str(),
            o.compression_method(zip::CompressionMethod::Stored),
        )?;
        z.write_all(&bytes)?;
    }
    z.finish()?.flush()?;
    Ok(())
}

/// Parameters of `md` a wrapper exposes, by identifier (for callers collecting values).
pub fn parameter_idents(md: &ModelDescription) -> Vec<String> {
    interface(md)
        .params
        .into_iter()
        .filter(|p| md.vars[p.var].causality == Causality::Parameter)
        .map(|p| p.ident)
        .collect()
}
