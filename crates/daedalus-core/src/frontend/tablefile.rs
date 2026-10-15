//! Reading tables from files, as the MSL `ModelicaIO_readRealTable2` does for
//! `CombiTimeTable`, `CombiTable1D*` and `CombiTable2D*` with `tableOnFile = true`.
//!
//! Supported formats (chosen by the file extension like the C code):
//! - text files (`#1` header, `double name(rows,cols)` blocks, `#` comments, rows that
//!   span several lines),
//! - CSV files (`.csv`: one table per file, column delimiter, header lines to skip),
//! - MATLAB MAT-files (`.mat`) version 4, 5/6 (uncompressed) and 7 (zlib-compressed
//!   variables), including fields of 1x1 structs (`"s.field"`). Version 7.3 (HDF5)
//!   files are rejected with an error.
//!
//! The parsing follows `ModelicaIO.c` of the Modelica Standard Library
//! (BSD-3-Clause, Copyright (C) 2008-2020 Modelica Association and contributors).

use anyhow::{Result, anyhow, bail};

/// A matrix read from a file: `rows x cols`, stored row-wise.
#[derive(Debug, Clone, PartialEq)]
pub struct Matrix {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
}

/// Read matrix `name` from `path` (`delimiter` and `header_lines` apply to CSV files).
#[cfg(not(target_arch = "wasm32"))]
pub fn read_table(path: &str, name: &str, delimiter: &str, header_lines: usize) -> Result<Matrix> {
    let bytes = std::fs::read(path)
        .map_err(|e| anyhow!("not possible to open table file \"{path}\": {e}"))?;
    parse_table(path, &bytes, name, delimiter, header_lines)
}

/// Tables cannot be read from files in the browser build.
#[cfg(target_arch = "wasm32")]
pub fn read_table(path: &str, name: &str, _: &str, _: usize) -> Result<Matrix> {
    bail!(
        "table \"{name}\" cannot be read from file \"{path}\": there is no file system \
         in the browser; give the table as a matrix instead"
    )
}

/// Parse the contents of table file `path` (the extension selects the format).
pub fn parse_table(
    path: &str,
    bytes: &[u8],
    name: &str,
    delimiter: &str,
    header_lines: usize,
) -> Result<Matrix> {
    let ext = path.rfind('.').map(|i| &path[i..]).unwrap_or("");
    let m = if ext.starts_with(".mat") || ext.starts_with(".MAT") {
        read_mat(bytes, name)
    } else if ext.starts_with(".csv") || ext.starts_with(".CSV") {
        read_csv(bytes, delimiter, header_lines)
    } else {
        read_txt(bytes, name)
    };
    m.map_err(|e| anyhow!("reading table \"{name}\" from file \"{path}\": {e}"))
}

/// C `strtod` on a whole token (the token must be a number and nothing else).
fn number(tok: &str) -> Option<f64> {
    let t = tok.trim_start_matches('+');
    if t.is_empty() {
        return None;
    }
    let lower = t.trim_start_matches('-').to_ascii_lowercase();
    if lower.starts_with("inf") || lower.starts_with("nan") {
        return match lower.as_str() {
            "inf" | "infinity" => Some(if t.starts_with('-') {
                f64::NEG_INFINITY
            } else {
                f64::INFINITY
            }),
            "nan" => Some(f64::NAN),
            _ => None,
        };
    }
    t.parse::<f64>().ok()
}

/// Text lines of a file (`\n` separated; `\r` is treated as white space by the tokenizers).
fn lines(bytes: &[u8]) -> Vec<String> {
    let text = String::from_utf8_lossy(bytes);
    let mut v: Vec<String> = text.split('\n').map(str::to_string).collect();
    if v.last().is_some_and(|l| l.is_empty()) {
        v.pop();
    }
    v
}

