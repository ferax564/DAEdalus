//! `daedalus` command line interface.

use daedalus_core::backend::Options;
use daedalus_core::frontend::Library;
use daedalus_core::sim::{Method, SimOptions};
mod fmu;
mod manage;
mod mos;

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
daedalus - fast Modelica-language modeling and simulation

USAGE:
  daedalus <command> [sources...] -m <Model> [options]

COMMANDS:
  check       translate the model and print statistics
  flatten     print the flat model
  dump        print the sorted, causalised equation system
  simulate    simulate and write results
  export-casadi  write the index-reduced DAE as JSON for CasADi (optimal
              control, MPC, estimation; see python/daedalus_casadi)
  linearize   linearize around the initial point (or --at T): the state-space
              matrices A, B, C, D of der(x) = A x + B u, y = C x + D u for the
              states x, top-level inputs u and outputs y
  steady      find a steady state (der(x) = 0) from the initial state, or from
              the state reached at --at-time; with --free/--target, trim: also
              solve for inputs or parameters so that variables hit targets
  run         run a Dymola / OpenModelica script: daedalus run script.mos
              [--msl DIR] (see docs/scripting.md)
  fmu         export an FMI 2.0 FMU (Model Exchange and Co-Simulation) to -o
              (default <Model>.fmu); needs the runtime libdaedalus_fmi.so
  test-library  translate and simulate every model with an experiment
                annotation under --model (a package), each in its own
                process with a time limit; prints a report
{MANAGE}
SOURCES:
  .mo files or directories (loaded recursively), or model packages (.zip). The
  bundled Daedalus library is found via --lib or the DAEDALUS_LIB environment
  variable. Libraries named in uses(...) annotations are taken from the
  installed libraries (daedalus lib install; $DAEDALUS_LIBRARIES or
  ~/.daedalus/libraries) unless given with -L.

OPTIONS:
  -m, --model NAME        model to translate (full dotted name)
  -L, --lib DIR           additional library directory (repeatable)
  --start T  --stop T     simulation interval
  --interval DT           output interval
  -n, --intervals N       number of output intervals (default 500)
  --tol TOL               relative tolerance (default 1e-6)
  --method M              bdf | dopri5 | rk4 | euler
  --step H                fixed step size
  --store-events E        rows at events: all | none | N (default: about
                          50 million values, at least the output points)
  -p, --param NAME=VALUE  parameter override (repeatable)
  -o, --output FILE       result file (.mat Dymola format, or .csv)
  --vars a,b,c            variables written to CSV / printed
  --no-index-reduction    disable Pantelides index reduction
  --no-tearing            disable tearing
  --jobs N                parallel processes for test-library (default: cores)
  --at T                  linearize: simulate to time T first (default: start)
  --format F              linearize: text | json | mo (default text; mo writes
                          model linearized_model, to linearized_model.mo
                          unless -o is given); steady: text | json
  --jacobian J            linearize: exact | fd (central differences)
  --at-time T             steady: simulate to time T first (default: start)
  --free a,b              steady: inputs or parameters to solve for (trim)
  --target y=v            steady: target value of a variable (repeatable or
                          comma-separated; as many targets as free variables)
  --fmi-lib FILE          fmu: the FMI runtime library (default: next to this
                          executable, or $DAEDALUS_FMI_LIB)
  --timeout S             per-model time limit for test-library (default 60)
";

