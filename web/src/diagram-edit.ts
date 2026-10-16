// Graphical model editing on the diagram view: drop classes from the package browser, select,
// move, rotate, flip and delete components, draw connections, edit parameters, undo and redo.
//
// Every gesture becomes a list of edit ops that the worker applies to the class's source text
// (crates/daedalus-core/src/frontend/edit.rs), so the Text view, Check and Simulate always see
// the same model and the rest of the file keeps its formatting.

import "./diagram-edit.css";
import { api, type ParamInfo } from "./rpc";
import type { AnnRecord, ClassInfo, ComponentInfo } from "./types";
import type { DiagramView } from "./diagram";
import { apply as applyM, type Mat, el, mul, placementMatrix, systemExtent } from "./graphics";

type Pt = [number, number];

/** What the editor needs from the workbench. */
export interface EditHost {
  current(): ClassInfo | null;
  /** The view shown in the model pane ("diagram", "icon", "text", ...). */
  view(): string;
  classInfo(path: string): Promise<ClassInfo | null>;
  /** A document changed in the worker: refresh caches and views; open `open` if given. */
  documentChanged(uri: string, text: string, open?: string): Promise<void>;
  log(msg: string, cls?: string): void;
}

interface Port {
  ref: string;
  comp: string | null;
  center: Pt;
  /** Outward direction of a line leaving the port. */
  dir: Pt;
  r: number;
  typePath: string;
  typeName: string;
  causality: string;
  color: [number, number, number];
}

interface Version {
  uri: string;
  before: string;
  after: string;
  path: string;
}

/** Documents of the bundled libraries are read-only; user documents can be edited. */
export const editableUri = (uri: string | undefined) => !!uri && !uri.startsWith("library/") && !uri.startsWith("msl/");

export const MY_MODELS_URI = "user/MyModels.mo";
const MY_MODELS = "MyModels";
const STORE_KEY = "daedalus.mymodels";
export const DRAG_TYPE = "application/x-daedalus-class";
const GRID = 2;
const snap = (x: number) => Math.round(x / GRID) * GRID;

export class DiagramEditor {
  editing = false;
  private selected = new Set<string>();
  private selConn: { from: string; to: string } | null = null;
  private ports: Port[] = [];
  private boxes = new Map<string, { corners: Pt[]; center: Pt; m: Mat; inner: [Pt, Pt] }>();
  private overlay: SVGGElement | null = null;
  private undoStack: Version[] = [];
  private redoStack: Version[] = [];
  private busy = false;
  private toolbar: HTMLDivElement;
  private buttons: Record<string, HTMLButtonElement> = {};

  constructor(
    private diagram: DiagramView,
    private host: EditHost,
    container: HTMLElement,
  ) {
    this.toolbar = document.createElement("div");
    this.toolbar.className = "edit-toolbar";
    const btn = (id: string, label: string, title: string, run: () => void) => {
      const b = document.createElement("button");
      b.textContent = label;
      b.title = title;
      b.dataset.edit = id;
      b.onclick = (e) => {
        e.stopPropagation();
        run();
      };
      this.toolbar.appendChild(b);
      this.buttons[id] = b;
      return b;
    };
    btn("edit", "✎ Edit", "Edit the diagram: drag classes from the package browser, drag between connectors to connect", () => this.toggle());
    btn("duplicate", "Duplicate to my models", "Copy this read-only class into MyModels.mo and edit the copy", () => void this.duplicate());
    btn("new", "New model", "Add an empty model to MyModels.mo", () => void this.newModel());
    this.buttons.new.classList.add("hidden");
    btn("rotate", "⟳", "Rotate the selection by 90° (R)", () => void this.rotate(-90));
    btn("flipH", "⇋", "Flip horizontally (H)", () => void this.flip(true));
    btn("flipV", "⇵", "Flip vertically (V)", () => void this.flip(false));
    btn("delete", "🗑", "Delete the selection (Del)", () => void this.deleteSelection());
    btn("params", "Parameters…", "Edit the parameters of the selected component (double-click)", () => {
      const n = [...this.selected][0];
      const c = n ? this.cls()?.components.find((k) => k.name === n) : undefined;
      if (c) void this.paramDialog(c);
    });
    btn("undo", "↶", "Undo (Ctrl+Z)", () => void this.undo());
    btn("redo", "↷", "Redo (Ctrl+Y)", () => void this.redo());
    btn("save", "Save .mo", "Download the edited file", () => this.save());
    container.appendChild(this.toolbar);

    const svg = diagram.svg;
    // Capture-phase listeners run before the view's own pan and open handlers.
    svg.addEventListener("pointerdown", (e) => this.pointerDown(e), { capture: true });
    svg.addEventListener(
      "dblclick",
      (e) => {
        if (!this.editing) return;
        const hit = document.elementsFromPoint(e.clientX, e.clientY).find((x) => x.closest(".component"));
        const g = hit?.closest<SVGGElement>(".component");
        e.stopPropagation();
        const c = g && this.cls()?.components.find((k) => k.name === g.dataset.name);
        if (c && this.ownComponent(c)) void this.paramDialog(c);
      },
      { capture: true },
    );
    svg.addEventListener("dragover", (e) => {
      if (!e.dataTransfer?.types.includes(DRAG_TYPE)) return;
      e.preventDefault();
      e.stopPropagation();
      e.dataTransfer.dropEffect = "copy";
    });
    svg.addEventListener("drop", (e) => {
      const path = e.dataTransfer?.getData(DRAG_TYPE);
      if (!path) return;
      e.preventDefault();
      e.stopPropagation();
      void this.dropClass(path, this.toModel(e.clientX, e.clientY));
    });
    window.addEventListener("keydown", (e) => this.key(e));
    diagram.onRendered = (kind) => this.rendered(kind);
  }