/// The MSL text format: `#1` on the first line, then `double name(rows, cols)` headers
/// each followed by the rows of that matrix.
fn read_txt(bytes: &[u8], name: &str) -> Result<Matrix> {
    let ls = lines(bytes);
    let first = ls.first().map(String::as_str).unwrap_or("");
    let first = first.strip_prefix('\u{feff}').unwrap_or(first);
    if !first.starts_with("#1") {
        bail!("\"#1\" expected in the first line (format and version information)");
    }
    let header_delims = |c: char| matches!(c, ' ' | '\t' | '(' | ',' | ')' | '\r');
    let number_delims = |c: char| matches!(c, ' ' | '\t' | ',' | ';' | '\r');
    let mut found = false;
    let mut k = 1;
    while k < ls.len() {
        let line = &ls[k];
        k += 1;
        let mut toks = line.split(header_delims).filter(|t| !t.is_empty());
        if !matches!(toks.next(), Some("double" | "float")) {
            continue;
        }
        if toks.next() != Some(name) {
            continue;
        }
        found = true;
        let dims: Option<(usize, usize)> = (|| {
            let r = toks.next()?.parse().ok()?;
            let c = toks.next()?.parse().ok()?;
            Some((r, c))
        })();
        let Some((rows, cols)) = dims else {
            continue;
        };
        let mut data = Vec::with_capacity(rows * cols);
        let n = rows * cols;
        while data.len() < n {
            let Some(line) = ls.get(k) else {
                bail!(
                    "end of file reached when reading numeric data of matrix \"{name}({rows},{cols})\""
                );
            };
            k += 1;
            let body = line.trim_start_matches([' ', '\t']);
            if body.is_empty() || body.starts_with('#') || body.trim().is_empty() {
                continue;
            }
            for tok in body.split(number_delims).filter(|t| !t.is_empty()) {
                if tok.starts_with('#') {
                    break;
                }
                if data.len() == n {
                    if number(tok).is_some() {
                        bail!(
                            "the table dimensions of matrix \"{name}({rows},{cols})\" do not \
                             match the actual table size (line {k})"
                        );
                    }
                    bail!(
                        "error in line {k} when reading numeric data of matrix \"{name}({rows},{cols})\""
                    );
                }
                let v = number(tok).ok_or_else(|| {
                    anyhow!("error in line {k} when reading numeric data of matrix \"{name}({rows},{cols})\"")
                })?;
                data.push(v);
            }
        }
        return Ok(Matrix { rows, cols, data });
    }
    if found {
        bail!("invalid table dimensions of matrix \"{name}\"");
    }
    bail!("table matrix \"{name}\" not found")
}

/// A CSV file holds one table; `header_lines` lines are skipped first.
fn read_csv(bytes: &[u8], delimiter: &str, header_lines: usize) -> Result<Matrix> {
    let mut dch = delimiter.chars();
    let (Some(d), None) = (dch.next(), dch.next()) else {
        bail!("invalid column delimiter \"{delimiter}\", must be a single character");
    };
    let ls = lines(bytes);
    if ls.len() < header_lines {
        bail!("end of file reached in the {header_lines} header lines");
    }
    let delim = |c: char| matches!(c, ' ' | '\t' | '\r') || c == d;
    let mut rows: Vec<&str> = ls[header_lines..].iter().map(String::as_str).collect();
    while rows.last().is_some_and(|l| l.trim().is_empty()) {
        rows.pop();
    }
    let cols = rows
        .first()
        .map(|l| l.split(delim).filter(|t| !t.is_empty()).count())
        .unwrap_or(0);
    let mut data = Vec::with_capacity(rows.len() * cols);
    for (i, l) in rows.iter().enumerate() {
        let line_no = header_lines + i + 1;
        let mut toks = l.split(delim).filter(|t| !t.is_empty());
        for _ in 0..cols {
            let v = toks
                .next()
                .and_then(number)
                .ok_or_else(|| anyhow!("error in line {line_no} when reading numeric data"))?;
            data.push(v);
        }
    }
    Ok(Matrix {
        rows: rows.len(),
        cols,
        data,
    })
}

// ----- MATLAB MAT-files -----

