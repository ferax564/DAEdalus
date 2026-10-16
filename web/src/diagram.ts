// Icon and diagram views of a class: SVG canvas with pan, zoom, selection and tooltips.

import type { AnnRecord, ClassInfo, ComponentInfo } from "./types";
import {
  apply as applyM,
  condition,
  type ClassSource,
  type Mat,
  diagramInk,
  drawGraphics,
  drawIcon,
  el,
  IDENTITY,
  mul,
  newCtx,
  placementMatrix,
  points,
  SVG_NS,
  systemExtent,
} from "./graphics";

export type ViewKind = "icon" | "diagram";

export class DiagramView {
  readonly svg: SVGSVGElement;
  private root: SVGGElement;
  private view = { x: -110, y: -110, w: 220, h: 220 };
  private drag: { x: number; y: number; vx: number; vy: number; id: number; moved: boolean } | null = null;
  private renderToken = 0;
  onSelect: (c: ComponentInfo | null) => void = () => {};
  onOpen: (typePath: string) => void = () => {};
  /** The last diagram drawn and its placement matrices (for the diagram editor). */
  rendered: { cls: ClassInfo; mats: Map<string, Mat>; root: SVGGElement } | null = null;
  /** Called after every render of a class. */
  onRendered: (kind: ViewKind) => void = () => {};
  /** A component was clicked (after `onSelect`), with the click. */
  onComponentClick: (c: ComponentInfo, ev: MouseEvent) => void = () => {};
  /** A connection line was clicked. */
  onConnection: ((from: string, to: string, ev: MouseEvent) => void) | null = null;

