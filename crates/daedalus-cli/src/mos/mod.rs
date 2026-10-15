//! `daedalus run script.mos`: a small interpreter for Dymola / OpenModelica scripts, so
//! that existing regression scripts run unchanged. See `docs/scripting.md` for the
//! supported commands.

mod parse;

use anyhow::{Result, anyhow, bail};
use daedalus_core::codegen::Model;
use daedalus_core::frontend::Library;
use daedalus_core::results::Results;
use daedalus_core::sim::{Method, SimOptions};
use parse::{Arg, Expr, Line, Stmt};
use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;

#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Real(f64),
    Int(i64),
    Bool(bool),
    Str(String),
    Array(Vec<Value>),
    /// Record: type name and fields.
    Record(String, Vec<(String, Value)>),
    None,
}

impl Value {
    fn num(&self) -> Option<f64> {
        match self {
            Value::Real(x) => Some(*x),
            Value::Int(i) => Some(*i as f64),
            _ => None,
        }
    }
    fn type_name(&self) -> &'static str {
        match self {
            Value::Real(_) => "Real",
            Value::Int(_) => "Integer",
            Value::Bool(_) => "Boolean",
            Value::Str(_) => "String",
            Value::Array(_) => "array",
            Value::Record(..) => "record",
            Value::None => "no value",
        }
    }
}

fn fmt_real(x: f64) -> String {
    if x.is_finite() && x == x.trunc() && x.abs() < 1e15 {
        format!("{x:.1}")
    } else {
        format!("{x}")
    }
}

impl std::fmt::Display for Value {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Value::Real(x) => f.write_str(&fmt_real(*x)),
            Value::Int(i) => write!(f, "{i}"),
            Value::Bool(b) => write!(f, "{b}"),
            Value::Str(s) => write!(f, "\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\"")),
            Value::Array(v) => {
                f.write_str("{")?;
                for (i, x) in v.iter().enumerate() {
                    if i > 0 {
                        f.write_str(", ")?;
                    }
                    write!(f, "{x}")?;
                }
                f.write_str("}")
            }
            Value::Record(name, fields) => {
                writeln!(f, "record {name}")?;
                for (i, (k, v)) in fields.iter().enumerate() {
                    let sep = if i + 1 < fields.len() { "," } else { "" };
                    writeln!(f, "    {k} = {v}{sep}")?;
                }
                write!(f, "end {name};")
            }
            Value::None => Ok(()),
        }
    }
}

/// Command line of `daedalus run`.
pub fn run_cli(args: &[String]) -> Result<std::process::ExitCode> {
    let mut script = None;
    let mut msl = Vec::new();
    let mut libs = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--msl" => {
                i += 1;
                msl.push(PathBuf::from(
                    args.get(i)
                        .ok_or_else(|| anyhow!("missing value for --msl"))?,
                ));
            }
            "-L" | "--lib" => {
                i += 1;
                libs.push(PathBuf::from(
                    args.get(i)
                        .ok_or_else(|| anyhow!("missing value for --lib"))?,
                ));
            }
            a if a.starts_with('-') => bail!("unknown option {a} for run"),
            a => {
                if script.is_some() {
                    bail!("run takes one script");
                }
                script = Some(PathBuf::from(a));
            }
        }
        i += 1;
    }
    let script = script.ok_or_else(|| anyhow!("usage: daedalus run script.mos [--msl DIR]"))?;
    let text =
        std::fs::read_to_string(&script).map_err(|e| anyhow!("{}: {e}", script.display()))?;
    let mut it = Interp::new(std::env::current_dir()?, msl);
    for l in libs {
        it.load_path(&l)?;
    }
    let code = it.run_script(&text)?;
    Ok(std::process::ExitCode::from(code.clamp(0, 255) as u8))
}

/// The roots searched by `loadModel`: `--msl`, `$DAEDALUS_MSL`, `$MODELICAPATH`, the
/// OpenModelica user library folder.
fn library_roots(msl: &[PathBuf]) -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = msl.to_vec();
    if let Some(p) = std::env::var_os("DAEDALUS_MSL") {
        roots.extend(std::env::split_paths(&p));
    }
    if let Some(p) = std::env::var_os("MODELICAPATH") {
        roots.extend(std::env::split_paths(&p));
    }
    if let Some(p) = std::env::var_os("DAEDALUS_LIB") {
        // The bundled library: its parent holds `Daedalus/`.
        let p = PathBuf::from(p);
        roots.push(p.parent().map(Path::to_path_buf).unwrap_or(p));
    }
    if let Some(h) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(h).join(".openmodelica/libraries"));
    }
    roots
}

/// Version from a package's `annotation(version = "...")`.
fn package_version(dir_or_file: &Path) -> Option<String> {
    let f = if dir_or_file.is_dir() {
        dir_or_file.join("package.mo")
    } else {
        dir_or_file.to_path_buf()
    };
    let text = std::fs::read_to_string(f).ok()?;
    let k = text.find("version")?;
    let rest = text[k + 7..].trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"')?;
    Some(rest[..rest.find('"')?].to_string())
}

/// Find library `name` (one of `versions` if given) under the roots.
fn find_library(roots: &[PathBuf], name: &str, versions: &[String]) -> Option<PathBuf> {
    let is_lib = |p: &Path| p.join("package.mo").is_file();
    let mut found: Vec<(PathBuf, Option<String>)> = Vec::new();
    for root in roots {
        // The root may be the library itself.
        if let Some(base) = root.file_name().map(|s| s.to_string_lossy().to_string())
            && base.split(' ').next() == Some(name)
            && is_lib(root)
        {
            found.push((root.clone(), package_version(root)));
        }
        let Ok(rd) = std::fs::read_dir(root) else {
            continue;
        };
        let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
        entries.sort();
        for p in entries {
            let fname = p
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let (base, ver) = match fname.split_once(' ') {
                Some((b, v)) => (b.to_string(), Some(v.trim_end_matches(".mo").to_string())),
                None => (fname.trim_end_matches(".mo").to_string(), None),
            };
            if base != name {
                continue;
            }
            if (p.is_dir() && is_lib(&p)) || (p.is_file() && fname.ends_with(".mo")) {
                let ver = ver.or_else(|| package_version(&p));
                found.push((p, ver));
            }
        }
    }
    if found.is_empty() {
        return None;
    }
    if !versions.is_empty() {
        for v in versions {
            if let Some((p, _)) = found.iter().find(|(_, fv)| {
                fv.as_deref()
                    .is_some_and(|fv| fv == v || fv.starts_with(&format!("{v}.")) || v == "default")
            }) {
                return Some(p.clone());
            }
        }
        if versions.iter().any(|v| v == "default") {
            return Some(found[0].0.clone());
        }
        return None;
    }
    Some(found[0].0.clone())
}

