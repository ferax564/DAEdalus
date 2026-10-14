//! Hash-consed symbolic expression arena.
//!
//! Every expression in the flat model, the symbolic backend and the code
//! generator lives in one [`Arena`]. Nodes are interned, so structurally equal
//! sub-expressions share one id: common-subexpression elimination is free and
//! derivative / substitution caches can be keyed by id.

use rustc_hash::FxHashMap;
use std::fmt::Write as _;

/// Index of an expression node in the arena.
pub type ExprId = u32;
/// Index of a variable in the flat model.
pub type VarId = u32;

/// Scalar functions of one argument.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Fn1 {
    Sin,
    Cos,
    Tan,
    Asin,
    Acos,
    Atan,
    Sinh,
    Cosh,
    Tanh,
    Exp,
    Log,
    Log10,
    Sqrt,
    Abs,
    Sign,
    Floor,
    Ceil,
    Not,
}

/// Scalar functions of two arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Fn2 {
    Atan2,
    Min,
    Max,
    /// `div(x, y)`: truncated integer division.
    Div,
    /// `mod(x, y)`: x - floor(x/y)*y.
    Mod,
    /// `rem(x, y)`: x - div(x,y)*y.
    Rem,
    And,
    Or,
}

/// Comparison operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
    Ne,
}

impl Cmp {
    pub fn eval(self, a: f64, b: f64) -> bool {
        match self {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
            Cmp::Eq => a == b,
            Cmp::Ne => a != b,
        }
    }
    pub fn symbol(self) -> &'static str {
        match self {
            Cmp::Lt => "<",
            Cmp::Le => "<=",
            Cmp::Gt => ">",
            Cmp::Ge => ">=",
            Cmp::Eq => "==",
            Cmp::Ne => "<>",
        }
    }
}

/// One node of the expression DAG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Node {
    /// Constant stored as raw bits so the node is hashable.
    Const(u64),
    /// Model variable (parameter, state, algebraic, discrete or input).
    Var(VarId),
    /// Time derivative of a variable. Replaced by derivative variables in the backend.
    Der(VarId),
    /// `pre(v)`: left limit of a discrete (or continuous) variable at an event.
    Pre(VarId),
    Time,
    Neg(ExprId),
    Add(ExprId, ExprId),
    Sub(ExprId, ExprId),
    Mul(ExprId, ExprId),
    Div(ExprId, ExprId),
    Pow(ExprId, ExprId),
    F1(Fn1, ExprId),
    F2(Fn2, ExprId, ExprId),
    /// Direct comparison (inside `noEvent` or on discrete operands); value 0/1.
    Cmp(Cmp, ExprId, ExprId),
    /// Event-generating relation; value latched between events. Index into the relation table.
    Rel(u32),
    /// `if c then a else b` with c a 0/1 valued expression.
    If(ExprId, ExprId, ExprId),
    /// Piecewise-linear interpolation in a constant table: (table id, column, x).
    Interp(u32, u32, ExprId),
    /// Slope of [`Node::Interp`] with respect to x (used by symbolic differentiation).
    InterpDer(u32, u32, ExprId),
    /// Integration-time `initial()` flag (1 during initialization, else 0).
    Initial,
}

/// An event-generating relation `lhs op rhs`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Relation {
    pub op: Cmp,
    pub lhs: ExprId,
    pub rhs: ExprId,
}

/// Constant interpolation table (rows of `[x, y1, y2, ...]`).
///
/// This is the runtime half of the bundled `interpolate` builtin and of the MSL
/// `CombiTable1D*` / `CombiTimeTable` blocks, whose external C tables are implemented
/// natively here.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Table {
    pub rows: usize,
    pub cols: usize,
    pub data: Vec<f64>,
    /// 0 = hold the last value, 1 = extrapolate the last two points, 2 = periodic.
    pub extrapolation: u8,
    /// 0 = linear segments, 1 = constant segments, 2 = cubic Hermite (slopes in `slopes`).
    #[serde(default)]
    pub smoothness: u8,
    /// Hermite slopes, same layout as `data` (column 0 unused); empty unless `smoothness == 2`.
    #[serde(default)]
    pub slopes: Vec<f64>,
    /// The abscissa is `x - shift` (time tables: `shiftTime`).
    #[serde(default)]
    pub shift: f64,
    /// Below `start` the table yields 0 (time tables: `startTime`).
    #[serde(default = "neg_inf")]
    pub start: f64,
}

fn neg_inf() -> f64 {
    f64::NEG_INFINITY
}

impl Table {
    /// Column index that asks for the next breakpoint after `x` (time events).
    pub const NEXT_EVENT: u32 = u32::MAX;

    pub fn linear(rows: usize, cols: usize, data: Vec<f64>, extrapolation: u8) -> Table {
        Table {
            rows,
            cols,
            data,
            extrapolation,
            smoothness: 0,
            slopes: Vec::new(),
            shift: 0.0,
            start: f64::NEG_INFINITY,
        }
    }

    #[inline]
    pub fn at(&self, r: usize, c: usize) -> f64 {
        self.data[r * self.cols + c]
    }

