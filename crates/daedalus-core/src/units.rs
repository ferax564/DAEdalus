//! Units of measure: parsing Modelica unit strings, dimensional checking and
//! inference over a flat model, and display-unit conversion.
//!
//! **Unit strings** follow the grammar of MLS section 19.1 (`"kg.m2"`, `"rad/s"`,
//! `"W/(m2.K)"`, `"s-1"`, `"1"`), with the SI prefixes and a table of SI, accepted
//! non-SI and Imperial symbols. A parsed unit is a [`Unit`]: a dimension vector over
//! the seven SI base dimensions, a scale factor to SI and an offset (`degC`, `degF`).
//! The vector carries an eighth, *angle* exponent: radians are dimensionless for
//! checking (`rad/s` and `Hz` agree), but the angle exponent keeps `rad/s` from being
//! offered `Hz` as a display unit.
//!
//! **Checking** ([`check`]) is a separate pass over the [`FlatModel`]. It works on
//! dimensions only (scale is ignored, `mm` and `m` agree), never fails, and reports
//! [`UnitIssue`]s meant to be shown as warnings:
//! - the terms of a sum, the two sides of an equation, the branches of an `if`, the
//!   operands of a relation, `min`/`max` and `atan2` must agree;
//! - products and quotients combine, `der(x)` divides by seconds, `sqrt` halves;
//! - the arguments of `sin`, `exp`, ... and exponents must be dimensionless (`log`
//!   takes any unit: `log(p)` in property correlations means `log(p/unit)`);
//! - numeric literals are unit-polymorphic: they adapt in sums (`x + 1`), and a
//!   product with a literal factor (`m*v^2/2`, `1.5*R` meant as a voltage) has a
//!   *soft* unit that can infer units but is never reported. The frontend folds
//!   constants such as `Modelica.Constants.R` to literals, so this also covers them;
//! - functions are inlined by the frontend, so their bodies are checked with the
//!   units of the actual arguments; calls the inliner kept (loop programs), table
//!   look-ups, Integer/Boolean variables and every variable declared `unit="1"` are
//!   unit-polymorphic (MSL declares gains, ideal-switch and friction path parameters
//!   `"1"` and uses them to convert between units), and a sum with such a term is too;
//! - each branch of an if-equation is a separate equation;
//! - a clash by exactly the unit of a constant equal to 1 (`constant SI.Time
//!   unitTime = 1`, folded away by the frontend) is not reported.
//!
//! **Inference** fills in variables with an empty unit: whenever a sum has a term of
//! known dimension and another term whose dimension is `rest * unit(v)^k` for a
//! single unknown variable `v`, `unit(v)` follows. Passes repeat to a fixed point.
//! Branches of if-equations do not infer (a tank's `s` is a length in one branch and
//! a mass flow in the other). When a clash involves an inferred unit, the variables
//! inferred are made polymorphic and the check runs again, so only declared units
//! are reported against each other.

use crate::expr::{Cmp, ExprId, Fn1, Fn2, Node, VarId};
use crate::flat::{FlatModel, VarKind, VarType, WhenAction};
use rustc_hash::FxHashMap;
use serde::Serialize;
use std::collections::HashMap;

/// Number of base dimensions compared by the checker (m, kg, s, A, K, mol, cd).
pub const N_BASE: usize = 7;
/// Index of the angle pseudo-dimension (ignored by checking, kept for display).
const ANGLE: usize = 7;
const BASE_SYMBOLS: [&str; N_BASE] = ["m", "kg", "s", "A", "K", "mol", "cd"];

/// Exponents of m, kg, s, A, K, mol, cd and rad, in halves (so `sqrt` stays exact).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Dim(pub [i16; 8]);

impl Dim {
    pub const ONE: Dim = Dim([0; 8]);
    fn base(i: usize) -> Dim {
        let mut d = [0; 8];
        d[i] = 2;
        Dim(d)
    }
    fn mul(self, o: Dim) -> Dim {
        let mut d = self.0;
        for (a, b) in d.iter_mut().zip(o.0) {
            *a += b;
        }
        Dim(d)
    }
    fn div(self, o: Dim) -> Dim {
        let mut d = self.0;
        for (a, b) in d.iter_mut().zip(o.0) {
            *a -= b;
        }
        Dim(d)
    }
    /// Raise to the power `p` (None when an exponent would not be a multiple of 1/2).
    fn powf(self, p: f64) -> Option<Dim> {
        let mut d = [0i16; 8];
        for (o, &x) in d.iter_mut().zip(self.0.iter()) {
            let v = x as f64 * p;
            let r = v.round();
            if (v - r).abs() > 1e-9 || r.abs() > 1000.0 {
                return None;
            }
            *o = r as i16;
        }
        Some(Dim(d))
    }
    /// Equal in the seven checked dimensions (angles are dimensionless).
    pub fn same(self, o: Dim) -> bool {
        self.0[..N_BASE] == o.0[..N_BASE]
    }
    /// Dimensionless (angles count as dimensionless).
    pub fn is_one(self) -> bool {
        self.0[..N_BASE].iter().all(|&x| x == 0)
    }
}

/// A parsed unit: `value_SI = value * scale + offset`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Unit {
    pub dim: Dim,
    pub scale: f64,
    pub offset: f64,
}

impl Unit {
    const ONE: Unit = Unit {
        dim: Dim::ONE,
        scale: 1.0,
        offset: 0.0,
    };
}

// ------------------------------------------------------------------ symbol table

const PI: f64 = std::f64::consts::PI;

