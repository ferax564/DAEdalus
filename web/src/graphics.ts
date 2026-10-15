// Rendering of Modelica graphical annotations (Icon/Diagram layers) to SVG.
//
// All geometry is mapped to diagram coordinates with explicit affine matrices, so nested
// placements (a connector inside a component inside a diagram) compose exactly as the
// Modelica specification defines, and text can be laid out upright in the final frame.

import type { AnnRecord, AnnValue, ClassInfo, ComponentInfo } from "./types";

export const SVG_NS = "http://www.w3.org/2000/svg";

/** Affine map [a, b, c, d, e, f]: (x, y) -> (a x + c y + e, b x + d y + f). */
export type Mat = [number, number, number, number, number, number];
export const IDENTITY: Mat = [1, 0, 0, 1, 0, 0];

export function mul(m: Mat, n: Mat): Mat {
  return [
    m[0] * n[0] + m[2] * n[1],
    m[1] * n[0] + m[3] * n[1],
    m[0] * n[2] + m[2] * n[3],
    m[1] * n[2] + m[3] * n[3],
    m[0] * n[4] + m[2] * n[5] + m[4],
    m[1] * n[4] + m[3] * n[5] + m[5],
  ];
}
const translate = (x: number, y: number): Mat => [1, 0, 0, 1, x, y];
const scale = (sx: number, sy: number): Mat => [sx, 0, 0, sy, 0, 0];
function rotate(deg: number): Mat {
  const r = (deg * Math.PI) / 180;
  const c = Math.cos(r);
  const s = Math.sin(r);
  return [c, s, -s, c, 0, 0];
}
export function apply(m: Mat, x: number, y: number): [number, number] {
  return [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]];
}

export const num = (v: AnnValue | undefined, d = 0): number => (typeof v === "number" ? v : d);
export function points(v: AnnValue | undefined): [number, number][] {
  if (!Array.isArray(v)) return [];
  return v.filter(Array.isArray).map((p) => [num((p as AnnValue[])[0]), num((p as AnnValue[])[1])]);
}
function color(v: AnnValue | undefined, d: string): string {
  if (Array.isArray(v) && v.length >= 3) return `rgb(${num(v[0])},${num(v[1])},${num(v[2])})`;
  return d;
}
const enumLit = (v: AnnValue | undefined, d: string): string => (typeof v === "string" ? v.split(".").pop()! : d);

/** Extent of a coordinate system ({{-100,-100},{100,100}} by default). */
export function systemExtent(cs: AnnRecord | null | undefined): [[number, number], [number, number]] {
  const e = points(cs?.extent);
  return e.length === 2 ? [e[0], e[1]] : [[-100, -100], [100, 100]];
}

/** Matrix of a Placement's transformation (or iconTransformation) record. */
export function placementMatrix(tr: AnnRecord | undefined, inner: [[number, number], [number, number]]): Mat | null {
  if (!tr) return null;
  const ext = points(tr.extent);
  if (ext.length !== 2) return null;
  const origin = points([tr.origin ?? [0, 0]] as AnnValue)[0] ?? [0, 0];
  const rot = num(tr.rotation);
  const [[cx1, cy1], [cx2, cy2]] = inner;
  const [[x1, y1], [x2, y2]] = ext;
  const sx = (x2 - x1) / (cx2 - cx1 || 1);
  const sy = (y2 - y1) / (cy2 - cy1 || 1);
  let m = mul(translate(x1 - cx1 * sx, y1 - cy1 * sy), scale(sx, sy));
  m = mul(rotate(rot), m);
  return mul(translate(origin[0], origin[1]), m);
}

export interface RenderCtx {
  defs: SVGDefsElement;
  defIds: Map<string, string>;
  /** Text substitutions: %name, %par. */
  subst: (s: string) => string;
  /** Value of a Boolean expression such as `visible = useAxisFlange` (null: unknown). */
  cond: (expr: string) => boolean | null;
}

function el<K extends keyof SVGElementTagNameMap>(tag: K, attrs: Record<string, string | number>): SVGElementTagNameMap[K] {
  const e = document.createElementNS(SVG_NS, tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, String(v));
  return e;
}
const fmt = (x: number) => (Math.abs(x) < 1e-9 ? "0" : x.toFixed(3).replace(/\.?0+$/, ""));
const matAttr = (m: Mat) => `matrix(${m.map(fmt).join(" ")})`;

