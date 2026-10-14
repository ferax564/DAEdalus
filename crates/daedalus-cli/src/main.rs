//! `daedalus` command line interface.

use daedalus_core::backend::Options;
use daedalus_core::frontend::Library;
use daedalus_core::sim::{Method, SimOptions};
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
  test-library  translate and simulate every model with an experiment
                annotation under --model (a package); prints a report

SOURCES:
  .mo files or directories (loaded recursively). The bundled Daedalus library
  is found via --lib or the DAEDALUS_LIB environment variable.

OPTIONS:
  -m, --model NAME        model to translate (full dotted name)
  -L, --lib DIR           additional library directory (repeatable)
  --start T  --stop T     simulation interval
  --interval DT           output interval
  -n, --intervals N       number of output intervals (default 500)
  --tol TOL               relative tolerance (default 1e-6)
  --method M              bdf | dopri5 | rk4 | euler
  --step H                fixed step size
  -p, --param NAME=VALUE  parameter override (repeatable)
  -o, --output FILE       result file (.mat Dymola format, or .csv)
  --vars a,b,c            variables written to CSV / printed
  --no-index-reduction    disable Pantelides index reduction
  --no-tearing            disable tearing
";

fn main() -> ExitCode {
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
        print!("{USAGE}");
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
    let mut set_method = false;
    let mut output = None;
    let mut vars: Option<Vec<String>> = None;
    let mut opts = Options::default();
    let mut i = 1;
    let next = |i: &mut usize| -> anyhow::Result<String> {
        *i += 1;
        args.get(*i).cloned().ok_or_else(|| anyhow::anyhow!("missing value for {}", args[*i - 1]))
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
            "-n" | "--intervals" => so.intervals = next(&mut i)?.parse()?,
            "--tol" => {
                so.tolerance = next(&mut i)?.parse()?;
                set_tol = true
            }
            "--method" => {
                let m = next(&mut i)?;
                so.method = Method::parse(&m).ok_or_else(|| anyhow::anyhow!("unknown method '{m}'"))?;
                set_method = true;
            }
            "--step" => so.fixed_step = next(&mut i)?.parse()?,
            "-p" | "--param" => {
                let kv = next(&mut i)?;
                let (k, v) = kv.split_once('=').ok_or_else(|| anyhow::anyhow!("expected NAME=VALUE"))?;
                so.params.push((k.to_string(), v.parse()?));
            }
            "-o" | "--output" => output = Some(next(&mut i)?),
            "--vars" => vars = Some(next(&mut i)?.split(',').map(|s| s.trim().to_string()).collect()),
            "--no-index-reduction" => opts.index_reduction = false,
            "--no-tearing" => opts.tearing = false,
            _ if a.starts_with('-') => anyhow::bail!("unknown option {a}\n\n{USAGE}"),
            _ => sources.push(a.to_string()),
        }
        i += 1;
    }
    let model = model.ok_or_else(|| anyhow::anyhow!("no model given (-m)"))?;
    let mut lib = Library::new();
    if let Ok(dir) = std::env::var("DAEDALUS_LIB") {
        libs.push(dir);
    }
    let t0 = Instant::now();
    for s in libs.iter().chain(sources.iter()) {
        let p = Path::new(s);
        if p.is_dir() {
            lib.load_dir(p)?;
            for d in &lib.docs {
                for e in d.errors.iter().take(3) {
                    eprintln!("{e}");
                }
            }
        } else {
            let text = std::fs::read_to_string(p).map_err(|e| anyhow::anyhow!("{s}: {e}"))?;
            for e in lib.set_document(s, &text) {
                eprintln!("{e}");
            }
        }
    }
    let t_parse = t0.elapsed();
    match cmd.as_str() {
        "flatten" => {
            let f = lib.flatten(&model)?;
            print!("{}", f.dump());
            Ok(())
        }
        "dump" | "check" => {
            let t1 = Instant::now();
            let flat = lib.flatten(&model)?;
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
                println!("  parse {:.1} ms, flatten {:.1} ms, backend {:.1} ms", ms(t_parse), ms(t_flat), st.backend_ms);
                println!("  flat: {} variables, {} equations", st.flat_variables, st.flat_equations);
                println!("  aliases removed: {}", st.aliases_removed);
                println!("  states: {} ({} dummy derivatives)", st.states, st.dummy_derivatives);
                println!("  differentiated equations: {}", st.differentiated_equations);
                println!(
                    "  blocks: {}, algebraic loops: {} (largest {}, {} tearing vars, {} linear)",
                    st.blocks, st.algebraic_loops, st.largest_loop, st.torn_loop_vars, st.linear_loops
                );
                println!("  initialization: {} blocks, {} loops", st.init_blocks, st.init_loops);
                println!("  events: {} relations, {} when-clauses", st.relations, st.when_clauses);
                let m = daedalus_core::codegen::compile(sys);
                println!("  tape: {} instructions, {} registers", m.tape_len, m.layout.n_regs);
            }
            Ok(())
        }
        "simulate" => {
            let t1 = Instant::now();
            let m = daedalus_core::translate(&lib, &model, &opts)?;
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
            }
            o.intervals = so.intervals;
            o.fixed_step = so.fixed_step;
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
                        None => m.states.iter().map(|&s| m.vars[s as usize].name.clone()).collect(),
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
        "test-library" => test_library(&lib, &model, &opts),
        _ => anyhow::bail!("unknown command '{cmd}'\n\n{USAGE}"),
    }
}

fn write_result(res: &daedalus_core::results::Results, out: &str, vars: Option<&[String]>) -> anyhow::Result<()> {
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

fn test_library(lib: &Library, prefix: &str, opts: &Options) -> anyhow::Result<()> {
    let tree = lib.tree();
    let mut models = Vec::new();
    tree.walk(&mut |c| {
        let node = tree.node(c);
        if (node.path == prefix || node.path.starts_with(&format!("{prefix}.")))
            && let Some(def) = node.def
            && !def.partial
            && matches!(def.class_type, rumoca_core::ClassType::Model | rumoca_core::ClassType::Block)
            && has_experiment(def)
        {
            models.push(node.path.clone());
        }
    });
    drop(tree);
    let mut ok = 0;
    let mut failures: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let t_all = Instant::now();
    for name in &models {
        let t0 = Instant::now();
        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| -> Result<String, String> {
            let m = daedalus_core::translate(lib, name, opts).map_err(|e| format!("translate: {e:#}"))?;
            let mut o = SimOptions::from_model(&m);
            o.max_wall_time = 20.0;
            let (_, st) = daedalus_core::sim::simulate(&m, o).map_err(|e| format!("simulate: {}", e.message))?;
            Ok(format!("{} states, {} steps", m.states.len(), st.solver.steps))
        }));
        let dt = t0.elapsed().as_secs_f64() * 1000.0;
        match r {
            Ok(Ok(info)) => {
                ok += 1;
                println!("PASS {name} ({info}, {dt:.0} ms)");
            }
            Ok(Err(e)) => {
                let first = e.lines().next().unwrap_or("").to_string();
                println!("FAIL {name}: {first}");
                failures.entry(categorize(&first)).or_default().push(name.clone());
            }
            Err(_) => {
                println!("FAIL {name}: panic");
                failures.entry("panic".into()).or_default().push(name.clone());
            }
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