struct Interp {
    lib: Library,
    cwd: PathBuf,
    msl: Vec<PathBuf>,
    vars: HashMap<String, Value>,
    loaded: Vec<PathBuf>,
    models: HashMap<String, Rc<Model>>,
    /// Messages for `getErrorString()`.
    errors: String,
    failed: bool,
    exit: Option<i32>,
    last: Option<Results>,
    warned: std::collections::HashSet<String>,
}

enum Flow {
    Next,
    Exit,
}

impl Interp {
    fn new(cwd: PathBuf, msl: Vec<PathBuf>) -> Self {
        Interp {
            lib: Library::new(),
            cwd,
            msl,
            vars: HashMap::new(),
            loaded: Vec::new(),
            models: HashMap::new(),
            errors: String::new(),
            failed: false,
            exit: None,
            last: None,
            warned: Default::default(),
        }
    }

    fn path(&self, p: &str) -> PathBuf {
        let p = Path::new(p);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            self.cwd.join(p)
        }
    }

    /// A command failed: report it (stderr and `getErrorString()`), result `false`.
    fn fail(&mut self, msg: impl std::fmt::Display) -> Value {
        eprintln!("error: {msg}");
        let _ = writeln!(self.errors, "Error: {msg}");
        self.failed = true;
        Value::Bool(false)
    }

    fn warn(&mut self, msg: impl std::fmt::Display) {
        let msg = msg.to_string();
        if self.warned.insert(msg.clone()) {
            eprintln!("warning: {msg}");
            let _ = writeln!(self.errors, "Warning: {msg}");
        }
    }

    fn run_script(&mut self, text: &str) -> Result<i32> {
        let lines = parse::parse(text)?;
        self.exec_lines(&lines)?;
        Ok(self.exit.unwrap_or(if self.failed { 1 } else { 0 }))
    }

    fn exec_lines(&mut self, lines: &[Line]) -> Result<()> {
        for l in lines {
            match self.exec(&l.stmt) {
                Ok(Flow::Next) => {}
                Ok(Flow::Exit) => return Ok(()),
                Err(e) => bail!("line {}: {e:#}", l.line),
            }
        }
        Ok(())
    }

    fn exec_block(&mut self, body: &[Stmt]) -> Result<Flow> {
        for s in body {
            if let Flow::Exit = self.exec(s)? {
                return Ok(Flow::Exit);
            }
        }
        Ok(Flow::Next)
    }

    fn exec(&mut self, s: &Stmt) -> Result<Flow> {
        match s {
            Stmt::Expr(e) => {
                let v = self.eval(e)?;
                if self.exit.is_some() {
                    return Ok(Flow::Exit);
                }
                match &v {
                    Value::None => {}
                    Value::Record(..) => println!("{v}"),
                    _ => println!("= {v}"),
                }
            }
            Stmt::Assign(target, e) => {
                let v = self.eval(e)?;
                if self.exit.is_some() {
                    return Ok(Flow::Exit);
                }
                match target {
                    Expr::Name(n) if is_flag(n) => {
                        self.warn(format!("{n} is a tool setting: ignored"));
                    }
                    Expr::Name(n) => {
                        self.vars.insert(n.clone(), v);
                    }
                    Expr::Index(base, idx) => {
                        let Expr::Name(n) = &**base else {
                            bail!("cannot assign to this expression");
                        };
                        let idx: Vec<usize> = idx
                            .iter()
                            .map(|i| self.eval_index(i))
                            .collect::<Result<_>>()?;
                        let slot = self
                            .vars
                            .entry(n.clone())
                            .or_insert_with(|| Value::Array(Vec::new()));
                        set_index(slot, &idx, v)?;
                    }
                    _ => bail!("cannot assign to this expression"),
                }
            }
            Stmt::TupleAssign(names, e) => {
                let v = self.eval(e)?;
                let items = match v {
                    Value::Array(items) => items,
                    Value::Record(_, f) => f.into_iter().map(|(_, v)| v).collect(),
                    v => vec![v],
                };
                for (n, v) in names.iter().zip(items) {
                    if let Some(n) = n {
                        self.vars.insert(n.clone(), v);
                    }
                }
            }
            Stmt::Decl(n, init) => {
                let v = match init {
                    Some(e) => self.eval(e)?,
                    None => Value::None,
                };
                self.vars.insert(n.clone(), v);
            }
            Stmt::For(var, range, body) => {
                let items = match self.eval(range)? {
                    Value::Array(v) => v,
                    v => bail!("for loop over a {}", v.type_name()),
                };
                for it in items {
                    self.vars.insert(var.clone(), it);
                    if let Flow::Exit = self.exec_block(body)? {
                        return Ok(Flow::Exit);
                    }
                }
            }
            Stmt::While(c, body) => {
                let mut guard = 0u64;
                while self.eval_bool(c)? {
                    if let Flow::Exit = self.exec_block(body)? {
                        return Ok(Flow::Exit);
                    }
                    guard += 1;
                    if guard > 10_000_000 {
                        bail!("while loop does not terminate");
                    }
                }
            }
            Stmt::If(branches, els) => {
                for (c, body) in branches {
                    if self.eval_bool(c)? {
                        return self.exec_block(body);
                    }
                }
                return self.exec_block(els);
            }
        }
        Ok(Flow::Next)
    }

    fn eval_bool(&mut self, e: &Expr) -> Result<bool> {
        match self.eval(e)? {
            Value::Bool(b) => Ok(b),
            v => bail!("expected a Boolean, got a {}", v.type_name()),
        }
    }

    fn eval_index(&mut self, e: &Expr) -> Result<usize> {
        let v = self.eval(e)?;
        match v.num() {
            Some(x) if x >= 1.0 && x == x.trunc() => Ok(x as usize),
            _ => bail!("bad index {v}"),
        }
    }

    fn eval(&mut self, e: &Expr) -> Result<Value> {
        Ok(match e {
            Expr::Num(x) => Value::Real(*x),
            Expr::Int(i) => Value::Int(*i),
            Expr::Str(s) => Value::Str(s.clone()),
            Expr::Bool(b) => Value::Bool(*b),
            Expr::Name(n) => self.lookup(n)?,
            Expr::Index(base, idx) => {
                let mut v = self.eval(base)?;
                for i in idx {
                    let k = self.eval_index(i)?;
                    v = match v {
                        Value::Array(mut items) if k <= items.len() => items.swap_remove(k - 1),
                        v => bail!("index {k} out of range of {v}"),
                    };
                }
                v
            }
            Expr::Array(items) => {
                Value::Array(items.iter().map(|x| self.eval(x)).collect::<Result<_>>()?)
            }
            Expr::Matrix(rows) => Value::Array(
                rows.iter()
                    .map(|r| {
                        Ok(Value::Array(
                            r.iter().map(|x| self.eval(x)).collect::<Result<_>>()?,
                        ))
                    })
                    .collect::<Result<_>>()?,
            ),
            Expr::Tuple(items) => {
                Value::Array(items.iter().map(|x| self.eval(x)).collect::<Result<_>>()?)
            }
            Expr::Range(a, step, b) => {
                let a = self.eval(a)?;
                let b = self.eval(b)?;
                let s = match step {
                    Some(s) => self.eval(s)?,
                    None => Value::Int(1),
                };
                let ints = [&a, &b, &s].iter().all(|v| matches!(v, Value::Int(_)));
                let (a, b, s) = (num(&a)?, num(&b)?, num(&s)?);
                if s == 0.0 {
                    bail!("range with step 0");
                }
                let n = ((b - a) / s + 1e-10).floor();
                let n = if n < 0.0 { 0 } else { n as usize + 1 };
                Value::Array(
                    (0..n)
                        .map(|k| {
                            let x = a + k as f64 * s;
                            if ints {
                                Value::Int(x as i64)
                            } else {
                                Value::Real(x)
                            }
                        })
                        .collect(),
                )
            }
            Expr::Not(a) => Value::Bool(!self.eval_bool(a)?),
            Expr::Neg(a) => match self.eval(a)? {
                Value::Int(i) => Value::Int(-i),
                Value::Real(x) => Value::Real(-x),
                Value::Array(v) => Value::Array(
                    v.into_iter()
                        .map(|x| match x {
                            Value::Int(i) => Ok(Value::Int(-i)),
                            Value::Real(x) => Ok(Value::Real(-x)),
                            x => bail!("cannot negate {x}"),
                        })
                        .collect::<Result<_>>()?,
                ),
                v => bail!("cannot negate a {}", v.type_name()),
            },
            Expr::If(branches, els) => {
                for (c, v) in branches {
                    if self.eval_bool(c)? {
                        return self.eval(v);
                    }
                }
                self.eval(els)?
            }
            Expr::Bin(op, a, b) => {
                if *op == "and" || *op == "or" {
                    let x = self.eval_bool(a)?;
                    if (*op == "and") != x {
                        return Ok(Value::Bool(x));
                    }
                    return Ok(Value::Bool(self.eval_bool(b)?));
                }
                let x = self.eval(a)?;
                let y = self.eval(b)?;
                binop(op, x, y)?
            }
            Expr::Call(name, args) => self.call(name, args)?,
        })
    }

    fn lookup(&self, n: &str) -> Result<Value> {
        if let Some(v) = self.vars.get(n) {
            return Ok(v.clone());
        }
        // Record field: `r.field` (fields can be dotted variable names).
        let mut split = n.len();
        while let Some(k) = n[..split].rfind('.') {
            if let Some(Value::Record(_, fields)) = self.vars.get(&n[..k]) {
                let field = &n[k + 1..];
                return fields
                    .iter()
                    .find(|(f, _)| f == field)
                    .map(|(_, v)| v.clone())
                    .ok_or_else(|| anyhow!("record '{}' has no field '{field}'", &n[..k]));
            }
            split = k;
        }
        match n {
            "pi" => Ok(Value::Real(std::f64::consts::PI)),
            "Modelica.Constants.pi" => Ok(Value::Real(std::f64::consts::PI)),
            "Modelica.Constants.inf" => Ok(Value::Real(f64::INFINITY)),
            _ => bail!("unknown variable '{n}'"),
        }
    }

    /// Load a library directory or file (directories in the `package.mo` layout are
    /// parsed on demand).
    fn load_path(&mut self, p: &Path) -> Result<()> {
        let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
        if self.loaded.contains(&canon) {
            return Ok(());
        }
        if p.is_dir() {
            self.lib.add_dir_lazy(p)?;
        } else if p.file_name().is_some_and(|f| f == "package.mo")
            && let Some(dir) = p.parent()
        {
            // the whole package, like OpenModelica's loadFile
            self.lib.add_dir_lazy(dir)?;
        } else {
            let text = std::fs::read_to_string(p).map_err(|e| anyhow!("{}: {e}", p.display()))?;
            let errs = self.lib.set_document(&p.to_string_lossy(), &text);
            if !errs.is_empty() {
                bail!("{}", errs.join("\n"));
            }
        }
        self.loaded.push(canon);
        self.models.clear();
        Ok(())
    }

    /// Translate `class` (with modifiers `M(p = 1)` turned into parameter overrides).
    fn model(&mut self, problem: &str) -> Result<(Rc<Model>, Vec<(String, f64)>)> {
        let (class, mods) = split_modifiers(problem)?;
        let mut params = Vec::new();
        for (k, src) in mods {
            let lines = parse::parse(&format!("__v := {src};"))?;
            let v = match &lines[0].stmt {
                Stmt::Assign(_, e) => self.eval(e)?,
                _ => unreachable!(),
            };
            let x = match v {
                Value::Bool(b) => b as i32 as f64,
                v => v
                    .num()
                    .ok_or_else(|| anyhow!("modifier {k} = {v}: only numbers are supported"))?,
            };
            params.push((k, x));
        }
        if let Some(m) = self.models.get(&class) {
            return Ok((m.clone(), params));
        }
        if self.lib.has_pending() {
            self.lib.load_for(&class);
        }
        let opts = daedalus_core::backend::Options::default();
        let r = daedalus_core::translate(&self.lib, &class, &opts);
        let m = match r {
            Ok(m) => m,
            Err(_) if self.lib.has_pending() => {
                self.lib.load_all();
                daedalus_core::translate(&self.lib, &class, &opts)?
            }
            Err(e) => return Err(e),
        };
        for w in &m.warnings {
            self.warn(w);
        }
        let m = Rc::new(m);
        self.models.insert(class, m.clone());
        Ok((m, params))
    }

    fn call(&mut self, name: &str, args: &[Arg]) -> Result<Value> {
        let short = name
            .strip_prefix("Modelica.Utilities.Streams.")
            .or_else(|| name.strip_prefix("Modelica.Utilities.System."))
            .or_else(|| name.strip_prefix("Modelica.Math."))
            .or_else(|| name.strip_prefix("OpenModelica.Scripting."))
            .unwrap_or(name);
        if matches!(
            short,
            "plot"
                | "createPlot"
                | "plotExpression"
                | "plotArray"
                | "plotArrays"
                | "plotParametric"
                | "plotAll"
                | "removePlots"
                | "clearPlot"
                | "exportPlotAsImage"
                | "plotWindowSetup"
                | "plotSignalDifference"
                | "plotText"
        ) {
            // Arguments are not evaluated (OpenModelica's `plot({x, y})` names variables).
            self.warn(format!(
                "{short}: plotting is not supported, no plot made (open the .mat result in a plotting tool)"
            ));
            return Ok(Value::Bool(true));
        }
        // OpenModelica takes class names unquoted: `loadModel(Modelica)`,
        // `simulate(P.M, stopTime = 1)`.
        let class_arg = matches!(
            short,
            "loadModel"
                | "simulate"
                | "linearize"
                | "instantiateModel"
                | "checkModel"
                | "translateModel"
                | "buildModel"
                | "steadyState"
                | "trim"
                | "val"
        );
        let mut a = Vec::with_capacity(args.len());
        for (i, arg) in args.iter().enumerate() {
            let v = match &arg.value {
                Expr::Name(n) if class_arg && i == 0 && !self.vars.contains_key(n) => {
                    Value::Str(n.clone())
                }
                e => self.eval(e)?,
            };
            a.push((arg.name.clone(), v));
        }
        let math1 = |f: fn(f64) -> f64, a: &[(Option<String>, Value)]| -> Result<Value> {
            match a {
                [(None, v)] => Ok(Value::Real(f(num(v)?))),
                _ => bail!("{name} takes one argument"),
            }
        };
        match short {
            // ---- library and models ----
            "loadFile" | "openModel" => {
                let b = bind(
                    &a,
                    &[
                        "fileName",
                        "encoding",
                        "uses",
                        "notify",
                        "requireExactVersion",
                        "mustRead",
                        "changeDirectory",
                    ],
                    name,
                )?;
                let f = req_str(&b, 0, name)?;
                let p = self.path(&f);
                Ok(match self.load_path(&p) {
                    Ok(()) => Value::Bool(true),
                    Err(e) => self.fail(format!("loadFile(\"{f}\"): {e:#}")),
                })
            }
            "loadModel" => {
                let b = bind(
                    &a,
                    &[
                        "className",
                        "priorityVersion",
                        "notify",
                        "languageStandard",
                        "requireExactVersion",
                    ],
                    name,
                )?;
                let class = match &b[0] {
                    Some(Value::Str(s)) => s.clone(),
                    _ => bail!("loadModel: expected a class name"),
                };
                // `loadModel(Modelica)` parses as a variable reference: use the name.
                let versions: Vec<String> = match &b[1] {
                    Some(Value::Array(v)) => v
                        .iter()
                        .filter_map(|x| match x {
                            Value::Str(s) => Some(s.clone()),
                            _ => None,
                        })
                        .collect(),
                    Some(Value::Str(s)) => vec![s.clone()],
                    _ => Vec::new(),
                };
                Ok(self.load_model(&class, &versions))
            }
            "cd" => {
                if let Some((_, v)) = a.first() {
                    let Value::Str(d) = v else {
                        bail!("cd: expected a directory name");
                    };
                    let p = self.path(d);
                    if !p.is_dir() {
                        if let Err(e) = std::fs::create_dir_all(&p) {
                            return Ok(self.fail(format!("cd(\"{d}\"): {e}")));
                        }
                    }
                    self.cwd = p.canonicalize().unwrap_or(p);
                }
                Ok(Value::Str(self.cwd.to_string_lossy().to_string()))
            }
            "translateModel" | "checkModel" | "buildModel" => {
                let b = bind(&a, &["problem"], name)?;
                let problem = req_str(&b, 0, name)?;
                Ok(match self.model(&problem) {
                    Ok((m, _)) => {
                        if short == "checkModel" {
                            println!(
                                "Check of {} successful: {} states, {} variables",
                                m.name,
                                m.states.len(),
                                m.vars.iter().filter(|v| !v.name.starts_with('$')).count()
                            );
                        }
                        Value::Bool(true)
                    }
                    Err(e) => self.fail(format!("{short}(\"{problem}\"): {e:#}")),
                })
            }
            "instantiateModel" => {
                let b = bind(&a, &["className"], name)?;
                let class = req_str(&b, 0, name)?;
                if self.lib.has_pending() {
                    self.lib.load_for(&class);
                }
                Ok(match self.lib.flatten(&class) {
                    Ok(f) => Value::Str(f.dump()),
                    Err(e) => {
                        self.fail(format!("instantiateModel({class}): {e:#}"));
                        Value::Str(String::new())
                    }
                })
            }
            "simulateModel" | "simulateExtendedModel" => self.simulate_dymola(name, &a),
            "simulate" => self.simulate_om(&a),
            "linearizeModel" => self.linearize_dymola(&a),
            "linearize" => self.linearize_om(&a),
            "steadyState" | "trim" => self.steady(short, &a),
            "val" => {
                let b = bind(&a, &["var", "timePoint", "fileName"], name)?;
                let var = req_str(&b, 0, name)?;
                let res = match &b[2] {
                    Some(Value::Str(f)) => {
                        let bytes = std::fs::read(self.path(f))?;
                        daedalus_core::results::read_mat(&bytes).map_err(|e| anyhow!(e))?
                    }
                    _ => self
                        .last
                        .clone()
                        .ok_or_else(|| anyhow!("val: no simulation result yet"))?,
                };
                let v = match &b[1] {
                    Some(t) => res.at(&var, num(t)?),
                    None => res.final_value(&var),
                };
                Ok(match v {
                    Some(x) => Value::Real(x),
                    None => {
                        self.fail(format!("val: no variable '{var}' in the result"));
                        Value::Real(f64::NAN)
                    }
                })
            }
            // ---- misc ----
            "getErrorString" | "getMessagesString" => {
                Ok(Value::Str(std::mem::take(&mut self.errors)))
            }
            "getVersion" => Ok(Value::Str(format!(
                "DAEdalus {}",
                env!("CARGO_PKG_VERSION")
            ))),
            "print" => {
                for (_, v) in &a {
                    match v {
                        Value::Str(s) => println!("{s}"),
                        v => println!("{v}"),
                    }
                }
                Ok(Value::None)
            }
            "exit" | "quit" => {
                let code = match a.first() {
                    Some((_, v)) => num(v)? as i32,
                    None => 0,
                };
                self.exit = Some(code);
                Ok(Value::None)
            }
            "String" => match a.first() {
                Some((_, Value::Str(s))) => Ok(Value::Str(s.clone())),
                Some((_, Value::Real(x))) => Ok(Value::Str(format!("{x}"))),
                Some((_, v)) => Ok(Value::Str(v.to_string())),
                None => bail!("String() takes one argument"),
            },
            "integer" => Ok(Value::Int(
                num(&a.first().ok_or_else(|| anyhow!("integer()"))?.1)?.floor() as i64,
            )),
            "abs" => math1(f64::abs, &a),
            "sqrt" => math1(f64::sqrt, &a),
            "sin" => math1(f64::sin, &a),
            "cos" => math1(f64::cos, &a),
            "tan" => math1(f64::tan, &a),
            "exp" => math1(f64::exp, &a),
            "log" => math1(f64::ln, &a),
            "log10" => math1(f64::log10, &a),
            "floor" => math1(f64::floor, &a),
            "ceil" => math1(f64::ceil, &a),
            "min" | "max" | "sum" => {
                let mut xs = Vec::new();
                for (_, v) in &a {
                    flatten_nums(v, &mut xs)?;
                }
                if xs.is_empty() {
                    bail!("{short}() of nothing");
                }
                Ok(Value::Real(match short {
                    "min" => xs.iter().copied().fold(f64::INFINITY, f64::min),
                    "max" => xs.iter().copied().fold(f64::NEG_INFINITY, f64::max),
                    _ => xs.iter().sum(),
                }))
            }
            "size" => match a.first() {
                Some((_, Value::Array(v))) => Ok(Value::Int(v.len() as i64)),
                _ => Ok(Value::Int(1)),
            },
            "mod" => match a.as_slice() {
                [(_, x), (_, y)] => {
                    let (x, y) = (num(x)?, num(y)?);
                    Ok(Value::Real(x - (x / y).floor() * y))
                }
                _ => bail!("mod takes two arguments"),
            },
            "removeFile" | "deleteFile" => {
                let f = req_str(&bind(&a, &["fileName"], name)?, 0, name)?;
                Ok(Value::Bool(std::fs::remove_file(self.path(&f)).is_ok()))
            }
            "regularFileExists" | "exist" => {
                let f = req_str(&bind(&a, &["fileName"], name)?, 0, name)?;
                Ok(Value::Bool(self.path(&f).is_file()))
            }
            // ---- plotting and tool settings: accepted, no effect ----
            "experimentSetupOutput"
            | "setCommandLineOptions"
            | "setDebugFlags"
            | "clearlog"
            | "clear"
            | "savelog"
            | "OutputCPUtime"
            | "evaluate"
            | "Advanced"
            | "experiment"
            | "setModelicaPath"
            | "setDatesAndVersions"
            | "simulateMultiExtendedModel"
            | "SetVariable"
            | "setOption"
            | "system"
            | "importFMU"
            | "translateModelFMU"
            | "buildModelFMU"
            | "dumpXMLDAE" => {
                self.warn(format!("{short}(...) is ignored by daedalus run"));
                Ok(Value::Bool(true))
            }
            "runScript" | "RunScript" => self.run_nested(&a),
            _ => Ok(self.fail(format!("unknown function '{name}'"))),
        }
    }

    fn run_nested(&mut self, a: &[(Option<String>, Value)]) -> Result<Value> {
        let f = req_str(
            &bind(a, &["script", "silent"], "RunScript")?,
            0,
            "RunScript",
        )?;
        let p = self.path(&f);
        let text = std::fs::read_to_string(&p).map_err(|e| anyhow!("{}: {e}", p.display()))?;
        let lines = parse::parse(&text)?;
        self.exec_lines(&lines)?;
        Ok(Value::Bool(true))
    }

    fn load_model(&mut self, class: &str, versions: &[String]) -> Value {
        // Already loaded (a user file or a library defining it)?
        if versions.is_empty() && self.lib.tree().find_path(class).is_some() {
            return Value::Bool(true);
        }
        let roots = library_roots(&self.msl);
        let Some(p) = find_library(&roots, class, versions) else {
            let msg = if roots.is_empty() {
                format!(
                    "loadModel({class}): library not found (set DAEDALUS_MSL or MODELICAPATH, or pass --msl DIR)"
                )
            } else {
                format!(
                    "loadModel({class}{}): library not found in {}",
                    if versions.is_empty() {
                        String::new()
                    } else {
                        format!(
                            ", {{{}}}",
                            versions
                                .iter()
                                .map(|v| format!("\"{v}\""))
                                .collect::<Vec<_>>()
                                .join(", ")
                        )
                    },
                    roots
                        .iter()
                        .map(|r| r.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            };
            return self.fail(msg);
        };
        if let Err(e) = self.load_path(&p) {
            return self.fail(format!("loadModel({class}): {e:#}"));
        }
        // The Modelica Standard Library needs its companions.
        if class == "Modelica" {
            let mut near: Vec<PathBuf> = p
                .parent()
                .map(|d| vec![d.to_path_buf()])
                .unwrap_or_default();
            near.extend(roots.iter().cloned());
            for dep in ["ModelicaServices", "Complex"] {
                if self.lib.tree().find_path(dep).is_none()
                    && let Some(d) = find_library(&near, dep, &[])
                {
                    let _ = self.load_path(&d);
                }
            }
        }
        Value::Bool(true)
    }

    /// Simulation options for `problem` with the experiment annotation as defaults.
    fn sim_setup(
        &mut self,
        problem: &str,
        b: &[Option<Value>],
        idx: SimArgs,
    ) -> Result<(Rc<Model>, SimOptions)> {
        let (m, params) = self.model(problem)?;
        let mut o = SimOptions::from_model(&m);
        let get = |i: Option<usize>| i.and_then(|i| b[i].as_ref());
        if let Some(v) = get(idx.start) {
            o.start_time = num(v)?;
        }
        if let Some(v) = get(idx.stop) {
            o.stop_time = num(v)?;
        }
        let n = get(idx.intervals).map(num).transpose()?.unwrap_or(0.0);
        let dt = get(idx.interval).map(num).transpose()?.unwrap_or(0.0);
        if dt > 0.0 {
            o.interval = dt;
        } else if n > 0.0 {
            o.interval = 0.0;
            o.intervals = n as usize;
        }
        if let Some(v) = get(idx.tolerance) {
            let t = num(v)?;
            if t > 0.0 {
                o.tolerance = t;
            }
        }
        if let Some(Value::Str(mth)) = get(idx.method) {
            match Method::parse(mth) {
                Some(mm) => o.method = mm,
                None => self.warn(format!(
                    "integration method \"{mth}\" not available: using BDF"
                )),
            }
        }
        if let Some(v) = get(idx.fixed_step) {
            o.fixed_step = num(v)?;
        }
        o.params = params;
        Ok((m, o))
    }

    fn result_path(&self, name: &str, suffix: &str) -> PathBuf {
        let f = if name.ends_with(".mat") || name.ends_with(".csv") {
            name.to_string()
        } else {
            format!("{name}{suffix}")
        };
        self.path(&f)
    }

    fn simulate_dymola(&mut self, fname: &str, a: &[(Option<String>, Value)]) -> Result<Value> {
        let names = [
            "problem",
            "startTime",
            "stopTime",
            "numberOfIntervals",
            "outputInterval",
            "method",
            "tolerance",
            "fixedstepsize",
            "resultFile",
            "initialNames",
            "initialValues",
            "finalNames",
            "autoLoad",
        ];
        let b = bind(a, &names, fname)?;
        let extended = fname.ends_with("simulateExtendedModel");
        let problem = req_str(&b, 0, fname)?;
        let result = match &b[8] {
            Some(Value::Str(s)) => s.clone(),
            _ => "dsres".into(),
        };
        let fail_value = |ok: Value| {
            if extended {
                Value::Array(vec![ok, Value::Array(Vec::new())])
            } else {
                ok
            }
        };
        let (m, mut o) = match self.sim_setup(&problem, &b, SimArgs::DYMOLA) {
            Ok(x) => x,
            Err(e) => {
                return Ok(fail_value(
                    self.fail(format!("{fname}(\"{problem}\"): {e:#}")),
                ));
            }
        };
        if extended {
            let inames = str_list(&b[9]);
            let ivals = num_list(&b[10])?;
            if inames.len() != ivals.len() {
                bail!("initialNames and initialValues differ in length");
            }
            o.params.extend(inames.into_iter().zip(ivals));
        }
        let out = self.result_path(&result, ".mat");
        let ok = self.run_sim(&m, o, &out, &problem, fname);
        if !extended {
            return Ok(ok);
        }
        let finals: Vec<Value> = str_list(&b[11])
            .iter()
            .map(|n| {
                Value::Real(
                    self.last
                        .as_ref()
                        .and_then(|r| r.final_value(n))
                        .unwrap_or(f64::NAN),
                )
            })
            .collect();
        Ok(Value::Array(vec![ok, Value::Array(finals)]))
    }

    /// Simulate and write the result; `true` on success.
    fn run_sim(&mut self, m: &Model, o: SimOptions, out: &Path, problem: &str, f: &str) -> Value {
        match daedalus_core::sim::simulate(m, o) {
            Ok((res, _)) => {
                for l in &res.log {
                    eprintln!("{l}");
                }
                let w = write_results(&res, out);
                self.last = Some(res);
                match w {
                    Ok(()) => Value::Bool(true),
                    Err(e) => self.fail(format!("{}: {e}", out.display())),
                }
            }
            Err(e) => {
                if let Some(p) = &e.partial {
                    let _ = write_results(p, out);
                    self.last = Some(p.clone());
                }
                self.fail(format!("{f}(\"{problem}\"): {}", e.message))
            }
        }
    }

    fn simulate_om(&mut self, a: &[(Option<String>, Value)]) -> Result<Value> {
        let names = [
            "className",
            "startTime",
            "stopTime",
            "numberOfIntervals",
            "tolerance",
            "method",
            "fileNamePrefix",
            "options",
            "outputFormat",
            "variableFilter",
            "cflags",
            "simflags",
            "interval",
        ];
        let b = bind(a, &names, "simulate")?;
        let problem = match &b[0] {
            Some(Value::Str(s)) => s.clone(),
            _ => bail!("simulate: expected a class name"),
        };
        for (i, n) in names.iter().enumerate() {
            if matches!(n, &"options" | &"variableFilter" | &"cflags" | &"simflags")
                && b[i]
                    .as_ref()
                    .is_some_and(|v| v != &Value::Str(String::new()))
            {
                self.warn(format!("simulate: {n} is ignored"));
            }
        }
        let fmt = match &b[8] {
            Some(Value::Str(s)) => s.clone(),
            _ => "mat".into(),
        };
        let class = split_modifiers(&problem)?.0;
        let prefix = match &b[6] {
            Some(Value::Str(s)) => s.clone(),
            _ => class.clone(),
        };
        let t0 = std::time::Instant::now();
        let record = |file: String, opts: String, msg: String, t: f64| {
            Value::Record(
                "SimulationResult".into(),
                vec![
                    ("resultFile".into(), Value::Str(file)),
                    ("simulationOptions".into(), Value::Str(opts)),
                    ("messages".into(), Value::Str(msg)),
                    ("timeTotal".into(), Value::Real(t)),
                ],
            )
        };
        let (m, o) = match self.sim_setup(&problem, &b, SimArgs::OM) {
            Ok(x) => x,
            Err(e) => {
                let msg = format!("simulate({problem}): {e:#}");
                self.fail(&msg);
                return Ok(record(String::new(), String::new(), msg, 0.0));
            }
        };
        let opts = format!(
            "startTime = {}, stopTime = {}, numberOfIntervals = {}, tolerance = {}, method = '{}', fileNamePrefix = '{prefix}', outputFormat = '{fmt}'",
            fmt_real(o.start_time),
            fmt_real(o.stop_time),
            if o.interval > 0.0 {
                ((o.stop_time - o.start_time) / o.interval).round() as usize
            } else {
                o.intervals
            },
            o.tolerance,
            format!("{:?}", o.method).to_lowercase()
        );
        let out = self.path(&format!(
            "{prefix}_res.{}",
            if fmt == "csv" { "csv" } else { "mat" }
        ));
        let ok = self.run_sim(&m, o, &out, &problem, "simulate");
        let t = t0.elapsed().as_secs_f64();
        Ok(if ok == Value::Bool(true) {
            record(out.to_string_lossy().to_string(), opts, String::new(), t)
        } else {
            let msg = self.errors.lines().last().unwrap_or("").to_string();
            record(String::new(), opts, msg, t)
        })
    }

    fn linearization(
        &mut self,
        problem: &str,
        b: &[Option<Value>],
        idx: SimArgs,
    ) -> Result<daedalus_core::linearize::Linearization> {
        let (m, o) = self.sim_setup(problem, b, idx)?;
        // At the stop time when one is given, else right after initialization.
        let at = idx
            .stop
            .and_then(|i| b[i].as_ref())
            .map(num)
            .transpose()?
            .filter(|&t| t > o.start_time);
        daedalus_core::linearize::linearize(
            &m,
            o,
            at,
            daedalus_core::linearize::JacobianMethod::Exact,
        )
        .map_err(|e| anyhow!(e))
    }

    fn linearize_dymola(&mut self, a: &[(Option<String>, Value)]) -> Result<Value> {
        let names = [
            "problem",
            "startTime",
            "stopTime",
            "numberOfIntervals",
            "outputInterval",
            "method",
            "tolerance",
            "fixedstepsize",
            "resultFile",
        ];
        let b = bind(a, &names, "linearizeModel")?;
        let problem = req_str(&b, 0, "linearizeModel")?;
        let result = match &b[8] {
            Some(Value::Str(s)) => s.clone(),
            _ => "dslin".into(),
        };
        let lin = match self.linearization(&problem, &b, SimArgs::DYMOLA) {
            Ok(l) => l,
            Err(e) => return Ok(self.fail(format!("linearizeModel(\"{problem}\"): {e:#}"))),
        };
        let path = if result.ends_with(".json") {
            let mut v = serde_json::to_value(&lin)?;
            v["eigenvalues"] = serde_json::to_value(lin.eigenvalues())?;
            let p = self.path(&result);
            std::fs::write(&p, serde_json::to_string_pretty(&v)? + "\n")?;
            p
        } else {
            let p = self.result_path(&result, ".mat");
            let mut w = std::io::BufWriter::new(std::fs::File::create(&p)?);
            lin.write_dslin(&mut w)?;
            p
        };
        eprintln!("wrote {}", path.display());
        Ok(Value::Bool(true))
    }

    fn linearize_om(&mut self, a: &[(Option<String>, Value)]) -> Result<Value> {
        let names = [
            "className",
            "startTime",
            "stopTime",
            "numberOfIntervals",
            "tolerance",
            "method",
            "fileNamePrefix",
            "options",
            "outputFormat",
            "variableFilter",
            "cflags",
            "simflags",
            "interval",
        ];
        let b = bind(a, &names, "linearize")?;
        let problem = match &b[0] {
            Some(Value::Str(s)) => s.clone(),
            _ => bail!("linearize: expected a class name"),
        };
        let lin = match self.linearization(&problem, &b, SimArgs::OM) {
            Ok(l) => l,
            Err(e) => {
                let msg = format!("linearize({problem}): {e:#}");
                self.fail(&msg);
                return Ok(Value::Record(
                    "SimulationResult".into(),
                    vec![
                        ("resultFile".into(), Value::Str(String::new())),
                        ("messages".into(), Value::Str(msg)),
                    ],
                ));
            }
        };
        let p = self.path("linearized_model.mo");
        std::fs::write(&p, lin.to_modelica())?;
        Ok(Value::Record(
            "SimulationResult".into(),
            vec![
                (
                    "resultFile".into(),
                    Value::Str(p.to_string_lossy().to_string()),
                ),
                (
                    "messages".into(),
                    Value::Str(format!(
                        "Linear model is created at {}",
                        p.to_string_lossy()
                    )),
                ),
            ],
        ))
    }

    /// `steadyState(problem, atTime, tolerance)` and
    /// `trim(problem, free, targetNames, targetValues, atTime, tolerance)`.
    fn steady(&mut self, fname: &str, a: &[(Option<String>, Value)]) -> Result<Value> {
        let trim = fname == "trim";
        let names: &[&str] = if trim {
            &[
                "problem",
                "free",
                "targetNames",
                "targetValues",
                "atTime",
                "tolerance",
            ]
        } else {
            &["problem", "atTime", "tolerance"]
        };
        let b = bind(a, names, fname)?;
        let problem = req_str(&b, 0, fname)?;
        let get = |n: &str| {
            names
                .iter()
                .position(|x| *x == n)
                .and_then(|i| b[i].clone())
        };
        let mut so = daedalus_core::steady::SteadyOptions::default();
        if let Some(t) = get("atTime") {
            so.at_time = Some(num(&t)?);
        }
        if let Some(t) = get("tolerance") {
            so.tolerance = num(&t)?;
        }
        if trim {
            so.free = str_list(&get("free"));
            let tn = str_list(&get("targetNames"));
            let tv = num_list(&get("targetValues"))?;
            if tn.len() != tv.len() {
                bail!("trim: targetNames and targetValues differ in length");
            }
            so.targets = tn.into_iter().zip(tv).collect();
        }
        let r = self.model(&problem).and_then(|(m, params)| {
            let mut o = SimOptions::from_model(&m);
            o.params = params;
            daedalus_core::steady::steady_state(&m, o, &so).map_err(|e| anyhow!(e))
        });
        let s = match r {
            Ok(s) => s,
            Err(e) => {
                self.fail(format!("{fname}(\"{problem}\"): {e:#}"));
                return Ok(Value::Record(
                    "SteadyState".into(),
                    vec![("converged".into(), Value::Bool(false))],
                ));
            }
        };
        let mut fields = vec![
            ("converged".into(), Value::Bool(true)),
            ("time".into(), Value::Real(s.time)),
            ("iterations".into(), Value::Int(s.iterations as i64)),
            ("residual".into(), Value::Real(s.residual)),
            ("stable".into(), Value::Bool(s.stable)),
        ];
        let mut seen = std::collections::HashSet::new();
        for v in s
            .states
            .iter()
            .chain(&s.free)
            .chain(&s.inputs)
            .chain(&s.outputs)
        {
            if seen.insert(v.name.clone()) {
                fields.push((v.name.clone(), Value::Real(v.value)));
            }
        }
        for t in &s.targets {
            if seen.insert(t.name.clone()) {
                fields.push((t.name.clone(), Value::Real(t.value)));
            }
        }
        Ok(Value::Record("SteadyState".into(), fields))
    }
}

/// Argument positions of the simulation settings in the two tools' signatures.
#[derive(Clone, Copy)]
struct SimArgs {
    start: Option<usize>,
    stop: Option<usize>,
    intervals: Option<usize>,
    interval: Option<usize>,
    method: Option<usize>,
    tolerance: Option<usize>,
    fixed_step: Option<usize>,
}

impl SimArgs {
    const DYMOLA: SimArgs = SimArgs {
        start: Some(1),
        stop: Some(2),
        intervals: Some(3),
        interval: Some(4),
        method: Some(5),
        tolerance: Some(6),
        fixed_step: Some(7),
    };
    const OM: SimArgs = SimArgs {
        start: Some(1),
        stop: Some(2),
        intervals: Some(3),
        interval: Some(12),
        method: Some(5),
        tolerance: Some(4),
        fixed_step: None,
    };
}

fn write_results(res: &Results, out: &Path) -> std::io::Result<()> {
    if let Some(d) = out.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
    if out.extension().is_some_and(|e| e == "csv") {
        res.write_csv(&mut w, None)
    } else {
        res.write_mat(&mut w)
    }
}

/// Tool settings assigned in scripts (`Advanced.X = ...`, `OutputCPUtime = true`).
fn is_flag(n: &str) -> bool {
    n.starts_with("Advanced.")
        || n.starts_with("Evaluate")
        || matches!(
            n,
            "OutputCPUtime" | "Hidden.PrintFailureToDifferentiate" | "experimentSetupOutput"
        )
        || n.starts_with("Hidden.")
}

fn num(v: &Value) -> Result<f64> {
    v.num()
        .ok_or_else(|| anyhow!("expected a number, got {} {v}", v.type_name()))
}

fn flatten_nums(v: &Value, out: &mut Vec<f64>) -> Result<()> {
    match v {
        Value::Array(items) => {
            for x in items {
                flatten_nums(x, out)?;
            }
            Ok(())
        }
        v => {
            out.push(num(v)?);
            Ok(())
        }
    }
}

fn str_list(v: &Option<Value>) -> Vec<String> {
    match v {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(|x| match x {
                Value::Str(s) => Some(s.clone()),
                _ => None,
            })
            .collect(),
        Some(Value::Str(s)) => s
            .split(',')
            .map(|x| x.trim().to_string())
            .filter(|x| !x.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn num_list(v: &Option<Value>) -> Result<Vec<f64>> {
    let mut out = Vec::new();
    if let Some(v) = v {
        flatten_nums(v, &mut out)?;
    }
    Ok(out)
}

/// Bind positional and named arguments to `params`; unknown named arguments are
/// ignored with a warning.
fn bind(args: &[(Option<String>, Value)], params: &[&str], f: &str) -> Result<Vec<Option<Value>>> {
    let mut out = vec![None; params.len()];
    let mut pos = 0;
    for (n, v) in args {
        match n {
            None => {
                if pos >= params.len() {
                    bail!("{f}: too many arguments");
                }
                out[pos] = Some(v.clone());
                pos += 1;
            }
            Some(n) => match params.iter().position(|p| p == n) {
                Some(i) => out[i] = Some(v.clone()),
                None => eprintln!("warning: {f}: argument {n} is ignored"),
            },
        }
    }
    Ok(out)
}

fn req_str(b: &[Option<Value>], i: usize, f: &str) -> Result<String> {
    match &b[i] {
        Some(Value::Str(s)) => Ok(s.clone()),
        Some(v) => bail!("{f}: expected a string, got {} {v}", v.type_name()),
        None => bail!("{f}: missing argument"),
    }
}

fn set_index(slot: &mut Value, idx: &[usize], v: Value) -> Result<()> {
    let Some((&k, rest)) = idx.split_first() else {
        *slot = v;
        return Ok(());
    };
    if !matches!(slot, Value::Array(_)) {
        *slot = Value::Array(Vec::new());
    }
    let Value::Array(items) = slot else {
        unreachable!()
    };
    if items.len() < k {
        items.resize(k, Value::Real(0.0));
    }
    set_index(&mut items[k - 1], rest, v)
}

fn binop(op: &str, x: Value, y: Value) -> Result<Value> {
    use Value::*;
    Ok(match (op, x, y) {
        ("+", Str(a), Str(b)) => Str(a + &b),
        ("==", Str(a), Str(b)) => Bool(a == b),
        ("<>", Str(a), Str(b)) => Bool(a != b),
        ("==", Bool(a), Bool(b)) => Bool(a == b),
        ("<>", Bool(a), Bool(b)) => Bool(a != b),
        (op, Array(a), Array(b)) if matches!(op, "+" | "-") => {
            if a.len() != b.len() {
                bail!("array sizes differ ({} and {})", a.len(), b.len());
            }
            Array(
                a.into_iter()
                    .zip(b)
                    .map(|(x, y)| binop(op, x, y))
                    .collect::<Result<_>>()?,
            )
        }
        (op, Array(a), y) if matches!(op, "*" | "/" | "+" | "-" | "^") => Array(
            a.into_iter()
                .map(|x| binop(op, x, y.clone()))
                .collect::<Result<_>>()?,
        ),
        (op, x, Array(b)) if matches!(op, "*" | "+" | "-") => Array(
            b.into_iter()
                .map(|y| binop(op, x.clone(), y))
                .collect::<Result<_>>()?,
        ),
        (op, Int(a), Int(b)) if op != "/" && op != "^" => match op {
            "+" => Int(a + b),
            "-" => Int(a - b),
            "*" => Int(a * b),
            "==" => Bool(a == b),
            "<>" => Bool(a != b),
            "<" => Bool(a < b),
            "<=" => Bool(a <= b),
            ">" => Bool(a > b),
            ">=" => Bool(a >= b),
            _ => bail!("bad operator {op}"),
        },
        (op, x, y) => {
            let (a, b) = match (x.num(), y.num()) {
                (Some(a), Some(b)) => (a, b),
                _ => bail!(
                    "operator {op} on {} and {} ({x} {op} {y})",
                    x.type_name(),
                    y.type_name()
                ),
            };
            match op {
                "+" => Real(a + b),
                "-" => Real(a - b),
                "*" => Real(a * b),
                "/" => Real(a / b),
                "^" => Real(a.powf(b)),
                "==" => Bool(a == b),
                "<>" => Bool(a != b),
                "<" => Bool(a < b),
                "<=" => Bool(a <= b),
                ">" => Bool(a > b),
                ">=" => Bool(a >= b),
                _ => bail!("bad operator {op}"),
            }
        }
    })
}

/// `"P.M(p = 1, sub(k = 2), q.r = 3)"` → `("P.M", [("p", "1"), ("sub.k", "2"),
/// ("q.r", "3")])`.
fn split_modifiers(problem: &str) -> Result<(String, Vec<(String, String)>)> {
    let problem = problem.trim();
    let Some(open) = problem.find('(') else {
        return Ok((problem.to_string(), Vec::new()));
    };
    if !problem.ends_with(')') {
        bail!("bad model name '{problem}'");
    }
    let class = problem[..open].trim().to_string();
    let mut out = Vec::new();
    collect_mods(&problem[open + 1..problem.len() - 1], "", &mut out)?;
    Ok((class, out))
}

fn collect_mods(s: &str, prefix: &str, out: &mut Vec<(String, String)>) -> Result<()> {
    // split at top-level commas
    let mut parts = Vec::new();
    let (mut depth, mut start, mut in_str) = (0i32, 0, false);
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '(' | '{' | '[' if !in_str => depth += 1,
            ')' | '}' | ']' if !in_str => depth -= 1,
            ',' if depth == 0 && !in_str => {
                parts.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        let eq = p.find('=');
        let paren = p.find('(');
        match (eq, paren) {
            (Some(e), Some(o)) if o < e => {
                // nested: `sub(k = 2)` (possibly `sub(k = 2) = value`, unsupported)
                let name = p[..o].trim();
                let close = p.rfind(')').ok_or_else(|| anyhow!("bad modifier '{p}'"))?;
                collect_mods(&p[o + 1..close], &format!("{prefix}{name}."), out)?;
            }
            (Some(e), _) => {
                let name = p[..e].trim();
                let name = name
                    .strip_prefix("final ")
                    .or_else(|| name.strip_prefix("redeclare "))
                    .unwrap_or(name)
                    .trim();
                out.push((format!("{prefix}{name}"), p[e + 1..].trim().to_string()));
            }
            _ => bail!("modifier '{p}' has no value"),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers() {
        let (c, m) = split_modifiers("P.M(p = 1, sub(k = 2, l=-3e-2), q.r = {1,2})").unwrap();
        assert_eq!(c, "P.M");
        assert_eq!(
            m,
            vec![
                ("p".to_string(), "1".to_string()),
                ("sub.k".to_string(), "2".to_string()),
                ("sub.l".to_string(), "-3e-2".to_string()),
                ("q.r".to_string(), "{1,2}".to_string()),
            ]
        );
        assert_eq!(split_modifiers("M").unwrap().0, "M");
    }

    #[test]
    fn expressions() {
        let mut it = Interp::new(std::env::temp_dir(), Vec::new());
        it.run_script(
            "x := 2; y := x * 3 + 1; s := \"a\" + String(y); v := {1, 2, 3} * 2;\n\
             acc := 0; for i in 1:4 loop acc := acc + i; end for;\n\
             if acc > 5 then big := true; else big := false; end if;\n\
             k := 0; while k < 3 loop k := k + 1; end while; a[2] := 5;",
        )
        .unwrap();
        assert_eq!(it.vars["y"], Value::Int(7));
        assert_eq!(it.vars["s"], Value::Str("a7".into()));
        assert_eq!(
            it.vars["v"],
            Value::Array(vec![Value::Int(2), Value::Int(4), Value::Int(6)])
        );
        assert_eq!(it.vars["acc"], Value::Int(10));
        assert_eq!(it.vars["big"], Value::Bool(true));
        assert_eq!(it.vars["k"], Value::Int(3));
        assert_eq!(
            it.vars["a"],
            Value::Array(vec![Value::Real(0.0), Value::Int(5)])
        );
        assert_eq!(it.run_script("exit(3); x := 1;").unwrap(), 3);
    }
}
