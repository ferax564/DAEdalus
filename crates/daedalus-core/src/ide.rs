//! Editor intelligence over a [`Library`]: go to definition, find references, rename,
//! completion, hover, document outline and diagnostics.
//!
//! The analysis is lexical first and semantic second. Every request lexes the documents
//! it looks at (cheap, and always in step with the text even while it does not parse),
//! finds the class structure (`model M ... end M;`) and classifies each dotted name by
//! its syntactic context: a type in a declaration, the declared component, an `extends`
//! or `import` target, a modifier name inside `(...)`, or an ordinary reference in an
//! equation or binding. Names are then resolved against the class tree with the Modelica
//! lookup rules of [`ClassTree`] (enclosing scopes, imports, inherited members), and
//! component prefixes (`a.b.c`) through the declared types of the components.
//!
//! Only the edited document is re-parsed per keystroke ([`Ide::update`]); while it does
//! not parse, the last version that did stands in for it in the class tree, so completion
//! keeps seeing its components.

use crate::frontend::Library;
use crate::frontend::tree::{ClassId, ClassTree, ROOT, builtin_name};
use rumoca_ir_ast as ast;
use rustc_hash::{FxHashMap, FxHashSet};
use serde::Serialize;
use std::cell::RefCell;
use std::rc::Rc;

const NONE: u32 = u32::MAX;

// ------------------------------------------------------------------------------ lexer

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum K {
    Ident,
    Kw,
    Num,
    Str,
    Op,
}

#[derive(Clone, Copy, Debug)]
struct Tok {
    k: K,
    s: usize,
    e: usize,
}

/// Modelica keywords, sorted (binary search).
pub const KEYWORDS: &[&str] = &[
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
];

fn is_keyword(s: &str) -> bool {
    KEYWORDS.binary_search(&s).is_ok()
}

const CLASS_KW: &[&str] = &[
    "block",
    "class",
    "connector",
    "function",
    "model",
    "operator",
    "package",
    "record",
    "type",
];

/// Builtin functions and operators (offered by completion, never reported as unknown).
const BUILTIN_FUNCTIONS: &[&str] = &[
    "abs",
    "acos",
    "activeState",
    "actualStream",
    "array",
    "asin",
    "assert",
    "atan",
    "atan2",
    "backSample",
    "cardinality",
    "cat",
    "ceil",
    "change",
    "cos",
    "cosh",
    "cross",
    "delay",
    "der",
    "diagonal",
    "div",
    "edge",
    "exp",
    "fill",
    "firstTick",
    "floor",
    "getInstanceName",
    "hold",
    "homotopy",
    "identity",
    "inStream",
    "initial",
    "initialState",
    "integer",
    "interval",
    "linspace",
    "log",
    "log10",
    "matrix",
    "max",
    "min",
    "mod",
    "ndims",
    "noClock",
    "noEvent",
    "ones",
    "outerProduct",
    "pre",
    "previous",
    "product",
    "reinit",
    "rem",
    "sample",
    "scalar",
    "semiLinear",
    "shiftSample",
    "sign",
    "sin",
    "sinh",
    "size",
    "skew",
    "smooth",
    "spatialDistribution",
    "sqrt",
    "subSample",
    "sum",
    "superSample",
    "symmetric",
    "tan",
    "tanh",
    "terminal",
    "terminate",
    "ticksInState",
    "timeInState",
    "transition",
    "transpose",
    "vector",
    "zeros",
    "print",
    "isPresent",
];

const BUILTIN_TYPES: &[&str] = &[
    "Real",
    "Integer",
    "Boolean",
    "String",
    "StateSelect",
    "AssertionLevel",
    "ExternalObject",
    "Clock",
    "Connections",
    "Subtask",
    "Modelica_Clocked",
];

const BUILTIN_VARS: &[&str] = &["time", "end"];

fn attributes(ty: &str) -> &'static [&'static str] {
    match ty {
        "Real" => &[
            "quantity",
            "unit",
            "displayUnit",
            "min",
            "max",
            "start",
            "fixed",
            "nominal",
            "unbounded",
            "stateSelect",
        ],
        "Integer" | "enumeration" => &["quantity", "min", "max", "start", "fixed"],
        "Boolean" | "String" => &["quantity", "start", "fixed"],
        _ => &[],
    }
}

struct Lexed {
    toks: Vec<Tok>,
    /// Comments and string literals: no completion inside them.
    skips: Vec<(usize, usize)>,
}

