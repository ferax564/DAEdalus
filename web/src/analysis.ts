// Analysis panel: local sensitivities and adjoint gradients, uncertainty (Monte Carlo, Sobol,
// Morris), calibration against measured data, design optimization and design of experiments.
// Every analysis runs in the worker in slices (see worker.ts "analysisRun") so progress shows
// and Cancel takes effect between runs; the runs of a design of experiments fan out over the
// worker pool (pool.ts), which evaluates the quantities of interest in each worker.
import "./analysis.css";
import { api } from "./rpc";
import { CancelledError, pool } from "./pool";
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

type Tab = "sensitivity" | "uncertainty" | "calibrate" | "optimize" | "doe";
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
interface GradResult {
  method: string;
  objective: string;
  value: Num;
  qois: string[];
  qoi_values: Num[];
  params: string[];
  param_values: number[];
  gradient: Num[];
  normalized: Num[];
  ranking: number[];
  notes: string[];
  stats: { states: number; params: number; nodes: number; steps: number; tangent_passes: number; reverse_passes: number; forward_ms: number; backward_ms: number } | null;
  report: string;
}
interface DoePlan {
  design: string;
  factors: string[];
  coded: number[][];
  values: number[][];
  notes: string[];
}
interface Surface {
  qoi: string;
  terms: string[];
  coef: Num[];
  std_err: Num[];
  t: Num[];
  r2: Num;
  adj_r2: Num;
  rmse: Num;
  n: number;
  p: number;
  dropped: string[];
  predicted: Num[];
}
interface DoeResult {
  design: string;
  factors: string[];
  lo: number[];
  hi: number[];
  qois: string[];
  coded: number[][];
  values: number[][];
  responses: Num[][];
  runs: number;
  failed: number;
  errors: string[];
  surfaces: Surface[];
  main_effects: { factor: string; levels: Num[]; means: Num[] }[][];
  interactions: { a: string; b: string; a_levels: Num[]; b_levels: Num[]; means: Num[][] }[][];
  pareto: { qois: [number, number]; senses: [string, string]; front: number[] } | null;
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

interface DoeSpecJs {
  factors: { name: string; lo: number; hi: number; levels: number }[];
  design: string;
  qois: string[];
  [k: string]: unknown;
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
        <button data-tab="doe" title="Design of experiments: factorial, central composite, Box–Behnken and Latin hypercube designs, response surfaces and Pareto fronts">DOE</button>
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
    this.q<HTMLButtonElement>(".an-cancel").onclick = () => {
      if (this.doeRunning) {
        this.doeCancelled = true;
        pool().cancel();
      } else void api.analysisCancel();
    };
  }

  setVisible(on: boolean) {
    this.pane.classList.toggle("hidden", !on);
    if (on) {
      this.refresh();
      requestAnimationFrame(() => this.showResult());
    }
  }