fn read_mat(bytes: &[u8], name: &str) -> Result<Matrix> {
    if bytes.len() >= 128 && bytes.starts_with(b"MATLAB") {
        let big = match &bytes[126..128] {
            b"IM" => Some(false),
            b"MI" => Some(true),
            _ => None,
        };
        if let Some(big) = big {
            let version = if big {
                u16::from_be_bytes([bytes[124], bytes[125]])
            } else {
                u16::from_le_bytes([bytes[124], bytes[125]])
            };
            if version == 0x0200 {
                bail!(
                    "MATLAB v7.3 MAT-files (HDF5) are not supported; save the file in \
                     the v7, v6 or v4 format"
                );
            }
            return read_mat5(bytes, big, name);
        }
    }
    read_mat4(bytes, name)
}

/// Column-major `rows x cols` values to a row-wise [`Matrix`].
fn from_column_major(rows: usize, cols: usize, v: &[f64]) -> Matrix {
    let mut data = vec![0.0; rows * cols];
    for j in 0..cols {
        for i in 0..rows {
            data[i * cols + j] = v[j * rows + i];
        }
    }
    Matrix { rows, cols, data }
}

/// Level 4 MAT-file: a sequence of matrices, each with a 20-byte header.
fn read_mat4(bytes: &[u8], name: &str) -> Result<Matrix> {
    let mut p = 0;
    while p + 20 <= bytes.len() {
        let h = |k: usize, big: bool| {
            let b = [
                bytes[p + 4 * k],
                bytes[p + 4 * k + 1],
                bytes[p + 4 * k + 2],
                bytes[p + 4 * k + 3],
            ];
            if big {
                i32::from_be_bytes(b)
            } else {
                i32::from_le_bytes(b)
            }
        };
        // The type MOPT is at most 4052; its byte order tells the file's.
        let big = !(0..=4052).contains(&h(0, false));
        let ty = h(0, big);
        if !(0..=4052).contains(&ty) {
            bail!("not a MATLAB MAT-file (unknown format)");
        }
        let (mrows, ncols, imagf, namlen) = (h(1, big), h(2, big), h(3, big), h(4, big));
        if mrows < 0 || ncols < 0 || namlen < 1 {
            bail!("corrupt MATLAB v4 MAT-file");
        }
        let (mrows, ncols, namlen) = (mrows as usize, ncols as usize, namlen as usize);
        let prec = (ty / 10) % 10;
        let kind = ty % 10;
        let size = match prec {
            0 => 8,
            1 | 2 => 4,
            3 | 4 => 2,
            5 => 1,
            _ => bail!("unknown precision in MATLAB v4 MAT-file"),
        };
        p += 20;
        let nm = bytes
            .get(p..p + namlen)
            .ok_or_else(|| anyhow!("truncated MATLAB v4 MAT-file"))?;
        let nm = String::from_utf8_lossy(nm.split(|&b| b == 0).next().unwrap_or(&[]));
        p += namlen;
        let n = mrows * ncols;
        let len = n * size * if imagf != 0 { 2 } else { 1 };
        let data = bytes
            .get(p..p + len)
            .ok_or_else(|| anyhow!("truncated MATLAB v4 MAT-file"))?;
        p += len;
        if nm != name {
            continue;
        }
        if kind == 2 {
            bail!("matrix \"{name}\" is sparse");
        }
        if imagf != 0 {
            bail!("matrix \"{name}\" must not be complex");
        }
        let get = |b: &[u8]| -> f64 {
            macro_rules! rd {
                ($t:ty) => {{
                    let a = b.try_into().unwrap();
                    (if big {
                        <$t>::from_be_bytes(a)
                    } else {
                        <$t>::from_le_bytes(a)
                    }) as f64
                }};
            }
            match prec {
                0 => rd!(f64),
                1 => rd!(f32),
                2 => rd!(i32),
                3 => rd!(i16),
                4 => rd!(u16),
                _ => b[0] as f64,
            }
        };
        let v: Vec<f64> = data[..n * size].chunks_exact(size).map(get).collect();
        return Ok(from_column_major(mrows, ncols, &v));
    }
    bail!("variable \"{name}\" not found")
}

