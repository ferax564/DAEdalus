//! Native implementations of the external C functions of the Modelica Standard
//! Library (`ModelicaRandom`, `ModelicaFFT`, `ModelicaInternal`, `ModelicaIO`,
//! `ModelicaStrings`), so that the same code runs natively and in WebAssembly.
//!
//! The algorithms follow the reference implementations in
//! `Modelica/Resources/C-Sources` (Copyright (C) Modelica Association and
//! contributors, BSD-3-Clause; the xorshift generators are by Sebastiano Vigna,
//! public domain; the string hash is Arash Partow's AP hash, MIT licence).
//!
//! A function with numeric arguments and results is a [`NativeFn`]: it becomes a
//! [`crate::expr::LoopProg`] with a `native` body, called through the ordinary
//! [`crate::expr::Node::Call`] machinery, so it is folded at translation time when
//! its arguments are constant and evaluated in the tape otherwise. String arguments
//! are translation-time constants carried in [`NativeCall::strs`]. Functions with
//! hidden state or side effects ([`NativeFn::impure`]) keep it in [`ExtState`] and
//! are never folded or cached.

use std::fmt::Write as _;

/// A natively implemented external function with numeric arguments and results.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NativeFn {
    /// `ModelicaRandom_xorshift64star(stateIn[2]) -> (stateOut[2], y)`.
    Xorshift64star,
    /// `ModelicaRandom_xorshift128plus(stateIn[4]) -> (stateOut[4], y)`.
    Xorshift128plus,
    /// `ModelicaRandom_xorshift1024star(stateIn[33]) -> (stateOut[33], y)`.
    Xorshift1024star,
    /// `ModelicaRandom_impureRandom_xorshift1024star(id)`; the arguments are
    /// `[id, initial state (33)]`: the state passed to `setInternalState`.
    ImpureRandom,
    /// `ModelicaRandom_automaticGlobalSeed(dummy) -> seed`.
    AutomaticGlobalSeed,
    /// `ModelicaInternal_getTime() -> (ms, sec, min, hour, day, mon, year)`.
    GetTime,
    /// `ModelicaInternal_getpid() -> pid`.
    GetPid,
    /// `ModelicaFFT_kiss_fftr(u[nu], nu, nwork) -> (info, work, amplitudes, phases)`.
    RealFft,
    /// `ModelicaInternal_print(string, fileName)` (strings only).
    Print,
    /// `ModelicaStreams_closeFile(fileName)`.
    CloseFile,
    /// `ModelicaInternal_removeFile(fileName)`.
    RemoveFile,
    /// `ModelicaIO_writeRealMatrix(fileName, matrixName, matrix, m, n, append, format)`:
    /// strings `[fileName, matrixName, format]`, numbers `[matrix (row-major), m, n, append]`.
    WriteRealMatrix,
    /// `ModelicaIO_readMatrixSizes(fileName, matrixName) -> dim[2]`.
    ReadMatrixSizes,
    /// `ModelicaIO_readRealMatrix(fileName, matrixName, nrow, ncol, verbose) -> matrix`.
    ReadRealMatrix,
}

impl NativeFn {
    /// The native function behind the C function `cname`.
    pub fn lookup(cname: &str) -> Option<NativeFn> {
        Some(match cname {
            "ModelicaRandom_xorshift64star" => NativeFn::Xorshift64star,
            "ModelicaRandom_xorshift128plus" => NativeFn::Xorshift128plus,
            "ModelicaRandom_xorshift1024star" => NativeFn::Xorshift1024star,
            "ModelicaRandom_impureRandom_xorshift1024star" => NativeFn::ImpureRandom,
            "ModelicaRandom_automaticGlobalSeed" => NativeFn::AutomaticGlobalSeed,
            "ModelicaInternal_getTime" => NativeFn::GetTime,
            "ModelicaInternal_getpid" => NativeFn::GetPid,
            "ModelicaFFT_kiss_fftr" => NativeFn::RealFft,
            "ModelicaInternal_print" => NativeFn::Print,
            "ModelicaStreams_closeFile" => NativeFn::CloseFile,
            "ModelicaInternal_removeFile" => NativeFn::RemoveFile,
            "ModelicaIO_writeRealMatrix" => NativeFn::WriteRealMatrix,
            "ModelicaIO_readMatrixSizes" => NativeFn::ReadMatrixSizes,
            "ModelicaIO_readRealMatrix" => NativeFn::ReadRealMatrix,
            _ => return None,
        })
    }

    /// Functions with hidden state or side effects: every call is executed (never
    /// folded, shared or cached).
    pub fn impure(self) -> bool {
        matches!(
            self,
            NativeFn::ImpureRandom
                | NativeFn::AutomaticGlobalSeed
                | NativeFn::GetTime
                | NativeFn::Print
                | NativeFn::CloseFile
                | NativeFn::RemoveFile
                | NativeFn::WriteRealMatrix
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            NativeFn::Xorshift64star => "xorshift64star",
            NativeFn::Xorshift128plus => "xorshift128plus",
            NativeFn::Xorshift1024star => "xorshift1024star",
            NativeFn::ImpureRandom => "impureRandom",
            NativeFn::AutomaticGlobalSeed => "automaticGlobalSeed",
            NativeFn::GetTime => "getTime",
            NativeFn::GetPid => "getpid",
            NativeFn::RealFft => "realFFT",
            NativeFn::Print => "print",
            NativeFn::CloseFile => "closeFile",
            NativeFn::RemoveFile => "removeFile",
            NativeFn::WriteRealMatrix => "writeRealMatrix",
            NativeFn::ReadMatrixSizes => "readMatrixSizes",
            NativeFn::ReadRealMatrix => "readRealMatrix",
        }
    }
}

