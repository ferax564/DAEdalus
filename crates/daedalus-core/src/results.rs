//! Simulation results: in-memory trajectories, CSV export and Dymola-compatible
//! `dsres.mat` (MATLAB v4, "binTrans") files that existing post-processing
//! scripts and tools can read unchanged.

use serde::Serialize;
use std::io::{self, Write};

#[derive(Clone, Debug, Serialize)]
pub enum ResultKind {
    /// Column of the trajectory matrix.
    Series(usize),
    /// Constant value (parameters).
    Param(f64),
    /// `sign * column`
    Alias { col: usize, sign: f64 },
}

#[derive(Clone, Debug, Serialize)]
pub struct ResultVar {
    pub name: String,
    pub description: String,
    pub unit: String,
    pub kind: ResultKind,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Results {
    pub model: String,
    pub vars: Vec<ResultVar>,
    pub time: Vec<f64>,
    /// Trajectories, one vector per stored column.
    pub series: Vec<Vec<f64>>,
    pub log: Vec<String>,
}

impl Results {
    pub fn find(&self, name: &str) -> Option<&ResultVar> {
        self.vars.iter().find(|v| v.name == name)
    }

    /// Trajectory of a variable sampled on the result time grid.
    pub fn values(&self, name: &str) -> Option<Vec<f64>> {
        let v = self.find(name)?;
        Some(self.values_of(v))
    }

    pub fn values_of(&self, v: &ResultVar) -> Vec<f64> {
        match v.kind {
            ResultKind::Series(c) => self.series[c].clone(),
            ResultKind::Param(x) => vec![x; self.time.len()],
            ResultKind::Alias { col, sign } => self.series[col].iter().map(|x| sign * x).collect(),
        }
    }

    /// Final value of a variable.
    pub fn final_value(&self, name: &str) -> Option<f64> {
        self.values(name).and_then(|v| v.last().copied())
    }

    /// Linear interpolation of a variable at time `t` (right value at events).
    pub fn at(&self, name: &str, t: f64) -> Option<f64> {
        let y = self.values(name)?;
        let ts = &self.time;
        if ts.is_empty() {
            return None;
        }
        let i = ts.partition_point(|&x| x <= t);
        if i == 0 {
            return Some(y[0]);
        }
        if i >= ts.len() {
            return Some(*y.last().unwrap());
        }
        let (t0, t1) = (ts[i - 1], ts[i]);
        if t1 == t0 {
            return Some(y[i]);
        }
        let w = (t - t0) / (t1 - t0);
        Some(y[i - 1] + w * (y[i] - y[i - 1]))
    }

    pub fn write_csv(&self, out: &mut dyn Write, names: Option<&[String]>) -> io::Result<()> {
        let sel: Vec<&ResultVar> = match names {
            Some(ns) => ns.iter().filter_map(|n| self.find(n)).collect(),
            None => self.vars.iter().collect(),
        };
        write!(out, "time")?;
        for v in &sel {
            write!(out, ",\"{}\"", v.name)?;
        }
        writeln!(out)?;
        let cols: Vec<Vec<f64>> = sel.iter().map(|v| self.values_of(v)).collect();
        let mut line = String::new();
        for (i, t) in self.time.iter().enumerate() {
            line.clear();
            line.push_str(&fmt_num(*t));
            for c in &cols {
                line.push(',');
                line.push_str(&fmt_num(c[i]));
            }
            writeln!(out, "{line}")?;
        }
        Ok(())
    }

