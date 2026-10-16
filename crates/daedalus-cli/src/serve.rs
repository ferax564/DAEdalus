//! `daedalus serve`: the local engine. The browser workbench hands its simulations to this
//! process on the user's own machine instead of running them in WebAssembly: native code,
//! one thread per run (sweeps use every core) and no 4 GB memory ceiling.
//!
//! The engine runs the workbench's own [`Workspace`] natively, so a run gives what the
//! browser would give. It mirrors the browser worker's sources: the worker sends every
//! document and the library bundle it loaded, and the engine replays them into each
//! workspace ("space") before it is used. Spaces are named by the caller: `main` for the
//! Simulate button, `pool-N` for the runs of an experiment, which run in parallel.
//!
//! HTTP on 127.0.0.1 only. Browsers may call it only from the allowed origins (the public
//! workbench and local development servers); requests without an `Origin` header come from
//! local programs, which can run `daedalus` directly anyway.
//!
//! - `GET /status` → `{"engine": "daedalus", "version", "cores"}`
//! - `POST /reset` → forget all sources and spaces (a worker connecting afresh)
//! - `POST /sources` `{"set": [[uri, text], ...], "remove": [uri, ...]}`
//! - `POST /bundle?prefix=P` `{"files": [[rel, text], ...]}` (a library bundle)
//! - `POST /call/<space>` `{"op", "args": [...]}`: a `Workspace` method; JSON or text back,
//!   except `values` (binary, little-endian: `u64` row count, column count and each
//!   column's length, then time and the columns as `f64`) and `resultMat` (bytes).

use daedalus_wasm::Workspace;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tiny_http::{Header, Method, Request, Response, Server};

const DEFAULT_PORT: u16 = 7878;
const DEFAULT_ORIGINS: &[&str] = &["https://daedalus.apps.ezkeel.com"];

enum SrcOp {
    Set(String, String),
    Remove(String),
    Bundle(String, Arc<Vec<(String, String)>>),
}

struct Space {
    ws: Workspace,
    /// Entries of the source log applied to `ws`.
    applied: usize,
    /// Generation of the source log `applied` counts in (a reset starts a new one).
    generation: u64,
}

// SAFETY: a Workspace owns everything it points to (a live run's model is a Box it frees
// itself, and the integrators hold plain numeric state); it is only `!Send` because the
// live run keeps that Box as a raw pointer and the integrator as a trait object without a
// `Send` bound. Each Space is only ever touched under its own Mutex.
unsafe impl Send for Space {}

#[derive(Default)]
struct Engine {
    log: Mutex<(u64, Vec<Arc<SrcOp>>)>,
    spaces: Mutex<HashMap<String, Arc<Mutex<Space>>>>,
}

impl Engine {
    fn push(&self, ops: Vec<SrcOp>) {
        let mut log = self.log.lock().unwrap();
        log.1.extend(ops.into_iter().map(Arc::new));
    }

    fn reset(&self) {
        let mut log = self.log.lock().unwrap();
        log.0 += 1;
        log.1.clear();
        self.spaces.lock().unwrap().clear();
    }

    fn space(&self, name: &str) -> Arc<Mutex<Space>> {
        self.spaces
            .lock()
            .unwrap()
            .entry(name.to_string())
            .or_insert_with(|| {
                Arc::new(Mutex::new(Space {
                    ws: Workspace::new(),
                    applied: 0,
                    generation: 0,
                }))
            })
            .clone()
    }

    /// Bring a space up to date with the source log.
    fn sync(&self, sp: &mut Space) {
        let (generation, pending): (u64, Vec<Arc<SrcOp>>) = {
            let log = self.log.lock().unwrap();
            if log.0 != sp.generation {
                sp.ws = Workspace::new();
                sp.applied = 0;
                sp.generation = log.0;
            }
            (log.0, log.1[sp.applied.min(log.1.len())..].to_vec())
        };
        for op in &pending {
            match &**op {
                SrcOp::Set(uri, text) => {
                    sp.ws.set_document(uri, text);
                }
                SrcOp::Remove(uri) => sp.ws.remove_document(uri),
                SrcOp::Bundle(prefix, files) => {
                    for (rel, text) in files.iter() {
                        sp.ws.add_library_file(prefix, rel, text);
                    }
                }
            }
        }
        sp.applied += pending.len();
        sp.generation = generation;
    }
}

enum Reply {
    Json(String),
    Text(String),
    Bytes(Vec<u8>, Vec<(String, String)>),
}

