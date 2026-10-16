//! `daedalus sensitivity | montecarlo | gsa | calibrate | optimize`: the analyses of
//! `daedalus_core::{sensitivity, uq, calibrate, optimize}` on the command line.

use daedalus_core::calibrate::{CalibSpec, Calibration, DataMap, Tuner};
use daedalus_core::codegen::Model;
use daedalus_core::optimize::{ObjTerm, OptSpec, Optimization};
use daedalus_core::sensitivity::{SensJob, SensMethod, SensOptions};
use daedalus_core::sim::SimOptions;
use daedalus_core::uq::{Dist, GsaJob, GsaMode, GsaSpec, ParamDist, Sampling, run_job};

pub const COMMANDS: &[&str] = &["sensitivity", "montecarlo", "gsa", "calibrate", "optimize"];

/// Options of the analysis commands (taken out of the argument list before the
/// common options are parsed).
#[derive(Default)]
pub struct Args {
    params: Vec<String>,
    outputs: Vec<String>,
    sens_method: SensMethod,
    rel_step: Option<f64>,
    dists: Vec<String>,
    qois: Vec<String>,
    samples: Option<usize>,
    sampling: Sampling,
    seed: Option<u64>,
    mode: Option<GsaMode>,
    levels: Option<usize>,
    data: Option<String>,
    tune: Vec<String>,
    map: Vec<String>,
    weights: Vec<String>,
    window: Option<(f64, f64)>,
    max_iter: Option<usize>,
    objective: Vec<ObjTerm>,
}

fn list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|x| x.trim().to_string())
        .filter(|x| !x.is_empty())
        .collect()
}

/// Remove the analysis options from `args` (only for the analysis commands).
pub fn take_args(args: Vec<String>) -> anyhow::Result<(Vec<String>, Args)> {
    let mut a = Args::default();
    if !COMMANDS.contains(&args[0].as_str()) {
        return Ok((args, a));
    }
    let mut rest = Vec::new();
    let mut it = args.into_iter();
    while let Some(x) = it.next() {
        let mut val = || {
            it.next()
                .ok_or_else(|| anyhow::anyhow!("missing value for {x}"))
        };
        match x.as_str() {
            "--params" => a.params.extend(list(&val()?)),
            "--outputs" => a.outputs.extend(list(&val()?)),
            "--sens-method" => {
                let v = val()?;
                a.sens_method = SensMethod::parse(&v).ok_or_else(|| {
                    anyhow::anyhow!("unknown sensitivity method '{v}' (auto | exact | fd)")
                })?;
            }
            "--rel-step" => a.rel_step = Some(val()?.parse()?),
            "--dist" => a.dists.push(val()?),
            "--qoi" => a.qois.push(val()?),
            "-N" | "--samples" => a.samples = Some(val()?.parse()?),
            "--sampling" => {
                a.sampling = match val()?.as_str() {
                    "lhs" => Sampling::Lhs,
                    "random" | "mc" => Sampling::Random,
                    s => anyhow::bail!("unknown sampling '{s}' (lhs | random)"),
                }
            }
            "--seed" => a.seed = Some(val()?.parse()?),
            "--sobol" => a.mode = Some(GsaMode::Sobol),
            "--morris" => a.mode = Some(GsaMode::Morris),
            "--levels" => a.levels = Some(val()?.parse()?),
            "--data" => a.data = Some(val()?),
            "--tune" => a.tune.push(val()?),
            "--map" => a.map.push(val()?),
            "--weight" => a.weights.push(val()?),
            "--window" => {
                let v = val()?;
                let (t0, t1) = v
                    .split_once(':')
                    .ok_or_else(|| anyhow::anyhow!("expected --window T0:T1"))?;
                a.window = Some((t0.parse()?, t1.parse()?));
            }
            "--max-iter" => a.max_iter = Some(val()?.parse()?),
            "--minimize" | "--maximize" => {
                let w = if x == "--minimize" { 1.0 } else { -1.0 };
                a.objective.push(ObjTerm {
                    qoi: val()?,
                    weight: w,
                    target: None,
                });
            }
            "--target" => {
                let v = val()?;
                let (q, t) = v
                    .rsplit_once('=')
                    .ok_or_else(|| anyhow::anyhow!("expected --target QOI=VALUE"))?;
                a.objective.push(ObjTerm {
                    qoi: q.to_string(),
                    weight: 1.0,
                    target: Some(t.trim().parse()?),
                });
            }
            _ => rest.push(x),
        }
    }
    Ok((rest, a))
}

