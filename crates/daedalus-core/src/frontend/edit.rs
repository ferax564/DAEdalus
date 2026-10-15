//! Source-preserving graphical edits of a class.
//!
//! The diagram editor changes a model by editing its source text: adding and deleting
//! components and connect equations, moving and rotating components, setting modifiers and
//! renaming. Each edit touches only the tokens it has to and leaves every other byte of the
//! document (comments, layout, annotations) as it was, the way Dymola and OMEdit keep a
//! hand-written file readable after graphical editing.
//!
//! The edits work on a token scan of the document, not on the AST: the parser's source
//! positions do not cover everything an edit needs (short class definitions, annotations of
//! connect equations, the extent of a declaration), and a scan of the class structure is
//! enough to place every edit. The result of each edit is parsed again before it is
//! accepted (see [`edit_class`]).

use super::inspect::{self, parse_annotation};
use super::{Library, parse_document};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ---------------------------------------------------------------- lexer

#[derive(Debug, Clone, Copy, PartialEq)]
enum K {
    Ident,
    Num,
    Str,
    Sym,
}

#[derive(Debug, Clone, Copy)]
struct Tok {
    k: K,
    s: usize,
    e: usize,
}

/// Tokens of Modelica source; comments and whitespace are skipped (and so preserved by
/// every edit, which only replaces token ranges).
fn lex(text: &str) -> Vec<Tok> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && b.get(i + 1) == Some(&b'/') {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if c == b'/' && b.get(i + 1) == Some(&b'*') {
            i += 2;
            while i < b.len() && !(b[i] == b'*' && b.get(i + 1) == Some(&b'/')) {
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else if c == b'"' || c == b'\'' {
            let s = i;
            i += 1;
            while i < b.len() && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(b.len());
            let k = if c == b'"' { K::Str } else { K::Ident };
            out.push(Tok { k, s, e: i });
        } else if c.is_ascii_alphabetic() || c == b'_' || c >= 0x80 {
            let s = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] >= 0x80) {
                i += 1;
            }
            out.push(Tok {
                k: K::Ident,
                s,
                e: i,
            });
        } else if c.is_ascii_digit() {
            let s = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' {
                i += 1;
                while i < b.len() && b[i].is_ascii_digit() {
                    i += 1;
                }
            }
            if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < b.len() && b[j].is_ascii_digit() {
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            out.push(Tok { k: K::Num, s, e: i });
        } else {
            let two = text.get(i..i + 2).unwrap_or("");
            let n = if matches!(two, "==" | "<=" | ">=" | "<>" | ":=") {
                2
            } else {
                // a multi-byte character outside identifiers: one whole char
                text[i..].chars().next().map_or(1, |ch| ch.len_utf8())
            };
            out.push(Tok {
                k: K::Sym,
                s: i,
                e: i + n,
            });
            i += n;
        }
    }
    out
}

// ---------------------------------------------------------------- class structure

#[derive(Debug, Clone, Copy, PartialEq)]
enum Sec {
    Public,
    Protected,
    Equation,
    InitialEquation,
    Algorithm,
    InitialAlgorithm,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum SK {
    /// `public`, `protected`, `equation`, `initial equation`, ...
    Section,
    /// A class annotation `annotation(...);`.
    Annotation,
    Element,
    Extends,
    Import,
    NestedClass,
    /// A simple equation or statement (also inside if/for/when).
    Equation,
    /// An if/for/when/while equation or statement, `if ... end if;`.
    Compound,
}

#[derive(Debug, Clone)]
struct Stmt {
    kind: SK,
    sec: Sec,
    /// First and last token (the `;`, or the keyword of a section).
    first: usize,
    last: usize,
    /// Nesting inside compound equations (0 at the top of a section).
    depth: u32,
}

#[derive(Debug, Clone)]
struct ClassSyn {
    name: String,
    /// `model`, `connector`, ... (the last kind keyword of the header).
    kind: String,
    /// First token of the definition (prefixes included).
    start: usize,
    name_tok: usize,
    /// `end` of `end Name;` (long classes).
    end_tok: usize,
    /// The final `;`.
    semi: usize,
    short: bool,
    stmts: Vec<Stmt>,
    classes: Vec<ClassSyn>,
}

const CLASS_KINDS: &[&str] = &[
    "class",
    "model",
    "record",
    "block",
    "connector",
    "type",
    "package",
    "function",
    "operator",
];
const CLASS_PREFIXES: &[&str] = &[
    "encapsulated",
    "partial",
    "final",
    "redeclare",
    "replaceable",
    "inner",
    "outer",
    "expandable",
    "pure",
    "impure",
];
const TYPE_PREFIXES: &[&str] = &[
    "flow",
    "stream",
    "discrete",
    "parameter",
    "constant",
    "input",
    "output",
    "each",
];
const KEYWORDS: &[&str] = &[
    "and", "or", "not", "if", "then", "else", "elseif", "for", "loop", "in", "while", "when",
    "elsewhen", "end", "true", "false", "der", "time", "initial", "connect",
];

/// A scanned document: its tokens and top-level classes.
struct Doc<'t> {
    text: &'t str,
    toks: Vec<Tok>,
    within: Option<String>,
    classes: Vec<ClassSyn>,
}

impl<'t> Doc<'t> {
    fn new(text: &'t str) -> Result<Self> {
        let mut d = Doc {
            text,
            toks: lex(text),
            within: None,
            classes: Vec::new(),
        };
        let mut i = 0;
        if d.is(0, "within") {
            let semi = d.until_semi(1)?;
            d.within = Some((1..semi).map(|k| d.t(k)).collect::<String>());
            i = semi + 1;
        }
        while i < d.toks.len() {
            if d.is(i, ";") {
                i += 1;
                continue;
            }
            let (c, next) = d.class(i)?;
            d.classes.push(c);
            i = next;
        }
        Ok(d)
    }

