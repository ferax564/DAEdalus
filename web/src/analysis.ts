// Analysis panel: local sensitivities, uncertainty (Monte Carlo, Sobol, Morris), calibration
// against measured data and design optimization. Every analysis runs in the worker in
// slices (see worker.ts "analysisRun") so progress shows and Cancel takes effect between runs.
import "./analysis.css";
import { api } from "./rpc";
import type { SimSettings, VarSummary } from "./types";
import { BarChart, PlotWindow, type Series } from "./plot";

/** What the panel needs from the workbench. */
export interface AnalysisHost {
  /** The open model. */
  model(): string | null;
  /** Variables of the open model's last simulation (null if it has not been simulated). */
  vars(): VarSummary[] | null;
  settings(): SimSettings;
  /** Variables plotted in the plot window. */
  plotted(): string[];
  simulate(): Promise<void>;
  /** Set parameter overrides in the variable browser and re-simulate. */
  applyParams(p: [string, number][]): void;
  log(msg: string, cls?: string): void;
}

type Tab = "sensitivity" | "uncertainty" | "calibrate" | "optimize";
type Num = number | null;

interface SensResult {
  method: string;
  time: Num[];
  params: string[];
  param_values: number[];
  outputs: string[];
  values: Num[][];
  dydp: Num[][][];
  normalized: Num[][][];
  ranking: { param: string; score: number; per_output: number[] }[];
  notes: string[];
  report: string;
}
interface Band {
  name: string;
  mean: Num[];
  std: Num[];
  q05: Num[];
  q50: Num[];
  q95: Num[];
}
interface GsaResult {
  mode: "montecarlo" | "sobol" | "morris";
  params: string[];
  runs: number;
  failed: number;
  errors: string[];
  time: Num[];
  bands: Band[];
  qois: { name: string; mean: Num; std: Num; q05: Num; q50: Num; q95: Num }[];
  sobol: { qoi: string; s1: Num[]; st: Num[]; s1_conf: Num[]; st_conf: Num[] }[];
  morris: { qoi: string; mu: Num[]; mu_star: Num[]; sigma: Num[] }[];
  report: string;
}
interface CalibResult {
  method: string;
  params: { name: string; initial: number; value: number; lo: Num; hi: Num; std_error: Num; at_bound: boolean }[];
  correlation: Num[][];
  rms_before: Num;
  rms_after: Num;
  columns: { column: string; var: string; weight: number; rms_before: Num; rms_after: Num; t: Num[]; measured: Num[]; before: Num[]; after: Num[] }[];
  time: Num[];
  iterations: number;
  converged: boolean;
  message: string;
  modifier: string;
  report: string;
}
interface OptResult {
  params: string[];
  initial: number[];
  values: number[];
  objective_before: Num;
  objective_after: Num;
  qois: string[];
  qoi_before: Num[];
  qoi_after: Num[];
  iterations: number;
  runs: number;
  history: Num[];
  message: string;
  modifier: string;
  report: string;
}

const f64 = (a: Num[]) => Float64Array.from(a, (v) => (v === null ? NaN : v));
const fmt = (v: Num | undefined, d = 5) => (v === null || v === undefined || !isFinite(v) ? "—" : Math.abs(v) !== 0 && (Math.abs(v) >= 1e5 || Math.abs(v) < 1e-3) ? v.toExponential(d - 1) : String(+v.toPrecision(d)));
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

/** Per-parameter settings of the tabs. */
interface ParamCfg {
  dist: "uniform" | "normal" | "lognormal";
  a: string;
  b: string;
  lo: string;
  hi: string;
}

export class AnalysisPanel {
  readonly pane: HTMLElement;
  private tab: Tab = "sensitivity";
  private selected = new Set<string>();
  private cfg = new Map<string, ParamCfg>();
  private results = new Map<Tab, unknown>();
  private running = false;
  private plot: PlotWindow;
  private bars: BarChart;
  private csv: { name: string; text: string; header: string[] } | null = null;
  private colMap = new Map<string, { v: string; w: string }>();
  private objectives: { qoi: string; goal: "min" | "max" | "target"; target: string }[] = [];
  private q = <T extends HTMLElement = HTMLElement>(sel: string) => this.pane.querySelector(sel) as T;

