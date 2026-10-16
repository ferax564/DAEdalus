// Plot expressions and signal operators: `a.w*60/(2*pi)`, `der(x)`, `abs(y)`, `movavg(x, 0.1)`,
// `fft(y)`, evaluated element-wise over the result's time grid; window statistics.

export type Value = Float64Array | number;

type Node =
  | { k: "num"; v: number }
  | { k: "var"; name: string }
  | { k: "un"; op: string; a: Node }
  | { k: "bin"; op: string; a: Node; b: Node }
  | { k: "call"; f: string; args: Node[] };

/** Functions applied element by element. */
const UNARY: Record<string, (x: number) => number> = {
  abs: Math.abs, sqrt: Math.sqrt, exp: Math.exp, log: Math.log, ln: Math.log, log10: Math.log10,
  sin: Math.sin, cos: Math.cos, tan: Math.tan, asin: Math.asin, acos: Math.acos, atan: Math.atan,
  sinh: Math.sinh, cosh: Math.cosh, tanh: Math.tanh, sign: Math.sign, floor: Math.floor, ceil: Math.ceil,
  sq: (x) => x * x,
};
const BINARY: Record<string, (a: number, b: number) => number> = {
  min: Math.min, max: Math.max, pow: Math.pow, atan2: Math.atan2, mod: (a, b) => a - Math.floor(a / b) * b,
};
/** Signal operators: work on the whole trajectory. */
export const OPERATORS = ["der", "integral", "movavg", "rms", "fft"] as const;
const CONSTS: Record<string, number> = { pi: Math.PI, e: Math.E, inf: Infinity };

export class ExprError extends Error {}

/** Parse an expression; variable names may be dotted with subscripts (`a.b[2].c`). */
export function parse(src: string): Node {
  let i = 0;
  const ws = () => {
    while (i < src.length && /\s/.test(src[i])) i++;
  };
  const peek = () => {
    ws();
    return src[i] ?? "";
  };
  const expect = (c: string) => {
    if (peek() !== c) throw new ExprError(`expected '${c}' at ${i + 1}${src[i] ? ` (found '${src[i]}')` : ""}`);
    i++;
  };
  const ident = (): string => {
    ws();
    let s = "";
    const m = /^[A-Za-z_$][\w$]*/.exec(src.slice(i));
    if (!m) throw new ExprError(`expected a name at ${i + 1}`);
    s += m[0];
    i += m[0].length;
    for (;;) {
      if (src[i] === "[") {
        const j = src.indexOf("]", i);
        if (j < 0) throw new ExprError("unclosed '['");
        s += src.slice(i, j + 1).replace(/\s+/g, "");
        i = j + 1;
      } else if (src[i] === "." && /[A-Za-z_$]/.test(src[i + 1] ?? "")) {
        const m2 = /^\.[A-Za-z_$][\w$]*/.exec(src.slice(i))!;
        s += m2[0];
        i += m2[0].length;
      } else break;
    }
    return s;
  };
  const primary = (): Node => {
    const c = peek();
    if (c === "(") {
      i++;
      const e = expr();
      expect(")");
      return e;
    }
    if (/[0-9.]/.test(c)) {
      const m = /^(\d+\.?\d*|\.\d+)([eE][+-]?\d+)?/.exec(src.slice(i));
      if (!m) throw new ExprError(`bad number at ${i + 1}`);
      i += m[0].length;
      return { k: "num", v: parseFloat(m[0]) };
    }
    const name = ident();
    if (peek() === "(") {
      i++;
      const args: Node[] = [];
      if (peek() !== ")") {
        for (;;) {
          args.push(expr());
          if (peek() === ",") i++;
          else break;
        }
      }
      expect(")");
      return { k: "call", f: name, args };
    }
    if (name in CONSTS) return { k: "num", v: CONSTS[name] };
    return { k: "var", name };
  };
  const unary = (): Node => {
    const c = peek();
    if (c === "-" || c === "+") {
      i++;
      const a = unary();
      return c === "-" ? { k: "un", op: "-", a } : a;
    }
    return power();
  };
  const power = (): Node => {
    const a = primary();
    if (peek() === "^") {
      i++;
      return { k: "bin", op: "^", a, b: unary() };
    }
    return a;
  };
  const term = (): Node => {
    let a = unary();
    for (;;) {
      const c = peek();
      if (c === "*" || c === "/") {
        i++;
        a = { k: "bin", op: c, a, b: unary() };
      } else return a;
    }
  };
  const expr = (): Node => {
    let a = term();
    for (;;) {
      const c = peek();
      if (c === "+" || c === "-") {
        i++;
        a = { k: "bin", op: c, a, b: term() };
      } else return a;
    }
  };
  const e = expr();
  ws();
  if (i < src.length) throw new ExprError(`unexpected '${src[i]}' at ${i + 1}`);
  return e;
}

