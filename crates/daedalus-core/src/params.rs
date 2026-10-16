//! Parameter data from tables: CSV (`name,value[,unit]`) or SSV (the SSP standard's
//! parameter values, `.ssv`) turned into a Modelica modifier or record.
//!
//! Dotted names become nested modifiers (`body.m` → `body(m=...)`). When the class the
//! values are meant for is known, [`check`] reports names it does not declare and
//! converts values whose unit differs from the declared one (`mm` to `m`, `bar` to `Pa`,
//! `degC` to `K`, ...).

use crate::frontend::Library;
use crate::frontend::tree::{ClassId, ClassTree};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};

/// One parameter value read from a table.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ParamValue {
    /// Dotted name relative to the class or component the values are for.
    pub name: String,
    /// The value as Modelica source text (`0.5`, `true`, `"text"`, `{1, 2}`).
    pub value: String,
    pub unit: Option<String>,
}

/// Format a table cell as a Modelica literal.
pub fn literal(cell: &str) -> String {
    let c = cell.trim();
    if c.parse::<f64>().is_ok() {
        return c.trim_start_matches('+').to_string();
    }
    match c.to_ascii_lowercase().as_str() {
        "true" => return "true".into(),
        "false" => return "false".into(),
        _ => {}
    }
    if c.starts_with('{') || (c.starts_with('"') && c.ends_with('"') && c.len() >= 2) {
        return c.to_string();
    }
    // An enumeration literal or other dotted name (`Modelica.Blocks.Types.Init.NoInit`).
    let ident = |s: &str| {
        !s.is_empty()
            && s.chars().next().is_some_and(|ch| ch.is_alphabetic() || ch == '_')
            && s.chars().all(|ch| ch.is_alphanumeric() || ch == '_')
    };
    if c.contains('.') && c.split('.').all(ident) {
        return c.to_string();
    }
    let esc = c.replace('\\', "\\\\").replace('"', "\\\"");
    format!("\"{esc}\"")
}

/// Split one CSV line on `delim`: double-quoted fields (with `""` for a quote) and
/// `{...}` arrays may contain the delimiter.
fn split_csv(line: &str, delim: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    let mut depth = 0i32;
    let mut it = line.chars().peekable();
    while let Some(c) = it.next() {
        if q {
            if c == '"' {
                if it.peek() == Some(&'"') {
                    cur.push('"');
                    it.next();
                } else {
                    q = false;
                }
            } else {
                cur.push(c);
            }
            continue;
        }
        match c {
            '"' => q = true,
            '{' => {
                depth += 1;
                cur.push(c);
            }
            '}' => {
                depth -= 1;
                cur.push(c);
            }
            c if c == delim && depth <= 0 => out.push(std::mem::take(&mut cur)),
            c => cur.push(c),
        }
    }
    out.push(cur);
    out.into_iter().map(|s| s.trim().to_string()).collect()
}

/// Read a CSV table of `name,value[,unit]` rows. The delimiter is `,`, `;` or a tab
/// (whichever the first data line uses); a header row, blank lines and `#` or `//`
/// comment lines are skipped. A `{...}` array value may contain the delimiter.
pub fn parse_csv(text: &str) -> Result<Vec<ParamValue>> {
    let lines: Vec<&str> = text
        .lines()
        .map(|l| l.trim_start_matches('\u{feff}'))
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#') && !t.starts_with("//")
        })
        .collect();
    let Some(first) = lines.first() else {
        return Ok(Vec::new());
    };
    let delim = if first.contains('\t') {
        '\t'
    } else if first.contains(';') && !first.contains(',') {
        ';'
    } else if first.contains(';') && first.find(';') < first.find(',') {
        ';'
    } else {
        ','
    };
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let cells = split_csv(l, delim);
        if cells.len() < 2 {
            bail!("line {}: expected name{delim}value[{delim}unit], got '{l}'", i + 1);
        }
        let name = cells[0].trim().to_string();
        if i == 0 {
            let h = name.to_ascii_lowercase();
            if matches!(h.as_str(), "name" | "parameter" | "variable" | "param") {
                continue;
            }
        }
        if name.is_empty() {
            bail!("line {}: empty parameter name", i + 1);
        }
        let unit = cells
            .get(2)
            .map(|u| u.trim().to_string())
            .filter(|u| !u.is_empty() && u != "1" && u != "-");
        out.push(ParamValue {
            name,
            value: literal(&cells[1]),
            unit,
        });
    }
    Ok(out)
}