    fn t(&self, i: usize) -> &'t str {
        self.toks.get(i).map_or("", |t| &self.text[t.s..t.e])
    }
    fn is(&self, i: usize, s: &str) -> bool {
        self.t(i) == s
    }
    fn kind(&self, i: usize) -> Option<K> {
        self.toks.get(i).map(|t| t.k)
    }
    fn ident(&self, i: usize) -> bool {
        self.kind(i) == Some(K::Ident)
    }
    fn s(&self, i: usize) -> usize {
        self.toks.get(i).map_or(self.text.len(), |t| t.s)
    }
    fn e(&self, i: usize) -> usize {
        self.toks.get(i).map_or(self.text.len(), |t| t.e)
    }
    /// Matching closer of the bracket at `i`.
    fn close(&self, i: usize) -> Result<usize> {
        let mut depth = 0i32;
        for j in i..self.toks.len() {
            match self.t(j) {
                "(" | "{" | "[" => depth += 1,
                ")" | "}" | "]" => {
                    depth -= 1;
                    if depth == 0 {
                        return Ok(j);
                    }
                }
                _ => {}
            }
        }
        bail!("unbalanced brackets at byte {}", self.s(i))
    }
    /// The `;` that ends the statement starting at `i` (outside brackets).
    fn until_semi(&self, i: usize) -> Result<usize> {
        let mut depth = 0i32;
        for j in i..self.toks.len() {
            match self.t(j) {
                "(" | "{" | "[" => depth += 1,
                ")" | "}" | "]" => depth -= 1,
                ";" if depth <= 0 => return Ok(j),
                _ => {}
            }
        }
        bail!("missing ';' after byte {}", self.s(i))
    }

    /// Scan a class definition starting at token `i` (its first prefix).
    fn class(&self, i: usize) -> Result<(ClassSyn, usize)> {
        let start = i;
        let mut j = i;
        while self.ident(j)
            && (CLASS_PREFIXES.contains(&self.t(j)) || CLASS_KINDS.contains(&self.t(j)))
        {
            j += 1;
        }
        if j == i {
            bail!("expected a class definition at byte {}", self.s(i));
        }
        let kind = (i..j)
            .rev()
            .map(|k| self.t(k))
            .find(|t| CLASS_KINDS.contains(t))
            .unwrap_or("class")
            .to_string();
        if self.is(j, "extends") {
            j += 1;
        }
        if !self.ident(j) {
            bail!("expected a class name at byte {}", self.s(j));
        }
        let name_tok = j;
        let name = self.t(j).to_string();
        j += 1;
        let mut c = ClassSyn {
            name,
            kind,
            start,
            name_tok,
            end_tok: 0,
            semi: 0,
            short: false,
            stmts: Vec::new(),
            classes: Vec::new(),
        };
        if self.is(j, "=") {
            c.short = true;
            c.semi = self.until_semi(j)?;
            c.end_tok = c.semi;
            let next = c.semi + 1;
            return Ok((c, next));
        }
        // `model extends A(mods)` and the description
        if self.is(j, "(") {
            j = self.close(j)? + 1;
        }
        while self.kind(j) == Some(K::Str) || self.is(j, "+") {
            j += 1;
        }
        let mut sec = Sec::Public;
        loop {
            if j >= self.toks.len() {
                bail!("class {} has no 'end {};'", c.name, c.name);
            }
            let w = self.t(j);
            let section = |kind_sec: Sec, first: usize, last: usize| Stmt {
                kind: SK::Section,
                sec: kind_sec,
                first,
                last,
                depth: 0,
            };
            if w == "end" && self.t(j + 1) == c.name && self.is(j + 2, ";") {
                c.end_tok = j;
                c.semi = j + 2;
                return Ok((c, j + 3));
            } else if w == ";" {
                j += 1;
            } else if w == "public" || w == "protected" {
                sec = if w == "public" {
                    Sec::Public
                } else {
                    Sec::Protected
                };
                c.stmts.push(section(sec, j, j));
                j += 1;
            } else if w == "equation" || w == "algorithm" {
                sec = if w == "equation" {
                    Sec::Equation
                } else {
                    Sec::Algorithm
                };
                c.stmts.push(section(sec, j, j));
                j += 1;
            } else if w == "initial" && (self.is(j + 1, "equation") || self.is(j + 1, "algorithm"))
            {
                sec = if self.is(j + 1, "equation") {
                    Sec::InitialEquation
                } else {
                    Sec::InitialAlgorithm
                };
                c.stmts.push(section(sec, j, j + 1));
                j += 2;
            } else if w == "annotation" && self.is(j + 1, "(") {
                let semi = self.until_semi(j)?;
                c.stmts.push(Stmt {
                    kind: SK::Annotation,
                    sec,
                    first: j,
                    last: semi,
                    depth: 0,
                });
                j = semi + 1;
            } else if matches!(sec, Sec::Public | Sec::Protected) {
                let mut k = j;
                while self.ident(k) && CLASS_PREFIXES.contains(&self.t(k)) {
                    k += 1;
                }
                if self.ident(k) && CLASS_KINDS.contains(&self.t(k)) {
                    let (nested, next) = self.class(j)?;
                    // `replaceable model X = Y constrainedby Z ...;` ends at its own `;`
                    let last = nested.semi;
                    c.stmts.push(Stmt {
                        kind: SK::NestedClass,
                        sec,
                        first: j,
                        last,
                        depth: 0,
                    });
                    c.classes.push(nested);
                    j = next;
                } else {
                    let semi = self.until_semi(j)?;
                    let kind = match self.t(k) {
                        "extends" => SK::Extends,
                        "import" => SK::Import,
                        _ => SK::Element,
                    };
                    c.stmts.push(Stmt {
                        kind,
                        sec,
                        first: j,
                        last: semi,
                        depth: 0,
                    });
                    j = semi + 1;
                }
            } else {
                j = self.equation(j, sec, 0, &mut c.stmts)?;
            }
        }
    }

    /// Scan one equation or algorithm statement; compound ones recursively.
    fn equation(&self, i: usize, sec: Sec, depth: u32, out: &mut Vec<Stmt>) -> Result<usize> {
        let w = self.t(i);
        if self.ident(i) && matches!(w, "if" | "for" | "when" | "while") {
            let slot = out.len();
            out.push(Stmt {
                kind: SK::Compound,
                sec,
                first: i,
                last: i,
                depth,
            });
            let stop = if matches!(w, "for" | "while") {
                "loop"
            } else {
                "then"
            };
            let mut j = self.header(i + 1, stop)?;
            loop {
                if j >= self.toks.len() {
                    bail!("'{w}' at byte {} has no 'end {w};'", self.s(i));
                }
                if self.is(j, "end") && self.is(j + 1, w) && self.is(j + 2, ";") {
                    out[slot].last = j + 2;
                    return Ok(j + 3);
                }
                if self.is(j, "elseif") || self.is(j, "elsewhen") {
                    j = self.header(j + 1, "then")?;
                } else if self.is(j, "else") {
                    j += 1;
                } else {
                    j = self.equation(j, sec, depth + 1, out)?;
                }
            }
        }
        let semi = self.until_semi(i)?;
        out.push(Stmt {
            kind: SK::Equation,
            sec,
            first: i,
            last: semi,
            depth,
        });
        Ok(semi + 1)
    }

    /// Skip the condition of a compound statement up to and including `stop`
    /// (`then`/`loop`); if-expressions inside the condition have their own `then`.
    fn header(&self, i: usize, stop: &str) -> Result<usize> {
        let (mut depth, mut pending) = (0i32, 0u32);
        for j in i..self.toks.len() {
            match self.t(j) {
                "(" | "{" | "[" => depth += 1,
                ")" | "}" | "]" => depth -= 1,
                "if" if depth == 0 => pending += 1,
                t if depth == 0 && t == stop => {
                    if stop == "then" && pending > 0 {
                        pending -= 1;
                    } else {
                        return Ok(j + 1);
                    }
                }
                _ => {}
            }
        }
        bail!("missing '{stop}' after byte {}", self.s(i))
    }

    /// The class at a dotted path (the document's `within` prefix is optional).
    fn find(&self, path: &str) -> Option<&ClassSyn> {
        let rel = match &self.within {
            Some(w) if !w.is_empty() => path.strip_prefix(&format!("{w}.")).unwrap_or(path),
            _ => path,
        };
        let mut parts = rel.split('.');
        let first = parts.next()?;
        let mut c = self.classes.iter().find(|c| c.name == first)?;
        for p in parts {
            c = c.classes.iter().find(|k| k.name == p)?;
        }
        Some(c)
    }

    /// Whether a token ends an operand (so a following string or `if` starts a new part).
    fn operand_end(&self, i: usize) -> bool {
        match self.kind(i) {
            Some(K::Num) | Some(K::Str) => true,
            Some(K::Ident) => {
                !KEYWORDS.contains(&self.t(i))
                    || matches!(self.t(i), "true" | "false" | "time" | "end")
            }
            Some(K::Sym) => matches!(self.t(i), ")" | "]" | "}"),
            None => false,
        }
    }

    // ------------------------------------------------------------ declarations

    /// The declarators of an element statement.
    fn decls(&self, st: &Stmt) -> Vec<Decl> {
        let mut out = Vec::new();
        let mut j = st.first;
        while self.ident(j)
            && (CLASS_PREFIXES.contains(&self.t(j)) || TYPE_PREFIXES.contains(&self.t(j)))
        {
            j += 1;
        }
        if self.is(j, ".") {
            j += 1;
        }
        if !self.ident(j) {
            return out;
        }
        let type_first = j;
        while self.is(j + 1, ".") && self.ident(j + 2) {
            j += 2;
        }
        let type_last = j;
        j += 1;
        if self.is(j, "[") {
            match self.close(j) {
                Ok(c) => j = c + 1,
                Err(_) => return out,
            }
        }
        while j < st.last && self.ident(j) {
            let mut d = Decl {
                name: self.t(j).to_string(),
                name_tok: j,
                type_first,
                type_last,
                modp: None,
                binding: None,
                ann: None,
                last: j,
            };
            j += 1;
            if self.is(j, "[") {
                let Ok(c) = self.close(j) else { return out };
                j = c + 1;
            }
            if self.is(j, "(") {
                let Ok(c) = self.close(j) else { return out };
                d.modp = Some((j, c));
                j = c + 1;
            }
            if self.is(j, "=") || self.is(j, ":=") {
                let b0 = j + 1;
                j = self.expr_end(b0, st.last);
                d.binding = Some((b0, j - 1));
            }
            if self.is(j, "if") {
                j = self.expr_end(j + 1, st.last);
            }
            while self.kind(j) == Some(K::Str) || (self.is(j, "+") && j > d.name_tok) {
                j += 1;
            }
            if self.is(j, "annotation") && self.is(j + 1, "(") {
                let Ok(c) = self.close(j + 1) else { return out };
                d.ann = Some((j, j + 1, c));
                j = c + 1;
            }
            d.last = j - 1;
            out.push(d);
            if self.is(j, ",") {
                j += 1;
            } else {
                break;
            }
        }
        out
    }

    /// End (exclusive) of an expression starting at `i`: the next top-level `,` or `;`,
    /// `annotation`, `constrainedby`, or a string or `if` that follows a complete operand.
    fn expr_end(&self, i: usize, limit: usize) -> usize {
        let mut depth = 0i32;
        let mut j = i;
        while j < limit {
            let t = self.t(j);
            if depth == 0 {
                if t == "," || t == ";" {
                    break;
                }
                if self.ident(j) && (t == "annotation" || t == "constrainedby") {
                    break;
                }
                if j > i
                    && self.operand_end(j - 1)
                    && (self.kind(j) == Some(K::Str) || (self.ident(j) && t == "if"))
                {
                    break;
                }
            }
            match t {
                "(" | "{" | "[" => depth += 1,
                ")" | "}" | "]" => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        j
    }

    /// Arguments of the bracket group opening at `open` (a modification or a record call).
    fn args(&self, open: usize) -> Result<Vec<Arg>> {
        let close = self.close(open)?;
        let mut out = Vec::new();
        let mut j = open + 1;
        while j < close {
            let first = j;
            let mut depth = 0i32;
            let mut k = j;
            while k < close {
                match self.t(k) {
                    "(" | "{" | "[" => depth += 1,
                    ")" | "}" | "]" => depth -= 1,
                    "," if depth == 0 => break,
                    _ => {}
                }
                k += 1;
            }
            let last = k - 1;
            let mut a = Arg {
                name: String::new(),
                first,
                last,
                sub: None,
                value: None,
            };
            let mut n = first;
            while self.ident(n) && matches!(self.t(n), "each" | "final") {
                n += 1;
            }
            if self.ident(n) && !matches!(self.t(n), "redeclare" | "replaceable") {
                let mut name = self.t(n).to_string();
                while self.is(n + 1, ".") && self.ident(n + 2) {
                    name.push('.');
                    name.push_str(self.t(n + 2));
                    n += 2;
                }
                n += 1;
                if self.is(n, "(") {
                    let c = self.close(n)?;
                    a.sub = Some((n, c));
                    n = c + 1;
                }
                if n <= last && (self.is(n, "=") || self.is(n, ":=")) {
                    a.value = Some((n + 1, last));
                    a.name = name;
                } else if n > last {
                    a.name = name;
                }
            }
            out.push(a);
            j = k + 1;
        }
        Ok(out)
    }

    /// A connect equation: `connect(a, b) [annotation(...)];`.
    fn connect(&self, st: &Stmt) -> Option<Conn> {
        if st.kind != SK::Equation || !self.is(st.first, "connect") || !self.is(st.first + 1, "(") {
            return None;
        }
        let open = st.first + 1;
        let close = self.close(open).ok()?;
        let mut depth = 0;
        let mut comma = None;
        for j in open + 1..close {
            match self.t(j) {
                "(" | "{" | "[" => depth += 1,
                ")" | "}" | "]" => depth -= 1,
                "," if depth == 0 => {
                    comma = Some(j);
                    break;
                }
                _ => {}
            }
        }
        let comma = comma?;
        let text = |a: usize, b: usize| (a..b).map(|k| self.t(k)).collect::<String>();
        let ann = if self.is(close + 1, "annotation") && self.is(close + 2, "(") {
            Some((close + 1, close + 2, self.close(close + 2).ok()?))
        } else {
            None
        };
        Some(Conn {
            from: text(open + 1, comma),
            to: text(comma + 1, close),
            close,
            ann,
        })
    }
}

#[derive(Debug, Clone)]
struct Decl {
    name: String,
    name_tok: usize,
    type_first: usize,
    type_last: usize,
    /// `(` and `)` of the modification.
    modp: Option<(usize, usize)>,
    /// First and last token of the binding expression.
    binding: Option<(usize, usize)>,
    /// `annotation`, `(` and `)`.
    ann: Option<(usize, usize, usize)>,
    /// Last token of the declarator.
    last: usize,
}

#[derive(Debug, Clone)]
struct Arg {
    /// Name of a modification or named argument (empty for positional arguments).
    name: String,
    first: usize,
    last: usize,
    /// `(` and `)` of a nested modification.
    sub: Option<(usize, usize)>,
    /// First and last token of the value after `=`.
    value: Option<(usize, usize)>,
}

#[derive(Debug, Clone)]
struct Conn {
    from: String,
    to: String,
    /// The `)` of `connect(...)`.
    close: usize,
    ann: Option<(usize, usize, usize)>,
}

// ---------------------------------------------------------------- operations

/// One graphical edit of a class. Coordinates are in the diagram's coordinate system.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum EditOp {
    /// Declare `type_name name(modifiers) annotation(Placement(...))` at the end of the
    /// public declarations. Without a name, one is made from the type (`constant1`).
    AddComponent {
        type_name: String,
        #[serde(default)]
        name: Option<String>,
        extent: [[f64; 2]; 2],
        #[serde(default)]
        rotation: Option<f64>,
        #[serde(default)]
        modifiers: Vec<(String, String)>,
    },
    /// Delete a component and every connect equation that refers to it.
    DeleteComponent { name: String },
    /// Translate a component's placement.
    MoveComponent { name: String, dx: f64, dy: f64 },
    /// Write a placement: `extent` relative to `origin`, rotated by `rotation` degrees.
    SetPlacement {
        name: String,
        #[serde(default)]
        origin: Option<[f64; 2]>,
        extent: [[f64; 2]; 2],
        #[serde(default)]
        rotation: Option<f64>,
    },
    /// Rotate a component about the centre of its icon (degrees, counter-clockwise).
    RotateComponent { name: String, angle: f64 },
    /// Mirror a component about the vertical (`horizontal`) or horizontal axis through
    /// the centre of its icon.
    FlipComponent { name: String, horizontal: bool },
    /// Add `connect(from, to) annotation(Line(points=..., color=...));`.
    AddConnection {
        from: String,
        to: String,
        #[serde(default)]
        points: Vec<[f64; 2]>,
        #[serde(default)]
        color: Option<[u8; 3]>,
    },
    /// Delete the connect equation between two connectors (in either order).
    DeleteConnection { from: String, to: String },
    /// Rewrite the points of a connection's `Line` annotation.
    SetConnectionPoints {
        from: String,
        to: String,
        points: Vec<[f64; 2]>,
    },
    /// Set (`Some`) or remove (`None`) a modifier of a component: `R` or `x.start`.
    SetModifier {
        name: String,
        modifier: String,
        #[serde(default)]
        value: Option<String>,
    },
    /// Rename a component in its declaration and in every equation of the class.
    RenameComponent { name: String, new_name: String },
}