fn lex(text: &str) -> Lexed {
    let b = text.as_bytes();
    let n = b.len();
    let mut i = 0;
    let mut toks = Vec::with_capacity(n / 4);
    let mut skips = Vec::new();
    while i < n {
        let c = b[i];
        if c.is_ascii_whitespace() {
            i += 1;
        } else if c == b'/' && i + 1 < n && b[i + 1] == b'/' {
            let s = i;
            while i < n && b[i] != b'\n' {
                i += 1;
            }
            skips.push((s, i));
        } else if c == b'/' && i + 1 < n && b[i + 1] == b'*' {
            let s = i;
            i += 2;
            while i + 1 < n && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(n);
            skips.push((s, i));
        } else if c == b'"' || c == b'\'' {
            let s = i;
            i += 1;
            while i < n && b[i] != c {
                if b[i] == b'\\' {
                    i += 1;
                }
                i += 1;
            }
            i = (i + 1).min(n);
            if c == b'"' {
                skips.push((s, i));
                toks.push(Tok { k: K::Str, s, e: i });
            } else {
                toks.push(Tok {
                    k: K::Ident,
                    s,
                    e: i,
                });
            }
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let s = i;
            while i < n && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            let k = if is_keyword(&text[s..i]) {
                K::Kw
            } else {
                K::Ident
            };
            toks.push(Tok { k, s, e: i });
        } else if c.is_ascii_digit() {
            let s = i;
            while i < n && b[i].is_ascii_digit() {
                i += 1;
            }
            if i + 1 < n && b[i] == b'.' && b[i + 1].is_ascii_digit() {
                i += 1;
                while i < n && b[i].is_ascii_digit() {
                    i += 1;
                }
            } else if i < n
                && b[i] == b'.'
                && !(i + 1 < n
                    && b[i + 1].is_ascii_alphabetic()
                    && b[i + 1] != b'e'
                    && b[i + 1] != b'E')
            {
                i += 1;
            }
            if i < n && (b[i] == b'e' || b[i] == b'E') {
                let mut j = i + 1;
                if j < n && (b[j] == b'+' || b[j] == b'-') {
                    j += 1;
                }
                if j < n && b[j].is_ascii_digit() {
                    i = j;
                    while i < n && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            toks.push(Tok { k: K::Num, s, e: i });
        } else {
            let s = i;
            let two = if i + 1 < n {
                &text.as_bytes()[i..i + 2]
            } else {
                &[][..]
            };
            let len = match two {
                b":=" | b"==" | b"<=" | b">=" | b"<>" | b".*" | b"./" | b".^" | b".+" | b".-" => 2,
                _ => text[i..].chars().next().map(|c| c.len_utf8()).unwrap_or(1),
            };
            i += len;
            toks.push(Tok { k: K::Op, s, e: i });
        }
    }
    Lexed { toks, skips }
}

// ---------------------------------------------------------------------- document analysis

/// A class found by the structure scan.
#[derive(Debug, Clone)]
struct LClass {
    name: String,
    path: String,
    kind: String,
    name_tok: usize,
    /// First and last token of the class (header keyword to `end X;`).
    start: usize,
    end: usize,
    parent: u32,
    short: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Ctx {
    /// A class name: declaration type, `constrainedby`, short-class base.
    Type,
    Extends,
    Within,
    Import,
    /// A name inside `import A.B.{x, y}`: member of the chain given.
    ImportSel(u32),
    /// The name of a class in its header or `end` clause.
    ClassName(u32),
    /// The name of a declared component.
    Decl,
    /// A modifier (or named argument) applying to what the chain given denotes.
    Modifier(u32),
    Ref,
    ForIndex,
    Unknown,
}

#[derive(Debug)]
struct Chain {
    /// Identifier tokens of `a.b[1].c`.
    toks: Vec<usize>,
    ctx: Ctx,
    /// Lexical class holding the chain.
    cls: u32,
    global: bool,
    /// For a declared component: the chain of its type.
    typ: u32,
}

struct DocAn<'t> {
    text: &'t str,
    toks: Vec<Tok>,
    skips: Vec<(usize, usize)>,
    classes: Vec<LClass>,
    tok_class: Vec<u32>,
    chains: Vec<Chain>,
    tok_chain: Vec<u32>,
    /// Chain ending just before a token (the owner of a `(`).
    ends_at: FxHashMap<usize, u32>,
    /// `for i in`: (lexical class, token).
    for_idx: Vec<(u32, usize)>,
    /// The `within` package.
    within: String,
}

impl<'t> DocAn<'t> {
    fn tt(&self, i: usize) -> &'t str {
        &self.text[self.toks[i].s..self.toks[i].e]
    }

    fn is_op(&self, i: usize, o: &str) -> bool {
        i < self.toks.len() && self.toks[i].k == K::Op && self.tt(i) == o
    }

    fn is_kw(&self, i: usize, w: &str) -> bool {
        i < self.toks.len() && self.toks[i].k == K::Kw && self.tt(i) == w
    }

    fn is_ident(&self, i: usize) -> bool {
        i < self.toks.len() && self.toks[i].k == K::Ident
    }

    /// Index of the token containing (or ending at) `off`, preferring identifiers.
    fn token_at(&self, off: usize) -> Option<usize> {
        let i = self.toks.partition_point(|t| t.e < off);
        let mut best = None;
        for j in i..(i + 2).min(self.toks.len()) {
            let t = self.toks[j];
            if t.s <= off && off <= t.e {
                if t.k == K::Ident {
                    return Some(j);
                }
                best.get_or_insert(j);
            }
        }
        best
    }

    fn in_skip(&self, off: usize) -> bool {
        let i = self.skips.partition_point(|&(_, e)| e <= off);
        i < self.skips.len() && self.skips[i].0 < off
    }

    fn class_by_path(&self, path: &str) -> Option<u32> {
        self.classes
            .iter()
            .position(|c| c.path == path)
            .map(|i| i as u32)
    }

    fn line_col(&self, off: usize) -> (usize, usize) {
        let before = &self.text[..off.min(self.text.len())];
        let line = before.bytes().filter(|&b| b == b'\n').count();
        let col = off - before.rfind('\n').map(|p| p + 1).unwrap_or(0);
        (line, col)
    }

    fn line_text(&self, off: usize) -> String {
        let s = self.text[..off].rfind('\n').map(|p| p + 1).unwrap_or(0);
        let e = self.text[off..]
            .find('\n')
            .map(|p| off + p)
            .unwrap_or(self.text.len());
        self.text[s..e].trim().to_string()
    }

    fn is_ancestor(&self, a: u32, mut c: u32) -> bool {
        while c != NONE {
            if c == a {
                return true;
            }
            c = self.classes[c as usize].parent;
        }
        false
    }
}

fn analyze(text: &str) -> DocAn<'_> {
    let Lexed { toks, skips } = lex(text);
    let mut an = DocAn {
        text,
        tok_class: vec![NONE; toks.len()],
        tok_chain: vec![NONE; toks.len()],
        toks,
        skips,
        classes: Vec::new(),
        chains: Vec::new(),
        ends_at: FxHashMap::default(),
        for_idx: Vec::new(),
        within: String::new(),
    };
    let n = an.toks.len();

    // ---- structure: within, class headers and `end` clauses
    let mut within: Vec<&str> = Vec::new();
    let mut stack: Vec<u32> = Vec::new();
    let mut header_of: FxHashMap<usize, u32> = FxHashMap::default();
    let mut i = 0;
    while i < n {
        if an.is_kw(i, "within") {
            let mut j = i + 1;
            while j < n && !an.is_op(j, ";") {
                if an.is_ident(j) {
                    within.push(an.tt(j));
                }
                j += 1;
            }
            i = j + 1;
            continue;
        }
        if an.toks[i].k == K::Kw
            && CLASS_KW.contains(&an.tt(i))
            && !(i > 0 && an.is_kw(i - 1, "end"))
        {
            let mut kind = an.tt(i).to_string();
            let mut j = i + 1;
            if kind == "operator" && (an.is_kw(j, "record") || an.is_kw(j, "function")) {
                kind = format!("operator {}", an.tt(j));
                j += 1;
            }
            if kind == "connector" && i > 0 && an.is_kw(i - 1, "expandable") {
                kind = "expandable connector".into();
            }
            let class_extends = an.is_kw(j, "extends");
            if class_extends {
                j += 1;
            }
            if an.is_ident(j) && !an.is_op(j + 1, ".") && (class_extends || !an.is_op(j + 1, "(")) {
                let parent = stack.last().copied().unwrap_or(NONE);
                let name = an.tt(j).to_string();
                let path = if parent != NONE {
                    format!("{}.{}", an.classes[parent as usize].path, name)
                } else if within.is_empty() {
                    name.clone()
                } else {
                    format!("{}.{}", within.join("."), name)
                };
                let idx = an.classes.len() as u32;
                if an.is_op(j + 1, "=") {
                    let mut k = j + 2;
                    let mut d = 0i32;
                    while k < n {
                        if an.is_op(k, "(") || an.is_op(k, "[") || an.is_op(k, "{") {
                            d += 1;
                        } else if an.is_op(k, ")") || an.is_op(k, "]") || an.is_op(k, "}") {
                            if d == 0 {
                                break;
                            }
                            d -= 1;
                        } else if d == 0 && (an.is_op(k, ";") || an.is_op(k, ",")) {
                            break;
                        }
                        k += 1;
                    }
                    an.classes.push(LClass {
                        name,
                        path,
                        kind,
                        name_tok: j,
                        start: i,
                        end: k.min(n - 1),
                        parent,
                        short: true,
                    });
                } else {
                    an.classes.push(LClass {
                        name,
                        path,
                        kind,
                        name_tok: j,
                        start: i,
                        end: n - 1,
                        parent,
                        short: false,
                    });
                    stack.push(idx);
                }
                header_of.insert(j, idx);
                i = j + 1;
                continue;
            }
        }
        if an.is_kw(i, "end") && an.is_ident(i + 1) {
            if let Some(c) = stack.pop() {
                let end = if an.is_op(i + 2, ";") { i + 2 } else { i + 1 };
                an.classes[c as usize].end = end;
                header_of.insert(i + 1, c);
            }
            i += 2;
            continue;
        }
        i += 1;
    }
    an.within = within.join(".");
    for (ci, c) in an.classes.iter().enumerate() {
        if c.short {
            continue;
        }
        for t in c.start..=c.end.min(n.saturating_sub(1)) {
            an.tok_class[t] = ci as u32;
        }
    }

    // ---- names and their contexts
    struct Frame {
        skip: bool,
        owner: u32,
        paren: bool,
    }
    let mut frames: Vec<Frame> = Vec::new();
    let mut decl_depth: Option<usize> = None;
    let mut decl_type = NONE;
    let mut decl_next = usize::MAX;
    let mut in_import = false;
    let mut import_base = NONE;
    let mut external = false;
    for i in 0..n {
        if frames.last().is_some_and(|f| f.skip) {
            if an.is_op(i, "(") {
                frames.push(Frame {
                    skip: true,
                    owner: NONE,
                    paren: true,
                });
            } else if an.is_op(i, ")") {
                frames.pop();
            }
            continue;
        }
        if external {
            if an.is_op(i, ";") {
                external = false;
            }
            continue;
        }
        match an.toks[i].k {
            K::Kw => match an.tt(i) {
                "external" => external = true,
                "import" => {
                    in_import = true;
                    import_base = NONE;
                }
                _ => {}
            },
            K::Op => match an.tt(i) {
                ";" => {
                    in_import = false;
                    if decl_depth == Some(frames.len()) {
                        decl_depth = None;
                    }
                }
                "(" => {
                    let skip =
                        i > 0 && (an.is_kw(i - 1, "annotation") || an.is_kw(i - 1, "enumeration"));
                    let owner = an.ends_at.get(&i).copied().unwrap_or(NONE);
                    frames.push(Frame {
                        skip,
                        owner,
                        paren: true,
                    });
                }
                o @ ("[" | "{") => {
                    if o == "{" && in_import && i > 0 {
                        import_base = an.ends_at.get(&(i - 1)).copied().unwrap_or(NONE);
                    }
                    frames.push(Frame {
                        skip: false,
                        owner: NONE,
                        paren: false,
                    });
                }
                ")" | "]" | "}" => {
                    frames.pop();
                    if decl_depth.is_some_and(|d| d > frames.len()) {
                        decl_depth = None;
                    }
                }
                "," => {
                    if decl_depth == Some(frames.len()) {
                        if frames.is_empty() && an.is_ident(i + 1) {
                            decl_next = i + 1;
                        } else {
                            // a redeclaration inside a modification declares one element
                            decl_depth = None;
                        }
                    }
                }
                _ => {}
            },
            K::Ident if an.tok_chain[i] == NONE => {
                // collect `a.b[i].c`
                let mut parts = vec![i];
                let mut j = i;
                let after = loop {
                    let mut k = j + 1;
                    if an.is_op(k, "[") {
                        let mut d = 0;
                        while k < n {
                            if an.is_op(k, "[") {
                                d += 1;
                            } else if an.is_op(k, "]") {
                                d -= 1;
                                if d == 0 {
                                    break;
                                }
                            }
                            k += 1;
                        }
                        k += 1;
                    }
                    if an.is_op(k, ".") && an.is_ident(k + 1) {
                        j = k + 1;
                        parts.push(j);
                    } else {
                        break k;
                    }
                };
                let mut unknown = false;
                let mut global = false;
                let mut p = i.checked_sub(1);
                if i > 0 && an.is_op(i - 1, ".") {
                    if i >= 2
                        && (an.is_ident(i - 2) || an.is_op(i - 2, ")") || an.is_op(i - 2, "]"))
                    {
                        unknown = true;
                    } else {
                        global = true;
                        p = i.checked_sub(2);
                    }
                }
                let prev_kw = |w: &str| p.is_some_and(|p| an.is_kw(p, w));
                let prev_op = |o: &str| p.is_some_and(|p| an.is_op(p, o));
                let ci = an.chains.len() as u32;
                let mut typ = NONE;
                let ctx = if unknown {
                    Ctx::Unknown
                } else if let Some(&c) = header_of.get(&i) {
                    Ctx::ClassName(c)
                } else if i == decl_next {
                    typ = decl_type;
                    Ctx::Decl
                } else if prev_kw("within") {
                    Ctx::Within
                } else if prev_kw("extends") {
                    Ctx::Extends
                } else if prev_kw("constrainedby") {
                    Ctx::Type
                } else if in_import {
                    if import_base != NONE && frames.last().is_some_and(|f| !f.paren) {
                        Ctx::ImportSel(import_base)
                    } else if an.is_op(after, "=") {
                        Ctx::Unknown
                    } else {
                        Ctx::Import
                    }
                } else if prev_op("=")
                    && p.is_some_and(|p| {
                        p > 0
                            && header_of.get(&(p - 1)).is_some_and(|&c| {
                                an.classes[c as usize].short
                                    && an.classes[c as usize].name_tok == p - 1
                            })
                    })
                {
                    Ctx::Type
                } else if an.is_ident(after) {
                    decl_next = after;
                    decl_depth = Some(frames.len());
                    decl_type = ci;
                    Ctx::Type
                } else if an.is_kw(after, "in") {
                    an.for_idx.push((an.tok_class[i], i));
                    Ctx::ForIndex
                } else if let Some(f) = frames.last()
                    && f.paren
                    && f.owner != NONE
                    && (prev_op("(")
                        || prev_op(",")
                        || prev_kw("each")
                        || prev_kw("final")
                        || prev_kw("redeclare"))
                    && (an.is_op(after, "=") || an.is_op(after, "("))
                {
                    let oc = an.chains[f.owner as usize].ctx;
                    if an.is_op(after, "(") && oc == Ctx::Ref {
                        Ctx::Ref
                    } else {
                        Ctx::Modifier(f.owner)
                    }
                } else {
                    Ctx::Ref
                };
                for &t in &parts {
                    an.tok_chain[t] = ci;
                }
                an.ends_at.insert(after, ci);
                an.chains.push(Chain {
                    toks: parts,
                    ctx,
                    cls: an.tok_class[i],
                    global,
                    typ,
                });
            }
            _ => {}
        }
    }
    an
}