    /// Compute Hermite slopes: Akima (`monotone == false`) or Fritsch–Butland.
    pub fn make_cubic(&mut self, monotone: bool) {
        let n = self.rows;
        self.smoothness = 2;
        self.slopes = vec![0.0; self.data.len()];
        if n < 2 {
            return;
        }
        for c in 1..self.cols {
            // Secants between rows.
            let m: Vec<f64> = (0..n - 1)
                .map(|r| {
                    let h = self.at(r + 1, 0) - self.at(r, 0);
                    if h > 0.0 {
                        (self.at(r + 1, c) - self.at(r, c)) / h
                    } else {
                        0.0
                    }
                })
                .collect();
            for r in 0..n {
                let s = if n == 2 {
                    m[0]
                } else if monotone {
                    if r == 0 {
                        m[0]
                    } else if r == n - 1 {
                        m[n - 2]
                    } else {
                        let (a, b) = (m[r - 1], m[r]);
                        if a * b <= 0.0 {
                            0.0
                        } else {
                            2.0 * a * b / (a + b)
                        }
                    }
                } else {
                    // Akima with the usual end extension of the secants.
                    let get = |k: isize| -> f64 {
                        let last = (n - 2) as isize;
                        if (0..=last).contains(&k) {
                            m[k as usize]
                        } else if k < 0 {
                            let (m0, m1) = (m[0], *m.get(1).unwrap_or(&m[0]));
                            if k == -1 {
                                2.0 * m0 - m1
                            } else {
                                3.0 * m0 - 2.0 * m1
                            }
                        } else {
                            let ml = m[last as usize];
                            let ml1 = if last >= 1 { m[last as usize - 1] } else { ml };
                            if k == last + 1 {
                                2.0 * ml - ml1
                            } else {
                                3.0 * ml - 2.0 * ml1
                            }
                        }
                    };
                    let r = r as isize;
                    let (m1, m2, m3, m4) = (get(r - 2), get(r - 1), get(r), get(r + 1));
                    let (w1, w2) = ((m4 - m3).abs(), (m2 - m1).abs());
                    if w1 + w2 < 1e-300 {
                        0.5 * (m2 + m3)
                    } else {
                        (w1 * m2 + w2 * m3) / (w1 + w2)
                    }
                };
                self.slopes[r * self.cols + c] = s;
            }
        }
    }

    /// Map `x` into table coordinates: shift, then fold for periodic tables.
    #[inline]
    fn map(&self, x: f64) -> f64 {
        let x = x - self.shift;
        if self.extrapolation == 2 && self.rows >= 2 {
            let (x0, x1) = (self.at(0, 0), self.at(self.rows - 1, 0));
            let p = x1 - x0;
            if p > 0.0 {
                return x0 + (x - x0).rem_euclid(p);
            }
        }
        x
    }

    /// Locate the interval for `x` (binary search) and return (row, weight).
    #[inline]
    fn locate(&self, x: f64) -> (usize, f64) {
        let n = self.rows;
        if n < 2 {
            return (0, 0.0);
        }
        let (mut lo, mut hi) = (0usize, n - 1);
        if x <= self.at(0, 0) {
            lo = 0;
        } else if x >= self.at(n - 1, 0) {
            lo = n - 2;
        } else {
            while hi - lo > 1 {
                let mid = (lo + hi) / 2;
                if self.at(mid, 0) <= x {
                    lo = mid;
                } else {
                    hi = mid;
                }
            }
        }
        let x0 = self.at(lo, 0);
        let x1 = self.at(lo + 1, 0);
        let w = if x1 > x0 { (x - x0) / (x1 - x0) } else { 0.0 };
        (lo, w)
    }

    pub fn interp(&self, col: usize, x: f64) -> f64 {
        if col == Self::NEXT_EVENT as usize {
            return self.next_event(x);
        }
        if x < self.start || self.rows == 0 {
            return 0.0;
        }
        if self.rows == 1 {
            return self.at(0, col);
        }
        let x = self.map(x);
        let (r, w) = self.locate(x);
        let (y0, y1) = (self.at(r, col), self.at(r + 1, col));
        let inside = (0.0..=1.0).contains(&w);
        if !inside && self.extrapolation == 0 {
            return if w < 0.0 { y0 } else { y1 };
        }
        match self.smoothness {
            1 => {
                if w >= 1.0 {
                    y1
                } else {
                    y0
                }
            }
            2 => {
                let h = self.at(r + 1, 0) - self.at(r, 0);
                let (s0, s1) = (
                    self.slopes[r * self.cols + col],
                    self.slopes[(r + 1) * self.cols + col],
                );
                if !inside {
                    // Extrapolate linearly with the end slope.
                    return if w < 0.0 {
                        y0 + s0 * w * h
                    } else {
                        y1 + s1 * (w - 1.0) * h
                    };
                }
                let (w2, w3) = (w * w, w * w * w);
                (2.0 * w3 - 3.0 * w2 + 1.0) * y0
                    + (w3 - 2.0 * w2 + w) * h * s0
                    + (-2.0 * w3 + 3.0 * w2) * y1
                    + (w3 - w2) * h * s1
            }
            _ => y0 + w * (y1 - y0),
        }
    }

    pub fn slope(&self, col: usize, x: f64) -> f64 {
        if col == Self::NEXT_EVENT as usize || x < self.start || self.rows < 2 {
            return 0.0;
        }
        let x = self.map(x);
        let (r, w) = self.locate(x);
        let inside = (0.0..=1.0).contains(&w);
        if (!inside && self.extrapolation == 0) || self.smoothness == 1 {
            return 0.0;
        }
        let x0 = self.at(r, 0);
        let x1 = self.at(r + 1, 0);
        if x1 <= x0 {
            return 0.0;
        }
        let h = x1 - x0;
        if self.smoothness == 2 {
            let (s0, s1) = (
                self.slopes[r * self.cols + col],
                self.slopes[(r + 1) * self.cols + col],
            );
            if !inside {
                return if w < 0.0 { s0 } else { s1 };
            }
            let (y0, y1) = (self.at(r, col), self.at(r + 1, col));
            let w2 = w * w;
            return ((6.0 * w2 - 6.0 * w) * y0
                + (3.0 * w2 - 4.0 * w + 1.0) * h * s0
                + (-6.0 * w2 + 6.0 * w) * y1
                + (3.0 * w2 - 2.0 * w) * h * s1)
                / h;
        }
        (self.at(r + 1, col) - self.at(r, col)) / h
    }