/** Result variables an expression reads; `der(x)` reads `der(x)` when the result has it. */
export function variables(n: Node, has: (name: string) => boolean): string[] {
  const out = new Set<string>();
  const walk = (n: Node) => {
    if (n.k === "var") {
      if (n.name !== "time") out.add(n.name);
    } else if (n.k === "un") walk(n.a);
    else if (n.k === "bin") {
      walk(n.a);
      walk(n.b);
    } else if (n.k === "call") {
      if (n.f === "der" && n.args.length === 1 && n.args[0].k === "var" && has(`der(${n.args[0].name})`)) out.add(`der(${n.args[0].name})`);
      else n.args.forEach(walk);
    }
  };
  walk(n);
  return [...out];
}

/** Whether the expression is a spectrum (`fft(...)` at the top). */
export function isSpectrum(n: Node): boolean {
  return n.k === "call" && n.f === "fft";
}

function map1(a: Value, f: (x: number) => number): Value {
  if (typeof a === "number") return f(a);
  const o = new Float64Array(a.length);
  for (let i = 0; i < a.length; i++) o[i] = f(a[i]);
  return o;
}

function map2(a: Value, b: Value, f: (x: number, y: number) => number): Value {
  if (typeof a === "number" && typeof b === "number") return f(a, b);
  const n = typeof a === "number" ? (b as Float64Array).length : a.length;
  const o = new Float64Array(n);
  for (let i = 0; i < n; i++) o[i] = f(typeof a === "number" ? a : a[i], typeof b === "number" ? b : b[i]);
  return o;
}

const asArray = (v: Value, n: number) => (typeof v === "number" ? new Float64Array(n).fill(v) : v);

/** Time derivative by differences; rows at the same instant (events) are skipped. */
export function derivative(t: Float64Array, y: Float64Array): Float64Array {
  const n = t.length;
  const d = new Float64Array(n);
  for (let i = 0; i < n; i++) {
    let a = i > 0 ? i - 1 : i;
    let b = i < n - 1 ? i + 1 : i;
    // one-sided across an event (two rows at one time)
    if (b > i && t[b] === t[i]) b = i;
    if (a < i && t[a] === t[i]) a = i;
    const dt = t[b] - t[a];
    d[i] = dt > 0 ? (y[b] - y[a]) / dt : i > 0 ? d[i - 1] : 0;
  }
  return d;
}

/** Running integral (trapezoidal rule) from the first row. */
export function integral(t: Float64Array, y: Float64Array): Float64Array {
  const o = new Float64Array(t.length);
  for (let i = 1; i < t.length; i++) o[i] = o[i - 1] + 0.5 * (y[i] + y[i - 1]) * (t[i] - t[i - 1]);
  return o;
}