/// Text replacements on a document, applied back to front.
#[derive(Default)]
struct Patch(Vec<(usize, usize, String)>);

impl Patch {
    fn replace(&mut self, s: usize, e: usize, text: impl Into<String>) {
        self.0.push((s, e, text.into()));
    }
    fn insert(&mut self, at: usize, text: impl Into<String>) {
        self.0.push((at, at, text.into()));
    }
    fn apply(self, text: &str) -> String {
        // Back to front; of two insertions at one place the later one is applied first,
        // so they end up in the order they were made.
        let mut edits: Vec<_> = self.0.into_iter().enumerate().collect();
        edits.sort_by(|(i, a), (j, b)| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(j.cmp(i)));
        let mut out = text.to_string();
        for (_, (s, e, r)) in edits {
            out.replace_range(s..e, &r);
        }
        out
    }
}

/// Format a coordinate: integers without a decimal point, others with at most 4 decimals.
fn fmt_num(x: f64) -> String {
    let x = if x == 0.0 { 0.0 } else { x };
    if (x - x.round()).abs() < 1e-9 {
        format!("{}", x.round() as i64)
    } else {
        let s = format!("{x:.4}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}
fn fmt_point(p: [f64; 2]) -> String {
    format!("{{{},{}}}", fmt_num(p[0]), fmt_num(p[1]))
}
fn fmt_points(ps: &[[f64; 2]]) -> String {
    let v: Vec<String> = ps.iter().map(|p| fmt_point(*p)).collect();
    format!("{{{}}}", v.join(","))
}
fn fmt_extent(e: [[f64; 2]; 2]) -> String {
    fmt_points(&e)
}

fn line_start(text: &str, off: usize) -> usize {
    text[..off].rfind('\n').map_or(0, |i| i + 1)
}
/// Indentation of the line holding `off`, if only whitespace precedes `off` on it.
fn lead_indent(text: &str, off: usize) -> Option<String> {
    let ls = line_start(text, off);
    let pre = &text[ls..off];
    pre.chars()
        .all(|c| c == ' ' || c == '\t')
        .then(|| pre.to_string())
}
fn line_indent(text: &str, off: usize) -> String {
    let ls = line_start(text, off);
    text[ls..]
        .chars()
        .take_while(|c| *c == ' ' || *c == '\t')
        .collect()
}

/// A unit of indentation as used by the document (two spaces unless tabs are used).
fn indent_unit(text: &str) -> &'static str {
    if text.lines().any(|l| l.starts_with('\t')) {
        "\t"
    } else {
        "  "
    }
}

fn valid_ident(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_alphabetic() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !KEYWORDS.contains(&s)
        && !CLASS_KINDS.contains(&s)
        && !matches!(
            s,
            "equation"
                | "algorithm"
                | "annotation"
                | "extends"
                | "import"
                | "within"
                | "public"
                | "protected"
                | "parameter"
                | "constant"
                | "input"
                | "output"
                | "flow"
                | "stream"
                | "discrete"
                | "final"
                | "each"
                | "redeclare"
                | "replaceable"
                | "inner"
                | "outer"
                | "partial"
                | "encapsulated"
                | "expandable"
                | "constrainedby"
                | "break"
                | "return"
                | "external"
        )
}

/// Placement transformation of a component (missing fields are defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Transform {
    origin: Option<[f64; 2]>,
    extent: [[f64; 2]; 2],
    rotation: f64,
}

impl Transform {
    /// Centre of the icon in the parent's coordinates.
    fn center(&self) -> [f64; 2] {
        let o = self.origin.unwrap_or([0.0, 0.0]);
        let c = [
            (self.extent[0][0] + self.extent[1][0]) / 2.0,
            (self.extent[0][1] + self.extent[1][1]) / 2.0,
        ];
        let (s, co) = self.rotation.to_radians().sin_cos();
        [o[0] + co * c[0] - s * c[1], o[1] + s * c[0] + co * c[1]]
    }
    /// The same placement written as an extent centred on the origin.
    fn centered(&self) -> Transform {
        let c = [
            (self.extent[0][0] + self.extent[1][0]) / 2.0,
            (self.extent[0][1] + self.extent[1][1]) / 2.0,
        ];
        Transform {
            origin: Some(self.center()),
            extent: [
                [self.extent[0][0] - c[0], self.extent[0][1] - c[1]],
                [self.extent[1][0] - c[0], self.extent[1][1] - c[1]],
            ],
            rotation: self.rotation,
        }
    }
}

fn norm_angle(a: f64) -> f64 {
    let r = a.rem_euclid(360.0);
    if (r - 360.0).abs() < 1e-9 { 0.0 } else { r }
}

fn json_pt(v: &Value) -> Option<[f64; 2]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?])
}