    /// The first breakpoint strictly after `x` (in unshifted coordinates), or +inf.
    pub fn next_event(&self, x: f64) -> f64 {
        if x < self.start {
            return self.start;
        }
        if self.rows == 0 {
            return f64::INFINITY;
        }
        let eps = 1e-12 * (1.0 + x.abs());
        let first =
            |lo: f64| -> Option<f64> { (0..self.rows).map(|r| self.at(r, 0)).find(|&t| t > lo) };
        let xs = x - self.shift;
        if self.extrapolation == 2 && self.rows >= 2 {
            let (x0, x1) = (self.at(0, 0), self.at(self.rows - 1, 0));
            let p = x1 - x0;
            if p > 0.0 {
                let k = ((xs - x0) / p).floor();
                let local = xs - k * p;
                // `local` lies in [x0, x1), so the last row is always a candidate.
                let t = first(local + eps).unwrap_or(x1);
                return t + k * p + self.shift;
            }
        }
        first(xs + eps).map_or(f64::INFINITY, |t| t + self.shift)
    }
}

/// The expression arena with interning and algebraic simplification on construction.
#[derive(Default, Clone)]
pub struct Arena {
    nodes: Vec<Node>,
    intern: FxHashMap<Node, ExprId>,
    pub relations: Vec<Relation>,
    rel_intern: FxHashMap<Relation, u32>,
    pub tables: Vec<Table>,
}

#[inline]
fn bits(v: f64) -> u64 {
    // Normalise -0.0 to 0.0 so both intern to the same node.
    if v == 0.0 {
        0f64.to_bits()
    } else {
        v.to_bits()
    }
}

impl Arena {
    pub fn new() -> Self {
        let mut a = Arena::default();
        // Reserve ids 0 and 1 for the constants zero and one.
        a.intern_node(Node::Const(bits(0.0)));
        a.intern_node(Node::Const(bits(1.0)));
        a
    }

    pub const ZERO: ExprId = 0;
    pub const ONE: ExprId = 1;

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    #[inline]
    pub fn node(&self, id: ExprId) -> Node {
        self.nodes[id as usize]
    }

    fn intern_node(&mut self, n: Node) -> ExprId {
        if let Some(&id) = self.intern.get(&n) {
            return id;
        }
        let id = self.nodes.len() as ExprId;
        self.nodes.push(n);
        self.intern.insert(n, id);
        id
    }

    /// Constant value of a node, if it is a constant.
    #[inline]
    pub fn as_const(&self, id: ExprId) -> Option<f64> {
        match self.nodes[id as usize] {
            Node::Const(b) => Some(f64::from_bits(b)),
            _ => None,
        }
    }

    pub fn is_zero(&self, id: ExprId) -> bool {
        self.as_const(id) == Some(0.0)
    }

    pub fn is_one(&self, id: ExprId) -> bool {
        self.as_const(id) == Some(1.0)
    }

    pub fn constant(&mut self, v: f64) -> ExprId {
        self.intern_node(Node::Const(bits(v)))
    }

    pub fn var(&mut self, v: VarId) -> ExprId {
        self.intern_node(Node::Var(v))
    }

    pub fn der(&mut self, v: VarId) -> ExprId {
        self.intern_node(Node::Der(v))
    }

    pub fn pre(&mut self, v: VarId) -> ExprId {
        self.intern_node(Node::Pre(v))
    }

    pub fn time(&mut self) -> ExprId {
        self.intern_node(Node::Time)
    }

    pub fn initial(&mut self) -> ExprId {
        self.intern_node(Node::Initial)
    }

    pub fn neg(&mut self, a: ExprId) -> ExprId {
        if let Some(c) = self.as_const(a) {
            return self.constant(-c);
        }
        match self.node(a) {
            Node::Neg(x) => x,
            Node::Sub(x, y) => self.intern_node(Node::Sub(y, x)),
            _ => self.intern_node(Node::Neg(a)),
        }
    }

    pub fn add(&mut self, a: ExprId, b: ExprId) -> ExprId {
        match (self.as_const(a), self.as_const(b)) {
            (Some(x), Some(y)) => return self.constant(x + y),
            (Some(x), _) if x == 0.0 => return b,
            (_, Some(y)) if y == 0.0 => return a,
            _ => {}
        }
        if let Node::Neg(nb) = self.node(b) {
            return self.sub(a, nb);
        }
        if let Node::Neg(na) = self.node(a) {
            return self.sub(b, na);
        }
        // Canonical operand order keeps a+b and b+a as one node.
        let (a, b) = if a <= b { (a, b) } else { (b, a) };
        self.intern_node(Node::Add(a, b))
    }

    pub fn sub(&mut self, a: ExprId, b: ExprId) -> ExprId {
        if a == b {
            return Self::ZERO;
        }
        match (self.as_const(a), self.as_const(b)) {
            (Some(x), Some(y)) => return self.constant(x - y),
            (Some(x), _) if x == 0.0 => return self.neg(b),
            (_, Some(y)) if y == 0.0 => return a,
            _ => {}
        }
        if let Node::Neg(nb) = self.node(b) {
            return self.add(a, nb);
        }
        self.intern_node(Node::Sub(a, b))
    }

    pub fn mul(&mut self, a: ExprId, b: ExprId) -> ExprId {
        match (self.as_const(a), self.as_const(b)) {
            (Some(x), Some(y)) => return self.constant(x * y),
            (Some(x), _) if x == 0.0 => return Self::ZERO,
            (_, Some(y)) if y == 0.0 => return Self::ZERO,
            (Some(x), _) if x == 1.0 => return b,
            (_, Some(y)) if y == 1.0 => return a,
            (Some(x), _) if x == -1.0 => return self.neg(b),
            (_, Some(y)) if y == -1.0 => return self.neg(a),
            _ => {}
        }
        // Fold constant coefficients: c1 * (c2 * x) -> (c1*c2) * x
        if let (Some(c1), Node::Mul(x, y)) = (self.as_const(a), self.node(b))
            && let Some(c2) = self.as_const(x)
        {
            let c = self.constant(c1 * c2);
            return self.mul(c, y);
        }
        if let (Node::Neg(x), Node::Neg(y)) = (self.node(a), self.node(b)) {
            return self.mul(x, y);
        }
        if let Node::Neg(x) = self.node(a) {
            let m = self.mul(x, b);
            return self.neg(m);
        }
        if let Node::Neg(y) = self.node(b) {
            let m = self.mul(a, y);
            return self.neg(m);
        }
        // Constants first, then canonical order.
        let (a, b) = if self.as_const(b).is_some() || (self.as_const(a).is_none() && b < a) {
            (b, a)
        } else {
            (a, b)
        };
        self.intern_node(Node::Mul(a, b))
    }

