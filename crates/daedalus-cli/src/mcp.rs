//! `daedalus mcp`: a Model Context Protocol server over stdio.
//!
//! An AI assistant (Claude Code, Claude Desktop, any MCP client) gets tools to browse
//! the loaded libraries, read and write model source, check, simulate and linearize
//! models, and read the analysis reports. Messages are newline-delimited JSON-RPC 2.0
//! on stdin/stdout; diagnostics go to stderr.

use daedalus_core::backend::Options;
use daedalus_core::frontend::Library;
use daedalus_core::sim::{Method, SimOptions};
use serde_json::{Value, json};
use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

const PROTOCOL: &str = "2025-06-18";

pub struct Server {
    lib: Library,
    /// Directory `write_model` may write into (none: edits stay in memory).
    workspace: Option<PathBuf>,
}

/// One tool: name, description and JSON schema of its arguments.
struct Tool {
    name: &'static str,
    description: &'static str,
    schema: fn() -> Value,
}

fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": props, "required": required})
}

fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_classes",
            description: "List the classes in the loaded libraries under a package (or the top-level packages). Models with an experiment annotation are marked runnable.",
            schema: || {
                obj(
                    json!({
                        "package": {"type": "string", "description": "Dotted package name, e.g. Modelica.Blocks.Examples; empty for the top level"},
                        "depth": {"type": "integer", "description": "Levels to expand (default 1)"}
                    }),
                    &[],
                )
            },
        },
        Tool {
            name: "get_class",
            description: "Modelica source text of a class, with its kind, description, components and experiment annotation.",
            schema: || obj(json!({"path": {"type": "string"}}), &["path"]),
        },
        Tool {
            name: "set_source",
            description: "Load or replace a Modelica file in memory (and on disk when the server runs with --workspace). Returns parse errors, if any. Use it to create or edit models, then check them.",
            schema: || {
                obj(
                    json!({
                        "file": {"type": "string", "description": "File name such as MyModels.mo (relative to the workspace)"},
                        "text": {"type": "string", "description": "Complete file contents"}
                    }),
                    &["file", "text"],
                )
            },
        },
        Tool {
            name: "check_model",
            description: "Translate a model: balance, states, algebraic loops and warnings, or the translation error.",
            schema: || model_schema(json!({})),
        },
        Tool {
            name: "simulate_model",
            description: "Simulate a model and summarize the result: solver statistics, and for each requested variable its final, minimum and maximum value plus a short table over time.",
            schema: || {
                model_schema(json!({
                    "stop_time": {"type": "number"},
                    "start_time": {"type": "number"},
                    "tolerance": {"type": "number"},
                    "method": {"type": "string", "description": "Integrator, e.g. bdf, dopri5, rk4, euler"},
                    "parameters": {"type": "object", "additionalProperties": {"type": "number"}, "description": "Parameter overrides, name to value"},
                    "variables": {"type": "array", "items": {"type": "string"}, "description": "Variables to report (default: the states)"},
                    "points": {"type": "integer", "description": "Rows of the time table (default 11)"}
                }))
            },
        },
        Tool {
            name: "linearize_model",
            description: "Linear state-space model A, B, C, D around the initial point or after simulating to time `at`, with eigenvalues.",
            schema: || model_schema(json!({"at": {"type": "number"}})),
        },
        Tool {
            name: "model_structure",
            description: "Structure report: variable roles, BLT blocks and algebraic loops, the state-selection sets and why each state was chosen, and the state Jacobian at the start (or after simulating to `at`): eigenvalues, time constants, participation factors, stiffness ratio and a solver suggestion. `states` overrides stateSelect, e.g. {\"body.v\": \"prefer\"}.",
            schema: || {
                model_schema(json!({
                    "at": {"type": "number"},
                    "states": {"type": "object", "additionalProperties": {"type": "string", "enum": ["always", "prefer", "default", "avoid", "never"]}}
                }))
            },
        },
        Tool {
            name: "profile_simulation",
            description: "Simulate with the performance profiler: time split, which states limit the step size, the costliest blocks, the event log summary and chattering relations, with hints.",
            schema: || {
                model_schema(json!({
                    "stop_time": {"type": "number"},
                    "parameters": {"type": "object", "additionalProperties": {"type": "number"}}
                }))
            },
        },
        Tool {
            name: "equations",
            description: "The sorted, causalised equation system after alias elimination, index reduction and tearing (long for big models; use `max_lines`).",
            schema: || model_schema(json!({"max_lines": {"type": "integer"}})),
        },
        Tool {
            name: "flatten_model",
            description: "The flat model (all components expanded) as Modelica-like text.",
            schema: || model_schema(json!({"max_lines": {"type": "integer"}})),
        },
    ]
}