// ------------------------------------------------------------------------------ results

/// A place in a document. Offsets are bytes; `line` and `col` are 0-based (col in bytes).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Location {
    pub uri: String,
    pub start: usize,
    pub end: usize,
    pub line: usize,
    pub col: usize,
    /// The innermost class around the place (to open it in a class browser).
    pub class: Option<String>,
    /// The text of the line, trimmed.
    pub preview: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TextEdit {
    pub uri: String,
    pub start: usize,
    pub end: usize,
    pub new_text: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CompletionItem {
    pub label: String,
    /// class kind (`model`, `package`, ...), `variable`, `parameter`, `constant`,
    /// `property` (modifier / attribute), `function`, `keyword`, `enum`.
    pub kind: String,
    pub detail: String,
    pub info: String,
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct Completion {
    /// Start of the text the completion replaces (the identifier typed so far).
    pub from: usize,
    pub items: Vec<CompletionItem>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Hover {
    pub start: usize,
    pub end: usize,
    /// Markdown: a code block with the declaration, then the description.
    pub markdown: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct DocSymbol {
    pub name: String,
    /// Class kind, or `component` / `parameter` / `constant`.
    pub kind: String,
    pub detail: String,
    pub start: usize,
    pub end: usize,
    pub sel_start: usize,
    pub sel_end: usize,
    pub path: String,
    pub children: Vec<DocSymbol>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Diagnostic {
    pub start: usize,
    pub end: usize,
    /// `error` | `warning`
    pub severity: &'static str,
    pub message: String,
}

/// What a name denotes.
#[derive(Debug, Clone, PartialEq)]
enum Sym {
    Class(ClassId),
    /// Component `name` declared in class `owner`.
    Comp(ClassId, String),
    Enum(ClassId, String),
    /// A `for` index: document and byte range of its binding.
    Local(usize, usize, usize),
    Builtin(String, &'static str),
}

enum TypeRef {
    Class(ClassId),
    Builtin(String),
}

// ------------------------------------------------------------------------------ session

struct Session<'a> {
    lib: &'a Library,
    tree: ClassTree<'a>,
    ans: RefCell<FxHashMap<usize, Rc<DocAn<'a>>>>,
    memo: RefCell<FxHashMap<(usize, u32), Rc<Vec<Option<Sym>>>>>,
}

fn desc(toks: &[rumoca_core::Token]) -> String {
    toks.iter()
        .map(|t| t.text.trim_matches('"').to_string())
        .collect::<Vec<_>>()
        .join("")
}

fn string_value(e: &ast::Expression) -> Option<String> {
    match e {
        ast::Expression::Terminal { token, .. } => Some(token.text.trim_matches('"').to_string()),
        _ => None,
    }
}

impl<'a> Session<'a> {
    fn new(lib: &'a Library, ide: &'a Ide) -> Self {
        let docs: Vec<(&'a ast::StoredDefinition, usize)> = lib
            .docs
            .iter()
            .enumerate()
            .map(|(i, d)| {
                let ast = if d.errors.is_empty() {
                    &d.ast
                } else {
                    ide.last_good.get(&d.uri).map(|(_, a)| a).unwrap_or(&d.ast)
                };
                (ast, i)
            })
            .collect();
        Session {
            lib,
            tree: ClassTree::build(&docs),
            ans: Default::default(),
            memo: Default::default(),
        }
    }

    fn doc_index(&self, uri: &str) -> Option<usize> {
        self.lib.docs.iter().position(|d| d.uri == uri)
    }

    fn an(&self, d: usize) -> Rc<DocAn<'a>> {
        if let Some(a) = self.ans.borrow().get(&d) {
            return a.clone();
        }
        let a = Rc::new(analyze(&self.lib.docs[d].text));
        self.ans.borrow_mut().insert(d, a.clone());
        a
    }

    /// The tree class for a lexical class (or its nearest enclosing one that exists).
    fn scope(&self, an: &DocAn, cls: u32) -> ClassId {
        if cls == NONE && an.within.is_empty() {
            return ROOT;
        }
        let mut path: &str = if cls == NONE {
            &an.within
        } else {
            &an.classes[cls as usize].path
        };
        loop {
            if let Some(c) = self.tree.find_path(path) {
                return c;
            }
            match path.rfind('.') {
                Some(p) => path = &path[..p],
                None => return ROOT,
            }
        }
    }

    /// A replaceable class (or one inside a replaceable package): its members are known
    /// only once it is redeclared.
    fn replaceable(&self, c: ClassId) -> bool {
        let mut c = Some(c);
        while let Some(x) = c {
            if self.tree.def(x).is_some_and(|d| d.is_replaceable) {
                return true;
            }
            c = self.tree.node(x).parent;
        }
        false
    }

    fn comp_type(&self, owner: ClassId, name: &str) -> Option<TypeRef> {
        let (decl, comp) = self.tree.find_component(owner, name)?;
        let parts: Vec<&str> = comp.type_name.name.iter().map(|t| &*t.text).collect();
        if parts.len() == 1 && builtin_name(parts[0]).is_some() {
            return Some(TypeRef::Builtin(parts[0].to_string()));
        }
        self.tree.lookup(decl, &parts, None).map(TypeRef::Class)
    }

    fn enum_literal(&self, c: ClassId, name: &str, depth: usize) -> Option<ClassId> {
        if depth > 16 {
            return None;
        }
        let def = self.tree.def(c)?;
        if def.enum_literals.iter().any(|l| &*l.ident.text == name) {
            return Some(c);
        }
        self.tree
            .bases(c)
            .into_iter()
            .find_map(|b| self.enum_literal(b, name, depth + 1))
    }

    fn class_member(&self, c: ClassId, name: &str) -> Option<Sym> {
        if let Some(m) = self.tree.member_class(c, name) {
            return Some(Sym::Class(m));
        }
        if let Some((o, _)) = self.tree.find_component(c, name) {
            return Some(Sym::Comp(o, name.to_string()));
        }
        if let Some(e) = self.enum_literal(c, name, 0) {
            return Some(Sym::Enum(e, name.to_string()));
        }
        None
    }

    fn member(&self, s: &Sym, name: &str) -> Option<Sym> {
        match s {
            Sym::Class(c) => self.class_member(*c, name),
            Sym::Comp(o, n) => match self.comp_type(*o, n)? {
                TypeRef::Class(t) => self.class_member(t, name),
                TypeRef::Builtin(b) => attributes(&b)
                    .contains(&name)
                    .then(|| Sym::Builtin(name.to_string(), "attribute")),
            },
            Sym::Builtin(b, "type") if b == "StateSelect" || b == "AssertionLevel" => {
                Some(Sym::Builtin(format!("{b}.{name}"), "literal"))
            }
            Sym::Builtin(b, "type") if b == "Connections" => {
                Some(Sym::Builtin(format!("{b}.{name}"), "function"))
            }
            _ => None,
        }
    }

    fn target_of(&self, d: usize, owner: u32) -> Option<TypeRef> {
        let syms = self.resolve(d, owner);
        match syms.last()?.as_ref()? {
            Sym::Class(c) => Some(TypeRef::Class(*c)),
            Sym::Comp(o, n) => {
                // a declaration (or redeclaration): the type as written
                let an = self.an(d);
                let ch = &an.chains[owner as usize];
                if ch.ctx == Ctx::Decl && ch.typ != NONE {
                    let t = self.resolve(d, ch.typ);
                    match t.last().cloned().flatten() {
                        Some(Sym::Class(c)) => return Some(TypeRef::Class(c)),
                        Some(Sym::Builtin(b, "type")) => return Some(TypeRef::Builtin(b)),
                        _ => {}
                    }
                }
                self.comp_type(*o, n)
            }
            Sym::Builtin(b, "type") => Some(TypeRef::Builtin(b.clone())),
            _ => None,
        }
    }

    fn modifier(&self, t: &TypeRef, name: &str) -> Option<Sym> {
        match t {
            TypeRef::Class(c) => self.class_member(*c, name).or_else(|| {
                let b = self.tree.builtin_base(*c)?;
                attributes(b)
                    .contains(&name)
                    .then(|| Sym::Builtin(name.to_string(), "attribute"))
            }),
            TypeRef::Builtin(b) => attributes(b)
                .contains(&name)
                .then(|| Sym::Builtin(name.to_string(), "attribute")),
        }
    }

    fn class_first(
        &self,
        scope: ClassId,
        name: &str,
        global: bool,
        skip: Option<ClassId>,
    ) -> Option<Sym> {
        if global {
            return match self.tree.node(ROOT).children.get(name) {
                Some(&c) => Some(Sym::Class(c)),
                None if BUILTIN_FUNCTIONS.contains(&name) => {
                    Some(Sym::Builtin(name.to_string(), "function"))
                }
                None => None,
            };
        }
        if BUILTIN_TYPES.contains(&name) && self.tree.lookup(scope, &[name], skip).is_none() {
            return Some(Sym::Builtin(name.to_string(), "type"));
        }
        if let Some(c) = self.tree.lookup(scope, &[name], skip) {
            return Some(Sym::Class(c));
        }
        if skip.is_some()
            && let Some(p) = self.tree.node(scope).parent
        {
            return self.tree.lookup(p, &[name], skip).map(Sym::Class);
        }
        None
    }

    fn ref_first(
        &self,
        d: usize,
        an: &DocAn,
        ch: &Chain,
        scope: ClassId,
        name: &str,
    ) -> Option<Sym> {
        if ch.global {
            return self.class_first(scope, name, true, None);
        }
        let first = ch.toks[0];
        let mut best: Option<usize> = None;
        for &(fc, ft) in &an.for_idx {
            if an.tt(ft) == name && (fc == ch.cls || (fc != NONE && an.is_ancestor(fc, ch.cls))) {
                if best.is_none() || (ft <= first && best.is_some_and(|b| b > first || ft > b)) {
                    best = Some(ft);
                }
            }
        }
        if let Some(ft) = best {
            return Some(Sym::Local(d, an.toks[ft].s, an.toks[ft].e));
        }
        // scope by scope outwards: elements, then imports (MLS 5.3)
        let mut cur = Some(scope);
        while let Some(s) = cur {
            if let Some((o, _)) = self.tree.find_component(s, name) {
                return Some(Sym::Comp(o, name.to_string()));
            }
            if let Some(c) = self.tree.member_class(s, name) {
                return Some(Sym::Class(c));
            }
            if let Some(c) = self.tree.lookup_import(s, name) {
                return Some(Sym::Class(c));
            }
            if s != ROOT && self.tree.def(s).is_some_and(|d| d.encapsulated) {
                if let Some(&c) = self.tree.node(ROOT).children.get(name) {
                    return Some(Sym::Class(c));
                }
                break;
            }
            cur = self.tree.node(s).parent;
        }
        if let Some((pkg, n)) = self.tree.lookup_import_constant(scope, name) {
            let o = self
                .tree
                .find_component(pkg, &n)
                .map(|(o, _)| o)
                .unwrap_or(pkg);
            return Some(Sym::Comp(o, n));
        }
        if BUILTIN_TYPES.contains(&name) {
            return Some(Sym::Builtin(name.to_string(), "type"));
        }
        if BUILTIN_FUNCTIONS.contains(&name) {
            return Some(Sym::Builtin(name.to_string(), "function"));
        }
        if BUILTIN_VARS.contains(&name) {
            return Some(Sym::Builtin(name.to_string(), "variable"));
        }
        None
    }

    /// What each part of a chain denotes (memoised).
    fn resolve(&self, d: usize, ci: u32) -> Rc<Vec<Option<Sym>>> {
        if let Some(r) = self.memo.borrow().get(&(d, ci)) {
            return r.clone();
        }
        let an = self.an(d);
        let ch = &an.chains[ci as usize];
        let scope = self.scope(&an, ch.cls);
        let name = an.tt(ch.toks[0]);
        let first = match ch.ctx {
            Ctx::ClassName(lc) => self
                .tree
                .find_path(&an.classes[lc as usize].path)
                .map(Sym::Class),
            Ctx::Within | Ctx::Import => self.class_first(scope, name, true, None),
            Ctx::ImportSel(base) => {
                let b = self.resolve(d, base);
                b.last()
                    .cloned()
                    .flatten()
                    .and_then(|s| self.member(&s, name))
            }
            Ctx::Type => self.class_first(scope, name, ch.global, None),
            Ctx::Extends => self
                .class_first(scope, name, ch.global, Some(scope))
                .or_else(|| self.class_first(scope, name, ch.global, None)),
            Ctx::Decl => {
                let o = self
                    .tree
                    .find_component(scope, name)
                    .map(|(o, _)| o)
                    .unwrap_or(scope);
                Some(Sym::Comp(o, name.to_string()))
            }
            Ctx::Modifier(owner) => self
                .target_of(d, owner)
                .and_then(|t| self.modifier(&t, name)),
            Ctx::ForIndex => {
                let t = an.toks[ch.toks[0]];
                Some(Sym::Local(d, t.s, t.e))
            }
            Ctx::Ref => self.ref_first(d, &an, ch, scope, name),
            Ctx::Unknown => None,
        };
        let mut out = vec![first];
        for &t in &ch.toks[1..] {
            let next = out
                .last()
                .cloned()
                .flatten()
                .and_then(|s| self.member(&s, an.tt(t)));
            out.push(next);
        }
        let r = Rc::new(out);
        self.memo.borrow_mut().insert((d, ci), r.clone());
        r
    }

    /// The symbol under a byte offset, with the range of its name there.
    fn sym_at(&self, d: usize, off: usize) -> Option<(Sym, usize, usize)> {
        let an = self.an(d);
        let ti = an.token_at(off)?;
        let ci = an.tok_chain[ti];
        if ci == NONE {
            return None;
        }
        let k = an.chains[ci as usize].toks.iter().position(|&t| t == ti)?;
        let s = self.resolve(d, ci)[k].clone()?;
        Some((s, an.toks[ti].s, an.toks[ti].e))
    }

    fn location(&self, d: usize, s: usize, e: usize) -> Location {
        let an = self.an(d);
        let (line, col) = an.line_col(s);
        let ti = an.token_at(s);
        let class = ti
            .map(|t| an.tok_class[t])
            .filter(|&c| c != NONE)
            .map(|c| an.classes[c as usize].path.clone());
        Location {
            uri: self.lib.docs[d].uri.clone(),
            start: s,
            end: e,
            line,
            col,
            class,
            preview: an.line_text(s),
        }
    }

    fn loc_of_token(&self, d: usize, l: &rumoca_core::Location) -> Option<Location> {
        let (s, e) = (l.start as usize, l.end as usize);
        (e > s && e <= self.lib.docs[d].text.len()).then(|| self.location(d, s, e))
    }

    /// Where a symbol is declared.
    fn definition(&self, s: &Sym) -> Option<Location> {
        match s {
            Sym::Class(id) => {
                let node = self.tree.node(*id);
                let d = node.source;
                if d >= self.lib.docs.len() {
                    return None;
                }
                let an = self.an(d);
                if let Some(lc) = an.class_by_path(&node.path) {
                    let t = an.toks[an.classes[lc as usize].name_tok];
                    return Some(self.location(d, t.s, t.e));
                }
                self.loc_of_token(d, &node.def?.name.location)
            }
            Sym::Comp(o, name) => {
                let node = self.tree.node(*o);
                let d = node.source;
                if d >= self.lib.docs.len() {
                    return None;
                }
                let an = self.an(d);
                if let Some(lc) = an.class_by_path(&node.path) {
                    for ch in &an.chains {
                        if ch.ctx == Ctx::Decl && ch.cls == lc && an.tt(ch.toks[0]) == name {
                            let t = an.toks[ch.toks[0]];
                            return Some(self.location(d, t.s, t.e));
                        }
                    }
                }
                let c = node.def?.components.get(name.as_str())?;
                self.loc_of_token(d, &c.name_token.location)
            }
            Sym::Enum(c, lit) => {
                let node = self.tree.node(*c);
                let l = node
                    .def?
                    .enum_literals
                    .iter()
                    .find(|l| &*l.ident.text == lit)?;
                self.loc_of_token(node.source, &l.ident.location)
            }
            Sym::Local(d, s, e) => Some(self.location(*d, *s, *e)),
            Sym::Builtin(..) => None,
        }
    }

    fn sym_name(&self, s: &Sym) -> String {
        match s {
            Sym::Class(c) => self.tree.node(*c).name.clone(),
            Sym::Comp(_, n) | Sym::Enum(_, n) => n.clone(),
            Sym::Local(d, s, e) => self.lib.docs[*d].text[*s..*e].to_string(),
            Sym::Builtin(n, _) => n.rsplit('.').next().unwrap_or(n).to_string(),
        }
    }

    /// Every place that names `target`, across the loaded documents.
    fn references(&self, target: &Sym) -> Vec<Location> {
        let name = self.sym_name(target);
        let mut out = Vec::new();
        let docs: Vec<usize> = match target {
            Sym::Local(d, ..) => vec![*d],
            _ => (0..self.lib.docs.len()).collect(),
        };
        for d in docs {
            if !self.lib.docs[d].text.contains(name.as_str()) {
                continue;
            }
            let an = self.an(d);
            for (ci, ch) in an.chains.iter().enumerate() {
                for (k, &t) in ch.toks.iter().enumerate() {
                    if an.tt(t) != name {
                        continue;
                    }
                    if self.resolve(d, ci as u32)[k].as_ref() == Some(target) {
                        out.push(self.location(d, an.toks[t].s, an.toks[t].e));
                    }
                }
            }
        }
        out
    }

    // ---- completion helpers

    fn class_item(&self, c: ClassId) -> CompletionItem {
        let node = self.tree.node(c);
        let (kind, info) = match node.def {
            Some(d) => (d.class_type.as_str().to_string(), desc(&d.description)),
            None => ("package".to_string(), String::new()),
        };
        CompletionItem {
            label: node.name.clone(),
            detail: format!("{kind} {}", node.path),
            kind,
            info,
        }
    }

    fn comp_item(&self, c: &ast::Component) -> CompletionItem {
        let kind = match c.variability {
            rumoca_core::Variability::Parameter(_) => "parameter",
            rumoca_core::Variability::Constant(_) => "constant",
            _ => "variable",
        };
        CompletionItem {
            label: c.name.clone(),
            kind: kind.into(),
            detail: c.type_name.to_string(),
            info: desc(&c.description),
        }
    }

    /// Nested classes of `c`, own and inherited.
    fn member_classes(
        &self,
        c: ClassId,
        out: &mut Vec<CompletionItem>,
        seen: &mut FxHashSet<String>,
        depth: usize,
    ) {
        if depth > 16 {
            return;
        }
        for &k in &self.tree.node(c).order {
            if seen.insert(self.tree.node(k).name.clone()) {
                out.push(self.class_item(k));
            }
        }
        for b in self.tree.bases(c) {
            self.member_classes(b, out, seen, depth + 1);
        }
    }

    fn member_comps(
        &self,
        c: ClassId,
        out: &mut Vec<CompletionItem>,
        seen: &mut FxHashSet<String>,
        depth: usize,
    ) {
        if depth > 16 {
            return;
        }
        if let Some(d) = self.tree.def(c) {
            for comp in d.components.values() {
                if seen.insert(comp.name.clone()) {
                    out.push(self.comp_item(comp));
                }
            }
            for l in &d.enum_literals {
                if seen.insert(l.ident.text.to_string()) {
                    out.push(CompletionItem {
                        label: l.ident.text.to_string(),
                        kind: "enum".into(),
                        detail: format!("{}.{}", self.tree.node(c).path, l.ident.text),
                        info: desc(&l.description),
                    });
                }
            }
        }
        for b in self.tree.bases(c) {
            self.member_comps(b, out, seen, depth + 1);
        }
    }

    /// Classes registered for on-demand parsing directly below `path` ("" for the root).
    fn lazy_children(
        &self,
        path: &str,
        out: &mut Vec<CompletionItem>,
        seen: &mut FxHashSet<String>,
    ) {
        for f in &self.lib.lazy {
            let rest = if path.is_empty() {
                Some(f.class.as_str())
            } else {
                f.class.strip_prefix(path).and_then(|r| r.strip_prefix('.'))
            };
            let Some(rest) = rest else { continue };
            if rest.is_empty() {
                for o in &f.outline {
                    if !o.rel.is_empty() && !o.rel.contains('.') && seen.insert(o.rel.clone()) {
                        out.push(CompletionItem {
                            label: o.rel.clone(),
                            kind: o.kind.to_string(),
                            detail: format!("{} {path}.{}", o.kind, o.rel),
                            info: o.description.clone(),
                        });
                    }
                }
            } else if !rest.contains('.') && seen.insert(rest.to_string()) {
                let o = f.outline.iter().find(|o| o.rel.is_empty());
                out.push(CompletionItem {
                    label: rest.to_string(),
                    kind: o.map(|o| o.kind).unwrap_or("package").to_string(),
                    detail: f.class.clone(),
                    info: o.map(|o| o.description.clone()).unwrap_or_default(),
                });
            }
        }
    }

    fn members_of(&self, s: &Sym, out: &mut Vec<CompletionItem>) {
        let mut seen = FxHashSet::default();
        match s {
            Sym::Class(c) => {
                self.member_comps(*c, out, &mut seen, 0);
                self.member_classes(*c, out, &mut seen, 0);
                let path = self.tree.node(*c).path.clone();
                self.lazy_children(&path, out, &mut seen);
            }
            Sym::Comp(o, n) => {
                if let Some(TypeRef::Class(t)) = self.comp_type(*o, n) {
                    self.member_comps(t, out, &mut seen, 0);
                }
            }
            Sym::Builtin(b, "type") if b == "StateSelect" => {
                for l in ["never", "avoid", "default", "prefer", "always"] {
                    out.push(CompletionItem {
                        label: l.into(),
                        kind: "enum".into(),
                        detail: format!("StateSelect.{l}"),
                        info: String::new(),
                    });
                }
            }
            _ => {}
        }
    }

    fn scope_items(&self, an: &DocAn, cls: u32, scope: ClassId, out: &mut Vec<CompletionItem>) {
        let mut seen = FxHashSet::default();
        // for indices of the enclosing classes
        for &(fc, ft) in &an.for_idx {
            if (fc == cls || (fc != NONE && an.is_ancestor(fc, cls)))
                && seen.insert(an.tt(ft).to_string())
            {
                out.push(CompletionItem {
                    label: an.tt(ft).to_string(),
                    kind: "variable".into(),
                    detail: "for index".into(),
                    info: String::new(),
                });
            }
        }
        // components declared in the text but not parsed yet
        for ch in &an.chains {
            if ch.ctx == Ctx::Decl && ch.cls == cls {
                let n = an.tt(ch.toks[0]);
                if self.tree.find_component(scope, n).is_none() && seen.insert(n.to_string()) {
                    out.push(CompletionItem {
                        label: n.to_string(),
                        kind: "variable".into(),
                        detail: if ch.typ != NONE {
                            an.chains[ch.typ as usize]
                                .toks
                                .iter()
                                .map(|&t| an.tt(t))
                                .collect::<Vec<_>>()
                                .join(".")
                        } else {
                            String::new()
                        },
                        info: String::new(),
                    });
                }
            }
        }
        self.member_comps(scope, out, &mut seen, 0);
        let mut s = Some(scope);
        while let Some(c) = s {
            self.member_classes(c, out, &mut seen, 0);
            if let Some(d) = self.tree.def(c) {
                for imp in &d.imports {
                    match imp {
                        ast::Import::Qualified { path, .. } => {
                            if let Some(l) = path.name.last()
                                && seen.insert(l.text.to_string())
                            {
                                out.push(CompletionItem {
                                    label: l.text.to_string(),
                                    kind: "class".into(),
                                    detail: format!("import {path}"),
                                    info: String::new(),
                                });
                            }
                        }
                        ast::Import::Renamed { alias, path, .. } => {
                            if seen.insert(alias.text.to_string()) {
                                out.push(CompletionItem {
                                    label: alias.text.to_string(),
                                    kind: "class".into(),
                                    detail: format!("import {} = {path}", alias.text),
                                    info: String::new(),
                                });
                            }
                        }
                        ast::Import::Unqualified { path, .. } => {
                            let parts: Vec<&str> = path.name.iter().map(|t| &*t.text).collect();
                            if let Some(p) = self.tree.find_path(&parts.join(".")) {
                                self.member_comps(p, out, &mut seen, 0);
                                self.member_classes(p, out, &mut seen, 0);
                            }
                        }
                        ast::Import::Selective { path, names, .. } => {
                            for n in names {
                                if seen.insert(n.text.to_string()) {
                                    out.push(CompletionItem {
                                        label: n.text.to_string(),
                                        kind: "class".into(),
                                        detail: format!("import {path}.{}", n.text),
                                        info: String::new(),
                                    });
                                }
                            }
                        }
                    }
                }
            }
            s = self.tree.node(c).parent;
        }
        self.lazy_children("", out, &mut seen);
        for (list, kind) in [
            (BUILTIN_TYPES, "type"),
            (BUILTIN_FUNCTIONS, "function"),
            (&["time"][..], "variable"),
        ] {
            for &b in list {
                if seen.insert(b.to_string()) {
                    out.push(CompletionItem {
                        label: b.into(),
                        kind: kind.into(),
                        detail: format!("builtin {kind}"),
                        info: String::new(),
                    });
                }
            }
        }
        for &k in KEYWORDS {
            if seen.insert(k.to_string()) {
                out.push(CompletionItem {
                    label: k.into(),
                    kind: "keyword".into(),
                    detail: String::new(),
                    info: String::new(),
                });
            }
        }
    }

    fn unit_of_class(&self, c: ClassId, depth: usize) -> Option<String> {
        if depth > 16 {
            return None;
        }
        let def = self.tree.def(c)?;
        for ext in &def.extends {
            for m in &ext.modifications {
                if let ast::Expression::Modification { target, value, .. } = &m.expr
                    && target.parts.len() == 1
                    && &*target.parts[0].ident.text == "unit"
                {
                    return string_value(value);
                }
            }
        }
        self.tree
            .bases(c)
            .into_iter()
            .find_map(|b| self.unit_of_class(b, depth + 1))
    }

    fn hover(&self, s: &Sym) -> String {
        match s {
            Sym::Class(c) => {
                let node = self.tree.node(*c);
                let Some(def) = node.def else {
                    return format!("```modelica\npackage {}\n```", node.path);
                };
                let mut head = format!(
                    "{}{} {}",
                    if def.partial { "partial " } else { "" },
                    def.class_type.as_str(),
                    node.path
                );
                // a short class definition reads best as written
                if node.source < self.lib.docs.len() {
                    let an = self.an(node.source);
                    if let Some(lc) = an.class_by_path(&node.path) {
                        let l = &an.classes[lc as usize];
                        if l.short {
                            let s = an.toks[l.start].s;
                            let e = an.toks[l.end].e;
                            let src: String = an.text[s..e]
                                .split_whitespace()
                                .collect::<Vec<_>>()
                                .join(" ");
                            head = src.chars().take(400).collect();
                        }
                    }
                }
                let mut md = format!("```modelica\n{head}\n```\n");
                let d = desc(&def.description);
                if !d.is_empty() {
                    md += &format!("\n{d}\n");
                }
                let bases: Vec<String> = self
                    .tree
                    .bases(*c)
                    .iter()
                    .map(|b| format!("`{}`", self.tree.node(*b).path))
                    .collect();
                if !bases.is_empty() && !head.contains('=') {
                    md += &format!("\nextends {}\n", bases.join(", "));
                }
                if let Some(u) = self.unit_of_class(*c, 0)
                    && !u.is_empty()
                {
                    md += &format!("\nUnit: `{u}`\n");
                }
                md
            }
            Sym::Comp(o, n) => {
                let Some((decl, comp)) = self.tree.find_component(*o, n) else {
                    return format!("```modelica\n{n}\n```");
                };
                let node = self.tree.node(decl);
                let mut src = String::new();
                if node.source < self.lib.docs.len() {
                    let text = &self.lib.docs[node.source].text;
                    let (s, e) = (comp.location.start as usize, comp.location.end as usize);
                    if e > s && e <= text.len() {
                        src = text[s..e].split_whitespace().collect::<Vec<_>>().join(" ");
                        if let Some(p) = src.find(" annotation") {
                            src.truncate(p);
                        }
                        let mut c: String = src.chars().take(400).collect();
                        if c.len() < src.len() {
                            c.push('…');
                        }
                        src = c;
                    }
                }
                if src.is_empty() || !src.contains(n.as_str()) {
                    src = format!("{} {}", comp.type_name, n);
                }
                let mut md = format!("```modelica\n{src}\n```\n");
                let d = desc(&comp.description);
                if !d.is_empty() {
                    md += &format!("\n{d}\n");
                }
                md += &format!("\nIn `{}`", node.path);
                match self.comp_type(decl, n) {
                    Some(TypeRef::Class(t)) => {
                        let tn = self.tree.node(t);
                        let kind = tn.def.map(|d| d.class_type.as_str()).unwrap_or("class");
                        md += &format!(" · {kind} `{}`", tn.path);
                        if let Some(td) = tn.def {
                            let d = desc(&td.description);
                            if !d.is_empty() {
                                md += &format!(" — {d}");
                            }
                        }
                        md += "\n";
                        let unit = comp
                            .modifications
                            .get("unit")
                            .and_then(string_value)
                            .or_else(|| self.unit_of_class(t, 0));
                        if let Some(u) = unit
                            && !u.is_empty()
                        {
                            md += &format!("\nUnit: `{u}`\n");
                        }
                    }
                    Some(TypeRef::Builtin(b)) => {
                        md += &format!(" · `{b}`\n");
                        if let Some(u) = comp.modifications.get("unit").and_then(string_value) {
                            md += &format!("\nUnit: `{u}`\n");
                        }
                    }
                    None => md += "\n",
                }
                if comp.outer {
                    md += "\n`outer`: bound to the `inner` declaration of an enclosing instance\n";
                }
                md
            }
            Sym::Enum(c, l) => {
                let node = self.tree.node(*c);
                let d = node
                    .def
                    .and_then(|d| d.enum_literals.iter().find(|x| &*x.ident.text == l))
                    .map(|x| desc(&x.description))
                    .unwrap_or_default();
                format!(
                    "```modelica\n{}.{l}\n```\n\nenumeration literal\n\n{d}",
                    node.path
                )
            }
            Sym::Local(d, s, e) => {
                format!(
                    "```modelica\n{}\n```\n\nfor-loop index",
                    &self.lib.docs[*d].text[*s..*e]
                )
            }
            Sym::Builtin(n, k) => format!("```modelica\n{n}\n```\n\nbuiltin {k}"),
        }
    }
}

// ------------------------------------------------------------------------------ public API

/// Editor state kept between requests: the last version of each edited document that
/// parsed, standing in for it in the class tree while its current text does not.
#[derive(Default)]
pub struct Ide {
    last_good: FxHashMap<String, (std::sync::Arc<str>, ast::StoredDefinition)>,
}

impl Ide {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set a document's text, re-parsing it only if it changed. Returns whether it did.
    pub fn set_text(&mut self, lib: &mut Library, uri: &str, text: &str) -> bool {
        let same = lib.docs.iter().any(|d| d.uri == uri && &*d.text == text);
        if !same {
            lib.set_document(uri, text);
        }
        if let Some(d) = lib.docs.iter().find(|d| d.uri == uri)
            && d.errors.is_empty()
            && !self
                .last_good
                .get(uri)
                .is_some_and(|(t, _)| std::sync::Arc::ptr_eq(t, &d.text))
        {
            self.last_good
                .insert(uri.to_string(), (d.text.clone(), d.ast.clone()));
        }
        !same
    }

    /// Set a document's text (re-parsing it only if it changed) and return its
    /// diagnostics.
    pub fn update(&mut self, lib: &mut Library, uri: &str, text: &str) -> Vec<Diagnostic> {
        self.set_text(lib, uri, text);
        self.diagnostics(lib, uri)
    }

    /// Forget the stand-in of a removed document.
    pub fn forget(&mut self, uri: &str) {
        self.last_good.remove(uri);
    }

    /// Syntax errors only.
    pub fn syntax_diagnostics(&self, lib: &Library, uri: &str) -> Vec<Diagnostic> {
        let Some(doc) = lib.docs.iter().find(|d| d.uri == uri) else {
            return Vec::new();
        };
        let mut out: Vec<Diagnostic> = Vec::new();
        for e in &doc.error_spans {
            if out.iter().any(|d| d.start == e.start) {
                continue;
            }
            out.push(Diagnostic {
                start: e.start.min(doc.text.len()),
                end: e.end.min(doc.text.len()),
                severity: "error",
                message: e.message.clone(),
            });
        }
        out
    }

    /// Syntax errors, unknown classes (errors) and unknown names (warnings).
    pub fn diagnostics(&self, lib: &Library, uri: &str) -> Vec<Diagnostic> {
        let mut out = self.syntax_diagnostics(lib, uri);
        if !out.is_empty() {
            return out;
        }
        let ss = Session::new(lib, self);
        let Some(d) = ss.doc_index(uri) else {
            return out;
        };
        let an = ss.an(d);
        for (ci, ch) in an.chains.iter().enumerate() {
            let syms = ss.resolve(d, ci as u32);
            let Some(k) = syms.iter().position(|s| s.is_none()) else {
                continue;
            };
            let t = an.toks[ch.toks[k]];
            let name = an.tt(ch.toks[k]);
            let prev = if k > 0 { syms[k - 1].as_ref() } else { None };
            let (severity, message) = match ch.ctx {
                Ctx::Type | Ctx::Extends | Ctx::Within | Ctx::Import | Ctx::ImportSel(_) => {
                    match prev {
                        None if k == 0 => ("error", format!("unknown class `{name}`")),
                        Some(Sym::Class(c)) if !ss.replaceable(*c) => (
                            "error",
                            format!("`{}` has no member `{name}`", ss.tree.node(*c).path),
                        ),
                        _ => continue,
                    }
                }
                Ctx::Ref => match prev {
                    None if k == 0 => ("warning", format!("unknown name `{name}`")),
                    Some(Sym::Class(c)) if !ss.replaceable(*c) => (
                        "warning",
                        format!("`{}` has no member `{name}`", ss.tree.node(*c).path),
                    ),
                    _ => continue,
                },
                Ctx::Modifier(owner) if k == 0 => match ss.target_of(d, owner) {
                    Some(TypeRef::Class(c))
                        if !ss.replaceable(c)
                            && (an.chains[owner as usize].ctx != Ctx::Ref
                                || matches!(
                                    ss.tree.class_type(c),
                                    rumoca_core::ClassType::Function
                                        | rumoca_core::ClassType::Record
                                )) =>
                    {
                        (
                            "warning",
                            format!("`{}` has no element `{name}`", ss.tree.node(c).path),
                        )
                    }
                    Some(TypeRef::Builtin(b)) if an.chains[owner as usize].ctx != Ctx::Ref => {
                        ("warning", format!("`{b}` has no attribute `{name}`"))
                    }
                    _ => continue,
                },
                _ => continue,
            };
            out.push(Diagnostic {
                start: t.s,
                end: t.e,
                severity,
                message,
            });
        }
        out
    }

    /// Where the name at `offset` is declared. For an `outer` component, the `inner`
    /// declarations of the same name and type follow the declaration itself.
    pub fn definition(&self, lib: &Library, uri: &str, offset: usize) -> Vec<Location> {
        let ss = Session::new(lib, self);
        let Some(d) = ss.doc_index(uri) else {
            return Vec::new();
        };
        let Some((s, ..)) = ss.sym_at(d, offset) else {
            return Vec::new();
        };
        let mut out: Vec<Location> = ss.definition(&s).into_iter().collect();
        if let Sym::Comp(o, n) = &s
            && let Some((_, c)) = ss.tree.find_component(*o, n)
            && c.outer
        {
            let ty = c.type_name.to_string();
            for (id, node) in ss.tree.nodes.iter().enumerate() {
                if let Some(def) = node.def
                    && let Some(ic) = def.components.get(n.as_str())
                    && ic.inner
                    && ic.type_name.to_string().rsplit('.').next() == ty.rsplit('.').next()
                    && let Some(l) = ss.definition(&Sym::Comp(id as ClassId, n.clone()))
                {
                    out.push(l);
                    if out.len() > 20 {
                        break;
                    }
                }
            }
        }
        out
    }

    /// Every place in the loaded documents that names what `offset` names, the
    /// declaration included.
    pub fn references(&self, lib: &Library, uri: &str, offset: usize) -> Vec<Location> {
        let ss = Session::new(lib, self);
        let Some(d) = ss.doc_index(uri) else {
            return Vec::new();
        };
        let Some((s, ..)) = ss.sym_at(d, offset) else {
            return Vec::new();
        };
        if matches!(s, Sym::Builtin(..)) {
            return Vec::new();
        }
        let mut out = ss.references(&s);
        if let Some(def) = ss.definition(&s)
            && !out.contains(&def)
        {
            out.insert(0, def);
        }
        out
    }

    /// Text edits renaming the class or component at `offset` everywhere it is named.
    /// Refused when any of them would fall in a document `read_only` reports.
    pub fn rename(
        &self,
        lib: &Library,
        uri: &str,
        offset: usize,
        new_name: &str,
        read_only: &dyn Fn(&str) -> bool,
    ) -> Result<Vec<TextEdit>, String> {
        let valid = (new_name.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_')
            && new_name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_'))
            || (new_name.len() > 2 && new_name.starts_with('\'') && new_name.ends_with('\''));
        if !valid || is_keyword(new_name) {
            return Err(format!("`{new_name}` is not a valid Modelica identifier"));
        }
        let ss = Session::new(lib, self);
        let d = ss.doc_index(uri).ok_or("document not loaded")?;
        let (s, ..) = ss.sym_at(d, offset).ok_or("nothing to rename here")?;
        match &s {
            Sym::Class(c) => {
                let node = ss.tree.node(*c);
                if let Some(p) = node.parent
                    && ss.tree.node(p).children.contains_key(new_name)
                {
                    return Err(format!(
                        "`{}` already has a class `{new_name}`",
                        ss.tree.node(p).path
                    ));
                }
                // a class stored in its own file (`P/M.mo`, `P/M/package.mo`) names the file
                if node.source < lib.docs.len() {
                    let u = lib.docs[node.source].uri.replace('\\', "/");
                    let mut segs = u.rsplit('/');
                    let file = segs.next().unwrap_or("");
                    let dir = segs.next().unwrap_or("");
                    let top = ss
                        .an(node.source)
                        .class_by_path(&node.path)
                        .is_some_and(|lc| ss.an(node.source).classes[lc as usize].parent == NONE);
                    if top
                        && (file == format!("{}.mo", node.name)
                            || (file == "package.mo" && dir == node.name))
                    {
                        return Err(format!(
                            "`{}` is stored in its own file ({u}); renaming it would rename the file",
                            node.path
                        ));
                    }
                }
            }
            Sym::Comp(o, _) => {
                if ss.tree.has_component(*o, new_name) {
                    return Err(format!(
                        "`{}` already has an element `{new_name}`",
                        ss.tree.node(*o).path
                    ));
                }
            }
            Sym::Local(..) => {}
            Sym::Enum(..) | Sym::Builtin(..) => {
                return Err("only classes, components and loop indices can be renamed".into());
            }
        }
        let mut locs = ss.references(&s);
        if let Some(def) = ss.definition(&s)
            && !locs.contains(&def)
        {
            locs.push(def);
        }
        if let Some(l) = locs.iter().find(|l| read_only(&l.uri)) {
            return Err(format!(
                "`{}` is named in the read-only library file {} (line {})",
                ss.sym_name(&s),
                l.uri,
                l.line + 1
            ));
        }
        Ok(locs
            .into_iter()
            .map(|l| TextEdit {
                uri: l.uri,
                start: l.start,
                end: l.end,
                new_text: new_name.to_string(),
            })
            .collect())
    }

    /// Completion at `offset`: members after `a.b.`, modifier names inside `x(...)`,
    /// otherwise the components, classes, builtins and keywords in scope.
    pub fn completion(&self, lib: &Library, uri: &str, offset: usize) -> Completion {
        let ss = Session::new(lib, self);
        let Some(d) = ss.doc_index(uri) else {
            return Completion::default();
        };
        let an = ss.an(d);
        let offset = offset.min(an.text.len());
        if an.in_skip(offset) {
            return Completion {
                from: offset,
                items: Vec::new(),
            };
        }
        // the identifier typed so far, and the token before it
        let ti = an.toks.partition_point(|t| t.s < offset);
        let (from, before) = match ti.checked_sub(1) {
            Some(p) if an.toks[p].k == K::Ident && an.toks[p].e >= offset => {
                (an.toks[p].s, p.checked_sub(1))
            }
            Some(p) if an.toks[p].k == K::Kw && an.toks[p].e >= offset => {
                (an.toks[p].s, p.checked_sub(1))
            }
            p => (offset, p),
        };
        let prefix = an.text[from..offset].to_lowercase();
        let mut items = Vec::new();
        let finish = |mut items: Vec<CompletionItem>| {
            if !prefix.is_empty() {
                items.retain(|i| i.label.to_lowercase().starts_with(&prefix));
            }
            items.truncate(400);
            Completion { from, items }
        };
        // `a.b.|`
        if let Some(b) = before
            && an.is_op(b, ".")
            && b > 0
        {
            let mut q = b - 1;
            if an.is_op(q, "]") {
                let mut depth = 0;
                loop {
                    if an.is_op(q, "]") {
                        depth += 1;
                    } else if an.is_op(q, "[") {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    if q == 0 {
                        break;
                    }
                    q -= 1;
                }
                q = q.saturating_sub(1);
            }
            let ci = an.tok_chain[q];
            if ci != NONE
                && let Some(k) = an.chains[ci as usize].toks.iter().position(|&t| t == q)
                && let Some(s) = ss.resolve(d, ci)[k].clone()
            {
                ss.members_of(&s, &mut items);
            }
            return finish(items);
        }
        // inside `x(...)`: the elements of what x denotes
        let mut depth = 0i32;
        let mut open = None;
        let mut q = before;
        while let Some(i) = q {
            if an.is_op(i, ")") || an.is_op(i, "]") || an.is_op(i, "}") {
                depth += 1;
            } else if an.is_op(i, "(") || an.is_op(i, "[") || an.is_op(i, "{") {
                if depth == 0 {
                    if i > 0 && an.is_kw(i - 1, "annotation") {
                        return finish(items);
                    }
                    if open.is_none() {
                        open = Some(i);
                    }
                } else {
                    depth -= 1;
                }
            } else if depth == 0 && open.is_none() && an.is_op(i, ";") {
                break;
            }
            q = i.checked_sub(1);
        }
        let at_arg = before.is_some_and(|b| {
            an.is_op(b, "(")
                || an.is_op(b, ",")
                || an.is_kw(b, "each")
                || an.is_kw(b, "final")
                || an.is_kw(b, "redeclare")
        });
        if let Some(o) = open
            && an.is_op(o, "(")
            && at_arg
            && let Some(&owner) = an.ends_at.get(&o)
        {
            let call = an.chains[owner as usize].ctx == Ctx::Ref;
            if let Some(t) = ss.target_of(d, owner) {
                let mut seen = FxHashSet::default();
                match &t {
                    TypeRef::Class(c) => {
                        let mut comps = Vec::new();
                        ss.member_comps(*c, &mut comps, &mut seen, 0);
                        for mut it in comps {
                            if call {
                                it.kind = "property".into();
                            } else if it.kind != "enum" {
                                it.kind = "property".into();
                            }
                            items.push(it);
                        }
                        if let Some(b) = ss.tree.builtin_base(*c) {
                            for a in attributes(b) {
                                if seen.insert(a.to_string()) {
                                    items.push(CompletionItem {
                                        label: a.to_string(),
                                        kind: "property".into(),
                                        detail: format!("{b} attribute"),
                                        info: String::new(),
                                    });
                                }
                            }
                        }
                    }
                    TypeRef::Builtin(b) => {
                        for a in attributes(b) {
                            items.push(CompletionItem {
                                label: a.to_string(),
                                kind: "property".into(),
                                detail: format!("{b} attribute"),
                                info: String::new(),
                            });
                        }
                    }
                }
                if !call {
                    for k in ["redeclare", "each", "final"] {
                        items.push(CompletionItem {
                            label: k.into(),
                            kind: "keyword".into(),
                            detail: String::new(),
                            info: String::new(),
                        });
                    }
                    return finish(items);
                }
            }
        }
        let cls = before.map(|b| an.tok_class[b]).unwrap_or(NONE);
        let scope = ss.scope(&an, cls);
        ss.scope_items(&an, cls, scope, &mut items);
        finish(items)
    }

    /// Declaration, description, type and unit of the name at `offset`.
    pub fn hover(&self, lib: &Library, uri: &str, offset: usize) -> Option<Hover> {
        let ss = Session::new(lib, self);
        let d = ss.doc_index(uri)?;
        let (s, start, end) = ss.sym_at(d, offset)?;
        Some(Hover {
            start,
            end,
            markdown: ss.hover(&s),
        })
    }

    /// Outline: the classes of a document with their components.
    pub fn symbols(&self, lib: &Library, uri: &str) -> Vec<DocSymbol> {
        let Some(doc) = lib.docs.iter().find(|d| d.uri == uri) else {
            return Vec::new();
        };
        let an = analyze(&doc.text);
        let mut nodes: Vec<Option<DocSymbol>> = an
            .classes
            .iter()
            .map(|c| {
                let nt = an.toks[c.name_tok];
                Some(DocSymbol {
                    name: c.name.clone(),
                    kind: c.kind.clone(),
                    detail: String::new(),
                    start: an.toks[c.start].s,
                    end: an.toks[c.end].e,
                    sel_start: nt.s,
                    sel_end: nt.e,
                    path: c.path.clone(),
                    children: Vec::new(),
                })
            })
            .collect();
        let mut comps: Vec<(u32, DocSymbol)> = Vec::new();
        for ch in &an.chains {
            if ch.ctx != Ctx::Decl || ch.cls == NONE {
                continue;
            }
            let t = an.toks[ch.toks[0]];
            let (mut start, mut ty) = (t.s, String::new());
            let mut kind = "component";
            if ch.typ != NONE {
                let tc = &an.chains[ch.typ as usize];
                ty = tc
                    .toks
                    .iter()
                    .map(|&x| an.tt(x))
                    .collect::<Vec<_>>()
                    .join(".");
                let mut k = tc.toks[0];
                start = an.toks[k].s;
                while k > 0 && an.toks[k - 1].k == K::Kw {
                    k -= 1;
                    match an.tt(k) {
                        "parameter" => kind = "parameter",
                        "constant" => kind = "constant",
                        _ => {}
                    }
                }
            }
            comps.push((
                ch.cls,
                DocSymbol {
                    name: an.tt(ch.toks[0]).to_string(),
                    kind: kind.into(),
                    detail: ty,
                    start,
                    end: t.e,
                    sel_start: t.s,
                    sel_end: t.e,
                    path: format!("{}.{}", an.classes[ch.cls as usize].path, an.tt(ch.toks[0])),
                    children: Vec::new(),
                },
            ));
        }
        for (c, s) in comps {
            if let Some(n) = nodes[c as usize].as_mut() {
                n.children.push(s);
            }
        }
        // attach classes to their parents, innermost first
        let mut roots = Vec::new();
        for i in (0..an.classes.len()).rev() {
            let s = nodes[i].take().unwrap();
            let p = an.classes[i].parent;
            if p == NONE {
                roots.push(s);
            } else if let Some(pn) = nodes[p as usize].as_mut() {
                pn.children.push(s);
            }
        }
        roots.reverse();
        fn sort(v: &mut Vec<DocSymbol>) {
            v.sort_by_key(|s| s.start);
            for s in v {
                sort(&mut s.children);
            }
        }
        sort(&mut roots);
        roots
    }

    /// Paths of the classes a document defines (to load what they need first).
    pub fn document_classes(text: &str) -> Vec<(String, String)> {
        analyze(text)
            .classes
            .into_iter()
            .filter(|c| !c.short)
            .map(|c| (c.path, c.kind))
            .collect()
    }

    /// The dotted name being completed at `offset` (`Modelica.Blocks.` gives
    /// `Modelica.Blocks`), so that a caller can load that class first.
    pub fn dotted_prefix(text: &str, offset: usize) -> Option<String> {
        let b = text.as_bytes();
        let mut i = offset.min(b.len());
        while i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_') {
            i -= 1;
        }
        if i == 0 || b[i - 1] != b'.' {
            return None;
        }
        let end = i - 1;
        let mut s = end;
        while s > 0 && (b[s - 1].is_ascii_alphanumeric() || b[s - 1] == b'_' || b[s - 1] == b'.') {
            s -= 1;
        }
        let p = text[s..end].trim_matches('.');
        (!p.is_empty()).then(|| p.to_string())
    }
}

/// Byte offset of a UTF-16 offset (JavaScript string index) in `text`.
pub fn utf16_to_byte(text: &str, u: usize) -> usize {
    if text.is_ascii() {
        return u.min(text.len());
    }
    let mut n = 0;
    for (b, c) in text.char_indices() {
        if n >= u {
            return b;
        }
        n += c.len_utf16();
    }
    text.len()
}

/// UTF-16 offset (JavaScript string index) of a byte offset in `text`.
pub fn byte_to_utf16(text: &str, b: usize) -> usize {
    let b = b.min(text.len());
    if text.is_ascii() {
        return b;
    }
    text[..b].chars().map(char::len_utf16).sum()
}

#[cfg(test)]
mod tests {
    #[test]
    fn keywords_sorted() {
        let mut k = super::KEYWORDS.to_vec();
        k.sort();
        assert_eq!(k, super::KEYWORDS);
    }
}
