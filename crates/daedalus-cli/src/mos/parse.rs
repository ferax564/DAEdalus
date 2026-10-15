//! Lexer and parser for the `.mos` scripting subset: statements (expressions,
//! assignments, declarations, `for`/`while`/`if`), function calls with positional and
//! named arguments, numbers, strings, booleans, arrays `{..}` / `[..]`, ranges and the
//! usual operators.

use anyhow::{Result, bail};

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64, bool),
    Str(String),
    Ident(String),
    Op(&'static str),
    Eof,
}

const OPS: &[&str] = &[
    ":=", "==", "<>", "<=", ">=", ".+", ".-", ".*", "./", ".^", "(", ")", "{", "}", "[", "]", ",",
    ";", "=", ":", "<", ">", "+", "-", "*", "/", "^", ".",
];

fn lex(src: &str) -> Result<Vec<(Tok, usize)>> {
    let b = src.as_bytes();
    let mut i = 0;
    let mut line = 1;
    let mut out = Vec::new();
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            line += 1;
            i += 1;
        } else if c.is_ascii_whitespace() {
            i += 1;
        } else if src[i..].starts_with("//") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else if src[i..].starts_with("/*") {
            let end = src[i + 2..]
                .find("*/")
                .ok_or_else(|| anyhow::anyhow!("line {line}: unterminated comment"))?;
            line += src[i..i + 2 + end].matches('\n').count();
            i += end + 4;
        } else if c == b'"' {
            let start = line;
            let mut s = String::new();
            i += 1;
            loop {
                let Some(&ch) = b.get(i) else {
                    bail!("line {start}: unterminated string");
                };
                match ch {
                    b'"' => {
                        i += 1;
                        break;
                    }
                    b'\\' => {
                        let e = b.get(i + 1).copied().unwrap_or(b'\\');
                        match e {
                            b'n' => s.push('\n'),
                            b't' => s.push('\t'),
                            b'"' | b'\\' | b'\'' => s.push(e as char),
                            other => {
                                s.push('\\');
                                s.push(other as char);
                            }
                        }
                        i += 2;
                    }
                    _ => {
                        if ch == b'\n' {
                            line += 1;
                        }
                        // whole UTF-8 character
                        let len = src[i..].chars().next().map_or(1, |c| c.len_utf8());
                        s.push_str(&src[i..i + len]);
                        i += len;
                    }
                }
            }
            out.push((Tok::Str(s), start));
        } else if c.is_ascii_digit() || (c == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit))
        {
            let start = i;
            let mut int = true;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            if i < b.len() && b[i] == b'.' && !b.get(i + 1).is_some_and(|x| b"+-*/^".contains(x)) {
                int = false;
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
                    int = false;
                    i = j;
                    while i < b.len() && b[i].is_ascii_digit() {
                        i += 1;
                    }
                }
            }
            let text = &src[start..i];
            let v: f64 = text
                .parse()
                .map_err(|_| anyhow::anyhow!("line {line}: bad number '{text}'"))?;
            out.push((Tok::Num(v, int), line));
        } else if c.is_ascii_alphabetic() || c == b'_' {
            let start = i;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_') {
                i += 1;
            }
            out.push((Tok::Ident(src[start..i].to_string()), line));
        } else if c == b'\'' {
            // quoted identifier
            let end = src[i + 1..]
                .find('\'')
                .ok_or_else(|| anyhow::anyhow!("line {line}: unterminated quoted name"))?;
            out.push((Tok::Ident(src[i..i + end + 2].to_string()), line));
            i += end + 2;
        } else if let Some(op) = OPS.iter().find(|o| src[i..].starts_with(**o)) {
            out.push((Tok::Op(op), line));
            i += op.len();
        } else {
            let ch = src[i..].chars().next().unwrap_or('?');
            bail!("line {line}: unexpected character '{ch}'");
        }
    }
    out.push((Tok::Eof, line));
    Ok(out)
}