function rgbOf(c: AnnValue | undefined): [number, number, number] {
  return Array.isArray(c) && c.length >= 3 ? [num(c[0]), num(c[1]), num(c[2])] : [0, 0, 0];
}
const mix = (c: [number, number, number], w: number, t: [number, number, number] = [255, 255, 255]) =>
  `rgb(${c.map((v, i) => Math.round(v * (1 - w) + t[i] * w)).join(",")})`;

function fillOf(ctx: RenderCtx, g: AnnRecord): string {
  const pat = enumLit(g.fillPattern, "None");
  if (pat === "None") return "none";
  const c = rgbOf(g.fillColor);
  const base = color(g.fillColor, "black");
  if (pat === "Solid") return base;
  const key = `${pat}:${c.join(",")}`;
  let id = ctx.defIds.get(key);
  if (!id) {
    id = `g${ctx.defIds.size}_${Math.random().toString(36).slice(2, 7)}`;
    ctx.defIds.set(key, id);
    if (pat === "HorizontalCylinder" || pat === "VerticalCylinder") {
      const lg = el("linearGradient", pat === "HorizontalCylinder" ? { id, x1: 0, y1: 0, x2: 0, y2: 1 } : { id, x1: 0, y1: 0, x2: 1, y2: 0 });
      for (const [o, col] of [[0, mix(c, 0, [0, 0, 0])], [0.45, mix(c, 0.75)], [1, mix(c, 0.25, [0, 0, 0])]] as const) {
        lg.appendChild(el("stop", { offset: o, "stop-color": col }));
      }
      ctx.defs.appendChild(lg);
    } else if (pat === "Sphere") {
      const rg = el("radialGradient", { id, cx: 0.4, cy: 0.4, r: 0.65 });
      rg.appendChild(el("stop", { offset: 0, "stop-color": mix(c, 0.85) }));
      rg.appendChild(el("stop", { offset: 1, "stop-color": base }));
      ctx.defs.appendChild(rg);
    } else {
      const p = el("pattern", { id, width: 6, height: 6, patternUnits: "userSpaceOnUse" });
      const d =
        pat === "Horizontal" ? "M0 3H6" : pat === "Vertical" ? "M3 0V6" : pat === "Cross" ? "M0 3H6M3 0V6" : pat === "Forward" ? "M0 6L6 0" : pat === "Backward" ? "M0 0L6 6" : "M0 6L6 0M0 0L6 6";
      p.appendChild(el("path", { d, stroke: base, "stroke-width": 1 }));
      ctx.defs.appendChild(p);
    }
  }
  return `url(#${id})`;
}

function strokeAttrs(g: AnnRecord, key: "color" | "lineColor"): Record<string, string | number> {
  const pattern = enumLit(key === "color" ? g.pattern : g.pattern ?? g.borderPattern, "Solid");
  const thick = num(key === "color" ? g.thickness : g.lineThickness, 0.25);
  const a: Record<string, string | number> = {
    stroke: pattern === "None" ? "none" : color(g[key], "black"),
    "stroke-width": Math.max(1, thick * 4),
    "vector-effect": "non-scaling-stroke",
    "stroke-linejoin": "round",
  };
  const dash: Record<string, string> = { Dash: "6 3", Dot: "1.5 3", DashDot: "6 3 1.5 3", DashDotDot: "6 3 1.5 3 1.5 3" };
  if (dash[pattern]) a["stroke-dasharray"] = dash[pattern];
  return a;
}

/** Local transform of a graphic item (its own origin and rotation). */
function itemMatrix(g: AnnRecord): Mat {
  const o = points([g.origin ?? [0, 0]] as AnnValue)[0] ?? [0, 0];
  return mul(translate(o[0], o[1]), rotate(num(g.rotation)));
}

function pathOf(pts: [number, number][], m: Mat, smooth: boolean, closed: boolean): string {
  const p = pts.map(([x, y]) => apply(m, x, y));
  if (p.length === 0) return "";
  if (!smooth || p.length < 3) {
    return "M" + p.map(([x, y]) => `${fmt(x)} ${fmt(y)}`).join("L") + (closed ? "Z" : "");
  }
  // Modelica Smooth.Bezier: quadratic segments through midpoints
  let d = `M${fmt(p[0][0])} ${fmt(p[0][1])}`;
  for (let i = 1; i < p.length - 1; i++) {
    const mx = (p[i][0] + p[i + 1][0]) / 2;
    const my = (p[i][1] + p[i + 1][1]) / 2;
    d += `Q${fmt(p[i][0])} ${fmt(p[i][1])} ${fmt(i === p.length - 2 ? p[i + 1][0] : mx)} ${fmt(i === p.length - 2 ? p[i + 1][1] : my)}`;
  }
  return d + (closed ? "Z" : "");
}