const MI_INT8: u32 = 1;
const MI_UINT8: u32 = 2;
const MI_INT16: u32 = 3;
const MI_UINT16: u32 = 4;
const MI_INT32: u32 = 5;
const MI_UINT32: u32 = 6;
const MI_SINGLE: u32 = 7;
const MI_DOUBLE: u32 = 9;
const MI_INT64: u32 = 12;
const MI_UINT64: u32 = 13;
const MI_MATRIX: u32 = 14;
const MI_COMPRESSED: u32 = 15;
const MX_STRUCT: u32 = 2;

/// A Level 5 data element: its type and data bytes.
struct Element<'b> {
    ty: u32,
    data: &'b [u8],
}

struct Reader<'b> {
    b: &'b [u8],
    p: usize,
    big: bool,
}

impl<'b> Reader<'b> {
    fn u32_at(&self, p: usize) -> Result<u32> {
        let s = self
            .b
            .get(p..p + 4)
            .ok_or_else(|| anyhow!("truncated MAT-file"))?;
        let a = [s[0], s[1], s[2], s[3]];
        Ok(if self.big {
            u32::from_be_bytes(a)
        } else {
            u32::from_le_bytes(a)
        })
    }

    fn done(&self) -> bool {
        self.p + 8 > self.b.len()
    }

    /// The next element; `pad` rounds its size up to 8 bytes (sub-elements of a matrix).
    fn next(&mut self, pad: bool) -> Result<Element<'b>> {
        let tag = self.u32_at(self.p)?;
        if tag >> 16 != 0 {
            // small data element: type and size in one word, data in the next four bytes
            let (ty, n) = (tag & 0xffff, (tag >> 16) as usize);
            let data = self
                .b
                .get(self.p + 4..self.p + 4 + n.min(4))
                .ok_or_else(|| anyhow!("truncated MAT-file"))?;
            self.p += 8;
            return Ok(Element { ty, data });
        }
        let n = self.u32_at(self.p + 4)? as usize;
        let start = self.p + 8;
        let data = self
            .b
            .get(start..start + n)
            .ok_or_else(|| anyhow!("truncated MAT-file"))?;
        self.p = start + if pad { n.div_ceil(8) * 8 } else { n };
        Ok(Element { ty: tag, data })
    }
}

/// A parsed `miMATRIX`: class, dimensions, name, and either numbers or struct fields.
struct MatVar {
    class: u32,
    complex: bool,
    dims: Vec<usize>,
    name: String,
    real: Vec<f64>,
    /// Struct arrays: field names and, per element, the field values.
    fields: Vec<String>,
    values: Vec<MatVar>,
}

fn numbers(e: &Element, big: bool) -> Result<Vec<f64>> {
    let size = match e.ty {
        MI_INT8 | MI_UINT8 => 1,
        MI_INT16 | MI_UINT16 => 2,
        MI_INT32 | MI_UINT32 | MI_SINGLE => 4,
        MI_DOUBLE | MI_INT64 | MI_UINT64 => 8,
        t => bail!("unsupported MAT-file data type {t}"),
    };
    let ty = e.ty;
    Ok(e.data
        .chunks_exact(size)
        .map(|b| {
            macro_rules! rd {
                ($t:ty) => {{
                    let a = b.try_into().unwrap();
                    (if big {
                        <$t>::from_be_bytes(a)
                    } else {
                        <$t>::from_le_bytes(a)
                    }) as f64
                }};
            }
            match ty {
                MI_INT8 => b[0] as i8 as f64,
                MI_UINT8 => b[0] as f64,
                MI_INT16 => rd!(i16),
                MI_UINT16 => rd!(u16),
                MI_INT32 => rd!(i32),
                MI_UINT32 => rd!(u32),
                MI_SINGLE => rd!(f32),
                MI_INT64 => rd!(i64),
                MI_UINT64 => rd!(u64),
                _ => rd!(f64),
            }
        })
        .collect())
}