    pub fn div(&mut self, a: ExprId, b: ExprId) -> ExprId {
        match (self.as_const(a), self.as_const(b)) {
            (Some(x), Some(y)) if y != 0.0 => return self.constant(x / y),
            (Some(x), _) if x == 0.0 => return Self::ZERO,
            (_, Some(y)) if y == 1.0 => return a,
            (_, Some(y)) if y == -1.0 => return self.neg(a),
            (_, Some(y)) if y != 0.0 => {
                let inv = self.constant(1.0 / y);
                return self.mul(inv, a);
            }
            _ => {}
        }
        if a == b {
            return Self::ONE;
        }
        if let Node::Neg(x) = self.node(a) {
            let d = self.div(x, b);
            return self.neg(d);
        }
        self.intern_node(Node::Div(a, b))
    }

    pub fn pow(&mut self, a: ExprId, b: ExprId) -> ExprId {
        match (self.as_const(a), self.as_const(b)) {
            (Some(x), Some(y)) => return self.constant(x.powf(y)),
            (_, Some(y)) if y == 0.0 => return Self::ONE,
            (_, Some(y)) if y == 1.0 => return a,
            (Some(x), _) if x == 1.0 => return Self::ONE,
            _ => {}
        }
        self.intern_node(Node::Pow(a, b))
    }

    pub fn f1(&mut self, f: Fn1, a: ExprId) -> ExprId {
        if let Some(x) = self.as_const(a) {
            return self.constant(eval_f1(f, x));
        }
        if f == Fn1::Abs
            && let Node::F1(Fn1::Abs, _) = self.node(a)
        {
            return a;
        }
        self.intern_node(Node::F1(f, a))
    }

    pub fn f2(&mut self, f: Fn2, a: ExprId, b: ExprId) -> ExprId {
        if let (Some(x), Some(y)) = (self.as_const(a), self.as_const(b)) {
            return self.constant(eval_f2(f, x, y));
        }
        match f {
            Fn2::And => {
                if self.as_const(a) == Some(0.0) || self.as_const(b) == Some(0.0) {
                    return Self::ZERO;
                }
                if self.as_const(a).is_some() {
                    return b;
                }
                if self.as_const(b).is_some() {
                    return a;
                }
            }
            Fn2::Or => {
                if matches!(self.as_const(a), Some(x) if x != 0.0)
                    || matches!(self.as_const(b), Some(x) if x != 0.0)
                {
                    return Self::ONE;
                }
                if self.as_const(a).is_some() {
                    return b;
                }
                if self.as_const(b).is_some() {
                    return a;
                }
            }
            Fn2::Min | Fn2::Max if a == b => return a,
            _ => {}
        }
        self.intern_node(Node::F2(f, a, b))
    }

    pub fn not(&mut self, a: ExprId) -> ExprId {
        self.f1(Fn1::Not, a)
    }

    pub fn cmp(&mut self, op: Cmp, a: ExprId, b: ExprId) -> ExprId {
        if let (Some(x), Some(y)) = (self.as_const(a), self.as_const(b)) {
            return if op.eval(x, y) { Self::ONE } else { Self::ZERO };
        }
        self.intern_node(Node::Cmp(op, a, b))
    }

    /// Event-generating relation. Collapses to a constant when both sides are constant.
    pub fn relation(&mut self, op: Cmp, a: ExprId, b: ExprId) -> ExprId {
        if let (Some(x), Some(y)) = (self.as_const(a), self.as_const(b)) {
            return if op.eval(x, y) { Self::ONE } else { Self::ZERO };
        }
        let r = Relation { op, lhs: a, rhs: b };
        let idx = if let Some(&i) = self.rel_intern.get(&r) {
            i
        } else {
            let i = self.relations.len() as u32;
            self.relations.push(r);
            self.rel_intern.insert(r, i);
            i
        };
        self.intern_node(Node::Rel(idx))
    }

    pub fn if_(&mut self, c: ExprId, a: ExprId, b: ExprId) -> ExprId {
        if let Some(cv) = self.as_const(c) {
            return if cv != 0.0 { a } else { b };
        }
        if a == b {
            return a;
        }
        self.intern_node(Node::If(c, a, b))
    }

    pub fn add_table(&mut self, t: Table) -> u32 {
        self.tables.push(t);
        (self.tables.len() - 1) as u32
    }

    pub fn interp(&mut self, table: u32, col: u32, x: ExprId) -> ExprId {
        if let Some(xv) = self.as_const(x) {
            let v = self.tables[table as usize].interp(col as usize, xv);
            return self.constant(v);
        }
        self.intern_node(Node::Interp(table, col, x))
    }

    pub fn interp_der(&mut self, table: u32, col: u32, x: ExprId) -> ExprId {
        if let Some(xv) = self.as_const(x) {
            let v = self.tables[table as usize].slope(col as usize, xv);
            return self.constant(v);
        }
        self.intern_node(Node::InterpDer(table, col, x))
    }

    /// Children of a node, in evaluation order.
    pub fn children(&self, id: ExprId, out: &mut Vec<ExprId>) {
        match self.node(id) {
            Node::Const(_)
            | Node::Var(_)
            | Node::Der(_)
            | Node::Pre(_)
            | Node::Time
            | Node::Initial => {}
            Node::Rel(r) => {
                let rel = self.relations[r as usize];
                out.push(rel.lhs);
                out.push(rel.rhs);
            }
            Node::Neg(a) | Node::F1(_, a) | Node::Interp(_, _, a) | Node::InterpDer(_, _, a) => {
                out.push(a)
            }
            Node::Add(a, b)
            | Node::Sub(a, b)
            | Node::Mul(a, b)
            | Node::Div(a, b)
            | Node::Pow(a, b)
            | Node::F2(_, a, b)
            | Node::Cmp(_, a, b) => {
                out.push(a);
                out.push(b);
            }
            Node::If(c, a, b) => {
                out.push(c);
                out.push(a);
                out.push(b);
            }
        }
    }