function arrowHead(parent: SVGGElement, tip: [number, number], from: [number, number], size: number, filled: boolean, stroke: string) {
  const dx = tip[0] - from[0];
  const dy = tip[1] - from[1];
  const L = Math.hypot(dx, dy) || 1;
  const ux = dx / L;
  const uy = dy / L;
  const b: [number, number] = [tip[0] - ux * size, tip[1] - uy * size];
  const w = size * 0.4;
  const p1 = [b[0] - uy * w, b[1] + ux * w];
  const p2 = [b[0] + uy * w, b[1] - ux * w];
  parent.appendChild(
    el("path", {
      d: `M${fmt(p1[0])} ${fmt(p1[1])}L${fmt(tip[0])} ${fmt(tip[1])}L${fmt(p2[0])} ${fmt(p2[1])}${filled ? "Z" : ""}`,
      fill: filled ? stroke : "none",
      stroke,
      "vector-effect": "non-scaling-stroke",
    }),
  );
}

/** Draw a list of graphic primitives into `parent` with transform `m` (Modelica -> diagram coords). */
export function drawGraphics(parent: SVGGElement, graphics: AnnRecord[], m: Mat, ctx: RenderCtx) {
  for (const g of graphics) {
    if (g.visible === false) continue;
    if (typeof g.visible === "string" && ctx.cond(g.visible) === false) continue;
    const mm = mul(m, itemMatrix(g));
    const kind = g["@"];
    if (kind === "Line") {
      const pts = points(g.points);
      if (pts.length < 2) continue;
      const sa = strokeAttrs(g, "color");
      parent.appendChild(el("path", { d: pathOf(pts, mm, enumLit(g.smooth, "None") === "Bezier", false), fill: "none", ...sa }));
      const arrows = Array.isArray(g.arrow) ? g.arrow.map((a) => enumLit(a, "None")) : ["None", "None"];
      const size = num(g.arrowSize, 3) * Math.hypot(mm[0], mm[1]);
      const tp = pts.map(([x, y]) => apply(mm, x, y));
      if (arrows[0] !== "None") arrowHead(parent, tp[0], tp[1], size, arrows[0] === "Filled", String(sa.stroke));
      if (arrows[1] !== "None") arrowHead(parent, tp[tp.length - 1], tp[tp.length - 2], size, arrows[1] === "Filled", String(sa.stroke));
    } else if (kind === "Polygon") {
      const pts = points(g.points);
      if (pts.length < 2) continue;
      parent.appendChild(el("path", { d: pathOf(pts, mm, enumLit(g.smooth, "None") === "Bezier", true), fill: fillOf(ctx, g), ...strokeAttrs(g, "lineColor") }));
    } else if (kind === "Rectangle" || kind === "Ellipse" || kind === "Bitmap") {
      const e = points(g.extent);
      if (e.length !== 2) continue;
      const x = Math.min(e[0][0], e[1][0]);
      const y = Math.min(e[0][1], e[1][1]);
      const w = Math.abs(e[1][0] - e[0][0]);
      const h = Math.abs(e[1][1] - e[0][1]);
      const grp = el("g", { transform: matAttr(mm) });
      let shape: SVGElement;
      if (kind === "Ellipse") {
        const sa = num(g.startAngle, 0);
        const ea = num(g.endAngle, 360);
        if (Math.abs(ea - sa) >= 360) {
          shape = el("ellipse", { cx: x + w / 2, cy: y + h / 2, rx: w / 2, ry: h / 2 });
        } else {
          const cx = x + w / 2;
          const cy = y + h / 2;
          const at = (a: number) => [cx + (w / 2) * Math.cos((a * Math.PI) / 180), cy + (h / 2) * Math.sin((a * Math.PI) / 180)];
          const [x0, y0] = at(sa);
          const [x1, y1] = at(ea);
          const large = (((ea - sa) % 360) + 360) % 360 > 180 ? 1 : 0;
          const closure = enumLit(g.closure, "Chord");
          shape = el("path", { d: `M${x0} ${y0}A${w / 2} ${h / 2} 0 ${large} 1 ${x1} ${y1}${closure === "Radial" ? `L${cx} ${cy}Z` : closure === "Chord" ? "Z" : ""}` });
        }
      } else {
        const r = num(g.radius);
        shape = el("rect", { x, y, width: w, height: h, rx: r, ry: r });
      }
      if (kind === "Bitmap") {
        shape.setAttribute("fill", "rgba(200,200,200,0.25)");
        shape.setAttribute("stroke", "#aaa");
        shape.setAttribute("vector-effect", "non-scaling-stroke");
      } else {
        shape.setAttribute("fill", fillOf(ctx, g));
        for (const [k, v] of Object.entries(strokeAttrs(g, "lineColor"))) shape.setAttribute(k, String(v));
      }
      grp.appendChild(shape);
      parent.appendChild(grp);
    } else if (kind === "Text") {
      const e = points(g.extent);
      if (e.length !== 2) continue;
      const raw = typeof g.textString === "string" ? g.textString : typeof g.string === "string" ? g.string : "";
      const text = ctx.subst(raw);
      if (!text) continue;
      // upright text in the bounding box of the transformed extent (diagram coords, y up)
      const c = [apply(mm, e[0][0], e[0][1]), apply(mm, e[1][0], e[0][1]), apply(mm, e[1][0], e[1][1]), apply(mm, e[0][0], e[1][1])];
      const xs = c.map((p) => p[0]);
      const ys = c.map((p) => p[1]);
      const bx = Math.min(...xs);
      const by = Math.min(...ys);
      const bw = Math.max(...xs) - bx;
      const bh = Math.max(...ys) - by;
      const scaleLen = Math.hypot(m[0], m[1]);
      // Text turns with its component (a vertical label in a quarter-turned box) but is
      // never drawn upside down; it fits the extent measured along the item's own axes.
      const ew = Math.abs(e[1][0] - e[0][0]) * Math.hypot(mm[0], mm[1]);
      const eh = Math.abs(e[1][1] - e[0][1]) * Math.hypot(mm[2], mm[3]);
      let deg = (Math.atan2(mm[1], mm[0]) * 180) / Math.PI;
      if (deg > 90.5) deg -= 180;
      else if (deg <= -89.5) deg += 180;
      deg = Math.round(deg * 1000) / 1000;
      const fs = num(g.fontSize) > 0 ? num(g.fontSize) * 0.35 * scaleLen * 2.83 : Math.min(eh * 0.9, (ew / Math.max(1, text.length)) * 1.7);
      if (fs < 0.5) continue;
      const align = enumLit(g.horizontalAlignment, "Center");
      const cx = bx + bw / 2;
      const cy = by + bh / 2;
      const off = align === "Left" ? -ew / 2 : align === "Right" ? ew / 2 : 0;
      const t = el("text", {
        x: off,
        y: 0,
        "font-size": fs,
        "text-anchor": align === "Left" ? "start" : align === "Right" ? "end" : "middle",
        "dominant-baseline": "central",
        fill: color(g.textColor ?? g.lineColor, "black"),
        transform: `scale(1,-1) translate(${cx},${-cy})${deg ? ` rotate(${-deg})` : ""}`,
        "font-family": typeof g.fontName === "string" && g.fontName ? g.fontName : "inherit",
      });
      const styles = Array.isArray(g.textStyle) ? g.textStyle.map((s) => enumLit(s, "")) : [];
      if (styles.includes("Bold")) t.setAttribute("font-weight", "bold");
      if (styles.includes("Italic")) t.setAttribute("font-style", "italic");
      t.textContent = text;
      parent.appendChild(t);
    }
  }
}