/** Moving average over the last `w` seconds (time-weighted); with `square`, the moving RMS. */
export function moving(t: Float64Array, y: Float64Array, w: number, square = false): Float64Array {
  const n = t.length;
  const o = new Float64Array(n);
  const f = square ? (v: number) => v * v : (v: number) => v;
  // cumulative integral of f(y)
  const c = new Float64Array(n);
  for (let i = 1; i < n; i++) c[i] = c[i - 1] + 0.5 * (f(y[i]) + f(y[i - 1])) * (t[i] - t[i - 1]);
  let j = 0;
  for (let i = 0; i < n; i++) {
    const from = t[i] - w;
    while (j < i && t[j] < from) j++;
    // integral from `from` (interpolated in row j-1..j) to t[i]
    let ci = c[j];
    let tj = t[j];
    if (j > 0 && t[j] > from && t[j - 1] < from) {
      const a = (from - t[j - 1]) / (t[j] - t[j - 1]);
      const fy = f(y[j - 1] + a * (y[j] - y[j - 1]));
      ci = c[j] - 0.5 * (fy + f(y[j])) * (t[j] - from);
      tj = from;
    }
    const span = t[i] - tj;
    const v = span > 0 ? (c[i] - ci) / span : f(y[i]);
    o[i] = square ? Math.sqrt(Math.max(0, v)) : v;
  }
  return o;
}

function radix2(re: Float64Array, im: Float64Array) {
  const n = re.length;
  for (let i = 1, j = 0; i < n; i++) {
    let bit = n >> 1;
    for (; j & bit; bit >>= 1) j ^= bit;
    j ^= bit;
    if (i < j) {
      [re[i], re[j]] = [re[j], re[i]];
      [im[i], im[j]] = [im[j], im[i]];
    }
  }
  for (let len = 2; len <= n; len <<= 1) {
    const ang = (-2 * Math.PI) / len;
    const wr = Math.cos(ang);
    const wi = Math.sin(ang);
    for (let i = 0; i < n; i += len) {
      let cr = 1;
      let ci = 0;
      for (let k = 0; k < len / 2; k++) {
        const ar = re[i + k + len / 2] * cr - im[i + k + len / 2] * ci;
        const ai = re[i + k + len / 2] * ci + im[i + k + len / 2] * cr;
        re[i + k + len / 2] = re[i + k] - ar;
        im[i + k + len / 2] = im[i + k] - ai;
        re[i + k] += ar;
        im[i + k] += ai;
        const t = cr * wr - ci * wi;
        ci = cr * wi + ci * wr;
        cr = t;
      }
    }
  }
}

/**
 * Single-sided amplitude spectrum of `y(t)` over `[t0, t1]`: resampled on a uniform grid
 * (a power of two, at most 65536 points), mean removed, Hann window. An input sine of
 * amplitude A shows a peak of about A at its frequency.
 */
export function spectrum(t: Float64Array, y: Float64Array, t0 = t[0], t1 = t[t.length - 1]): { f: Float64Array; a: Float64Array } {
  let count = 0;
  for (let i = 0; i < t.length; i++) if (t[i] >= t0 && t[i] <= t1) count++;
  const n = Math.max(8, Math.min(65536, 1 << Math.floor(Math.log2(Math.max(8, count)))));
  if (!(t1 > t0)) return { f: new Float64Array(), a: new Float64Array() };
  const dt = (t1 - t0) / n;
  const re = new Float64Array(n);
  const im = new Float64Array(n);
  let k = 0;
  let mean = 0;
  for (let i = 0; i < n; i++) {
    const ti = t0 + i * dt;
    while (k < t.length - 2 && t[k + 1] <= ti) k++;
    const d = t[k + 1] - t[k];
    re[i] = d > 0 ? y[k] + ((y[k + 1] - y[k]) * (ti - t[k])) / d : y[k + 1];
    mean += re[i];
  }
  mean /= n;
  let wsum = 0;
  for (let i = 0; i < n; i++) {
    const w = 0.5 - 0.5 * Math.cos((2 * Math.PI * i) / n);
    wsum += w;
    re[i] = (re[i] - mean) * w;
  }
  radix2(re, im);
  const half = n / 2;
  const f = new Float64Array(half + 1);
  const a = new Float64Array(half + 1);
  for (let i = 0; i <= half; i++) {
    f[i] = i / (n * dt);
    a[i] = ((i === 0 || i === half ? 1 : 2) * Math.hypot(re[i], im[i])) / wsum;
  }
  return { f, a };
}

export interface Ctx {
  time: Float64Array;
  /** Trajectory of a result variable (fetched beforehand). */
  get(name: string): Float64Array | undefined;
  has(name: string): boolean;
  /** Time window for `fft` (the visible range). */
  window?: [number, number];
}