/// Parse the body of a `miMATRIX` element.
fn mat_var(data: &[u8], big: bool) -> Result<MatVar> {
    let mut r = Reader { b: data, p: 0, big };
    let mut v = MatVar {
        class: 0,
        complex: false,
        dims: Vec::new(),
        name: String::new(),
        real: Vec::new(),
        fields: Vec::new(),
        values: Vec::new(),
    };
    if data.is_empty() {
        return Ok(v);
    }
    let flags = r.next(true)?;
    let fl = numbers(&flags, big)?;
    let f0 = fl.first().copied().unwrap_or(0.0) as u32;
    v.class = f0 & 0xff;
    v.complex = f0 & 0x0800 != 0;
    let dims = r.next(true)?;
    v.dims = numbers(&dims, big)?
        .into_iter()
        .map(|d| d as usize)
        .collect();
    let name = r.next(true)?;
    v.name = String::from_utf8_lossy(name.data)
        .trim_end_matches('\0')
        .to_string();
    match v.class {
        6..=15 => {
            let re = r.next(true)?;
            v.real = numbers(&re, big)?;
        }
        MX_STRUCT => {
            let len = numbers(&r.next(true)?, big)?
                .first()
                .copied()
                .unwrap_or(0.0) as usize;
            let names = r.next(true)?;
            if len > 0 {
                v.fields = names
                    .data
                    .chunks(len)
                    .map(|c| {
                        String::from_utf8_lossy(c)
                            .trim_end_matches('\0')
                            .to_string()
                    })
                    .collect();
            }
            let n: usize = v.dims.iter().product();
            for _ in 0..n * v.fields.len() {
                let e = r.next(true)?;
                if e.ty != MI_MATRIX {
                    bail!("corrupt struct array in MAT-file");
                }
                v.values.push(mat_var(e.data, big)?);
            }
        }
        _ => {}
    }
    Ok(v)
}

/// Level 5 MAT-file (v5/v6 and the zlib-compressed variables of v7).
fn read_mat5(bytes: &[u8], big: bool, name: &str) -> Result<Matrix> {
    let mut parts = name.split('.');
    let top = parts.next().unwrap_or("");
    let mut r = Reader {
        b: bytes,
        p: 128,
        big,
    };
    let mut var = None;
    while !r.done() {
        let e = r.next(false)?;
        let inflated;
        let (ty, data) = if e.ty == MI_COMPRESSED {
            inflated = inflate(e.data)?;
            let mut ir = Reader {
                b: &inflated,
                p: 0,
                big,
            };
            let ie = ir.next(false)?;
            (ie.ty, ie.data.to_vec())
        } else {
            (e.ty, e.data.to_vec())
        };
        if ty != MI_MATRIX {
            continue;
        }
        let v = mat_var(&data, big)?;
        if v.name == top {
            var = Some(v);
            break;
        }
    }
    let mut v = var.ok_or_else(|| anyhow!("variable \"{top}\" not found"))?;
    for field in parts {
        if v.class != MX_STRUCT {
            bail!(
                "variable \"{}\" of \"{name}\" is not a struct array",
                v.name
            );
        }
        if v.dims.len() != 2 || v.dims[0] != 1 || v.dims[1] != 1 {
            bail!(
                "variable \"{}\" of \"{name}\" is not a 1x1 struct array",
                v.name
            );
        }
        let k = v
            .fields
            .iter()
            .position(|f| f == field)
            .ok_or_else(|| anyhow!("variable \"{name}\" not found"))?;
        let mut f = v.values.swap_remove(k);
        f.name = field.to_string();
        v = f;
    }
    if v.dims.len() != 2 {
        bail!("variable \"{name}\" is not of rank 2");
    }
    if !(6..=15).contains(&v.class) {
        bail!("matrix \"{name}\" is not a numeric array");
    }
    if v.complex {
        bail!("matrix \"{name}\" must not be complex");
    }
    let (rows, cols) = (v.dims[0], v.dims[1]);
    if v.real.len() < rows * cols {
        bail!("error when reading numeric data of matrix \"{name}({rows},{cols})\"");
    }
    Ok(from_column_major(rows, cols, &v.real))
}

