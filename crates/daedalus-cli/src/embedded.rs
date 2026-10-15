//! `codegen-c` (embedded C code) and `hil` (software / processor in the loop).

use daedalus_core::codegen::Model;
use daedalus_core::embedded::{self, Integrator, Manifest, RealType};
use daedalus_core::embedded_hil::{self, LoopOptions, StreamLink};
use daedalus_core::sim::SimOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

pub const COMMANDS: &[&str] = &["codegen-c", "hil"];

#[derive(Default)]
pub struct Args {
    pub prefix: Option<String>,
    pub real: Option<RealType>,
    pub integrator: Option<Integrator>,
    pub hil: bool,
    pub controller: Option<String>,
    pub exec: Option<String>,
    pub tcp: Option<String>,
    pub serial: Option<String>,
    pub baud: Option<u32>,
    pub map: Vec<(String, String)>,
    pub timeout: Option<f64>,
}

/// Remove this module's options from `args` (only for its commands).
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
            "--prefix" => a.prefix = Some(val()?),
            "--real" => {
                let v = val()?;
                a.real =
                    Some(RealType::parse(&v).ok_or_else(|| {
                        anyhow::anyhow!("unknown real type '{v}' (float | double)")
                    })?);
            }
            "--integrator" => {
                let v = val()?;
                a.integrator =
                    Some(Integrator::parse(&v).ok_or_else(|| {
                        anyhow::anyhow!("unknown integrator '{v}' (euler | rk4)")
                    })?);
            }
            "--hil" => a.hil = true,
            "--controller" => a.controller = Some(val()?),
            "--exec" => a.exec = Some(val()?),
            "--tcp" => a.tcp = Some(val()?),
            "--serial" => a.serial = Some(val()?),
            "--baud" => a.baud = Some(val()?.parse()?),
            "--reply-timeout" => a.timeout = Some(val()?.parse()?),
            "--map" => {
                for kv in val()?.split(',') {
                    let (k, v) = kv.split_once('=').ok_or_else(|| {
                        anyhow::anyhow!("--map expects PORT=VARIABLE, got '{kv}'")
                    })?;
                    a.map.push((k.trim().to_string(), v.trim().to_string()));
                }
            }
            _ => rest.push(x),
        }
    }
    Ok((rest, a))
}

/// `daedalus codegen-c`: write `<prefix>.h/.c/.json`, a host driver and, with `--hil`,
/// the processor-in-the-loop shells, into `out_dir`.
pub fn codegen(m: &Model, so: &SimOptions, a: &Args, out_dir: Option<&str>) -> anyhow::Result<()> {
    if !(so.fixed_step > 0.0) {
        anyhow::bail!("give the controller's step size with --step (seconds)");
    }
    let opts = embedded::Options {
        prefix: a.prefix.clone(),
        step: so.fixed_step,
        real: a.real.unwrap_or(RealType::Float),
        integrator: a.integrator.unwrap_or(Integrator::Rk4),
        params: so.params.clone(),
    };
    let g = embedded::generate(m, &opts)?;
    let dir = PathBuf::from(out_dir.unwrap_or(&g.manifest.prefix));
    std::fs::create_dir_all(&dir)?;
    let mut files: Vec<(String, String)> = vec![
        (g.header_name(), g.header.clone()),
        (g.source_name(), g.source.clone()),
        (
            format!("{}.json", g.manifest.prefix),
            serde_json::to_string_pretty(&g.manifest)? + "\n",
        ),
        (
            format!("{}_main.c", g.manifest.prefix),
            embedded::host_main(&g.manifest),
        ),
    ];
    if a.hil {
        files.extend(embedded_hil::hil_files(&g.manifest, &g.header, &g.source));
    }
    for (name, text) in &files {
        let p = dir.join(name);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&p, text)?;
    }
    let man = &g.manifest;
    eprintln!(
        "wrote {} files to {}: {} inputs, {} outputs, {} states, {} algebraic loops; {} persistent registers ({} bytes of RAM per instance, {})",
        files.len(),
        dir.display(),
        man.inputs.len(),
        man.outputs.len(),
        man.n_states,
        man.n_newton,
        man.registers,
        man.instance_bytes,
        opts.real.c_name()
    );
    for p in &man.inputs {
        eprintln!("  input  {:<3} {} ({})", p.index, p.name, p.macro_name);
    }
    for p in &man.outputs {
        eprintln!("  output {:<3} {} ({})", p.index, p.name, p.macro_name);
    }
    Ok(())
}

/// A reader that gives up after a timeout (the other end may hang or crash).
struct TimedReader {
    rx: mpsc::Receiver<Vec<u8>>,
    buf: Vec<u8>,
    pos: usize,
    timeout: Duration,
}

impl TimedReader {
    fn new<R: Read + Send + 'static>(mut r: R, timeout: Duration) -> Self {
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut b = [0u8; 4096];
            loop {
                match r.read(&mut b) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx.send(b[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });
        TimedReader {
            rx,
            buf: Vec::new(),
            pos: 0,
            timeout,
        }
    }
}