/** Fetch-and-cache access to class information. */
export interface ClassSource {
  get(path: string): Promise<ClassInfo | null>;
}

/** Value text of parameter `name` for component `comp` of class `cls`: its modifier, or the
 * default binding in the class or its bases. */
function paramText(name: string, comp: ComponentInfo | null, cls: ClassInfo, classes: Map<string, ClassInfo>): string | null {
  const mod = comp?.modifiers.find(([k]) => k === name);
  if (mod) return mod[1];
  for (const c of [cls, ...cls.extends.map((e) => classes.get(e)).filter((x): x is ClassInfo => !!x)]) {
    const p = c.components.find((x) => x.name === name);
    if (p?.binding) return p.binding;
  }
  return null;
}

/** Evaluate a small Boolean expression (`not a and (b or c)`, `n > 1`); null if unknown. */
export function evalCond(expr: string, lookup: (name: string) => string | null, depth = 0): boolean | null {
  if (depth > 8) return null;
  const toks = expr.match(/[A-Za-z_][A-Za-z0-9_.]*|\d+(?:\.\d*)?(?:[eE][-+]?\d+)?|==|<>|<=|>=|[()<>]/g) ?? [];
  let i = 0;
  type V = boolean | number | null;
  const atom = (): V => {
    const t = toks[i++];
    if (t === undefined) return null;
    if (t === "(") {
      const v = or();
      i++;
      return v;
    }
    if (t === "not") {
      const v = atom();
      return typeof v === "boolean" ? !v : null;
    }
    if (t === "true") return true;
    if (t === "false") return false;
    if (/^\d/.test(t)) return parseFloat(t);
    const s = lookup(t);
    if (s === null) return null;
    if (s.trim() === "true") return true;
    if (s.trim() === "false") return false;
    const x = Number(s);
    if (s.trim() !== "" && !Number.isNaN(x)) return x;
    return evalCond(s, lookup, depth + 1);
  };
  const cmp = (): V => {
    const a = atom();
    const op = toks[i];
    if (op && ["==", "<>", "<", ">", "<=", ">="].includes(op)) {
      i++;
      const b = atom();
      if (a === null || b === null) return null;
      const [x, y] = [Number(a), Number(b)];
      return op === "==" ? x === y : op === "<>" ? x !== y : op === "<" ? x < y : op === ">" ? x > y : op === "<=" ? x <= y : x >= y;
    }
    return a;
  };
  const and = (): V => {
    let a = cmp();
    while (toks[i] === "and") {
      i++;
      const b = cmp();
      a = a === false || b === false ? false : a === null || b === null ? null : Boolean(a) && Boolean(b);
    }
    return a;
  };
  const or = (): V => {
    let a = and();
    while (toks[i] === "or") {
      i++;
      const b = and();
      a = a === true || b === true ? true : a === null || b === null ? null : Boolean(a) || Boolean(b);
    }
    return a;
  };
  const v = or();
  return typeof v === "boolean" && i === toks.length ? v : null;
}