  // ------------------------------------------------------------ state and toolbar

  private cls(): ClassInfo | null {
    return this.diagram.rendered?.cls ?? null;
  }
  private editable(): boolean {
    return editableUri(this.host.current()?.source?.uri);
  }
  private ownComponent(c: ComponentInfo) {
    return c.declared_in === this.cls()?.path;
  }

  /** Update the toolbar for the class and view shown. */
  updateToolbar() {
    const cur = this.host.current();
    const diagramShown = this.host.view() === "diagram" && !!cur && !["package", "connector", "record", "type", "function"].includes(cur.kind);
    this.toolbar.classList.toggle("hidden", this.host.view() !== "diagram");
    const ed = this.editable();
    if (!ed && this.editing) this.setEditing(false);
    const show = (id: string, on: boolean) => this.buttons[id].classList.toggle("hidden", !on);
    show("edit", diagramShown && ed);
    show("duplicate", diagramShown && !ed);
    show("new", false);
    for (const id of ["rotate", "flipH", "flipV", "delete", "params"]) show(id, this.editing);
    show("undo", this.undoStack.length > 0 || this.redoStack.length > 0);
    show("redo", this.undoStack.length > 0 || this.redoStack.length > 0);
    show("save", !!cur && ed);
    this.buttons.undo.disabled = this.undoStack.length === 0;
    this.buttons.redo.disabled = this.redoStack.length === 0;
    this.buttons.edit.classList.toggle("active", this.editing);
    const sel = this.selected.size > 0;
    for (const id of ["rotate", "flipH", "flipV"]) this.buttons[id].disabled = !sel;
    this.buttons.delete.disabled = !sel && !this.selConn;
    this.buttons.params.disabled = this.selected.size !== 1;
  }

  toggle() {
    this.setEditing(!this.editing);
  }

  setEditing(on: boolean) {
    if (on && !this.editable()) {
      this.host.log("This class belongs to a read-only library: duplicate it to MyModels to edit it.", "warn");
      return;
    }
    this.editing = on;
    this.diagram.svg.classList.toggle("editing", on);
    this.clearSelection();
    this.drawOverlay();
    this.updateToolbar();
  }

  private clearSelection() {
    this.selected.clear();
    this.selConn = null;
  }

  // ------------------------------------------------------------ geometry

  /** Client coordinates to diagram (Modelica) coordinates. */
  private toModel(x: number, y: number): Pt {
    const root = this.diagram.rendered?.root;
    const m = root?.getScreenCTM();
    if (!m) return [0, 0];
    const p = new DOMPoint(x, y).matrixTransform(m.inverse());
    return [p.x, p.y];
  }

  /** Component boxes and connector ports of the diagram just drawn. */
  private async measure() {
    this.ports = [];
    this.boxes.clear();
    const r = this.diagram.rendered;
    if (!r) return;
    const { cls, mats } = r;
    for (const comp of cls.components) {
      const m = mats.get(comp.name);
      if (!m || !comp.type_path) continue;
      const t = await this.host.classInfo(comp.type_path);
      if (!t) continue;
      const inner = systemExtent(t.icon.coordinate_system);
      const [[a, b], [c, d]] = inner;
      const corners = ([[a, b], [c, b], [c, d], [a, d]] as Pt[]).map(([x, y]) => applyM(m, x, y));
      const center = applyM(m, (a + c) / 2, (b + d) / 2);
      this.boxes.set(comp.name, { corners, center, m, inner });
      for (const k of t.components) {
        if (k.type_kind !== "connector" || k.protected || !k.type_path) continue;
        const pl = k.placement as AnnRecord | null;
        const tr = (pl?.iconTransformation ?? pl?.transformation) as AnnRecord | undefined;
        const p = await this.port(`${comp.name}.${k.name}`, comp.name, k, m, tr, center);
        if (p) this.ports.push(p);
      }
    }
    // connectors of the class itself, on the diagram layer
    const [[x1, y1], [x2, y2]] = systemExtent(cls.diagram.coordinate_system);
    const mid: Pt = [(x1 + x2) / 2, (y1 + y2) / 2];
    for (const k of cls.components) {
      if (k.type_kind !== "connector" || k.protected || !k.type_path) continue;
      const tr = (k.placement as AnnRecord | null)?.transformation as AnnRecord | undefined;
      const p = await this.port(k.name, null, k, [1, 0, 0, 1, 0, 0], tr, mid, true);
      if (p) this.ports.push(p);
    }
  }