impl Read for TimedReader {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        if self.pos >= self.buf.len() {
            match self.rx.recv_timeout(self.timeout) {
                Ok(v) => {
                    self.buf = v;
                    self.pos = 0;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(0),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::TimedOut,
                        "no answer from the controller (raise --reply-timeout?)",
                    ));
                }
            }
        }
        let n = out.len().min(self.buf.len() - self.pos);
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

fn split_command(cmd: &str) -> Vec<String> {
    // Whitespace-separated, with "double" or 'single' quotes.
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for ch in cmd.chars() {
        match quote {
            Some(q) if ch == q => quote = None,
            Some(_) => cur.push(ch),
            None if ch == '"' || ch == '\'' => {
                quote = Some(ch);
                any = true;
            }
            None if ch.is_whitespace() => {
                if !cur.is_empty() || any {
                    out.push(std::mem::take(&mut cur));
                    any = false;
                }
            }
            None => cur.push(ch),
        }
    }
    if !cur.is_empty() || any {
        out.push(cur);
    }
    out
}

/// `daedalus hil`: simulate the bench model in closed loop with the controller.
pub fn hil(
    bench: &Model,
    so: &SimOptions,
    set_stop: bool,
    a: &Args,
    record: &[String],
    output: Option<&str>,
) -> anyhow::Result<()> {
    let mpath = a.controller.as_deref().ok_or_else(|| {
        anyhow::anyhow!("give the controller's manifest with --controller DIR/<prefix>.json")
    })?;
    let man: Manifest = serde_json::from_str(
        &std::fs::read_to_string(mpath).map_err(|e| anyhow::anyhow!("{mpath}: {e}"))?,
    )?;
    let timeout = Duration::from_secs_f64(a.timeout.unwrap_or(30.0));
    let mut sim = SimOptions::from_model(bench);
    sim.params = so.params.clone();
    sim.tolerance = so.tolerance.min(sim.tolerance);
    let lo = LoopOptions {
        stop_time: if set_stop { Some(so.stop_time) } else { None },
        map: a.map.clone(),
        record: record.to_vec(),
        sim: Some(sim),
    };
    let t0 = std::time::Instant::now();
    let mut child = None;
    let res = if let Some(cmd) = &a.exec {
        let parts = split_command(cmd);
        if parts.is_empty() {
            anyhow::bail!("--exec: empty command");
        }
        let mut c = std::process::Command::new(&parts[0])
            .args(&parts[1..])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| anyhow::anyhow!("--exec {}: {e}", parts[0]))?;
        let stdin = c.stdin.take().unwrap();
        let stdout = c.stdout.take().unwrap();
        child = Some(c);
        let mut link = StreamLink::connect(TimedReader::new(stdout, timeout), stdin, &man)?;
        embedded_hil::closed_loop(bench, &man, &mut link, &lo)
    } else if let Some(addr) = &a.tcp {
        let s = std::net::TcpStream::connect(addr)?;
        s.set_nodelay(true)?;
        let r = s.try_clone()?;
        let mut link = StreamLink::connect(TimedReader::new(r, timeout), s, &man)?;
        embedded_hil::closed_loop(bench, &man, &mut link, &lo)
    } else if let Some(dev) = &a.serial {
        if let Some(b) = a.baud.or(Some(115200)) {
            // Raw mode at the baud rate (Linux and macOS).
            let flag = if cfg!(target_os = "macos") {
                "-f"
            } else {
                "-F"
            };
            let _ = std::process::Command::new("stty")
                .args([flag, dev, &b.to_string(), "raw", "-echo"])
                .status();
        }
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(Path::new(dev))?;
        let r = f.try_clone()?;
        // A board that resets when the port opens (Uno) needs a moment to boot.
        let mut link = StreamLink::connect(TimedReader::new(r, timeout), f, &man)?;
        embedded_hil::closed_loop(bench, &man, &mut link, &lo)
    } else {
        anyhow::bail!(
            "say how to reach the controller: --exec CMD, --tcp HOST:PORT or --serial DEVICE"
        );
    };
    if let Some(mut c) = child {
        let _ = c.kill();
        let _ = c.wait();
    }
    let res = res?;
    let wall = t0.elapsed().as_secs_f64();
    eprintln!(
        "{} samples of {} s in closed loop in {:.2} s wall-clock",
        res.samples, man.step, wall
    );
    if res.max_cycles > 0 {
        eprintln!(
            "controller step: {:.0} cycles on average, {} at most",
            res.mean_cycles, res.max_cycles
        );
    }
    match output {
        Some(path) => {
            let mut f = std::fs::File::create(path)?;
            f.write_all(res.to_csv().as_bytes())?;
            eprintln!("wrote {path}");
        }
        None => print!("{}", res.to_csv()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::split_command;

    #[test]
    fn splits_quoted_commands() {
        assert_eq!(
            split_command(r#"qemu -drive "file=a b.bin,if=mtd" -x"#),
            vec!["qemu", "-drive", "file=a b.bin,if=mtd", "-x"]
        );
        assert_eq!(split_command("  a  ''  b "), vec!["a", "", "b"]);
    }
}