    /// Rebuild a node with new children (in the order given by [`Arena::children`]).
    pub fn rebuild(&mut self, id: ExprId, ch: &[ExprId]) -> ExprId {
        match self.node(id) {
            Node::Const(_)
            | Node::Var(_)
            | Node::Der(_)
            | Node::Pre(_)
            | Node::Time
            | Node::Initial => id,
            Node::Rel(r) => {
                let op = self.relations[r as usize].op;
                self.relation(op, ch[0], ch[1])
            }
            Node::Neg(_) => self.neg(ch[0]),
            Node::F1(f, _) => self.f1(f, ch[0]),
            Node::Interp(t, c, _) => self.interp(t, c, ch[0]),
            Node::InterpDer(t, c, _) => self.interp_der(t, c, ch[0]),
            Node::Add(..) => self.add(ch[0], ch[1]),
            Node::Sub(..) => self.sub(ch[0], ch[1]),
            Node::Mul(..) => self.mul(ch[0], ch[1]),
            Node::Div(..) => self.div(ch[0], ch[1]),
            Node::Pow(..) => self.pow(ch[0], ch[1]),
            Node::F2(f, ..) => self.f2(f, ch[0], ch[1]),
            Node::Cmp(op, ..) => self.cmp(op, ch[0], ch[1]),
            Node::If(..) => self.if_(ch[0], ch[1], ch[2]),
        }
    }

    /// Bottom-up rewrite of `root`. `leaf` maps leaf nodes (returns `None` to keep them).
    /// Memoised per call, so shared sub-DAGs are visited once.
    pub fn rewrite(
        &mut self,
        root: ExprId,
        memo: &mut FxHashMap<ExprId, ExprId>,
        leaf: &mut dyn FnMut(&mut Arena, Node) -> Option<ExprId>,
    ) -> ExprId {
        // Iterative post-order to avoid stack overflow on deep expressions.
        let mut stack: Vec<(ExprId, bool)> = vec![(root, false)];
        let mut ch = Vec::new();
        while let Some((id, expanded)) = stack.pop() {
            if memo.contains_key(&id) {
                continue;
            }
            let n = self.node(id);
            if let Some(r) = leaf(self, n) {
                memo.insert(id, r);
                continue;
            }
            ch.clear();
            self.children(id, &mut ch);
            if ch.is_empty() {
                memo.insert(id, id);
                continue;
            }
            if !expanded {
                stack.push((id, true));
                for &c in ch.iter().rev() {
                    if !memo.contains_key(&c) {
                        stack.push((c, false));
                    }
                }
            } else {
                let new_ch: Vec<ExprId> = ch.iter().map(|c| memo[c]).collect();
                let r = if new_ch == ch {
                    id
                } else {
                    self.rebuild(id, &new_ch)
                };
                memo.insert(id, r);
            }
        }
        memo[&root]
    }

    /// Substitute variables according to `map` (Var and optionally Der/Pre nodes).
    pub fn substitute(&mut self, root: ExprId, map: &dyn Fn(Node) -> Option<ExprId>) -> ExprId {
        let mut memo = FxHashMap::default();
        self.rewrite(root, &mut memo, &mut |_, n| map(n))
    }

    /// Collect all variables referenced (Var, Der and Pre leaves) in post-order.
    /// Look up the node `Var(v)` if it exists (without interning a new one).
    pub fn var_ref(&self, v: VarId) -> ExprId {
        self.intern
            .get(&Node::Var(v))
            .copied()
            .unwrap_or(Self::ZERO)
    }