fn usage() -> String {
    USAGE.replace("{MANAGE}", manage::USAGE)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    type Sub = fn(&[String]) -> anyhow::Result<ExitCode>;
    let sub: Option<Sub> = match args.first().map(String::as_str) {
        Some("diff") => Some(manage::diff),
        Some("params-from") => Some(manage::params_from),
        Some("lib") => Some(manage::lib),
        Some("package") => Some(manage::package),
        _ => None,
    };
    if let Some(f) = sub {
        return match f(&args[1..]) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e:#}");
                ExitCode::FAILURE
            }
        };
    }
    if args.first().is_some_and(|a| a == "run") {
        return match mos::run_cli(&args[1..]) {
            Ok(code) => code,
            Err(e) => {
                eprintln!("error: {e:#}");
                ExitCode::FAILURE
            }
        };
    }
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        print!("{}", usage());
        return Ok(());
    }
    let cmd = args[0].clone();
    let mut sources: Vec<String> = Vec::new();
    let mut libs: Vec<String> = Vec::new();
    let mut model = None;
    let mut so = SimOptions::default();
    let mut set_start = false;
    let mut set_stop = false;
    let mut set_tol = false;
    let mut set_intervals = false;
    let mut set_method = false;
    let mut output = None;
    let mut vars: Option<Vec<String>> = None;
    let mut opts = Options::default();
    let mut jobs = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut timeout = 60.0f64;
    let mut lin_at: Option<f64> = None;
    let mut format = String::from("text");
    let mut fmi_lib: Option<std::path::PathBuf> = None;
    let mut jac_method = daedalus_core::linearize::JacobianMethod::Exact;
    let mut free: Vec<String> = Vec::new();
    let mut targets: Vec<(String, f64)> = Vec::new();
    let mut i = 1;
    let next = |i: &mut usize| -> anyhow::Result<String> {
        *i += 1;
        args.get(*i)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing value for {}", args[*i - 1]))
    };
    while i < args.len() {
        let a = args[i].as_str();
        match a {
            "-m" | "--model" => model = Some(next(&mut i)?),
            "-L" | "--lib" => libs.push(next(&mut i)?),
            "--start" => {
                so.start_time = next(&mut i)?.parse()?;
                set_start = true
            }
            "--stop" => {
                so.stop_time = next(&mut i)?.parse()?;
                set_stop = true
            }
            "--interval" => so.interval = next(&mut i)?.parse()?,
            "-n" | "--intervals" => {
                so.intervals = next(&mut i)?.parse()?;
                set_intervals = true
            }
            "--tol" => {
                so.tolerance = next(&mut i)?.parse()?;
                set_tol = true
            }
            "--method" => {
                let m = next(&mut i)?;
                so.method =
                    Method::parse(&m).ok_or_else(|| anyhow::anyhow!("unknown method '{m}'"))?;
                set_method = true;
            }
            "--store-events" => match next(&mut i)?.as_str() {
                "all" => so.max_event_rows = usize::MAX,
                "none" => so.store_events = false,
                n => so.max_event_rows = n.parse()?,
            },
            "--step" => so.fixed_step = next(&mut i)?.parse()?,
            "-p" | "--param" => {
                let kv = next(&mut i)?;
                let (k, v) = kv
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("expected NAME=VALUE"))?;
                so.params.push((k.to_string(), v.parse()?));
            }
            "-o" | "--output" => output = Some(next(&mut i)?),
            "--vars" => {
                vars = Some(
                    next(&mut i)?
                        .split(',')
                        .map(|s| s.trim().to_string())
                        .collect(),
                )
            }
            "--no-index-reduction" => opts.index_reduction = false,
            "--no-tearing" => opts.tearing = false,
            "--jobs" => jobs = next(&mut i)?.parse::<usize>()?.max(1),
            "--timeout" => timeout = next(&mut i)?.parse()?,
            "--fmi-lib" => fmi_lib = Some(next(&mut i)?.into()),
            "--at" | "--at-time" => lin_at = Some(next(&mut i)?.parse()?),
            "--free" => free.extend(
                next(&mut i)?
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty()),
            ),
            "--target" => {
                for kv in next(&mut i)?.split(',') {
                    let (k, v) = kv
                        .split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("expected --target NAME=VALUE"))?;
                    targets.push((k.trim().to_string(), v.trim().parse()?));
                }
            }
            "--format" => format = next(&mut i)?,
            "--jacobian" => {
                jac_method = match next(&mut i)?.as_str() {
                    "exact" => daedalus_core::linearize::JacobianMethod::Exact,
                    "fd" | "cd" | "central" => {
                        daedalus_core::linearize::JacobianMethod::CentralDifference
                    }
                    j => anyhow::bail!("unknown jacobian method '{j}' (exact | fd)"),
                }
            }
            _ if a.starts_with('-') => anyhow::bail!("unknown option {a}\n\n{}", usage()),
            _ => sources.push(a.to_string()),
        }
        i += 1;
    }
    // Model packages: extracted, and their model is the default
    let mut opened = Vec::new();
    for s in std::mem::take(&mut sources) {
        if s.ends_with(".zip") {
            let (paths, m) = manage::open_zip(&s)?;
            opened.extend(paths);
            if model.is_none() {
                model = m;
            }
        } else {
            opened.push(s);
        }
    }
    sources = opened;
    let model = model.ok_or_else(|| anyhow::anyhow!("no model given (-m)"))?;
    let mut lib = Library::new();
    if let Ok(dir) = std::env::var("DAEDALUS_LIB") {
        libs.push(dir);
    }
    let t0 = Instant::now();
    // One model needs only part of a library: parse just the files it can reach
    // (test-library lists every model, so it parses everything).
    let lazy = cmd != "test-library";
    for s in libs.iter().chain(sources.iter()) {
        let p = Path::new(s);
        if p.is_dir() {
            if lazy {
                lib.add_dir_lazy(p)?;
            } else {
                lib.load_dir(p)?;
                for d in &lib.docs {
                    for e in d.errors.iter().take(3) {
                        eprintln!("{e}");
                    }
                }
            }
        } else {
            let text = std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{s}: {e}"))?;
            for e in lib.set_document(s, &text) {
                eprintln!("{e}");
            }
        }
    }
    manage::auto_uses(&mut lib);
    if lazy && lib.has_pending() {
        let before = lib.docs.len();
        lib.load_for(&model);
        if std::env::var_os("DAEDALUS_DEBUG_LAZY").is_some() {
            let bytes: usize = lib.docs[before..].iter().map(|d| d.text.len()).sum();
            eprintln!(
                "parsed {} files, {} kB",
                lib.docs.len() - before,
                bytes / 1000
            );
            for d in &lib.docs[before..] {
                eprintln!("  {} {}", d.text.len() / 1000, d.uri);
            }
        }
        for d in &lib.docs[before..] {
            for e in d.errors.iter().take(3) {
                eprintln!("{e}");
            }
        }
    }
    let t_parse = t0.elapsed();
    match cmd.as_str() {
        "flatten" => {
            let f = with_all_files(&mut lib, |l| l.flatten(&model))?;
            print!("{}", f.dump());
            Ok(())
        }
        "dump" | "check" => {
            let t1 = Instant::now();
            let flat = with_all_files(&mut lib, |l| l.flatten(&model))?;
            let t_flat = t1.elapsed();
            let sys = daedalus_core::backend::compile(flat, &opts)?;
            for w in &sys.warnings {
                eprintln!("warning: {w}");
            }
            if cmd == "dump" {
                print!("{}", sys.dump());
            } else {
                let st = &sys.stats;
                println!("model {}", sys.name);
                println!(
                    "  parse {:.1} ms, flatten {:.1} ms, backend {:.1} ms",
                    ms(t_parse),
                    ms(t_flat),
                    st.backend_ms
                );
                println!(
                    "  flat: {} variables, {} equations",
                    st.flat_variables, st.flat_equations
                );
                println!("  aliases removed: {}", st.aliases_removed);
                println!(
                    "  states: {} ({} dummy derivatives)",
                    st.states, st.dummy_derivatives
                );
                println!(
                    "  differentiated equations: {}",
                    st.differentiated_equations
                );
                println!(
                    "  blocks: {}, algebraic loops: {} (largest {}, {} tearing vars, {} linear)",
                    st.blocks,
                    st.algebraic_loops,
                    st.largest_loop,
                    st.torn_loop_vars,
                    st.linear_loops
                );
                println!(
                    "  initialization: {} blocks, {} loops",
                    st.init_blocks, st.init_loops
                );
                println!(
                    "  events: {} relations, {} when-clauses",
                    st.relations, st.when_clauses
                );
                let m = daedalus_core::codegen::compile(sys);
                println!(
                    "  tape: {} instructions, {} registers",
                    m.tape_len, m.layout.n_regs
                );
            }
            Ok(())
        }
        "simulate" => {
            let t1 = Instant::now();
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            let t_tr = t1.elapsed();
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            let mut o = SimOptions::from_model(&m);
            if set_start {
                o.start_time = so.start_time;
            }
            if set_stop {
                o.stop_time = so.stop_time;
            }
            if set_tol {
                o.tolerance = so.tolerance;
            }
            if set_method {
                o.method = so.method;
            }
            if so.interval > 0.0 {
                o.interval = so.interval;
            } else if set_intervals {
                // An explicit interval count replaces the experiment's Interval.
                o.interval = 0.0;
            }
            o.intervals = so.intervals;
            o.fixed_step = so.fixed_step;
            o.store_events = so.store_events;
            o.max_event_rows = so.max_event_rows;
            o.params = so.params.clone();
            let (res, stats) = match daedalus_core::sim::simulate(&m, o) {
                Ok(r) => r,
                Err(e) => {
                    if let (Some(out), Some(p)) = (&output, &e.partial) {
                        write_result(p, out, vars.as_deref())?;
                        eprintln!("partial results written to {out}");
                    }
                    anyhow::bail!(e.message);
                }
            };
            for l in &res.log {
                eprintln!("{l}");
            }
            eprintln!(
                "translated in {:.1} ms; simulated in {:.1} ms ({} steps, {} rejected ({} Newton), {} f-evals, {} jacobians, {} events)",
                ms(t_tr) + ms(t_parse),
                stats.init_ms + stats.sim_ms,
                stats.solver.steps,
                stats.solver.rejected,
                stats.solver.newton_failures,
                stats.solver.rhs_evals,
                stats.solver.jac_evals,
                stats.events
            );
            match &output {
                Some(out) => write_result(&res, out, vars.as_deref())?,
                None => {
                    let names: Vec<String> = match &vars {
                        Some(v) => v.clone(),
                        None => m
                            .states
                            .iter()
                            .map(|&s| m.vars[s as usize].name.clone())
                            .collect(),
                    };
                    for n in names {
                        match res.final_value(&n) {
                            Some(v) => println!("{n} = {v}"),
                            None => eprintln!("warning: no variable '{n}' in results"),
                        }
                    }
                }
            }
            Ok(())
        }
        "export-casadi" => {
            let flat = with_all_files(&mut lib, |l| l.flatten(&model))?;
            let sys = daedalus_core::backend::compile(flat, &opts)?;
            let o = SimOptions {
                start_time: if set_start {
                    so.start_time
                } else {
                    sys.experiment.start_time.unwrap_or(0.0)
                },
                params: so.params.clone(),
                ..Default::default()
            };
            let doc = daedalus_core::casadi::export(sys, &o)?;
            if let Some(w) = doc["warnings"].as_array() {
                for w in w {
                    eprintln!("warning: {}", w.as_str().unwrap_or_default());
                }
            }
            if let Some(e) = doc["init_error"].as_str() {
                eprintln!("warning: initialization failed ({e}); values are start values");
            }
            let text = doc.to_string();
            match &output {
                Some(out) => std::fs::write(out, text)?,
                None => println!("{text}"),
            }
            eprintln!(
                "exported {}: {} states, {} algebraic, {} inputs, {} parameters",
                model,
                doc["states"].as_array().map_or(0, |a| a.len()),
                doc["algebraics"].as_array().map_or(0, |a| a.len()),
                doc["inputs"].as_array().map_or(0, |a| a.len()),
                doc["parameters"].as_array().map_or(0, |a| a.len()),
            );
            Ok(())
        }
        "linearize" => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            let mut o = SimOptions::from_model(&m);
            if set_start {
                o.start_time = so.start_time;
            }
            if set_tol {
                o.tolerance = so.tolerance;
            }
            if set_method {
                o.method = so.method;
            }
            o.params = so.params.clone();
            let lin = daedalus_core::linearize::linearize(&m, o, lin_at, jac_method)
                .map_err(|e| anyhow::anyhow!(e))?;
            let text = match format.as_str() {
                "text" => lin.report(),
                "json" => {
                    let mut v = serde_json::to_value(&lin)?;
                    v["eigenvalues"] = serde_json::to_value(lin.eigenvalues())?;
                    serde_json::to_string_pretty(&v)? + "\n"
                }
                "mo" | "modelica" => lin.to_modelica(),
                f => anyhow::bail!("unknown format '{f}' (text | json | mo)"),
            };
            let out = output.clone().or_else(|| {
                matches!(format.as_str(), "mo" | "modelica")
                    .then(|| "linearized_model.mo".to_string())
            });
            match out {
                Some(path) => {
                    std::fs::write(&path, text)?;
                    eprintln!("wrote {path}");
                }
                None => print!("{text}"),
            }
            Ok(())
        }
        "steady" => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            let mut o = SimOptions::from_model(&m);
            if set_start {
                o.start_time = so.start_time;
            }
            if set_tol {
                o.tolerance = so.tolerance;
            }
            if set_method {
                o.method = so.method;
            }
            o.params = so.params.clone();
            let sopts = daedalus_core::steady::SteadyOptions {
                at_time: lin_at,
                free: free.clone(),
                targets: targets.clone(),
                ..Default::default()
            };
            let st = daedalus_core::steady::steady_state(&m, o, &sopts)
                .map_err(|e| anyhow::anyhow!(e))?;
            let text = match format.as_str() {
                "text" => st.report(),
                "json" => serde_json::to_string_pretty(&st)? + "\n",
                f => anyhow::bail!("unknown format '{f}' (text | json)"),
            };
            match &output {
                Some(path) => {
                    std::fs::write(path, text)?;
                    eprintln!("wrote {path}");
                }
                None => print!("{text}"),
            }
            Ok(())
        }
        "fmu" => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            let out = output
                .clone()
                .unwrap_or_else(|| format!("{}.fmu", m.name.replace('.', "_")));
            fmu::export(&m, std::path::Path::new(&out), fmi_lib.as_deref())?;
            eprintln!("wrote {out}");
            Ok(())
        }
        "test-library" => {
            let mut child_args: Vec<String> = Vec::new();
            for l in &libs {
                child_args.push("-L".into());
                child_args.push(l.clone());
            }
            child_args.extend(sources.iter().cloned());
            if !opts.index_reduction {
                child_args.push("--no-index-reduction".into());
            }
            if !opts.tearing {
                child_args.push("--no-tearing".into());
            }
            test_library(&lib, &model, &child_args, jobs, timeout)
        }
        "probe" => {
            // Internal: used by test-library to isolate one model per process.
            let t = Instant::now();
            let m = match with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts)) {
                Ok(m) => m,
                Err(e) => {
                    println!(
                        "ERR translate: {}",
                        format!("{e:#}").lines().next().unwrap_or("")
                    );
                    return Ok(());
                }
            };
            let mut o = SimOptions::from_model(&m);
            o.max_wall_time = timeout * 0.8;
            match daedalus_core::sim::simulate(&m, o) {
                Ok((_, st)) => println!(
                    "OK {} states, {} steps, {:.0} ms",
                    m.states.len(),
                    st.solver.steps,
                    t.elapsed().as_secs_f64() * 1000.0
                ),
                Err(e) => println!("ERR simulate: {}", e.message.lines().next().unwrap_or("")),
            }
            Ok(())
        }
        _ => anyhow::bail!("unknown command '{cmd}'\n\n{USAGE}"),
    }
}