/// Symbols: (name, base-dimension exponents as whole numbers [m,kg,s,A,K,mol,cd,rad], scale, offset, prefixable).
#[rustfmt::skip]
const SYMBOLS: &[(&str, [i8; 8], f64, f64, bool)] = &[
    // SI base (the kilogram's symbol is "g" with the prefix k)
    ("m",   [1,0,0,0,0,0,0,0], 1.0, 0.0, true),
    ("g",   [0,1,0,0,0,0,0,0], 1e-3, 0.0, true),
    ("s",   [0,0,1,0,0,0,0,0], 1.0, 0.0, true),
    ("A",   [0,0,0,1,0,0,0,0], 1.0, 0.0, true),
    ("K",   [0,0,0,0,1,0,0,0], 1.0, 0.0, true),
    ("mol", [0,0,0,0,0,1,0,0], 1.0, 0.0, true),
    ("cd",  [0,0,0,0,0,0,1,0], 1.0, 0.0, true),
    // SI derived
    ("rad", [0,0,0,0,0,0,0,1], 1.0, 0.0, true),
    ("sr",  [0,0,0,0,0,0,0,0], 1.0, 0.0, true),
    ("Hz",  [0,0,-1,0,0,0,0,0], 1.0, 0.0, true),
    ("N",   [1,1,-2,0,0,0,0,0], 1.0, 0.0, true),
    ("Pa",  [-1,1,-2,0,0,0,0,0], 1.0, 0.0, true),
    ("J",   [2,1,-2,0,0,0,0,0], 1.0, 0.0, true),
    ("W",   [2,1,-3,0,0,0,0,0], 1.0, 0.0, true),
    ("C",   [0,0,1,1,0,0,0,0], 1.0, 0.0, true),
    ("V",   [2,1,-3,-1,0,0,0,0], 1.0, 0.0, true),
    ("F",   [-2,-1,4,2,0,0,0,0], 1.0, 0.0, true),
    ("Ohm", [2,1,-3,-2,0,0,0,0], 1.0, 0.0, true),
    ("Ω",   [2,1,-3,-2,0,0,0,0], 1.0, 0.0, true),
    ("S",   [-2,-1,3,2,0,0,0,0], 1.0, 0.0, true),
    ("Wb",  [2,1,-2,-1,0,0,0,0], 1.0, 0.0, true),
    ("T",   [0,1,-2,-1,0,0,0,0], 1.0, 0.0, true),
    ("H",   [2,1,-2,-2,0,0,0,0], 1.0, 0.0, true),
    ("lm",  [0,0,0,0,0,0,1,0], 1.0, 0.0, true),
    ("lx",  [-2,0,0,0,0,0,1,0], 1.0, 0.0, true),
    ("Bq",  [0,0,-1,0,0,0,0,0], 1.0, 0.0, true),
    ("Gy",  [2,0,-2,0,0,0,0,0], 1.0, 0.0, true),
    ("Sv",  [2,0,-2,0,0,0,0,0], 1.0, 0.0, true),
    ("kat", [0,0,-1,0,0,1,0,0], 1.0, 0.0, true),
    ("degC",[0,0,0,0,1,0,0,0], 1.0, 273.15, false),
    // accepted non-SI
    ("min", [0,0,1,0,0,0,0,0], 60.0, 0.0, false),
    ("h",   [0,0,1,0,0,0,0,0], 3600.0, 0.0, false),
    ("d",   [0,0,1,0,0,0,0,0], 86400.0, 0.0, false),
    ("l",   [3,0,0,0,0,0,0,0], 1e-3, 0.0, true),
    ("L",   [3,0,0,0,0,0,0,0], 1e-3, 0.0, true),
    ("t",   [0,1,0,0,0,0,0,0], 1e3, 0.0, false),
    ("bar", [-1,1,-2,0,0,0,0,0], 1e5, 0.0, true),
    ("atm", [-1,1,-2,0,0,0,0,0], 101325.0, 0.0, false),
    ("mmHg",[-1,1,-2,0,0,0,0,0], 133.322387415, 0.0, false),
    ("deg", [0,0,0,0,0,0,0,1], PI / 180.0, 0.0, false),
    ("rev", [0,0,0,0,0,0,0,1], 2.0 * PI, 0.0, false),
    ("rpm", [0,0,-1,0,0,0,0,1], 2.0 * PI / 60.0, 0.0, false),
    ("Wh",  [2,1,-2,0,0,0,0,0], 3600.0, 0.0, true),
    ("eV",  [2,1,-2,0,0,0,0,0], 1.602176634e-19, 0.0, true),
    ("cal", [2,1,-2,0,0,0,0,0], 4.184, 0.0, true),
    ("%",   [0,0,0,0,0,0,0,0], 0.01, 0.0, false),
    ("ppm", [0,0,0,0,0,0,0,0], 1e-6, 0.0, false),
    // Imperial / US customary
    ("in",  [1,0,0,0,0,0,0,0], 0.0254, 0.0, false),
    ("ft",  [1,0,0,0,0,0,0,0], 0.3048, 0.0, false),
    ("yd",  [1,0,0,0,0,0,0,0], 0.9144, 0.0, false),
    ("mi",  [1,0,0,0,0,0,0,0], 1609.344, 0.0, false),
    ("nmi", [1,0,0,0,0,0,0,0], 1852.0, 0.0, false),
    ("mph", [1,0,-1,0,0,0,0,0], 0.44704, 0.0, false),
    ("kn",  [1,0,-1,0,0,0,0,0], 1852.0 / 3600.0, 0.0, false),
    ("lb",  [0,1,0,0,0,0,0,0], 0.45359237, 0.0, false),
    ("lbm", [0,1,0,0,0,0,0,0], 0.45359237, 0.0, false),
    ("oz",  [0,1,0,0,0,0,0,0], 0.028349523125, 0.0, false),
    ("lbf", [1,1,-2,0,0,0,0,0], 4.4482216152605, 0.0, false),
    ("psi", [-1,1,-2,0,0,0,0,0], 6894.757293168, 0.0, false),
    ("hp",  [2,1,-3,0,0,0,0,0], 745.69987158227, 0.0, false),
    ("BTU", [2,1,-2,0,0,0,0,0], 1055.05585262, 0.0, false),
    ("Btu", [2,1,-2,0,0,0,0,0], 1055.05585262, 0.0, false),
    ("gal", [3,0,0,0,0,0,0,0], 3.785411784e-3, 0.0, false),
    ("degF",[0,0,0,0,1,0,0,0], 5.0 / 9.0, 459.67 * 5.0 / 9.0, false),
    ("degR",[0,0,0,0,1,0,0,0], 5.0 / 9.0, 0.0, false),
];

/// SI prefixes, longest first.
#[rustfmt::skip]
const PREFIXES: &[(&str, f64)] = &[
    ("da", 1e1), ("Y", 1e24), ("Z", 1e21), ("E", 1e18), ("P", 1e15), ("T", 1e12),
    ("G", 1e9), ("M", 1e6), ("k", 1e3), ("h", 1e2), ("d", 1e-1), ("c", 1e-2),
    ("m", 1e-3), ("u", 1e-6), ("µ", 1e-6), ("μ", 1e-6), ("n", 1e-9), ("p", 1e-12),
    ("f", 1e-15), ("a", 1e-18), ("z", 1e-21), ("y", 1e-24),
];

fn symbol(name: &str) -> Option<Unit> {
    let mk = |e: &[i8; 8], scale: f64, offset: f64| {
        let mut d = [0i16; 8];
        for (o, &x) in d.iter_mut().zip(e) {
            *o = 2 * x as i16;
        }
        Unit {
            dim: Dim(d),
            scale,
            offset,
        }
    };
    if let Some((_, e, s, o, _)) = SYMBOLS.iter().find(|s| s.0 == name) {
        return Some(mk(e, *s, *o));
    }
    for (p, f) in PREFIXES {
        if let Some(rest) = name.strip_prefix(p)
            && let Some((_, e, s, o, true)) = SYMBOLS.iter().find(|s| s.0 == rest)
        {
            return Some(mk(e, s * f, *o));
        }
    }
    None
}

// ------------------------------------------------------------------ parser