    /// All variables (including under `der` and `pre`) referenced from `roots`.
    pub fn vars_in(&self, roots: &[ExprId]) -> rustc_hash::FxHashSet<VarId> {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut out = rustc_hash::FxHashSet::default();
        let mut stack: Vec<ExprId> = roots.to_vec();
        let mut ch = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            match self.node(id) {
                Node::Var(v) | Node::Der(v) | Node::Pre(v) => {
                    out.insert(v);
                }
                _ => {}
            }
            ch.clear();
            self.children(id, &mut ch);
            stack.extend(ch.iter().copied());
        }
        out
    }

    /// Visit every leaf node: variables, `der`, `pre`, time, constants, relations, `initial()`.
    pub fn visit_all_leaves(&self, root: ExprId, f: &mut dyn FnMut(Node)) {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![root];
        let mut ch = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            ch.clear();
            self.children(id, &mut ch);
            if ch.is_empty() {
                f(self.node(id));
            }
            stack.extend(ch.iter().copied());
        }
    }

    /// Whether `initial()` occurs in the expression.
    pub fn mentions_initial(&self, root: ExprId) -> bool {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![root];
        let mut ch = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            if matches!(self.node(id), Node::Initial) {
                return true;
            }
            ch.clear();
            self.children(id, &mut ch);
            stack.extend(ch.iter().copied());
        }
        false
    }

    pub fn visit_leaves(&self, root: ExprId, f: &mut dyn FnMut(Node)) {
        let mut seen = rustc_hash::FxHashSet::default();
        let mut stack = vec![root];
        let mut ch = Vec::new();
        while let Some(id) = stack.pop() {
            if !seen.insert(id) {
                continue;
            }
            let n = self.node(id);
            match n {
                Node::Var(_) | Node::Der(_) | Node::Pre(_) | Node::Time => f(n),
                _ => {}
            }
            ch.clear();
            self.children(id, &mut ch);
            stack.extend(ch.iter().copied());
        }
    }

    /// True if `root` references the leaf `target`.
    pub fn contains(&self, root: ExprId, target: Node) -> bool {
        let mut found = false;
        self.visit_leaves(root, &mut |n| {
            if n == target {
                found = true;
            }
        });
        found
    }

    /// Symbolic partial derivative of `e` with respect to the leaf `wrt`.
    /// `der_of` maps a leaf to its time derivative (used for total time derivatives);
    /// for plain partial derivatives pass a closure returning `None`.
    pub fn diff(&mut self, e: ExprId, wrt: Node, memo: &mut FxHashMap<ExprId, ExprId>) -> ExprId {
        let mut seed = |_: &mut Arena, n: Node| -> Option<ExprId> {
            if n == wrt { Some(Self::ONE) } else { None }
        };
        self.forward_diff(e, &mut seed, memo)
    }

    /// Forward-mode symbolic differentiation: `seed` gives the derivative of each leaf
    /// (`None` means zero). Used both for partial derivatives and total time derivatives.
    pub fn forward_diff(
        &mut self,
        root: ExprId,
        seed: &mut dyn FnMut(&mut Arena, Node) -> Option<ExprId>,
        memo: &mut FxHashMap<ExprId, ExprId>,
    ) -> ExprId {
        let mut stack: Vec<(ExprId, bool)> = vec![(root, false)];
        let mut ch = Vec::new();
        while let Some((id, expanded)) = stack.pop() {
            if memo.contains_key(&id) {
                continue;
            }
            let n = self.node(id);
            match n {
                Node::Const(_) | Node::Initial | Node::Rel(_) | Node::Cmp(..) => {
                    memo.insert(id, Self::ZERO);
                    continue;
                }
                Node::Var(_) | Node::Der(_) | Node::Pre(_) | Node::Time => {
                    let d = seed(self, n).unwrap_or(Self::ZERO);
                    memo.insert(id, d);
                    continue;
                }
                _ => {}
            }
            ch.clear();
            self.children(id, &mut ch);
            if !expanded {
                stack.push((id, true));
                for &c in ch.iter().rev() {
                    if !memo.contains_key(&c) {
                        stack.push((c, false));
                    }
                }
                continue;
            }
            let d = |i: usize| memo[&ch[i]];
            let r = match n {
                Node::Neg(_) => {
                    let da = d(0);
                    self.neg(da)
                }
                Node::Add(..) => {
                    let (da, db) = (d(0), d(1));
                    self.add(da, db)
                }
                Node::Sub(..) => {
                    let (da, db) = (d(0), d(1));
                    self.sub(da, db)
                }
                Node::Mul(a, b) => {
                    let (da, db) = (d(0), d(1));
                    let t1 = self.mul(da, b);
                    let t2 = self.mul(a, db);
                    self.add(t1, t2)
                }
                Node::Div(_a, b) => {
                    // (da*b - a*db)/b^2  ==  da/b - (a/b)*db/b
                    let (da, db) = (d(0), d(1));
                    let t1 = self.div(da, b);
                    if self.is_zero(db) {
                        t1
                    } else {
                        let q = self.div(id, b);
                        let t2 = self.mul(q, db);
                        self.sub(t1, t2)
                    }
                }
                Node::Pow(a, b) => {
                    let (da, db) = (d(0), d(1));
                    let mut r = Self::ZERO;
                    if !self.is_zero(da) {
                        // b * a^(b-1) * da
                        let bm1 = self.sub(b, Self::ONE);
                        let p = self.pow(a, bm1);
                        let t = self.mul(b, p);
                        r = self.mul(t, da);
                    }
                    if !self.is_zero(db) {
                        // a^b * log(a) * db
                        let la = self.f1(Fn1::Log, a);
                        let t = self.mul(id, la);
                        let t = self.mul(t, db);
                        r = self.add(r, t);
                    }
                    r
                }
                Node::F1(f, a) => {
                    let da = d(0);
                    if self.is_zero(da) {
                        Self::ZERO
                    } else {
                        let g = self.f1_deriv(f, a, id);
                        self.mul(g, da)
                    }
                }
                Node::F2(f, a, b) => {
                    let (da, db) = (d(0), d(1));
                    match f {
                        Fn2::Atan2 => {
                            // d atan2(a,b) = (b*da - a*db)/(a^2+b^2)
                            let a2 = self.mul(a, a);
                            let b2 = self.mul(b, b);
                            let den = self.add(a2, b2);
                            let t1 = self.mul(b, da);
                            let t2 = self.mul(a, db);
                            let num = self.sub(t1, t2);
                            self.div(num, den)
                        }
                        Fn2::Min => {
                            let c = self.cmp(Cmp::Le, a, b);
                            self.if_(c, da, db)
                        }
                        Fn2::Max => {
                            let c = self.cmp(Cmp::Ge, a, b);
                            self.if_(c, da, db)
                        }
                        Fn2::Mod | Fn2::Rem => da,
                        Fn2::Div | Fn2::And | Fn2::Or => Self::ZERO,
                    }
                }
                Node::If(c, _, _) => {
                    let (da, db) = (d(1), d(2));
                    self.if_(c, da, db)
                }
                Node::Interp(t, c, x) => {
                    let dx = d(0);
                    let s = self.interp_der(t, c, x);
                    self.mul(s, dx)
                }
                Node::InterpDer(..) => Self::ZERO,
                _ => unreachable!(),
            };
            memo.insert(id, r);
        }
        memo[&root]
    }

    fn f1_deriv(&mut self, f: Fn1, a: ExprId, fa: ExprId) -> ExprId {
        match f {
            Fn1::Sin => self.f1(Fn1::Cos, a),
            Fn1::Cos => {
                let s = self.f1(Fn1::Sin, a);
                self.neg(s)
            }
            Fn1::Tan => {
                // 1 + tan^2
                let t2 = self.mul(fa, fa);
                self.add(Self::ONE, t2)
            }
            Fn1::Asin | Fn1::Acos => {
                let a2 = self.mul(a, a);
                let one_m = self.sub(Self::ONE, a2);
                let s = self.f1(Fn1::Sqrt, one_m);
                let r = self.div(Self::ONE, s);
                if f == Fn1::Acos { self.neg(r) } else { r }
            }
            Fn1::Atan => {
                let a2 = self.mul(a, a);
                let den = self.add(Self::ONE, a2);
                self.div(Self::ONE, den)
            }
            Fn1::Sinh => self.f1(Fn1::Cosh, a),
            Fn1::Cosh => self.f1(Fn1::Sinh, a),
            Fn1::Tanh => {
                let t2 = self.mul(fa, fa);
                self.sub(Self::ONE, t2)
            }
            Fn1::Exp => fa,
            Fn1::Log => self.div(Self::ONE, a),
            Fn1::Log10 => {
                let c = self.constant(std::f64::consts::LN_10);
                let den = self.mul(c, a);
                self.div(Self::ONE, den)
            }
            Fn1::Sqrt => {
                let two = self.constant(2.0);
                let den = self.mul(two, fa);
                self.div(Self::ONE, den)
            }
            Fn1::Abs => self.f1(Fn1::Sign, a),
            Fn1::Sign | Fn1::Floor | Fn1::Ceil | Fn1::Not => Self::ZERO,
        }
    }

    /// Human-readable rendering using a variable-name callback.
    pub fn render(&self, id: ExprId, name: &dyn Fn(VarId) -> String) -> String {
        let mut s = String::new();
        self.render_into(id, name, &mut s, 0);
        s
    }

    fn render_into(&self, id: ExprId, name: &dyn Fn(VarId) -> String, s: &mut String, prec: u8) {
        let paren = |s: &mut String, p: u8, f: &mut dyn FnMut(&mut String)| {
            if p < prec {
                s.push('(');
                f(s);
                s.push(')');
            } else {
                f(s);
            }
        };
        match self.node(id) {
            Node::Const(b) => {
                let v = f64::from_bits(b);
                if v < 0.0 && prec > 0 {
                    let _ = write!(s, "({v})");
                } else {
                    let _ = write!(s, "{v}");
                }
            }
            Node::Var(v) => s.push_str(&name(v)),
            Node::Der(v) => {
                let _ = write!(s, "der({})", name(v));
            }
            Node::Pre(v) => {
                let _ = write!(s, "pre({})", name(v));
            }
            Node::Time => s.push_str("time"),
            Node::Initial => s.push_str("initial()"),
            Node::Neg(a) => paren(s, 4, &mut |s| {
                s.push('-');
                self.render_into(a, name, s, 5);
            }),
            Node::Add(a, b) => paren(s, 2, &mut |s| {
                self.render_into(a, name, s, 2);
                s.push_str(" + ");
                self.render_into(b, name, s, 2);
            }),
            Node::Sub(a, b) => paren(s, 2, &mut |s| {
                self.render_into(a, name, s, 2);
                s.push_str(" - ");
                self.render_into(b, name, s, 3);
            }),
            Node::Mul(a, b) => paren(s, 3, &mut |s| {
                self.render_into(a, name, s, 3);
                s.push('*');
                self.render_into(b, name, s, 3);
            }),
            Node::Div(a, b) => paren(s, 3, &mut |s| {
                self.render_into(a, name, s, 3);
                s.push('/');
                self.render_into(b, name, s, 4);
            }),
            Node::Pow(a, b) => paren(s, 5, &mut |s| {
                self.render_into(a, name, s, 6);
                s.push('^');
                self.render_into(b, name, s, 6);
            }),
            Node::F1(f, a) => {
                let n = match f {
                    Fn1::Not => "not ",
                    _ => fn1_name(f),
                };
                if f == Fn1::Not {
                    s.push_str(n);
                    self.render_into(a, name, s, 6);
                } else {
                    s.push_str(n);
                    s.push('(');
                    self.render_into(a, name, s, 0);
                    s.push(')');
                }
            }
            Node::F2(f, a, b) => match f {
                Fn2::And | Fn2::Or => paren(s, 1, &mut |s| {
                    self.render_into(a, name, s, 1);
                    s.push_str(if f == Fn2::And { " and " } else { " or " });
                    self.render_into(b, name, s, 1);
                }),
                _ => {
                    s.push_str(fn2_name(f));
                    s.push('(');
                    self.render_into(a, name, s, 0);
                    s.push_str(", ");
                    self.render_into(b, name, s, 0);
                    s.push(')');
                }
            },
            Node::Cmp(op, a, b) => {
                s.push_str("noEvent(");
                self.render_into(a, name, s, 2);
                let _ = write!(s, " {} ", op.symbol());
                self.render_into(b, name, s, 2);
                s.push(')');
            }
            Node::Rel(r) => {
                let rel = self.relations[r as usize];
                paren(s, 1, &mut |s| {
                    self.render_into(rel.lhs, name, s, 2);
                    let _ = write!(s, " {} ", rel.op.symbol());
                    self.render_into(rel.rhs, name, s, 2);
                });
            }
            Node::If(c, a, b) => paren(s, 0, &mut |s| {
                s.push_str("if ");
                self.render_into(c, name, s, 0);
                s.push_str(" then ");
                self.render_into(a, name, s, 0);
                s.push_str(" else ");
                self.render_into(b, name, s, 0);
            }),
            Node::Interp(t, c, x) => {
                let _ = write!(s, "interpolate(table{t}, {c}, ");
                self.render_into(x, name, s, 0);
                s.push(')');
            }
            Node::InterpDer(t, c, x) => {
                let _ = write!(s, "interpolateSlope(table{t}, {c}, ");
                self.render_into(x, name, s, 0);
                s.push(')');
            }
        }
    }
}