fn write_result(
    res: &daedalus_core::results::Results,
    out: &str,
    vars: Option<&[String]>,
) -> anyhow::Result<()> {
    let f = std::fs::File::create(out)?;
    let mut w = std::io::BufWriter::new(f);
    if out.ends_with(".csv") {
        res.write_csv(&mut w, vars)?;
    } else {
        res.write_mat(&mut w)?;
    }
    Ok(())
}

fn ms(d: std::time::Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn has_experiment(def: &rumoca_ir_ast::ClassDef) -> bool {
    def.annotation.iter().any(|a| match a {
        rumoca_ir_ast::Expression::ClassModification { target, .. } => {
            target.parts.len() == 1 && &*target.parts[0].ident.text == "experiment"
        }
        _ => false,
    })
}

fn test_library(
    lib: &Library,
    prefix: &str,
    child_args: &[String],
    jobs: usize,
    timeout: f64,
) -> anyhow::Result<()> {
    use std::io::Read;
    use std::process::{Command, Stdio};
    use std::sync::{Arc, Mutex};
    let tree = lib.tree();
    let mut models = Vec::new();
    tree.walk(&mut |c| {
        let node = tree.node(c);
        if (node.path == prefix || node.path.starts_with(&format!("{prefix}.")))
            && let Some(def) = node.def
            && !def.partial
            && matches!(
                def.class_type,
                rumoca_core::ClassType::Model | rumoca_core::ClassType::Block
            )
            && has_experiment(def)
        {
            models.push(node.path.clone());
        }
    });
    drop(tree);
    let exe = std::env::current_exe()?;
    let queue = Arc::new(Mutex::new(
        models
            .iter()
            .cloned()
            .enumerate()
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>(),
    ));
    let results: Arc<Mutex<Vec<(usize, String, Result<String, String>)>>> = Arc::default();
    let t_all = Instant::now();
    let workers: Vec<_> = (0..jobs.min(models.len()).max(1))
        .map(|_| {
            let (queue, results, exe) = (queue.clone(), results.clone(), exe.clone());
            let args = child_args.to_vec();
            std::thread::spawn(move || {
                loop {
                    let Some((idx, name)) = queue.lock().unwrap().pop() else {
                        break;
                    };
                    let t0 = Instant::now();
                    let child = Command::new(&exe)
                        .arg("probe")
                        .args(&args)
                        .args(["-m", &name, "--timeout", &timeout.to_string()])
                        .stdout(Stdio::piped())
                        .stderr(Stdio::null())
                        .spawn();
                    let r = match child {
                        Err(e) => Err(format!("spawn: {e}")),
                        Ok(mut ch) => loop {
                            match ch.try_wait() {
                                Ok(Some(status)) => {
                                    let mut out = String::new();
                                    if let Some(mut so) = ch.stdout.take() {
                                        let _ = so.read_to_string(&mut out);
                                    }
                                    let line = out.lines().last().unwrap_or("").to_string();
                                    break if let Some(rest) = line.strip_prefix("OK ") {
                                        Ok(rest.to_string())
                                    } else if let Some(rest) = line.strip_prefix("ERR ") {
                                        Err(rest.to_string())
                                    } else {
                                        Err(format!("crash ({status})"))
                                    };
                                }
                                Ok(None) if t0.elapsed().as_secs_f64() > timeout => {
                                    let _ = ch.kill();
                                    let _ = ch.wait();
                                    break Err(format!("timeout after {timeout} s"));
                                }
                                Ok(None) => {
                                    std::thread::sleep(std::time::Duration::from_millis(20))
                                }
                                Err(e) => break Err(format!("wait: {e}")),
                            }
                        },
                    };
                    match &r {
                        Ok(info) => println!("PASS {name} ({info})"),
                        Err(e) => println!("FAIL {name}: {e}"),
                    }
                    results.lock().unwrap().push((idx, name, r));
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    let results = std::mem::take(&mut *results.lock().unwrap());
    let ok = results.iter().filter(|r| r.2.is_ok()).count();
    let mut failures: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    for (_, name, r) in &results {
        if let Err(e) = r {
            failures
                .entry(categorize(e))
                .or_default()
                .push(name.clone());
        }
    }
    println!(
        "\n{ok}/{} passed ({:.1}%) in {:.1} s",
        models.len(),
        100.0 * ok as f64 / models.len().max(1) as f64,
        t_all.elapsed().as_secs_f64()
    );
    let mut cats: Vec<_> = failures.into_iter().collect();
    cats.sort_by_key(|(_, v)| std::cmp::Reverse(v.len()));
    for (c, v) in cats {
        println!("{:4}  {c}", v.len());
    }
    Ok(())
}

/// Group error messages by stripping names and numbers.
fn categorize(msg: &str) -> String {
    let mut out = String::new();
    let mut in_quote = false;
    for ch in msg.chars() {
        if ch == '\'' {
            in_quote = !in_quote;
            if in_quote {
                out.push_str("'…");
            } else {
                out.push('\'');
            }
            continue;
        }
        if in_quote {
            continue;
        }
        if ch.is_ascii_digit() {
            if !out.ends_with('#') {
                out.push('#');
            }
            continue;
        }
        out.push(ch);
    }
    out.chars().take(120).collect()
}

/// Run `f`; if it fails while library files are still unparsed (a class the file scan
/// could not see), parse the rest and run it again.
fn with_all_files<T>(
    lib: &mut Library,
    f: impl Fn(&Library) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    let r = f(lib);
    if r.is_err() && lib.has_pending() {
        if std::env::var_os("DAEDALUS_DEBUG_LAZY").is_some() {
            eprintln!("lazy load fell back to the whole library");
        }
        lib.load_all();
        return f(lib);
    }
    r
}