/// Normalised connector reference (`a . p` and `a.p` compare equal).
fn norm_ref(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
}
/// The component a connector reference starts with: `a` of `a[2].p`.
fn ref_head(s: &str) -> &str {
    let end = s.find(['.', '[']).unwrap_or(s.len());
    s[..end].trim()
}

/// Result of applying edits to a document.
#[derive(Debug, Clone, Serialize)]
pub struct Applied {
    pub text: String,
    /// Name of the component added by an `add_component` op.
    pub name: Option<String>,
}

/// Apply one edit to the class at `class` (dotted path) in a document's text. `taken`
/// lists names already used by the class (inherited components) that a new component
/// must not take.
pub fn apply_op(text: &str, class: &str, op: &EditOp, taken: &[String]) -> Result<Applied> {
    let doc = Doc::new(text)?;
    let c = doc
        .find(class)
        .ok_or_else(|| anyhow!("class {class} not found in the document"))?;
    if c.short {
        bail!("{class} is a short class definition and has no diagram to edit");
    }
    let mut p = Patch::default();
    let mut added = None;
    match op {
        EditOp::AddComponent {
            type_name,
            name,
            extent,
            rotation,
            modifiers,
        } => {
            let used = declared_names(&doc, c);
            let name = match name {
                Some(n) => {
                    if !valid_ident(n) {
                        bail!("'{n}' is not a valid component name");
                    }
                    if used.iter().chain(taken).any(|u| u == n) {
                        bail!("{class} already has an element named {n}");
                    }
                    n.clone()
                }
                None => unique_name(type_name, &used, taken),
            };
            let mods = if modifiers.is_empty() {
                String::new()
            } else {
                let v: Vec<String> = modifiers.iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("({})", v.join(", "))
            };
            let tr = match rotation {
                Some(r) if norm_angle(*r) != 0.0 => {
                    let t = Transform {
                        origin: None,
                        extent: *extent,
                        rotation: norm_angle(*r),
                    }
                    .centered();
                    // centred on the extent, so the icon turns in place
                    let c0 = [
                        (extent[0][0] + extent[1][0]) / 2.0,
                        (extent[0][1] + extent[1][1]) / 2.0,
                    ];
                    format!(
                        "transformation(origin={}, extent={}, rotation={})",
                        fmt_point(c0),
                        fmt_extent(t.extent),
                        fmt_num(t.rotation)
                    )
                }
                _ => format!("transformation(extent={})", fmt_extent(*extent)),
            };
            let decl = format!("{type_name} {name}{mods} annotation (Placement({tr}));");
            insert_element(&doc, c, &decl, &mut p);
            added = Some(name);
        }
        EditOp::DeleteComponent { name } => {
            let (st, d, all) = find_decl(&doc, c, name)?;
            if all.len() == 1 {
                delete_stmt(&doc, st.first, st.last, &mut p);
            } else {
                // `C a, b;`: drop the declarator and one comma
                let i = all.iter().position(|x| x.name == *name).unwrap_or(0);
                if i + 1 < all.len() {
                    p.replace(doc.s(d.name_tok), doc.s(all[i + 1].name_tok), "");
                } else {
                    p.replace(doc.e(all[i - 1].last), doc.e(d.last), "");
                }
            }
            let mut gone = Vec::new();
            for (i, st) in c.stmts.iter().enumerate() {
                if let Some(cn) = doc.connect(st)
                    && (ref_head(&cn.from) == name || ref_head(&cn.to) == name)
                {
                    delete_stmt(&doc, st.first, st.last, &mut p);
                    gone.push(i);
                }
            }
            drop_empty_sections(&doc, c, &gone, &mut p);
        }
        EditOp::MoveComponent { name, dx, dy } => {
            let (_, d, _) = find_decl(&doc, c, name)?;
            let t = read_transform(&doc, &d)?
                .ok_or_else(|| anyhow!("{name} has no Placement to move"))?;
            let t2 = match t.origin {
                Some(o) => Transform {
                    origin: Some([o[0] + dx, o[1] + dy]),
                    ..t
                },
                None => Transform {
                    extent: [
                        [t.extent[0][0] + dx, t.extent[0][1] + dy],
                        [t.extent[1][0] + dx, t.extent[1][1] + dy],
                    ],
                    ..t
                },
            };
            write_transform(&doc, &d, &t, &t2, &mut p)?;
        }
        EditOp::SetPlacement {
            name,
            origin,
            extent,
            rotation,
        } => {
            let (_, d, _) = find_decl(&doc, c, name)?;
            let t = read_transform(&doc, &d)?;
            let t2 = Transform {
                origin: *origin,
                extent: *extent,
                rotation: rotation.map(norm_angle).unwrap_or(0.0),
            };
            match t {
                Some(t) => write_transform(&doc, &d, &t, &t2, &mut p)?,
                None => add_placement(&doc, &d, &t2, &mut p),
            }
        }
        EditOp::RotateComponent { name, angle } => {
            let (_, d, _) = find_decl(&doc, c, name)?;
            let t = read_transform(&doc, &d)?
                .ok_or_else(|| anyhow!("{name} has no Placement to rotate"))?;
            let mut t2 = t.centered();
            t2.rotation = norm_angle(t.rotation + angle);
            write_transform(&doc, &d, &t, &t2, &mut p)?;
        }
        EditOp::FlipComponent { name, horizontal } => {
            let (_, d, _) = find_decl(&doc, c, name)?;
            let t = read_transform(&doc, &d)?
                .ok_or_else(|| anyhow!("{name} has no Placement to flip"))?;
            // A mirror F about an axis turns R(r)·S into R(-r)·(F·S): mirror the extent
            // and negate the rotation.
            let mut t2 = t.centered();
            if *horizontal {
                t2.extent = [
                    [t2.extent[1][0], t2.extent[0][1]],
                    [t2.extent[0][0], t2.extent[1][1]],
                ];
            } else {
                t2.extent = [
                    [t2.extent[0][0], t2.extent[1][1]],
                    [t2.extent[1][0], t2.extent[0][1]],
                ];
            }
            t2.rotation = norm_angle(-t.rotation);
            if t.origin.is_none() && t.rotation == 0.0 {
                // keep the simple form: same box, mirrored corners
                let ctr = t.center();
                t2 = Transform {
                    origin: None,
                    extent: [
                        [t2.extent[0][0] + ctr[0], t2.extent[0][1] + ctr[1]],
                        [t2.extent[1][0] + ctr[0], t2.extent[1][1] + ctr[1]],
                    ],
                    rotation: 0.0,
                };
            }
            write_transform(&doc, &d, &t, &t2, &mut p)?;
        }
        EditOp::AddConnection {
            from,
            to,
            points,
            color,
        } => {
            if norm_ref(from) == norm_ref(to) {
                bail!("cannot connect {from} to itself");
            }
            if matches!(
                c.kind.as_str(),
                "connector" | "record" | "type" | "function" | "package"
            ) {
                bail!("a {} cannot have connect equations", c.kind);
            }
            if find_connect(&doc, c, from, to).is_some() {
                bail!("{from} and {to} are connected already");
            }
            let col = color.unwrap_or([0, 0, 255]);
            let ann = if points.len() >= 2 {
                format!(
                    " annotation (Line(points={}, color={{{},{},{}}}))",
                    fmt_points(points),
                    col[0],
                    col[1],
                    col[2]
                )
            } else {
                String::new()
            };
            let eq = format!("connect({}, {}){ann};", from.trim(), to.trim());
            insert_equation(&doc, c, &eq, &mut p);
        }
        EditOp::DeleteConnection { from, to } => {
            let (st, _) = find_connect(&doc, c, from, to)
                .ok_or_else(|| anyhow!("no connect({from}, {to}) in {class}"))?;
            delete_stmt(&doc, st.first, st.last, &mut p);
            let i = c
                .stmts
                .iter()
                .position(|s| s.first == st.first)
                .unwrap_or(0);
            drop_empty_sections(&doc, c, &[i], &mut p);
        }
        EditOp::SetConnectionPoints { from, to, points } => {
            let (_, cn) = find_connect(&doc, c, from, to)
                .ok_or_else(|| anyhow!("no connect({from}, {to}) in {class}"))?;
            // Points run from the first connector of the equation to the second.
            let mut pts = points.clone();
            if norm_ref(&cn.from) != norm_ref(from) {
                pts.reverse();
            }
            let val = fmt_points(&pts);
            match cn.ann {
                None => p.insert(
                    doc.e(cn.close),
                    format!(" annotation (Line(points={val}, color={{0,0,255}}))"),
                ),
                Some((_, open, _)) => {
                    let args = doc.args(open)?;
                    match args.iter().find(|a| a.name == "Line" && a.sub.is_some()) {
                        None => {
                            let sep = if args.is_empty() { "" } else { ", " };
                            p.insert(doc.e(open), format!("Line(points={val}){sep}"));
                        }
                        Some(line) => {
                            let (lo, _) = line.sub.unwrap();
                            set_arg(&doc, lo, "points", &val, &mut p)?;
                        }
                    }
                }
            }
        }
        EditOp::SetModifier {
            name,
            modifier,
            value,
        } => {
            let (_, d, _) = find_decl(&doc, c, name)?;
            let path: Vec<&str> = modifier.split('.').map(str::trim).collect();
            if path
                .iter()
                .any(|s| !valid_ident(s) && !matches!(*s, "start" | "fixed"))
            {
                bail!("'{modifier}' is not a modifier name");
            }
            let value = value.as_deref().map(str::trim).filter(|v| !v.is_empty());
            set_modifier(&doc, &d, &path, value, &mut p)?;
        }
        EditOp::RenameComponent { name, new_name } => {
            if !valid_ident(new_name) {
                bail!("'{new_name}' is not a valid component name");
            }
            if new_name != name
                && declared_names(&doc, c)
                    .iter()
                    .chain(taken)
                    .any(|u| u == new_name)
            {
                bail!("{class} already has an element named {new_name}");
            }
            let (_, d, _) = find_decl(&doc, c, name)?;
            p.replace(doc.s(d.name_tok), doc.e(d.name_tok), new_name.clone());
            rename_refs(&doc, c, name, new_name, d.name_tok, &mut p);
        }
    }
    Ok(Applied {
        text: p.apply(text),
        name: added,
    })
}