fn model_schema(mut extra: Value) -> Value {
    extra.as_object_mut().unwrap().insert(
        "model".into(),
        json!({"type": "string", "description": "Full dotted model name"}),
    );
    obj(extra, &["model"])
}

impl Server {
    pub fn new(lib: Library, workspace: Option<PathBuf>) -> Self {
        Server { lib, workspace }
    }

    /// Serve requests from `input` until it closes.
    pub fn serve(&mut self, input: impl BufRead, mut output: impl Write) -> anyhow::Result<()> {
        for line in input.lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            let reply = match serde_json::from_str::<Value>(&line) {
                Ok(msg) => self.handle(&msg),
                Err(e) => Some(error(Value::Null, -32700, &format!("parse error: {e}"))),
            };
            if let Some(r) = reply {
                writeln!(output, "{}", serde_json::to_string(&r)?)?;
                output.flush()?;
            }
        }
        Ok(())
    }

    /// Answer one JSON-RPC message; notifications get no reply.
    pub fn handle(&mut self, msg: &Value) -> Option<Value> {
        let id = msg.get("id").cloned();
        let method = msg.get("method").and_then(Value::as_str).unwrap_or("");
        let params = msg.get("params").cloned().unwrap_or(json!({}));
        let id = id?; // notification
        let result = match method {
            "initialize" => {
                let version = params
                    .get("protocolVersion")
                    .and_then(Value::as_str)
                    .unwrap_or(PROTOCOL);
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {}},
                    "serverInfo": {"name": "daedalus", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": "DAEdalus Modelica tools. Browse with list_classes/get_class, edit with set_source, then check_model before simulate_model. Every edit should be checked."
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": tools().iter().map(|t| json!({
                "name": t.name, "description": t.description, "inputSchema": (t.schema)()
            })).collect::<Vec<_>>()}),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                if !tools().iter().any(|t| t.name == name) {
                    return Some(error(id, -32602, &format!("unknown tool '{name}'")));
                }
                match self.call(name, &args) {
                    Ok(text) => {
                        json!({"content": [{"type": "text", "text": text}], "isError": false})
                    }
                    Err(e) => {
                        json!({"content": [{"type": "text", "text": format!("error: {e:#}")}], "isError": true})
                    }
                }
            }
            _ => return Some(error(id, -32601, &format!("method not found: {method}"))),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    fn call(&mut self, name: &str, a: &Value) -> anyhow::Result<String> {
        let s = |k: &str| a.get(k).and_then(Value::as_str).map(str::to_string);
        let n = |k: &str| a.get(k).and_then(Value::as_f64);
        let model = || s("model").ok_or_else(|| anyhow::anyhow!("missing 'model'"));
        match name {
            "list_classes" => {
                let pkg = s("package").unwrap_or_default();
                let depth = a.get("depth").and_then(Value::as_u64).unwrap_or(1).max(1) as usize;
                if !pkg.is_empty() {
                    self.lib.load_class(&pkg, false);
                }
                let tree = daedalus_core::frontend::inspect::class_tree(&self.lib);
                let mut out = String::new();
                let found = if pkg.is_empty() {
                    list(&tree, depth, 0, &mut out);
                    true
                } else {
                    find_and_list(&tree, &pkg, depth, &mut out)
                };
                anyhow::ensure!(found, "no class '{pkg}'");
                Ok(out)
            }
            "get_class" => {
                let path = s("path").ok_or_else(|| anyhow::anyhow!("missing 'path'"))?;
                self.lib.load_class(&path, true);
                let info = daedalus_core::frontend::inspect::class_info(&self.lib, &path)
                    .ok_or_else(|| anyhow::anyhow!("no class '{path}'"))?;
                let mut out = format!("{} {} — {}\n", info.kind, info.path, info.description);
                if let Some(src) = &info.source {
                    let text = self
                        .lib
                        .docs
                        .iter()
                        .find(|d| d.uri == src.uri)
                        .map(|d| d.text.to_string());
                    if let Some(t) = text {
                        out += &format!("file {} line {}\n\n", src.uri, src.line);
                        out += t.get(src.start..src.end).unwrap_or("");
                        out.push('\n');
                    }
                }
                Ok(out)
            }
            "set_source" => {
                let file = s("file").ok_or_else(|| anyhow::anyhow!("missing 'file'"))?;
                let text = s("text").ok_or_else(|| anyhow::anyhow!("missing 'text'"))?;
                anyhow::ensure!(
                    !file.contains("..") && !Path::new(&file).is_absolute(),
                    "file must be a relative path inside the workspace"
                );
                let mut note = String::new();
                if let Some(ws) = &self.workspace {
                    let p = ws.join(&file);
                    if let Some(d) = p.parent() {
                        std::fs::create_dir_all(d)?;
                    }
                    std::fs::write(&p, &text)?;
                    note = format!("written to {}\n", p.display());
                }
                let errs = self.lib.set_document(&file, &text);
                if errs.is_empty() {
                    Ok(format!("{note}{file}: parsed without errors"))
                } else {
                    Ok(format!("{note}{file}: parse errors\n{}", errs.join("\n")))
                }
            }
            "check_model" => {
                let model = model()?;
                let m = self.translate(&model)?;
                let st = &m.stats;
                let mut out = format!(
                    "model {model} translates.\n  flat: {} variables, {} equations\n  states: {} ({} dummy derivatives)\n  blocks: {}, algebraic loops: {} (largest {}, {} linear)\n  events: {} relations, {} when-clauses\n",
                    st.flat_variables,
                    st.flat_equations,
                    st.states,
                    st.dummy_derivatives,
                    st.blocks,
                    st.algebraic_loops,
                    st.largest_loop,
                    st.linear_loops,
                    st.relations,
                    st.when_clauses
                );
                out += &format!(
                    "  states: {}\n",
                    m.states
                        .iter()
                        .map(|&v| m.vars[v as usize].name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                for w in &m.warnings {
                    out += &format!("warning: {w}\n");
                }
                Ok(out)
            }
            "simulate_model" => {
                let model = model()?;
                let m = self.translate(&model)?;
                let mut o = SimOptions::from_model(&m);
                if let Some(v) = n("stop_time") {
                    o.stop_time = v;
                }
                if let Some(v) = n("start_time") {
                    o.start_time = v;
                }
                if let Some(v) = n("tolerance") {
                    o.tolerance = v;
                }
                if let Some(meth) = s("method") {
                    o.method = Method::parse(&meth)
                        .ok_or_else(|| anyhow::anyhow!("unknown method '{meth}'"))?;
                }
                if let Some(Value::Object(p)) = a.get("parameters") {
                    for (k, v) in p {
                        let v = v
                            .as_f64()
                            .ok_or_else(|| anyhow::anyhow!("parameter {k}: not a number"))?;
                        o.params.push((k.clone(), v));
                    }
                }
                if o.max_wall_time <= 0.0 || o.max_wall_time > 120.0 {
                    o.max_wall_time = 120.0;
                }
                let vars: Vec<String> = match a.get("variables").and_then(Value::as_array) {
                    Some(v) => v
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect(),
                    None => m
                        .states
                        .iter()
                        .take(8)
                        .map(|&v| m.vars[v as usize].name.clone())
                        .collect(),
                };
                let points = a
                    .get("points")
                    .and_then(Value::as_u64)
                    .unwrap_or(11)
                    .clamp(2, 200) as usize;
                let (res, stats, err) = match daedalus_core::sim::simulate(&m, o) {
                    Ok((r, s)) => (r, Some(s), None),
                    Err(e) => (e.partial.unwrap_or_default(), None, Some(e.message)),
                };
                Ok(summarize(
                    &model,
                    &res,
                    stats.as_ref(),
                    err.as_deref(),
                    &vars,
                    points,
                ))
            }
            "linearize_model" => {
                let model = model()?;
                let m = self.translate(&model)?;
                let o = SimOptions::from_model(&m);
                let lin = daedalus_core::linearize::linearize(
                    &m,
                    o,
                    n("at"),
                    daedalus_core::linearize::JacobianMethod::Exact,
                )
                .map_err(|e| anyhow::anyhow!(e))?;
                Ok(lin.report())
            }
            "model_structure" => {
                let model = model()?;
                self.prepare(&model);
                let mut bopts = Options::default();
                if let Some(Value::Object(st)) = a.get("states") {
                    for (k, v) in st {
                        let tier = v
                            .as_str()
                            .and_then(daedalus_core::backend::state_select::parse_tier)
                            .ok_or_else(|| anyhow::anyhow!("state {k}: unknown tier"))?;
                        bopts.state_overrides.push(
                            daedalus_core::backend::state_select::StateOverride {
                                pattern: k.clone(),
                                tier,
                            },
                        );
                    }
                }
                let sopts = daedalus_core::structure::StructureOptions {
                    at: n("at"),
                    ..Default::default()
                };
                let r = daedalus_core::structure::structure(&self.lib, &model, &bopts, &sopts)
                    .or_else(|e| {
                        if self.lib.has_pending() {
                            self.lib.load_all();
                            daedalus_core::structure::structure(&self.lib, &model, &bopts, &sopts)
                        } else {
                            Err(e)
                        }
                    })?;
                let text = r.to_text();
                Ok(if text.len() > 40_000 {
                    format!("{}\n… truncated", &text[..text.floor_char_boundary(40_000)])
                } else {
                    text
                })
            }
            "profile_simulation" => {
                let model = model()?;
                let m = self.translate(&model)?;
                let mut o = SimOptions::from_model(&m);
                if let Some(v) = n("stop_time") {
                    o.stop_time = v;
                }
                if let Some(Value::Object(p)) = a.get("parameters") {
                    for (k, v) in p {
                        o.params.push((k.clone(), v.as_f64().unwrap_or(f64::NAN)));
                    }
                }
                if o.max_wall_time <= 0.0 || o.max_wall_time > 120.0 {
                    o.max_wall_time = 120.0;
                }
                o.profile = true;
                let (r, prof) = daedalus_core::sim::simulate_profiled(&m, o);
                let mut out = match r {
                    Ok(_) => format!("simulation of {model} finished.\n"),
                    Err(e) => format!("simulation of {model} FAILED: {}\n", e.message),
                };
                if let Some(p) = prof {
                    out += &p.summary_text();
                }
                Ok(out)
            }
            "equations" | "flatten_model" => {
                let model = model()?;
                self.prepare(&model);
                let text = if name == "equations" {
                    let flat = self.flatten(&model)?;
                    daedalus_core::backend::compile(flat, &Options::default())?.dump()
                } else {
                    self.flatten(&model)?.dump()
                };
                let max = a.get("max_lines").and_then(Value::as_u64).unwrap_or(400) as usize;
                let total = text.lines().count();
                let mut out: String = text.lines().take(max).collect::<Vec<_>>().join("\n");
                if total > max {
                    out += &format!("\n… {} more lines (raise max_lines)", total - max);
                }
                Ok(out)
            }
            _ => unreachable!(),
        }
    }

    fn prepare(&mut self, model: &str) {
        if self.lib.has_pending() {
            self.lib.load_for(model);
        }
    }

    fn flatten(&mut self, model: &str) -> anyhow::Result<daedalus_core::flat::FlatModel> {
        let r = self.lib.flatten(model);
        if r.is_err() && self.lib.has_pending() {
            self.lib.load_all();
            return self.lib.flatten(model);
        }
        r
    }

    fn translate(&mut self, model: &str) -> anyhow::Result<daedalus_core::codegen::Model> {
        self.prepare(model);
        let r = daedalus_core::translate(&self.lib, model, &Options::default());
        if r.is_err() && self.lib.has_pending() {
            self.lib.load_all();
            return daedalus_core::translate(&self.lib, model, &Options::default());
        }
        r
    }
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

type Entry = daedalus_core::frontend::inspect::ClassEntry;

fn list(entries: &[Entry], depth: usize, indent: usize, out: &mut String) {
    for e in entries {
        out.push_str(&format!(
            "{}{} {}{}{}\n",
            "  ".repeat(indent),
            e.kind,
            e.path,
            if e.runnable { " [runnable]" } else { "" },
            if e.description.is_empty() {
                String::new()
            } else {
                format!(" — {}", e.description)
            }
        ));
        if depth > 1 {
            list(&e.children, depth - 1, indent + 1, out);
        }
    }
}

fn find_and_list(entries: &[Entry], path: &str, depth: usize, out: &mut String) -> bool {
    for e in entries {
        if e.path == path {
            if e.children.is_empty() {
                list(std::slice::from_ref(e), 1, 0, out);
            } else {
                list(&e.children, depth, 0, out);
            }
            return true;
        }
        if path.starts_with(&format!("{}.", e.path)) && find_and_list(&e.children, path, depth, out)
        {
            return true;
        }
    }
    false
}

fn fmt(x: f64) -> String {
    if x == 0.0 || (1e-3..1e5).contains(&x.abs()) {
        format!("{:.6}", x)
            .trim_end_matches('0')
            .trim_end_matches('.')
            .to_string()
    } else {
        format!("{x:.4e}")
    }
}

fn summarize(
    model: &str,
    res: &daedalus_core::results::Results,
    stats: Option<&daedalus_core::sim::SimStats>,
    err: Option<&str>,
    vars: &[String],
    points: usize,
) -> String {
    let mut out = String::new();
    match err {
        Some(e) => out += &format!("simulation of {model} FAILED: {e}\n"),
        None => out += &format!("simulation of {model} finished.\n"),
    }
    if let Some(s) = stats {
        out += &format!(
            "  {} output points, {} steps ({} rejected), {} events, {:.1} ms\n",
            s.output_points, s.solver.steps, s.solver.rejected, s.events, s.sim_ms
        );
    }
    for l in res.log.iter().take(20) {
        out += &format!("log: {l}\n");
    }
    let t = &res.time;
    if t.is_empty() {
        return out;
    }
    out += &format!("  time {} .. {}\n\n", fmt(t[0]), fmt(t[t.len() - 1]));
    let mut cols: Vec<(&str, Vec<f64>)> = Vec::new();
    for v in vars {
        match res.values(v) {
            Some(y) => {
                let (lo, hi) = y
                    .iter()
                    .fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), &x| {
                        (a.min(x), b.max(x))
                    });
                out += &format!(
                    "{v}: final {}, min {}, max {}\n",
                    fmt(*y.last().unwrap_or(&f64::NAN)),
                    fmt(lo),
                    fmt(hi)
                );
                cols.push((v, y));
            }
            None => out += &format!("{v}: not in the result\n"),
        }
    }
    if !cols.is_empty() {
        out += &format!(
            "\ntime\t{}\n",
            cols.iter().map(|c| c.0).collect::<Vec<_>>().join("\t")
        );
        let (t0, t1) = (t[0], t[t.len() - 1]);
        for k in 0..points {
            let tq = t0 + (t1 - t0) * k as f64 / (points - 1) as f64;
            let i = t.partition_point(|&x| x < tq).min(t.len() - 1);
            out += &fmt(t[i]);
            for c in &cols {
                out += &format!("\t{}", fmt(c.1[i]));
            }
            out.push('\n');
        }
    }
    out
}