fn xml_unescape(s: &str) -> String {
    s.replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

/// Attributes of the XML start tag that begins at `tag` (`<ssv:Real value="1" .../>`).
fn attrs(tag: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let b = tag.as_bytes();
    let mut i = tag.find(|c: char| c.is_whitespace()).unwrap_or(tag.len());
    while i < b.len() {
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        let ks = i;
        while i < b.len() && b[i] != b'=' && !(b[i] as char).is_whitespace() && b[i] != b'/' && b[i] != b'>' {
            i += 1;
        }
        let key = &tag[ks..i];
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] != b'=' {
            i += 1;
            continue;
        }
        i += 1;
        while i < b.len() && (b[i] as char).is_whitespace() {
            i += 1;
        }
        if i >= b.len() {
            break;
        }
        let q = b[i];
        if q != b'"' && q != b'\'' {
            break;
        }
        let vs = i + 1;
        let ve = tag[vs..].find(q as char).map_or(tag.len(), |k| vs + k);
        out.push((key.to_string(), xml_unescape(&tag[vs..ve])));
        i = ve + 1;
    }
    out
}

/// Read SSP parameter values (`<ssv:ParameterSet>` with `<ssv:Parameter name=...>`
/// holding one `ssv:Real`, `ssv:Integer`, `ssv:Boolean`, `ssv:String` or
/// `ssv:Enumeration` element).
pub fn parse_ssv(text: &str) -> Result<Vec<ParamValue>> {
    let mut out = Vec::new();
    let mut rest = text;
    let mut current: Option<String> = None;
    while let Some(lt) = rest.find('<') {
        rest = &rest[lt + 1..];
        if rest.starts_with("!--") {
            rest = rest.find("-->").map_or("", |k| &rest[k + 3..]);
            continue;
        }
        let end = rest.find('>').unwrap_or(rest.len());
        let tag = &rest[..end];
        rest = &rest[end.min(rest.len())..];
        let name = tag.split(|c: char| c.is_whitespace() || c == '/').next().unwrap_or("");
        let local = name.rsplit(':').next().unwrap_or(name);
        if name.starts_with('/') {
            if local == "Parameter" {
                current = None;
            }
            continue;
        }
        let a = attrs(tag);
        let get = |k: &str| a.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        match local {
            "Parameter" => current = get("name"),
            "Real" | "Integer" | "Boolean" | "String" | "Enumeration" => {
                let Some(pname) = current.clone() else { continue };
                let Some(v) = get("value") else {
                    bail!("parameter {pname}: {local} without a value");
                };
                let value = match local {
                    "String" => format!("\"{}\"", v.replace('\\', "\\\\").replace('"', "\\\"")),
                    "Boolean" => v.trim().to_ascii_lowercase(),
                    "Enumeration" => v.trim().to_string(),
                    _ => v.trim().to_string(),
                };
                out.push(ParamValue {
                    name: pname,
                    value,
                    unit: get("unit").filter(|u| !u.is_empty()),
                });
            }
            _ => {}
        }
    }
    if out.is_empty() && !text.contains("ParameterSet") {
        bail!("not an SSV file (no ssv:ParameterSet)");
    }
    Ok(out)
}

/// Read a parameter table, choosing the format by file name (`.ssv`/`.xml`) or content.
pub fn parse(text: &str, file_name: &str) -> Result<Vec<ParamValue>> {
    let lower = file_name.to_ascii_lowercase();
    if lower.ends_with(".ssv")
        || lower.ends_with(".xml")
        || text.trim_start().starts_with("<?xml")
        || text.contains("<ssv:ParameterSet")
    {
        parse_ssv(text)
    } else {
        parse_csv(text)
    }
}

