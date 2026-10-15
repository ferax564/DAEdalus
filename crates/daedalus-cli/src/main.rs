//! `daedalus` command line interface.

use daedalus_core::backend::Options;
use daedalus_core::frontend::Library;
use daedalus_core::sim::{Method, SimOptions};
mod analysis;
mod embedded;
mod fmu;
mod fmu_import;
mod manage;
mod mcp;
mod mos;
mod sweep;

use std::path::Path;
use std::process::ExitCode;
use std::time::Instant;

const USAGE: &str = "\
daedalus - fast Modelica-language modeling and simulation

USAGE:
  daedalus <command> [sources...] -m <Model> [options]

COMMANDS:
  check       translate the model and print statistics (--units: also check
              units and print the unit issues as warnings)
  units       check units: print every variable's declared or inferred unit
              and the unit inconsistencies (--json for JSON)
  flatten     print the flat model
  dump        print the sorted, causalised equation system
  simulate    simulate and write results
  export-casadi  write the index-reduced DAE as JSON for CasADi (optimal
              control, MPC, estimation; see python/daedalus_casadi)
  linearize   linearize around the initial point (or --at T): the state-space
              matrices A, B, C, D of der(x) = A x + B u, y = C x + D u for the
              states x, top-level inputs u and outputs y
  linear-analysis  linearize (like linearize) and analyze chosen input ->
              output pairs: poles, transmission zeros, DC gain, Bode and
              Nyquist data with gain/phase margins, step and impulse
              responses, PID suggestions (SIMC, Ziegler-Nichols), balanced
              truncation (--reduce N) and LQR (--lqr Q,R); text summary on
              stdout, data to -o file.json | file.csv
  steady      find a steady state (der(x) = 0) from the initial state, or from
              the state reached at --at-time; with --free/--target, trim: also
              solve for inputs or parameters so that variables hit targets
  run         run a Dymola / OpenModelica script: daedalus run script.mos
              [--msl DIR] (see docs/scripting.md)
  fmu         export an FMI 2.0 FMU (or FMI 3.0 with --fmi-version 3; Model
              Exchange and Co-Simulation) to -o (default <Model>.fmu); needs the
              runtime libdaedalus_fmi.so
  import-fmu  daedalus import-fmu X.fmu [-o X.mo]: write a Modelica wrapper
              block for an FMI 2.0 / 3.0 FMU, usable as a component
              (--fmi-type me|cs, --class NAME, --within PACKAGE, --locals)
  import-ssp  daedalus import-ssp X.ssp [-o X.mo]: the SSP system as a
              Modelica model, with its FMUs unpacked next to it
  export-ssp  write a model composed of FMU components as an .ssp (-o);
              `simulate` also runs an .fmu or .ssp directly (Co-Simulation
              components step at --comm-step, default span/500)
  codegen-c   embedded C code for microcontrollers: fixed-step, allocation-free
              C99 of the model (a controller with top-level inputs and outputs)
              in -o DIR (default <prefix>/); see EMBEDDED OPTIONS
  hil         close the loop between a plant (bench) model simulated here and
              generated controller code running on the host, on an emulated
              Arduino or ESP32, or on a real board over its serial port
  mcp         serve the libraries to an AI assistant over the Model Context
              Protocol (stdio): browse, edit, check, simulate, linearize.
              `daedalus mcp [sources...] [-L DIR] [--workspace DIR]`
  test-library  translate and simulate every model with an experiment
                annotation under --model (a package), each in its own
                process with a time limit; prints a report
  sweep       simulate once per parameter combination (-s), in parallel
              threads; prints a summary table (see SWEEP OPTIONS)
  structure   model structure: variable roles, BLT blocks, incidence,
              state selection and why; with the state Jacobian at the start
              (or --at T): sparsity, condition, eigenvalues, time constants,
              participation factors, stiffness, algebraic-loop Jacobians
{MANAGE}
  debug       equation and initialization debugger: --var NAME (repeatable)
              traces a variable from its equation as written (component,
              file:line) through alias substitution, index reduction, state
              selection, solving and tearing; --equation N does the same for
              equation N of `structure`; --init (the default) reports the
              initial system, the start values used (fixed, defaulted or guess,
              and the modifier that set them) and, on failure, the failing
              block's residuals and largest Newton updates; a structurally
              singular model gets its over- and underdetermined subsets;
              unconnected connectors are listed; --format text | json

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
  --method M              bdf | radau | esdirk | auto | dopri5 | rk4 | euler
                          (default: the experiment's __Dymola_Algorithm, else
                          bdf; auto switches between dopri5 and bdf)
  --step H                fixed step size (linear-analysis: --step is a flag)
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
  --jacobian J            linearize: exact | fd (central differences);
                          simulate: state Jacobian of bdf/radau/esdirk/auto,
                          exact (default) | fd (forward differences)
  --inputs a,b            linear-analysis: inputs to analyze (default: all
                          top-level Real inputs)
  --outputs y,z           linear-analysis: outputs (default: all top-level
                          Real outputs; any continuous variable or state)
  --bode                  linear-analysis: write Bode/Nyquist data
  --step                  linear-analysis: write step and impulse responses
  --freq LO,HI            linear-analysis: frequency range in rad/s
  --tfinal T              linear-analysis: step response horizon
  --reduce N              linear-analysis: balanced truncation to N states
  --lqr Q,R               linear-analysis: LQR gain for Q = q*I, R = r*I
  --at-time T             steady: simulate to time T first (default: start)
  --free a,b              steady: inputs or parameters to solve for (trim)
  --target y=v            steady: target value of a variable (repeatable or
                          comma-separated; as many targets as free variables)
  --fmi-lib FILE          fmu: the FMI runtime library (default: next to this
                          executable, or $DAEDALUS_FMI_LIB)
  --timeout S             per-model time limit for test-library (default 60)
  --fmi-version V         fmu: 2 (default) or 3
  --fmi-type T            import-fmu, import-ssp, simulate X.fmu/.ssp: me | cs
                          (default: Model Exchange for an FMU if it has it,
                          Co-Simulation for SSP components)
  --comm-step H           Co-Simulation communication step size
  --class NAME            import-fmu: name of the wrapper block
  --within PACKAGE        import-fmu: within clause of the wrapper
  --locals                import-fmu: also expose the FMU's local variables as
                          outputs (always on for `simulate X.fmu`)
  --state NAME=TIER       stateSelect override, TIER = always | prefer | default
                          | avoid | never; NAME may be a glob (arm.*.phi)
                          (repeatable; check, dump, simulate, linearize,
                          structure, fmu, export-casadi)
  --no-jacobian           structure: skip the Jacobian analysis (structure also
                          takes --at T and --format text | json)
  --profile [FILE.json]   simulate: record a performance profile (step sizes,
                          states limiting the step, time per block, event log,
                          chattering) and print its summary; with a .json file
                          also write the full profile there
  --events                simulate: print the event log (implies profiling)
  --units                 check: run the unit checker too
  --json                  units: JSON output (same as --format json)
  --all                   units: list variables without a unit too