/// One use of a native function: the function, its constant string arguments and the
/// number of numeric results.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct NativeCall {
    pub f: NativeFn,
    pub strs: Vec<String>,
    pub n_out: usize,
}

/// Hidden state of the impure external functions and the messages they print.
#[derive(Debug, Clone, Default)]
pub struct ExtState {
    /// Impure xorshift1024* generators by initial state.
    rng: Vec<(Vec<u64>, [u64; 16], i32)>,
    /// Lines printed with `print` (to the terminal) or reported as errors.
    pub log: Vec<String>,
}

impl ExtState {
    /// Take the messages printed since the last call.
    pub fn take_log(&mut self) -> Vec<String> {
        std::mem::take(&mut self.log)
    }
}

/// Evaluate `call` on `args`; the results go to `out` (resized to `call.n_out`).
/// Errors are reported as `Err` (at translation time) or logged by the caller.
pub fn eval(
    call: &NativeCall,
    args: &[f64],
    out: &mut Vec<f64>,
    st: &mut ExtState,
) -> Result<(), String> {
    out.clear();
    out.resize(call.n_out, 0.0);
    let i32_of = |x: f64| x as i64 as i32;
    match call.f {
        NativeFn::Xorshift64star | NativeFn::Xorshift128plus | NativeFn::Xorshift1024star => {
            let n = match call.f {
                NativeFn::Xorshift64star => 2,
                NativeFn::Xorshift128plus => 4,
                _ => 33,
            };
            if args.len() < n || out.len() < n + 1 {
                return Err(format!("{}: wrong number of arguments", call.f.name()));
            }
            let s: Vec<i32> = args[..n].iter().map(|&x| i32_of(x)).collect();
            let (so, y) = match call.f {
                NativeFn::Xorshift64star => xorshift64star(&s),
                NativeFn::Xorshift128plus => xorshift128plus(&s),
                _ => xorshift1024star(&s),
            };
            for (o, v) in out.iter_mut().zip(&so) {
                *o = f64::from(*v);
            }
            out[n] = y;
        }
        NativeFn::ImpureRandom => {
            let init = args.get(1..34).ok_or("impureRandom: missing state")?;
            let key: Vec<u64> = init.iter().map(|x| x.to_bits()).collect();
            if init.iter().all(|&x| x == 0.0) {
                return Err(
                    "Function impureRandom not initialized with function initializeImpureRandom"
                        .into(),
                );
            }
            let k = match st.rng.iter().position(|(k, ..)| *k == key) {
                Some(k) => k,
                None => {
                    let ints: Vec<i32> = init.iter().map(|&x| i32_of(x)).collect();
                    let (s, p) = unpack1024(&ints);
                    st.rng.push((key, s, p));
                    st.rng.len() - 1
                }
            };
            let (_, s, p) = &mut st.rng[k];
            out[0] = xorshift1024star_internal(s, p);
        }
        NativeFn::AutomaticGlobalSeed => {
            let t = local_time();
            let pid = process_id();
            let seed = 1i64
                + i64::from(t[0])
                + 1000 * i64::from(t[1])
                + 1000 * 60 * i64::from(t[2])
                + 1000 * 60 * 60 * i64::from(t[3])
                + 6007 * i64::from(pid);
            out[0] = f64::from(seed as i32);
        }
        NativeFn::GetTime => {
            for (o, v) in out.iter_mut().zip(local_time()) {
                *o = f64::from(v);
            }
        }
        NativeFn::GetPid => out[0] = f64::from(process_id()),
        NativeFn::RealFft => {
            // [u.., nu, nwork] -> [info, work.., amplitudes.., phases..]
            if args.len() < 2 {
                return Err("realFFT: wrong number of arguments".into());
            }
            let nwork = args[args.len() - 1] as usize;
            let nu = args[args.len() - 2] as usize;
            let u = &args[..args.len() - 2];
            let nf = nu / 2 + 1;
            let info = if nu % 2 != 0 {
                1
            } else if nwork < 3 * nu + 2 * nf {
                2
            } else if u.len() != nu || out.len() < 1 + nwork + 2 * nf {
                3
            } else {
                let (amp, ph) = real_fft(u);
                let a0 = 1 + nwork;
                out[a0..a0 + nf].copy_from_slice(&amp);
                out[a0 + nf..a0 + 2 * nf].copy_from_slice(&ph);
                0
            };
            out[0] = f64::from(info);
        }
        NativeFn::Print => {
            let s = call.strs.first().map_or("", |s| s.as_str());
            let file = call.strs.get(1).map_or("", |s| s.as_str());
            if file.is_empty() {
                st.log.push(s.to_string());
            } else {
                let mut text = s.to_string();
                text.push('\n');
                fs::write(file, text.as_bytes(), true)
                    .map_err(|e| format!("print: cannot write to file \"{file}\": {e}"))?;
            }
        }
        NativeFn::CloseFile => {}
        NativeFn::RemoveFile => {
            let file = call.strs.first().map_or("", |s| s.as_str());
            fs::remove(file).map_err(|e| format!("removeFile: \"{file}\": {e}"))?;
        }
        NativeFn::WriteRealMatrix => {
            let file = call.strs.first().map_or("", |s| s.as_str());
            let name = call.strs.get(1).map_or("", |s| s.as_str());
            let format = call.strs.get(2).map_or("4", |s| s.as_str());
            if args.len() < 3 {
                return Err("writeRealMatrix: wrong number of arguments".into());
            }
            let k = args.len();
            let (m, n, append) = (
                args[k - 3] as usize,
                args[k - 2] as usize,
                args[k - 1] != 0.0,
            );
            let data = &args[..k - 3];
            if data.len() != m * n {
                return Err("writeRealMatrix: matrix size mismatch".into());
            }
            matio::write(file, name, m, n, data, append, format)?;
            out[0] = 1.0;
        }
        NativeFn::ReadMatrixSizes => {
            let file = call.strs.first().map_or("", |s| s.as_str());
            let name = call.strs.get(1).map_or("", |s| s.as_str());
            let (m, n, _) = matio::read(file, name, false)?;
            out[0] = m as f64;
            out[1] = n as f64;
        }
        NativeFn::ReadRealMatrix => {
            let file = call.strs.first().map_or("", |s| s.as_str());
            let name = call.strs.get(1).map_or("", |s| s.as_str());
            let (m, n, data) = matio::read(file, name, true)?;
            let (rm, rn) = (
                args.first().copied().unwrap_or(0.0) as usize,
                args.get(1).copied().unwrap_or(0.0) as usize,
            );
            if (m, n) != (rm, rn) {
                return Err(format!(
                    "Cannot read matrix \"{name}({rm},{rn})\" from file \"{file}\": it has size ({m},{n})"
                ));
            }
            if args.get(2).is_some_and(|&v| v != 0.0) {
                st.log
                    .push(format!("... loading \"{name}\" from \"{file}\""));
            }
            out[..m * n].copy_from_slice(&data);
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------
// Random number generators (ModelicaRandom.c)
// ----------------------------------------------------------------------------

/// `2^-64`.
const INVM64: f64 = 5.421_010_862_427_522e-20;

/// A 64-bit integer as a double in (0, 1] (`ModelicaRandom_RAND`).
fn rand_of(x: u64) -> f64 {
    (x as i64) as f64 * INVM64 + 0.5
}

fn pack(lo: i32, hi: i32) -> u64 {
    u64::from(lo as u32) | (u64::from(hi as u32) << 32)
}

fn unpack(x: u64) -> (i32, i32) {
    (x as u32 as i32, (x >> 32) as u32 as i32)
}

/// xorshift64*: new state and the random number.
pub fn xorshift64star(s: &[i32]) -> (Vec<i32>, f64) {
    let mut x = pack(s[0], s[1]);
    x ^= x >> 12;
    x ^= x << 25;
    x ^= x >> 27;
    x = x.wrapping_mul(2_685_821_657_736_338_717);
    let (a, b) = unpack(x);
    (vec![a, b], rand_of(x))
}

/// xorshift128+: new state and the random number.
pub fn xorshift128plus(s: &[i32]) -> (Vec<i32>, f64) {
    let mut s1 = pack(s[0], s[1]);
    let s0 = pack(s[2], s[3]);
    let n0 = s0;
    s1 ^= s1 << 23;
    let n1 = (s1 ^ s0 ^ (s1 >> 17) ^ (s0 >> 26)).wrapping_add(s0);
    let (a, b) = unpack(n0);
    let (c, d) = unpack(n1);
    (vec![a, b, c, d], rand_of(n1))
}

fn unpack1024(s: &[i32]) -> ([u64; 16], i32) {
    let mut st = [0u64; 16];
    for (i, v) in st.iter_mut().enumerate() {
        *v = pack(s[2 * i], s[2 * i + 1]);
    }
    (st, s[32])
}

fn xorshift1024star_internal(s: &mut [u64; 16], p: &mut i32) -> f64 {
    *p &= 15;
    let mut s0 = s[*p as usize];
    *p = (*p + 1) & 15;
    let mut s1 = s[*p as usize];
    s1 ^= s1 << 31;
    s1 ^= s1 >> 11;
    s0 ^= s0 >> 30;
    s[*p as usize] = s0 ^ s1;
    rand_of(s[*p as usize].wrapping_mul(1_181_783_497_276_652_981))
}

/// xorshift1024*: new state (33 integers, the last one the index) and the random number.
pub fn xorshift1024star(s: &[i32]) -> (Vec<i32>, f64) {
    let (mut st, mut p) = unpack1024(s);
    let y = xorshift1024star_internal(&mut st, &mut p);
    let mut out = Vec::with_capacity(33);
    for v in st {
        let (a, b) = unpack(v);
        out.push(a);
        out.push(b);
    }
    out.push(p);
    (out, y)
}

/// `ModelicaStrings_hashString`: AP hash of the bytes of `s`.
pub fn hash_string(s: &str) -> i32 {
    let mut h: u32 = 0xAAAA_AAAA;
    for (i, &c) in s.as_bytes().iter().enumerate() {
        let c = u32::from(c);
        h ^= if i & 1 == 0 {
            (h << 7) ^ c.wrapping_mul(h >> 3)
        } else {
            !((h << 11).wrapping_add(c ^ (h >> 5)))
        };
    }
    h as i32
}

// ----------------------------------------------------------------------------
// FFT (ModelicaFFT.c)
// ----------------------------------------------------------------------------

/// Complex discrete Fourier transform `X[k] = sum x[n] exp(-2 pi i k n / N)` by a
/// recursive mixed-radix decimation in time (radix = smallest prime factor).
fn dft(re: &[f64], im: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let n = re.len();
    if n <= 1 {
        return (re.to_vec(), im.to_vec());
    }
    let mut p = 2;
    while p * p <= n && n % p != 0 {
        p += 1;
    }
    if p * p > n {
        p = n;
    }
    let m = n / p;
    if m == 1 {
        // Direct DFT of a prime length.
        let mut xr = vec![0.0; n];
        let mut xi = vec![0.0; n];
        for k in 0..n {
            let (mut sr, mut si) = (0.0, 0.0);
            for j in 0..n {
                let a = -2.0 * std::f64::consts::PI * ((k * j) % n) as f64 / n as f64;
                let (s, c) = a.sin_cos();
                sr += re[j] * c - im[j] * s;
                si += re[j] * s + im[j] * c;
            }
            xr[k] = sr;
            xi[k] = si;
        }
        return (xr, xi);
    }
    // p interleaved sub-sequences of length m.
    let subs: Vec<(Vec<f64>, Vec<f64>)> = (0..p)
        .map(|r| {
            let sr: Vec<f64> = (0..m).map(|j| re[j * p + r]).collect();
            let si: Vec<f64> = (0..m).map(|j| im[j * p + r]).collect();
            dft(&sr, &si)
        })
        .collect();
    let mut xr = vec![0.0; n];
    let mut xi = vec![0.0; n];
    for k in 0..n {
        let (mut sr, mut si) = (0.0, 0.0);
        for (r, (yr, yi)) in subs.iter().enumerate() {
            let a = -2.0 * std::f64::consts::PI * ((r * k) % n) as f64 / n as f64;
            let (s, c) = a.sin_cos();
            let (u, v) = (yr[k % m], yi[k % m]);
            sr += u * c - v * s;
            si += u * s + v * c;
        }
        xr[k] = sr;
        xi[k] = si;
    }
    (xr, xi)
}

/// Amplitudes and phases of the real FFT of `u` as `ModelicaFFT_kiss_fftr` returns
/// them: `|X[k]| / nf` and `atan2(Im X[k], Re X[k])` for `k = 0 .. nu/2`.
pub fn real_fft(u: &[f64]) -> (Vec<f64>, Vec<f64>) {
    let nu = u.len();
    let nf = nu / 2 + 1;
    let (xr, xi) = dft(u, &vec![0.0; nu]);
    let mut amp = Vec::with_capacity(nf);
    let mut ph = Vec::with_capacity(nf);
    for k in 0..nf {
        let (r, i) = (
            xr.get(k).copied().unwrap_or(0.0),
            xi.get(k).copied().unwrap_or(0.0),
        );
        amp.push((r * r + i * i).sqrt() / nf as f64);
        ph.push(i.atan2(r));
    }
    (amp, ph)
}

// ----------------------------------------------------------------------------
// Time, process id, file system
// ----------------------------------------------------------------------------

/// `[ms, sec, min, hour, day, mon, year]` of the current (UTC) time.
pub fn local_time() -> [i32; 7] {
    let t = crate::backend::web_time_now();
    let secs = t.floor() as i64;
    let ms = ((t - t.floor()) * 1000.0) as i32;
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (h, mi, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil date from days since 1970-01-01 (H. Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    [
        ms, s as i32, mi as i32, h as i32, d as i32, m as i32, y as i32,
    ]
}

pub fn process_id() -> i32 {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::process::id() as i32
    }
    #[cfg(target_arch = "wasm32")]
    {
        0
    }
}

/// File access: the real file system natively; in WebAssembly an in-memory file
/// system (files written during the session can be read back).
pub mod fs {
    #[cfg(target_arch = "wasm32")]
    thread_local! {
        static FILES: std::cell::RefCell<rustc_hash::FxHashMap<String, Vec<u8>>> =
            Default::default();
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn read(path: &str) -> Result<Vec<u8>, String> {
        std::fs::read(path).map_err(|e| e.to_string())
    }

    #[cfg(target_arch = "wasm32")]
    pub fn read(path: &str) -> Result<Vec<u8>, String> {
        FILES
            .with(|f| f.borrow().get(path).cloned())
            .ok_or_else(|| {
                "file not found (no file system in the browser; only files written by \
             the model can be read)"
                    .to_string()
            })
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn write(path: &str, data: &[u8], append: bool) -> Result<(), String> {
        use std::io::Write;
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(path)
            .map_err(|e| e.to_string())?;
        f.write_all(data).map_err(|e| e.to_string())
    }

    #[cfg(target_arch = "wasm32")]
    pub fn write(path: &str, data: &[u8], append: bool) -> Result<(), String> {
        FILES.with(|f| {
            let mut f = f.borrow_mut();
            let e = f.entry(path.to_string()).or_default();
            if !append {
                e.clear();
            }
            e.extend_from_slice(data);
        });
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn remove(path: &str) -> Result<(), String> {
        match std::fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn remove(path: &str) -> Result<(), String> {
        FILES.with(|f| f.borrow_mut().remove(path));
        Ok(())
    }

    /// `1` no file, `2` regular file, `3` directory, `4` other (`Types.FileType`).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn stat(path: &str) -> i32 {
        match std::fs::metadata(path) {
            Ok(m) if m.is_file() => 2,
            Ok(m) if m.is_dir() => 3,
            Ok(_) => 4,
            Err(_) => 1,
        }
    }

    #[cfg(target_arch = "wasm32")]
    pub fn stat(path: &str) -> i32 {
        if FILES.with(|f| f.borrow().contains_key(path)) {
            2
        } else {
            1
        }
    }

    /// Current working directory (with `/` separators and no trailing slash).
    pub fn cwd() -> String {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::env::current_dir()
                .map(|p| p.to_string_lossy().replace('\\', "/"))
                .unwrap_or_else(|_| ".".into())
        }
        #[cfg(target_arch = "wasm32")]
        {
            "/".into()
        }
    }

    /// Absolute path of `name` (`ModelicaInternal_fullPathName`), normalising `.`/`..`.
    pub fn full_path(name: &str) -> String {
        let name = name.replace('\\', "/");
        let abs = if name.starts_with('/') || name.as_bytes().get(1) == Some(&b':') {
            name
        } else {
            format!("{}/{}", cwd().trim_end_matches('/'), name)
        };
        let mut parts: Vec<&str> = Vec::new();
        let trailing = abs.ends_with('/');
        for p in abs.split('/') {
            match p {
                "" | "." => {}
                ".." => {
                    parts.pop();
                }
                _ => parts.push(p),
            }
        }
        let mut s = String::new();
        if !abs.as_bytes().get(1).is_some_and(|&c| c == b':') {
            s.push('/');
        }
        s.push_str(&parts.join("/"));
        if trailing && !s.ends_with('/') {
            s.push('/');
        }
        s
    }

    pub fn getenv(name: &str) -> Option<String> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            std::env::var(name).ok()
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = name;
            None
        }
    }
}

/// The lines of a text file (`readFile`, `readLine`, `countLines`): split at `\n`,
/// a trailing `\r` removed, no empty last line after a final newline.
pub fn read_lines(path: &str) -> Result<Vec<String>, String> {
    let bytes = fs::read(path).map_err(|e| format!("Not possible to open file \"{path}\": {e}"))?;
    let text = String::from_utf8_lossy(&bytes);
    let mut lines: Vec<String> = text
        .split('\n')
        .map(|l| l.strip_suffix('\r').unwrap_or(l).to_string())
        .collect();
    if text.ends_with('\n') || text.is_empty() {
        lines.pop();
    }
    Ok(lines)
}

// ----------------------------------------------------------------------------
// String scanning (ModelicaStrings.c)
// ----------------------------------------------------------------------------

/// Byte at 1-based index `i` (0 past the end), as the C functions see the string.
fn at(s: &[u8], i: i64) -> u8 {
    if i >= 1 && (i as usize) <= s.len() {
        s[i as usize - 1]
    } else {
        0
    }
}

pub fn skip_white_space(s: &str, start: i64) -> i64 {
    let b = s.as_bytes();
    let mut i = start.max(1);
    while at(b, i) != 0 && at(b, i).is_ascii_whitespace() {
        i += 1;
    }
    i
}

fn match_unsigned(b: &[u8], start: i64) -> i64 {
    let mut k = 0;
    while at(b, start + k).is_ascii_digit() {
        k += 1;
    }
    k
}

/// `scanReal`: (nextIndex, number); nextIndex = start on failure.
pub fn scan_real(s: &str, start: i64, unsigned: bool) -> (i64, f64) {
    let b = s.as_bytes();
    let ts = skip_white_space(s, start);
    let mut total = 0;
    let c = at(b, ts);
    if c == b'+' || c == b'-' {
        if unsigned {
            return (start, 0.0);
        }
        total = 1;
    }
    total += match_unsigned(b, ts + total);
    if at(b, ts + total) == b'.' {
        total += 1;
        total += match_unsigned(b, ts + total);
    }
    let c = at(b, ts + total);
    if c == b'e' || c == b'E' {
        let mut el = 1;
        let c2 = at(b, ts + total + 1);
        if c2 == b'+' || c2 == b'-' {
            el += 1;
        }
        let len = match_unsigned(b, ts + total + el);
        if len == 0 {
            return (start, 0.0);
        }
        total += el + len;
    }
    if total > 0 && total < 100 {
        let tok = &s[(ts - 1) as usize..(ts - 1 + total) as usize];
        let tok = if tok.ends_with('.') {
            format!("{tok}0")
        } else {
            tok.to_string()
        };
        if let Ok(x) = tok.parse::<f64>() {
            return (ts + total, x);
        }
    }
    (start, 0.0)
}

/// `scanInteger`: (nextIndex, number); nextIndex = start on failure.
pub fn scan_integer(s: &str, start: i64, unsigned: bool) -> (i64, i64) {
    let b = s.as_bytes();
    let ts = skip_white_space(s, start);
    let c = at(b, ts);
    let sign = i64::from(c == b'+' || c == b'-');
    if !unsigned || sign == 0 {
        let len = match_unsigned(b, ts + sign);
        if len > 0 && sign + len < 100 {
            let next = at(b, ts + sign + len);
            if next != b'.' && next != b'e' && next != b'E' {
                let tok = &s[(ts - 1) as usize..(ts - 1 + sign + len) as usize];
                if let Ok(x) = tok.parse::<i64>() {
                    return (ts + sign + len, x as i32 as i64);
                }
            }
        }
    }
    (start, 0)
}

/// `scanString`: (nextIndex, string) of a `"..."` token.
pub fn scan_string(s: &str, start: i64) -> (i64, String) {
    let b = s.as_bytes();
    let ts = skip_white_space(s, start);
    if at(b, ts) != b'"' {
        return (start, String::new());
    }
    let mut i = ts + 1;
    loop {
        let c = at(b, i);
        if c == 0 {
            return (start, String::new());
        }
        if !(at(b, i - 1) == b'\\' && c == b'"') && c == b'"' {
            break;
        }
        i += 1;
    }
    let past = i + 1;
    let len = past - ts - 2;
    if len > 0 {
        let t = String::from_utf8_lossy(&b[ts as usize..(ts + len) as usize]).into_owned();
        return (past, t);
    }
    (start, String::new())
}

/// `scanIdentifier`: (nextIndex, identifier).
pub fn scan_identifier(s: &str, start: i64) -> (i64, String) {
    let b = s.as_bytes();
    let ts = skip_white_space(s, start);
    if at(b, ts).is_ascii_alphabetic() {
        let mut len = 1;
        loop {
            let c = at(b, ts + len);
            if c != 0 && (c.is_ascii_alphanumeric() || c == b'_') {
                len += 1;
            } else {
                break;
            }
        }
        let t = String::from_utf8_lossy(&b[(ts - 1) as usize..(ts - 1 + len) as usize]);
        return (ts + len, t.into_owned());
    }
    (start, String::new())
}

// ----------------------------------------------------------------------------
// MATLAB MAT files (ModelicaIO.c): levels 4 and 5 (v6 and compressed v7).
// ----------------------------------------------------------------------------

pub mod matio {
    use super::fs;

    fn u32_at(b: &[u8], o: usize) -> Option<u32> {
        Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
    }

    /// Read matrix `name` of a MAT file: (rows, columns, row-major data). With
    /// `data == false` only the size is returned.
    pub fn read(file: &str, name: &str, data: bool) -> Result<(usize, usize, Vec<f64>), String> {
        let bytes =
            fs::read(file).map_err(|e| format!("Not possible to open file \"{file}\": {e}"))?;
        let found = if bytes.len() >= 128 && bytes.starts_with(b"MATLAB") {
            if bytes.get(0..19) == Some(b"MATLAB 7.3 MAT-file".as_slice()) {
                return Err(format!(
                    "Not possible to read file \"{file}\": MATLAB v7.3 (HDF5) files are not supported"
                ));
            }
            read_v5(&bytes, name, data)?
        } else {
            read_v4(&bytes, name, data)?
        };
        found.ok_or_else(|| format!("Matrix \"{name}\" not found on file \"{file}\""))
    }

    type Found = Option<(usize, usize, Vec<f64>)>;

    /// Column-major `m x n` to row-major.
    fn transpose(m: usize, n: usize, cm: &[f64]) -> Vec<f64> {
        let mut out = vec![0.0; m * n];
        for i in 0..m {
            for j in 0..n {
                out[i * n + j] = cm[j * m + i];
            }
        }
        out
    }

    fn read_v4(b: &[u8], name: &str, data: bool) -> Result<Found, String> {
        let mut o = 0;
        while o + 20 <= b.len() {
            let h: Vec<i32> = (0..5)
                .map(|k| i32::from_le_bytes(b[o + 4 * k..o + 4 * k + 4].try_into().unwrap()))
                .collect();
            let (ty, m, n, imag, nlen) = (h[0], h[1] as usize, h[2] as usize, h[3], h[4] as usize);
            if !(0..10_000).contains(&ty) || ty / 1000 != 0 {
                return Err("unsupported MAT v4 file (only little-endian files are read)".into());
            }
            let p = (ty / 10) % 10;
            let size = match p {
                0 => 8,
                1 => 4,
                2 => 4,
                3 | 4 => 2,
                5 => 1,
                _ => return Err("invalid MAT v4 file".into()),
            };
            o += 20;
            let nm = b.get(o..o + nlen).ok_or("truncated MAT v4 file")?;
            let nm = String::from_utf8_lossy(nm.split(|&c| c == 0).next().unwrap_or(&[]));
            o += nlen;
            let count = m * n * if imag != 0 { 2 } else { 1 };
            let body = b.get(o..o + count * size).ok_or("truncated MAT v4 file")?;
            if nm == name {
                if !data {
                    return Ok(Some((m, n, Vec::new())));
                }
                let mut cm = Vec::with_capacity(m * n);
                for k in 0..m * n {
                    let s = &body[k * size..(k + 1) * size];
                    cm.push(match p {
                        0 => f64::from_le_bytes(s.try_into().unwrap()),
                        1 => f64::from(f32::from_le_bytes(s.try_into().unwrap())),
                        2 => f64::from(i32::from_le_bytes(s.try_into().unwrap())),
                        3 => f64::from(i16::from_le_bytes(s.try_into().unwrap())),
                        4 => f64::from(u16::from_le_bytes(s.try_into().unwrap())),
                        _ => f64::from(s[0]),
                    });
                }
                return Ok(Some((m, n, transpose(m, n, &cm))));
            }
            o += count * size;
        }
        Ok(None)
    }

    fn read_v5(b: &[u8], name: &str, data: bool) -> Result<Found, String> {
        if b.get(126..128) != Some(b"IM".as_slice()) {
            return Err("unsupported MAT file (only little-endian files are read)".into());
        }
        let mut o = 128;
        while o + 8 <= b.len() {
            let ty = u32_at(b, o).unwrap();
            let nb = u32_at(b, o + 4).unwrap() as usize;
            let body = b.get(o + 8..o + 8 + nb).ok_or("truncated MAT file")?;
            o += 8 + nb;
            o = (o + 7) & !7;
            let elem: Vec<u8>;
            let el = match ty {
                15 => {
                    elem = miniz_oxide::inflate::decompress_to_vec_zlib(body)
                        .map_err(|e| format!("corrupt compressed MAT data: {e:?}"))?;
                    let t = u32_at(&elem, 0).unwrap_or(0);
                    if t != 14 {
                        continue;
                    }
                    let n = u32_at(&elem, 4).unwrap_or(0) as usize;
                    elem.get(8..8 + n).ok_or("truncated compressed MAT data")?
                }
                14 => body,
                _ => continue,
            };
            if let Some(r) = parse_matrix(el, name, data)? {
                return Ok(Some(r));
            }
        }
        Ok(None)
    }

    /// Sub-elements of a miMATRIX: (type, data) in order.
    fn sub_elements(b: &[u8]) -> Vec<(u32, &[u8])> {
        let mut out = Vec::new();
        let mut o = 0;
        while o + 8 <= b.len() {
            let w = u32_at(b, o).unwrap();
            if w >> 16 != 0 {
                // small data element
                let (ty, n) = (w & 0xffff, (w >> 16) as usize);
                out.push((ty, &b[o + 4..(o + 4 + n).min(b.len())]));
                o += 8;
            } else {
                let n = u32_at(b, o + 4).unwrap() as usize;
                let end = (o + 8 + n).min(b.len());
                out.push((w, &b[o + 8..end]));
                o = (o + 8 + n + 7) & !7;
            }
        }
        out
    }

    fn numbers(ty: u32, b: &[u8]) -> Result<Vec<f64>, String> {
        let size = match ty {
            1 | 2 => 1,
            3 | 4 => 2,
            5..=7 => 4,
            9 | 12 | 13 => 8,
            _ => return Err(format!("unsupported MAT data type {ty}")),
        };
        Ok(b.chunks_exact(size)
            .map(|s| match ty {
                1 => f64::from(s[0] as i8),
                2 => f64::from(s[0]),
                3 => f64::from(i16::from_le_bytes(s.try_into().unwrap())),
                4 => f64::from(u16::from_le_bytes(s.try_into().unwrap())),
                5 => f64::from(i32::from_le_bytes(s.try_into().unwrap())),
                6 => f64::from(u32::from_le_bytes(s.try_into().unwrap())),
                7 => f64::from(f32::from_le_bytes(s.try_into().unwrap())),
                9 => f64::from_le_bytes(s.try_into().unwrap()),
                12 => i64::from_le_bytes(s.try_into().unwrap()) as f64,
                _ => u64::from_le_bytes(s.try_into().unwrap()) as f64,
            })
            .collect())
    }

    fn parse_matrix(el: &[u8], name: &str, data: bool) -> Result<Found, String> {
        let subs = sub_elements(el);
        if subs.len() < 3 {
            return Ok(None);
        }
        let nm = String::from_utf8_lossy(subs[2].1);
        if nm.trim_end_matches('\0') != name {
            return Ok(None);
        }
        let class = subs[0].1.first().copied().unwrap_or(0);
        if !(6..=15).contains(&class) {
            return Err(format!("matrix \"{name}\" is not numeric"));
        }
        let dims = numbers(subs[1].0, subs[1].1)?;
        if dims.len() != 2 {
            return Err(format!("matrix \"{name}\" is not two-dimensional"));
        }
        let (m, n) = (dims[0] as usize, dims[1] as usize);
        if !data {
            return Ok(Some((m, n, Vec::new())));
        }
        let re = subs.get(3).ok_or("missing matrix data")?;
        let cm = numbers(re.0, re.1)?;
        if cm.len() != m * n {
            return Err(format!("matrix \"{name}\" has inconsistent data"));
        }
        Ok(Some((m, n, transpose(m, n, &cm))))
    }

    fn pad8(v: &mut Vec<u8>) {
        while v.len() % 8 != 0 {
            v.push(0);
        }
    }

    /// The miMATRIX element of a double matrix (row-major `data`).
    fn v5_matrix(name: &str, m: usize, n: usize, data: &[f64]) -> Vec<u8> {
        let mut body = Vec::new();
        // array flags: mxDOUBLE_CLASS
        body.extend_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(&8u32.to_le_bytes());
        body.extend_from_slice(&6u32.to_le_bytes());
        body.extend_from_slice(&0u32.to_le_bytes());
        // dimensions
        body.extend_from_slice(&5u32.to_le_bytes());
        body.extend_from_slice(&8u32.to_le_bytes());
        body.extend_from_slice(&(m as i32).to_le_bytes());
        body.extend_from_slice(&(n as i32).to_le_bytes());
        // name
        body.extend_from_slice(&1u32.to_le_bytes());
        body.extend_from_slice(&(name.len() as u32).to_le_bytes());
        body.extend_from_slice(name.as_bytes());
        pad8(&mut body);
        // real part, column-major
        body.extend_from_slice(&9u32.to_le_bytes());
        body.extend_from_slice(&((m * n * 8) as u32).to_le_bytes());
        for j in 0..n {
            for i in 0..m {
                body.extend_from_slice(&data[i * n + j].to_le_bytes());
            }
        }
        let mut el = Vec::with_capacity(body.len() + 8);
        el.extend_from_slice(&14u32.to_le_bytes());
        el.extend_from_slice(&(body.len() as u32).to_le_bytes());
        el.extend_from_slice(&body);
        el
    }

    /// Write (or append) matrix `name` to a MAT file of version "4", "6" or "7".
    pub fn write(
        file: &str,
        name: &str,
        m: usize,
        n: usize,
        data: &[f64],
        append: bool,
        version: &str,
    ) -> Result<(), String> {
        let err = |e: String| format!("Not possible to write file \"{file}\": {e}");
        match version {
            "4" => {
                let mut b = Vec::new();
                for v in [0i32, m as i32, n as i32, 0, name.len() as i32 + 1] {
                    b.extend_from_slice(&v.to_le_bytes());
                }
                b.extend_from_slice(name.as_bytes());
                b.push(0);
                for j in 0..n {
                    for i in 0..m {
                        b.extend_from_slice(&data[i * n + j].to_le_bytes());
                    }
                }
                let append = append && fs::stat(file) == 2;
                fs::write(file, &b, append).map_err(err)
            }
            "6" | "7" => {
                let mut el = v5_matrix(name, m, n, data);
                if version == "7" {
                    let z = miniz_oxide::deflate::compress_to_vec_zlib(&el, 6);
                    el = Vec::with_capacity(z.len() + 16);
                    el.extend_from_slice(&15u32.to_le_bytes());
                    el.extend_from_slice(&(z.len() as u32).to_le_bytes());
                    el.extend_from_slice(&z);
                    pad8(&mut el);
                }
                let append = append && fs::stat(file) == 2;
                let mut b = Vec::new();
                if !append {
                    let mut h = b"MATLAB 5.0 MAT-file, Platform: DAEdalus, Created by: DAEdalus \
                        (Modelica.Utilities.Streams.writeRealMatrix)"
                        .to_vec();
                    h.resize(116, b' ');
                    b.extend_from_slice(&h);
                    b.extend_from_slice(&[0u8; 8]);
                    b.extend_from_slice(&0x0100u16.to_le_bytes());
                    b.extend_from_slice(b"IM");
                }
                b.extend_from_slice(&el);
                fs::write(file, &b, append).map_err(err)
            }
            other => Err(format!("Invalid version {other} for file \"{file}\"")),
        }
    }
}

/// Readable text of a native call (for diagnostics).
pub fn describe(call: &NativeCall) -> String {
    let mut s = String::from(call.f.name());
    if !call.strs.is_empty() {
        let _ = write!(s, "{:?}", call.strs);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_matches_direct_dft() {
        let u: Vec<f64> = (0..30)
            .map(|k| (k as f64 * 0.37).sin() + 0.1 * k as f64)
            .collect();
        let (amp, ph) = real_fft(&u);
        let n = u.len();
        let nf = n / 2 + 1;
        for k in 0..nf {
            let (mut r, mut i) = (0.0, 0.0);
            for (j, &x) in u.iter().enumerate() {
                let a = -2.0 * std::f64::consts::PI * (k * j) as f64 / n as f64;
                r += x * a.cos();
                i += x * a.sin();
            }
            assert!((amp[k] - (r * r + i * i).sqrt() / nf as f64).abs() < 1e-10);
            if (r * r + i * i).sqrt() > 1e-9 {
                // Modulo 2 pi: a real X (k = 0 or n/2) may come out at +pi or -pi.
                let d = ph[k] - i.atan2(r);
                let d = d - (d / std::f64::consts::TAU).round() * std::f64::consts::TAU;
                assert!(d.abs() < 1e-9, "phase {k}: {} vs {}", ph[k], i.atan2(r));
            }
        }
    }

    #[test]
    fn mat_roundtrip() {
        let dir = std::env::temp_dir();
        for v in ["4", "6", "7"] {
            let f = dir.join(format!("daedalus_mat_{v}_{}.mat", std::process::id()));
            let f = f.to_string_lossy().to_string();
            let a = [11.0, 12.0, 21.0, 22.0, 31.0, 32.0];
            matio::write(&f, "A", 3, 2, &a, false, v).unwrap();
            matio::write(&f, "B", 1, 2, &[1.0, 2.0], true, v).unwrap();
            assert_eq!(matio::read(&f, "A", true).unwrap(), (3, 2, a.to_vec()));
            assert_eq!(matio::read(&f, "B", true).unwrap(), (1, 2, vec![1.0, 2.0]));
            let _ = std::fs::remove_file(&f);
        }
    }

    #[test]
    fn scanning() {
        assert_eq!(scan_real("  1.5e3 x", 1, false), (8, 1500.0));
        assert_eq!(scan_real(" -2.", 1, false), (5, -2.0));
        assert_eq!(scan_real(" -2", 1, true), (1, 0.0));
        assert_eq!(scan_integer(" 42;", 1, false), (4, 42));
        assert_eq!(scan_integer(" 4.2", 1, false), (1, 0));
        assert_eq!(scan_string(" \"ab\" ", 1), (6, "ab".into()));
        assert_eq!(scan_identifier(" a_1+", 1), (5, "a_1".into()));
    }
}