fn arg<'a>(args: &'a [Value], i: usize) -> &'a str {
    args.get(i).and_then(Value::as_str).unwrap_or("")
}

/// Options arguments arrive as JSON values; the workspace takes them as JSON text.
fn json_arg(args: &[Value], i: usize) -> String {
    match args.get(i) {
        Some(Value::String(s)) => s.clone(),
        Some(v) if !v.is_null() => v.to_string(),
        _ => "{}".into(),
    }
}

fn call(ws: &mut Workspace, op: &str, args: &[Value]) -> Result<Reply, String> {
    Ok(match op {
        "simulate" => Reply::Json(ws.simulate(arg(args, 0), &json_arg(args, 1))),
        "simulateRun" => Reply::Json(ws.simulate_run(arg(args, 0), &json_arg(args, 1))),
        "structure" => Reply::Json(ws.structure(arg(args, 0), &json_arg(args, 1))),
        "check" => Reply::Json(ws.check(arg(args, 0))),
        "profile" => Reply::Json(ws.profile()),
        "resultCsv" => Reply::Text(ws.result_csv(arg(args, 0))),
        "resultMat" => Reply::Bytes(ws.result_mat(), Vec::new()),
        "values" => {
            let names: Vec<&str> = match args.first() {
                Some(Value::Array(a)) => a.iter().filter_map(Value::as_str).collect(),
                _ => Vec::new(),
            };
            let time = ws.time();
            let cols: Vec<Vec<f64>> = names.iter().map(|n| ws.values(n)).collect();
            // Header of u64 counts (rows, columns, then each column's length: a missing
            // variable gives an empty column, as in the browser), then the f64 data.
            let mut out = Vec::new();
            out.extend_from_slice(&(time.len() as u64).to_le_bytes());
            out.extend_from_slice(&(cols.len() as u64).to_le_bytes());
            for c in &cols {
                out.extend_from_slice(&(c.len() as u64).to_le_bytes());
            }
            for x in time.iter().chain(cols.iter().flatten()) {
                out.extend_from_slice(&x.to_le_bytes());
            }
            Reply::Bytes(out, Vec::new())
        }
        "analysisStart" => Reply::Json(ws.analysis_start(
            arg(args, 0),
            arg(args, 1),
            &json_arg(args, 2),
            &json_arg(args, 3),
        )),
        "analysisStep" => Reply::Json(
            ws.analysis_step(args.first().and_then(Value::as_f64).unwrap_or(120.0)),
        ),
        "analysisResult" => Reply::Json(ws.analysis_result()),
        "analysisCancel" => {
            ws.analysis_cancel();
            Reply::Json("null".into())
        }
        _ => return Err(format!("unknown op {op}")),
    })
}

fn header(name: &str, value: &str) -> Header {
    Header::from_bytes(name.as_bytes(), value.as_bytes()).expect("ASCII header")
}

fn allowed(origin: &str, extra: &[String]) -> bool {
    let local = ["http://localhost", "http://127.0.0.1", "http://[::1]"]
        .iter()
        .any(|p| origin == *p || origin.starts_with(&format!("{p}:")));
    local || DEFAULT_ORIGINS.contains(&origin) || extra.iter().any(|o| o == origin)
}

