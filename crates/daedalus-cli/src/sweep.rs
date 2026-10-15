//! `daedalus sweep`: many simulations of one model with parameter overrides, in parallel.

use daedalus_core::backend::Options;
use daedalus_core::codegen::Model;
use daedalus_core::frontend::Library;
use daedalus_core::sim::SimOptions;
use daedalus_core::sweep::{self, Combine, RunRecord, Stat};
use std::io::Write;
use std::path::Path;
use std::time::Instant;

pub struct Args {
    pub sweeps: Vec<String>,
    pub combine: Combine,
    pub vars: Option<Vec<String>>,
    pub stats: Vec<Stat>,
    pub output: Option<String>,
    /// Per-run file format in an output directory: csv | mat.
    pub format: String,
    pub summary: Option<String>,
    pub jobs: usize,
    /// Translate every run again, even for run-time parameters.
    pub retranslate: bool,
}

pub fn run(
    lib: &mut Library,
    model: &str,
    base: Model,
    o: SimOptions,
    opts: &Options,
    a: Args,
) -> anyhow::Result<()> {
    if a.sweeps.is_empty() {
        anyhow::bail!("no sweep given (-s NAME=VALUES)");
    }
    let params = a
        .sweeps
        .iter()
        .map(|s| sweep::parse_param(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!(e))?;
    let points = sweep::expand(&params, a.combine).map_err(|e| anyhow::anyhow!(e))?;
    let vars: Vec<String> = match &a.vars {
        Some(v) => v.clone(),
        None => base
            .states
            .iter()
            .map(|&s| base.vars[s as usize].name.clone())
            .collect(),
    };
    sweep::check_names(&base, params.iter().map(|p| p.name.as_str()))
        .map_err(|e| anyhow::anyhow!(e))?;
    for p in &params {
        if sweep::classify(&base, &p.name) == sweep::ParamClass::Structural && !a.retranslate {
            eprintln!(
                "note: '{}' is not a free Real parameter; it is applied as a modifier and each value is translated again (slower)",
                p.name
            );
        }
    }
    // One translation per distinct set of structural overrides; index 0 is the base model.
    let t_tr = Instant::now();
    let mut models: Vec<(sweep::Overrides, Model)> = vec![(Vec::new(), base)];
    // (model index, run-time overrides, translated for this run)
    let mut plan: Vec<(usize, sweep::Overrides, bool)> = Vec::new();
    let mut translate_errors: Vec<Option<String>> = Vec::new();
    for p in &points {
        let (rt, st) = sweep::split_overrides(&models[0].1, p, a.retranslate);
        if st.is_empty() {
            plan.push((0, rt, false));
            translate_errors.push(None);
            continue;
        }
        let i = match models.iter().position(|(s, _)| *s == st) {
            Some(i) => Ok(i),
            None => {
                match sweep::translate_with_modifiers(lib, model, &st, Some(&models[0].1), opts) {
                    Ok(m) => {
                        models.push((st, m));
                        Ok(models.len() - 1)
                    }
                    Err(e) => Err(format!("translation: {e:#}")),
                }
            }
        };
        match i {
            Ok(i) => {
                plan.push((i, rt, true));
                translate_errors.push(None);
            }
            Err(e) => {
                plan.push((0, rt, true));
                translate_errors.push(Some(e));
            }
        }
    }
    let n_tr = models.len() - 1;
    let jobs = a.jobs.min(points.len()).max(1);
    eprintln!(
        "sweep of {model}: {} runs ({} parameters, {}), {} extra translations in {:.0} ms, {jobs} threads",
        points.len(),
        params.len(),
        match a.combine {
            Combine::Grid => "grid",
            Combine::Grouped => "grouped",
        },
        n_tr,
        t_tr.elapsed().as_secs_f64() * 1000.0
    );
    // Where results go: a combined CSV, or a directory with one file per run.
    let out_dir = a.output.as_ref().filter(|o| {
        let p = Path::new(o);
        p.is_dir() || p.extension().is_none() || o.ends_with('/')
    });
    if let Some(d) = out_dir {
        std::fs::create_dir_all(d)?;
    }
    let ext = if a.format == "mat" { "mat" } else { "csv" };
    let width = points.len().to_string().len().max(3);
    let t0 = Instant::now();
    let done = std::sync::atomic::AtomicUsize::new(0);
    let total = points.len();
    let records = sweep::run_parallel(
        total,
        jobs,
        None,
        |i| {
            let t = Instant::now();
            let (mi, rt, retranslated) = &plan[i];
            let (results, stats, error) = if let Some(e) = &translate_errors[i] {
                (Default::default(), None, Some(e.clone()))
            } else {
                match sweep::simulate_run(&models[*mi].1, &o, rt) {
                    Ok((r, s)) => (r, Some(s), None),
                    Err(e) => (e.partial.unwrap_or_default(), None, Some(e.message)),
                }
            };
            let mut write_error = None;
            if let Some(d) = out_dir
                && !results.time.is_empty()
            {
                let path = Path::new(d).join(format!("run_{:0width$}.{ext}", i + 1));
                let w = (|| -> std::io::Result<()> {
                    let mut f = std::io::BufWriter::new(std::fs::File::create(&path)?);
                    if ext == "mat" {
                        results.write_mat(&mut f)
                    } else {
                        results.write_csv(&mut f, a.vars.as_deref())
                    }
                })();
                if let Err(e) = w {
                    write_error = Some(format!("{}: {e}", path.display()));
                }
            }
            RunRecord {
                index: i,
                params: points[i].clone(),
                retranslated: *retranslated,
                results: sweep::subset(&results, &vars),
                error: error.or(write_error),
                stats,
                wall_ms: t.elapsed().as_secs_f64() * 1000.0,
            }
        },
        |_, r| {
            let k = done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
            let ps: Vec<String> = r.params.iter().map(|(n, v)| format!("{n}={v}")).collect();
            match &r.error {
                None => eprintln!(
                    "[{k}/{total}] run {} {} ok ({:.0} ms{})",
                    r.index + 1,
                    ps.join(" "),
                    r.wall_ms,
                    if r.retranslated { ", translated" } else { "" }
                ),
                Some(e) => eprintln!(
                    "[{k}/{total}] run {} {} FAILED: {}",
                    r.index + 1,
                    ps.join(" "),
                    e.lines().next().unwrap_or("")
                ),
            }
        },
    );
    let records: Vec<RunRecord> = records.into_iter().flatten().collect();
    let ok = records.iter().filter(|r| r.error.is_none()).count();
    eprintln!(
        "{ok}/{} runs ok in {:.0} ms",
        records.len(),
        t0.elapsed().as_secs_f64() * 1000.0
    );
    if let Some(out) = &a.output
        && out_dir.is_none()
    {
        let mut w = std::io::BufWriter::new(std::fs::File::create(out)?);
        sweep::write_combined_csv(&mut w, &records, &vars)?;
        eprintln!("wrote {out}");
    }
    let mut table = Vec::new();
    sweep::write_summary_csv(&mut table, &records, &vars, &a.stats)?;
    let summary_path = a.summary.clone().or_else(|| {
        out_dir.map(|d| {
            Path::new(d)
                .join("summary.csv")
                .to_string_lossy()
                .into_owned()
        })
    });
    if let Some(p) = &summary_path {
        std::fs::write(p, &table)?;
        eprintln!("wrote {p}");
    }
    print_table(&String::from_utf8_lossy(&table));
    if ok == 0 {
        anyhow::bail!("every run failed");
    }
    Ok(())
}

/// Print the summary CSV as an aligned table.
fn print_table(csv: &str) {
    let rows: Vec<Vec<String>> = csv
        .lines()
        .map(|l| split_csv(l).into_iter().map(|c| short(&c)).collect())
        .collect();
    let n = rows.iter().map(|r| r.len()).max().unwrap_or(0);
    let widths: Vec<usize> = (0..n)
        .map(|j| {
            rows.iter()
                .map(|r| r.get(j).map_or(0, |c| c.chars().count()))
                .max()
                .unwrap_or(0)
        })
        .collect();
    let mut out = std::io::stdout().lock();
    for r in rows {
        let line: Vec<String> = r
            .iter()
            .enumerate()
            .map(|(j, c)| format!("{c:>w$}", w = widths[j]))
            .collect();
        let _ = writeln!(out, "{}", line.join("  ").trim_end());
    }
}

fn split_csv(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    for ch in line.chars() {
        match ch {
            '"' => quoted = !quoted,
            ',' if !quoted => out.push(std::mem::take(&mut cur)),
            _ => cur.push(ch),
        }
    }
    out.push(cur);
    out
}

/// Numbers with 6 significant digits; long text cut.
fn short(c: &str) -> String {
    match c.parse::<f64>() {
        Ok(x) if c.contains('.') || c.contains('e') => {
            let a = x.abs();
            if a != 0.0 && !(1e-4..1e7).contains(&a) {
                format!("{x:.5e}")
            } else {
                // round to 6 significant digits, then print the shortest form
                format!("{x:.5e}").parse::<f64>().unwrap_or(x).to_string()
            }
        }
        _ if c.chars().count() > 60 => format!("{}…", c.chars().take(59).collect::<String>()),
        _ => c.to_string(),
    }
}