struct Parser<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.s.get(self.i).copied()
    }
    fn err<T>(&self, what: &str) -> Result<T, String> {
        Err(format!("unit \"{}\": {what} at {}", self.src, self.i))
    }
    /// unit_expression: numerator [ "/" denominator ]  (a/b/c reads as a/(b.c))
    fn expr(&mut self) -> Result<Unit, String> {
        let mut u = self.factors()?;
        while self.peek() == Some(b'/') {
            self.i += 1;
            // Lenient: "J/kg.K" means J/(kg.K), the intent of every such string seen.
            let d = self.factors()?;
            u = Unit {
                dim: u.dim.div(d.dim),
                scale: u.scale / d.scale,
                offset: 0.0,
            };
        }
        Ok(u)
    }
    /// factors separated by "." (or by nothing after a closing parenthesis)
    fn factors(&mut self) -> Result<Unit, String> {
        let mut u = self.factor()?;
        let mut n = 1;
        while self.peek() == Some(b'.') {
            self.i += 1;
            let f = self.factor()?;
            u = Unit {
                dim: u.dim.mul(f.dim),
                scale: u.scale * f.scale,
                offset: 0.0,
            };
            n += 1;
        }
        if n > 1 {
            u.offset = 0.0;
        }
        Ok(u)
    }
    fn factor(&mut self) -> Result<Unit, String> {
        let base = match self.peek() {
            Some(b'(') => {
                self.i += 1;
                let u = self.expr()?;
                if self.peek() != Some(b')') {
                    return self.err("expected ')'");
                }
                self.i += 1;
                u
            }
            Some(c) if c.is_ascii_digit() => {
                // A bare number: "1" (dimensionless), also "1000.m" style scale factors.
                let st = self.i;
                while self
                    .peek()
                    .is_some_and(|c| c.is_ascii_digit() || c == b'e' || c == b'E')
                {
                    self.i += 1;
                }
                let v: f64 = self.src[st..self.i]
                    .parse()
                    .map_err(|_| format!("unit \"{}\": bad number", self.src))?;
                return Ok(Unit {
                    dim: Dim::ONE,
                    scale: v,
                    offset: 0.0,
                });
            }
            Some(_) => {
                let st = self.i;
                while let Some(c) = self.peek() {
                    if c.is_ascii_digit() || matches!(c, b'.' | b'/' | b'(' | b')' | b'+' | b'-') {
                        break;
                    }
                    self.i += 1;
                }
                if self.i == st {
                    return self.err("expected a unit symbol");
                }
                let name = &self.src[st..self.i];
                match symbol(name) {
                    Some(u) => u,
                    None => return Err(format!("unit \"{}\": unknown symbol '{name}'", self.src)),
                }
            }
            None => return self.err("unexpected end"),
        };
        // exponent
        let st = self.i;
        if matches!(self.peek(), Some(b'+' | b'-')) {
            self.i += 1;
        }
        let ds = self.i;
        while self.peek().is_some_and(|c| c.is_ascii_digit()) {
            self.i += 1;
        }
        if self.i == ds {
            if self.i != st {
                return self.err("expected an exponent");
            }
            return Ok(base);
        }
        let e: i32 = self.src[st..self.i]
            .parse()
            .map_err(|_| format!("unit \"{}\": bad exponent", self.src))?;
        Ok(Unit {
            dim: base.dim.powf(e as f64).ok_or("exponent too large")?,
            scale: base.scale.powi(e),
            offset: if e == 1 { base.offset } else { 0.0 },
        })
    }
}

/// Parse a Modelica unit string. The empty string is an error (no unit).
pub fn parse_unit(s: &str) -> Result<Unit, String> {
    let t: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if t.is_empty() {
        return Err("empty unit".into());
    }
    if t == "1" {
        return Ok(Unit::ONE);
    }
    let mut p = Parser {
        s: t.as_bytes(),
        src: &t,
        i: 0,
    };
    let u = p.expr()?;
    if p.i != t.len() {
        return p.err("unexpected character");
    }
    Ok(u)
}

/// Readable name of a dimension: a common derived unit when one matches, else
/// a product of base units (`kg.m2/s3`).
pub fn format_dim(d: Dim) -> String {
    #[rustfmt::skip]
    const NAMED: &[&str] = &[
        "rad", "rad/s", "rad/s2", "N", "J", "N.m", "Pa", "W", "V", "Ohm", "C", "F", "H", "Wb", "T", "S",
        "m/s", "m/s2", "m2", "m3", "kg/m3", "kg/s", "m3/s", "N.m.s/rad", "N.m/rad", "N/m", "N.s/m",
        "kg.m2", "J/K", "J/(kg.K)", "W/K", "W/(m.K)", "W/(m2.K)", "J/kg", "Pa.s", "m2/s",
        "1/s", "Hz", "1/K", "1/Pa", "J/mol", "J/(mol.K)", "mol/s", "mol/m3", "kg/mol", "A/m2",
    ];
    if d == Dim::ONE {
        return "1".into();
    }
    // An exact match (angle exponent included) first, then one up to angles.
    for pass in 0..2 {
        for n in NAMED {
            if let Ok(u) = parse_unit(n)
                && u.scale == 1.0
                && if pass == 0 { u.dim == d } else { u.dim.same(d) }
            {
                return n.to_string();
            }
        }
    }
    let part = |x: i16| -> String {
        if x % 2 == 0 {
            format!("{}", x / 2)
        } else {
            format!("{}.5", x / 2)
        }
    };
    let mut num = Vec::new();
    let mut den = Vec::new();
    for (i, &x) in d.0.iter().enumerate() {
        let sym = if i == ANGLE { "rad" } else { BASE_SYMBOLS[i] };
        if x > 0 {
            num.push(if x == 2 {
                sym.to_string()
            } else {
                format!("{sym}{}", part(x))
            });
        } else if x < 0 {
            den.push(if x == -2 {
                sym.to_string()
            } else {
                format!("{sym}{}", part(-x))
            });
        }
    }
    if num.iter().chain(&den).any(|s| s.contains(".5")) {
        // Half exponents: write everything as signed exponents.
        let mut all = Vec::new();
        for (i, &x) in d.0.iter().enumerate() {
            if x != 0 {
                let sym = if i == ANGLE { "rad" } else { BASE_SYMBOLS[i] };
                all.push(format!("{sym}^{}", x as f64 / 2.0));
            }
        }
        return all.join(".");
    }
    let n = if num.is_empty() {
        "1".to_string()
    } else {
        num.join(".")
    };
    match den.len() {
        0 => n,
        1 => format!("{n}/{}", den[0]),
        _ => format!("{n}/({})", den.join(".")),
    }
}

// ------------------------------------------------------------------ display units

/// One display unit a value can be converted to: `shown = value * scale + offset`.
#[derive(Debug, Clone, Serialize)]
pub struct DisplayUnit {
    pub unit: String,
    pub scale: f64,
    pub offset: f64,
    /// An Imperial / US customary unit.
    pub imperial: bool,
}

/// The display choices for values of one unit, with the defaults of each unit system.
#[derive(Debug, Clone, Serialize)]
pub struct DisplayChoices {
    pub options: Vec<DisplayUnit>,
    /// Default in the SI system (Dymola-style: rad/s in rpm, Pa in bar, K in degC, ...).
    pub si_default: String,
    /// Default in the Imperial system.
    pub imperial_default: String,
}