/// Names of all elements declared in the class itself.
fn declared_names(doc: &Doc, c: &ClassSyn) -> Vec<String> {
    let mut out: Vec<String> = c.classes.iter().map(|k| k.name.clone()).collect();
    for st in c.stmts.iter().filter(|s| s.kind == SK::Element) {
        out.extend(doc.decls(st).into_iter().map(|d| d.name));
    }
    out
}

/// A free name for a new component of type `type_name`: the class name with a lower-case
/// first letter and a number (`resistor1`).
fn unique_name(type_name: &str, used: &[String], taken: &[String]) -> String {
    let last = type_name
        .rsplit('.')
        .next()
        .unwrap_or("c")
        .trim_matches('\'');
    let mut base: String = last
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect();
    if base.is_empty() || base.starts_with(|c: char| c.is_ascii_digit()) {
        base.insert(0, 'c');
    }
    let mut cs = base.chars();
    let base: String = cs
        .next()
        .map(|f| f.to_ascii_lowercase())
        .into_iter()
        .chain(cs)
        .collect();
    (1..)
        .map(|i| format!("{base}{i}"))
        .find(|n| !used.contains(n) && !taken.contains(n))
        .unwrap()
}

fn find_decl<'d>(doc: &Doc, c: &'d ClassSyn, name: &str) -> Result<(&'d Stmt, Decl, Vec<Decl>)> {
    for st in c.stmts.iter().filter(|s| s.kind == SK::Element) {
        let all = doc.decls(st);
        if let Some(d) = all.iter().find(|d| d.name == name) {
            return Ok((st, d.clone(), all));
        }
    }
    bail!(
        "{name} is not declared in {} (components inherited from a base class cannot be edited here)",
        c.name
    )
}

fn find_connect<'d>(doc: &Doc, c: &'d ClassSyn, a: &str, b: &str) -> Option<(&'d Stmt, Conn)> {
    let (a, b) = (norm_ref(a), norm_ref(b));
    c.stmts
        .iter()
        .filter(|s| s.kind == SK::Equation)
        .filter_map(|s| doc.connect(s).map(|cn| (s, cn)))
        .find(|(_, cn)| {
            let (f, t) = (norm_ref(&cn.from), norm_ref(&cn.to));
            (f == a && t == b) || (f == b && t == a)
        })
}

/// End of the line after byte `e`, when the rest of that line is blank or a `//` comment.
fn rest_of_line(text: &str, e: usize) -> Option<usize> {
    let eol = text[e..].find('\n').map_or(text.len(), |i| e + i);
    let rest = text[e..eol].trim();
    (rest.is_empty() || rest.starts_with("//")).then_some(eol)
}

/// Delete the statement from token `first` to `last`, with its line (and a trailing `//`
/// comment) if it has the line to itself.
fn delete_stmt(doc: &Doc, first: usize, last: usize, p: &mut Patch) {
    let text = doc.text;
    let (s, e) = (doc.s(first), doc.e(last));
    let rest = &text[e..];
    match (lead_indent(text, s), rest_of_line(text, e)) {
        (Some(_), Some(eol)) => p.replace(line_start(text, s), (eol + 1).min(text.len()), ""),
        _ => {
            // inline: also drop the spaces after it
            let ws = rest.len() - rest.trim_start_matches([' ', '\t']).len();
            p.replace(s, e + ws, "");
        }
    }
}

/// Delete the keyword of an equation section whose equations were all deleted (the
/// section an added connection created).
fn drop_empty_sections(doc: &Doc, c: &ClassSyn, gone: &[usize], p: &mut Patch) {
    for (i, st) in c.stmts.iter().enumerate() {
        if st.kind != SK::Section || st.sec != Sec::Equation {
            continue;
        }
        let end = c.stmts[i + 1..]
            .iter()
            .position(|s| s.kind == SK::Section)
            .map_or(c.stmts.len(), |k| i + 1 + k);
        let mut body =
            (i + 1..end).filter(|&k| c.stmts[k].depth == 0 && c.stmts[k].kind != SK::Annotation);
        let first = body.next();
        if first.is_some() && first.into_iter().chain(body).all(|k| gone.contains(&k)) {
            delete_stmt(doc, st.first, st.last, p);
        }
    }
}

/// Insert `line` before token `anchor`: on a line of its own when the anchor starts a
/// line, else inline.
fn insert_before(doc: &Doc, anchor: usize, indent: &str, line: &str, p: &mut Patch) {
    let at = doc.s(anchor);
    if lead_indent(doc.text, at).is_some() {
        p.insert(line_start(doc.text, at), format!("{indent}{line}\n"));
    } else {
        p.insert(at, format!("{line} "));
    }
}

/// Insert `line` after the statement ending at token `last`: on a new line after a
/// trailing `//` comment, or inline when more follows on the same line.
fn insert_after(doc: &Doc, last: usize, indent: &str, line: &str, p: &mut Patch) {
    let e = doc.e(last);
    match rest_of_line(doc.text, e) {
        Some(eol) => p.insert(eol, format!("\n{indent}{line}")),
        None => p.insert(e, format!(" {line}")),
    }
}

/// Indentation for a new statement after `prev` (a statement whose indentation is
/// copied) or else one level below the class header.
fn stmt_indent(doc: &Doc, c: &ClassSyn, prev: Option<&Stmt>) -> String {
    match prev.and_then(|s| lead_indent(doc.text, doc.s(s.first))) {
        Some(i) => i,
        None => format!(
            "{}{}",
            line_indent(doc.text, doc.s(c.start)),
            indent_unit(doc.text)
        ),
    }
}