fn inflate(data: &[u8]) -> Result<Vec<u8>> {
    // A truncated stream still yields the variable when its data is complete, which
    // is how some writers end the last variable.
    match miniz_oxide::inflate::decompress_to_vec_zlib(data) {
        Ok(v) => Ok(v),
        Err(e) if !e.output.is_empty() => Ok(e.output),
        Err(e) => bail!("corrupt compressed variable in MAT-file: {e:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TXT: &str = "#1\n\
        double tab0(2,2)   # dummy table\n\
        0 0 1 1\n\
        double tab1(6,2)   # comment line\n\
        \t\t# comment line with leading white space\n\
        \x20 0   0  1   0 # two rows per line\n\
        \x20 1   1 # comment in line 9\n\
        \x20 2     # first part of row\n\
        \x20     4 # continued part of row\n\
        #\tnext line is empty\n\
        \n\
        \x20 3   9\r\n\
        \x20 4  16 \n";

    #[test]
    fn text_format() {
        let m = parse_table("t.txt", TXT.as_bytes(), "tab1", ",", 0).unwrap();
        assert_eq!((m.rows, m.cols), (6, 2));
        assert_eq!(
            m.data,
            vec![0., 0., 1., 0., 1., 1., 2., 4., 3., 9., 4., 16.]
        );
        let m = parse_table("t.txt", TXT.as_bytes(), "tab0", ",", 0).unwrap();
        assert_eq!(m.data, vec![0., 0., 1., 1.]);
        assert!(parse_table("t.txt", TXT.as_bytes(), "tab2", ",", 0).is_err());
        let bom = format!("\u{feff}{TXT}");
        assert!(parse_table("t.txt", bom.as_bytes(), "tab1", ",", 0).is_ok());
        assert!(parse_table("t.txt", b"double a(1,1)\n1\n", "a", ",", 0).is_err());
    }

    #[test]
    fn csv_format() {
        let s = "time, value\n0.0, 0.0\t\n1,1\n\n";
        let m = parse_table("t.csv", s.as_bytes(), "x", ",", 1).unwrap();
        assert_eq!(
            (m.rows, m.cols, m.data.clone()),
            (2, 2, vec![0., 0., 1., 1.])
        );
        let m = parse_table("t.CSV", b"1;2;3\n4;5;6\n", "", ";", 0).unwrap();
        assert_eq!((m.rows, m.cols), (2, 3));
        assert_eq!(m.data[5], 6.0);
        assert!(parse_table("t.csv", b"1,2\n3,x\n", "", ",", 0).is_err());
    }

    fn mat4(name: &str, rows: usize, cols: usize, colmajor: &[f64]) -> Vec<u8> {
        let mut b = Vec::new();
        for v in [0i32, rows as i32, cols as i32, 0, name.len() as i32 + 1] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b.extend_from_slice(name.as_bytes());
        b.push(0);
        for v in colmajor {
            b.extend_from_slice(&v.to_le_bytes());
        }
        b
    }

    #[test]
    fn mat_v4() {
        let mut f = mat4("dummy", 1, 1, &[7.0]);
        f.extend(mat4("tab", 2, 3, &[1., 4., 2., 5., 3., 6.]));
        let m = parse_table("t.mat", &f, "tab", ",", 0).unwrap();
        assert_eq!((m.rows, m.cols), (2, 3));
        assert_eq!(m.data, vec![1., 2., 3., 4., 5., 6.]);
        assert!(parse_table("t.mat", &f, "nope", ",", 0).is_err());
    }

    fn elem(ty: u32, data: &[u8]) -> Vec<u8> {
        let mut b = Vec::new();
        b.extend_from_slice(&ty.to_le_bytes());
        b.extend_from_slice(&(data.len() as u32).to_le_bytes());
        b.extend_from_slice(data);
        while b.len() % 8 != 0 {
            b.push(0);
        }
        b
    }

    fn matrix5(name: &str, class: u32, dims: &[i32], body: &[u8]) -> Vec<u8> {
        let mut m = Vec::new();
        let mut fl = Vec::new();
        fl.extend_from_slice(&class.to_le_bytes());
        fl.extend_from_slice(&0u32.to_le_bytes());
        m.extend(elem(MI_UINT32, &fl));
        let d: Vec<u8> = dims.iter().flat_map(|x| x.to_le_bytes()).collect();
        m.extend(elem(MI_INT32, &d));
        m.extend(elem(MI_INT8, name.as_bytes()));
        m.extend_from_slice(body);
        elem(MI_MATRIX, &m)
    }

    fn header5() -> Vec<u8> {
        let mut h = b"MATLAB 5.0 MAT-file, test".to_vec();
        h.resize(124, b' ');
        h.extend_from_slice(&0x0100u16.to_le_bytes());
        h.extend_from_slice(b"IM");
        h
    }

    #[test]
    fn mat_v5_v7_and_structs() {
        // a 2x2 double matrix stored as uint8 (as MATLAB does for small integers)
        let a = matrix5("a", 6, &[2, 2], &elem(MI_UINT8, &[1, 3, 2, 4]));
        let b: Vec<u8> = [0.5f64, 1.5, 2.5]
            .iter()
            .flat_map(|x| x.to_le_bytes())
            .collect();
        let b = matrix5("b", 6, &[1, 3], &elem(MI_DOUBLE, &b));
        // struct s with fields x (1x1) and b
        // field name length as a small element
        let mut body = Vec::new();
        body.extend_from_slice(&((4u32 << 16) | MI_INT32).to_le_bytes());
        body.extend_from_slice(&4i32.to_le_bytes());
        body.extend(elem(MI_INT8, b"x\0\0\0tab\0"));
        body.extend(matrix5(
            "",
            6,
            &[1, 1],
            &elem(MI_DOUBLE, &9.0f64.to_le_bytes()),
        ));
        let tab: Vec<u8> = [3.0f64, 4.0].iter().flat_map(|x| x.to_le_bytes()).collect();
        body.extend(matrix5("", 6, &[1, 2], &elem(MI_DOUBLE, &tab)));
        let s = matrix5("s", MX_STRUCT, &[1, 1], &body);
        let mut f = header5();
        f.extend(&a);
        f.extend(&s);
        f.extend(&b);
        let m = parse_table("t.mat", &f, "a", ",", 0).unwrap();
        assert_eq!(m.data, vec![1., 2., 3., 4.]);
        let m = parse_table("t.mat", &f, "b", ",", 0).unwrap();
        assert_eq!((m.rows, m.cols, m.data), (1, 3, vec![0.5, 1.5, 2.5]));
        let m = parse_table("t.mat", &f, "s.tab", ",", 0).unwrap();
        assert_eq!((m.rows, m.cols, m.data), (1, 2, vec![3.0, 4.0]));
        assert!(parse_table("t.mat", &f, "s.y", ",", 0).is_err());
        // v7: the same variables compressed
        let mut f7 = header5();
        for v in [&a, &s, &b] {
            let z = miniz_oxide::deflate::compress_to_vec_zlib(v, 6);
            f7.extend_from_slice(&MI_COMPRESSED.to_le_bytes());
            f7.extend_from_slice(&(z.len() as u32).to_le_bytes());
            f7.extend(z);
        }
        let m = parse_table("t.mat", &f7, "b", ",", 0).unwrap();
        assert_eq!(m.data, vec![0.5, 1.5, 2.5]);
        let m = parse_table("t.mat", &f7, "s.tab", ",", 0).unwrap();
        assert_eq!(m.data, vec![3.0, 4.0]);
        // v7.3 is rejected with a clear message
        let mut h = header5();
        h[124..126].copy_from_slice(&0x0200u16.to_le_bytes());
        let e = parse_table("t.mat", &h, "b", ",", 0).unwrap_err();
        assert!(format!("{e}").contains("7.3"), "{e}");
    }
}
