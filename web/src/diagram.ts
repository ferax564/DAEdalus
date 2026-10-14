// Icon and diagram views of a class: SVG canvas with pan, zoom, selection and tooltips.

import type { AnnRecord, ClassInfo, ComponentInfo } from "./types";
import {
  apply as applyM,
  type ClassSource,
  type Mat,
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
  private drag: { x: number; y: number; vx: number; vy: number } | null = null;
  private renderToken = 0;
  onSelect: (c: ComponentInfo | null) => void = () => {};
  onOpen: (typePath: string) => void = () => {};

  constructor(host: HTMLElement, private src: ClassSource, private classes: Map<string, ClassInfo>) {
    this.svg = document.createElementNS(SVG_NS, "svg");
    this.svg.classList.add("diagram");
    this.root = el("g", { transform: "scale(1,-1)" });
    this.svg.appendChild(this.root);
    host.appendChild(this.svg);
    this.svg.addEventListener("wheel", (e) => this.zoom(e), { passive: false });
    this.svg.addEventListener("pointerdown", (e) => {
      if (e.button !== 0 && e.button !== 1) return;
      this.drag = { x: e.clientX, y: e.clientY, vx: this.view.x, vy: this.view.y };
      this.svg.setPointerCapture(e.pointerId);
    });
    this.svg.addEventListener("pointermove", (e) => {
      if (!this.drag) return;
      const k = this.view.w / this.svg.clientWidth;
      this.view.x = this.drag.vx - (e.clientX - this.drag.x) * k;
      this.view.y = this.drag.vy - (e.clientY - this.drag.y) * k;
      this.applyView();
    });
    this.svg.addEventListener("pointerup", () => (this.drag = null));
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

  async show(cls: ClassInfo, kind: ViewKind) {
    const token = ++this.renderToken;
    const g = el("g", { transform: "scale(1,-1)" });
    const ctx = newCtx(this.svg);
    const layer = kind === "icon" ? cls.icon : cls.diagram;
    const ext = systemExtent(layer.coordinate_system);
    // drawing frame
    const [[x1, y1], [x2, y2]] = ext;
    g.appendChild(el("rect", { x: Math.min(x1, x2), y: Math.min(y1, y2), width: Math.abs(x2 - x1), height: Math.abs(y2 - y1), class: "frame" }));
    if (kind === "icon") {
      await drawIcon(g, cls, null, IDENTITY, this.src, ctx, this.classes);
    } else {
      ctx.subst = (s) => s.replace("%name", cls.name);
      drawGraphics(g, cls.diagram.graphics, IDENTITY, ctx);
      const mats = await this.layout(cls);
      if (token !== this.renderToken) return;
      for (const comp of cls.components) {
        const m = mats.get(comp.name);
        if (!m) continue;
        await this.drawComponent(g, comp, m, ctx);
        if (token !== this.renderToken) return;
      }
      for (const c of cls.connections) {
        if (c.line) this.drawConnection(g, c.line);
        else await this.drawAutoConnection(g, cls, mats, c.from, c.to);
      }
    }
    if (token !== this.renderToken) return;
    this.svg.replaceChild(g, this.root);
    this.root = g;
    this.fit(this.contentExtent(ext));
  }

  private contentExtent(ext: [[number, number], [number, number]]): [[number, number], [number, number]] {
    try {
      const b = this.root.getBBox();
      if (b.width > 0 && b.height > 0) {
        // root is mirrored (scale(1,-1)), so flip the box back to Modelica coordinates
        return [
          [Math.min(b.x, ext[0][0]), Math.min(-(b.y + b.height), ext[0][1])],
          [Math.max(b.x + b.width, ext[1][0]), Math.max(-b.y, ext[1][1])],
        ];
      }
    } catch {
      /* not laid out yet */
    }
    return ext;
  }

  private drawConnection(g: SVGGElement, line: AnnRecord | null) {
    if (!line) return;
    const pts = points(line.points);
    if (pts.length < 2) return;
    const ctx = newCtx(this.svg);
    const grp = el("g", { class: "connection" });
    drawGraphics(grp, [{ ...line, "@": "Line" }], IDENTITY, ctx);
    g.appendChild(grp);
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
    const adj = new Map<string, Set<string>>();
    for (const n of names) adj.set(n, new Set());
    const head = (r: string) => r.split(".")[0].replace(/\[.*$/, "");
    for (const c of cls.connections) {
      const a = head(c.from);
      const b = head(c.to);
      if (names.has(a) && names.has(b) && a !== b) {
        adj.get(a)!.add(b);
        adj.get(b)!.add(a);
      }
    }
    // breadth-first layers from components that are sources (outputs only) or the first one
    const layer = new Map<string, number>();
    const order = auto.map((c) => c.name);
    for (const start of order) {
      if (layer.has(start)) continue;
      const base = layer.size ? Math.max(...layer.values()) + 1 : 0;
      layer.set(start, base);
      const q = [start];
      while (q.length) {
        const n = q.shift()!;
        for (const k of adj.get(n)!) {
          if (!layer.has(k)) {
            layer.set(k, layer.get(n)! + 1);
            q.push(k);
          }
        }
      }
    }
    const rows = new Map<number, number>();
    const cell = 60;
    for (const n of order) {
      const L = layer.get(n)!;
      const r = rows.get(L) ?? 0;
      rows.set(L, r + 1);
      const x = -100 + L * cell;
      const y = 60 - r * cell;
      const t = await this.src.get(auto.find((c) => c.name === n)!.type_path!);
      const m = placementMatrix({ extent: [[x - 20, y - 20], [x + 20, y + 20]] }, systemExtent(t!.icon.coordinate_system));
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
    return { p: applyM(full, center[0], center[1]), color: col ? `rgb(${col[0]},${col[1]},${col[2]})` : "#555" };
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
      }),
    );
  }

  clearSelection() {
    this.svg.querySelectorAll(".component.selected").forEach((n) => n.classList.remove("selected"));
  }
}