/// Display units, grouped by quantity: (units, SI default, Imperial default).
#[rustfmt::skip]
const DISPLAY: &[(&[&str], &str, &str)] = &[
    (&["m", "mm", "cm", "km", "um", "in", "ft", "yd", "mi"], "", "ft"),
    (&["kg", "g", "t", "lb", "oz"], "", "lb"),
    (&["s", "ms", "min", "h", "d"], "", ""),
    (&["m/s", "km/h", "mph", "ft/s", "kn"], "", "mph"),
    (&["m/s2", "ft/s2"], "", "ft/s2"),
    (&["rad/s", "rpm", "deg/s", "rev/s"], "rpm", "rpm"),
    (&["rad/s2", "deg/s2", "rpm/s"], "", ""),
    (&["rad", "deg", "rev"], "deg", "deg"),
    (&["N", "kN", "lbf"], "", "lbf"),
    (&["N.m", "kN.m", "N.cm", "lbf.ft", "lbf.in"], "", "lbf.ft"),
    (&["Pa", "kPa", "bar", "mbar", "MPa", "psi", "atm", "mmHg"], "bar", "psi"),
    (&["J", "kJ", "MJ", "Wh", "kWh", "cal", "kcal", "BTU", "ft.lbf"], "kJ", "BTU"),
    (&["W", "kW", "MW", "hp", "BTU/h"], "", "hp"),
    (&["K", "degC", "degF", "degR"], "degC", "degF"),
    (&["m2", "cm2", "mm2", "ft2", "in2"], "", "ft2"),
    (&["m3", "l", "ml", "cm3", "gal", "ft3", "in3"], "", "gal"),
    (&["m3/s", "l/min", "l/s", "m3/h", "gal/min", "ft3/min"], "l/min", "gal/min"),
    (&["kg/s", "kg/h", "g/s", "kg/min", "lb/s", "lb/min", "lb/h"], "", "lb/s"),
    (&["kg/m3", "g/cm3", "kg/l", "lb/ft3"], "", "lb/ft3"),
    (&["N/m", "kN/m", "N/mm", "lbf/in"], "", "lbf/in"),
    (&["N.s/m", "kN.s/m", "lbf.s/in"], "", "lbf.s/in"),
    (&["N.m/rad", "N.m/deg", "kN.m/rad", "lbf.ft/deg"], "", ""),
    (&["kg.m2", "kg.cm2", "lb.ft2"], "", "lb.ft2"),
    (&["Hz", "kHz", "1/min"], "", ""),
    (&["A", "mA", "kA"], "", ""),
    (&["V", "mV", "kV"], "", ""),
    (&["Ohm", "mOhm", "kOhm", "MOhm"], "", ""),
    (&["C", "A.h", "mA.h"], "", ""),
    (&["J/(kg.K)", "kJ/(kg.K)", "BTU/(lb.degR)"], "", ""),
    (&["J/kg", "kJ/kg", "BTU/lb"], "", ""),
    (&["W/(m2.K)", "BTU/(h.ft2.degR)"], "", ""),
    (&["W/(m.K)", "BTU/(h.ft.degR)"], "", ""),
    (&["Pa.s", "mPa.s", "cP"], "", ""),
    (&["m2/s", "mm2/s", "cSt"], "", ""),
    (&["1", "%"], "", ""),
];

const IMPERIAL: &[&str] = &[
    "in",
    "ft",
    "yd",
    "mi",
    "mph",
    "ft/s",
    "ft/s2",
    "lb",
    "oz",
    "lbf",
    "lbf.ft",
    "lbf.in",
    "psi",
    "BTU",
    "ft.lbf",
    "hp",
    "BTU/h",
    "degF",
    "degR",
    "ft2",
    "in2",
    "gal",
    "ft3",
    "in3",
    "gal/min",
    "ft3/min",
    "lb/s",
    "lb/min",
    "lb/h",
    "lb/ft3",
    "lbf/in",
    "lbf.s/in",
    "lbf.ft/deg",
    "lb.ft2",
    "BTU/(lb.degR)",
    "BTU/lb",
    "BTU/(h.ft2.degR)",
    "BTU/(h.ft.degR)",
    "kn",
];

fn parse_display(u: &str) -> Option<Unit> {
    match u {
        "cP" => parse_unit("mPa.s").ok(),
        "cSt" => parse_unit("mm2/s").ok(),
        _ => parse_unit(u).ok(),
    }
}

/// Conversion `shown = value * scale + offset` from unit `from` to unit `to`, when
/// both parse and have the same dimension (angles included).
pub fn conversion(from: &str, to: &str) -> Option<(f64, f64)> {
    let a = parse_display(from)?;
    let b = parse_display(to)?;
    if a.dim != b.dim {
        return None;
    }
    // SI = v*a.scale + a.offset;  shown = (SI - b.offset) / b.scale
    Some((a.scale / b.scale, (a.offset - b.offset) / b.scale))
}

/// The display units values of `unit` can be shown in. `display_unit` (the
/// variable's `displayUnit` attribute) becomes the SI default when it converts.
pub fn display_choices(unit: &str, display_unit: &str) -> Option<DisplayChoices> {
    let u = parse_display(unit)?;
    let mut options: Vec<DisplayUnit> = Vec::new();
    let push = |name: &str, options: &mut Vec<DisplayUnit>| {
        if options.iter().any(|o| o.unit == name) {
            return;
        }
        if let Some((scale, offset)) = conversion(unit, name) {
            options.push(DisplayUnit {
                unit: name.to_string(),
                scale,
                offset,
                imperial: IMPERIAL.contains(&name),
            });
        }
    };
    push(unit, &mut options);
    let mut si_default = String::new();
    let mut imperial_default = String::new();
    for (names, si, imp) in DISPLAY {
        let Some(first) = parse_display(names[0]) else {
            continue;
        };
        if first.dim != u.dim {
            continue;
        }
        for n in names.iter() {
            push(n, &mut options);
        }
        si_default = si.to_string();
        imperial_default = imp.to_string();
        break;
    }
    if !display_unit.is_empty() && conversion(unit, display_unit).is_some() {
        push(display_unit, &mut options);
        si_default = display_unit.to_string();
    }
    if si_default.is_empty() {
        si_default = unit.to_string();
    }
    if imperial_default.is_empty() {
        imperial_default = si_default.clone();
    }
    Some(DisplayChoices {
        options,
        si_default,
        imperial_default,
    })
}

// ------------------------------------------------------------------ checking

/// One unit inconsistency, reported as a warning.
#[derive(Debug, Clone, Serialize)]
pub struct UnitIssue {
    /// Where the equation came from (`equation in spring`, `binding of k`, ...).
    pub origin: String,
    /// The equation, shortened.
    pub equation: String,
    /// The two disagreeing parts and their units.
    pub left: String,
    pub left_unit: String,
    pub right: String,
    pub right_unit: String,
    /// One-line message.
    pub message: String,
}

/// A unit inferred for a variable without a declared unit.
#[derive(Debug, Clone, Serialize)]
pub struct InferredUnit {
    pub var: VarId,
    pub name: String,
    pub unit: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UnitReport {
    pub issues: Vec<UnitIssue>,
    pub inferred: Vec<InferredUnit>,
    /// Declared unit strings that do not parse (the variables are treated as unknown).
    pub unparsed: Vec<String>,
    /// Real variables with a declared unit / with no unit after inference.
    pub declared: usize,
    pub unknown: usize,
    pub equations: usize,
    pub passes: usize,
    pub ms: f64,
}

impl UnitReport {
    /// Unit of a variable: the declared one, else the inferred one.
    pub fn unit_of<'a>(&'a self, flat: &'a FlatModel, v: VarId) -> Option<&'a str> {
        let d = &flat.vars[v as usize].unit;
        if !d.is_empty() {
            return Some(d);
        }
        self.inferred
            .iter()
            .find(|i| i.var == v)
            .map(|i| i.unit.as_str())
    }