  private async port(ref: string, comp: string | null, k: ComponentInfo, m: Mat, tr: AnnRecord | undefined, center: Pt, inward = false): Promise<Port | null> {
    const ct = await this.host.classInfo(k.type_path!);
    const inner = systemExtent(ct?.icon.coordinate_system);
    const pm = placementMatrix(tr, inner);
    if (!pm) return null;
    const full = mul(m, pm);
    const [[a, b], [c, d]] = inner;
    const pc = applyM(full, (a + c) / 2, (b + d) / 2);
    const corner = applyM(full, a, b);
    const r = Math.max(1.5, Math.hypot(corner[0] - pc[0], corner[1] - pc[1]) * 0.6);
    let dx = pc[0] - center[0];
    let dy = pc[1] - center[1];
    if (inward) {
      dx = -dx;
      dy = -dy;
    }
    const dir: Pt = Math.abs(dx) >= Math.abs(dy) ? [Math.sign(dx) || 1, 0] : [0, Math.sign(dy) || 1];
    const g = ct?.icon.graphics.find((x) => x.lineColor || x.fillColor || x.color);
    const col = (g?.lineColor ?? g?.color ?? g?.fillColor) as number[] | undefined;
    return {
      ref,
      comp,
      center: pc,
      dir,
      r,
      typePath: k.type_path!,
      typeName: k.type_name,
      causality: k.causality || (/Input$/.test(k.type_path!) ? "input" : /Output$/.test(k.type_path!) ? "output" : ""),
      color: col && col.length >= 3 ? [col[0], col[1], col[2]] : [0, 0, 255],
    };
  }

  /** Connector classes that can be connected: the same class, the same members, or one
   * signal family (`RealInput`/`RealOutput`, `PositivePin`/`NegativePin`, `Flange_a`/`Flange_b`). */
  private compatible(a: Port, b: Port): boolean {
    if (a.ref === b.ref) return false;
    const fam = (p: Port) => {
      const n = p.typePath.split(".").pop()!;
      return n.replace(/(Input|Output)$/, "").replace(/^(Positive|Negative)/, "").replace(/_[ab]$/, "").replace(/[AB]$/, "");
    };
    const out = (p: Port) => /Output$/.test(p.typePath);
    if (a.comp && b.comp && out(a) && out(b)) return false;
    if (a.typePath === b.typePath) return true;
    const ma = this.memberNames(a.typePath);
    const mb = this.memberNames(b.typePath);
    if (ma && ma === mb) return true;
    return fam(a) === fam(b);
  }
  private memberCache = new Map<string, string>();
  private memberNames(path: string): string {
    return this.memberCache.get(path) ?? "";
  }
  private async cacheMembers() {
    for (const p of this.ports) {
      if (this.memberCache.has(p.typePath)) continue;
      const c = await this.host.classInfo(p.typePath);
      this.memberCache.set(p.typePath, c ? c.components.filter((k) => !k.protected).map((k) => k.name).sort().join(",") : "");
    }
  }

  // ------------------------------------------------------------ overlay

  private async rendered(kind: string) {
    this.overlay = null;
    if (kind !== "diagram") {
      this.updateToolbar();
      return;
    }
    // drop selections that no longer exist (after a delete or undo)
    const names = new Set(this.cls()?.components.map((c) => c.name));
    for (const n of [...this.selected]) if (!names.has(n)) this.selected.delete(n);
    // the inspector shows the edited component as it is now
    const one = this.selected.size === 1 ? this.cls()?.components.find((c) => this.selected.has(c.name)) : undefined;
    if (one && this.editing) this.diagram.onSelect(one);
    await this.measure();
    await this.cacheMembers();
    this.drawOverlay();
    this.updateToolbar();
  }

  private drawOverlay() {
    const root = this.diagram.rendered?.root;
    this.overlay?.remove();
    this.overlay = null;
    if (!root || !this.editing) return;
    const g = el("g", { class: "edit-overlay" });
    // wide invisible strokes to pick connections
    const cls = this.cls()!;
    for (const c of cls.connections) {
      if (c.declared_in !== cls.path) continue;
      const pts = this.connectionPoints(c.from, c.to, c.line);
      if (pts.length < 2) continue;
      const sel = this.selConn && sameConn(this.selConn, c);
      const d = "M" + pts.map((p) => p.join(" ")).join("L");
      const wire = el("path", { d, class: "edit-wire" + (sel ? " selected" : ""), "data-from": c.from, "data-to": c.to });
      g.appendChild(wire);
      if (sel && c.line) {
        pts.slice(1, -1).forEach((p, i) => {
          const h = this.handleSize();
          g.appendChild(el("rect", { x: p[0] - h, y: p[1] - h, width: 2 * h, height: 2 * h, class: "edit-vertex", "data-index": i + 1 }));
        });
      }
    }
    for (const n of this.selected) {
      const b = this.boxes.get(n);
      if (!b) continue;
      g.appendChild(el("path", { d: "M" + b.corners.map((p) => p.join(" ")).join("L") + "Z", class: "edit-selection" }));
      if (this.selected.size === 1) {
        const h = this.handleSize();
        const far = b.corners.reduce((best, p) => (p[0] - p[1] > best[0] - best[1] ? p : best));
        g.appendChild(el("rect", { x: far[0] - h, y: far[1] - h, width: 2 * h, height: 2 * h, class: "edit-resize", "data-name": n }));
      }
    }
    for (const p of this.ports) {
      g.appendChild(el("circle", { cx: p.center[0], cy: p.center[1], r: p.r, class: "edit-port", "data-ref": p.ref }));
    }
    root.appendChild(g);
    this.overlay = g;
  }