/** Boolean expressions of a component's icon, evaluated with its parameters. */
export function condition(comp: ComponentInfo | null, cls: ClassInfo, classes: Map<string, ClassInfo>): (expr: string) => boolean | null {
  return (expr) => evalCond(expr, (n) => paramText(n, comp, cls, classes));
}

const CONSTANTS: Record<string, number> = {
  pi: Math.PI,
  e: Math.E,
  "Modelica.Constants.pi": Math.PI,
  "Modelica.Constants.e": Math.E,
  "Modelica.Constants.D2R": Math.PI / 180,
  "Modelica.Constants.R2D": 180 / Math.PI,
  "Modelica.Constants.g_n": 9.80665,
  "Modelica.Constants.sigma": 5.670374419e-8,
  "Modelica.Constants.T_zero": -273.15,
};
const FUNS: Record<string, (x: number) => number> = {
  sqrt: Math.sqrt, sin: Math.sin, cos: Math.cos, tan: Math.tan, exp: Math.exp, log: Math.log, log10: Math.log10, abs: Math.abs,
};

/** Evaluate an arithmetic parameter expression (`1/(2*Modelica.Constants.pi)`); null if unknown. */
export function evalNum(expr: string, lookup: (name: string) => string | null, depth = 0): number | null {
  if (depth > 8) return null;
  const toks = expr.match(/[A-Za-z_][A-Za-z0-9_.]*|\d+(?:\.\d*)?(?:[eE][-+]?\d+)?|\.\d+(?:[eE][-+]?\d+)?|[-+*/^(),]|\S/g) ?? [];
  let i = 0;
  let bad = false;
  const prim = (): number => {
    const t = toks[i++];
    if (t === undefined) return (bad = true), NaN;
    if (t === "(") {
      const v = sum();
      if (toks[i++] !== ")") bad = true;
      return v;
    }
    if (t === "-") return -pow();
    if (t === "+") return pow();
    if (/^[\d.]/.test(t)) return parseFloat(t);
    if (/^[A-Za-z_]/.test(t)) {
      const f = FUNS[t.replace(/^Modelica\.Math\./, "")];
      if (f && toks[i] === "(") {
        i++;
        const v = sum();
        if (toks[i++] !== ")") bad = true;
        return f(v);
      }
      if (t in CONSTANTS) return CONSTANTS[t];
      const s = lookup(t);
      const v = s === null ? null : evalNum(s, lookup, depth + 1);
      if (v === null) bad = true;
      return v ?? NaN;
    }
    bad = true;
    return NaN;
  };
  const pow = (): number => {
    const a = prim();
    if (toks[i] === "^") {
      i++;
      return a ** pow();
    }
    return a;
  };
  const prod = (): number => {
    let a = pow();
    while (toks[i] === "*" || toks[i] === "/") a = toks[i++] === "*" ? a * pow() : a / pow();
    return a;
  };
  const sum = (): number => {
    let a = prod();
    while (toks[i] === "+" || toks[i] === "-") a = toks[i++] === "+" ? a + prod() : a - prod();
    return a;
  };
  const v = sum();
  return bad || i !== toks.length || !Number.isFinite(v) ? null : v;
}