    /// Issue messages ready to print as warnings, at most `max` (with a final count line).
    pub fn warnings(&self, max: usize) -> Vec<String> {
        let mut out: Vec<String> = self
            .issues
            .iter()
            .take(max)
            .map(|i| i.message.clone())
            .collect();
        if self.issues.len() > max {
            out.push(format!(
                "{} more unit issues not shown",
                self.issues.len() - max
            ));
        }
        out
    }
}

/// A term of a sum: an expression, or a variable that may have no node in the arena.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Term {
    E(ExprId),
    V(VarId),
}

/// Dimension of an expression during checking.
#[derive(Debug, Clone, Copy, PartialEq)]
enum U {
    /// Zero and other unit-less values (`initial()`, `sign(x)`): adapt in sums,
    /// dimensionless as a factor.
    Lit,
    /// A numeric literal: unit-polymorphic. It adapts in sums; as a factor it makes the
    /// product [`U::Soft`].
    Num,
    /// A product with a literal factor (`m*v^2/2`, `1.5*R`): the unit the literal would
    /// have if it were dimensionless. Used to infer units, never reported as a clash.
    Soft(Dim),
    /// Unit-polymorphic everywhere (physical constants, tables, untyped calls).
    Weak,
    Known(Dim),
    /// `rest * unit(var)^(exp/2)` with `var` not yet known.
    Lin {
        var: VarId,
        exp: i16,
        rest: Dim,
    },
    /// Depends on several unknown variables.
    Unknown,
}

#[derive(Clone, Copy, PartialEq)]
enum VarUnit {
    Known(Dim),
    /// Real without a unit: to be inferred.
    Open,
    /// Not checked (Integer, Boolean, unparsed unit, `unit="1"` parameters).
    Weak,
}

struct Checker<'a> {
    f: &'a FlatModel,
    vars: Vec<VarUnit>,
    inferred: Vec<bool>,
    /// Unit string of an inferred variable copied from the variable it equals
    /// (`tau = flange.tau` gives "N.m", where the dimension alone would print "J").
    names: Vec<Option<String>>,
    memo: Vec<(u32, U)>,
    epoch: u32,
    report: bool,
    /// The current equation already has an issue (one per equation).
    flagged: bool,
    /// Inside a branch of an if-equation: no inference (path variables such as
    /// a tank's `s` take a different unit in each branch).
    in_branch: bool,
    /// The current equation still mentions an unknown variable.
    open: bool,
    changed: bool,
    origin: String,
    eq_text: (Term, Term),
    issues: Vec<UnitIssue>,
    /// Inferred variables found in clashing terms.
    demote: Vec<VarId>,
    /// Units of constants equal to 1 (`unitTorque = 1`), which the frontend folds.
    unit_constants: Vec<Dim>,
}

impl<'a> Checker<'a> {
    fn new(f: &'a FlatModel) -> (Self, Vec<String>, usize) {
        let mut cache: HashMap<&str, Option<Dim>> = HashMap::new();
        let mut unparsed = Vec::new();
        let mut declared = 0;
        let vars: Vec<VarUnit> = f
            .vars
            .iter()
            .map(|v| {
                if v.ty != VarType::Real {
                    return VarUnit::Weak;
                }
                let u = v.unit.trim();
                if u.is_empty() {
                    return VarUnit::Open;
                }
                // MSL declares gains, ratios and path parameters (ideal switches, friction
                // characteristics) "1" and uses them to convert between units.
                if u == "1" {
                    return VarUnit::Weak;
                }
                let d = *cache.entry(u).or_insert_with(|| match parse_unit(u) {
                    Ok(x) => Some(x.dim),
                    Err(_) => {
                        unparsed.push(u.to_string());
                        None
                    }
                });
                match d {
                    Some(d) => {
                        declared += 1;
                        VarUnit::Known(d)
                    }
                    None => VarUnit::Weak,
                }
            })
            .collect();
        let n = f.vars.len();
        let mut unit_constants: Vec<Dim> = Vec::new();
        for (v, vu) in f.vars.iter().zip(&vars) {
            if v.kind == VarKind::Constant
                && let VarUnit::Known(d) = vu
                && !d.is_one()
                && v.binding.and_then(|b| f.arena.as_const(b)).map(f64::abs) == Some(1.0)
                && !unit_constants.contains(d)
            {
                unit_constants.push(*d);
            }
        }
        (
            Checker {
                f,
                vars,
                inferred: vec![false; n],
                names: vec![None; n],
                memo: vec![(0, U::Unknown); f.arena.len()],
                epoch: 0,
                report: false,
                flagged: false,
                in_branch: false,
                open: false,
                changed: false,
                origin: String::new(),
                eq_text: (Term::E(0), Term::E(0)),
                issues: Vec::new(),
                demote: Vec::new(),
                unit_constants,
            },
            unparsed,
            declared,
        )
    }

    fn var(&mut self, v: VarId) -> U {
        match self.vars[v as usize] {
            VarUnit::Known(d) => U::Known(d),
            VarUnit::Weak => U::Weak,
            VarUnit::Open => {
                self.open = true;
                U::Lin {
                    var: v,
                    exp: 2,
                    rest: Dim::ONE,
                }
            }
        }
    }

    fn short(&self, t: Term) -> String {
        let s = match t {
            Term::E(e) => self.f.show(e),
            Term::V(v) => self.f.vars[v as usize].name.clone(),
        };
        if s.chars().count() > 120 {
            let t: String = s.chars().take(117).collect();
            format!("{t}...")
        } else {
            s
        }
    }

    fn unit_text(&self, d: Dim, t: Term) -> String {
        let name = format_dim(d);
        // Mention when a variable's unit was inferred.
        let v = match t {
            Term::V(v) => Some(v),
            Term::E(e) => match self.f.arena.node(e) {
                Node::Var(v) | Node::Der(v) | Node::Pre(v) => Some(v),
                _ => None,
            },
        };
        if let Some(v) = v {
            if self.inferred[v as usize] {
                return format!("{name} (inferred)");
            }
            // A bare variable: its declared unit as written ("N.m", not "J").
            let bare = match t {
                Term::V(_) => true,
                Term::E(e) => matches!(self.f.arena.node(e), Node::Var(_) | Node::Pre(_)),
            };
            let u = self.f.vars[v as usize].unit.trim();
            if bare && !u.is_empty() {
                return u.to_string();
            }
        }
        name
    }