/// A modifier tree: `name = value` and `name(nested...)`.
#[derive(Debug, Clone, Default)]
pub struct ModTree {
    /// In first-seen order: (name, value, nested arguments). Entries the parser could not
    /// read as a plain modification (`redeclare ...`) keep their text as `name` with
    /// `opaque` set.
    pub items: Vec<ModItem>,
}

#[derive(Debug, Clone, Default)]
pub struct ModItem {
    pub name: String,
    /// `each` / `final` prefixes as written.
    pub prefix: String,
    pub value: Option<String>,
    pub nested: ModTree,
    pub opaque: bool,
}

impl ModTree {
    /// Build a tree from dotted names and values.
    pub fn from_params(params: &[ParamValue]) -> ModTree {
        let mut t = ModTree::default();
        for p in params {
            t.set(&p.name, &p.value);
        }
        t
    }

    fn item(&mut self, name: &str) -> &mut ModItem {
        if let Some(i) = self.items.iter().position(|x| !x.opaque && x.name == name) {
            return &mut self.items[i];
        }
        self.items.push(ModItem {
            name: name.to_string(),
            ..Default::default()
        });
        self.items.last_mut().unwrap()
    }

    /// Set `a.b.c = value`.
    pub fn set(&mut self, dotted: &str, value: &str) {
        // a dot inside a subscript (`x[1].y`) is not a separator: split at depth 0 only
        let mut parts = Vec::new();
        let mut depth = 0;
        let mut cur = String::new();
        for ch in dotted.chars() {
            match ch {
                '[' => depth += 1,
                ']' => depth -= 1,
                '.' if depth == 0 => {
                    parts.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(ch);
        }
        parts.push(cur);
        self.set_parts(&parts, value);
    }

    fn set_parts(&mut self, parts: &[String], value: &str) {
        let it = self.item(parts[0].trim());
        if parts.len() == 1 {
            it.value = Some(value.to_string());
        } else {
            it.nested.set_parts(&parts[1..], value);
        }
    }

    /// Merge `other` into this tree: its values win, nested trees merge.
    pub fn merge(&mut self, other: &ModTree) {
        for o in &other.items {
            if o.opaque {
                self.items.push(o.clone());
                continue;
            }
            let it = self.item(&o.name);
            if o.value.is_some() {
                it.value = o.value.clone();
            }
            it.nested.merge(&o.nested);
            if !o.prefix.is_empty() {
                it.prefix = o.prefix.clone();
            }
        }
    }

    /// The argument list without parentheses: `J=0.5, body(m=2)`.
    pub fn args(&self) -> String {
        self.items
            .iter()
            .map(|i| {
                if i.opaque {
                    return i.name.clone();
                }
                let mut s = i.prefix.clone();
                s.push_str(&i.name);
                if !i.nested.items.is_empty() {
                    s.push('(');
                    s.push_str(&i.nested.args());
                    s.push(')');
                }
                if let Some(v) = &i.value {
                    s.push_str(" = ");
                    s.push_str(v);
                }
                s
            })
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Parse an argument list (the text between a modifier's parentheses).
    pub fn parse_args(text: &str) -> ModTree {
        let mut t = ModTree::default();
        for arg in split_top(text, ',') {
            let a = arg.trim();
            if a.is_empty() {
                continue;
            }
            let mut prefix = String::new();
            let mut body = a;
            loop {
                let w = body.split_whitespace().next().unwrap_or("");
                if w == "each" || w == "final" {
                    prefix.push_str(w);
                    prefix.push(' ');
                    body = body[w.len()..].trim_start();
                } else {
                    break;
                }
            }
            let name_end = body
                .find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '.' || c == '[' || c == ']' || c == ' ' || c == ','))
                .unwrap_or(body.len());
            let name = body[..name_end].trim();
            let ok_name = !name.is_empty()
                && !name.contains(' ')
                && !matches!(name, "redeclare" | "replaceable");
            if !ok_name || body.starts_with("redeclare") || body.starts_with("replaceable") {
                t.items.push(ModItem {
                    name: a.to_string(),
                    opaque: true,
                    ..Default::default()
                });
                continue;
            }
            let mut rest = body[name_end..].trim_start();
            let mut item = ModItem {
                name: name.to_string(),
                prefix,
                ..Default::default()
            };
            if rest.starts_with('(') {
                let close = matching(rest, 0).unwrap_or(rest.len() - 1);
                item.nested = ModTree::parse_args(&rest[1..close]);
                rest = rest[close + 1..].trim_start();
            }
            if let Some(v) = rest.strip_prefix('=') {
                item.value = Some(v.trim().to_string());
            } else if let Some(v) = rest.strip_prefix(":=") {
                item.value = Some(v.trim().to_string());
            } else if !rest.is_empty() {
                // description string or something unexpected: keep the whole argument
                t.items.push(ModItem {
                    name: a.to_string(),
                    opaque: true,
                    ..Default::default()
                });
                continue;
            }
            t.items.push(item);
        }
        t
    }
}

/// Index of the bracket closing the one at `open` (`(`, `{` or `[`), skipping strings.
pub(crate) fn matching(s: &str, open: usize) -> Option<usize> {
    let b = s.as_bytes();
    let mut depth = 0i32;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' => {
                i += 1;
                while i < b.len() && b[i] != b'"' {
                    if b[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
            }
            b'(' | b'{' | b'[' => depth += 1,
            b')' | b'}' | b']' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Split at `sep` outside brackets and strings.
pub(crate) fn split_top(s: &str, sep: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth = 0i32;
    let mut cur = String::new();
    let mut q = false;
    let mut esc = false;
    for c in s.chars() {
        if q {
            cur.push(c);
            if esc {
                esc = false;
            } else if c == '\\' {
                esc = true;
            } else if c == '"' {
                q = false;
            }
            continue;
        }
        match c {
            '"' => q = true,
            '(' | '{' | '[' => depth += 1,
            ')' | '}' | ']' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(std::mem::take(&mut cur));
                continue;
            }
            _ => {}
        }
        cur.push(c);
    }
    out.push(cur);
    out
}

/// The modifier for a component: `name(J = 0.5, body(m = 2))`.
pub fn modifier(component: &str, params: &[ParamValue]) -> String {
    format!("{component}({})", ModTree::from_params(params).args())
}

/// A record declaring the values as parameters, for use as a parameter set. Dotted names
/// cannot be record fields: they are listed in a comment instead.
pub fn record(name: &str, params: &[ParamValue]) -> String {
    let mut s = format!("record {name} \"Parameters imported by DAEdalus\"\n");
    let mut skipped = Vec::new();
    for p in params {
        if p.name.contains('.') || p.name.contains('[') {
            skipped.push(p);
            continue;
        }
        let ty = if p.value.starts_with('"') {
            "String"
        } else if p.value == "true" || p.value == "false" {
            "Boolean"
        } else if p.value.starts_with('{') {
            "Real[:]"
        } else if p.value.parse::<i64>().is_ok() && p.unit.is_none() {
            "Integer"
        } else {
            "Real"
        };
        let (base, dims) = match ty.split_once('[') {
            Some((b, d)) => (b, format!("[{d}")),
            None => (ty, String::new()),
        };
        let unit = p
            .unit
            .as_ref()
            .filter(|_| base == "Real")
            .map(|u| format!("(unit=\"{u}\")"))
            .unwrap_or_default();
        s.push_str(&format!(
            "  parameter {base} {}{dims}{unit} = {};\n",
            p.name, p.value
        ));
    }
    if !skipped.is_empty() {
        s.push_str("  // not record fields (nested names):\n");
        for p in skipped {
            s.push_str(&format!("  //   {} = {}\n", p.name, p.value));
        }
    }
    s.push_str(&format!("end {name};\n"));
    s
}

/// A unit as a multiple of its SI unit: (SI unit, scale, offset), `si = scale * x + offset`.
pub fn unit_si(u: &str) -> Option<(&'static str, f64, f64)> {
    use std::f64::consts::PI;
    Some(match u.trim() {
        "m" => ("m", 1.0, 0.0),
        "mm" => ("m", 1e-3, 0.0),
        "cm" => ("m", 1e-2, 0.0),
        "km" => ("m", 1e3, 0.0),
        "kg" => ("kg", 1.0, 0.0),
        "g" => ("kg", 1e-3, 0.0),
        "t" => ("kg", 1e3, 0.0),
        "s" => ("s", 1.0, 0.0),
        "ms" => ("s", 1e-3, 0.0),
        "min" => ("s", 60.0, 0.0),
        "h" => ("s", 3600.0, 0.0),
        "Pa" => ("Pa", 1.0, 0.0),
        "kPa" => ("Pa", 1e3, 0.0),
        "MPa" => ("Pa", 1e6, 0.0),
        "bar" => ("Pa", 1e5, 0.0),
        "mbar" => ("Pa", 1e2, 0.0),
        "K" => ("K", 1.0, 0.0),
        "degC" => ("K", 1.0, 273.15),
        "rad" => ("rad", 1.0, 0.0),
        "deg" => ("rad", PI / 180.0, 0.0),
        "rad/s" => ("rad/s", 1.0, 0.0),
        "rpm" | "rev/min" => ("rad/s", 2.0 * PI / 60.0, 0.0),
        "m/s" => ("m/s", 1.0, 0.0),
        "km/h" => ("m/s", 1.0 / 3.6, 0.0),
        "m3" => ("m3", 1.0, 0.0),
        "l" | "L" => ("m3", 1e-3, 0.0),
        "m3/s" => ("m3/s", 1.0, 0.0),
        "l/min" | "L/min" => ("m3/s", 1e-3 / 60.0, 0.0),
        "l/s" => ("m3/s", 1e-3, 0.0),
        "N" => ("N", 1.0, 0.0),
        "kN" => ("N", 1e3, 0.0),
        "N.m" | "Nm" | "N·m" => ("N.m", 1.0, 0.0),
        "kN.m" => ("N.m", 1e3, 0.0),
        "W" => ("W", 1.0, 0.0),
        "kW" => ("W", 1e3, 0.0),
        "MW" => ("W", 1e6, 0.0),
        "J" => ("J", 1.0, 0.0),
        "kJ" => ("J", 1e3, 0.0),
        "MJ" => ("J", 1e6, 0.0),
        "Wh" => ("J", 3600.0, 0.0),
        "kWh" => ("J", 3.6e6, 0.0),
        "N/m" => ("N/m", 1.0, 0.0),
        "N/mm" => ("N/m", 1e3, 0.0),
        "kN/m" => ("N/m", 1e3, 0.0),
        "N.s/m" => ("N.s/m", 1.0, 0.0),
        "kg.m2" => ("kg.m2", 1.0, 0.0),
        "A" => ("A", 1.0, 0.0),
        "mA" => ("A", 1e-3, 0.0),
        "V" => ("V", 1.0, 0.0),
        "kV" => ("V", 1e3, 0.0),
        "Ohm" => ("Ohm", 1.0, 0.0),
        "kOhm" => ("Ohm", 1e3, 0.0),
        "F" => ("F", 1.0, 0.0),
        "uF" => ("F", 1e-6, 0.0),
        "H" => ("H", 1.0, 0.0),
        "mH" => ("H", 1e-3, 0.0),
        "Hz" => ("Hz", 1.0, 0.0),
        _ => return None,
    })
}

/// Convert `x` from unit `from` to unit `to`, when both are known and compatible.
pub fn convert(x: f64, from: &str, to: &str) -> Option<f64> {
    if from == to {
        return Some(x);
    }
    let (sa, ka, oa) = unit_si(from)?;
    let (sb, kb, ob) = unit_si(to)?;
    if sa != sb {
        return None;
    }
    Some((ka * x + oa - ob) / kb)
}

/// Result of checking parameter values against the class they are for.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Checked {
    /// The values, converted to the declared units where needed.
    pub params: Vec<ParamValue>,
    /// One line per problem or conversion.
    pub messages: Vec<String>,
}

/// The unit declared for component `name` of class `scope`: its own `unit` modifier, or
/// that of its type (following short type definitions like `type Mass = Real(unit="kg")`).
fn declared_unit(tree: &ClassTree, scope: ClassId, comp: &rumoca_ir_ast::Component) -> Option<String> {
    fn unit_in(mods: &[rumoca_ir_ast::Expression]) -> Option<String> {
        for m in mods {
            if let rumoca_ir_ast::Expression::Modification { target, value, .. } = m
                && target.parts.len() == 1
                && &*target.parts[0].ident.text == "unit"
                && let serde_json::Value::String(s) =
                    crate::frontend::inspect::expr_json(value, None)
            {
                return Some(s);
            }
        }
        None
    }
    if let Some(u) = unit_in(&comp.source_modifications) {
        return Some(u);
    }
    let parts: Vec<&str> = comp.type_name.name.iter().map(|t| &*t.text).collect();
    let mut ty = tree.lookup(scope, &parts, None)?;
    for _ in 0..16 {
        let def = tree.def(ty)?;
        for e in &def.extends {
            let mods: Vec<rumoca_ir_ast::Expression> =
                e.modifications.iter().map(|m| m.expr.clone()).collect();
            if let Some(u) = unit_in(&mods) {
                return Some(u);
            }
        }
        let base = def.extends.first()?;
        let bp: Vec<&str> = base.base_name.name.iter().map(|t| &*t.text).collect();
        ty = tree.lookup(ty, &bp, None)?;
    }
    None
}

/// Check `params` against class `class` of `lib` (or against the type of component
/// `component` of it): names it does not declare are reported, and values with a unit
/// other than the declared one are converted when the units are known.
pub fn check(lib: &Library, class: &str, component: Option<&str>, params: &[ParamValue]) -> Result<Checked> {
    let tree = lib.tree();
    let Some(mut id) = tree.find_path(class) else {
        bail!("class {class} not found");
    };
    if let Some(c) = component {
        let Some((owner, comp)) = tree.find_component(id, c) else {
            bail!("{class} has no component {c}");
        };
        let parts: Vec<&str> = comp.type_name.name.iter().map(|t| &*t.text).collect();
        let Some(t) = tree.lookup(owner, &parts, None) else {
            bail!("type {} of {c} not found", parts.join("."));
        };
        id = t;
    }
    let mut out = Checked::default();
    for p in params {
        let mut p = p.clone();
        // walk the dotted name through the component types
        let segs: Vec<String> = p
            .name
            .split('.')
            .map(|s| s.split('[').next().unwrap_or(s).trim().to_string())
            .collect();
        let mut scope = id;
        let mut found = None;
        for (k, s) in segs.iter().enumerate() {
            match tree.find_component(scope, s) {
                Some((owner, comp)) => {
                    if k + 1 == segs.len() {
                        found = Some((owner, comp));
                    } else {
                        let parts: Vec<&str> = comp.type_name.name.iter().map(|t| &*t.text).collect();
                        match tree.lookup(owner, &parts, None) {
                            Some(t) => scope = t,
                            None => break,
                        }
                    }
                }
                None => break,
            }
        }
        match found {
            None => {
                out.messages.push(format!(
                    "warning: {} is not a component of {}: skipped",
                    p.name,
                    component.map_or(class.to_string(), |c| format!("{class}.{c}"))
                ));
                continue;
            }
            Some((owner, comp)) => {
                if !matches!(comp.variability, rumoca_core::Variability::Parameter(_) | rumoca_core::Variability::Constant(_)) {
                    out.messages.push(format!("note: {} is not declared as a parameter", p.name));
                }
                if let (Some(given), Some(decl)) = (p.unit.clone(), declared_unit(&tree, owner, comp))
                    && given != decl
                {
                    match p.value.parse::<f64>().ok().and_then(|x| convert(x, &given, &decl)) {
                        Some(y) => {
                            let v = format!("{}", (y * 1e12).round() / 1e12);
                            out.messages.push(format!(
                                "note: {} = {} {given} converted to {v} {decl}",
                                p.name, p.value
                            ));
                            p.value = v;
                            p.unit = Some(decl);
                        }
                        None => out.messages.push(format!(
                            "warning: {} is given in {given}, declared in {decl} (no conversion known)",
                            p.name
                        )),
                    }
                }
            }
        }
        out.params.push(p);
    }
    Ok(out)
}