  constructor(host: HTMLElement, private src: ClassSource, private classes: Map<string, ClassInfo>) {
    this.svg = document.createElementNS(SVG_NS, "svg");
    this.svg.classList.add("diagram");
    this.root = el("g", { transform: "scale(1,-1)" });
    this.svg.appendChild(this.root);
    host.appendChild(this.svg);
    this.svg.addEventListener("wheel", (e) => this.zoom(e), { passive: false });
    this.svg.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 && e.button !== 1) return;
      this.drag = { x: e.clientX, y: e.clientY, vx: this.view.x, vy: this.view.y, id: e.pointerId, moved: false };
    });
    this.svg.addEventListener("pointermove", (e) => {
      if (!this.drag) return;
      // Capture only once this is a pan, so plain clicks still reach the components.
      if (!this.drag.moved) {
        if (Math.hypot(e.clientX - this.drag.x, e.clientY - this.drag.y) < 4) return;
        this.drag.moved = true;
        this.svg.setPointerCapture(this.drag.id);
      }
      const k = this.view.w / this.svg.clientWidth;
      this.view.x = this.drag.vx - (e.clientX - this.drag.x) * k;
      this.view.y = this.drag.vy - (e.clientY - this.drag.y) * k;
      this.applyView();
    });
    const endDrag = () => {
      if (this.drag?.moved) {
        // Swallow the click that ends a pan so it does not select a component.
        this.svg.addEventListener("click", (ev) => ev.stopPropagation(), { capture: true, once: true });
      }
      this.drag = null;
    };
    this.svg.addEventListener("pointerup", endDrag);
    this.svg.addEventListener("pointercancel", () => (this.drag = null));
    new ResizeObserver(() => this.applyView()).observe(this.svg);
  }

  private applyView() {
    const aspect = this.svg.clientWidth / Math.max(1, this.svg.clientHeight);
    const h = this.view.w / (aspect || 1);
    this.svg.setAttribute("viewBox", `${this.view.x} ${this.view.y} ${this.view.w} ${h}`);
  }

  private zoom(e: WheelEvent) {
    e.preventDefault();
    const r = this.svg.getBoundingClientRect();
    const fx = (e.clientX - r.left) / r.width;
    const fy = (e.clientY - r.top) / r.height;
    const aspect = r.width / Math.max(1, r.height);
    const f = Math.exp(e.deltaY * 0.0015);
    const w = this.view.w * f;
    this.view.x += (this.view.w - w) * fx;
    this.view.y += (this.view.w / aspect - w / aspect) * fy;
    this.view.w = w;
    this.applyView();
  }

  fit(ext: [[number, number], [number, number]]) {
    const [[x1, y1], [x2, y2]] = ext;
    const pad = 0.08 * Math.max(x2 - x1, y2 - y1);
    const aspect = this.svg.clientWidth / Math.max(1, this.svg.clientHeight) || 1;
    const w = Math.max(x2 - x1 + 2 * pad, (y2 - y1 + 2 * pad) * aspect);
    const h = w / aspect;
    this.view = { x: (x1 + x2) / 2 - w / 2, y: -(y1 + y2) / 2 - h / 2, w, h };
    this.applyView();
  }

  /** Draw a class; `keepView` keeps the current pan and zoom (after an edit). */
  async show(cls: ClassInfo, kind: ViewKind, keepView = false) {
    const token = ++this.renderToken;
    const g = el("g", { transform: "scale(1,-1)" });
    const ctx = newCtx(this.svg);
    let mats = new Map<string, Mat>();
    const layer = kind === "icon" ? cls.icon : cls.diagram;
    const ext = systemExtent(layer.coordinate_system);
    // drawing frame
    const [[x1, y1], [x2, y2]] = ext;
    g.appendChild(el("rect", { x: Math.min(x1, x2), y: Math.min(y1, y2), width: Math.abs(x2 - x1), height: Math.abs(y2 - y1), class: "frame" }));
    if (kind === "icon") {
      await drawIcon(g, cls, null, IDENTITY, this.src, ctx, this.classes);
    } else {
      ctx.subst = (s) => s.replace("%name", cls.name);
      ctx.cond = condition(null, cls, this.classes);
      drawGraphics(g, cls.diagram.graphics, IDENTITY, ctx);
      mats = await this.layout(cls);
      if (token !== this.renderToken) return;
      // Conditional components switched off by their parameters are not drawn.
      const off = new Set(cls.components.filter((c) => c.condition && ctx.cond(c.condition) === false).map((c) => c.name));
      for (const comp of cls.components) {
        const m = mats.get(comp.name);
        if (!m || off.has(comp.name)) continue;
        await this.drawComponent(g, comp, m, ctx);
        if (token !== this.renderToken) return;
      }
      const head = (r: string) => r.split(".")[0].replace(/\[.*$/, "");
      for (const c of cls.connections) {
        if (off.has(head(c.from)) || off.has(head(c.to))) continue;
        if (c.line) this.drawConnection(g, c.line, c.from, c.to);
        else await this.drawAutoConnection(g, cls, mats, c.from, c.to);
      }
    }
    if (token !== this.renderToken) return;
    this.svg.replaceChild(g, this.root);
    this.root = g;
    if (!keepView) this.fit(this.contentExtent(ext));
    this.rendered = kind === "diagram" ? { cls, mats, root: g } : null;
    this.onRendered(kind);
  }

  private contentExtent(ext: [[number, number], [number, number]]): [[number, number], [number, number]] {
    try {
      const b = this.root.getBBox();
      if (b.width > 0 && b.height > 0) {
        // getBBox is in the root's own user space, before its scale(1,-1): Modelica coordinates.
        const [[ax, ay], [bx, by]] = ext;
        return [
          [Math.min(b.x, ax, bx), Math.min(b.y, ay, by)],
          [Math.max(b.x + b.width, ax, bx), Math.max(b.y + b.height, ay, by)],
        ];
      }
    } catch {
      /* not laid out yet */
    }
    return ext;
  }

  private drawConnection(g: SVGGElement, line: AnnRecord | null, from = "", to = "") {
    if (!line) return;
    const pts = points(line.points);
    if (pts.length < 2) return;
    const ctx = newCtx(this.svg);
    const grp = el("g", { class: "connection", "data-from": from, "data-to": to });
    drawGraphics(grp, [{ ...line, "@": "Line" }], IDENTITY, ctx);
    g.appendChild(grp);
    this.connectionHit(g, pts.map((p) => [p[0], p[1]] as [number, number]), from, to);
  }

  /** Wide transparent stroke over a connection that picks it (plotting its variables). */
  private connectionHit(g: SVGGElement, pts: [number, number][], from: string, to: string) {
    const hit = el("path", { d: "M" + pts.map((p) => `${p[0]} ${p[1]}`).join("L"), class: "connection-hit", "data-from": from, "data-to": to });
    hit.addEventListener("click", (e) => {
      if (!this.onConnection) return;
      e.stopPropagation();
      this.onConnection(from, to, e as MouseEvent);
    });
    g.appendChild(hit);
  }

  private async drawComponent(g: SVGGElement, comp: ComponentInfo, m: Mat, ctx: ReturnType<typeof newCtx>) {
    const cls = await this.src.get(comp.type_path!);
    if (!cls) return;
    const grp = el("g", { class: "component", "data-name": comp.name });
    const title = document.createElementNS(SVG_NS, "title");
    title.textContent = `${comp.name} : ${comp.type_name}${comp.description ? `\n${comp.description}` : ""}`;
    grp.appendChild(title);
    if (cls.icon.graphics.length === 0) {
      // classes without an Icon annotation get a plain labelled box, as in Dymola
      drawGraphics(
        grp,
        [
          { "@": "Rectangle", extent: [[-100, -100], [100, 100]], lineColor: [110, 120, 140], fillColor: [246, 247, 250], fillPattern: "FillPattern.Solid", radius: 6 },
          { "@": "Text", extent: [[-90, 30], [90, -30]], textString: comp.name, textColor: [40, 50, 70] },
        ],
        m,
        ctx,
      );
    }
    await drawIcon(grp, cls, comp, m, this.src, ctx, this.classes);
    // transparent hit box over the placement extent
    const hit = el("path", { class: "hit", d: "" });
    const [[a, b], [c, d]] = systemExtent(cls.icon.coordinate_system);
    const corners = [
      [a, b],
      [c, b],
      [c, d],
      [a, d],
    ].map(([x, y]) => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]);
    hit.setAttribute("d", "M" + corners.map((p) => p.join(" ")).join("L") + "Z");
    grp.prepend(hit);
    grp.addEventListener("click", (e) => {
      e.stopPropagation();
      this.svg.querySelectorAll(".component.selected").forEach((n) => n.classList.remove("selected"));
      grp.classList.add("selected");
      this.onSelect(comp);
      this.onComponentClick(comp, e);
    });
    grp.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      if (comp.type_path) this.onOpen(comp.type_path);
    });
    g.appendChild(grp);
  }

  /**
   * Placement matrices of all components that have an icon. Components without a Placement
   * annotation (models written as text) are laid out automatically: layers follow the
   * connection graph from left to right, so a hand-written model still reads as a diagram.
   */
  private async layout(cls: ClassInfo): Promise<Map<string, Mat>> {
    const mats = new Map<string, Mat>();
    const auto: ComponentInfo[] = [];
    for (const comp of cls.components) {
      if (!comp.type_path || comp.type_kind === "type" || comp.type_kind === "record" || comp.type_kind === "function") continue;
      const t = await this.src.get(comp.type_path);
      if (!t) continue;
      const tr = (comp.placement as AnnRecord | null)?.transformation as AnnRecord | undefined;
      const m = placementMatrix(tr, systemExtent(t.icon.coordinate_system));
      if (m) mats.set(comp.name, m);
      else if (t.icon.graphics.length || t.components.length) auto.push(comp);
    }
    if (auto.length === 0) return mats;
    const names = new Set(auto.map((c) => c.name));
    const order = auto.map((c) => c.name);
    // Layers follow the direction of the connect statements (`connect(source.y, sink.u)`):
    // a component sits one layer right of everything that feeds it. Cycles are cut by
    // capping the layer index at the number of components.
    const head = (r: string) => r.split(".")[0].replace(/\[.*$/, "");
    const edges: [string, string][] = [];
    for (const c of cls.connections) {
      const a = head(c.from);
      const b = head(c.to);
      if (names.has(a) && names.has(b) && a !== b) edges.push([a, b]);
    }
    const layer = new Map<string, number>(order.map((n) => [n, 0]));
    const cap = order.length - 1;
    for (let it = 0; it < order.length; it++) {
      let changed = false;
      for (const [a, b] of edges) {
        const L = Math.min(cap, layer.get(a)! + 1);
        if (L > layer.get(b)!) {
          layer.set(b, L);
          changed = true;
        }
      }
      if (!changed) break;
    }
    // Compact: drop empty layers.
    const used = [...new Set(layer.values())].sort((p, q) => p - q);
    for (const n of order) layer.set(n, used.indexOf(layer.get(n)!));
    // Grid cells sized so that every layer and row fits inside the diagram extent.
    const rowOf = new Map<string, number>();
    const rows = new Map<number, number>();
    for (const n of order) {
      const L = layer.get(n)!;
      const r = rows.get(L) ?? 0;
      rows.set(L, r + 1);
      rowOf.set(n, r);
    }
    const nL = Math.max(...layer.values()) + 1;
    const nR = Math.max(...rows.values());
    const [[ex1, ey1], [ex2, ey2]] = systemExtent(cls.diagram.coordinate_system);
    const [x0, x1] = [Math.min(ex1, ex2), Math.max(ex1, ex2)];
    const [y0, y1] = [Math.min(ey1, ey2), Math.max(ey1, ey2)];
    const cw = Math.min(60, (x1 - x0) / nL);
    const ch = Math.min(60, (y1 - y0) / nR);
    // Icons take half a cell, leaving room for the name above each one.
    const half = Math.max(6, 0.25 * Math.min(cw, ch));
    const left = (x0 + x1) / 2 - (nL * cw) / 2;
    const top = (y0 + y1) / 2 + (nR * ch) / 2;
    for (const n of order) {
      const x = left + (layer.get(n)! + 0.5) * cw;
      // Layers with fewer components are centred vertically.
      const shift = (nR - rows.get(layer.get(n)!)!) / 2;
      const y = top - (rowOf.get(n)! + shift + 0.5) * ch - 0.1 * ch;
      const t = await this.src.get(auto.find((c) => c.name === n)!.type_path!);
      const m = placementMatrix({ extent: [[x - half, y - half], [x + half, y + half]] }, systemExtent(t!.icon.coordinate_system));
      if (m) mats.set(n, m);
    }
    return mats;
  }

  /** Global position and colour of a connector reference like `comp.port` or `port`. */
  private async connectorPoint(cls: ClassInfo, mats: Map<string, Mat>, ref: string): Promise<{ p: [number, number]; color: string } | null> {
    const parts = ref.replace(/\[[^\]]*\]/g, "").split(".");
    let m: Mat;
    let owner: ClassInfo;
    let connName: string;
    if (parts.length === 1) {
      owner = cls;
      connName = parts[0];
      m = IDENTITY;
    } else {
      const cm = mats.get(parts[0]);
      const comp = cls.components.find((c) => c.name === parts[0]);
      if (!cm || !comp?.type_path) return null;
      const t = await this.src.get(comp.type_path);
      if (!t) return null;
      owner = t;
      connName = parts[1];
      m = cm;
    }
    const conn = owner.components.find((c) => c.name === connName);
    if (!conn?.type_path) {
      const c = applyM(m, 0, 0);
      return { p: c, color: "#555" };
    }
    const ct = await this.src.get(conn.type_path);
    const pl = conn.placement as AnnRecord | null;
    const tr = (parts.length === 1 ? pl?.transformation : (pl?.iconTransformation ?? pl?.transformation)) as AnnRecord | undefined;
    const inner = systemExtent(ct?.icon.coordinate_system);
    const pm = placementMatrix(tr, inner);
    const [[a, b], [c, d]] = inner;
    const center: [number, number] = [(a + c) / 2, (b + d) / 2];
    const full = pm ? mul(m, pm) : m;
    const first = ct?.icon.graphics.find((g) => g.lineColor || g.fillColor || g.color);
    const col = (first?.lineColor ?? first?.color ?? first?.fillColor) as number[] | undefined;
    return { p: applyM(full, center[0], center[1]), color: col ? diagramInk(col) : "#555" };
  }

  private async drawAutoConnection(g: SVGGElement, cls: ClassInfo, mats: Map<string, Mat>, from: string, to: string) {
    const a = await this.connectorPoint(cls, mats, from);
    const b = await this.connectorPoint(cls, mats, to);
    if (!a || !b) return;
    const mx = (a.p[0] + b.p[0]) / 2;
    const pts = [a.p, [mx, a.p[1]], [mx, b.p[1]], b.p];
    const color = a.color;
    g.appendChild(
      el("path", {
        d: "M" + pts.map((p) => `${p[0]} ${p[1]}`).join("L"),
        fill: "none",
        stroke: color,
        "stroke-width": 1,
        "vector-effect": "non-scaling-stroke",
        class: "connection auto",
        "data-from": from,
        "data-to": to,
      }),
    );
    this.connectionHit(g, pts as [number, number][], from, to);
  }

  /** A group drawn over the class, in Modelica coordinates (for diff highlights). */
  addOverlay(): SVGGElement {
    const g = el("g", { class: "overlay" });
    this.root.appendChild(g);
    return g;
  }

  /** The drawn component `name`, if any. */
  componentElement(name: string): SVGGElement | null {
    return this.root.querySelector<SVGGElement>(`.component[data-name="${CSS.escape(name)}"]`);
  }

  /** The drawn connection between `a` and `b`, in either direction. */
  connectionElement(a: string, b: string): SVGElement | null {
    const q = (x: string, y: string) => `.connection[data-from="${CSS.escape(x)}"][data-to="${CSS.escape(y)}"]`;
    return this.root.querySelector<SVGElement>(`${q(a, b)}, ${q(b, a)}`);
  }

  clearSelection() {
    this.svg.querySelectorAll(".component.selected").forEach((n) => n.classList.remove("selected"));
  }
}