/// `daedalus mcp [sources...] [-L DIR]... [--workspace DIR]`
pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut lib = Library::new();
    let mut workspace = None;
    let mut dirs: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-L" | "--lib" => {
                i += 1;
                dirs.push(
                    args.get(i)
                        .cloned()
                        .ok_or_else(|| anyhow::anyhow!("missing value for -L"))?,
                );
            }
            "--workspace" => {
                i += 1;
                workspace =
                    Some(PathBuf::from(args.get(i).ok_or_else(|| {
                        anyhow::anyhow!("missing value for --workspace")
                    })?));
            }
            a if a.starts_with('-') => anyhow::bail!("unknown option {a}"),
            a => dirs.push(a.to_string()),
        }
        i += 1;
    }
    if let Ok(d) = std::env::var("DAEDALUS_LIB") {
        dirs.push(d);
    }
    for s in &dirs {
        let p = Path::new(s);
        if p.is_dir() {
            lib.add_dir_lazy(p)?;
        } else {
            let text = std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{s}: {e}"))?;
            lib.set_document(s, &text);
        }
    }
    if let Some(ws) = &workspace {
        std::fs::create_dir_all(ws)?;
        for e in std::fs::read_dir(ws)? {
            let p = e?.path();
            if p.extension().is_some_and(|x| x == "mo") {
                let rel = p
                    .strip_prefix(ws)
                    .unwrap_or(&p)
                    .to_string_lossy()
                    .to_string();
                lib.set_document(&rel, &std::fs::read_to_string(&p)?);
            }
        }
    }
    eprintln!("daedalus mcp: serving on stdio");
    let stdin = std::io::stdin();
    Server::new(lib, workspace).serve(stdin.lock(), std::io::stdout().lock())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MODEL: &str = "model Decay\n  parameter Real k = 2;\n  Real x(start = 1, fixed = true);\nequation\n  der(x) = -k*x;\n  annotation(experiment(StopTime = 1));\nend Decay;\n";

    fn call(s: &mut Server, id: i64, method: &str, params: Value) -> Value {
        s.handle(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))
            .expect("reply")
    }

    fn tool(s: &mut Server, name: &str, args: Value) -> (String, bool) {
        let r = call(s, 9, "tools/call", json!({"name": name, "arguments": args}));
        let res = &r["result"];
        (
            res["content"][0]["text"].as_str().unwrap().to_string(),
            res["isError"].as_bool().unwrap(),
        )
    }

    #[test]
    fn handshake_and_tool_list() {
        let mut s = Server::new(Library::new(), None);
        let r = call(
            &mut s,
            1,
            "initialize",
            json!({"protocolVersion": "2025-06-18"}),
        );
        assert_eq!(r["result"]["protocolVersion"], "2025-06-18");
        assert!(
            s.handle(&json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
                .is_none()
        );
        let r = call(&mut s, 2, "tools/list", json!({}));
        let names: Vec<&str> = r["result"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert!(names.contains(&"simulate_model"));
        assert!(r["result"]["tools"][0]["inputSchema"]["type"] == "object");
        let r = call(&mut s, 3, "nope", json!({}));
        assert_eq!(r["error"]["code"], -32601);
    }

    #[test]
    fn edit_check_simulate() {
        let mut s = Server::new(Library::new(), None);
        let (t, err) = tool(
            &mut s,
            "set_source",
            json!({"file": "Decay.mo", "text": MODEL}),
        );
        assert!(!err && t.contains("without errors"), "{t}");
        let (t, err) = tool(&mut s, "check_model", json!({"model": "Decay"}));
        assert!(!err && t.contains("states: 1"), "{t}");
        let (t, err) = tool(
            &mut s,
            "simulate_model",
            json!({"model": "Decay", "parameters": {"k": 1.0}, "variables": ["x"]}),
        );
        assert!(!err, "{t}");
        // x(1) = exp(-1) with k overridden to 1
        let line = t.lines().find(|l| l.starts_with("x: final")).unwrap();
        let v: f64 = line
            .split_whitespace()
            .nth(2)
            .unwrap()
            .trim_end_matches(',')
            .parse()
            .unwrap();
        assert!((v - (-1f64).exp()).abs() < 1e-4, "{line}");
        let (t, err) = tool(&mut s, "linearize_model", json!({"model": "Decay"}));
        assert!(!err && t.contains("-2"), "{t}");
        let (t, err) = tool(&mut s, "model_structure", json!({"model": "Decay"}));
        assert!(!err && t.contains("structure of Decay"), "{t}");
        let (t, err) = tool(&mut s, "profile_simulation", json!({"model": "Decay"}));
        assert!(!err && t.contains("finished"), "{t}");
        let (t, err) = tool(&mut s, "check_model", json!({"model": "Missing"}));
        assert!(err && t.starts_with("error"), "{t}");
        let (t, err) = tool(&mut s, "set_source", json!({"file": "../x.mo", "text": ""}));
        assert!(err, "{t}");
    }
}