fn progress(label: &str) -> impl FnMut(usize, usize) -> bool {
    let t0 = std::time::Instant::now();
    let mut last = 0.0;
    let label = label.to_string();
    move |d, t| {
        let el = t0.elapsed().as_secs_f64();
        if el - last > 0.5 || d == t {
            last = el;
            eprint!("\r{label}: {d}/{t}  ({el:.1} s)   ");
            if d == t {
                eprintln!();
            }
        }
        true
    }
}

/// Run an analysis command and print / write its result.
pub fn run(
    cmd: &str,
    m: Model,
    o: SimOptions,
    a: &Args,
    output: Option<&str>,
    format: &str,
    jobs: usize,
) -> anyhow::Result<()> {
    let err = |e: String| anyhow::anyhow!(e);
    let (text, json, csv): (String, serde_json::Value, Option<String>) = match cmd {
        "sensitivity" => {
            if a.params.is_empty() {
                anyhow::bail!("give the parameters with --params a,b");
            }
            let outputs = if a.outputs.is_empty() {
                m.states
                    .iter()
                    .map(|&s| m.vars[s as usize].name.clone())
                    .collect()
            } else {
                a.outputs.clone()
            };
            let mut so = SensOptions::new(a.params.clone(), outputs, a.sens_method);
            if let Some(h) = a.rel_step {
                so.rel_step = h;
            }
            let mut job = SensJob::new(m, o, so).map_err(err)?;
            run_job(&mut job, &mut progress("runs")).map_err(err)?;
            let s = job.result().unwrap();
            (s.report(), serde_json::to_value(s)?, Some(s.to_csv()))
        }
        "montecarlo" | "gsa" => {
            if a.dists.is_empty() {
                anyhow::bail!(
                    "give the uncertain parameters with --dist p=lo:hi (or normal:mean:std, lognormal:mean:std)"
                );
            }
            let params = a
                .dists
                .iter()
                .map(|d| {
                    let (n, s) = d
                        .split_once('=')
                        .ok_or_else(|| anyhow::anyhow!("expected --dist NAME=DIST, got '{d}'"))?;
                    Ok(ParamDist {
                        name: n.trim().to_string(),
                        dist: Dist::parse(s).map_err(err)?,
                    })
                })
                .collect::<anyhow::Result<Vec<_>>>()?;
            let mode = a.mode.unwrap_or(if cmd == "gsa" {
                GsaMode::Sobol
            } else {
                GsaMode::MonteCarlo
            });
            let n = a.samples.unwrap_or(match mode {
                GsaMode::MonteCarlo => 200,
                GsaMode::Sobol => 512,
                GsaMode::Morris => 20,
            });
            let spec = GsaSpec {
                mode,
                params,
                outputs: a.outputs.clone(),
                qois: a.qois.clone(),
                n,
                sampling: a.sampling,
                seed: a.seed.unwrap_or(12345),
                levels: a.levels.unwrap_or(4),
            };
            let mut job = GsaJob::new(m, o, spec).map_err(err)?;
            let total = job.samples().len();
            eprintln!("{total} runs on {jobs} threads");
            job.run_parallel(jobs, &mut progress("runs")).map_err(err)?;
            let r = job.result();
            let csv = (!r.bands.is_empty()).then(|| {
                let mut s = String::from("time");
                for b in &r.bands {
                    for k in ["mean", "std", "q05", "q50", "q95"] {
                        s.push_str(&format!(",\"{}.{k}\"", b.name));
                    }
                }
                s.push('\n');
                for (i, t) in r.time.iter().enumerate() {
                    s.push_str(&t.to_string());
                    for b in &r.bands {
                        for v in [b.mean[i], b.std[i], b.q05[i], b.q50[i], b.q95[i]] {
                            s.push_str(&format!(",{v}"));
                        }
                    }
                    s.push('\n');
                }
                s
            });
            (GsaJob::report_of(&r), serde_json::to_value(&r)?, csv)
        }
        "calibrate" => {
            let data = a
                .data
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("give the measurements with --data meas.csv"))?;
            let csv_text =
                std::fs::read_to_string(data).map_err(|e| anyhow::anyhow!("{data}: {e}"))?;
            let tuners = tuners(&a.tune)?;
            let mut map: Vec<DataMap> = a
                .map
                .iter()
                .map(|s| {
                    let (c, v) = s.split_once('=').unwrap_or((s, s));
                    DataMap {
                        column: c.trim().into(),
                        var: v.trim().into(),
                        weight: None,
                    }
                })
                .collect();
            for w in &a.weights {
                let (c, v) = w
                    .split_once('=')
                    .ok_or_else(|| anyhow::anyhow!("expected --weight COLUMN=W"))?;
                let v: f64 = v.parse()?;
                match map.iter_mut().find(|d| d.column == c) {
                    Some(d) => d.weight = Some(v),
                    None => map.push(DataMap {
                        column: c.into(),
                        var: c.into(),
                        weight: Some(v),
                    }),
                }
            }
            let spec = CalibSpec {
                tuners,
                csv: csv_text,
                time_column: None,
                map,
                window: a.window,
                method: a.sens_method,
                max_iter: a.max_iter.unwrap_or(40),
            };
            let mut job = Calibration::new(m, o, spec).map_err(err)?;
            run_job(&mut job, &mut progress("iterations")).map_err(err)?;
            let r = job.result();
            let mut csv = String::from("time");
            for c in &r.columns {
                csv.push_str(&format!(",\"{}.before\",\"{}.after\"", c.var, c.var));
            }
            csv.push('\n');
            for (i, t) in r.time.iter().enumerate() {
                csv.push_str(&t.to_string());
                for c in &r.columns {
                    csv.push_str(&format!(",{},{}", c.before[i], c.after[i]));
                }
                csv.push('\n');
            }
            (
                Calibration::report_of(&r),
                serde_json::to_value(&r)?,
                Some(csv),
            )
        }
        "optimize" => {
            if a.objective.is_empty() {
                anyhow::bail!(
                    "give an objective with --minimize QOI, --maximize QOI or --target QOI=VALUE"
                );
            }
            let spec = OptSpec {
                tuners: tuners(&a.tune)?,
                objective: a.objective.clone(),
                max_iter: a.max_iter.unwrap_or(200),
                tol: 1e-8,
            };
            let mut job = Optimization::new(m, o, spec).map_err(err)?;
            run_job(&mut job, &mut progress("iterations")).map_err(err)?;
            let r = job.result();
            (Optimization::report_of(&r), serde_json::to_value(&r)?, None)
        }
        _ => unreachable!(),
    };
    let json_text = serde_json::to_string_pretty(&json)? + "\n";
    match output {
        Some(path) => {
            let body = if path.ends_with(".csv") {
                csv.ok_or_else(|| anyhow::anyhow!("{cmd} has no CSV output; use .json"))?
            } else {
                json_text
            };
            std::fs::write(path, body)?;
            print!("{text}");
            eprintln!("wrote {path}");
        }
        None => match format {
            "json" => print!("{json_text}"),
            "csv" => print!(
                "{}",
                csv.ok_or_else(|| anyhow::anyhow!("{cmd} has no CSV output"))?
            ),
            _ => print!("{text}"),
        },
    }
    Ok(())
}

fn tuners(specs: &[String]) -> anyhow::Result<Vec<Tuner>> {
    if specs.is_empty() {
        anyhow::bail!("give the parameters to tune with --tune name=lo:hi");
    }
    specs
        .iter()
        .map(|s| Tuner::parse(s).map_err(|e| anyhow::anyhow!(e)))
        .collect()
}
