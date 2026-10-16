// Plot tools: matrix layout, plot expressions and signal operators, window statistics, and
// the variable menus that plot a component's or a connection's variables from the diagram.
import "./plottools.css";
import { PlotWindow, displayUnit, type Series } from "./plot";
import { ExprError, evaluate, isSpectrum, parse, variables, windowStats } from "./signal";
import type { ClassInfo, ComponentInfo, VarSummary } from "./types";

/** What the workbench (main.ts) lends the plot tools. */
export interface ToolsHost {
  plot: PlotWindow;
  /** Variables of the shown result (null: none). */
  vars(): VarSummary[] | null;
  /** Time axis and trajectories of the shown result (a live run's rows so far). */
  time(): Float64Array | null;
  col(name: string): Float64Array | undefined;
  /** Make the trajectories of `names` available through `col`. */
  fetch(names: string[]): Promise<void>;
  /** Plot or unplot a result variable in the active subplot. */
  toggle(name: string): Promise<void>;
  log(msg: string, cls?: string): void;
  /** Class information (for connector flow variables). */
  classInfo(path: string): Promise<ClassInfo | null>;
}

export function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) e.setAttribute(k, v);
  e.append(...kids);
  return e;
}

const fmt = (v: number) => {
  const a = Math.abs(v);
  if (!isFinite(v)) return String(v);
  if (a !== 0 && (a >= 1e5 || a < 1e-3)) return v.toExponential(3);
  return String(+v.toPrecision(5));
};

export const LAYOUTS: [string, { rows: number; cols: number } | null][] = [
  ["Stacked", null],
  ["1 × 2", { rows: 1, cols: 2 }],
  ["2 × 1", { rows: 2, cols: 1 }],
  ["2 × 2", { rows: 2, cols: 2 }],
  ["2 × 3", { rows: 2, cols: 3 }],
  ["3 × 2", { rows: 3, cols: 2 }],
  ["3 × 3", { rows: 3, cols: 3 }],
];

export class PlotTools {
  private statsOn = false;
  private statsQueued = false;
  private menu: HTMLElement | null = null;

  constructor(private host: ToolsHost, bar: HTMLElement, private statsEl: HTMLElement) {
    const plot = host.plot;
    // layout
    const layout = h("select", { id: "plot-layout", title: "Arrange the subplots in a matrix; the time axes zoom and pan together" });
    LAYOUTS.forEach(([label], i) => layout.append(h("option", { value: String(i) }, label)));
    layout.onchange = () => plot.setLayout(LAYOUTS[+layout.value][1]);
    // expression
    const expr = h("input", {
      id: "plot-expr",
      type: "text",
      placeholder: "Expression: a.w*60/(2*pi), der(x), abs(y), movavg(x, 0.1), rms(x, 0.02), fft(y)",
      title:
        "Plot an expression over the result variables (Enter). Operators + - * / ^; functions abs sqrt exp log log10 sin cos tan asin acos atan " +
        "sinh cosh tanh sign floor ceil min max pow atan2 mod; signal operators der(x), integral(x), movavg(x, window), rms(x[, window]), " +
        "and fft(x) (amplitude spectrum of the visible time window, in a new subplot); constants pi, e; time.",
      spellcheck: "false",
      autocomplete: "off",
    });
    expr.onkeydown = (e) => {
      if (e.key === "Enter") {
        const t = expr.value.trim();
        if (t) void this.addExpression(t).then((ok) => ok && (expr.value = ""));
      }
    };
    const exprBtn = h("button", { id: "plot-expr-add", title: "Plot the expression" }, "Plot");
    exprBtn.onclick = () => {
      const t = expr.value.trim();
      if (t) void this.addExpression(t).then((ok) => ok && (expr.value = ""));
    };
    // signal operators on the active subplot's variables
    const ops = h("select", { id: "plot-ops", title: "Apply a signal operator to the variables of the active subplot" });
    for (const [v, label] of [
      ["", "Signal…"],
      ["der", "Derivative"],
      ["integral", "Integral"],
      ["movavg", "Moving average"],
      ["rms", "Moving RMS"],
      ["fft", "FFT spectrum"],
    ])
      ops.append(h("option", { value: v }, label));
    ops.onchange = () => {
      const op = ops.value;
      ops.value = "";
      if (op) void this.applyOperator(op);
    };
    const stats = h("button", { id: "plot-stats-btn", title: "Min, max, mean, RMS and peak-to-peak of every plotted variable over the visible time window" }, "Stats");
    stats.onclick = () => {
      this.statsOn = !this.statsOn;
      stats.classList.toggle("active", this.statsOn);
      this.statsEl.classList.toggle("hidden", !this.statsOn);
      this.refreshStats();
    };
    bar.append(h("label", {}, "Layout ", layout), expr, exprBtn, ops, stats);
    const prev = plot.onView;
    plot.onView = () => {
      prev();
      if (this.statsOn && !this.statsQueued) {
        this.statsQueued = true;
        requestAnimationFrame(() => {
          this.statsQueued = false;
          this.refreshStats();
        });
      }
    };
    document.addEventListener("pointerdown", (e) => {
      if (this.menu && !this.menu.contains(e.target as Node)) this.closeMenu();
    });
    window.addEventListener("keydown", (e) => e.key === "Escape" && this.closeMenu());
  }