fn handle(engine: &Engine, mut req: Request, origins: &[String]) {
    let origin = req
        .headers()
        .iter()
        .find(|h| h.field.equiv("Origin"))
        .map(|h| h.value.as_str().to_string());
    if let Some(o) = &origin
        && !allowed(o, origins)
    {
        let _ = req.respond(Response::from_string("origin not allowed").with_status_code(403));
        return;
    }
    let cors = |mut r: Response<std::io::Cursor<Vec<u8>>>| {
        if let Some(o) = &origin {
            r.add_header(header("Access-Control-Allow-Origin", o));
            r.add_header(header("Vary", "Origin"));
        }
        r
    };
    if *req.method() == Method::Options {
        // CORS preflight, including Chrome's private network access check (a public
        // https page calling 127.0.0.1).
        let mut r = cors(Response::from_data(Vec::new()).with_status_code(204));
        r.add_header(header("Access-Control-Allow-Methods", "GET, POST, OPTIONS"));
        r.add_header(header("Access-Control-Allow-Headers", "Content-Type"));
        r.add_header(header("Access-Control-Allow-Private-Network", "true"));
        r.add_header(header("Access-Control-Max-Age", "600"));
        let _ = req.respond(r);
        return;
    }
    let url = req.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let mut body = Vec::new();
    if let Err(e) = req.as_reader().read_to_end(&mut body) {
        let _ = req.respond(cors(Response::from_string(e.to_string()).with_status_code(400)));
        return;
    }
    let result: Result<Reply, String> = (|| {
        let json = || -> Result<Value, String> {
            serde_json::from_slice(&body).map_err(|e| format!("bad JSON: {e}"))
        };
        match (req.method(), path) {
            (Method::Get, "/status") => Ok(Reply::Json(
                serde_json::json!({
                    "engine": "daedalus",
                    "version": env!("CARGO_PKG_VERSION"),
                    "cores": std::thread::available_parallelism().map_or(1, |n| n.get()),
                })
                .to_string(),
            )),
            (Method::Post, "/reset") => {
                engine.reset();
                Ok(Reply::Json("null".into()))
            }
            (Method::Post, "/sources") => {
                let v = json()?;
                let mut ops = Vec::new();
                for uri in v["remove"].as_array().into_iter().flatten() {
                    ops.push(SrcOp::Remove(uri.as_str().unwrap_or("").to_string()));
                }
                for d in v["set"].as_array().into_iter().flatten() {
                    if let (Some(u), Some(t)) = (d[0].as_str(), d[1].as_str()) {
                        ops.push(SrcOp::Set(u.to_string(), t.to_string()));
                    }
                }
                engine.push(ops);
                Ok(Reply::Json("null".into()))
            }
            (Method::Post, "/bundle") => {
                let prefix = query
                    .split('&')
                    .find_map(|kv| kv.strip_prefix("prefix="))
                    .map(|p| p.replace("%2F", "/"))
                    .unwrap_or_default();
                let v = json()?;
                let files: Vec<(String, String)> = v["files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|f| Some((f[0].as_str()?.to_string(), f[1].as_str()?.to_string())))
                    .collect();
                let n = files.len();
                engine.push(vec![SrcOp::Bundle(prefix, Arc::new(files))]);
                Ok(Reply::Json(serde_json::json!({"files": n}).to_string()))
            }
            (Method::Post, p) if p.starts_with("/call/") => {
                let v = json()?;
                let op = v["op"].as_str().unwrap_or("");
                let args = v["args"].as_array().cloned().unwrap_or_default();
                let sp = engine.space(&p["/call/".len()..]);
                let mut sp = sp.lock().unwrap_or_else(|e| e.into_inner());
                engine.sync(&mut sp);
                call(&mut sp.ws, op, &args)
            }
            _ => Err(format!("no route {path}")),
        }
    })();
    let resp = match result {
        Ok(Reply::Json(s)) => Response::from_string(s).with_header(header("Content-Type", "application/json")),
        Ok(Reply::Text(s)) => Response::from_string(s).with_header(header("Content-Type", "text/plain")),
        Ok(Reply::Bytes(b, hs)) => {
            let mut r = Response::from_data(b).with_header(header("Content-Type", "application/octet-stream"));
            for (k, v) in hs {
                r.add_header(header(&k, &v));
            }
            r
        }
        Err(e) => Response::from_string(e).with_status_code(400),
    };
    let _ = req.respond(cors(resp));
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let mut port = DEFAULT_PORT;
    let mut origins = Vec::new();
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                i += 1;
                port = args.get(i).and_then(|p| p.parse().ok()).ok_or_else(|| anyhow::anyhow!("--port needs a number"))?;
            }
            "--allow-origin" => {
                i += 1;
                origins.push(args.get(i).cloned().ok_or_else(|| anyhow::anyhow!("--allow-origin needs an origin"))?);
            }
            a => anyhow::bail!("unknown option {a} (serve takes --port N and --allow-origin ORIGIN)"),
        }
        i += 1;
    }
    let server = Server::http(("127.0.0.1", port)).map_err(|e| anyhow::anyhow!("cannot listen on 127.0.0.1:{port}: {e}"))?;
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    eprintln!(
        "DAEdalus local engine on http://127.0.0.1:{port} ({cores} cores). In the workbench, \
         turn on \"Local engine\" to simulate here. Ctrl+C stops it."
    );
    let engine = Arc::new(Engine::default());
    let origins = Arc::new(origins);
    for req in server.incoming_requests() {
        let engine = engine.clone();
        let origins = origins.clone();
        std::thread::spawn(move || handle(&engine, req, &origins));
    }
    Ok(())
}