/** Evaluate over the time grid. A spectrum returns its frequency axis as `x`. */
export function evaluate(n: Node, ctx: Ctx): { x: Float64Array; y: Float64Array } {
  const N = ctx.time.length;
  const ev = (n: Node): Value => {
    switch (n.k) {
      case "num":
        return n.v;
      case "var": {
        if (n.name === "time") return ctx.time;
        const v = ctx.get(n.name);
        if (!v) throw new ExprError(`unknown variable '${n.name}'`);
        return v;
      }
      case "un":
        return map1(ev(n.a), (x) => -x);
      case "bin": {
        const a = ev(n.a);
        const b = ev(n.b);
        const f = { "+": (x: number, y: number) => x + y, "-": (x: number, y: number) => x - y, "*": (x: number, y: number) => x * y, "/": (x: number, y: number) => x / y, "^": Math.pow }[n.op]!;
        return map2(a, b, f);
      }
      case "call": {
        const args = n.args;
        const arity = (k: number) => {
          if (args.length !== k) throw new ExprError(`${n.f}() takes ${k} argument${k > 1 ? "s" : ""}`);
        };
        if (n.f in UNARY) {
          arity(1);
          return map1(ev(args[0]), UNARY[n.f]);
        }
        if (n.f in BINARY) {
          arity(2);
          return map2(ev(args[0]), ev(args[1]), BINARY[n.f]);
        }
        const scalar = (v: Value, what: string) => {
          if (typeof v !== "number") throw new ExprError(`${what} must be a number`);
          return v;
        };
        switch (n.f) {
          case "der": {
            arity(1);
            const a = args[0];
            if (a.k === "var" && ctx.has(`der(${a.name})`)) return ctx.get(`der(${a.name})`)!;
            return derivative(ctx.time, asArray(ev(a), N));
          }
          case "integral":
          case "int":
            arity(1);
            return integral(ctx.time, asArray(ev(args[0]), N));
          case "movavg":
          case "rms": {
            if (args.length < 1 || args.length > 2) throw new ExprError(`${n.f}(x, window) takes 1 or 2 arguments`);
            const w = args.length > 1 ? scalar(ev(args[1]), "the window") : Infinity;
            return moving(ctx.time, asArray(ev(args[0]), N), w, n.f === "rms");
          }
          case "fft":
            throw new ExprError("fft() can only be the whole expression");
          default:
            throw new ExprError(`unknown function ${n.f}()`);
        }
      }
    }
  };
  if (isSpectrum(n)) {
    if (n.k !== "call" || n.args.length !== 1) throw new ExprError("fft() takes 1 argument");
    const y = asArray(ev(n.args[0]), N);
    const [a, b] = ctx.window ?? [ctx.time[0], ctx.time[N - 1]];
    const s = spectrum(ctx.time, y, a, b);
    return { x: s.f, y: s.a };
  }
  return { x: ctx.time, y: asArray(ev(n), N) };
}

/** Statistics of `y(x)` over `[a, b]` (time-weighted mean and RMS). */
export function windowStats(x: Float64Array, y: Float64Array, a: number, b: number) {
  let min = Infinity;
  let max = -Infinity;
  let int = 0;
  let int2 = 0;
  let span = 0;
  let prev = -1;
  let count = 0;
  for (let i = 0; i < x.length; i++) {
    if (x[i] < a || x[i] > b) continue;
    const v = y[i];
    count++;
    if (v < min) min = v;
    if (v > max) max = v;
    if (prev >= 0) {
      const dt = x[i] - x[prev];
      int += 0.5 * (v + y[prev]) * dt;
      int2 += 0.5 * (v * v + y[prev] * y[prev]) * dt;
      span += dt;
    }
    prev = i;
  }
  if (!count) return null;
  const mean = span > 0 ? int / span : y[prev];
  const rms = span > 0 ? Math.sqrt(int2 / span) : Math.abs(y[prev]);
  return { min, max, mean, rms, p2p: max - min, count };
}