  // ---------------------------------------------------------------- expressions
  private has = (n: string) => !!this.host.vars()?.some((v) => v.name === n);

  /** Variables every expression series needs. */
  exprNames(): string[] {
    const out = new Set<string>();
    for (const sp of this.host.plot.subplots)
      for (const s of sp.series)
        if (s.expr && !s.run) {
          try {
            for (const n of variables(parse(s.expr), this.has)) out.add(n);
          } catch {
            /* reported when it was added */
          }
        }
    return [...out];
  }

  /** Data of an expression series over the current result (null: cannot be evaluated). */
  evalSeries(s: Series): { x: Float64Array; y: Float64Array } | null {
    if (!s.expr) return null;
    const time = this.host.time();
    if (!time || !time.length) return null;
    try {
      const node = parse(s.expr);
      const range = this.host.plot.visibleRange();
      // Rows every input has (a live run's variables may lag the time axis by a slice).
      let n = time.length;
      for (const name of variables(node, this.has)) n = Math.min(n, this.host.col(name)?.length ?? n);
      return evaluate(node, {
        time: time.subarray(0, n),
        get: (name) => this.host.col(name)?.subarray(0, n),
        has: this.has,
        window: range,
      });
    } catch {
      return null;
    }
  }

  /** Parse, fetch and plot an expression; fft() goes into a new frequency subplot. */
  async addExpression(text: string): Promise<boolean> {
    const plot = this.host.plot;
    if (!this.host.vars()) {
      this.host.log("Simulate a model before plotting expressions.", "warn");
      return false;
    }
    let node;
    try {
      node = parse(text);
    } catch (e) {
      this.host.log(`expression: ${(e as Error).message}`, "err");
      return false;
    }
    const names = variables(node, this.has);
    const unknown = names.filter((n) => !this.has(n));
    if (unknown.length) {
      this.host.log(`expression: unknown variable${unknown.length > 1 ? "s" : ""} ${unknown.join(", ")}`, "err");
      return false;
    }
    await this.host.fetch(names);
    const s: Series = { name: text, unit: "", x: new Float64Array(), y: new Float64Array(), color: "", expr: text };
    // A single variable keeps its unit (and display unit) through linear operators.
    const d = this.evalSeries(s);
    if (!d) {
      try {
        evaluate(node, { time: this.host.time()!, get: (n) => this.host.col(n), has: this.has });
      } catch (e) {
        this.host.log(`expression: ${e instanceof ExprError ? e.message : String(e)}`, "err");
      }
      return false;
    }
    if (isSpectrum(node)) {
      let k = plot.subplots.findIndex((sp) => sp.xName === "frequency");
      if (k < 0) {
        const empty = plot.subplots.findIndex((sp) => sp.series.length === 0 && !sp.xName);
        if (empty >= 0) k = empty;
        else {
          plot.addSubplot();
          k = plot.subplots.length - 1;
        }
        plot.subplots[k].xName = "frequency";
        plot.subplots[k].xUnit = "Hz";
      }
      plot.active = k;
      // Show the frequencies that carry the signal: up to 1.5 times the highest one above 1 % of the peak.
      let peak = 0;
      for (let i = 1; i < d.y.length; i++) peak = Math.max(peak, d.y[i]);
      let last = 0;
      for (let i = 1; i < d.y.length; i++) if (d.y[i] > 0.01 * peak) last = i;
      const fmax = Math.min(d.x[d.x.length - 1], 1.5 * d.x[Math.max(1, last)]);
      const prevZoom = plot.subplots[k].xZoom;
      plot.add({ name: text, unit: "", x: d.x, y: d.y, expr: text, scale: 1, offset: 0, displayUnit: "" }, k);
      if (fmax > 0) plot.subplots[k].xZoom = prevZoom && prevZoom[1] > fmax ? prevZoom : [0, fmax];
      plot.draw();
    } else {
      if (plot.subplots[plot.active].xName) {
        const k = plot.subplots.findIndex((sp) => !sp.xName);
        if (k >= 0) plot.active = k;
        else plot.addSubplot();
      }
      plot.add({ name: text, unit: "", x: d.x, y: d.y, expr: text, scale: 1, offset: 0, displayUnit: "" });
    }
    return true;
  }