    fn issue(&mut self, a: (Term, Dim), b: (Term, Dim), what: &str) {
        if !self.report || self.flagged {
            return;
        }
        // MSL writes `Rv0 + Rv1*unitAngularVelocity` with `constant AngularVelocity
        // unitAngularVelocity = 1`; the frontend folds the constant, and with it its unit.
        let ratio = a.1.div(b.1);
        let inv = b.1.div(a.1);
        if self
            .unit_constants
            .iter()
            .any(|c| c.same(ratio) || c.same(inv))
        {
            return;
        }
        self.flagged = true;
        for t in [a.0, b.0] {
            match t {
                Term::V(v) => {
                    if self.inferred[v as usize] {
                        self.demote.push(v);
                    }
                }
                Term::E(e) => {
                    let (inferred, demote) = (&self.inferred, &mut self.demote);
                    self.f.arena.visit_all_leaves(e, &mut |n| {
                        if let Node::Var(v) | Node::Der(v) | Node::Pre(v) = n
                            && inferred[v as usize]
                        {
                            demote.push(v);
                        }
                    });
                }
            }
        }
        let (l, r) = self.eq_text;
        let equation = if r == Term::E(crate::expr::Arena::ZERO) {
            self.short(l)
        } else {
            format!("{} = {}", self.short(l), self.short(r))
        };
        let left = self.short(a.0);
        let right = self.short(b.0);
        let left_unit = self.unit_text(a.1, a.0);
        let right_unit = self.unit_text(b.1, b.0);
        let message = format!(
            "unit mismatch ({what}) in {}: `{left}` has unit {left_unit} but `{right}` has unit {right_unit}; equation: {equation}",
            self.origin
        );
        self.issues.push(UnitIssue {
            origin: self.origin.clone(),
            equation,
            left,
            left_unit,
            right,
            right_unit,
            message,
        });
    }

    fn infer(&mut self, var: VarId, exp: i16, rest: Dim, target: Dim) -> bool {
        if exp == 0 || self.in_branch || self.vars[var as usize] != VarUnit::Open {
            return false;
        }
        let mut d = [0i16; 8];
        for (i, o) in d.iter_mut().enumerate() {
            let num = 2 * (target.0[i] as i32 - rest.0[i] as i32);
            if num % exp as i32 != 0 {
                return false;
            }
            *o = (num / exp as i32) as i16;
        }
        self.vars[var as usize] = VarUnit::Known(Dim(d));
        self.inferred[var as usize] = true;
        self.changed = true;
        true
    }

    /// The unit string of a bare variable term (declared, or copied when inferred).
    fn term_name(&self, t: Term) -> Option<String> {
        let v = match t {
            Term::V(v) => v,
            Term::E(e) => match self.f.arena.node(e) {
                Node::Var(v) | Node::Pre(v) => v,
                _ => return None,
            },
        };
        let u = self.f.vars[v as usize].unit.trim();
        if !u.is_empty() {
            return Some(u.to_string());
        }
        self.names[v as usize].clone()
    }

    /// Terms that must share one unit; returns their common unit.
    fn unify(&mut self, terms: &[Term], what: &str) -> U {
        let mut us: Vec<U> = Vec::with_capacity(terms.len());
        for &t in terms {
            let u = match t {
                Term::E(e) => self.eval(e),
                Term::V(v) => self.var(v),
            };
            us.push(u);
        }
        let mut reference: Option<(Term, Dim)> = None;
        let mut soft: Option<Dim> = None;
        for (i, u) in us.iter().enumerate() {
            if let U::Soft(d) = *u
                && soft.is_none()
            {
                soft = Some(d);
            }
            if let U::Known(d) = *u {
                match reference {
                    None => reference = Some((terms[i], d)),
                    Some((e, r)) => {
                        if !r.same(d) {
                            self.issue((e, r), (terms[i], d), what);
                        }
                    }
                }
            }
        }
        // A polymorphic term that is not a plain constant (a `unit="1"` path parameter,
        // a physical constant times something) makes the whole sum polymorphic: MSL's
        // friction and switch models add such a term to quantities of several units.
        let weak = us.iter().zip(terms).any(|(u, t)| {
            *u == U::Weak && !matches!(t, Term::E(e) if self.f.arena.as_const(*e).is_some())
        });
        if let Some((t, r)) = reference {
            let mut name = None;
            for u in &us {
                if let U::Lin { var, exp, rest } = *u
                    && self.infer(var, exp, rest, r)
                    && exp == 2
                    && rest == Dim::ONE
                {
                    if name.is_none() {
                        name = Some(self.term_name(t));
                    }
                    self.names[var as usize] = name.clone().flatten();
                }
            }
            return if weak { U::Weak } else { U::Known(r) };
        }
        if let Some(r) = soft {
            for u in &us {
                if let U::Lin { var, exp, rest } = *u {
                    self.infer(var, exp, rest, r);
                }
            }
            return if weak { U::Weak } else { U::Soft(r) };
        }
        let mut out = U::Lit;
        for u in us {
            out = match (out, u) {
                (U::Unknown, _) | (_, U::Unknown) => U::Unknown,
                (U::Lin { var: a, .. }, U::Lin { var: b, .. }) if a != b => U::Unknown,
                (l @ U::Lin { .. }, _) | (_, l @ U::Lin { .. }) => l,
                (U::Weak, _) | (_, U::Weak) => U::Weak,
                _ => U::Lit,
            };
        }
        out
    }