  constructor(center: HTMLElement, private host: AnalysisHost) {
    this.pane = document.createElement("div");
    this.pane.id = "analysis-pane";
    this.pane.className = "pane analysis hidden";
    this.pane.innerHTML = `
      <div class="tabs an-tabs">
        <span class="model-title">Analysis</span>
        <button data-tab="sensitivity" class="active">Sensitivity</button>
        <button data-tab="uncertainty">Uncertainty</button>
        <button data-tab="calibrate">Calibrate</button>
        <button data-tab="optimize">Optimize</button>
        <span class="spacer"></span>
        <div class="an-progress hidden"><div class="track"><div class="fill"></div></div><span class="label"></span></div>
        <button class="primary an-run">Run</button>
        <button class="an-cancel" disabled>Cancel</button>
      </div>
      <div class="an-body">
        <div class="an-side">
          <div class="an-need hidden"></div>
          <div class="an-section an-params">
            <div class="an-h">Parameters <input class="an-filter" type="search" placeholder="Filter" /></div>
            <div class="an-plist"></div>
          </div>
          <div class="an-section an-config"></div>
        </div>
        <div class="an-main">
          <div class="an-plot plot-host"></div>
          <div class="an-bars plot-host"></div>
          <div class="an-report"></div>
        </div>
      </div>`;
    center.appendChild(this.pane);
    this.plot = new PlotWindow(this.q(".an-plot"));
    this.plot.emptyText = "Choose parameters and outputs, then Run";
    this.bars = new BarChart(this.q(".an-bars"));
    this.pane.querySelectorAll<HTMLButtonElement>("button[data-tab]").forEach((b) => {
      b.onclick = () => {
        this.tab = b.dataset.tab as Tab;
        this.pane.querySelectorAll("button[data-tab]").forEach((x) => x.classList.toggle("active", x === b));
        this.renderConfig();
        this.showResult();
      };
    });
    this.q(".an-filter").addEventListener("input", () => this.renderParams());
    this.q<HTMLButtonElement>(".an-run").onclick = () => void this.run();
    this.q<HTMLButtonElement>(".an-cancel").onclick = () => void api.analysisCancel();
  }

  setVisible(on: boolean) {
    this.pane.classList.toggle("hidden", !on);
    if (on) {
      this.refresh();
      requestAnimationFrame(() => this.showResult());
    }
  }

  /** Re-read the model's parameters (after a simulation or opening another model). */
  refresh() {
    const vars = this.host.vars();
    const need = this.q(".an-need");
    if (!vars) {
      const m = this.host.model();
      need.classList.remove("hidden");
      need.innerHTML = m
        ? `<p>Simulate <b>${esc(m.split(".").pop()!)}</b> once to list its parameters and variables.</p><button class="primary">Simulate</button>`
        : `<p>Open a model first.</p>`;
      const b = need.querySelector("button");
      if (b) b.onclick = () => void this.host.simulate().then(() => this.refresh());
    } else need.classList.add("hidden");
    for (const s of [...this.selected]) if (!this.params().some((p) => p.name === s)) this.selected.delete(s);
    this.renderParams();
    this.renderConfig();
  }

  private params(): VarSummary[] {
    return (this.host.vars() ?? []).filter((v) => v.kind === "param" && v.editable && !v.name.startsWith("$"));
  }

  private series(): string[] {
    return (this.host.vars() ?? []).filter((v) => v.kind !== "param" && v.name !== "time").map((v) => v.name);
  }

  private value(name: string): number {
    return this.params().find((p) => p.name === name)?.value ?? 0;
  }

  private config(name: string): ParamCfg {
    let c = this.cfg.get(name);
    if (!c) {
      const v = this.value(name);
      const d = Math.abs(v) || 1;
      const r = (x: number) => String(+x.toPrecision(4));
      c = { dist: "uniform", a: r(v - 0.1 * d), b: r(v + 0.1 * d), lo: r(v === 0 ? -1 : v > 0 ? v / 2 : 2 * v), hi: r(v === 0 ? 1 : v > 0 ? 2 * v : v / 2) };
      this.cfg.set(name, c);
    }
    return c;
  }

  private renderParams() {
    const host = this.q(".an-plist");
    const filter = this.q<HTMLInputElement>(".an-filter").value.trim().toLowerCase();
    const ps = this.params();
    const shown = ps.filter((p) => this.selected.has(p.name) || !filter || p.name.toLowerCase().includes(filter));
    shown.sort((a, b) => Number(this.selected.has(b.name)) - Number(this.selected.has(a.name)));
    host.innerHTML = "";
    for (const p of shown.slice(0, 300)) {
      const row = document.createElement("label");
      row.className = "an-prow";
      row.title = `${p.name}${p.unit ? ` [${p.unit}]` : ""}${p.description ? `\n${p.description}` : ""}`;
      row.innerHTML = `<input type="checkbox"${this.selected.has(p.name) ? " checked" : ""}/><span class="nm">${esc(p.name)}</span><span class="val">${fmt(p.value)}</span>`;
      row.querySelector("input")!.onchange = (e) => {
        if ((e.target as HTMLInputElement).checked) this.selected.add(p.name);
        else this.selected.delete(p.name);
        this.renderConfig();
      };
      host.appendChild(row);
    }
    if (!ps.length && this.host.vars()) host.innerHTML = `<p class="muted">This model has no free parameters.</p>`;
    else if (shown.length > 300) host.insertAdjacentHTML("beforeend", `<p class="muted">${shown.length - 300} more: use the filter</p>`);
  }