    /// Write a Dymola-compatible result file (MATLAB v4 `binTrans` format).
    pub fn write_mat(&self, out: &mut dyn Write) -> io::Result<()> {
        let mut names = vec!["Time".to_string()];
        let mut descs = vec!["Time in [s]".to_string()];
        let mut info: Vec<[i32; 4]> = vec![[0, 1, 0, -1]];
        let mut data1: Vec<f64> = Vec::new();
        let mut n1 = 1; // first row of data_1 is time
        let n2_cols = self.series.len();
        for v in &self.vars {
            names.push(v.name.clone());
            let d = if v.unit.is_empty() { v.description.clone() } else { format!("{} [{}]", v.description, v.unit) };
            descs.push(d);
            match v.kind {
                ResultKind::Series(c) => info.push([2, c as i32 + 2, 0, -1]),
                ResultKind::Alias { col, sign } => {
                    let idx = col as i32 + 2;
                    info.push([2, if sign < 0.0 { -idx } else { idx }, 0, -1]);
                }
                ResultKind::Param(x) => {
                    n1 += 1;
                    info.push([1, n1, 0, 0]);
                    data1.push(x);
                }
            }
        }
        let t0 = self.time.first().copied().unwrap_or(0.0);
        let tf = self.time.last().copied().unwrap_or(0.0);
        // Aclass
        write_text(out, "Aclass", &["Atrajectory", "1.1", "", "binTrans"], false)?;
        let nr: Vec<&str> = names.iter().map(|s| s.as_str()).collect();
        write_text(out, "name", &nr, true)?;
        let dr: Vec<&str> = descs.iter().map(|s| s.as_str()).collect();
        write_text(out, "description", &dr, true)?;
        // dataInfo: int32, 4 rows x nvars (stored transposed: column per variable)
        write_header(out, 20, 4, info.len() as i32, "dataInfo")?;
        for row in &info {
            for x in row {
                out.write_all(&x.to_le_bytes())?;
            }
        }
        // data_1: n1 rows x 2 columns, column-major => all rows at t0 then all at tf.
        write_header(out, 0, n1, 2, "data_1")?;
        for t in [t0, tf] {
            out.write_all(&t.to_le_bytes())?;
            for x in &data1 {
                out.write_all(&x.to_le_bytes())?;
            }
        }
        // data_2: (1 + series) rows x n_time columns.
        write_header(out, 0, (n2_cols + 1) as i32, self.time.len() as i32, "data_2")?;
        let mut buf = Vec::with_capacity((n2_cols + 1) * 8);
        for (i, t) in self.time.iter().enumerate() {
            buf.clear();
            buf.extend_from_slice(&t.to_le_bytes());
            for c in &self.series {
                buf.extend_from_slice(&c[i].to_le_bytes());
            }
            out.write_all(&buf)?;
        }
        Ok(())
    }
}

fn fmt_num(x: f64) -> String {
    format!("{x}")
}

fn write_header(out: &mut dyn Write, ty: i32, mrows: i32, ncols: i32, name: &str) -> io::Result<()> {
    out.write_all(&ty.to_le_bytes())?;
    out.write_all(&mrows.to_le_bytes())?;
    out.write_all(&ncols.to_le_bytes())?;
    out.write_all(&0i32.to_le_bytes())?;
    out.write_all(&(name.len() as i32 + 1).to_le_bytes())?;
    out.write_all(name.as_bytes())?;
    out.write_all(&[0])?;
    Ok(())
}

/// Character matrix. `transposed`: one string per column (binTrans), else one per row.
fn write_text(out: &mut dyn Write, name: &str, rows: &[&str], transposed: bool) -> io::Result<()> {
    let len = rows.iter().map(|s| s.len()).max().unwrap_or(0).max(1);
    let n = rows.len();
    if transposed {
        write_header(out, 51, len as i32, n as i32, name)?;
        // column-major: column j = string j
        let mut buf = vec![b' '; len * n];
        for (j, s) in rows.iter().enumerate() {
            for (i, b) in s.bytes().enumerate() {
                buf[j * len + i] = b;
            }
        }
        out.write_all(&buf)
    } else {
        write_header(out, 51, n as i32, len as i32, name)?;
        let mut buf = vec![b' '; len * n];
        for (i, s) in rows.iter().enumerate() {
            for (j, b) in s.bytes().enumerate() {
                buf[j * n + i] = b;
            }
        }
        out.write_all(&buf)
    }
}

/// Read a Dymola/OpenModelica `.mat` result file (MATLAB v4, binTrans or binNormal).
pub fn read_mat(bytes: &[u8]) -> Result<Results, String> {
    struct Mat {
        ty: i32,
        rows: usize,
        cols: usize,
        data: Vec<f64>,
        text: Vec<u8>,
    }
    let mut pos = 0;
    let rd_i32 = |b: &[u8], p: usize| -> Result<i32, String> {
        b.get(p..p + 4).map(|s| i32::from_le_bytes(s.try_into().unwrap())).ok_or("truncated file".into())
    };
    let mut mats: Vec<(String, Mat)> = Vec::new();
    while pos + 20 <= bytes.len() {
        let ty = rd_i32(bytes, pos)?;
        let rows = rd_i32(bytes, pos + 4)? as usize;
        let cols = rd_i32(bytes, pos + 8)? as usize;
        let namelen = rd_i32(bytes, pos + 16)? as usize;
        pos += 20;
        let name = String::from_utf8_lossy(&bytes[pos..pos + namelen - 1]).to_string();
        pos += namelen;
        let p = (ty / 10) % 10;
        let size = match p {
            0 => 8,
            1 => 4,
            2 => 4,
            3 => 2,
            4 => 2,
            5 => 1,
            _ => return Err(format!("unsupported matrix type {ty}")),
        };
        let n = rows * cols;
        let raw = bytes.get(pos..pos + n * size).ok_or("truncated matrix")?;
        pos += n * size;
        let mut data = Vec::new();
        let mut text = Vec::new();
        if ty % 10 == 1 {
            text = raw.iter().map(|&c| if size == 1 { c } else { 0 }).collect();
            if size != 1 {
                for k in 0..n {
                    let v = f64::from_le_bytes(raw[k * 8..k * 8 + 8].try_into().unwrap());
                    text[k] = v as u8;
                }
            }
        } else {
            data = (0..n)
                .map(|k| {
                    let s = &raw[k * size..k * size + size];
                    match p {
                        0 => f64::from_le_bytes(s.try_into().unwrap()),
                        1 => f32::from_le_bytes(s.try_into().unwrap()) as f64,
                        2 => i32::from_le_bytes(s.try_into().unwrap()) as f64,
                        3 => i16::from_le_bytes(s.try_into().unwrap()) as f64,
                        4 => u16::from_le_bytes(s.try_into().unwrap()) as f64,
                        _ => s[0] as f64,
                    }
                })
                .collect();
        }
        mats.push((name, Mat { ty, rows, cols, data, text }));
    }
    let get = |n: &str| mats.iter().find(|(m, _)| m == n).map(|(_, m)| m);
    let aclass = get("Aclass").ok_or("not a trajectory file")?;
    let atext = |m: &Mat, idx: usize, transposed: bool| -> String {
        let (len, count) = if transposed { (m.rows, m.cols) } else { (m.cols, m.rows) };
        let _ = count;
        let s: Vec<u8> = (0..len)
            .map(|i| if transposed { m.text[idx * m.rows + i] } else { m.text[i * m.rows + idx] })
            .collect();
        String::from_utf8_lossy(&s).trim_end_matches([' ', '\0']).to_string()
    };
    let transposed = aclass.rows >= 4 && atext(aclass, 3, false) == "binTrans";
    let name_m = get("name").ok_or("missing name")?;
    let desc_m = get("description");
    let info = get("dataInfo").ok_or("missing dataInfo")?;
    let d1 = get("data_1");
    let d2 = get("data_2").ok_or("missing data_2")?;
    let nvars = if transposed { name_m.cols } else { name_m.rows };
    let info_at = |var: usize, k: usize| -> i32 {
        if transposed { info.data[var * info.rows + k] as i32 } else { info.data[k * info.rows + var] as i32 }
    };
    // data_2 accessor: (signal row, time col)
    let (n_sig2, n_t) = if transposed { (d2.rows, d2.cols) } else { (d2.cols, d2.rows) };
    let d2_at = |sig: usize, t: usize| -> f64 { if transposed { d2.data[t * d2.rows + sig] } else { d2.data[sig * d2.rows + t] } };
    let mut res = Results::default();
    res.time = (0..n_t).map(|t| d2_at(0, t)).collect();
    for s in 1..n_sig2 {
        res.series.push((0..n_t).map(|t| d2_at(s, t)).collect());
    }
    for v in 1..nvars {
        let name = atext(name_m, v, transposed);
        let description = desc_m.map(|m| atext(m, v, transposed)).unwrap_or_default();
        let which = info_at(v, 0);
        let idx = info_at(v, 1);
        let kind = if which == 2 || (which == 0 && idx.abs() > 1) {
            let col = idx.unsigned_abs() as usize - 2;
            if idx < 0 { ResultKind::Alias { col, sign: -1.0 } } else { ResultKind::Series(col) }
        } else if which == 1 {
            let d1 = d1.ok_or("missing data_1")?;
            let row = idx.unsigned_abs() as usize - 1;
            let x = if transposed { d1.data[row] } else { d1.data[row * d1.rows] };
            ResultKind::Param(if idx < 0 { -x } else { x })
        } else {
            continue;
        };
        res.vars.push(ResultVar { name, description, unit: String::new(), kind });
    }
    let _ = |m: &Mat| m.ty;
    Ok(res)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mat_roundtrip() {
        let r = Results {
            model: "M".into(),
            vars: vec![
                ResultVar { name: "x".into(), description: "pos".into(), unit: "m".into(), kind: ResultKind::Series(0) },
                ResultVar { name: "k".into(), description: "".into(), unit: "".into(), kind: ResultKind::Param(3.5) },
                ResultVar {
                    name: "y".into(),
                    description: "".into(),
                    unit: "".into(),
                    kind: ResultKind::Alias { col: 0, sign: -1.0 },
                },
            ],
            time: vec![0.0, 0.5, 1.0],
            series: vec![vec![1.0, 2.0, 3.0]],
            log: vec![],
        };
        let mut buf = Vec::new();
        r.write_mat(&mut buf).unwrap();
        let back = read_mat(&buf).unwrap();
        assert_eq!(back.time, r.time);
        assert_eq!(back.values("x").unwrap(), vec![1.0, 2.0, 3.0]);
        assert_eq!(back.values("y").unwrap(), vec![-1.0, -2.0, -3.0]);
        assert_eq!(back.final_value("k"), Some(3.5));
    }
}