  /** Redraw analysis canvases after a theme change. */
  redraw() {
    this.plot.draw();
    this.bars.draw();
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
      const method = this.get("s.method", "auto");
      const adj = method === "adjoint";
      const sensHelp = "exact: forward sensitivity equations with exact tape derivatives (models without events); fd: central differences, two runs per parameter; adjoint: the gradient of one scalar objective.";
      h = `<div class="an-h">Sensitivity</div>
        <label class="an-field">Method <select data-k="s.method" title="${esc(sensHelp)}">${[
          ["auto", "auto"],
          ["exact", "exact (forward)"],
          ["fd", "fd"],
          ["adjoint", "adjoint (scalar objective)"],
        ]
          .map(([v, l]) => `<option value="${v}"${method === v ? " selected" : ""}>${l}</option>`)
          .join("")}</select></label>
        ${
          adj
            ? `${this.field("s.objective", "Objective", this.get("s.objective", `integral(${this.defaultOutputs()[0] ?? "x"})`), 'class="an-objective"')}
        <p class="muted small">The gradient dJ/dp of one scalar J for all ticked parameters from one backward adjoint solve (reverse-mode tape passes), whatever their number. J: final(x), integral(x), mean(x), at(x, t), max(x), min(x), optionally with a window: integral(x, t0, t1). Models with events fall back to forward sensitivities.</p>`
            : `${this.field("s.outputs", "Outputs", this.get("s.outputs", outputsDefault), 'list="an-vars"')}`
        }
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
        <label class="an-field">Optimizer <select data-k="c.opt">${[
          ["lm", "Levenberg–Marquardt"],
          ["lbfgsb", "L-BFGS-B (adjoint gradients)"],
        ]
          .map(([v, l]) => `<option value="${v}"${this.get("c.opt", "lm") === v ? " selected" : ""}>${l}</option>`)
          .join("")}</select></label>
        ${this.field("c.iter", "Max iterations", this.get("c.iter", "40"), 'type="number" min="1"')}
        <div class="an-h">Tuners</div>${sel.length ? this.boundsTable(sel) : none}`;
    } else if (this.tab === "doe") {
      const design = this.get("d.design", "ccd");
      const qdef = this.defaultOutputs()
        .slice(0, 2)
        .map((o) => `max(${o})`)
        .join(", ");
      h = `<div class="an-h">Design of experiments</div>
        <label class="an-field">Design <select data-k="d.design">${[
          ["ccd", "central composite"],
          ["bbd", "Box–Behnken"],
          ["full", "full factorial"],
          ["frac", "fractional factorial"],
          ["lhs", "Latin hypercube"],
        ]
          .map(([v, l]) => `<option value="${v}"${design === v ? " selected" : ""}>${l}</option>`)
          .join("")}</select></label>
        ${design === "full" ? this.field("d.levels", "Levels", this.get("d.levels", "3"), 'type="number" min="2"') : ""}
        ${design === "lhs" ? this.field("d.samples", "Samples", this.get("d.samples", String(Math.max(10, 10 * sel.length))), 'type="number" min="2"') + this.field("d.seed", "Seed", this.get("d.seed", "12345"), 'type="number"') : ""}
        ${design === "ccd" ? `<label class="an-field">Axial α <select data-k="d.alpha">${["face", "rotatable", "orthogonal"].map((m) => `<option${this.get("d.alpha", "face") === m ? " selected" : ""}>${m}</option>`).join("")}</select></label>` : ""}
        ${design === "frac" ? `<label class="an-field">Resolution <select data-k="d.res">${["3", "4", "5"].map((m) => `<option value="${m}"${this.get("d.res", "4") === m ? " selected" : ""}>${["III", "IV", "V"][+m - 3]}</option>`).join("")}</select></label>` : ""}
        ${this.field("d.qois", "Quantities", this.get("d.qois", qdef))}
        <div class="an-row"><label class="an-field">Pareto <select data-k="d.s0">${["min", "max"].map((m) => `<option${this.get("d.s0", "min") === m ? " selected" : ""}>${m}</option>`).join("")}</select></label><label class="an-field">× <select data-k="d.s1">${["min", "max"].map((m) => `<option${this.get("d.s1", "min") === m ? " selected" : ""}>${m}</option>`).join("")}</select></label></div>
        <p class="muted small">${this.doeCount(design, sel.length)}. Runs go to ${pool().size} parallel workers. Fits a quadratic response surface per quantity (coded units), main effects, two-factor interactions and the Pareto front of the first two quantities.</p>
        <div class="an-h">Factors</div>${sel.length ? this.boundsTable(sel) : none}`;
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
        <label class="an-field">Algorithm <select data-k="o.alg">${[
          ["neldermead", "Nelder–Mead (no derivatives)"],
          ["lbfgsb", "L-BFGS-B (adjoint gradients)"],
        ]
          .map(([v, l]) => `<option value="${v}"${this.get("o.alg", "neldermead") === v ? " selected" : ""}>${l}</option>`)
          .join("")}</select></label>
        ${this.field("o.iter", "Max iterations", this.get("o.iter", "200"), 'type="number" min="1"')}
        <p class="muted small">The objective is the sum of the terms (target terms are squared errors) over the bounded parameters. Nelder–Mead needs no derivatives and handles events; L-BFGS-B takes one gradient per iteration from an adjoint solve (forward sensitivities at events).</p>
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
        if (k === "u.mode" || k === "s.method" || k === "d.design") this.renderConfig();
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
        if (this.get("s.method", "auto") === "adjoint") {
          const q = this.get("s.objective", `integral(${this.defaultOutputs()[0] ?? "x"})`).trim();
          if (!q) throw new Error("Give the objective, such as integral(x).");
          return { kind: "adjoint", spec: { params: sel, objective: [{ qoi: q, weight: 1, target: null }], method: "auto" } };
        }
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
          spec: { tuners: bounds(), csv: this.csv.text, time_column: this.csv.header[0], map, window, method: this.get("c.method", "auto"), max_iter: parseInt(this.get("c.iter", "40"), 10) || 40, optimizer: this.get("c.opt", "lm") },
        };
      }
      case "optimize": {
        if (!this.objectives.length) throw new Error("Add an objective term.");
        const objective = this.objectives.map((o) => ({ qoi: o.qoi, weight: o.goal === "max" ? -1 : 1, target: o.goal === "target" ? num(o.target, `target of ${o.qoi}`) : null }));
        return { kind: "optimize", spec: { tuners: bounds(), objective, max_iter: parseInt(this.get("o.iter", "200"), 10) || 200, tol: 1e-8, algorithm: this.get("o.alg", "neldermead") } };
      }
      case "doe": {
        const design = this.get("d.design", "ccd");
        const levels = design === "full" ? parseInt(this.get("d.levels", "3"), 10) || 2 : 0;
        const factors = bounds().map((b) => {
          if (b.lo === null || b.hi === null || !(b.hi > b.lo)) throw new Error(`Give ${b.name} a range with lower < upper.`);
          return { name: b.name, lo: b.lo, hi: b.hi, levels };
        });
        const qois = this.csvList("d.qois", this.defaultOutputs().slice(0, 2).map((o) => `max(${o})`).join(","));
        if (!qois.length) throw new Error("Give at least one quantity, such as max(x).");
        return {
          kind: "doe",
          spec: {
            factors,
            design,
            qois,
            samples: design === "lhs" ? parseInt(this.get("d.samples", String(Math.max(10, 10 * sel.length))), 10) || 0 : 0,
            seed: parseInt(this.get("d.seed", "12345"), 10) || 0,
            alpha: design === "ccd" ? this.get("d.alpha", "face") : null,
            resolution: design === "frac" ? parseInt(this.get("d.res", "4"), 10) : null,
            pareto: [this.get("d.s0", "min"), this.get("d.s1", "min")],
          },
        };
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
    const progress = (p: { done: number; total: number }) => {
      fill.style.width = `${p.total ? (100 * p.done) / p.total : 0}%`;
      label.textContent = `${p.done} / ${p.total} ${unit}`;
    };
    try {
      const r =
        job.kind === "doe"
          ? await this.runDoe(model, job.spec as DoeSpecJs, progress)
          : await api.analysisRun<Record<string, unknown>>(model, job.kind, job.spec, this.host.settings(), progress);
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
      if (this.tab === "sensitivity" && "gradient" in (r as object)) this.showGrad(r as GradResult);
      else if (this.tab === "doe") this.showDoe(r as DoeResult);
      else if (this.tab === "sensitivity") this.showSens(r as SensResult);
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

  // ------------------------------------------------------------------ adjoint gradients
  private showGrad(r: GradResult) {
    this.q(".an-plot").classList.add("hidden");
    this.q(".an-bars").classList.remove("hidden");
    const order = r.ranking.length ? r.ranking : r.params.map((_, i) => i);
    this.bars.set(
      `Gradient of ${r.objective}: normalized p/J·dJ/dp`,
      ["p/J dJ/dp"],
      order.map((i) => ({ label: r.params[i], values: [r.normalized[i] ?? NaN] })),
    );
    const how = r.method === "adjoint" ? "Adjoint gradient" : r.method === "forward" ? "Forward sensitivities" : r.method === "fd" ? "Central differences" : r.method;
    const st = r.stats;
    this.q(".an-report").innerHTML = `<div class="an-rhead"><b>${esc(how)}</b> <span class="muted">J = ${esc(r.objective)} = ${fmt(r.value, 8)}${
      st ? `; ${st.steps} backward steps, ${st.reverse_passes} reverse + ${st.tangent_passes} tangent tape passes, forward ${fmt(st.forward_ms, 3)} ms, backward ${fmt(st.backward_ms, 3)} ms` : ""
    }</span></div>
      ${r.notes.map((n) => `<div class="muted small">${esc(n)}</div>`).join("")}
      <table class="an-table an-grad"><tr><th>rank</th><th>parameter</th><th>value</th><th>dJ/dp</th><th>p/J dJ/dp</th></tr>${order
        .map((i, k) => `<tr><td>${k + 1}</td><td>${esc(r.params[i])}</td><td class="num">${fmt(r.param_values[i])}</td><td class="num">${fmt(r.gradient[i], 7)}</td><td class="num">${fmt(r.normalized[i], 4)}</td></tr>`)
        .join("")}</table>`;
  }

  // ------------------------------------------------------------------ design of experiments
  private doeRunning = false;
  private doeCancelled = false;

  private doeCount(design: string, k: number): string {
    if (k === 0) return "Tick the factors (parameters) above";
    const levels = parseInt(this.get("d.levels", "3"), 10) || 2;
    const n =
      design === "full"
        ? Math.pow(levels, k)
        : design === "ccd"
          ? (k >= 5 ? Math.pow(2, k - (k >= 8 ? 2 : 1)) : Math.pow(2, k)) + 2 * k + 1
          : design === "bbd"
            ? k >= 3
              ? 2 * k * (k - 1) + 1
              : NaN
            : design === "lhs"
              ? parseInt(this.get("d.samples", String(Math.max(10, 10 * k))), 10)
              : NaN;
    return isFinite(n) ? `${n} runs for ${k} factor${k > 1 ? "s" : ""}` : design === "bbd" ? "Box–Behnken needs 3 or more factors" : `${k} factor${k > 1 ? "s" : ""}`;
  }

  /** Plan the design, run it on the worker pool and fit the surfaces. */
  private async runDoe(model: string, spec: DoeSpecJs, progress: (p: { done: number; total: number }) => void): Promise<Record<string, unknown>> {
    const plan = await api.doePlan<DoePlan | { error: string }>(spec);
    if ("error" in plan) throw new Error(plan.error);
    const n = plan.values.length;
    const settings = this.host.settings();
    const base = (settings.params ?? []).filter(([p]) => !plan.factors.includes(p));
    const responses: ((number | null)[] | null)[] = new Array(n).fill(null);
    const errors: string[] = [];
    let done = 0;
    this.doeRunning = true;
    this.doeCancelled = false;
    progress({ done: 0, total: n });
    const p = pool();
    try {
      await Promise.all(
        plan.values.map(async (vals, i) => {
          const params: [string, number][] = [...base, ...plan.factors.map((f, j) => [f, vals[j]] as [string, number])];
          try {
            const out = await p.run(model, { ...settings, params, profile: false }, [], spec.qois);
            if (!out.summary.ok) {
              if (errors.length < 5) errors.push(`run ${i + 1}: ${out.summary.error}`);
            } else responses[i] = out.qoi ?? null;
          } catch (e) {
            if (e instanceof CancelledError) return;
            if (errors.length < 5) errors.push(`run ${i + 1}: ${(e as Error).message}`);
          }
          progress({ done: ++done, total: n });
        }),
      );
    } finally {
      this.doeRunning = false;
    }
    if (this.doeCancelled) return { cancelled: true };
    const r = await api.doeAnalyze<Record<string, unknown>>(model, spec, plan, responses, errors);
    if (typeof r.error === "string") throw new Error(r.error);
    return r;
  }

  private showDoe(r: DoeResult) {
    const nq = r.qois.length;
    const qi = Math.min(parseInt(this.get("d.qi", "0"), 10) || 0, Math.max(0, nq - 1));
    const pairs = r.interactions[qi] ?? [];
    const views: [string, string][] = [
      ["effects", "Main effects"],
      ["interactions", "Interactions"],
      ["fit", "Predicted vs simulated"],
    ];
    if (r.pareto) views.push(["pareto", "Pareto front"]);
    let view = this.get("d.view", "effects");
    if (!views.some(([v]) => v === view)) view = "effects";
    const pi = Math.min(parseInt(this.get("d.pair", "0"), 10) || 0, Math.max(0, pairs.length - 1));
    const s = r.surfaces[qi];
    const y = r.responses[qi] ?? [];
    if (view === "effects") {
      this.plot.subplots = (r.main_effects[qi] ?? []).map((m, j) => ({
        xName: m.factor,
        series: [this.line(`mean ${r.qois[qi]} vs ${m.factor}`, f64(m.levels), f64(m.means), j), this.line(`${m.factor} levels`, f64(m.levels), f64(m.means), j, { points: true })],
      }));
    } else if (view === "interactions" && pairs.length) {
      const it = pairs[pi];
      const x = f64(it.a_levels);
      this.plot.subplots = [
        {
          xName: it.a,
          series: [
            this.line(`${it.b} low (${fmt(it.b_levels[0], 4)})`, x, f64(it.means[0]), 0),
            this.line(`${it.b} high (${fmt(it.b_levels[1], 4)})`, x, f64(it.means[1]), 1),
            this.line(`${it.b} low`, x, f64(it.means[0]), 0, { points: true }),
            this.line(`${it.b} high`, x, f64(it.means[1]), 1, { points: true }),
          ],
        },
      ];
    } else if (view === "fit" && s) {
      const ok = y.map((v, i) => (v !== null && s.predicted[i] !== null ? i : -1)).filter((i) => i >= 0);
      const yy = ok.map((i) => y[i] as number);
      const lo = Math.min(...yy);
      const hi = Math.max(...yy);
      this.plot.subplots = [
        {
          xName: `simulated ${r.qois[qi]}`,
          series: [
            this.line("y = x", Float64Array.from([lo, hi]), Float64Array.from([lo, hi]), 3, { dash: [4, 3] }),
            this.line(`surface prediction (R² = ${fmt(s.r2, 5)})`, f64(ok.map((i) => y[i])), f64(ok.map((i) => s.predicted[i])), 0, { points: true }),
          ],
        },
      ];
    } else if (view === "pareto" && r.pareto) {
      const [a, b] = r.pareto.qois;
      const fa = r.responses[a];
      const fb = r.responses[b];
      const fr = r.pareto.front;
      this.plot.subplots = [
        {
          xName: `${r.qois[a]} (${r.pareto.senses[0]})`,
          series: [
            this.line(`runs: ${r.qois[b]} (${r.pareto.senses[1]})`, f64(fa), f64(fb), 7, { points: true }),
            this.line("Pareto front", f64(fr.map((i) => fa[i])), f64(fr.map((i) => fb[i])), 1),
            this.line("non-dominated runs", f64(fr.map((i) => fa[i])), f64(fr.map((i) => fb[i])), 1, { points: true }),
          ],
        },
      ];
    }
    if (s) {
      this.q(".an-bars").classList.remove("hidden");
      const terms = s.terms.map((t, i) => ({ t, i })).filter((x) => x.t !== "1");
      this.bars.set(
        `Response surface of ${s.qoi} (coded units): R² = ${fmt(s.r2, 5)}, adj. R² = ${fmt(s.adj_r2, 5)}`,
        ["coefficient (± 2 std. errors)"],
        terms.map(({ t, i }) => ({ label: t, values: [s.coef[i] ?? NaN], errors: [s.std_err[i] !== null && isFinite(s.std_err[i] as number) ? 2 * (s.std_err[i] as number) : null] })),
      );
    }
    const sel = (cls: string, opts: [string, string][], cur: string) => `<select class="${cls}">${opts.map(([v, l]) => `<option value="${v}"${v === cur ? " selected" : ""}>${esc(l)}</option>`).join("")}</select>`;
    const rep: string[] = [];
    rep.push(`<div class="an-rhead"><b>${esc(r.design)} design</b> <span class="muted">${r.runs} runs, ${r.failed} failed</span>
      <label>Quantity ${sel("an-dq", r.qois.map((q, i) => [String(i), q]), String(qi))}</label>
      <label>Plot ${sel("an-dview", views, view)}</label>
      ${view === "interactions" && pairs.length ? `<label>Pair ${sel("an-dpair", pairs.map((p, i) => [String(i), `${p.a} × ${p.b}`]), String(pi))}</label>` : ""}</div>`);
    for (const n of r.notes) rep.push(`<div class="muted small">${esc(n)}</div>`);
    for (const e of r.errors) rep.push(`<div class="muted small">${esc(e)}</div>`);
    if (s)
      rep.push(`<table class="an-table an-surface"><tr><th>term</th><th>coefficient</th><th>std. error</th><th>t</th></tr>${s.terms
        .map((t, i) => `<tr><td>${esc(t)}</td><td class="num">${fmt(s.coef[i], 6)}</td><td class="num">${fmt(s.std_err[i], 3)}</td><td class="num">${fmt(s.t[i], 3)}</td></tr>`)
        .join("")}</table>
        <div class="muted small">R² ${fmt(s.r2, 6)}, adjusted R² ${fmt(s.adj_r2, 6)}, RMSE ${fmt(s.rmse, 4)}; ${s.n} runs, ${s.p} terms${s.dropped.length ? `; not estimable: ${esc(s.dropped.join(", "))}` : ""}</div>`);
    if (r.pareto) {
      const [a, b] = r.pareto.qois;
      rep.push(`<div class="an-h">Pareto front: ${esc(r.qois[a])} (${r.pareto.senses[0]}) × ${esc(r.qois[b])} (${r.pareto.senses[1]})</div>
        <table class="an-table an-pareto"><tr><th>run</th>${r.factors.map((f) => `<th>${esc(f)}</th>`).join("")}<th>${esc(r.qois[a])}</th><th>${esc(r.qois[b])}</th><th></th></tr>${r.pareto.front
          .map(
            (i) =>
              `<tr><td>${i + 1}</td>${r.values[i].map((v) => `<td class="num">${fmt(v, 5)}</td>`).join("")}<td class="num">${fmt(r.responses[a][i])}</td><td class="num">${fmt(r.responses[b][i])}</td><td><button data-run="${i}">Use</button></td></tr>`,
          )
          .join("")}</table>`);
    }
    this.q(".an-report").innerHTML = rep.join("");
    const wire = (cls: string, key: string) => {
      const e = this.pane.querySelector<HTMLSelectElement>(`.${cls}`);
      if (e)
        e.onchange = () => {
          this.stored.set(key, e.value);
          this.showResult();
        };
    };
    wire("an-dq", "d.qi");
    wire("an-dview", "d.view");
    wire("an-dpair", "d.pair");
    this.pane.querySelectorAll<HTMLButtonElement>(".an-pareto button[data-run]").forEach((b) => {
      b.onclick = () => {
        const i = +b.dataset.run!;
        this.host.applyParams(r.factors.map((f, j) => [f, r.values[i][j]]));
      };
    });
    this.plot.emptyText = "No plot for this view";
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