    /// Flatten a sum into its terms (signs do not matter for units).
    fn terms(&self, root: ExprId, out: &mut Vec<Term>) {
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            match self.f.arena.node(e) {
                Node::Add(a, b) | Node::Sub(a, b) => {
                    stack.push(b);
                    stack.push(a);
                }
                Node::Neg(a) => stack.push(a),
                _ => out.push(Term::E(e)),
            }
        }
    }

    fn mul(a: U, b: U, inv: bool) -> U {
        // A literal factor: multiply as if dimensionless, and mark the result soft.
        let soften = |u: U| match u {
            U::Num => (U::Known(Dim::ONE), true),
            U::Soft(d) => (U::Known(d), true),
            x => (x, false),
        };
        let (a, sa) = soften(a);
        let (b, sb) = soften(b);
        let r = Self::mul_hard(a, b, inv);
        if !(sa || sb) {
            return r;
        }
        match r {
            U::Known(d) => U::Soft(d),
            U::Lin { .. } => U::Weak,
            x => x,
        }
    }

    fn mul_hard(a: U, b: U, inv: bool) -> U {
        let neg = |d: Dim| Dim::ONE.div(d);
        match (a, b) {
            (U::Unknown, _) | (_, U::Unknown) => U::Unknown,
            (U::Weak, _) | (_, U::Weak) => U::Weak,
            (U::Lit, U::Lit) => U::Lit,
            (x, U::Lit) => x,
            (U::Num | U::Soft(_), _) | (_, U::Num | U::Soft(_)) => U::Weak,
            (U::Lit, U::Known(d)) => U::Known(if inv { neg(d) } else { d }),
            (U::Lit, U::Lin { var, exp, rest }) => {
                if inv {
                    U::Lin {
                        var,
                        exp: -exp,
                        rest: neg(rest),
                    }
                } else {
                    U::Lin { var, exp, rest }
                }
            }
            (U::Known(x), U::Known(y)) => U::Known(if inv { x.div(y) } else { x.mul(y) }),
            (U::Known(x), U::Lin { var, exp, rest }) => {
                if inv {
                    U::Lin {
                        var,
                        exp: -exp,
                        rest: x.div(rest),
                    }
                } else {
                    U::Lin {
                        var,
                        exp,
                        rest: x.mul(rest),
                    }
                }
            }
            (U::Lin { var, exp, rest }, U::Known(y)) => U::Lin {
                var,
                exp,
                rest: if inv { rest.div(y) } else { rest.mul(y) },
            },
            (
                U::Lin {
                    var: v1,
                    exp: e1,
                    rest: r1,
                },
                U::Lin {
                    var: v2,
                    exp: e2,
                    rest: r2,
                },
            ) => {
                if v1 != v2 {
                    return U::Unknown;
                }
                let (exp, rest) = if inv {
                    (e1 - e2, r1.div(r2))
                } else {
                    (e1 + e2, r1.mul(r2))
                };
                if exp == 0 {
                    U::Known(rest)
                } else {
                    U::Lin { var: v1, exp, rest }
                }
            }
        }
    }

    /// The argument must be dimensionless.
    fn dimensionless(&mut self, e: ExprId, what: &str) {
        match self.eval(e) {
            // A soft argument (`sin(pi*x/(2*dx))` in an inlined MSL function whose `dx = 1`
            // was folded) is most likely scaled by its literal: stay quiet.
            U::Known(d) if !d.is_one() => self.issue(
                (Term::E(e), d),
                (Term::E(crate::expr::Arena::ONE), Dim::ONE),
                what,
            ),
            U::Lin { var, exp, rest } => {
                self.infer(var, exp, rest, Dim::ONE);
            }
            _ => {}
        }
    }

    fn eval(&mut self, e: ExprId) -> U {
        if self.memo[e as usize].0 == self.epoch {
            let u = self.memo[e as usize].1;
            if matches!(u, U::Lin { .. } | U::Unknown) {
                self.open = true;
            }
            return u;
        }
        let u = self.eval_node(e);
        self.memo[e as usize] = (self.epoch, u);
        u
    }

    fn eval_node(&mut self, e: ExprId) -> U {
        let ar = &self.f.arena;
        let sec = Dim::base(2);
        match ar.node(e) {
            // Literals are unit-polymorphic: `x + 1` and `1.5*R` (a voltage) both pass.
            Node::Const(_) => {
                if ar.as_const(e) == Some(0.0) {
                    U::Lit
                } else {
                    U::Num
                }
            }
            Node::Var(v) | Node::Pre(v) => self.var(v),
            Node::Der(v) => {
                let u = self.var(v);
                Self::mul(u, U::Known(sec), true)
            }
            Node::Time => U::Known(sec),
            Node::Initial | Node::Terminal | Node::Local(_) => U::Lit,
            Node::Neg(_) | Node::Add(..) | Node::Sub(..) => {
                let mut t = Vec::new();
                self.terms(e, &mut t);
                self.unify(&t, "sum")
            }
            Node::Mul(a, b) | Node::Div(a, b) => {
                let inv = matches!(ar.node(e), Node::Div(..));
                let x = self.eval(a);
                let y = self.eval(b);
                Self::mul(x, y, inv)
            }
            Node::Pow(a, b) => {
                let base = self.eval(a);
                let p = ar.as_const(b);
                if p.is_none() {
                    self.dimensionless(b, "exponent");
                }
                match (base, p) {
                    (U::Known(d), Some(p)) => d.powf(p).map_or(U::Weak, U::Known),
                    (U::Known(d), None) if d.is_one() => U::Known(d),
                    (U::Lin { var, exp, rest }, Some(p)) => {
                        let ne = exp as f64 * p;
                        match rest.powf(p) {
                            Some(r) if (ne - ne.round()).abs() < 1e-9 && ne != 0.0 => U::Lin {
                                var,
                                exp: ne.round() as i16,
                                rest: r,
                            },
                            _ => U::Unknown,
                        }
                    }
                    (U::Lit, _) => U::Lit,
                    (U::Num, _) => U::Num,
                    (U::Soft(d), Some(p)) => d.powf(p).map_or(U::Weak, U::Soft),
                    (U::Unknown, _) => U::Unknown,
                    _ => U::Weak,
                }
            }
            Node::F1(f, a) => match f {
                Fn1::Sin
                | Fn1::Cos
                | Fn1::Tan
                | Fn1::Asin
                | Fn1::Acos
                | Fn1::Atan
                | Fn1::Sinh
                | Fn1::Cosh
                | Fn1::Tanh
                | Fn1::Exp => {
                    let name = crate::expr::fn1_name(f);
                    self.dimensionless(a, &format!("argument of {name}"));
                    U::Known(Dim::ONE)
                }
                // The logarithm of a dimensional value (`log(p)`, `log(T)`) is common in
                // property correlations: log(x) = log(x/unit) + const. Not checked.
                Fn1::Log | Fn1::Log10 => {
                    self.eval(a);
                    U::Known(Dim::ONE)
                }
                Fn1::Sqrt => {
                    let x = self.eval(a);
                    match x {
                        U::Known(d) => d.powf(0.5).map_or(U::Weak, U::Known),
                        U::Soft(d) => d.powf(0.5).map_or(U::Weak, U::Soft),
                        U::Lin { var, exp, rest } if exp % 2 == 0 => match rest.powf(0.5) {
                            Some(r) => U::Lin {
                                var,
                                exp: exp / 2,
                                rest: r,
                            },
                            None => U::Unknown,
                        },
                        U::Lin { .. } => U::Unknown,
                        o => o,
                    }
                }
                Fn1::Abs | Fn1::Floor | Fn1::Ceil => self.eval(a),
                Fn1::Sign | Fn1::Not => {
                    self.eval(a);
                    U::Lit
                }
            },
            Node::F2(f, a, b) => match f {
                Fn2::Min | Fn2::Max | Fn2::Mod | Fn2::Rem => {
                    let name = crate::expr::fn2_name(f);
                    self.unify(&[Term::E(a), Term::E(b)], &format!("arguments of {name}"))
                }
                Fn2::Atan2 => {
                    self.unify(&[Term::E(a), Term::E(b)], "arguments of atan2");
                    U::Known(Dim::ONE)
                }
                Fn2::Div => {
                    let x = self.eval(a);
                    let y = self.eval(b);
                    Self::mul(x, y, true)
                }
                Fn2::And | Fn2::Or => {
                    self.eval(a);
                    self.eval(b);
                    U::Lit
                }
            },
            Node::Cmp(op, a, b) => {
                self.unify(&[Term::E(a), Term::E(b)], cmp_what(op));
                U::Lit
            }
            Node::Rel(r) => {
                let rel = ar.relations[r as usize];
                self.unify(&[Term::E(rel.lhs), Term::E(rel.rhs)], cmp_what(rel.op));
                U::Lit
            }
            Node::If(c, a, b) => {
                self.eval(c);
                let mut t = Vec::new();
                self.terms(a, &mut t);
                self.terms(b, &mut t);
                self.unify(&t, "if-branches")
            }
            Node::Interp(_, x, _) => {
                self.eval(x);
                U::Weak
            }
            Node::Interp2(_, u1, u2) => {
                self.eval(u1);
                self.eval(u2);
                U::Weak
            }
            Node::Delay(k, d) => {
                let v = ar.delays[k as usize];
                self.eval(d);
                self.var(v)
            }
            Node::Call(..) | Node::CallD(..) | Node::Clk(..) => {
                let mut ch = Vec::new();
                ar.children(e, &mut ch);
                for c in ch {
                    self.eval(c);
                }
                U::Weak
            }
        }
    }

    /// Check (and infer from) one equation `lhs = rhs`.
    fn equation(&mut self, lhs: Term, rhs: ExprId, origin: &str) -> bool {
        self.flagged = false;
        self.open = false;
        if self.report {
            self.origin.clear();
            self.origin.push_str(origin);
        }
        self.eq_text = (lhs, Term::E(rhs));
        let mut t = Vec::new();
        match lhs {
            Term::E(l) => self.terms(l, &mut t),
            v => t.push(v),
        }
        self.terms(rhs, &mut t);
        self.residual(t);
        self.open
    }

    /// Terms of one residual. An if-equation is flattened to `0 = if c then r1 else r2`
    /// with one residual per branch: those are separate equations, checked one by one.
    fn residual(&mut self, mut t: Vec<Term>) {
        t.retain(|x| !matches!(x, Term::E(e) if self.f.arena.as_const(*e).is_some()));
        if let [Term::E(e)] = t[..]
            && let Node::If(c, a, b) = self.f.arena.node(e)
        {
            self.eval(c);
            let outer = self.in_branch;
            self.in_branch = true;
            for br in [a, b] {
                let mut tb = Vec::new();
                self.terms(br, &mut tb);
                self.residual(tb);
            }
            self.in_branch = outer;
            return;
        }
        self.unify(&t, "equation");
    }
}