/// Declare a component after the last public element, before any protected section,
/// equation or algorithm section, class annotation or `end`.
fn insert_element(doc: &Doc, c: &ClassSyn, decl: &str, p: &mut Patch) {
    let anchor = c
        .stmts
        .iter()
        .find(|s| (s.kind == SK::Section && s.sec != Sec::Public) || s.kind == SK::Annotation)
        .map_or(c.end_tok, |s| s.first);
    let prev = c
        .stmts
        .iter()
        .rev()
        .find(|s| s.last < anchor && matches!(s.kind, SK::Element | SK::Extends | SK::Import));
    let indent = stmt_indent(doc, c, prev);
    match prev {
        // directly after the previous declaration, keeping blank lines and comments that
        // follow it where they are
        Some(prev) if lead_indent(doc.text, doc.s(prev.first)).is_some() => {
            insert_after(doc, prev.last, &indent, decl, p)
        }
        _ => insert_before(doc, anchor, &indent, decl, p),
    }
}

/// Add an equation after the last connect equation, else at the end of the first
/// equation section, creating the section when the class has none.
fn insert_equation(doc: &Doc, c: &ClassSyn, eq: &str, p: &mut Patch) {
    let last_conn = c
        .stmts
        .iter()
        .rfind(|s| s.sec == Sec::Equation && s.depth == 0 && doc.connect(s).is_some());
    if let Some(st) = last_conn
        && let Some(indent) = lead_indent(doc.text, doc.s(st.first))
    {
        insert_after(doc, st.last, &indent, eq, p);
        return;
    }
    if let Some(si) = c
        .stmts
        .iter()
        .position(|s| s.kind == SK::Section && s.sec == Sec::Equation)
    {
        // end of that section: the next section keyword, class annotation or `end`
        let next = c.stmts[si + 1..]
            .iter()
            .find(|s| s.kind == SK::Section || s.kind == SK::Annotation)
            .map_or(c.end_tok, |s| s.first);
        let prev = c.stmts[si + 1..]
            .iter()
            .rfind(|s| s.depth == 0 && s.last < next && s.kind != SK::Annotation);
        let indent = match prev {
            Some(_) => stmt_indent(doc, c, prev),
            None => format!(
                "{}{}",
                line_indent(doc.text, doc.s(c.stmts[si].first)),
                indent_unit(doc.text)
            ),
        };
        insert_before(doc, next, &indent, eq, p);
        return;
    }
    // No equation section: open one before the class annotation (if it is last) or `end`.
    let anchor = match c.stmts.last() {
        Some(s) if s.kind == SK::Annotation => s.first,
        _ => c.end_tok,
    };
    let cls_indent = line_indent(doc.text, doc.s(c.start));
    let unit = indent_unit(doc.text);
    let at = doc.s(anchor);
    if lead_indent(doc.text, at).is_some() {
        p.insert(
            line_start(doc.text, at),
            format!("{cls_indent}equation\n{cls_indent}{unit}{eq}\n"),
        );
    } else {
        p.insert(at, format!("equation {eq} "));
    }
}

/// The `transformation(...)` group of a declarator's Placement: (`(`, `)`) tokens.
fn transform_group(doc: &Doc, d: &Decl) -> Result<Option<(usize, usize)>> {
    let Some((_, open, _)) = d.ann else {
        return Ok(None);
    };
    let Some(pl) = doc
        .args(open)?
        .into_iter()
        .find(|a| a.name == "Placement" && a.sub.is_some())
    else {
        return Ok(None);
    };
    let (po, _) = pl.sub.unwrap();
    Ok(doc
        .args(po)?
        .into_iter()
        .find(|a| a.name == "transformation" && a.sub.is_some())
        .and_then(|a| a.sub))
}

fn read_transform(doc: &Doc, d: &Decl) -> Result<Option<Transform>> {
    let Some((o, c)) = transform_group(doc, d)? else {
        return Ok(None);
    };
    let src = format!("transformation{}", &doc.text[doc.s(o)..doc.e(c)]);
    let v =
        parse_annotation(&src).ok_or_else(|| anyhow!("cannot read the Placement of {}", d.name))?;
    let t = &v["transformation"];
    let ext = t["extent"]
        .as_array()
        .and_then(|a| Some([json_pt(a.first()?)?, json_pt(a.get(1)?)?]))
        .ok_or_else(|| anyhow!("the Placement of {} has no numeric extent", d.name))?;
    Ok(Some(Transform {
        origin: t.get("origin").and_then(json_pt),
        extent: ext,
        rotation: t.get("rotation").and_then(Value::as_f64).unwrap_or(0.0),
    }))
}

/// Rewrite the fields of a transformation that changed from `old` to `new`, value by
/// value, so the rest of the annotation keeps its layout.
fn write_transform(
    doc: &Doc,
    d: &Decl,
    old: &Transform,
    new: &Transform,
    p: &mut Patch,
) -> Result<()> {
    let (open, _) =
        transform_group(doc, d)?.ok_or_else(|| anyhow!("{} has no Placement", d.name))?;
    let flat = |e: [[f64; 2]; 2]| [e[0][0], e[0][1], e[1][0], e[1][1]];
    if new.extent != old.extent
        && !set_numbers(doc, open, "extent", &flat(old.extent), &flat(new.extent), p)?
    {
        set_arg(doc, open, "extent", &fmt_extent(new.extent), p)?;
    }
    if new.origin != old.origin {
        let o = new.origin.unwrap_or([0.0, 0.0]);
        let in_place = old
            .origin
            .is_some_and(|oo| set_numbers(doc, open, "origin", &oo, &o, p).unwrap_or(false));
        if !in_place {
            set_arg(doc, open, "origin", &fmt_point(o), p)?;
        }
    }
    if new.rotation != old.rotation {
        set_arg(doc, open, "rotation", &fmt_num(new.rotation), p)?;
    }
    Ok(())
}

/// Rewrite only the numbers that changed in the literal value of argument `name`
/// (`{{-10,-10},{10,10}}`), so line breaks and spacing inside it stay. False when the
/// value is not a literal holding exactly the numbers `old`.
fn set_numbers(
    doc: &Doc,
    open: usize,
    name: &str,
    old: &[f64],
    new: &[f64],
    p: &mut Patch,
) -> Result<bool> {
    let args = doc.args(open)?;
    let Some(Arg {
        value: Some((v0, v1)),
        ..
    }) = args.iter().find(|a| a.name == name)
    else {
        return Ok(false);
    };
    let mut slots = Vec::new();
    let mut j = *v0;
    while j <= *v1 {
        match doc.t(j) {
            "{" | "}" | "," => j += 1,
            "-" | "+" if doc.kind(j + 1) == Some(K::Num) => {
                let x: f64 = doc.t(j + 1).parse().unwrap_or(f64::NAN);
                slots.push((j, j + 1, if doc.t(j) == "-" { -x } else { x }));
                j += 2;
            }
            _ if doc.kind(j) == Some(K::Num) => {
                slots.push((j, j, doc.t(j).parse().unwrap_or(f64::NAN)));
                j += 1;
            }
            _ => return Ok(false),
        }
    }
    if slots.len() != old.len() || slots.iter().zip(old).any(|(s, o)| (s.2 - o).abs() > 1e-9) {
        return Ok(false);
    }
    for ((a, b, x), y) in slots.into_iter().zip(new) {
        if (x - y).abs() > 1e-12 {
            p.replace(doc.s(a), doc.e(b), fmt_num(*y));
        }
    }
    Ok(true)
}

/// Give a declarator without a Placement one.
fn add_placement(doc: &Doc, d: &Decl, t: &Transform, p: &mut Patch) {
    let mut fields = Vec::new();
    if let Some(o) = t.origin {
        fields.push(format!("origin={}", fmt_point(o)));
    }
    fields.push(format!("extent={}", fmt_extent(t.extent)));
    if t.rotation != 0.0 {
        fields.push(format!("rotation={}", fmt_num(t.rotation)));
    }
    let pl = format!("Placement(transformation({}))", fields.join(", "));
    match d.ann {
        Some((_, open, close)) => {
            let sep = if close == open + 1 { "" } else { ", " };
            p.insert(doc.e(open), format!("{pl}{sep}"));
        }
        None => p.insert(doc.e(d.last), format!(" annotation ({pl})")),
    }
}

/// Set `name=value` among the arguments of the group opening at `open`: replace an
/// existing value or append the argument.
fn set_arg(doc: &Doc, open: usize, name: &str, value: &str, p: &mut Patch) -> Result<()> {
    let args = doc.args(open)?;
    match args.iter().find(|a| a.name == name) {
        Some(Arg {
            value: Some((v0, v1)),
            ..
        }) => p.replace(doc.s(*v0), doc.e(*v1), value),
        Some(a) => p.insert(doc.e(a.last), format!("={value}")),
        None => {
            let close = doc.close(open)?;
            let sep = if args.is_empty() { "" } else { ", " };
            p.insert(doc.s(close), format!("{sep}{name}={value}"));
        }
    }
    Ok(())
}