SWEEP OPTIONS:
  -s, --sweep NAME=VALUES  swept parameter (repeatable); VALUES is a list
                          1,2,3 or lin:START:STOP:N or log:START:STOP:N
  --grouped               run the n-th values of all parameters together
                          (default: full factorial grid)
  --vars a,b,c            variables kept per run (default: the states)
  --stats S,...           summary columns per variable: initial | final |
                          min | max | mean | at=T (default final)
  -o FILE.csv | DIR       one combined CSV (run, parameters, time, vars), or
                          a directory with run_001.csv... and summary.csv
  --format csv | mat      per-run file format in an output directory
  --summary FILE          write the summary table as CSV
  --jobs N                parallel threads (default: cores)
  --retranslate           translate every run again (default only for
                          non-free, Integer and Boolean parameters)

EMBEDDED OPTIONS (codegen-c, hil; see docs/embedded.md):
  --step H                codegen-c: the fixed step (sample time) in seconds
  --real float|double     codegen-c: arithmetic type (default float)
  --integrator rk4|euler  codegen-c: for continuous states (default rk4)
  --prefix NAME           codegen-c: C identifier prefix (default: model name)
  --hil                   codegen-c: also write the processor-in-the-loop shells
                          (host, ATmega328P/Arduino Uno, ESP32, Arduino sketch)
  -p NAME=VALUE           codegen-c: parameter value baked into the code
  --controller FILE.json  hil: the manifest written by codegen-c
  --exec CMD              hil: run the controller as a program on stdin/stdout
                          (host build, simavr_bridge, qemu-system-xtensa ...)
  --tcp HOST:PORT         hil: reach it over TCP (QEMU -serial tcp::PORT,server)
  --serial DEV [--baud N] hil: a real board on a serial port (default 115200)
  --map PORT=VAR          hil: wire a controller port to another bench variable
                          (default: same names); --vars records bench variables
  --stop T, -o FILE.csv   hil: stop time and result file