fn cmp_what(op: Cmp) -> &'static str {
    let _ = op;
    "relation"
}

/// Items checked: (lhs, rhs, origin index or a fixed description).
enum Item {
    Eq(ExprId, ExprId, u32),
    Binding(VarId, ExprId),
    Expr(ExprId, u32),
    Assign(VarId, ExprId, u32),
}

/// Check the units of a flat model and infer units of variables without one.
pub fn check(f: &FlatModel) -> UnitReport {
    let t0 = now();
    let mut items: Vec<Item> = Vec::new();
    for e in f.equations.iter().chain(&f.initial_equations) {
        items.push(Item::Eq(e.lhs, e.rhs, e.origin));
    }
    for (i, v) in f.vars.iter().enumerate() {
        if matches!(v.kind, VarKind::Parameter | VarKind::Constant)
            && v.ty == VarType::Real
            && let Some(b) = v.binding
        {
            items.push(Item::Binding(i as VarId, b));
        }
    }
    for w in &f.whens {
        for (cond, acts) in &w.branches {
            items.push(Item::Expr(*cond, w.origin));
            for a in acts {
                match a {
                    WhenAction::Assign(v, e) | WhenAction::Reinit(v, e) => {
                        items.push(Item::Assign(*v, *e, w.origin))
                    }
                    WhenAction::Assert(e, _) => items.push(Item::Expr(*e, w.origin)),
                    WhenAction::Effect(c, e) => {
                        items.push(Item::Expr(*c, w.origin));
                        items.push(Item::Expr(*e, w.origin));
                    }
                    WhenAction::Terminate(_) => {}
                }
            }
        }
    }
    for (e, _) in &f.asserts {
        items.push(Item::Expr(*e, u32::MAX));
    }
    let origin = |o: u32| -> String {
        f.origins
            .get(o as usize)
            .cloned()
            .unwrap_or_else(|| "assertion".into())
    };
    let run = |c: &mut Checker, it: &Item| -> bool {
        match *it {
            Item::Eq(l, r, o) => {
                let s = if c.report { origin(o) } else { String::new() };
                c.equation(Term::E(l), r, &s)
            }
            Item::Binding(v, b) => {
                let s = if c.report {
                    format!("binding of {}", f.vars[v as usize].name)
                } else {
                    String::new()
                };
                c.equation(Term::V(v), b, &s)
            }
            Item::Assign(v, e, o) => {
                let s = if c.report {
                    format!("{} (when-clause)", origin(o))
                } else {
                    String::new()
                };
                c.equation(Term::V(v), e, &s)
            }
            Item::Expr(e, o) => {
                let s = if c.report { origin(o) } else { String::new() };
                c.equation(Term::E(e), crate::expr::Arena::ZERO, &s)
            }
        }
    };
    // Variables whose inferred unit clashed: path variables used with several units
    // (a tank's `s`, a pump's `s`) or the victims of such a clash. A new round treats
    // them as unit-polymorphic, so only declared units are left to disagree.
    let mut demoted: rustc_hash::FxHashSet<VarId> = Default::default();
    let mut passes = 0;
    let mut round = 0;
    let (c, unparsed, declared) = loop {
        round += 1;
        let (mut c, unparsed, declared) = Checker::new(f);
        for &v in &demoted {
            c.vars[v as usize] = VarUnit::Weak;
        }
        // Inference passes over the items that still mention unknowns, to a fixed point.
        let mut pending: Vec<usize> = (0..items.len()).collect();
        while !pending.is_empty() && passes < 50 * round {
            passes += 1;
            c.epoch += 1;
            c.changed = false;
            let mut next = Vec::new();
            for &i in &pending {
                if run(&mut c, &items[i]) {
                    next.push(i);
                }
            }
            if !c.changed {
                break;
            }
            pending = next;
        }
        // Final pass: report.
        c.epoch += 1;
        c.report = true;
        for it in &items {
            run(&mut c, it);
        }
        let before = demoted.len();
        demoted.extend(c.demote.iter().copied());
        if round == 4 || demoted.len() == before {
            break (c, unparsed, declared);
        }
    };
    let inferred = (0..f.vars.len())
        .filter(|&i| c.inferred[i])
        .filter_map(|i| match c.vars[i] {
            VarUnit::Known(d) => Some(InferredUnit {
                var: i as VarId,
                name: f.vars[i].name.clone(),
                unit: c.names[i].clone().unwrap_or_else(|| format_dim(d)),
            }),
            _ => None,
        })
        .collect();
    let unknown = c.vars.iter().filter(|v| **v == VarUnit::Open).count();
    let mut unparsed = unparsed;
    unparsed.sort();
    unparsed.dedup();
    UnitReport {
        issues: c.issues,
        inferred,
        unparsed,
        declared,
        unknown,
        equations: items.len(),
        passes: passes + 1,
        ms: now() - t0,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn now() -> f64 {
    use std::sync::OnceLock;
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now)
        .elapsed()
        .as_secs_f64()
        * 1e3
}
#[cfg(target_arch = "wasm32")]
fn now() -> f64 {
    0.0
}

/// Units of all variables of `f` after inference: declared, else inferred, else "".
pub fn resolved_units(f: &FlatModel, r: &UnitReport) -> FxHashMap<VarId, String> {
    let mut m = FxHashMap::default();
    for i in &r.inferred {
        m.insert(i.var, i.unit.clone());
    }
    for (i, v) in f.vars.iter().enumerate() {
        if !v.unit.is_empty() {
            m.insert(i as VarId, v.unit.clone());
        }
    }
    m
}