  private handleSize() {
    const root = this.diagram.rendered?.root;
    const m = root?.getScreenCTM();
    return m ? 4 / Math.abs(m.a || 1) : 2;
  }

  /** Points of a connection: its Line annotation, or a route between its connectors. */
  private connectionPoints(from: string, to: string, line: AnnRecord | null): Pt[] {
    const pts = (line?.points as number[][] | undefined)?.filter((p) => Array.isArray(p)).map((p) => [p[0], p[1]] as Pt);
    if (pts && pts.length >= 2) return pts;
    const a = this.findPort(from);
    const b = this.findPort(to);
    return a && b ? route(a, b) : [];
  }

  private findPort(ref: string): Port | undefined {
    const r = ref.replace(/\s+/g, "");
    return this.ports.find((p) => p.ref === r) ?? this.ports.find((p) => p.ref === r.replace(/\[[^\]]*\]/g, ""));
  }

  // ------------------------------------------------------------ pointer gestures

  private pointerDown(e: PointerEvent) {
    if (!this.editing || e.button !== 0 || this.busy) return;
    const t = e.target as Element;
    const start = this.toModel(e.clientX, e.clientY);
    const stop = () => {
      e.stopImmediatePropagation();
      e.preventDefault();
    };
    const port = t.closest<SVGElement>(".edit-port");
    if (port) {
      stop();
      const p = this.ports.find((q) => q.ref === port.dataset.ref);
      if (p) this.dragConnection(e, p);
      return;
    }
    const vertex = t.closest<SVGElement>(".edit-vertex");
    if (vertex && this.selConn) {
      stop();
      this.dragVertex(e, Number(vertex.dataset.index));
      return;
    }
    const resize = t.closest<SVGElement>(".edit-resize");
    if (resize) {
      stop();
      this.dragResize(e, resize.dataset.name!);
      return;
    }
    const wire = t.closest<SVGElement>(".edit-wire");
    if (wire) {
      stop();
      this.selected.clear();
      this.selConn = { from: wire.dataset.from!, to: wire.dataset.to! };
      this.drawOverlay();
      this.updateToolbar();
      return;
    }
    const comp = t.closest<SVGGElement>(".component");
    const info = comp && this.cls()?.components.find((c) => c.name === comp.dataset.name);
    if (comp && info) {
      stop();
      if (!this.ownComponent(info)) {
        this.host.log(`${info.name} is inherited from ${info.declared_in} and cannot be moved here.`, "warn");
        return;
      }
      const name = info.name;
      this.selConn = null;
      if (e.shiftKey) {
        if (this.selected.has(name)) this.selected.delete(name);
        else this.selected.add(name);
      } else if (!this.selected.has(name)) {
        this.selected.clear();
        this.selected.add(name);
      }
      this.drawOverlay();
      this.updateToolbar();
      this.diagram.onSelect(info);
      if (this.selected.has(name)) this.dragMove(e, start);
      return;
    }
    // background: rubber band selection
    stop();
    this.rubberBand(e, start, e.shiftKey);
  }

  /** Track a drag until the pointer is released. */
  private track(e: PointerEvent, move: (p: Pt, ev: PointerEvent) => void, up: (p: Pt, ev: PointerEvent) => void) {
    const svg = this.diagram.svg;
    // Capture once the pointer moves, so a plain click or double-click keeps its target.
    const onMove = (ev: PointerEvent) => {
      if (!svg.hasPointerCapture(ev.pointerId)) svg.setPointerCapture(ev.pointerId);
      move(this.toModel(ev.clientX, ev.clientY), ev);
    };
    const onUp = (ev: PointerEvent) => {
      svg.removeEventListener("pointermove", onMove);
      svg.removeEventListener("pointerup", onUp);
      svg.removeEventListener("pointercancel", onUp);
      if (svg.hasPointerCapture(ev.pointerId)) svg.releasePointerCapture(ev.pointerId);
      up(this.toModel(ev.clientX, ev.clientY), ev);
    };
    svg.addEventListener("pointermove", onMove);
    svg.addEventListener("pointerup", onUp);
    svg.addEventListener("pointercancel", onUp);
  }

  private dragMove(e: PointerEvent, start: Pt) {
    const root = this.diagram.rendered!.root;
    const groups = [...this.selected].map((n) => root.querySelector<SVGGElement>(`.component[data-name="${CSS.escape(n)}"]`)).filter((g): g is SVGGElement => !!g);
    let d: Pt = [0, 0];
    this.track(
      e,
      (p) => {
        d = [snap(p[0] - start[0]), snap(p[1] - start[1])];
        const tr = `translate(${d[0]},${d[1]})`;
        for (const g of groups) g.setAttribute("transform", tr);
        this.overlay?.querySelectorAll(".edit-selection, .edit-resize").forEach((x) => x.setAttribute("transform", tr));
      },
      () => {
        if (d[0] === 0 && d[1] === 0) return;
        void this.moveSelection(d);
      },
    );
  }

  /** Move the selected components by `d`, re-routing the lines attached to them. */
  private async moveSelection(d: Pt) {
    const cls = this.cls();
    if (!cls) return;
    const ops: unknown[] = [];
    for (const n of this.selected) {
      const comp = cls.components.find((c) => c.name === n);
      if (!comp) continue;
      if (comp.placement) ops.push({ op: "move_component", name: n, dx: d[0], dy: d[1] });
      else {
        // laid out automatically so far: write the placement where it is drawn
        const b = this.boxes.get(n);
        if (!b) continue;
        const xs = b.corners.map((p) => p[0]);
        const ys = b.corners.map((p) => p[1]);
        ops.push({ op: "set_placement", name: n, extent: [[snap(Math.min(...xs)) + d[0], snap(Math.min(...ys)) + d[1]], [snap(Math.max(...xs)) + d[0], snap(Math.max(...ys)) + d[1]]] });
      }
    }
    const head = (r: string) => r.split(".")[0].replace(/\[.*$/, "");
    for (const c of cls.connections) {
      if (c.declared_in !== cls.path || !c.line) continue;
      const ma = this.selected.has(head(c.from)) && c.from.includes(".");
      const mb = this.selected.has(head(c.to)) && c.to.includes(".");
      if (!ma && !mb) continue;
      const pts = this.connectionPoints(c.from, c.to, c.line);
      let out: Pt[];
      if (ma && mb) out = pts.map((p) => [p[0] + d[0], p[1] + d[1]]);
      else {
        const pa = this.findPort(c.from);
        const pb = this.findPort(c.to);
        if (!pa || !pb) continue;
        const shift = (p: Port, moved: boolean): Port => (moved ? { ...p, center: [p.center[0] + d[0], p.center[1] + d[1]] } : p);
        out = route(shift(pa, ma), shift(pb, mb));
      }
      ops.push({ op: "set_connection_points", from: c.from, to: c.to, points: out });
    }
    await this.apply(ops);
  }

  private dragResize(e: PointerEvent, name: string) {
    const b = this.boxes.get(name);
    const comp = this.cls()?.components.find((c) => c.name === name);
    if (!b || !comp) return;
    const half0 = Math.max(...b.corners.map((p) => Math.max(Math.abs(p[0] - b.center[0]), Math.abs(p[1] - b.center[1]))));
    let f = 1;
    const sel = this.overlay?.querySelector(".edit-selection");
    this.track(
      e,
      (p) => {
        const half = Math.max(Math.abs(p[0] - b.center[0]), Math.abs(p[1] - b.center[1]));
        f = Math.max(0.2, snap(half) / half0);
        sel?.setAttribute("transform", `translate(${b.center[0]},${b.center[1]}) scale(${f}) translate(${-b.center[0]},${-b.center[1]})`);
      },
      () => {
        if (Math.abs(f - 1) < 1e-6) return;
        const tr = ((comp.placement as AnnRecord | null)?.transformation ?? {}) as AnnRecord;
        const ext = (tr.extent as number[][] | undefined) ?? [];
        const rot = typeof tr.rotation === "number" ? tr.rotation : 0;
        if (ext.length !== 2) return;
        // scale the extent about its centre and keep the icon centre where it is
        const cx = (ext[0][0] + ext[1][0]) / 2;
        const cy = (ext[0][1] + ext[1][1]) / 2;
        const extent = ext.map((q) => [snap((q[0] - cx) * f), snap((q[1] - cy) * f)]);
        void this.apply([{ op: "set_placement", name, origin: [snap(b.center[0]), snap(b.center[1])], extent, rotation: rot }]);
      },
    );
  }

  private dragConnection(e: PointerEvent, from: Port) {
    const root = this.diagram.rendered!.root;
    const ok = new Set(this.ports.filter((p) => this.compatible(from, p)).map((p) => p.ref));
    this.overlay?.querySelectorAll<SVGElement>(".edit-port").forEach((c) => {
      c.classList.toggle("compatible", ok.has(c.dataset.ref!));
      c.classList.toggle("incompatible", !ok.has(c.dataset.ref!) && c.dataset.ref !== from.ref);
    });
    const line = el("path", { class: "edit-rubber-line", d: "" });
    root.appendChild(line);
    const target = (p: Pt) => {
      let best: Port | null = null;
      let bd = Infinity;
      for (const q of this.ports) {
        if (!ok.has(q.ref)) continue;
        const dd = Math.hypot(q.center[0] - p[0], q.center[1] - p[1]);
        if (dd < Math.max(q.r * 1.6, 4) && dd < bd) {
          best = q;
          bd = dd;
        }
      }
      return best;
    };
    this.track(
      e,
      (p) => {
        const t = target(p);
        const pts = t ? route(from, t) : [from.center, p];
        line.setAttribute("d", "M" + pts.map((q) => q.join(" ")).join("L"));
      },
      (p) => {
        line.remove();
        const t = target(p);
        this.drawOverlay();
        if (!t) return;
        // an output (or the class's own input) comes first, as Dymola writes it
        const [a, b] = from.causality === "input" && from.comp ? [t, from] : [from, t];
        void this.apply([{ op: "add_connection", from: a.ref, to: b.ref, points: route(a, b), color: from.color }]);
      },
    );
  }

  private dragVertex(e: PointerEvent, index: number) {
    const cls = this.cls();
    const c = cls?.connections.find((k) => this.selConn && sameConn(this.selConn, k));
    if (!c) return;
    const pts = this.connectionPoints(c.from, c.to, c.line).map((p) => [...p] as Pt);
    const wire = this.overlay?.querySelector<SVGPathElement>(".edit-wire.selected");
    let moved = false;
    this.track(
      e,
      (p) => {
        pts[index] = [snap(p[0]), snap(p[1])];
        moved = true;
        wire?.setAttribute("d", "M" + pts.map((q) => q.join(" ")).join("L"));
      },
      () => {
        if (moved) void this.apply([{ op: "set_connection_points", from: c.from, to: c.to, points: pts }]);
      },
    );
  }

  private rubberBand(e: PointerEvent, start: Pt, add: boolean) {
    const root = this.diagram.rendered!.root;
    const rect = el("rect", { class: "edit-band", x: start[0], y: start[1], width: 0, height: 0 });
    root.appendChild(rect);
    let end = start;
    this.track(
      e,
      (p) => {
        end = p;
        rect.setAttribute("x", String(Math.min(start[0], p[0])));
        rect.setAttribute("y", String(Math.min(start[1], p[1])));
        rect.setAttribute("width", String(Math.abs(p[0] - start[0])));
        rect.setAttribute("height", String(Math.abs(p[1] - start[1])));
      },
      () => {
        rect.remove();
        if (!add) this.clearSelection();
        const [x0, x1] = [Math.min(start[0], end[0]), Math.max(start[0], end[0])];
        const [y0, y1] = [Math.min(start[1], end[1]), Math.max(start[1], end[1])];
        if (x1 - x0 > 0.5 || y1 - y0 > 0.5) {
          for (const [n, b] of this.boxes) {
            const comp = this.cls()?.components.find((c) => c.name === n);
            if (!comp || !this.ownComponent(comp)) continue;
            if (b.corners.every((p) => p[0] >= x0 && p[0] <= x1 && p[1] >= y0 && p[1] <= y1)) this.selected.add(n);
          }
        }
        this.drawOverlay();
        this.updateToolbar();
      },
    );
  }

  // ------------------------------------------------------------ commands

  private key(e: KeyboardEvent) {
    const a = document.activeElement;
    if (a && (a.closest(".cm-editor") || ["INPUT", "TEXTAREA", "SELECT"].includes(a.tagName))) return;
    if (document.querySelector(".param-dialog")) return;
    const ctrl = e.ctrlKey || e.metaKey;
    if (ctrl && (e.key === "z" || e.key === "Z") && !e.shiftKey) {
      if (this.undoStack.length) {
        e.preventDefault();
        void this.undo();
      }
      return;
    }
    if (ctrl && (e.key === "y" || ((e.key === "z" || e.key === "Z") && e.shiftKey))) {
      if (this.redoStack.length) {
        e.preventDefault();
        void this.redo();
      }
      return;
    }
    if (!this.editing || this.host.view() !== "diagram" || ctrl || e.altKey) return;
    const arrows: Record<string, Pt> = { ArrowLeft: [-GRID, 0], ArrowRight: [GRID, 0], ArrowUp: [0, GRID], ArrowDown: [0, -GRID] };
    if (e.key === "Delete" || e.key === "Backspace") {
      e.preventDefault();
      void this.deleteSelection();
    } else if (e.key === "Escape") {
      this.clearSelection();
      this.drawOverlay();
      this.updateToolbar();
    } else if (e.key === "r" || e.key === "R") void this.rotate(e.shiftKey ? 90 : -90);
    else if (e.key === "h" || e.key === "H") void this.flip(true);
    else if (e.key === "v" || e.key === "V") void this.flip(false);
    else if (arrows[e.key] && this.selected.size) {
      e.preventDefault();
      const d = arrows[e.key];
      void this.moveSelection(e.shiftKey ? [d[0] * 5, d[1] * 5] : d);
    }
  }

  async deleteSelection() {
    const ops: unknown[] = [...this.selected].map((name) => ({ op: "delete_component", name }));
    const cls = this.cls();
    if (this.selConn && cls) {
      // a connection to a deleted component goes with it
      const head = (r: string) => r.split(".")[0].replace(/\[.*$/, "");
      const gone = (r: string) => r.includes(".") && this.selected.has(head(r));
      if (!gone(this.selConn.from) && !gone(this.selConn.to)) ops.push({ op: "delete_connection", from: this.selConn.from, to: this.selConn.to });
    }
    if (!ops.length) return;
    this.clearSelection();
    await this.apply(ops);
  }

  async rotate(angle: number) {
    if (!this.selected.size) return;
    await this.apply([...this.selected].map((name) => ({ op: "rotate_component", name, angle })));
  }

  async flip(horizontal: boolean) {
    if (!this.selected.size) return;
    await this.apply([...this.selected].map((name) => ({ op: "flip_component", name, horizontal })));
  }

  private async dropClass(path: string, at: Pt) {
    const cur = this.host.current();
    if (!cur) return;
    if (!this.editable()) {
      this.host.log(`${cur.path} is read-only: duplicate it to MyModels (or make a new model) to add components.`, "warn");
      return;
    }
    if (!this.editing) this.setEditing(true);
    const t = await this.host.classInfo(path);
    if (!t || !["model", "block", "class", "connector", "record"].includes(t.kind)) {
      this.host.log(`${path} is a ${t?.kind ?? "unknown class"} and cannot be placed in a diagram.`, "warn");
      return;
    }
    if (t.partial) {
      this.host.log(`${path} is partial: extend it in a model of your own before using it.`, "warn");
      return;
    }
    const [x, y] = [snap(at[0]), snap(at[1])];
    const r = await this.apply([{ op: "add_component", type_name: path, extent: [[x - 10, y - 10], [x + 10, y + 10]] }]);
    if (r?.added?.length) {
      this.selected = new Set(r.added);
      this.drawOverlay();
      this.updateToolbar();
    }
  }

  /** Apply edit ops to the open class, record the version and refresh the views. */
  async apply(ops: unknown[]) {
    const cur = this.host.current();
    const uri = cur?.source?.uri;
    if (!cur || !uri || !ops.length || this.busy) return null;
    this.busy = true;
    try {
      const before = (await api.documentText(uri)) ?? "";
      const r = await api.editClass(cur.path, ops);
      if (!r.ok) {
        this.host.log(`Edit failed: ${r.error}`, "err");
        return null;
      }
      this.undoStack.push({ uri, before, after: r.text, path: cur.path });
      this.redoStack = [];
      await this.changed(r.uri, r.text);
      return r;
    } finally {
      this.busy = false;
    }
  }

  private async changed(uri: string, text: string, open?: string) {
    if (uri === MY_MODELS_URI) {
      try {
        localStorage.setItem(STORE_KEY, text);
      } catch {
        /* storage unavailable: the models last until the page is closed */
      }
    }
    await this.host.documentChanged(uri, text, open);
  }

  async undo() {
    const v = this.undoStack.pop();
    if (!v) return;
    this.redoStack.push(v);
    await api.setDocuments([[v.uri, v.before]]);
    await this.changed(v.uri, v.before, v.path);
  }

  async redo() {
    const v = this.redoStack.pop();
    if (!v) return;
    this.undoStack.push(v);
    await api.setDocuments([[v.uri, v.after]]);
    await this.changed(v.uri, v.after, v.path);
  }

  async duplicate() {
    const cur = this.host.current();
    if (!cur) return;
    const before = (await api.documentText(MY_MODELS_URI)) ?? "";
    const r = await api.duplicateClass(cur.path, MY_MODELS_URI, MY_MODELS);
    if (!r.ok) {
      this.host.log(`Duplicate failed: ${r.error}`, "err");
      return;
    }
    this.undoStack.push({ uri: MY_MODELS_URI, before, after: r.text, path: r.path! });
    this.redoStack = [];
    this.host.log(`Copied ${cur.path} to ${r.path} (${MY_MODELS_URI}); its diagram can be edited.`, "ok");
    await this.changed(r.uri, r.text, r.path);
    this.setEditing(true);
  }

  async newModel(name?: string) {
    const text = (await api.documentText(MY_MODELS_URI)) ?? "";
    let n = 1;
    while (new RegExp(`\\bmodel\\s+Model${n}\\b`).test(text)) n++;
    const pick = name ?? window.prompt("Name of the new model", `Model${n}`);
    if (!pick) return;
    const r = await api.newModel(MY_MODELS_URI, MY_MODELS, pick.trim());
    if (!r.ok) {
      this.host.log(`New model: ${r.error}`, "err");
      return;
    }
    this.undoStack.push({ uri: MY_MODELS_URI, before: text, after: r.text, path: r.path! });
    this.redoStack = [];
    await this.changed(r.uri, r.text, r.path);
    this.setEditing(true);
  }

  /** Load MyModels.mo saved by an earlier session. */
  async restore() {
    let text: string | null = null;
    try {
      text = localStorage.getItem(STORE_KEY);
    } catch {
      /* no storage */
    }
    if (!text) return;
    const errs = await api.setDocuments([[MY_MODELS_URI, text]]);
    if (errs[MY_MODELS_URI]) this.host.log(`${MY_MODELS_URI}: ${errs[MY_MODELS_URI][0]}`, "warn");
    await this.host.documentChanged(MY_MODELS_URI, text);
  }

  save() {
    const uri = this.host.current()?.source?.uri;
    if (!uri) return;
    void api.documentText(uri).then((t) => {
      const a = document.createElement("a");
      a.href = URL.createObjectURL(new Blob([t ?? ""], { type: "text/plain" }));
      a.download = uri.split("/").pop() || "model.mo";
      a.click();
      setTimeout(() => URL.revokeObjectURL(a.href), 1000);
    });
  }

  // ------------------------------------------------------------ parameter dialog

  async paramDialog(comp: ComponentInfo) {
    if (!comp.type_path) return;
    const params: ParamInfo[] = await api.parameters(comp.type_path);
    const mods = new Map(comp.modifiers.map(([k, v]) => [k, v]));
    const back = document.createElement("div");
    back.className = "param-dialog";
    const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
    const rows = params
      .map(
        (p) => `<tr title="${esc(p.description)}">
          <td class="k">${esc(p.name)}</td>
          <td><input data-param="${esc(p.name)}" value="${esc(mods.get(p.name) ?? "")}" placeholder="${esc(p.default ?? "")}" /></td>
          <td class="u">${esc(p.unit)}</td>
          <td class="d">${esc(p.description)}</td></tr>`,
      )
      .join("");
    back.innerHTML = `<form class="box" role="dialog" aria-modal="true" aria-label="Parameters of ${esc(comp.name)}">
        <h3>${esc(comp.type_name.split(".").pop()!)} <span class="muted">${esc(comp.type_path)}</span></h3>
        <label class="name">Name <input data-name value="${esc(comp.name)}" /></label>
        <p class="muted">${esc(comp.description)}</p>
        <div class="scroll"><table>${rows || "<tr><td class='muted'>No parameters.</td></tr>"}</table></div>
        <p class="muted small">Empty fields keep the default shown in grey. Values are written as modifiers into the model.</p>
        <div class="actions"><button type="button" data-cancel>Cancel</button><button type="submit" class="primary">OK</button></div>
      </form>`;
    const restore = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    document.body.appendChild(back);
    const form = back.querySelector("form")!;
    const focusable = () =>
      [...form.querySelectorAll<HTMLElement>("input, button, select, textarea, a[href]")].filter((el) => !el.hasAttribute("disabled"));
    (form.querySelector<HTMLInputElement>("input[data-param], input[data-name]") ?? form).focus();
    const close = () => {
      back.remove();
      restore?.focus();
    };
    back.querySelector<HTMLButtonElement>("[data-cancel]")!.onclick = close;
    back.addEventListener("keydown", (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        close();
        return;
      }
      if (e.key !== "Tab") return;
      const els = focusable();
      if (!els.length) return;
      const first = els[0];
      const last = els[els.length - 1];
      if (e.shiftKey && document.activeElement === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && document.activeElement === last) {
        e.preventDefault();
        first.focus();
      }
    });
    form.onsubmit = (e) => {
      e.preventDefault();
      const ops: unknown[] = [];
      back.querySelectorAll<HTMLInputElement>("input[data-param]").forEach((inp) => {
        const k = inp.dataset.param!;
        const v = inp.value.trim();
        const old = mods.get(k) ?? "";
        if (v !== old.trim()) ops.push({ op: "set_modifier", name: comp.name, modifier: k, value: v || null });
      });
      const newName = back.querySelector<HTMLInputElement>("input[data-name]")!.value.trim();
      if (newName && newName !== comp.name) ops.push({ op: "rename_component", name: comp.name, new_name: newName });
      close();
      if (ops.length) {
        void this.apply(ops).then(() => {
          if (newName && newName !== comp.name && this.selected.delete(comp.name)) this.selected.add(newName);
        });
      }
    };
  }
}