  /** Apply a signal operator to each variable of the active subplot. */
  async applyOperator(op: string) {
    const plot = this.host.plot;
    const sp = plot.subplots[plot.active];
    const src = sp.series.filter((s) => !s.run && !sp.xName);
    if (!src.length) {
      this.host.log("Plot a variable first: the operator applies to the variables of the active subplot.", "warn");
      return;
    }
    const [a, b] = plot.visibleRange();
    // Windows: 2 % of the visible span.
    const w = +((b - a) * 0.02).toPrecision(2);
    for (const s of src) {
      const arg = s.expr ? `(${s.expr})` : s.name;
      const text = op === "movavg" || op === "rms" ? `${op}(${arg}, ${w})` : `${op}(${arg})`;
      // der() of a state is the solver's derivative; plain der(name) finds it.
      await this.addExpression(text);
      if (op !== "fft") plot.active = plot.subplots.indexOf(sp);
    }
  }

  // ---------------------------------------------------------------- statistics
  refreshStats() {
    if (!this.statsOn) return;
    const plot = this.host.plot;
    const [a, b] = plot.visibleRange();
    const rows: HTMLElement[] = [];
    plot.subplots.forEach((sp, k) => {
      for (const s of sp.series) {
        const [lo, hi] = sp.xName ? (sp.xZoom ?? [-Infinity, Infinity]) : [a, b];
        const st = windowStats(s.x, s.y, lo, hi);
        const sc = s.scale ?? 1;
        const of = s.offset ?? 0;
        const tr = (v: number) => v * sc + of;
        const sw = h("span", { class: "sw" });
        sw.style.background = s.color;
        rows.push(
          h(
            "tr",
            {},
            h("td", {}, String(k + 1)),
            h("td", { class: "nm", title: s.name }, sw, s.name),
            h("td", {}, s.displayUnit ?? ""),
            ...(st
              ? [tr(st.min), tr(st.max), tr(st.mean), Math.abs(sc) * st.rms + (of ? NaN : 0), Math.abs(sc) * st.p2p].map((v) => h("td", { class: "v" }, isNaN(v) ? "—" : fmt(v)))
              : [h("td", { class: "muted", colspan: "5" }, "no points in the window")]),
          ),
        );
      }
    });
    this.statsEl.replaceChildren(
      h("div", { class: "stats-head" }, `Window ${fmt(a)} … ${fmt(b)} s`, h("span", { class: "muted" }, "  (drag to zoom, wheel to zoom, Shift+drag to pan, double-click to reset)")),
      h(
        "table",
        { class: "stats-table" },
        h("thead", {}, h("tr", {}, ...["#", "Variable", "Unit", "Min", "Max", "Mean", "RMS", "Peak-to-peak"].map((t) => h("th", {}, t)))),
        h("tbody", {}, ...rows),
      ),
    );
  }

  // ---------------------------------------------------------------- variable menus
  closeMenu() {
    this.menu?.remove();
    this.menu = null;
  }