  // ------------------------------------------------------------------ configuration
  private field(id: string, label: string, value: string, attrs = ""): string {
    return `<label class="an-field">${label}<input data-k="${id}" value="${esc(value)}" ${attrs}/></label>`;
  }

  private stored = new Map<string, string>();
  private get(k: string, dflt: string): string {
    return this.stored.get(k) ?? dflt;
  }

  private renderConfig() {
    const host = this.q(".an-config");
    const sel = [...this.selected];
    const outputsDefault = this.defaultOutputs().join(", ");
    const none = `<p class="muted">Tick parameters above.</p>`;
    const list = `<datalist id="an-vars">${this.series().slice(0, 2000).map((n) => `<option value="${esc(n)}">`).join("")}</datalist>`;
    let h = "";
    if (this.tab === "sensitivity") {
      h = `<div class="an-h">Sensitivity</div>
        ${this.field("s.outputs", "Outputs", this.get("s.outputs", outputsDefault), 'list="an-vars"')}
        <label class="an-field">Method <select data-k="s.method">${["auto", "exact", "fd"].map((m) => `<option${this.get("s.method", "auto") === m ? " selected" : ""}>${m}</option>`).join("")}</select></label>
        <p class="muted small">exact: forward sensitivity equations with exact tape derivatives (models without events); fd: central differences, two runs per parameter.</p>
        ${sel.length ? `<div class="muted small">${sel.length} parameter${sel.length > 1 ? "s" : ""}: ${esc(sel.join(", "))}</div>` : none}`;
    } else if (this.tab === "uncertainty") {
      const mode = this.get("u.mode", "montecarlo");
      h = `<div class="an-h">Uncertainty</div>
        <label class="an-field">Analysis <select data-k="u.mode">${[["montecarlo", "Monte Carlo"], ["sobol", "Sobol indices"], ["morris", "Morris screening"]].map(([v, l]) => `<option value="${v}"${mode === v ? " selected" : ""}>${l}</option>`).join("")}</select></label>
        ${this.field("u.n", mode === "sobol" ? "Base samples N" : mode === "morris" ? "Trajectories r" : "Samples", this.get("u.n", mode === "sobol" ? "256" : mode === "morris" ? "20" : "200"), 'type="number" min="2"')}
        <label class="an-field">Sampling <select data-k="u.sampling">${["lhs", "random"].map((m) => `<option${this.get("u.sampling", "lhs") === m ? " selected" : ""}>${m}</option>`).join("")}</select></label>
        ${this.field("u.seed", "Seed", this.get("u.seed", "12345"), 'type="number"')}
        ${this.field("u.outputs", "Outputs (bands)", this.get("u.outputs", outputsDefault), 'list="an-vars"')}
        ${this.field("u.qois", "Quantities", this.get("u.qois", this.defaultOutputs().slice(0, 2).map((o) => `final(${o})`).join(", ")))}
        <p class="muted small">Quantities: final(x), max(x), min(x), mean(x), integral(x), at(x, t); a window: max(x, t0, t1). Runs: ${mode === "sobol" ? "N (k + 2)" : mode === "morris" ? "r (k + 1)" : "N"}.</p>
        ${sel.length ? `<table class="an-table"><tr><th>parameter</th><th>distribution</th><th>a</th><th>b</th></tr>${sel
          .map((n) => {
            const c = this.config(n);
            const [la, lb] = c.dist === "uniform" ? ["lo", "hi"] : ["mean", "std"];
            return `<tr data-p="${esc(n)}"><td>${esc(n)}</td><td><select data-c="dist">${["uniform", "normal", "lognormal"].map((d) => `<option${c.dist === d ? " selected" : ""}>${d}</option>`).join("")}</select></td><td><input data-c="a" value="${esc(c.a)}" title="${la}"/></td><td><input data-c="b" value="${esc(c.b)}" title="${lb}"/></td></tr>`;
          })
          .join("")}</table><p class="muted small">uniform: lo, hi; normal and lognormal: mean, std of the parameter.</p>` : none}`;
    } else if (this.tab === "calibrate") {
      h = `<div class="an-h">Measured data</div>
        <label class="file an-file">${this.csv ? esc(this.csv.name) : "Load CSV…"}<input type="file" accept=".csv,.txt" data-k="c.file"/></label>
        ${this.csv ? this.mapTable() : `<p class="muted small">A header row, a time column (named time, or the first) and columns of measured values.</p>`}
        <div class="an-row">${this.field("c.t0", "Window from", this.get("c.t0", ""), 'type="number" step="any" placeholder="start"')}${this.field("c.t1", "to", this.get("c.t1", ""), 'type="number" step="any" placeholder="end"')}</div>
        <label class="an-field">Sensitivities <select data-k="c.method">${["auto", "exact", "fd"].map((m) => `<option${this.get("c.method", "auto") === m ? " selected" : ""}>${m}</option>`).join("")}</select></label>
        ${this.field("c.iter", "Max iterations", this.get("c.iter", "40"), 'type="number" min="1"')}
        <div class="an-h">Tuners</div>${sel.length ? this.boundsTable(sel) : none}`;
    } else {
      if (!this.objectives.length) {
        const o = this.defaultOutputs()[0];
        if (o) this.objectives.push({ qoi: `max(${o})`, goal: "min", target: "" });
      }
      h = `<div class="an-h">Objective</div>
        <table class="an-table an-obj">${this.objectives
          .map(
            (o, i) =>
              `<tr data-o="${i}"><td><select data-c="goal">${[["min", "minimize"], ["max", "maximize"], ["target", "target"]].map(([v, l]) => `<option value="${v}"${o.goal === v ? " selected" : ""}>${l}</option>`).join("")}</select></td><td><input data-c="qoi" value="${esc(o.qoi)}" class="wide"/></td><td><input data-c="target" value="${esc(o.target)}" placeholder="value"${o.goal === "target" ? "" : " disabled"}/></td><td><button data-c="del" title="Remove">✕</button></td></tr>`,
          )
          .join("")}</table>
        <button class="an-addobj">Add term</button>
        ${this.field("o.iter", "Max iterations", this.get("o.iter", "200"), 'type="number" min="1"')}
        <p class="muted small">Nelder–Mead on the bounded parameters; the objective is the sum of the terms (target terms are squared errors).</p>
        <div class="an-h">Design parameters</div>${sel.length ? this.boundsTable(sel) : none}`;
    }
    host.innerHTML = h + list;
    host.querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-k]").forEach((e) => {
      const k = e.dataset.k!;
      if (k === "c.file") {
        e.onchange = async () => {
          const f = (e as HTMLInputElement).files?.[0];
          if (!f) return;
          this.loadCsv(f.name, await f.text());
        };
        return;
      }
      e.addEventListener("change", () => {
        this.stored.set(k, e.value);
        if (k === "u.mode") this.renderConfig();
      });
    });
    host.querySelectorAll<HTMLTableRowElement>("tr[data-p]").forEach((tr) => {
      const c = this.config(tr.dataset.p!);
      tr.querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-c]").forEach((e) => {
        e.addEventListener("change", () => {
          const k = e.dataset.c as keyof ParamCfg;
          (c as unknown as Record<string, string>)[k] = e.value;
          if (k === "dist") {
            const v = this.value(tr.dataset.p!);
            const d = Math.abs(v) || 1;
            if (e.value === "uniform") [c.a, c.b] = [String(+(v - 0.1 * d).toPrecision(4)), String(+(v + 0.1 * d).toPrecision(4))];
            else [c.a, c.b] = [String(v || 1), String(+(0.05 * d).toPrecision(3))];
            this.renderConfig();
          }
        });
      });
    });
    host.querySelectorAll<HTMLTableRowElement>("tr[data-col]").forEach((tr) => {
      const m = this.colMap.get(tr.dataset.col!)!;
      tr.querySelectorAll<HTMLInputElement | HTMLSelectElement>("[data-c]").forEach((e) =>
        e.addEventListener("change", () => {
          if (e.dataset.c === "v") m.v = e.value;
          else m.w = e.value;
        }),
      );
    });
    host.querySelectorAll<HTMLTableRowElement>("tr[data-o]").forEach((tr) => {
      const o = this.objectives[+tr.dataset.o!];
      tr.querySelectorAll<HTMLInputElement | HTMLSelectElement | HTMLButtonElement>("[data-c]").forEach((e) => {
        if (e.dataset.c === "del") {
          (e as HTMLButtonElement).onclick = () => {
            this.objectives.splice(+tr.dataset.o!, 1);
            this.renderConfig();
          };
          return;
        }
        e.addEventListener("change", () => {
          const k = e.dataset.c as "qoi" | "goal" | "target";
          (o as Record<string, string>)[k] = e.value;
          if (k === "goal") this.renderConfig();
        });
      });
    });
    const add = host.querySelector<HTMLButtonElement>(".an-addobj");
    if (add)
      add.onclick = () => {
        this.objectives.push({ qoi: `final(${this.defaultOutputs()[0] ?? "x"})`, goal: "target", target: "0" });
        this.renderConfig();
      };
  }

  private boundsTable(sel: string[]): string {
    return `<table class="an-table"><tr><th>parameter</th><th>value</th><th>lower</th><th>upper</th></tr>${sel
      .map((n) => {
        const c = this.config(n);
        return `<tr data-p="${esc(n)}"><td>${esc(n)}</td><td class="num">${fmt(this.value(n))}</td><td><input data-c="lo" value="${esc(c.lo)}"/></td><td><input data-c="hi" value="${esc(c.hi)}"/></td></tr>`;
      })
      .join("")}</table>`;
  }

  private mapTable(): string {
    const vars = new Set(this.series());
    const opts = (cur: string) => `<option value="">(ignore)</option>` + [...vars].slice(0, 3000).map((v) => `<option${v === cur ? " selected" : ""}>${esc(v)}</option>`).join("");
    return `<table class="an-table"><tr><th>column</th><th>model variable</th><th>weight</th></tr>${this.csv!.header.slice(1)
      .map((c) => {
        const m = this.colMap.get(c)!;
        return `<tr data-col="${esc(c)}"><td>${esc(c)}</td><td><select data-c="v">${opts(m.v)}</select></td><td><input data-c="w" value="${esc(m.w)}" placeholder="auto"/></td></tr>`;
      })
      .join("")}</table><p class="muted small">Weight auto: 1 / the column's standard deviation.</p>`;
  }

  private loadCsv(name: string, text: string) {
    const head = text.split(/\r?\n/).find((l) => l.trim() && !l.startsWith("#")) ?? "";
    const sep = [",", ";", "\t"].sort((a, b) => head.split(b).length - head.split(a).length)[0];
    let header = head.split(sep).map((h) => h.trim().replace(/^"|"$/g, ""));
    // The time column goes first.
    const ti = header.indexOf("time");
    if (ti > 0) header = [header[ti], ...header.filter((_, i) => i !== ti)];
    this.csv = { name, text, header };
    const vars = new Set(this.series());
    this.colMap.clear();
    for (const c of header.slice(1)) this.colMap.set(c, { v: vars.has(c) ? c : "", w: "" });
    this.host.log(`Loaded ${name}: ${header.length - 1} data columns`, "ok");
    this.renderConfig();
  }

  private defaultOutputs(): string[] {
    const s = new Set(this.series());
    const p = this.host.plotted().filter((n) => s.has(n));
    if (p.length) return p.slice(0, 4);
    return this.series()
      .filter((n) => !n.startsWith("der("))
      .slice(0, 2);
  }

  private csvList(k: string, dflt: string): string[] {
    return this.get(k, dflt)
      .split(/,(?![^(]*\))/)
      .map((s) => s.trim())
      .filter(Boolean);
  }

  // ------------------------------------------------------------------ running
  private spec(): { kind: string; spec: unknown } {
    const sel = [...this.selected];
    const num = (s: string, what: string) => {
      const v = parseFloat(s);
      if (!isFinite(v)) throw new Error(`${what}: '${s}' is not a number`);
      return v;
    };
    const bounds = () =>
      sel.map((n) => {
        const c = this.config(n);
        return { name: n, lo: c.lo.trim() ? num(c.lo, `lower bound of ${n}`) : null, hi: c.hi.trim() ? num(c.hi, `upper bound of ${n}`) : null };
      });
    if (!sel.length) throw new Error("Tick at least one parameter.");
    switch (this.tab) {
      case "sensitivity": {
        const outputs = this.csvList("s.outputs", this.defaultOutputs().join(","));
        if (!outputs.length) throw new Error("Give at least one output.");
        return { kind: "sensitivity", spec: { params: sel, outputs, method: this.get("s.method", "auto") } };
      }
      case "uncertainty": {
        const mode = this.get("u.mode", "montecarlo");
        const params = sel.map((n) => {
          const c = this.config(n);
          const a = num(c.a, n);
          const b = num(c.b, n);
          return { name: n, dist: c.dist === "uniform" ? { kind: "uniform", lo: a, hi: b } : { kind: c.dist, mean: a, std: b } };
        });
        const n = parseInt(this.get("u.n", mode === "sobol" ? "256" : mode === "morris" ? "20" : "200"), 10);
        return {
          kind: "gsa",
          spec: {
            mode,
            params,
            outputs: mode === "montecarlo" ? this.csvList("u.outputs", this.defaultOutputs().join(",")) : [],
            qois: this.csvList("u.qois", this.defaultOutputs().slice(0, 2).map((o) => `final(${o})`).join(",")),
            n,
            sampling: this.get("u.sampling", "lhs"),
            seed: parseInt(this.get("u.seed", "12345"), 10) || 0,
            levels: 4,
          },
        };
      }
      case "calibrate": {
        if (!this.csv) throw new Error("Load a CSV file with measurements.");
        const map = [...this.colMap.entries()].filter(([, m]) => m.v).map(([c, m]) => ({ column: c, var: m.v, weight: m.w.trim() ? num(m.w, `weight of ${c}`) : null }));
        if (!map.length) throw new Error("Map at least one data column to a model variable.");
        const t0 = parseFloat(this.get("c.t0", ""));
        const t1 = parseFloat(this.get("c.t1", ""));
        const window = isFinite(t0) || isFinite(t1) ? [isFinite(t0) ? t0 : -1e300, isFinite(t1) ? t1 : 1e300] : null;
        return {
          kind: "calibrate",
          spec: { tuners: bounds(), csv: this.csv.text, time_column: this.csv.header[0], map, window, method: this.get("c.method", "auto"), max_iter: parseInt(this.get("c.iter", "40"), 10) || 40 },
        };
      }
      case "optimize": {
        if (!this.objectives.length) throw new Error("Add an objective term.");
        const objective = this.objectives.map((o) => ({ qoi: o.qoi, weight: o.goal === "max" ? -1 : 1, target: o.goal === "target" ? num(o.target, `target of ${o.qoi}`) : null }));
        return { kind: "optimize", spec: { tuners: bounds(), objective, max_iter: parseInt(this.get("o.iter", "200"), 10) || 200, tol: 1e-8 } };
      }
    }
  }

  private async run() {
    const model = this.host.model();
    if (!model || this.running) return;
    let job: { kind: string; spec: unknown };
    try {
      job = this.spec();
    } catch (e) {
      this.host.log((e as Error).message, "warn");
      return;
    }
    const tab = this.tab;
    this.running = true;
    const runBtn = this.q<HTMLButtonElement>(".an-run");
    const cancel = this.q<HTMLButtonElement>(".an-cancel");
    const bar = this.q(".an-progress");
    const fill = bar.querySelector<HTMLElement>(".fill")!;
    const label = bar.querySelector<HTMLElement>(".label")!;
    runBtn.disabled = true;
    cancel.disabled = false;
    bar.classList.remove("hidden");
    fill.style.width = "0%";
    label.textContent = "starting…";
    const unit = job.kind === "calibrate" || job.kind === "optimize" ? "iterations" : "runs";
    const t0 = performance.now();
    this.host.log(`${tab} of ${model}…`);
    try {
      const r = await api.analysisRun<Record<string, unknown>>(model, job.kind, job.spec, this.host.settings(), (p) => {
        fill.style.width = `${p.total ? (100 * p.done) / p.total : 0}%`;
        label.textContent = `${p.done} / ${p.total} ${unit}`;
      });
      if ((r as { cancelled?: boolean }).cancelled) {
        this.host.log(`${tab}: cancelled`, "warn");
        return;
      }
      this.results.set(tab, r);
      this.host.log(`${tab} of ${model} done in ${((performance.now() - t0) / 1000).toFixed(2)} s`, "ok");
      if (this.tab === tab) this.showResult();
    } catch (e) {
      this.host.log(`${tab}: ${(e as Error).message ?? e}`, "err");
    } finally {
      this.running = false;
      runBtn.disabled = false;
      cancel.disabled = true;
      bar.classList.add("hidden");
    }
  }

  // ------------------------------------------------------------------ results
  private showResult() {
    const r = this.results.get(this.tab);
    const report = this.q(".an-report");
    const barsHost = this.q(".an-bars");
    this.plot.subplots = [{ series: [] }];
    this.plot.active = 0;
    report.innerHTML = "";
    barsHost.classList.add("hidden");
    this.q(".an-plot").classList.remove("hidden");
    if (r) {
      if (this.tab === "sensitivity") this.showSens(r as SensResult);
      else if (this.tab === "uncertainty") this.showGsa(r as GsaResult);
      else if (this.tab === "calibrate") this.showCalib(r as CalibResult);
      else this.showOpt(r as OptResult);
    }
    this.plot.draw();
    if (!barsHost.classList.contains("hidden")) this.bars.draw();
  }

  private colors = ["#0b5fff", "#d9480f", "#2b8a3e", "#ae3ec9", "#e8590c", "#1098ad", "#c2255c", "#5c940d", "#364fc7", "#862e9c"];
  private line(name: string, x: Float64Array, y: Float64Array, i: number, extra: Partial<Series> = {}): Series {
    return { name, unit: "", x, y, color: this.colors[i % this.colors.length], ...extra };
  }

  private showSens(r: SensResult) {
    const t = f64(r.time);
    const mode = this.get("s.view", "normalized");
    this.plot.subplots = r.outputs.map((o, k) => ({
      series: r.params.map((p, j) => this.line(mode === "normalized" ? `${o}: (${p}/${o}) d${o}/d${p}` : `d${o}/d${p}`, t, f64((mode === "normalized" ? r.normalized : r.dydp)[k][j]), j)),
    }));
    this.q(".an-bars").classList.remove("hidden");
    this.bars.set(
      "Parameter ranking: time average of |normalized sensitivity|",
      r.outputs,
      r.ranking.map((e) => ({ label: e.param, values: e.per_output })),
    );
    const last = r.time.length - 1;
    this.q(".an-report").innerHTML = `<div class="an-rhead"><b>${r.method === "Exact" || r.method === "exact" ? "Exact forward sensitivities" : "Central finite differences"}</b>
      <label>Show <select class="an-view"><option value="normalized"${mode === "normalized" ? " selected" : ""}>normalized p/y·dy/dp</option><option value="raw"${mode === "raw" ? " selected" : ""}>dy/dp</option></select></label></div>
      ${r.notes.map((n) => `<div class="muted small">${esc(n)}</div>`).join("")}
      <table class="an-table"><tr><th>rank</th><th>parameter</th><th>value</th><th>score</th>${r.outputs.map((o) => `<th>d${esc(o)}/dp at t = ${fmt(r.time[last])}</th>`).join("")}</tr>
      ${r.ranking
        .map((e, i) => {
          const j = r.params.indexOf(e.param);
          return `<tr><td>${i + 1}</td><td>${esc(e.param)}</td><td class="num">${fmt(r.param_values[j])}</td><td class="num">${fmt(e.score)}</td>${r.outputs.map((_, k) => `<td class="num">${fmt(r.dydp[k][j][last])}</td>`).join("")}</tr>`;
        })
        .join("")}</table>`;
    const v = this.q<HTMLSelectElement>(".an-view");
    v.onchange = () => {
      this.stored.set("s.view", v.value);
      this.showResult();
    };
  }

  private showGsa(r: GsaResult) {
    const t = f64(r.time);
    const rep: string[] = [`<div class="an-rhead"><b>${r.mode === "sobol" ? "Sobol indices" : r.mode === "morris" ? "Morris elementary effects" : "Monte Carlo"}</b> <span class="muted">${r.runs} runs, ${r.failed} failed</span></div>`];
    for (const e of r.errors) rep.push(`<div class="muted small">${esc(e)}</div>`);
    if (r.mode === "montecarlo") {
      this.plot.subplots = r.bands.map((b, i) => ({
        series: [
          this.line(`${b.name} mean (band: 5–95 %)`, t, f64(b.mean), i, { band: { lo: f64(b.q05), hi: f64(b.q95) } }),
          this.line(`${b.name} median`, t, f64(b.q50), i, { dash: [4, 3] }),
        ],
      }));
    }
    if (r.qois.length)
      rep.push(
        `<table class="an-table"><tr><th>quantity</th><th>mean</th><th>std</th><th>5 %</th><th>50 %</th><th>95 %</th></tr>${r.qois
          .map((q) => `<tr><td>${esc(q.name)}</td>${[q.mean, q.std, q.q05, q.q50, q.q95].map((v) => `<td class="num">${fmt(v)}</td>`).join("")}</tr>`)
          .join("")}</table>`,
      );
    const qi = Math.min(parseInt(this.get("u.qi", "0"), 10) || 0, Math.max(0, (r.mode === "sobol" ? r.sobol : r.morris).length - 1));
    if (r.mode === "sobol" && r.sobol.length) {
      const s = r.sobol[qi];
      this.q(".an-bars").classList.remove("hidden");
      this.bars.set(`Sobol indices of ${s.qoi} (95 % bootstrap intervals)`, ["first order S1", "total ST"], r.params.map((p, i) => ({ label: p, values: [s.s1[i] ?? NaN, s.st[i] ?? NaN], errors: [s.s1_conf[i], s.st_conf[i]] })));
      rep.push(
        `<table class="an-table"><tr><th>parameter</th>${r.sobol.map((x) => `<th>S1 ${esc(x.qoi)}</th><th>ST</th>`).join("")}</tr>${r.params
          .map((p, i) => `<tr><td>${esc(p)}</td>${r.sobol.map((x) => `<td class="num">${fmt(x.s1[i], 3)} ± ${fmt(x.s1_conf[i], 2)}</td><td class="num">${fmt(x.st[i], 3)} ± ${fmt(x.st_conf[i], 2)}</td>`).join("")}</tr>`)
          .join("")}</table>`,
      );
    }
    if (r.mode === "morris" && r.morris.length) {
      const m = r.morris[qi];
      this.q(".an-bars").classList.remove("hidden");
      this.bars.set(`Morris screening of ${m.qoi} (per unit parameter range)`, ["μ*", "σ"], r.params.map((p, i) => ({ label: p, values: [m.mu_star[i] ?? NaN, m.sigma[i] ?? NaN] })));
      rep.push(
        `<table class="an-table"><tr><th>parameter</th>${r.morris.map((x) => `<th>μ* ${esc(x.qoi)}</th><th>μ</th><th>σ</th>`).join("")}</tr>${r.params
          .map((p, i) => `<tr><td>${esc(p)}</td>${r.morris.map((x) => `<td class="num">${fmt(x.mu_star[i])}</td><td class="num">${fmt(x.mu[i])}</td><td class="num">${fmt(x.sigma[i])}</td>`).join("")}</tr>`)
          .join("")}</table>`,
      );
    }
    const list = r.mode === "sobol" ? r.sobol.map((s) => s.qoi) : r.mode === "morris" ? r.morris.map((s) => s.qoi) : [];
    if (list.length > 1) rep.splice(1, 0, `<label>Chart <select class="an-qi">${list.map((q, i) => `<option value="${i}"${i === qi ? " selected" : ""}>${esc(q)}</option>`).join("")}</select></label>`);
    if (r.mode !== "montecarlo") this.plot.emptyText = "Bands are drawn for Monte Carlo runs";
    else this.plot.emptyText = "Choose parameters and outputs, then Run";
    this.q(".an-report").innerHTML = rep.join("");
    const sel = this.pane.querySelector<HTMLSelectElement>(".an-qi");
    if (sel)
      sel.onchange = () => {
        this.stored.set("u.qi", sel.value);
        this.showResult();
      };
    this.q(".an-plot").classList.toggle("hidden", r.mode !== "montecarlo");
  }

  private showCalib(r: CalibResult) {
    const t = f64(r.time);
    this.plot.subplots = r.columns.map((c, i) => ({
      series: [
        this.line(`${c.column} measured`, f64(c.t), f64(c.measured), 2 * i + 1, { points: true }),
        this.line(`${c.var} before`, t, f64(c.before), 2 * i, { dash: [5, 4] }),
        this.line(`${c.var} fitted`, t, f64(c.after), 2 * i),
      ],
    }));
    const corr = r.params.length > 1 ? `<table class="an-table"><tr><th>correlation</th>${r.params.map((p) => `<th>${esc(p.name)}</th>`).join("")}</tr>${r.params.map((p, i) => `<tr><td>${esc(p.name)}</td>${r.correlation[i].map((v) => `<td class="num">${fmt(v, 3)}</td>`).join("")}</tr>`).join("")}</table>` : "";
    this.q(".an-report").innerHTML = `<div class="an-rhead"><b>${r.converged ? "Converged" : "Stopped"}</b> <span class="muted">${esc(r.message)}; ${r.iterations} iterations, sensitivities ${esc(String(r.method).toLowerCase())}; weighted RMS ${fmt(r.rms_before)} → ${fmt(r.rms_after)}</span></div>
      <table class="an-table"><tr><th>parameter</th><th>initial</th><th>fitted</th><th>std. error</th><th></th></tr>${r.params
        .map((p) => `<tr><td>${esc(p.name)}</td><td class="num">${fmt(p.initial, 6)}</td><td class="num"><b>${fmt(p.value, 6)}</b></td><td class="num">${fmt(p.std_error, 3)}</td><td>${p.at_bound ? "at bound" : ""}</td></tr>`)
        .join("")}</table>
      <table class="an-table"><tr><th>column</th><th>weight</th><th>RMS before</th><th>RMS after</th></tr>${r.columns.map((c) => `<tr><td>${esc(c.column)} → ${esc(c.var)}</td><td class="num">${fmt(c.weight, 3)}</td><td class="num">${fmt(c.rms_before)}</td><td class="num">${fmt(c.rms_after)}</td></tr>`).join("")}</table>
      ${corr}
      ${this.modifierBox(r.modifier, r.params.map((p) => [p.name, p.value]))}`;
    this.wireModifier(r.modifier, r.params.map((p) => [p.name, p.value]));
  }

  private showOpt(r: OptResult) {
    const h = f64(r.history);
    this.plot.subplots = [{ series: [this.line("objective (best of the simplex)", Float64Array.from(h, (_, i) => i), h, 0)] }];
    this.q(".an-report").innerHTML = `<div class="an-rhead"><b>${esc(r.message)}</b> <span class="muted">${r.iterations} iterations, ${r.runs} runs; objective ${fmt(r.objective_before)} → ${fmt(r.objective_after)}</span></div>
      <table class="an-table"><tr><th>parameter</th><th>initial</th><th>optimal</th></tr>${r.params.map((p, i) => `<tr><td>${esc(p)}</td><td class="num">${fmt(r.initial[i], 6)}</td><td class="num"><b>${fmt(r.values[i], 6)}</b></td></tr>`).join("")}</table>
      <table class="an-table"><tr><th>quantity</th><th>before</th><th>after</th></tr>${r.qois.map((q, i) => `<tr><td>${esc(q)}</td><td class="num">${fmt(r.qoi_before[i])}</td><td class="num">${fmt(r.qoi_after[i])}</td></tr>`).join("")}</table>
      ${this.modifierBox(r.modifier, r.params.map((p, i) => [p, r.values[i]]))}`;
    this.wireModifier(r.modifier, r.params.map((p, i) => [p, r.values[i]]));
  }

  private modifierBox(mod: string, _p: [string, number][]): string {
    return `<div class="an-mod"><code>${esc(mod)}</code><button class="an-copy">Copy as modifier</button><button class="an-apply">Use these values</button></div>`;
  }

  private wireModifier(mod: string, p: [string, number][]) {
    this.q<HTMLButtonElement>(".an-copy").onclick = async () => {
      try {
        await navigator.clipboard.writeText(mod);
        this.host.log(`Copied ${mod}`, "ok");
      } catch {
        this.host.log(`Modifier: ${mod}`);
      }
    };
    this.q<HTMLButtonElement>(".an-apply").onclick = () => this.host.applyParams(p);
  }
}
