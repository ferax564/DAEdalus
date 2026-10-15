//! Class inspection for editors: the package tree, graphical annotations, components,
//! connections and parameters of a class, as serialisable data.
//!
//! Annotations are converted to a small JSON vocabulary that a renderer can draw
//! directly: records become objects tagged with `"@"` (e.g. `{"@":"Rectangle", "extent":
//! [[-100,-100],[100,100]]}`), arrays become arrays, enumeration literals and other names
//! become strings, and numeric sub-expressions are folded where possible.

use super::Library;
use super::tree::{ClassId, ClassTree, ROOT};
use rumoca_ir_ast as ast;
use serde::Serialize;
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, Serialize)]
pub struct ClassEntry {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub partial: bool,
    pub description: String,
    /// Has an `experiment` annotation (can be simulated as is).
    pub runnable: bool,
    pub children: Vec<ClassEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceRef {
    pub uri: String,
    pub start: usize,
    pub end: usize,
    pub line: u32,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Layer {
    /// `coordinateSystem` record, if given.
    pub coordinate_system: Option<Value>,
    pub graphics: Vec<Value>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ComponentInfo {
    pub name: String,
    /// The type name as written.
    pub type_name: String,
    /// Fully qualified type, when it resolves to a class of the library.
    pub type_path: Option<String>,
    pub type_kind: String,
    pub description: String,
    pub causality: String,
    pub variability: String,
    pub flow: bool,
    pub protected: bool,
    pub conditional: bool,
    /// The condition of a conditional component as source text (`useHeatPort`).
    pub condition: Option<String>,
    /// `Placement` annotation.
    pub placement: Option<Value>,
    /// Modifiers as source text, e.g. `("J", "0.5")`.
    pub modifiers: Vec<(String, String)>,
    /// Binding equation as source text.
    pub binding: Option<String>,
    /// Class that declares the component (differs from the inspected class when inherited).
    pub declared_in: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConnectionInfo {
    pub from: String,
    pub to: String,
    /// `Line` annotation of the connect equation.
    pub line: Option<Value>,
    pub declared_in: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct ClassInfo {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub partial: bool,
    pub description: String,
    pub source: Option<SourceRef>,
    pub extends: Vec<String>,
    pub icon: Layer,
    pub diagram: Layer,
    pub components: Vec<ComponentInfo>,
    pub connections: Vec<ConnectionInfo>,
    pub documentation: Option<String>,
    pub experiment: Option<Value>,
}

fn kind_name(k: &rumoca_core::ClassType) -> &'static str {
    use rumoca_core::ClassType as K;
    match k {
        K::Model => "model",
        K::Class => "class",
        K::Block => "block",
        K::Connector => "connector",
        K::Record => "record",
        K::Type => "type",
        K::Package => "package",
        K::Function => "function",
        K::Operator => "operator",
    }
}

fn description(tokens: &[rumoca_core::Token]) -> String {
    tokens
        .iter()
        .map(|t| t.text.trim_matches('"').to_string())
        .collect::<Vec<_>>()
        .join("")
}

fn has_annotation(def: &ast::ClassDef, name: &str) -> bool {
    find_annotation(&def.annotation, name).is_some()
}

fn find_annotation<'e>(ann: &'e [ast::Expression], name: &str) -> Option<&'e ast::Expression> {
    ann.iter().find(|a| match a {
        ast::Expression::ClassModification { target, .. }
        | ast::Expression::FunctionCall { comp: target, .. } => {
            target.parts.len() == 1 && &*target.parts[0].ident.text == name
        }
        _ => false,
    })
}

/// The package tree of everything loaded, in declaration order.
pub fn class_tree(lib: &Library) -> Vec<ClassEntry> {
    let tree = lib.tree();
    let root = tree.node(ROOT);
    let mut kids: Vec<ClassId> = root.order.clone();
    kids.sort_by_key(|&c| tree.node(c).name.to_lowercase());
    kids.into_iter().map(|c| entry(&tree, c, 0)).collect()
}

fn entry(tree: &ClassTree, id: ClassId, depth: usize) -> ClassEntry {
    let node = tree.node(id);
    let def = node.def;
    let children = if depth < 32 {
        node.order
            .iter()
            .map(|&c| entry(tree, c, depth + 1))
            .collect()
    } else {
        Vec::new()
    };
    ClassEntry {
        name: node.name.clone(),
        path: node.path.clone(),
        kind: def
            .map_or("package", |d| kind_name(&d.class_type))
            .to_string(),
        partial: def.is_some_and(|d| d.partial),
        description: def.map(|d| description(&d.description)).unwrap_or_default(),
        runnable: def.is_some_and(|d| !d.partial && has_annotation(d, "experiment")),
        children,
    }
}

/// Everything an editor needs to draw and edit one class.
pub fn class_info(lib: &Library, path: &str) -> Option<ClassInfo> {
    let tree = lib.tree();
    let id = tree.find_path(path)?;
    let node = tree.node(id);
    let def = node.def?;
    let mut info = ClassInfo {
        path: node.path.clone(),
        name: node.name.clone(),
        kind: kind_name(&def.class_type).to_string(),
        partial: def.partial,
        description: description(&def.description),
        source: lib.docs.get(node.source).map(|d| SourceRef {
            uri: d.uri.clone(),
            start: def.location.start as usize,
            end: def.location.end as usize,
            line: def.location.start_line,
        }),
        extends: tree
            .bases(id)
            .iter()
            .map(|&b| tree.node(b).path.clone())
            .collect(),
        icon: Layer::default(),
        diagram: Layer::default(),
        components: Vec::new(),
        connections: Vec::new(),
        documentation: None,
        experiment: find_annotation(&def.annotation, "experiment").map(|e| expr_json(e, None)),
    };
    if let Some(ast::Expression::ClassModification { modifications, .. }) =
        find_annotation(&def.annotation, "Documentation")
    {
        for m in modifications {
            if let ast::Expression::Modification { target, value, .. } = m
                && target.parts.len() == 1
                && &*target.parts[0].ident.text == "info"
                && let Value::String(s) = expr_json(value, None)
            {
                info.documentation = Some(s);
            }
        }
    }
    collect(lib, &tree, id, &mut info, 0);
    Some(info)
}

/// Gather graphics, components and connections of `id` and its bases (bases first).
fn collect(lib: &Library, tree: &ClassTree, id: ClassId, info: &mut ClassInfo, depth: usize) {
    if depth > 16 {
        return;
    }
    for b in tree.bases(id) {
        collect(lib, tree, b, info, depth + 1);
    }
    let node = tree.node(id);
    let Some(def) = node.def else { return };
    let text = lib.docs.get(node.source).map(|d| &*d.text);
    // The parser drops the annotation of a short class definition
    // (`connector RealOutput = output Real annotation(Icon(...))`), so read it from source.
    let short = if def.annotation.is_empty() {
        text.and_then(|t| short_class_annotation(t, def.location.end as usize))
            .and_then(|a| parse_annotation(&a))
    } else {
        None
    };
    for (name, layer) in [("Icon", &mut info.icon), ("Diagram", &mut info.diagram)] {
        if let Some(Value::Object(o)) = short.as_ref().and_then(|v| v.get(name)) {
            if let Some(Value::Array(items)) = o.get("graphics") {
                layer.graphics.extend(items.iter().cloned());
            }
            if let Some(cs) = o.get("coordinateSystem") {
                layer.coordinate_system = Some(cs.clone());
            }
        }
        if let Some(ast::Expression::ClassModification { modifications, .. }) =
            find_annotation(&def.annotation, name)
        {
            for m in modifications {
                match m {
                    ast::Expression::Modification { target, value, .. }
                        if &*target.parts[0].ident.text == "graphics" =>
                    {
                        if let Value::Array(items) = expr_json(value, text) {
                            layer.graphics.extend(items);
                        }
                    }
                    ast::Expression::ClassModification { target, .. }
                        if &*target.parts[0].ident.text == "coordinateSystem" =>
                    {
                        layer.coordinate_system = Some(expr_json(m, text));
                    }
                    _ => {}
                }
            }
        }
    }
    let scope = id;
    for (name, c) in &def.components {
        let parts: Vec<&str> = c.type_name.name.iter().map(|t| &*t.text).collect();
        let resolved = tree.lookup(scope, &parts, None);
        let modifiers = c
            .source_modifications
            .iter()
            .filter_map(|m| match m {
                ast::Expression::Modification { target, value, .. } => {
                    Some((cref_text(target), src(text, value.span())))
                }
                ast::Expression::ClassModification { target, .. } => {
                    Some((cref_text(target), src(text, m.span())))
                }
                _ => None,
            })
            .collect();
        info.components.push(ComponentInfo {
            name: name.clone(),
            type_name: parts.join("."),
            type_path: resolved.map(|r| tree.node(r).path.clone()),
            type_kind: resolved
                .map_or("type", |r| kind_name(&tree.class_type(r)))
                .to_string(),
            description: description(&c.description),
            causality: match c.causality {
                rumoca_core::Causality::Input(_) => "input",
                rumoca_core::Causality::Output(_) => "output",
                rumoca_core::Causality::Empty => "",
            }
            .into(),
            variability: match c.variability {
                rumoca_core::Variability::Constant(_) => "constant",
                rumoca_core::Variability::Parameter(_) => "parameter",
                rumoca_core::Variability::Discrete(_) => "discrete",
                rumoca_core::Variability::Continuous(_) | rumoca_core::Variability::Empty => "",
            }
            .into(),
            flow: matches!(c.connection, ast::Connection::Flow(_)),
            protected: c.is_protected,
            conditional: c.condition.is_some(),
            condition: c.condition.as_ref().map(|x| src(text, x.span())),
            placement: find_annotation(&c.annotation, "Placement").map(|p| expr_json(p, text)),
            modifiers,
            binding: c.binding.as_ref().map(|b| src(text, b.span())),
            declared_in: node.path.clone(),
        });
    }
    for eq in &def.equations {
        collect_connect(eq, text, &node.path, &mut info.connections);
    }
}

fn collect_connect(
    eq: &ast::Equation,
    text: Option<&str>,
    declared_in: &str,
    out: &mut Vec<ConnectionInfo>,
) {
    match eq {
        ast::Equation::Connect { lhs, rhs } => {
            let line = text
                .and_then(|t| connect_annotation(t, rhs.span.end.0))
                .and_then(|a| {
                    let v = parse_annotation(&a)?;
                    match v {
                        Value::Object(o) => o.get("Line").cloned(),
                        _ => None,
                    }
                });
            out.push(ConnectionInfo {
                from: cref_text(lhs),
                to: cref_text(rhs),
                line,
                declared_in: declared_in.to_string(),
            });
        }
        ast::Equation::For { equations, .. } => {
            for e in equations {
                collect_connect(e, text, declared_in, out);
            }
        }
        _ => {}
    }
}

/// Source text of the `annotation(...)` argument list that follows a connect equation, if any.
/// The parser keeps connect equations but not their annotations, so they are read from source.
fn connect_annotation(text: &str, rhs_end: usize) -> Option<String> {
    let b = text.as_bytes();
    let mut i = rhs_end.min(b.len());
    let skip_ws = |i: &mut usize| {
        while *i < b.len() && (b[*i] as char).is_whitespace() {
            *i += 1;
        }
    };
    skip_ws(&mut i);
    if i >= b.len() || b[i] != b')' {
        return None;
    }
    i += 1;
    skip_ws(&mut i);
    if !text[i..].starts_with("annotation") {
        return None;
    }
    i += "annotation".len();
    skip_ws(&mut i);
    if i >= b.len() || b[i] != b'(' {
        return None;
    }
    let start = i + 1;
    let mut level = 0;
    let mut in_str = false;
    while i < b.len() {
        let ch = b[i];
        if in_str {
            if ch == b'\\' {
                i += 1;
            } else if ch == b'"' {
                in_str = false;
            }
        } else if ch == b'"' {
            in_str = true;
        } else if ch == b'(' {
            level += 1;
        } else if ch == b')' {
            level -= 1;
            if level == 0 {
                return Some(text[start..i].to_string());
            }
        }
        i += 1;
    }
    None
}

/// The annotation argument list of a short class definition `class A = B(...) annotation(...)`
/// whose name ends at `name_end` in `text`; `None` for long definitions.
fn short_class_annotation(text: &str, name_end: usize) -> Option<String> {
    let body = text.get(name_end..)?;
    // A short definition continues with `=` right after its name.
    let eq = body.find(|c: char| !c.is_whitespace())?;
    if body.as_bytes()[eq] != b'=' {
        return None;
    }
    // Find a top-level `annotation` keyword after the `=`, outside strings and parentheses.
    let b = body.as_bytes();
    let (mut i, mut level, mut in_str) = (eq, 0i32, false);
    while i < b.len() {
        let ch = b[i];
        if in_str {
            if ch == b'\\' {
                i += 1;
            } else if ch == b'"' {
                in_str = false;
            }
        } else if ch == b'"' {
            in_str = true;
        } else if ch == b'(' {
            level += 1;
        } else if ch == b')' {
            level -= 1;
        } else if ch == b';' && level == 0 {
            break;
        } else if level == 0
            && body[i..].starts_with("annotation")
            && !body[..i].ends_with(|c: char| c.is_alphanumeric() || c == '_')
        {
            let s = &body[i + "annotation".len()..];
            let open = s.find('(')?;
            if !s[..open].trim().is_empty() {
                return None;
            }
            return balanced(&s[open..]);
        }
        i += 1;
    }
    None
}

/// The contents of the parenthesised group that `s` starts with.
fn balanced(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let (mut level, mut in_str) = (0i32, false);
    for (i, &ch) in b.iter().enumerate() {
        if in_str {
            if ch == b'"' && (i == 0 || b[i - 1] != b'\\') {
                in_str = false;
            }
        } else if ch == b'"' {
            in_str = true;
        } else if ch == b'(' {
            level += 1;
        } else if ch == b')' {
            level -= 1;
            if level == 0 {
                return Some(s[1..i].to_string());
            }
        }
    }
    None
}

fn src(text: Option<&str>, span: rumoca_core::Span) -> String {
    text.and_then(|t| t.get(span.start.0..span.end.0))
        .map(|s| s.trim().to_string())
        .unwrap_or_default()
}

fn cref_text(c: &ast::ComponentReference) -> String {
    let mut s = String::new();
    for (i, p) in c.parts.iter().enumerate() {
        if i > 0 {
            s.push('.');
        }
        s.push_str(&p.ident.text);
        if let Some(subs) = &p.subs {
            s.push('[');
            for (k, sub) in subs.iter().enumerate() {
                if k > 0 {
                    s.push(',');
                }
                match sub {
                    ast::Subscript::Expression(e) => match expr_json(e, None) {
                        Value::Number(n) => s.push_str(&n.to_string()),
                        Value::String(t) => s.push_str(&t),
                        other => s.push_str(&other.to_string()),
                    },
                    _ => s.push(':'),
                }
            }
            s.push(']');
        }
    }
    s
}

fn num(v: f64) -> Value {
    if v.fract() == 0.0 && v.abs() < 1e15 {
        json!(v as i64)
    } else {
        json!(v)
    }
}

/// Convert an annotation expression to JSON.
pub fn expr_json(e: &ast::Expression, text: Option<&str>) -> Value {
    use ast::Expression as E;
    match e {
        E::Terminal {
            terminal_type,
            token,
            ..
        } => match terminal_type {
            ast::TerminalType::UnsignedInteger | ast::TerminalType::UnsignedReal => {
                token.text.parse::<f64>().map(num).unwrap_or(Value::Null)
            }
            ast::TerminalType::String => Value::String(unescape(token.text.trim_matches('"'))),
            ast::TerminalType::Bool => Value::Bool(&*token.text == "true"),
            _ => Value::String(token.text.to_string()),
        },
        E::ComponentReference(c) => Value::String(cref_text(c)),
        E::Unary { op, rhs, .. } => {
            let v = expr_json(rhs, text);
            match (format!("{op:?}").as_str(), v.as_f64()) {
                (o, Some(x)) if o.contains("Minus") => num(-x),
                (_, Some(x)) => num(x),
                // `not useAxisFlange`: keep the whole expression for the client to evaluate
                _ => Value::String(text.map(|t| src(Some(t), e.span())).unwrap_or_default()),
            }
        }
        E::Binary { op, lhs, rhs, .. } => {
            let (a, b) = (expr_json(lhs, text), expr_json(rhs, text));
            let o = format!("{op:?}");
            if let (Some(x), Some(y)) = (a.as_f64(), b.as_f64()) {
                let r = if o.starts_with("Add") {
                    Some(x + y)
                } else if o.starts_with("Sub") {
                    Some(x - y)
                } else if o.starts_with("Mul") {
                    Some(x * y)
                } else if o.starts_with("Div") {
                    Some(x / y)
                } else {
                    None
                };
                if let Some(r) = r {
                    return num(r);
                }
            }
            Value::String(text.map(|t| src(Some(t), e.span())).unwrap_or_default())
        }
        E::Array { elements, .. } | E::Tuple { elements, .. } => {
            Value::Array(elements.iter().map(|x| expr_json(x, text)).collect())
        }
        E::FunctionCall { comp, args, .. } => record(&cref_text(comp), args, text),
        E::ClassModification {
            target,
            modifications,
            ..
        } => record(&cref_text(target), modifications, text),
        E::Parenthesized { inner, .. } => expr_json(inner, text),
        _ => Value::String(text.map(|t| src(Some(t), e.span())).unwrap_or_default()),
    }
}

fn record(name: &str, args: &[ast::Expression], text: Option<&str>) -> Value {
    let mut m = Map::new();
    m.insert("@".into(), Value::String(name.to_string()));
    let mut pos = 0;
    for a in args {
        match a {
            ast::Expression::NamedArgument { name, value, .. } => {
                m.insert(name.text.to_string(), expr_json(value, text));
            }
            ast::Expression::Modification { target, value, .. } => {
                m.insert(cref_text(target), expr_json(value, text));
            }
            ast::Expression::ClassModification { target, .. } => {
                m.insert(cref_text(target), expr_json(a, text));
            }
            other => {
                m.insert(format!("_{pos}"), expr_json(other, text));
                pos += 1;
            }
        }
    }
    Value::Object(m)
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c == '\\' {
            match it.next() {
                Some('n') => out.push('\n'),
                Some('t') => out.push('\t'),
                Some(o) => out.push(o),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse the argument list of an annotation (`Line(points={{0,0},{1,1}}, color={0,0,127})`)
/// into an object keyed by record name. A tiny recursive-descent parser over the annotation
/// subset of the grammar: records with named or positional arguments, arrays, numbers,
/// strings, booleans and dotted names.
pub fn parse_annotation(s: &str) -> Option<Value> {
    let mut p = P {
        b: s.as_bytes(),
        i: 0,
    };
    let mut m = Map::new();
    loop {
        p.ws();
        if p.i >= p.b.len() {
            break;
        }
        let name = p.name()?;
        p.ws();
        let v = if p.eat(b'(') {
            p.args(&name)?
        } else if p.eat(b'=') {
            p.value()?
        } else {
            Value::Null
        };
        m.insert(name, v);
        p.ws();
        if !p.eat(b',') {
            break;
        }
    }
    Some(Value::Object(m))
}

struct P<'s> {
    b: &'s [u8],
    i: usize,
}

impl P<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && (self.b[self.i] as char).is_whitespace() {
            self.i += 1;
        }
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.i < self.b.len() && self.b[self.i] == c {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn name(&mut self) -> Option<String> {
        self.ws();
        let st = self.i;
        while self.i < self.b.len()
            && ((self.b[self.i] as char).is_alphanumeric() || b"._".contains(&self.b[self.i]))
        {
            self.i += 1;
        }
        (self.i > st).then(|| String::from_utf8_lossy(&self.b[st..self.i]).into_owned())
    }
    fn args(&mut self, name: &str) -> Option<Value> {
        let mut m = Map::new();
        m.insert("@".into(), Value::String(name.to_string()));
        let mut pos = 0;
        if self.eat(b')') {
            return Some(Value::Object(m));
        }
        loop {
            self.ws();
            let save = self.i;
            let named = self.name().and_then(|n| {
                self.ws();
                // `name =` but not `name ==`
                if self.i < self.b.len()
                    && self.b[self.i] == b'='
                    && self.b.get(self.i + 1) != Some(&b'=')
                {
                    self.i += 1;
                    Some(n)
                } else {
                    None
                }
            });
            match named {
                Some(n) => {
                    let v = self.value()?;
                    m.insert(n, v);
                }
                None => {
                    self.i = save;
                    let v = self.value()?;
                    m.insert(format!("_{pos}"), v);
                    pos += 1;
                }
            }
            if self.eat(b')') {
                break;
            }
            if !self.eat(b',') {
                return None;
            }
        }
        Some(Value::Object(m))
    }
    fn value(&mut self) -> Option<Value> {
        self.ws();
        let c = *self.b.get(self.i)?;
        if c == b'{' {
            self.i += 1;
            let mut items = Vec::new();
            if self.eat(b'}') {
                return Some(Value::Array(items));
            }
            loop {
                items.push(self.value()?);
                if self.eat(b'}') {
                    break;
                }
                if !self.eat(b',') {
                    return None;
                }
            }
            return Some(Value::Array(items));
        }
        if c == b'"' {
            self.i += 1;
            let st = self.i;
            while self.i < self.b.len() && self.b[self.i] != b'"' {
                if self.b[self.i] == b'\\' {
                    self.i += 1;
                }
                self.i += 1;
            }
            let s = String::from_utf8_lossy(&self.b[st..self.i.min(self.b.len())]).into_owned();
            self.i += 1;
            return Some(Value::String(unescape(&s)));
        }
        if c == b'-' || c == b'+' || c.is_ascii_digit() || c == b'.' {
            let st = self.i;
            self.i += 1;
            while self.i < self.b.len()
                && (self.b[self.i].is_ascii_digit()
                    || b".eE".contains(&self.b[self.i])
                    || ((self.b[self.i] == b'-' || self.b[self.i] == b'+')
                        && b"eE".contains(&self.b[self.i - 1])))
            {
                self.i += 1;
            }
            let t = String::from_utf8_lossy(&self.b[st..self.i]).into_owned();
            return t.parse::<f64>().ok().map(num);
        }
        let n = self.name()?;
        if self.eat(b'(') {
            return self.args(&n);
        }
        Some(match n.as_str() {
            "true" => Value::Bool(true),
            "false" => Value::Bool(false),
            _ => Value::String(n),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connect_line_annotation() {
        let v = parse_annotation("Line(points={{-60,0},{-40,0}}, color={0,0,127}, thickness=0.5)")
            .unwrap();
        assert_eq!(v["Line"]["points"], json!([[-60, 0], [-40, 0]]));
        assert_eq!(v["Line"]["color"], json!([0, 0, 127]));
        assert_eq!(v["Line"]["thickness"], json!(0.5));
    }

    #[test]
    fn short_connector_icon() {
        let mut lib = Library::new();
        lib.set_document(
            "a.mo",
            "package P connector RealOutput = output Real \"y\" annotation (Icon(graphics={Polygon(points={{-100,100},{100,0},{-100,-100}})}), Diagram(graphics={Line(points={{0,0},{1,1}})})); end P;",
        );
        let i = class_info(&lib, "P.RealOutput").unwrap();
        assert_eq!(i.icon.graphics.len(), 1);
        assert_eq!(i.diagram.graphics.len(), 1);
    }

    #[test]
    fn class_info_reads_graphics_and_connections() {
        let src = r#"
package P
  connector C Real e; flow Real f;
    annotation (Icon(graphics={Rectangle(extent={{-100,-100},{100,100}}, fillPattern=FillPattern.Solid)}));
  end C;
  model M "A model"
    C a annotation (Placement(transformation(extent={{-110,-10},{-90,10}})));
    C b(e(start=1)) annotation (Placement(transformation(extent={{90,-10},{110,10}}, rotation=180)));
  equation
    connect(a, b) annotation (Line(points={{-100,0},{100,0}}, color={0,0,127}));
    annotation (Icon(coordinateSystem(extent={{-100,-100},{100,100}}), graphics={Line(points={{-80,0},{80,0}})}),
      experiment(StopTime=2));
  end M;
end P;
"#;
        let mut lib = Library::new();
        assert!(lib.set_document("p.mo", src).is_empty());
        let info = class_info(&lib, "P.M").unwrap();
        assert_eq!(info.description, "A model");
        assert_eq!(info.icon.graphics[0]["@"], "Line");
        assert!(info.icon.coordinate_system.is_some());
        assert_eq!(info.components.len(), 2);
        assert_eq!(info.components[0].type_path.as_deref(), Some("P.C"));
        assert_eq!(info.components[0].type_kind, "connector");
        assert_eq!(
            info.components[1].placement.as_ref().unwrap()["transformation"]["rotation"],
            180
        );
        assert_eq!(info.components[1].modifiers[0].0, "e");
        assert_eq!(info.connections[0].from, "a");
        assert_eq!(
            info.connections[0].line.as_ref().unwrap()["points"],
            json!([[-100, 0], [100, 0]])
        );
        assert_eq!(info.experiment.as_ref().unwrap()["StopTime"], 2);
        let tree = class_tree(&lib);
        assert_eq!(tree[0].children[1].name, "M");
        assert!(tree[0].children[1].runnable);
    }
}