/** A parameter value as icon text: computed expressions shown as their number. */
function shownValue(v: string, lookup: (name: string) => string | null): string {
  if (!/[-+*/^(]/.test(v.replace(/^-/, "")) && !/[A-Za-z]/.test(v)) return v;
  if (/^\s*(true|false|"[^"]*")\s*$/.test(v)) return v;
  const x = evalNum(v, lookup);
  if (x === null) return v;
  return String(Number(x.toPrecision(4)));
}

function substitution(comp: ComponentInfo | null, cls: ClassInfo, classes: Map<string, ClassInfo>): (s: string) => string {
  const lookup = (n: string) => paramText(n, comp, cls, classes);
  return (s: string) =>
    s.replace(/%%|%\{?([A-Za-z_][A-Za-z0-9_.]*)\}?/g, (all, name: string | undefined) => {
      if (all === "%%") return "%";
      if (!name) return all;
      if (name === "name") return comp?.name ?? cls.name;
      if (name === "class") return cls.name;
      const v = lookup(name);
      return v === null ? name : shownValue(v, lookup);
    });
}

/**
 * Render the icon of `cls` (with its public connectors) into `parent` under transform `m`.
 * `comp` is the component being drawn (for %name and parameter substitution).
 */
export async function drawIcon(
  parent: SVGGElement,
  cls: ClassInfo,
  comp: ComponentInfo | null,
  m: Mat,
  src: ClassSource,
  ctx: RenderCtx,
  classes: Map<string, ClassInfo>,
  depth = 0,
): Promise<void> {
  const saved = ctx.subst;
  const savedCond = ctx.cond;
  ctx.subst = substitution(comp, cls, classes);
  ctx.cond = condition(comp, cls, classes);
  drawGraphics(parent, cls.icon.graphics, m, ctx);
  ctx.subst = saved;
  const visible = ctx.cond;
  ctx.cond = savedCond;
  if (depth > 2) return;
  const inner = systemExtent(cls.icon.coordinate_system);
  for (const c of cls.components) {
    if (c.protected || c.type_kind !== "connector" || !c.type_path || !c.placement) continue;
    // A conditional connector that is switched off (`heatPort if useHeatPort`) is not drawn.
    if (c.condition && visible(c.condition) === false) continue;
    const pl = c.placement as AnnRecord;
    const tr = (pl.iconTransformation ?? pl.transformation) as AnnRecord | undefined;
    const sub = await src.get(c.type_path);
    if (!sub) continue;
    const pm = placementMatrix(tr, systemExtent(sub.icon.coordinate_system));
    if (!pm) continue;
    const g = el("g", { "data-connector": c.name });
    parent.appendChild(g);
    await drawIcon(g, sub, null, mul(m, pm), src, ctx, classes, depth + 1);
  }
  void inner;
}

export function newCtx(svg: SVGSVGElement): RenderCtx {
  let defs = svg.querySelector("defs");
  if (!defs) {
    defs = el("defs", {});
    svg.appendChild(defs);
  }
  return { defs, defIds: new Map(), subst: (s) => s, cond: () => null };
}

export { el };