ANALYSIS COMMANDS (text summary on stdout; -o FILE.json or FILE.csv writes data):
  sensitivity  local sensitivities dy/dp along the trajectory:
                 --params a,b --outputs x,y [--sens-method auto|exact|fd]
                 [--rel-step H]; exact = forward sensitivity equations with
                 tape tangents (no events), fd = central differences.
                 With --adjoint --objective QOI: the gradient of one scalar
                 (final, integral, mean, at, max, min of a variable) for many
                 parameters by a backward adjoint solve with reverse-mode tape
                 passes [--grad-method auto|adjoint|forward|fd] [--substeps N];
                 auto falls back to forward sensitivities at events
  montecarlo   Monte Carlo statistics (mean, std, 5/50/95 % bands) over
                 --dist p=lo:hi | p=normal:MEAN:STD | p=lognormal:MEAN:STD
                 (repeatable) --outputs x,y [--qoi 'max(x)'] [-N SAMPLES]
                 [--sampling lhs|random] [--seed S]; with --sobol or --morris
                 global sensitivity indices of the --qoi quantities
  gsa          the same, Sobol indices by default (N base samples: N (k+2)
                 runs); quantities: final(x) max(x) min(x) mean(x)
                 integral(x) at(x,T), window: max(x,T0,T1)
  calibrate    fit parameters to measured data (projected Levenberg-Marquardt):
                 --data meas.csv --tune p=lo:hi[:start] (repeatable)
                 [--map column=var] [--weight column=W] [--window T0:T1]
                 [--max-iter N] [--sens-method auto|exact|fd]; prints the fit,
                 standard errors and a Modelica modifier; --optimizer lbfgsb
                 minimizes the cost with adjoint gradients instead
  optimize     minimize scalar quantities over bounded parameters (Nelder-Mead):
                 --tune p=lo:hi --minimize QOI | --maximize QOI |
                 --target QOI=VALUE (repeatable); --algorithm lbfgsb uses
                 bound-constrained L-BFGS with adjoint gradients
                 [--grad-method auto|adjoint|forward|fd]
  doe          design of experiments: --factor p=lo:hi[:levels] (repeatable)
                 --design ccd|bbd|full|frac|lhs --qoi 'max(x)' (repeatable)
                 [--alpha face|rotatable|orthogonal|A] [--center N]
                 [--generators D=ABC,E=AB] [--resolution R] [-N SAMPLES]
                 [--seed S] [--pareto min,max] [--jobs N]; runs in parallel
                 and fits quadratic response surfaces (R^2, coefficients),
                 main effects, interactions and the Pareto front of 2 QoIs
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
    if cmd == "mcp" {
        return mcp::run(&args[1..]);
    }
    let (args, an) = analysis::take_args(args)?;
    let (args, emb) = embedded::take_args(args)?;
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
    let mut sweeps: Vec<String> = Vec::new();
    let mut grouped = false;
    let mut stats_spec: Option<String> = None;
    let mut summary_out: Option<String> = None;
    let mut retranslate = false;
    let mut set_format = false;
    let mut no_jacobian = false;
    let mut profile: Option<Option<String>> = None;
    let mut print_events = false;
    let mut set_jac = false;
    let mut free: Vec<String> = Vec::new();
    let mut debug_vars: Vec<String> = Vec::new();
    let mut debug_eqs: Vec<usize> = Vec::new();
    let mut debug_init = false;
    let mut targets: Vec<(String, f64)> = Vec::new();
    let mut check_units = false;
    let mut list_all = false;
    let mut lti = daedalus_core::lti::LinearOptions::default();
    let mut fmi_version: u32 = 2;
    let mut import = fmu_import::ImportOptions::default();
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
            // linear-analysis: a flag (write step responses); otherwise the fixed step size
            "--step" if cmd == "linear-analysis" => lti.step = true,
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
            "--state" => opts.state_overrides.push(
                daedalus_core::backend::state_select::StateOverride::parse(&next(&mut i)?)?,
            ),
            "--no-jacobian" => no_jacobian = true,
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
            "--fmi-version" => {
                fmi_version = match next(&mut i)?.as_str() {
                    "2" | "2.0" => 2,
                    "3" | "3.0" => 3,
                    v => anyhow::bail!("unknown FMI version '{v}' (2 | 3)"),
                }
            }
            "--fmi-type" => {
                let t = next(&mut i)?;
                import.kind = Some(
                    daedalus_core::external_component::FmiKind::parse(&t)
                        .ok_or_else(|| anyhow::anyhow!("unknown FMI type '{t}' (me | cs)"))?,
                )
            }
            "--comm-step" => import.step = Some(next(&mut i)?.parse()?),
            "--class" => import.class = Some(next(&mut i)?),
            "--within" => import.within = Some(next(&mut i)?),
            "--locals" => import.locals = true,
            "--format" => {
                format = next(&mut i)?;
                set_format = true
            }
            "-s" | "--sweep" => sweeps.push(next(&mut i)?),
            "--grouped" => grouped = true,
            "--stats" => stats_spec = Some(next(&mut i)?),
            "--summary" => summary_out = Some(next(&mut i)?),
            "--retranslate" => retranslate = true,
            "--profile" => {
                // The report file is optional: only a following `.json` argument is one.
                let file = args.get(i + 1).filter(|f| f.ends_with(".json")).cloned();
                if file.is_some() {
                    i += 1;
                }
                profile = Some(file);
            }
            "--events" => print_events = true,
            "--units" => check_units = true,
            "--json" => format = "json".into(),
            "--all" => list_all = true,
            "--inputs" => lti.inputs = csv_list(&next(&mut i)?),
            "--outputs" => lti.outputs = csv_list(&next(&mut i)?),
            "--bode" => lti.bode = true,
            "--reduce" => lti.reduce = Some(next(&mut i)?.parse()?),
            "--tfinal" => lti.t_final = Some(next(&mut i)?.parse()?),
            "--freq" => {
                let v = next(&mut i)?;
                let (lo, hi) = v
                    .split_once(',')
                    .ok_or_else(|| anyhow::anyhow!("expected --freq LO,HI"))?;
                lti.w_min = Some(lo.trim().parse()?);
                lti.w_max = Some(hi.trim().parse()?);
            }
            "--lqr" => {
                let v = next(&mut i)?;
                let (q, r) = v
                    .split_once(',')
                    .ok_or_else(|| anyhow::anyhow!("expected --lqr Q,R"))?;
                lti.lqr = Some((q.trim().parse()?, r.trim().parse()?));
            }
            "--var" => debug_vars.push(next(&mut i)?),
            "--equation" => debug_eqs.push(next(&mut i)?.parse()?),
            "--init" => debug_init = true,
            "--jacobian" => {
                let j = next(&mut i)?;
                so.jacobian = daedalus_core::sim::JacobianMode::parse(&j)
                    .ok_or_else(|| anyhow::anyhow!("unknown jacobian method '{j}' (exact | fd)"))?;
                set_jac = true;
                jac_method = match j.as_str() {
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
    if cmd == "import-fmu" || cmd == "import-ssp" {
        let [src] = sources.as_slice() else {
            anyhow::bail!("{cmd} takes one file");
        };
        return if cmd == "import-fmu" {
            fmu_import::import_fmu(Path::new(src), output.as_deref(), &import)
        } else {
            fmu_import::import_ssp(Path::new(src), output.as_deref(), &import)
        };
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
    let mut lib = Library::new();
    if let Ok(dir) = std::env::var("DAEDALUS_LIB") {
        libs.push(dir);
    }
    let t0 = Instant::now();
    // One model needs only part of a library: parse just the files it can reach
    // (test-library lists every model, so it parses everything).
    let lazy = cmd != "test-library";
    // `.fmu` and `.ssp` sources become generated Modelica models (kept alive here).
    let mut generated = Vec::new();
    for s in libs.iter().chain(sources.iter()) {
        let p = Path::new(s);
        let ext = p.extension().and_then(|e| e.to_str()).unwrap_or("");
        if p.is_file() && (ext.eq_ignore_ascii_case("fmu") || ext.eq_ignore_ascii_case("ssp")) {
            let g = if ext.eq_ignore_ascii_case("fmu") {
                fmu_import::fmu_source(p, &import)?
            } else {
                fmu_import::ssp_source(p, &import)?
            };
            for e in lib.set_document(&g.uri, &g.text) {
                eprintln!("{e}");
            }
            if model.is_none() {
                model = Some(g.model.clone());
            }
            generated.push(g);
            continue;
        }
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
    let model = model.ok_or_else(|| anyhow::anyhow!("no model given (-m)"))?;
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
        "units" => {
            let flat = with_all_files(&mut lib, |l| l.flatten(&model))?;
            let r = daedalus_core::units::check(&flat);
            print_units(&flat, &r, &format, list_all)?;
            Ok(())
        }
        "flatten" => {
            let f = with_all_files(&mut lib, |l| l.flatten(&model))?;
            print!("{}", f.dump());
            Ok(())
        }
        "dump" | "check" => {
            let t1 = Instant::now();
            let flat = with_all_files(&mut lib, |l| l.flatten(&model))?;
            let t_flat = t1.elapsed();
            let unit_report = check_units.then(|| daedalus_core::units::check(&flat));
            if let Some(r) = &unit_report {
                for w in &r.issues {
                    eprintln!("warning: {}", w.message);
                }
            }
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
                if let Some(r) = &unit_report {
                    println!(
                        "  units: {} issues, {} declared, {} inferred, {} unknown ({:.1} ms)",
                        r.issues.len(),
                        r.declared,
                        r.inferred.len(),
                        r.unknown,
                        r.ms
                    );
                }
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
            if set_jac {
                o.jacobian = so.jacobian;
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
            let o_method = o.method;
            let o_dbg = o.clone();
            o.profile = profile.is_some() || print_events;
            let (r, prof) = daedalus_core::sim::simulate_profiled(&m, o);
            if let Some(p) = &prof {
                if print_events {
                    print!("{}", p.event_log_text());
                }
                if profile.is_some() {
                    print!("{}", p.summary_text());
                }
                if let Some(Some(file)) = &profile {
                    std::fs::write(file, serde_json::to_string(p)?)?;
                    eprintln!("profile written to {file}");
                }
            }
            let (res, stats) = match r {
                Ok(r) => r,
                Err(e) => {
                    if let (Some(out), Some(p)) = (&output, &e.partial) {
                        write_result(p, out, vars.as_deref())?;
                        eprintln!("partial results written to {out}");
                    }
                    // Explain the failure: the initialization report, or the nonlinear
                    // block that failed during the run.
                    let mut o2 = o_dbg.clone();
                    o2.profile = false;
                    if e.partial.is_none() {
                        let rep =
                            daedalus_core::debug::init_report(&lib, &model, &opts, Some(o2), &[]);
                        eprint!("{}", rep.to_text());
                    } else if let Some(f) =
                        daedalus_core::debug::simulation_failure(&lib, &model, &opts, o2)
                    {
                        eprint!("{}", f.to_text());
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
            if stats.solver.tangent_evals > 0 || stats.solver.switches > 0 {
                eprintln!(
                    "solver {}: {} exact Jacobian passes ({} colors), {} method switches",
                    o_method.name(),
                    stats.solver.tangent_evals,
                    stats.jacobian_colors,
                    stats.solver.switches
                );
            }
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
        "structure" => {
            let mut so2 = daedalus_core::structure::StructureOptions {
                jacobian: !no_jacobian,
                at: lin_at,
                params: so.params.clone(),
                ..Default::default()
            };
            if set_start || set_tol || set_method || set_stop {
                // Experiment defaults with the command-line changes on top.
                let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
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
                o.params = so.params.clone();
                so2.sim = Some(o);
            }
            let rep = with_all_files(&mut lib, |l| {
                daedalus_core::structure::structure(l, &model, &opts, &so2)
            })?;
            let text = match format.as_str() {
                "text" => rep.to_text(),
                "json" => serde_json::to_string_pretty(&rep)? + "\n",
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
        "linear-analysis" => {
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
            let lin = daedalus_core::linearize::linearize_with_outputs(
                &m,
                o,
                lin_at,
                jac_method,
                &lti.outputs,
            )
            .map_err(|e| anyhow::anyhow!(e))?;
            if lin.inputs.is_empty() {
                anyhow::bail!(
                    "{model} has no top-level Real inputs: linear analysis needs `input Real` variables"
                );
            }
            let csv = output.as_deref().is_some_and(|p| p.ends_with(".csv"));
            let mut lo = lti.clone();
            if output.is_some() && !lo.bode && !lo.step {
                // a data file without --bode/--step gets both
                lo.bode = true;
                lo.step = true;
            }
            let res = daedalus_core::lti::analyze(&lin, &lo).map_err(|e| anyhow::anyhow!(e))?;
            print!("{}", res.report());
            if let Some(path) = &output {
                if csv {
                    let mut written = Vec::new();
                    if lo.bode {
                        std::fs::write(path, res.bode_csv())?;
                        written.push(path.clone());
                    }
                    if lo.step {
                        let p = if lo.bode {
                            format!("{}_step.csv", path.trim_end_matches(".csv"))
                        } else {
                            path.clone()
                        };
                        std::fs::write(&p, res.step_csv())?;
                        written.push(p);
                    }
                    eprintln!("wrote {}", written.join(", "));
                } else {
                    std::fs::write(path, serde_json::to_string_pretty(&res)? + "\n")?;
                    eprintln!("wrote {path}");
                }
            }
            Ok(())
        }
        "debug" => {
            let json = format.as_str() == "json";
            let mut out = String::new();
            let mut values = Vec::new();
            if !debug_vars.is_empty() || !debug_eqs.is_empty() {
                let d = with_all_files(&mut lib, |l| {
                    daedalus_core::debug::Debugger::new(l, &model, &opts).map_err(|f| {
                        let mut msg = f.error.clone();
                        if let Some(sd) = &f.structural {
                            msg.push('\n');
                            msg.push_str(&sd.to_text());
                        }
                        anyhow::anyhow!(msg)
                    })
                })?;
                for v in &debug_vars {
                    let t = d.trace_var(v).map_err(|e| anyhow::anyhow!(e))?;
                    if json {
                        values.push(serde_json::to_value(&t)?);
                    } else {
                        out.push_str(&t.to_text());
                        out.push('\n');
                    }
                }
                for &e in &debug_eqs {
                    let t = d.trace_eq(e).map_err(|e| anyhow::anyhow!(e))?;
                    if json {
                        values.push(serde_json::to_value(&t)?);
                    } else {
                        out.push_str(&t.to_text());
                        out.push('\n');
                    }
                }
            }
            if debug_init || (debug_vars.is_empty() && debug_eqs.is_empty()) {
                // Load the whole library when the lazily loaded part does not flatten.
                let _ = with_all_files(&mut lib, |l| l.flatten(&model).map(|_| ()));
                let so2 = if set_start || set_stop || set_tol || set_method {
                    let m =
                        with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))
                            .ok();
                    m.map(|m| {
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
                        o
                    })
                } else {
                    None
                };
                let rep = daedalus_core::debug::init_report(&lib, &model, &opts, so2, &so.params);
                if json {
                    values.push(serde_json::to_value(&rep)?);
                } else {
                    out.push_str(&rep.to_text());
                }
            }
            let text = if json {
                serde_json::to_string_pretty(&if values.len() == 1 {
                    values.pop().unwrap()
                } else {
                    serde_json::Value::Array(values)
                })? + "\n"
            } else {
                out
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
        "codegen-c" => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            embedded::codegen(&m, &so, &emb, output.as_deref())
        }
        "hil" => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            let mut so = so.clone();
            if !set_tol {
                so.tolerance = 1e-6;
            }
            embedded::hil(
                &m,
                &so,
                set_stop,
                &emb,
                vars.as_deref().unwrap_or(&[]),
                output.as_deref(),
            )
        }
        "fmu" => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            let out = output
                .clone()
                .unwrap_or_else(|| format!("{}.fmu", m.name.replace('.', "_")));
            fmu::export(
                &m,
                std::path::Path::new(&out),
                fmi_lib.as_deref(),
                fmi_version,
            )?;
            eprintln!("wrote {out}");
            Ok(())
        }
        "export-ssp" => {
            let out = output
                .clone()
                .unwrap_or_else(|| format!("{}.ssp", model.rsplit('.').next().unwrap_or(&model)));
            with_all_files(&mut lib, |l| {
                fmu_import::export_ssp(l, &model, std::path::Path::new(&out))
            })
        }
        "sweep" => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
            for w in &m.warnings {
                eprintln!("warning: {w}");
            }
            // Simulation options as for `simulate`.
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
                o.interval = 0.0;
            }
            o.intervals = so.intervals;
            o.fixed_step = so.fixed_step;
            o.store_events = so.store_events;
            o.max_event_rows = so.max_event_rows;
            o.params = so.params.clone();
            let stats = stats_spec
                .as_deref()
                .unwrap_or("final")
                .split(',')
                .map(daedalus_core::sweep::Stat::parse)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| anyhow::anyhow!(e))?;
            let args = sweep::Args {
                sweeps: sweeps.clone(),
                combine: if grouped {
                    daedalus_core::sweep::Combine::Grouped
                } else {
                    daedalus_core::sweep::Combine::Grid
                },
                vars: vars.clone(),
                stats,
                output: output.clone(),
                format: if set_format {
                    format.clone()
                } else {
                    "csv".into()
                },
                summary: summary_out.clone(),
                jobs,
                retranslate,
            };
            sweep::run(&mut lib, &model, m, o, &opts, args)
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
            if set_method {
                child_args.extend(["--method".into(), so.method.name().into()]);
            }
            if set_jac {
                let j = match so.jacobian {
                    daedalus_core::sim::JacobianMode::Exact => "exact",
                    daedalus_core::sim::JacobianMode::Fd => "fd",
                };
                child_args.extend(["--jacobian".into(), j.into()]);
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
            if set_method {
                o.method = so.method;
            }
            if set_jac {
                o.jacobian = so.jacobian;
            }
            match daedalus_core::sim::simulate(&m, o) {
                Ok((_, st)) => println!(
                    "OK {} states, {} steps, {:.0} ms, {} rejected, {} f-evals, {} jacobians",
                    m.states.len(),
                    st.solver.steps,
                    t.elapsed().as_secs_f64() * 1000.0,
                    st.solver.rejected,
                    st.solver.rhs_evals,
                    st.solver.jac_evals
                ),
                Err(e) => println!("ERR simulate: {}", e.message.lines().next().unwrap_or("")),
            }
            Ok(())
        }
        c if analysis::COMMANDS.contains(&c) => {
            let m = with_all_files(&mut lib, |l| daedalus_core::translate(l, &model, &opts))?;
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
                o.interval = 0.0;
            }
            o.intervals = so.intervals;
            o.params = so.params.clone();
            analysis::run(c, m, o, &an, output.as_deref(), &format, jobs)
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

/// `daedalus units`: the unit of every variable and the unit issues.
fn print_units(
    flat: &daedalus_core::flat::FlatModel,
    r: &daedalus_core::units::UnitReport,
    format: &str,
    all: bool,
) -> anyhow::Result<()> {
    use daedalus_core::flat::VarType;
    let units = daedalus_core::units::resolved_units(flat, r);
    let inferred: std::collections::HashSet<u32> = r.inferred.iter().map(|i| i.var).collect();
    let rows: Vec<(usize, &str, &str)> = flat
        .vars
        .iter()
        .enumerate()
        .filter(|(_, v)| v.ty == VarType::Real && !v.name.starts_with('$'))
        .filter_map(|(i, v)| {
            let u = units.get(&(i as u32)).map(|s| s.as_str()).unwrap_or("");
            let src = if !v.unit.is_empty() {
                "declared"
            } else if inferred.contains(&(i as u32)) {
                "inferred"
            } else {
                "unknown"
            };
            (all || !u.is_empty()).then_some((i, u, src))
        })
        .collect();
    if format == "json" {
        let vars: Vec<serde_json::Value> = rows
            .iter()
            .map(|&(i, u, src)| {
                let v = &flat.vars[i];
                serde_json::json!({"name": v.name, "unit": u, "source": src, "displayUnit": v.display_unit})
            })
            .collect();
        let out = serde_json::json!({
            "model": flat.name,
            "variables": vars,
            "issues": r.issues,
            "unparsed": r.unparsed,
            "stats": {"declared": r.declared, "inferred": r.inferred.len(), "unknown": r.unknown,
                      "equations": r.equations, "passes": r.passes, "ms": r.ms},
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(());
    }
    println!("model {}", flat.name);
    let w = rows
        .iter()
        .map(|&(i, ..)| flat.vars[i].name.len())
        .max()
        .unwrap_or(0);
    for &(i, u, src) in &rows {
        let v = &flat.vars[i];
        let du = if v.display_unit.is_empty() {
            String::new()
        } else {
            format!(" (display {})", v.display_unit)
        };
        println!(
            "  {:w$}  {:14} {src}{du}",
            v.name,
            if u.is_empty() { "?" } else { u }
        );
    }
    if !r.unparsed.is_empty() {
        println!("unparsed unit strings: {}", r.unparsed.join(", "));
    }
    println!(
        "{} unit issues; {} declared, {} inferred, {} unknown; {} equations, {} passes, {:.1} ms",
        r.issues.len(),
        r.declared,
        r.inferred.len(),
        r.unknown,
        r.equations,
        r.passes,
        r.ms
    );
    for i in &r.issues {
        println!("warning: {}", i.message);
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

fn csv_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}