pub fn fn1_name(f: Fn1) -> &'static str {
    match f {
        Fn1::Sin => "sin",
        Fn1::Cos => "cos",
        Fn1::Tan => "tan",
        Fn1::Asin => "asin",
        Fn1::Acos => "acos",
        Fn1::Atan => "atan",
        Fn1::Sinh => "sinh",
        Fn1::Cosh => "cosh",
        Fn1::Tanh => "tanh",
        Fn1::Exp => "exp",
        Fn1::Log => "log",
        Fn1::Log10 => "log10",
        Fn1::Sqrt => "sqrt",
        Fn1::Abs => "abs",
        Fn1::Sign => "sign",
        Fn1::Floor => "floor",
        Fn1::Ceil => "ceil",
        Fn1::Not => "not",
    }
}

pub fn fn2_name(f: Fn2) -> &'static str {
    match f {
        Fn2::Atan2 => "atan2",
        Fn2::Min => "min",
        Fn2::Max => "max",
        Fn2::Div => "div",
        Fn2::Mod => "mod",
        Fn2::Rem => "rem",
        Fn2::And => "and",
        Fn2::Or => "or",
    }
}

#[inline]
pub fn eval_f1(f: Fn1, x: f64) -> f64 {
    match f {
        Fn1::Sin => x.sin(),
        Fn1::Cos => x.cos(),
        Fn1::Tan => x.tan(),
        Fn1::Asin => x.asin(),
        Fn1::Acos => x.acos(),
        Fn1::Atan => x.atan(),
        Fn1::Sinh => x.sinh(),
        Fn1::Cosh => x.cosh(),
        Fn1::Tanh => x.tanh(),
        Fn1::Exp => x.exp(),
        Fn1::Log => x.ln(),
        Fn1::Log10 => x.log10(),
        Fn1::Sqrt => x.sqrt(),
        Fn1::Abs => x.abs(),
        Fn1::Sign => {
            if x > 0.0 {
                1.0
            } else if x < 0.0 {
                -1.0
            } else {
                0.0
            }
        }
        Fn1::Floor => x.floor(),
        Fn1::Ceil => x.ceil(),
        Fn1::Not => {
            if x == 0.0 {
                1.0
            } else {
                0.0
            }
        }
    }
}