  private showMenu(x: number, y: number, title: string, sub: string, items: { name: string; label: string; tag?: string; unit: string }[]) {
    this.closeMenu();
    const plot = this.host.plot;
    const m = h("div", { class: "var-menu", role: "menu" });
    m.append(h("div", { class: "vm-title" }, title), h("div", { class: "vm-sub" }, sub));
    if (!items.length) m.append(h("div", { class: "muted vm-empty" }, "No result variables here. Simulate the model first."));
    const list = h("div", { class: "vm-list" });
    for (const it of items) {
      const cb = h("input", { type: "checkbox" });
      cb.checked = plot.has(it.name);
      cb.indeterminate = !cb.checked && plot.hasAnywhere(it.name);
      const row = h("label", { class: "vm-row", title: it.name }, cb, h("span", { class: "vm-name" }, it.label));
      if (it.tag) row.append(h("span", { class: `vm-tag ${it.tag}` }, it.tag));
      if (it.unit) row.append(h("span", { class: "vm-unit" }, displayUnit(it.unit).unit));
      cb.onchange = async () => {
        await this.host.toggle(it.name);
        cb.checked = plot.has(it.name);
      };
      list.append(row);
    }
    m.append(list);
    document.body.append(m);
    const r = m.getBoundingClientRect();
    m.style.left = `${Math.max(4, Math.min(x + 6, innerWidth - r.width - 8))}px`;
    m.style.top = `${Math.max(4, Math.min(y + 6, innerHeight - r.height - 8))}px`;
    this.menu = m;
  }

  /** Menu of a component's variables: its own first, then those of its connectors. */
  componentMenu(comp: ComponentInfo, x: number, y: number) {
    const vars = (this.host.vars() ?? []).filter((v) => v.kind !== "param" && (v.name.startsWith(`${comp.name}.`) || v.name.startsWith(`der(${comp.name}.`)));
    const rel = (v: VarSummary) => v.name.replace(/^der\((.*)\)$/, "$1").slice(comp.name.length + 1);
    const depth = (v: VarSummary) => rel(v).split(".").length;
    const items = vars
      .filter((v) => depth(v) <= 2)
      .sort((a, b) => depth(a) - depth(b) || a.name.localeCompare(b.name))
      .slice(0, 60)
      .map((v) => ({ name: v.name, label: v.name.startsWith("der(") ? `der(${rel(v)})` : rel(v), unit: v.unit, tag: v.kind === "alias" ? "" : undefined }));
    this.showMenu(x, y, comp.name, comp.type_name, items);
  }

  /** Menu of a connection's across and flow variables (both connectors). */
  async connectionMenu(cls: ClassInfo, from: string, to: string, x: number, y: number) {
    const all = this.host.vars() ?? [];
    const flows = new Set<string>();
    for (const ref of [from, to]) for (const f of await this.flowVars(cls, ref)) flows.add(`${ref}.${f}`);
    const items: { name: string; label: string; tag?: string; unit: string }[] = [];
    for (const ref of [from, to]) {
      const vs = all.filter((v) => v.kind !== "param" && v.name.startsWith(`${ref}.`) && !v.name.slice(ref.length + 1).includes("."));
      vs.sort((a, b) => Number(flows.has(a.name)) - Number(flows.has(b.name)) || a.name.localeCompare(b.name));
      for (const v of vs) items.push({ name: v.name, label: v.name, tag: flows.has(v.name) ? "flow" : "across", unit: v.unit });
    }
    this.showMenu(x, y, `connect(${from}, ${to})`, "Across variables are equal on both sides; flow variables sum to zero", items);
  }

  /** Names of the flow variables of the connector `ref` (`comp.port` or `port`) of `cls`. */
  private async flowVars(cls: ClassInfo, ref: string): Promise<string[]> {
    const parts = ref.replace(/\[[^\]]*\]/g, "").split(".");
    const members = async (c: ClassInfo): Promise<ComponentInfo[]> => {
      const out = [...c.components];
      for (const e of c.extends) {
        const b = await this.host.classInfo(e);
        if (b) out.push(...(await members(b)));
      }
      return out;
    };
    let owner: ClassInfo | null = cls;
    for (let i = 0; i < parts.length && owner; i++) {
      const comp: ComponentInfo | undefined = (await members(owner)).find((c) => c.name === parts[i]);
      if (!comp?.type_path) return [];
      owner = await this.host.classInfo(comp.type_path);
    }
    if (!owner) return [];
    return (await members(owner)).filter((c) => c.flow).map((c) => c.name);
  }
}