/// Remove one argument of a group with its separating comma.
fn remove_arg(doc: &Doc, args: &[Arg], i: usize, p: &mut Patch) {
    let a = &args[i];
    if args.len() == 1 {
        p.replace(doc.s(a.first), doc.e(a.last), "");
    } else if i + 1 < args.len() {
        p.replace(doc.s(a.first), doc.s(args[i + 1].first), "");
    } else {
        p.replace(doc.e(args[i - 1].last), doc.e(a.last), "");
    }
}

fn set_modifier(
    doc: &Doc,
    d: &Decl,
    path: &[&str],
    value: Option<&str>,
    p: &mut Patch,
) -> Result<()> {
    let Some((open, close)) = d.modp else {
        if let Some(v) = value {
            // no modification yet: `C name[..](path=v)` right after the name and subscripts
            let mut at = d.name_tok;
            if doc.is(at + 1, "[") {
                at = doc.close(at + 1)?;
            }
            p.insert(doc.e(at), format!("({})", nested_mod(path, v)));
        }
        return Ok(());
    };
    // Walk down `x(start=...)` levels; `stack` holds the groups passed through.
    let mut stack: Vec<(usize, usize)> = Vec::new();
    let mut group = open;
    let mut k = 0;
    loop {
        let args = doc.args(group)?;
        let want = path[k..].join(".");
        let hit = args
            .iter()
            .position(|a| a.name == path[k])
            .or_else(|| args.iter().position(|a| a.name == want));
        let Some(i) = hit else {
            if let Some(v) = value {
                let gclose = doc.close(group)?;
                let sep = if args.is_empty() { "" } else { ", " };
                p.insert(doc.s(gclose), format!("{sep}{}", nested_mod(&path[k..], v)));
            }
            return Ok(());
        };
        let a = &args[i];
        let leaf = k + 1 == path.len() || a.name == want;
        if leaf {
            match value {
                Some(v) => match a.value {
                    Some((v0, v1)) => p.replace(doc.s(v0), doc.e(v1), v),
                    None => p.insert(doc.e(a.last), format!("={v}")),
                },
                None => match (a.value, a.sub) {
                    // `x(start=1)=2`: removing x's value keeps its sub-modification
                    (Some((_, v1)), Some((_, sc))) => p.replace(doc.e(sc), doc.e(v1), ""),
                    (Some(_), None) | (None, None) => {
                        remove_in(doc, &stack, &args, i, (open, close), p)?;
                    }
                    (None, Some(_)) => {}
                },
            }
            return Ok(());
        }
        let Some((so, _)) = a.sub else {
            // `x=1` but the path goes deeper: `x(start=v)=1`
            if let Some(v) = value {
                let mut name_end = a.first;
                while doc.ident(name_end) && matches!(doc.t(name_end), "each" | "final") {
                    name_end += 1;
                }
                while doc.is(name_end + 1, ".") && doc.ident(name_end + 2) {
                    name_end += 2;
                }
                p.insert(
                    doc.e(name_end),
                    format!("({})", nested_mod(&path[k + 1..], v)),
                );
            }
            return Ok(());
        };
        stack.push((group, i));
        group = so;
        k += 1;
    }
}

/// Remove argument `i` of `group`, and parents that become empty, and the parentheses of
/// the declarator when nothing is left.
fn remove_in(
    doc: &Doc,
    stack: &[(usize, usize)],
    args: &[Arg],
    i: usize,
    parens: (usize, usize),
    p: &mut Patch,
) -> Result<()> {
    if args.len() > 1 {
        remove_arg(doc, args, i, p);
        return Ok(());
    }
    // the group becomes empty
    match stack.last() {
        Some(&(pg, pi)) => {
            let pargs = doc.args(pg)?;
            if pargs[pi].value.is_some() {
                // `x(start=1)=2` -> `x=2`
                let (so, sc) = pargs[pi].sub.unwrap();
                p.replace(doc.s(so), doc.e(sc), "");
                Ok(())
            } else {
                remove_in(doc, &stack[..stack.len() - 1], &pargs, pi, parens, p)
            }
        }
        None => {
            p.replace(doc.s(parens.0), doc.e(parens.1), "");
            Ok(())
        }
    }
}

/// `a.b.c` and `v` -> `a(b(c=v))`.
fn nested_mod(path: &[&str], v: &str) -> String {
    match path {
        [last] => format!("{last}={v}"),
        [first, rest @ ..] => format!("{first}({})", nested_mod(rest, v)),
        [] => v.to_string(),
    }
}

/// Rename references to component `old` in the class: in equations and algorithms, and in
/// bindings and modifier values of other declarations (not in annotations).
fn rename_refs(doc: &Doc, c: &ClassSyn, old: &str, new: &str, decl_tok: usize, p: &mut Patch) {
    for st in &c.stmts {
        let (lo, hi) = match st.kind {
            SK::Equation | SK::Compound if st.depth == 0 => (st.first, st.last),
            SK::Element | SK::Extends => (st.first, st.last),
            _ => continue,
        };
        let decls = if st.kind == SK::Element {
            doc.decls(st)
        } else {
            Vec::new()
        };
        let mut depth = 0i32;
        let mut j = lo;
        while j <= hi {
            let t = doc.t(j);
            if doc.ident(j) && t == "annotation" && doc.is(j + 1, "(") {
                j = doc.close(j + 1).unwrap_or(hi) + 1;
                continue;
            }
            match t {
                "(" | "{" | "[" => depth += 1,
                ")" | "}" | "]" => depth -= 1,
                _ => {}
            }
            let is_decl_part = decls
                .iter()
                .any(|d| j == d.name_tok || (j >= d.type_first && j <= d.type_last));
            if j != decl_tok
                && doc.ident(j)
                && t == old
                && !is_decl_part
                && !(j > 0 && doc.is(j - 1, "."))
                // a modifier or named-argument name, `(k=...)`
                && !(depth > 0 && doc.is(j + 1, "="))
                && !(st.kind != SK::Equation && st.kind != SK::Compound && doc.is(j + 1, "("))
            {
                p.replace(doc.s(j), doc.e(j), new);
            }
            j += 1;
        }
    }
}

// ---------------------------------------------------------------- library level

/// Result of [`edit_class`].
#[derive(Debug, Clone, Serialize)]
pub struct EditResult {
    /// Document that holds the class.
    pub uri: String,
    /// The document's new text.
    pub text: String,
    /// Names of components added, in op order.
    pub added: Vec<String>,
}

/// The document and source text holding class `path`.
pub fn class_document<'l>(lib: &'l Library, path: &str) -> Result<(&'l str, &'l str)> {
    let tree = lib.tree();
    let id = tree
        .find_path(path)
        .ok_or_else(|| anyhow!("class {path} not found"))?;
    let doc = lib
        .docs
        .get(tree.node(id).source)
        .ok_or_else(|| anyhow!("class {path} has no source document"))?;
    Ok((&doc.uri, &doc.text))
}

/// Apply edits to class `path` of the library and return the document's new text. The
/// library itself is not changed. The new text must parse without new errors.
pub fn edit_class(lib: &Library, path: &str, ops: &[EditOp]) -> Result<EditResult> {
    let (uri, text) = class_document(lib, path)?;
    let taken: Vec<String> = inspect::class_info(lib, path)
        .map(|i| i.components.into_iter().map(|c| c.name).collect())
        .unwrap_or_default();
    let mut cur = text.to_string();
    let mut added = Vec::new();
    for op in ops {
        let a = apply_op(&cur, path, op, &taken)?;
        cur = a.text;
        added.extend(a.name);
    }
    check_parses(uri, text, &cur)?;
    Ok(EditResult {
        uri: uri.to_string(),
        text: cur,
        added,
    })
}

/// Fail when `new` has parse errors that `old` did not have.
pub fn check_parses(uri: &str, old: &str, new: &str) -> Result<()> {
    let after = parse_document(uri, new).errors;
    if after.is_empty() {
        return Ok(());
    }
    let before = parse_document(uri, old).errors.len();
    if after.len() > before {
        bail!("the edit does not parse: {}", after[0]);
    }
    Ok(())
}