#[derive(Clone, Debug)]
pub enum Expr {
    Num(f64),
    Int(i64),
    Str(String),
    Bool(bool),
    /// Dotted name.
    Name(String),
    Index(Box<Expr>, Vec<Expr>),
    Call(String, Vec<Arg>),
    Array(Vec<Expr>),
    /// `[a, b; c, d]`
    Matrix(Vec<Vec<Expr>>),
    Tuple(Vec<Expr>),
    Range(Box<Expr>, Option<Box<Expr>>, Box<Expr>),
    Bin(&'static str, Box<Expr>, Box<Expr>),
    Not(Box<Expr>),
    Neg(Box<Expr>),
    If(Vec<(Expr, Expr)>, Box<Expr>),
}

#[derive(Clone, Debug)]
pub struct Arg {
    pub name: Option<String>,
    pub value: Expr,
}

#[derive(Clone, Debug)]
pub enum Stmt {
    /// An expression statement (its value is printed).
    Expr(Expr),
    Assign(Expr, Expr),
    /// `(a, b) := f(...)`
    TupleAssign(Vec<Option<String>>, Expr),
    /// `Real x[2] = ...;` — the type is not checked.
    Decl(String, Option<Expr>),
    For(String, Expr, Vec<Stmt>),
    While(Expr, Vec<Stmt>),
    If(Vec<(Expr, Vec<Stmt>)>, Vec<Stmt>),
}

#[derive(Debug)]
pub struct Line {
    pub stmt: Stmt,
    pub line: usize,
}

struct P {
    toks: Vec<(Tok, usize)>,
    pos: usize,
}

pub fn parse(src: &str) -> Result<Vec<Line>> {
    let mut p = P {
        toks: lex(src)?,
        pos: 0,
    };
    let body = p.statements(&[])?;
    if p.peek() != &Tok::Eof {
        bail!("line {}: unexpected {}", p.line(), p.describe());
    }
    Ok(body)
}

impl P {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].0
    }
    fn peek_at(&self, k: usize) -> &Tok {
        &self.toks[(self.pos + k).min(self.toks.len() - 1)].0
    }
    fn line(&self) -> usize {
        self.toks[self.pos].1
    }
    fn describe(&self) -> String {
        match self.peek() {
            Tok::Num(v, _) => format!("number {v}"),
            Tok::Str(s) => format!("string \"{s}\""),
            Tok::Ident(s) => format!("'{s}'"),
            Tok::Op(o) => format!("'{o}'"),
            Tok::Eof => "end of script".into(),
        }
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].0.clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn is_op(&self, o: &str) -> bool {
        matches!(self.peek(), Tok::Op(x) if *x == o)
    }
    fn is_kw(&self, k: &str) -> bool {
        matches!(self.peek(), Tok::Ident(x) if x == k)
    }
    fn eat_op(&mut self, o: &str) -> bool {
        if self.is_op(o) {
            self.bump();
            true
        } else {
            false
        }
    }
    fn expect_op(&mut self, o: &str) -> Result<()> {
        if !self.eat_op(o) {
            bail!(
                "line {}: expected '{o}', found {}",
                self.line(),
                self.describe()
            );
        }
        Ok(())
    }
    fn expect_kw(&mut self, k: &str) -> Result<()> {
        if !self.is_kw(k) {
            bail!(
                "line {}: expected '{k}', found {}",
                self.line(),
                self.describe()
            );
        }
        self.bump();
        Ok(())
    }
    fn ident(&mut self) -> Result<String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            _ => {
                self.pos -= 1;
                bail!(
                    "line {}: expected a name, found {}",
                    self.line(),
                    self.describe()
                )
            }
        }
    }

    /// Statements up to one of the keywords in `until` (or the end).
    fn statements(&mut self, until: &[&str]) -> Result<Vec<Line>> {
        let mut out = Vec::new();
        loop {
            while self.eat_op(";") {}
            if self.peek() == &Tok::Eof || until.iter().any(|k| self.is_kw(k)) {
                return Ok(out);
            }
            let line = self.line();
            let stmt = self.statement()?;
            out.push(Line { stmt, line });
        }
    }

    fn end(&mut self, k: &str) -> Result<()> {
        self.expect_kw("end")?;
        self.expect_kw(k)?;
        self.eat_op(";");
        Ok(())
    }

    fn statement(&mut self) -> Result<Stmt> {
        if self.is_kw("for") {
            self.bump();
            let var = self.ident()?;
            self.expect_kw("in")?;
            let range = self.expr()?;
            self.expect_kw("loop")?;
            let body = self.statements(&["end"])?;
            self.end("for")?;
            return Ok(Stmt::For(
                var,
                range,
                body.into_iter().map(|l| l.stmt).collect(),
            ));
        }
        if self.is_kw("while") {
            self.bump();
            let c = self.expr()?;
            self.expect_kw("loop")?;
            let body = self.statements(&["end"])?;
            self.end("while")?;
            return Ok(Stmt::While(c, body.into_iter().map(|l| l.stmt).collect()));
        }
        if self.is_kw("if") {
            self.bump();
            let mut branches = Vec::new();
            let mut c = self.expr()?;
            self.expect_kw("then")?;
            loop {
                let body = self.statements(&["elseif", "else", "end"])?;
                branches.push((c, body.into_iter().map(|l| l.stmt).collect()));
                if self.is_kw("elseif") {
                    self.bump();
                    c = self.expr()?;
                    self.expect_kw("then")?;
                } else {
                    break;
                }
            }
            let mut els = Vec::new();
            if self.is_kw("else") {
                self.bump();
                els = self
                    .statements(&["end"])?
                    .into_iter()
                    .map(|l| l.stmt)
                    .collect();
            }
            self.end("if")?;
            return Ok(Stmt::If(branches, els));
        }
        // Declarations: `Real x;`, `Boolean ok = true;`, `String s[2];`
        if let Tok::Ident(t) = self.peek()
            && matches!(t.as_str(), "Real" | "Integer" | "Boolean" | "String")
            && matches!(self.peek_at(1), Tok::Ident(_))
        {
            self.bump();
            let name = self.ident()?;
            if self.eat_op("[") {
                while !self.eat_op("]") {
                    if self.bump() == Tok::Eof {
                        bail!("line {}: unterminated dimensions", self.line());
                    }
                }
            }
            let init = if self.eat_op("=") || self.eat_op(":=") {
                Some(self.expr()?)
            } else {
                None
            };
            self.terminate()?;
            return Ok(Stmt::Decl(name, init));
        }
        let e = self.expr()?;
        if self.eat_op(":=") || self.eat_op("=") {
            let rhs = self.expr()?;
            self.terminate()?;
            return match e {
                Expr::Name(_) | Expr::Index(..) => Ok(Stmt::Assign(e, rhs)),
                Expr::Tuple(items) => {
                    let mut names = Vec::new();
                    for it in items {
                        match it {
                            Expr::Name(n) => names.push(Some(n)),
                            _ => bail!("tuple assignment to something that is not a name"),
                        }
                    }
                    Ok(Stmt::TupleAssign(names, rhs))
                }
                _ => bail!("line {}: cannot assign to this expression", self.line()),
            };
        }
        self.terminate()?;
        Ok(Stmt::Expr(e))
    }

    /// The `;` ending a statement: optional at a line end, required between statements
    /// on one line.
    fn terminate(&mut self) -> Result<()> {
        if self.eat_op(";")
            || matches!(self.peek(), Tok::Eof)
            || ["end", "else", "elseif"].iter().any(|k| self.is_kw(k))
        {
            return Ok(());
        }
        let here = self.line();
        let prev = self.toks[self.pos.saturating_sub(1)].1;
        if here == prev {
            bail!("line {here}: expected ';', found {}", self.describe());
        }
        Ok(())
    }

    fn expr(&mut self) -> Result<Expr> {
        if self.is_kw("if") {
            self.bump();
            let mut branches = Vec::new();
            let c = self.expr()?;
            self.expect_kw("then")?;
            let v = self.expr()?;
            branches.push((c, v));
            while self.is_kw("elseif") {
                self.bump();
                let c = self.expr()?;
                self.expect_kw("then")?;
                let v = self.expr()?;
                branches.push((c, v));
            }
            self.expect_kw("else")?;
            let e = self.expr()?;
            return Ok(Expr::If(branches, Box::new(e)));
        }
        let a = self.or()?;
        if self.eat_op(":") {
            let b = self.or()?;
            if self.eat_op(":") {
                let c = self.or()?;
                return Ok(Expr::Range(Box::new(a), Some(Box::new(b)), Box::new(c)));
            }
            return Ok(Expr::Range(Box::new(a), None, Box::new(b)));
        }
        Ok(a)
    }

    fn or(&mut self) -> Result<Expr> {
        let mut a = self.and()?;
        while self.is_kw("or") {
            self.bump();
            let b = self.and()?;
            a = Expr::Bin("or", Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn and(&mut self) -> Result<Expr> {
        let mut a = self.not()?;
        while self.is_kw("and") {
            self.bump();
            let b = self.not()?;
            a = Expr::Bin("and", Box::new(a), Box::new(b));
        }
        Ok(a)
    }

    fn not(&mut self) -> Result<Expr> {
        if self.is_kw("not") {
            self.bump();
            return Ok(Expr::Not(Box::new(self.not()?)));
        }
        self.rel()
    }

    fn rel(&mut self) -> Result<Expr> {
        let a = self.arith()?;
        for op in ["==", "<>", "<=", ">=", "<", ">"] {
            if self.is_op(op) {
                self.bump();
                let b = self.arith()?;
                return Ok(Expr::Bin(op, Box::new(a), Box::new(b)));
            }
        }
        Ok(a)
    }

    fn arith(&mut self) -> Result<Expr> {
        let mut a = if self.eat_op("-") {
            Expr::Neg(Box::new(self.term()?))
        } else {
            self.eat_op("+");
            self.term()?
        };
        loop {
            let op = if self.eat_op("+") || self.eat_op(".+") {
                "+"
            } else if self.eat_op("-") || self.eat_op(".-") {
                "-"
            } else {
                return Ok(a);
            };
            let b = self.term()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn term(&mut self) -> Result<Expr> {
        let mut a = self.power()?;
        loop {
            let op = if self.eat_op("*") || self.eat_op(".*") {
                "*"
            } else if self.eat_op("/") || self.eat_op("./") {
                "/"
            } else {
                return Ok(a);
            };
            let b = self.power()?;
            a = Expr::Bin(op, Box::new(a), Box::new(b));
        }
    }

    fn power(&mut self) -> Result<Expr> {
        let a = self.postfix()?;
        if self.eat_op("^") || self.eat_op(".^") {
            let b = if self.eat_op("-") {
                Expr::Neg(Box::new(self.postfix()?))
            } else {
                self.postfix()?
            };
            return Ok(Expr::Bin("^", Box::new(a), Box::new(b)));
        }
        Ok(a)
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        while self.is_op("[") {
            self.bump();
            let mut idx = vec![self.expr()?];
            while self.eat_op(",") {
                idx.push(self.expr()?);
            }
            self.expect_op("]")?;
            e = Expr::Index(Box::new(e), idx);
        }
        Ok(e)
    }

    fn args(&mut self) -> Result<Vec<Arg>> {
        let mut args = Vec::new();
        if self.eat_op(")") {
            return Ok(args);
        }
        loop {
            let named =
                matches!(self.peek(), Tok::Ident(_)) && matches!(self.peek_at(1), Tok::Op("="));
            let name = if named {
                let n = self.ident()?;
                self.bump();
                Some(n)
            } else {
                None
            };
            let value = self.expr()?;
            args.push(Arg { name, value });
            if self.eat_op(")") {
                return Ok(args);
            }
            self.expect_op(",")?;
        }
    }

    fn primary(&mut self) -> Result<Expr> {
        let line = self.line();
        match self.bump() {
            Tok::Num(v, true) if v.abs() < 9e15 => Ok(Expr::Int(v as i64)),
            Tok::Num(v, _) => Ok(Expr::Num(v)),
            Tok::Str(s) => Ok(Expr::Str(s)),
            Tok::Ident(s) if s == "true" => Ok(Expr::Bool(true)),
            Tok::Ident(s) if s == "false" => Ok(Expr::Bool(false)),
            Tok::Ident(mut s) => {
                while self.is_op(".") && matches!(self.peek_at(1), Tok::Ident(_)) {
                    self.bump();
                    s.push('.');
                    s.push_str(&self.ident()?);
                }
                if self.eat_op("(") {
                    let args = self.args()?;
                    return Ok(Expr::Call(s, args));
                }
                Ok(Expr::Name(s))
            }
            Tok::Op("(") => {
                let mut items = vec![self.expr()?];
                while self.eat_op(",") {
                    items.push(self.expr()?);
                }
                self.expect_op(")")?;
                Ok(if items.len() == 1 {
                    items.pop().unwrap()
                } else {
                    Expr::Tuple(items)
                })
            }
            Tok::Op("{") => {
                let mut items = Vec::new();
                if !self.eat_op("}") {
                    loop {
                        items.push(self.expr()?);
                        if self.eat_op("}") {
                            break;
                        }
                        self.expect_op(",")?;
                    }
                }
                Ok(Expr::Array(items))
            }
            Tok::Op("[") => {
                let mut rows = vec![Vec::new()];
                if !self.eat_op("]") {
                    loop {
                        rows.last_mut().unwrap().push(self.expr()?);
                        if self.eat_op("]") {
                            break;
                        }
                        if self.eat_op(";") {
                            rows.push(Vec::new());
                        } else {
                            self.expect_op(",")?;
                        }
                    }
                }
                Ok(if rows.len() == 1 {
                    Expr::Array(rows.pop().unwrap())
                } else {
                    Expr::Matrix(rows)
                })
            }
            _ => {
                self.pos -= 1;
                bail!("line {line}: unexpected {}", self.describe())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn statements() {
        let s = parse(
            "loadFile(\"a.mo\"); // comment\n/* block */ x := 1 + 2 * 3 ^ 2;\n\
             simulateModel(\"M(p=1)\", stopTime=2.5, resultFile=\"r\")\n\
             for i in 1:3 loop y = {i, 2.0e-3}; end for;\n\
             if x > 2 and not false then print(\"a\"); elseif x == 1 then exit(); else z = -x; end if;\n\
             Real r[2] = [1, 2];\n(ok, v) := f(1);",
        )
        .unwrap();
        assert_eq!(s.len(), 7);
        assert!(matches!(&s[0].stmt, Stmt::Expr(Expr::Call(n, _)) if n == "loadFile"));
        assert!(
            matches!(&s[2].stmt, Stmt::Expr(Expr::Call(_, a)) if a.len() == 3 && a[1].name.as_deref() == Some("stopTime"))
        );
        assert_eq!(s[2].line, 3);
        assert!(matches!(&s[3].stmt, Stmt::For(v, Expr::Range(..), b) if v == "i" && b.len() == 1));
        assert!(matches!(&s[4].stmt, Stmt::If(b, e) if b.len() == 2 && e.len() == 1));
        assert!(matches!(&s[5].stmt, Stmt::Decl(n, Some(_)) if n == "r"));
        assert!(matches!(&s[6].stmt, Stmt::TupleAssign(n, _) if n.len() == 2));
    }

    #[test]
    fn errors() {
        assert!(parse("f(1").is_err());
        assert!(parse("x := \"abc").is_err());
        let e = parse("a := 1\nb := 2 c").unwrap_err().to_string();
        assert!(e.contains("line 2"), "{e}");
    }
}