function sameConn(a: { from: string; to: string }, b: { from: string; to: string }) {
  const n = (s: string) => s.replace(/\s+/g, "");
  return (n(a.from) === n(b.from) && n(a.to) === n(b.to)) || (n(a.from) === n(b.to) && n(a.to) === n(b.from));
}

/**
 * Orthogonal route between two connectors: leave each along its outward direction, then
 * meet in the middle; a short detour goes around when the target lies behind the start.
 */
export function route(a: { center: Pt; dir: Pt }, b: { center: Pt; dir: Pt }): Pt[] {
  const r = (p: Pt): Pt => [Math.round(p[0]), Math.round(p[1])];
  const A = r(a.center);
  const B = r(b.center);
  const stub = 6;
  let pts: Pt[];
  if (a.dir[0] !== 0) {
    const ahead = (B[0] - A[0]) * a.dir[0] > 0;
    if (ahead && b.dir[0] !== 0) {
      const mx = Math.round((A[0] + B[0]) / 2);
      pts = [A, [mx, A[1]], [mx, B[1]], B];
    } else if (ahead) {
      pts = [A, [B[0], A[1]], B];
    } else {
      const a1: Pt = [A[0] + a.dir[0] * stub, A[1]];
      const b1: Pt = b.dir[0] !== 0 ? [B[0] + b.dir[0] * stub, B[1]] : [B[0], B[1] + b.dir[1] * stub];
      const my = A[1] === B[1] ? A[1] - 20 : Math.round((A[1] + B[1]) / 2);
      pts = [A, a1, [a1[0], my], [b1[0], my], b1, B];
    }
  } else {
    const ahead = (B[1] - A[1]) * a.dir[1] > 0;
    if (ahead && b.dir[1] !== 0) {
      const my = Math.round((A[1] + B[1]) / 2);
      pts = [A, [A[0], my], [B[0], my], B];
    } else if (ahead) {
      pts = [A, [A[0], B[1]], B];
    } else {
      const a1: Pt = [A[0], A[1] + a.dir[1] * stub];
      const b1: Pt = b.dir[1] !== 0 ? [B[0], B[1] + b.dir[1] * stub] : [B[0] + b.dir[0] * stub, B[1]];
      const mx = A[0] === B[0] ? A[0] - 20 : Math.round((A[0] + B[0]) / 2);
      pts = [A, a1, [mx, a1[1]], [mx, b1[1]], b1, B];
    }
  }
  // drop repeated points and the middle of straight runs
  const out: Pt[] = [];
  for (const p of pts) {
    const last = out[out.length - 1];
    if (last && last[0] === p[0] && last[1] === p[1]) continue;
    if (out.length >= 2) {
      const q = out[out.length - 2];
      if ((q[0] === last[0] && last[0] === p[0]) || (q[1] === last[1] && last[1] === p[1])) out.pop();
    }
    out.push(p);
  }
  return out;
}