#[inline]
pub fn eval_f2(f: Fn2, x: f64, y: f64) -> f64 {
    match f {
        Fn2::Atan2 => x.atan2(y),
        Fn2::Min => x.min(y),
        Fn2::Max => x.max(y),
        Fn2::Div => (x / y).trunc(),
        Fn2::Mod => x - (x / y).floor() * y,
        Fn2::Rem => x - (x / y).trunc() * y,
        Fn2::And => {
            if x != 0.0 && y != 0.0 {
                1.0
            } else {
                0.0
            }
        }
        Fn2::Or => {
            if x != 0.0 || y != 0.0 {
                1.0
            } else {
                0.0
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_and_folding() {
        let mut a = Arena::new();
        let x = a.var(0);
        let y = a.var(1);
        assert_eq!(a.add(x, y), a.add(y, x));
        let two = a.constant(2.0);
        let three = a.constant(3.0);
        let six = a.mul(two, three);
        assert_eq!(a.as_const(six), Some(6.0));
        assert_eq!(a.sub(x, x), Arena::ZERO);
        let zx = a.mul(Arena::ZERO, x);
        assert_eq!(zx, Arena::ZERO);
    }

    #[test]
    fn differentiation() {
        let mut a = Arena::new();
        let x = a.var(0);
        let s = a.f1(Fn1::Sin, x);
        let e = a.mul(x, s); // x*sin(x)
        let mut memo = FxHashMap::default();
        let d = a.diff(e, Node::Var(0), &mut memo);
        // evaluate at x=0.7 by substitution
        let c = a.constant(0.7);
        let v = a.substitute(d, &|n| if n == Node::Var(0) { Some(c) } else { None });
        let expect = 0.7f64.sin() + 0.7 * 0.7f64.cos();
        assert!((a.as_const(v).unwrap() - expect).abs() < 1e-14);
    }

    #[test]
    fn table_modes() {
        let data = vec![0.0, 0.0, 1.0, 1.0, 2.0, 4.0, 3.0, 9.0];
        let mut t = Table::linear(4, 2, data.clone(), 0);
        assert_eq!(t.interp(1, 1.5), 2.5);
        assert_eq!(t.interp(1, 5.0), 9.0);
        t.extrapolation = 1;
        assert_eq!(t.interp(1, 4.0), 14.0);
        t.smoothness = 1;
        assert_eq!(t.interp(1, 1.5), 1.0);
        assert_eq!(t.slope(1, 1.5), 0.0);
        // Periodic with a shift: period 3, so x = 4.5 + 1 maps to 1.5 + 1 = 2.5.
        let mut p = Table::linear(4, 2, data.clone(), 2);
        p.shift = 1.0;
        assert_eq!(p.interp(1, 5.5), p.interp(1, 2.5));
        assert_eq!(p.next_event(4.2), 5.0);
        assert_eq!(p.next_event(6.5), 7.0);
        // Cubic Hermite passes through the points and has a continuous slope.
        let mut c = Table::linear(4, 2, data, 0);
        c.make_cubic(false);
        for (x, y) in [(0.0, 0.0), (1.0, 1.0), (2.0, 4.0), (3.0, 9.0)] {
            assert!((c.interp(1, x) - y).abs() < 1e-12);
        }
        assert!((c.slope(1, 1.0 - 1e-9) - c.slope(1, 1.0 + 1e-9)).abs() < 1e-6);
        let h = 1e-6;
        let fd = (c.interp(1, 1.3 + h) - c.interp(1, 1.3 - h)) / (2.0 * h);
        assert!((fd - c.slope(1, 1.3)).abs() < 1e-6);
        // Start time: zero output and the first event at the start.
        let mut s = Table::linear(2, 2, vec![0.0, 1.0, 1.0, 1.0], 0);
        s.start = 0.5;
        assert_eq!(s.interp(1, 0.2), 0.0);
        assert_eq!(s.next_event(0.2), 0.5);
        assert_eq!(s.interp(Table::NEXT_EVENT as usize, 0.7), 1.0);
    }
}