/// Copy class `path` into package `package` of the document `target` (the document text,
/// `None` when it does not exist yet), with every class name it uses written out in full
/// so it means the same in its new place. Returns the new document text and the path of
/// the copy.
pub fn duplicate_class(
    lib: &Library,
    path: &str,
    target: Option<&str>,
    package: &str,
) -> Result<(String, String)> {
    let (_, text) = class_document(lib, path)?;
    let doc = Doc::new(text)?;
    let c = doc
        .find(path)
        .ok_or_else(|| anyhow!("class {path} not found in its document"))?;
    let tree = lib.tree();
    let id = tree.find_path(path).unwrap();
    let locals: Vec<String> = inspect::class_info(lib, path)
        .map(|i| i.components.into_iter().map(|c| c.name).collect())
        .unwrap_or_default();
    let mut p = Patch::default();
    // Qualify dotted names that resolve to classes outside the copied class.
    let (lo, hi) = (c.name_tok + 1, c.semi);
    let mut j = lo;
    while j < hi {
        if doc.ident(j) && doc.is(j, "annotation") && doc.is(j + 1, "(") {
            j = doc.close(j + 1)? + 1;
            continue;
        }
        let starts_name = doc.ident(j)
            && !(doc.is(j - 1, "."))
            && !doc.is(j - 1, "end")
            && !(doc.ident(j - 1) && CLASS_KINDS.contains(&doc.t(j - 1)))
            && !doc.is(j + 1, "=")
            && !KEYWORDS.contains(&doc.t(j))
            && !locals.iter().any(|l| l == doc.t(j));
        if !starts_name {
            j += 1;
            continue;
        }
        let mut parts = vec![doc.t(j)];
        let mut k = j;
        while doc.is(k + 1, ".") && doc.ident(k + 2) {
            parts.push(doc.t(k + 2));
            k += 2;
        }
        // the longest prefix that names a class
        for n in (1..=parts.len()).rev() {
            if let Some(found) = tree.lookup(id, &parts[..n], None) {
                let full = tree.node(found).path.clone();
                let inside = full == path || full.starts_with(&format!("{path}."));
                if !inside {
                    let last = j + 2 * (n - 1);
                    if doc.text[doc.s(j)..doc.e(last)] != full {
                        p.replace(doc.s(j), doc.e(last), full);
                    }
                }
                break;
            }
        }
        j = k + 1;
    }
    // The copy gets a free name in the target package.
    let target_text = target
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!("package {package} \"Models made in the workbench\"\nend {package};\n")
        });
    let tdoc = Doc::new(&target_text)?;
    let pkg = tdoc
        .find(package)
        .ok_or_else(|| anyhow!("package {package} not found in the target document"))?;
    let mut name = c.name.clone();
    let mut n = 1;
    while pkg.classes.iter().any(|k| k.name == name) {
        n += 1;
        name = format!("{}{n}", c.name);
    }
    if name != c.name {
        p.replace(doc.s(c.name_tok), doc.e(c.name_tok), name.clone());
        if !c.short {
            p.replace(doc.s(c.end_tok + 1), doc.e(c.end_tok + 1), name.clone());
        }
    }
    let body = &Patch::apply(
        Patch(
            p.0.into_iter()
                .map(|(s, e, r)| (s - doc.s(c.start), e - doc.s(c.start), r))
                .collect(),
        ),
        &text[doc.s(c.start)..doc.e(c.semi)],
    );
    // A copy is never partial or encapsulated just because its original was a base class.
    let body = body.strip_prefix("partial ").unwrap_or(body);
    let old_indent = line_indent(text, doc.s(c.start));
    let unit = indent_unit(&target_text);
    let new_indent = format!("{}{unit}", line_indent(&target_text, tdoc.s(pkg.start)));
    let mut out = String::new();
    for (i, l) in body.split('\n').enumerate() {
        if i > 0 {
            out.push('\n');
            out.push_str(&new_indent);
            out.push_str(l.strip_prefix(old_indent.as_str()).unwrap_or(l));
        } else {
            out.push_str(&new_indent);
            out.push_str(l);
        }
    }
    let mut tp = Patch::default();
    insert_before(&tdoc, pkg.end_tok, "", &out, &mut tp);
    let new_text = tp.apply(&target_text);
    check_parses("copy.mo", &target_text, &new_text)?;
    Ok((new_text, format!("{package}.{name}")))
}

/// Add an empty model `name` to package `package` of a document (created when `target`
/// is `None`). Returns the new text and the model's path.
pub fn new_model(target: Option<&str>, package: &str, name: &str) -> Result<(String, String)> {
    if !valid_ident(name) {
        bail!("'{name}' is not a valid class name");
    }
    let target_text = target
        .filter(|t| !t.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!("package {package} \"Models made in the workbench\"\nend {package};\n")
        });
    let tdoc = Doc::new(&target_text)?;
    let pkg = tdoc
        .find(package)
        .ok_or_else(|| anyhow!("package {package} not found in the target document"))?;
    if pkg.classes.iter().any(|k| k.name == name) {
        bail!("{package} already has a class {name}");
    }
    let unit = indent_unit(&target_text);
    let ind = format!("{}{unit}", line_indent(&target_text, tdoc.s(pkg.start)));
    let model = format!(
        "{ind}model {name}\n{ind}{unit}annotation (experiment(StopTime=1));\n{ind}end {name};"
    );
    let mut tp = Patch::default();
    insert_before(&tdoc, pkg.end_tok, "", &model, &mut tp);
    let new_text = tp.apply(&target_text);
    check_parses("new.mo", &target_text, &new_text)?;
    Ok((new_text, format!("{package}.{name}")))
}

/// A parameter of a class as the parameter dialog lists it.
#[derive(Debug, Clone, Serialize)]
pub struct ParamInfo {
    pub name: String,
    pub type_name: String,
    pub unit: String,
    pub description: String,
    /// The binding in the class (its default), as source text.
    pub default: Option<String>,
    /// Class that declares it.
    pub declared_in: String,
}

/// Public parameters of class `path`, inherited ones included, with their units.
pub fn parameters(lib: &Library, path: &str) -> Vec<ParamInfo> {
    let Some(info) = inspect::class_info(lib, path) else {
        return Vec::new();
    };
    let tree = lib.tree();
    info.components
        .iter()
        .filter(|c| c.variability == "parameter" && !c.protected)
        .map(|c| {
            let unit = c
                .modifiers
                .iter()
                .find(|(k, _)| k == "unit")
                .map(|(_, v)| v.trim_matches('"').to_string())
                .or_else(|| {
                    let id = tree.find_path(c.type_path.as_deref()?)?;
                    type_unit(&tree, id, 0)
                })
                .unwrap_or_default();
            ParamInfo {
                name: c.name.clone(),
                type_name: c.type_name.clone(),
                unit,
                description: c.description.clone(),
                default: c.binding.clone(),
                declared_in: c.declared_in.clone(),
            }
        })
        .collect()
}

/// `unit` of a type such as `type Time = Real(final unit="s")`, following its bases.
fn type_unit(
    tree: &super::tree::ClassTree,
    id: super::tree::ClassId,
    depth: usize,
) -> Option<String> {
    use rumoca_ir_ast as ast;
    if depth > 8 {
        return None;
    }
    let def = tree.def(id)?;
    for ext in &def.extends {
        for m in &ext.modifications {
            if let ast::Expression::Modification { target, value, .. } = &m.expr
                && target.parts.len() == 1
                && &*target.parts[0].ident.text == "unit"
                && let Value::String(s) = inspect::expr_json(value, None)
            {
                return Some(s);
            }
        }
    }
    tree.bases(id)
        .into_iter()
        .find_map(|b| type_unit(tree, b, depth + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lexes_comments_strings_and_numbers() {
        let src = "a.b /* c ; */ \"s;\\\"\" 1.5e-3 // x\n'q x' <= ;";
        let toks: Vec<&str> = lex(src).iter().map(|t| &src[t.s..t.e]).collect();
        assert_eq!(
            toks,
            ["a", ".", "b", "\"s;\\\"\"", "1.5e-3", "'q x'", "<=", ";"]
        );
    }

    #[test]
    fn scans_compound_equations() {
        let src = "model M\n  Real x;\nequation\n  if x > 0 then\n    x = if a then 1 else 2;\n  elseif b then\n    for i in 1:2 loop\n      connect(a, b);\n    end for;\n  else\n    x = 0;\n  end if;\n  annotation (Icon());\nend M;\n";
        let d = Doc::new(src).unwrap();
        let c = d.find("M").unwrap();
        let kinds: Vec<SK> = c.stmts.iter().map(|s| s.kind).collect();
        assert_eq!(
            kinds,
            [
                SK::Element,
                SK::Section,
                SK::Compound,
                SK::Equation,
                SK::Compound,
                SK::Equation,
                SK::Equation,
                SK::Annotation
            ]
        );
    }

    #[test]
    fn fmt_numbers() {
        assert_eq!(fmt_num(10.0), "10");
        assert_eq!(fmt_num(-0.0), "0");
        assert_eq!(fmt_num(2.5), "2.5");
        assert_eq!(fmt_num(1.0 / 3.0), "0.3333");
    }
}
