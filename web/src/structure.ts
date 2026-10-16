// Structure view: BLT blocks, incidence matrix, state selection with user overrides, and
// the state Jacobian (heatmap, eigenvalues, participation) at a chosen time point.
import { api } from "./rpc";
import { type EqTrace, type InitReport, type VarTrace, eqTraceHtml, initHtml, varTraceHtml } from "./debug";

// ---------------------------------------------------------------- report shapes (crates/daedalus-core/src/structure.rs)
interface BlockEq { eq: number; var: string | null }
interface BlockInfo { index: number; kind: "assign" | "linear" | "nonlinear"; size: number; torn: boolean; tears: string[]; inner: number; residuals: number; vars: string[]; eqs: BlockEq[] }
interface EqInfo { id: number; text: string; origin: string; differentiated: number; block: number | null }
interface VarInfo { name: string; role: string; block: number | null; state_select: string; model_state_select: string | null; unit: string; note: string | null }
interface Candidate { var: string; derivative: string; state_select: string; overridden: boolean; flags: string[]; chosen: "state" | "dummy" }
interface SetCheck { time: number; cond: number | null; best_cond: number | null; fails: boolean; best: string[] }
interface StateSet { index: number; level: number; eqs: number[]; candidates: Candidate[]; states: number; dummies: number; reason: string; state_dependent: boolean; at_start: SetCheck | null; worst: SetCheck | null }
interface Mode { re: number; im: number; time_constant: number | null; frequency: number | null; damping: number | null; participation: [number, number][] }
interface LoopJac { block: number; tears: string[]; size: number; cond: number | null; singular: boolean }
interface Jacobian {
  time: number; states: string[]; x0: number[]; method: string; entries: [number, number, number][]; nnz: number;
  cond: number | null; singular: boolean; modes: Mode[]; stiffness_ratio: number | null; fastest_rate: number | null; slowest_rate: number | null;
  horizon: number; suggested_solver: string; solver_reason: string; loops: LoopJac[]; notes: string[];
}
export interface StructureReport {
  ok: boolean; error?: string; ms?: number;
  model: string; states: string[]; state_reasons: { state: string; reason: string }[];
  variables: VarInfo[]; equations: EqInfo[]; blocks: BlockInfo[];
  incidence: { rows: number[]; cols: string[]; block_starts: number[]; entries: [number, number][] };
  state_sets: StateSet[]; overrides: { var: string; pattern: string; model: string; tier: string }[];
  state_change_eqs: number; warnings: string[]; jacobian: Jacobian | null; jacobian_error: string | null;
  stats: Record<string, number>;
}

export type Tier = "always" | "prefer" | "default" | "avoid" | "never";
const TIERS: Tier[] = ["always", "prefer", "default", "avoid", "never"];

// ---------------------------------------------------------------- overrides per model (UI session)
const KEY = "daedalus.stateOverrides";
const store = new Map<string, Map<string, Tier>>();
try {
  const saved = JSON.parse(sessionStorage.getItem(KEY) ?? "{}") as Record<string, [string, Tier][]>;
  for (const [m, list] of Object.entries(saved)) store.set(m, new Map(list));
} catch {
  /* no session storage */
}
function persist() {
  try {
    const o: Record<string, [string, Tier][]> = {};
    for (const [m, ov] of store) if (ov.size) o[m] = [...ov];
    sessionStorage.setItem(KEY, JSON.stringify(o));
  } catch {
    /* no session storage */
  }
}

/** The state-selection overrides of a model, as passed to simulate (`stateOverrides`). */
export function stateOverridesFor(model: string | undefined): [string, Tier][] {
  return model ? [...(store.get(model) ?? new Map())] : [];
}

/** `x(stateSelect = StateSelect.prefer)` for each override. */
export function asModifiers(list: [string, Tier][]): string {
  return list.map(([n, t]) => `${n}(stateSelect = StateSelect.${t})`).join(",\n");
}

// ---------------------------------------------------------------- helpers
const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
/** Number for display; `null` is "—" (not available). */
const fmt = (v: number | null | undefined, d = 3) => (v === null || v === undefined ? "—" : !isFinite(v) ? "∞" : Math.abs(v) >= 1e4 || (Math.abs(v) < 1e-3 && v !== 0) ? v.toExponential(d - 1) : String(+v.toPrecision(d + 1)));
/** A condition number: `null` (JSON of an infinite value) is "∞". */
const fmtCond = (v: number | null | undefined) => (v === null || v === undefined ? "∞" : fmt(v));
const css = (name: string) => getComputedStyle(document.documentElement).getPropertyValue(name).trim() || "#888";
const KIND_VAR: Record<string, string> = { assign: "--ok", linear: "--accent", nonlinear: "--warn" };
const kindColor = (k: string) => css(KIND_VAR[k] ?? "--ok");

function sizeCanvas(c: HTMLCanvasElement, w: number, h: number) {
  const dpr = window.devicePixelRatio || 1;
  c.width = Math.round(w * dpr);
  c.height = Math.round(h * dpr);
  c.style.width = `${w}px`;
  c.style.height = `${h}px`;
  const g = c.getContext("2d")!;
  g.setTransform(dpr, 0, 0, dpr, 0, 0);
  return g;
}

export interface StructureContext {
  /** Simulation settings of the toolbar (stop time, tolerance, parameter overrides). */
  settings(): { stop_time?: number; tolerance?: number; params?: [string, number][] };
  log(msg: string, cls?: string): void;
  /** Re-run the simulation (with the overrides). */
  simulate(): void;
  /** Show `file` at `line` (a source position of the debugger). */
  openLocation?(file: string, line: number): void;
}

type TraceState = { key: string; busy: boolean; var?: VarTrace; equation?: EqTrace; error?: string };

export class StructureView {
  private model = "";
  private report: StructureReport | null = null;
  private previous: StructureReport | null = null;
  private selectedBlock: number | null = null;
  private expandedSets = new Set<number>();
  private at: number | null = null;
  private busy = false;
  private gen = 0;
  private tip: HTMLDivElement;
  /** Debugger: the traced variable or equation, and the initialization report. */
  private trace: TraceState | null = null;
  private init: InitReport | null = null;
  private initBusy = false;
  private initAll = false;

  constructor(private host: HTMLElement, private ctx: StructureContext) {
    host.classList.add("structure");
    this.tip = document.createElement("div");
    this.tip.className = "st-tip hidden";
    document.body.appendChild(this.tip);
    // Debugger links anywhere in the view (delegated: the content is re-rendered often).
    host.addEventListener("click", (e) => {
      const t = e.target as HTMLElement;
      const v = t.closest<HTMLElement>("[data-trace-var]");
      if (v) {
        e.stopPropagation();
        void this.traceVar(v.dataset.traceVar!);
        return;
      }
      const q = t.closest<HTMLElement>("[data-trace-eq]");
      if (q) {
        e.stopPropagation();
        void this.traceEq(+q.dataset.traceEq!);
        return;
      }
      const b = t.closest<HTMLElement>("[data-goto-block]");
      if (b) {
        this.gotoBlock(+b.dataset.gotoBlock!);
        return;
      }
      const l = t.closest<HTMLElement>("[data-loc-file]");
      if (l && this.ctx.openLocation) {
        e.preventDefault();
        this.ctx.openLocation(l.dataset.locFile!, +l.dataset.locLine!);
        return;
      }
      if (t.closest(".dbg-run-init")) void this.runInit();
    });
    host.addEventListener("change", (e) => {
      const t = e.target as HTMLInputElement;
      if (t.classList.contains("dbg-showall")) {
        this.initAll = t.checked;
        this.renderInit();
      }
    });
  }

  /** Open the initialization debugger of `model` (from a failure in the messages); with
   * `simulation`, also examine the nonlinear solve that stopped the last run. */
  async showInit(model: string, simulation = false) {
    if (model !== this.model || !this.report) {
      if (model !== this.model) this.resetModel();
      this.model = model;
      const run = this.runInit(simulation);
      await this.analyse();
      await run;
    } else {
      this.render();
      await this.runInit(simulation);
    }
    this.host.querySelector(".dbg-init-card")?.scrollIntoView({ block: "start" });
  }

  private resetModel() {
    this.previous = null;
    this.selectedBlock = null;
    this.expandedSets.clear();
    this.trace = null;
    this.init = null;
    this.report = null;
  }

  private dbgOptions() {
    const s = this.ctx.settings();
    return { stop_time: s.stop_time, tolerance: s.tolerance, params: s.params, stateOverrides: [...this.overrides()] };
  }

  private async traceVar(name: string) {
    await this.runTrace(`var:${name}`, { var: name });
  }

  private async traceEq(id: number) {
    await this.runTrace(`eq:${id}`, { equation: id });
  }

  private async runTrace(key: string, opts: Record<string, unknown>) {
    this.trace = { key, busy: true };
    this.renderTrace();
    this.host.querySelector(".dbg-trace-card")?.scrollIntoView({ block: "nearest" });
    try {
      const r = (await api.debug(this.model, { ...this.dbgOptions(), ...opts })) as { ok: boolean; var?: VarTrace; equation?: EqTrace; error?: string };
      if (this.trace?.key !== key) return;
      this.trace = { key, busy: false, var: r.var, equation: r.equation, error: r.ok ? undefined : (r.error ?? "failed") };
    } catch (e) {
      this.trace = { key, busy: false, error: String(e) };
    }
    this.renderTrace();
  }

  private async runInit(simulation = false) {
    if (this.initBusy) return;
    this.initBusy = true;
    this.renderInit();
    const model = this.model;
    try {
      const r = (await api.debug(model, { ...this.dbgOptions(), init: true, simulation })) as InitReport;
      if (model !== this.model) return;
      this.init = r;
    } catch (e) {
      this.ctx.log(String(e), "err");
    } finally {
      this.initBusy = false;
      this.renderInit();
    }
  }

  private gotoBlock(b: number) {
    if (!this.report?.ok) return;
    this.selectedBlock = b;
    const list = this.host.querySelector(".st-blocks");
    if (!list) return;
    list.innerHTML = this.blocksHtml(this.report);
    this.wire();
    this.drawIncidence();
    list.querySelector(".st-block.open")?.scrollIntoView({ block: "nearest" });
  }

  private traceBody(): string {
    const t = this.trace;
    if (!t) return `<div class="muted">Click a variable or an equation (in the blocks, the state selection or the initialization table), or type a name, to see how it is computed: the equation as written (component, file:line), alias substitution, index reduction, state selection, solving and tearing.</div>`;
    if (t.busy) return `<div class="muted">Tracing ${esc(t.key.replace(/^(var|eq):/, ""))}…</div>`;
    if (t.error) return `<div class="st-warn">${esc(t.error)}</div>`;
    if (t.var) return varTraceHtml(t.var);
    if (t.equation) return eqTraceHtml(t.equation);
    return "";
  }

  private renderTrace() {
    const el = this.host.querySelector(".dbg-trace-body");
    if (el) el.innerHTML = this.traceBody();
  }

  private initBody(): string {
    if (this.initBusy) return `<div class="muted">Checking initialization…</div>`;
    if (!this.init) return `<div class="muted">Solves the initial equations and lists the unknowns of the initial system with the start values used (fixed, defaulted or iteration guess, and where they were set); on failure, the failing block's residuals and largest Newton updates; for a model that does not translate, its over- and underdetermined parts.</div>`;
    return initHtml(this.init, this.initAll);
  }

  private renderInit() {
    const el = this.host.querySelector(".dbg-init-body");
    if (el) el.innerHTML = this.initBody();
    const b = this.host.querySelector<HTMLButtonElement>(".dbg-run-init");
    if (b) {
      b.disabled = this.initBusy;
      b.textContent = this.init ? "Check again" : "Check initialization";
    }
  }

  private debuggerHtml(r: StructureReport | null): string {
    const names = r?.ok ? r.variables.filter((v) => v.role !== "parameter").slice(0, 4000) : [];
    return `<div class="st-grid dbg-grid">
      <section class="st-card dbg-trace-card">
        <h4>Transformation chain <span class="muted">(how a variable is computed)</span></h4>
        <form class="dbg-trace-form"><input class="dbg-trace-name" placeholder="variable, e.g. ${esc(names[0]?.name ?? "x")}" list="dbg-vars"/><button>Trace</button></form>
        <datalist id="dbg-vars">${names.map((v) => `<option value="${esc(v.name)}">`).join("")}</datalist>
        <div class="dbg-trace-body">${this.traceBody()}</div>
      </section>
      <section class="st-card dbg-init-card">
        <h4>Initialization <button class="dbg-run-init"${this.initBusy ? " disabled" : ""}>${this.init ? "Check again" : "Check initialization"}</button></h4>
        <div class="dbg-init-body">${this.initBody()}</div>
      </section>
    </div>`;
  }

  private wireDebugger() {
    const f = this.host.querySelector<HTMLFormElement>(".dbg-trace-form");
    if (f)
      f.onsubmit = (e) => {
        e.preventDefault();
        const n = this.host.querySelector<HTMLInputElement>(".dbg-trace-name")!.value.trim();
        if (n) void this.traceVar(n);
      };
  }

  /** Show the structure of `model` (analysing it unless already shown). */
  async show(model: string) {
    if (model !== this.model || !this.report) {
      if (model !== this.model) this.resetModel();
      this.model = model;
      await this.analyse();
    } else this.render();
  }

  private overrides(): Map<string, Tier> {
    let m = store.get(this.model);
    if (!m) store.set(this.model, (m = new Map()));
    return m;
  }

  private async analyse(keepPrevious = false) {
    const gen = ++this.gen;
    this.busy = true;
    if (!keepPrevious) this.previous = null;
    else if (this.report?.ok) this.previous = this.report;
    this.renderBusy();
    const s = this.ctx.settings();
    try {
      const r = (await api.structure(this.model, {
        jacobian: true,
        at: this.at ?? undefined,
        stop_time: s.stop_time,
        tolerance: s.tolerance,
        params: s.params,
        stateOverrides: [...this.overrides()],
      })) as StructureReport;
      if (gen !== this.gen) return;
      this.report = r;
      if (!r.ok) this.ctx.log(r.error ?? "structure analysis failed", "err");
      else {
        for (const w of r.warnings.filter((w) => w.includes("state"))) this.ctx.log(`warning: ${w}`, "warn");
        if (r.jacobian_error) this.ctx.log(`Jacobian analysis: ${r.jacobian_error}`, "err");
      }
    } catch (e) {
      this.ctx.log(String(e), "err");
    } finally {
      if (gen === this.gen) {
        this.busy = false;
        this.render();
      }
    }
  }

  private renderBusy() {
    const b = this.host.querySelector(".st-busy");
    if (b) b.classList.remove("hidden");
    else this.host.innerHTML = `<div class="st-busy">Translating ${esc(this.model)}…</div>`;
  }

  // ---------------------------------------------------------------- layout
  private render() {
    const r = this.report;
    const h = this.host;
    if (!r || !r.ok) {
      // The model does not translate: the initialization card carries the structural
      // diagnosis (over- and underdetermined parts, unconnected connectors).
      h.innerHTML = `<div class="st-warn">${esc(r?.error ?? "No report")}</div>${this.debuggerHtml(null)}`;
      this.wireDebugger();
      if (r && !this.init && !this.initBusy) void this.runInit();
      return;
    }
    const j = r.jacobian;
    const nb = (k: string) => r.blocks.filter((b) => b.kind === k).length;
    const ov = this.overrides();
    const prev = this.previous;
    const changed = prev && prev.states.join() !== r.states.join();
    h.innerHTML = `
      <div class="st-bar">
        <span class="st-busy-inline${this.busy ? "" : " hidden"}">analysing…</span>
        <label>Jacobian at <select class="st-at"><option value="start">start</option><option value="t"${this.at !== null ? " selected" : ""}>after simulating to</option></select></label>
        <input class="st-t" type="number" step="any" value="${this.at ?? ""}" placeholder="T" ${this.at === null ? "disabled" : ""}/>
        <button class="st-run">Analyse</button>
        <span class="spacer"></span>
        <span class="muted">${r.ms !== undefined ? `${r.ms.toFixed(0)} ms` : ""}</span>
      </div>
      <div class="st-summary">
        ${chip("States", String(r.states.length))}
        ${chip("Dummy derivatives", String(r.stats.dummy_derivatives ?? 0))}
        ${chip("Blocks", `${r.blocks.length} <small>(${nb("assign")} assign, ${nb("linear")} linear, ${nb("nonlinear")} nonlinear)</small>`)}
        ${j ? chip("Stiffness ratio", fmt(j.stiffness_ratio), prev?.jacobian ? `was ${fmt(prev.jacobian.stiffness_ratio)}` : "") : ""}
        ${j ? chip("cond₁(∂f/∂x)", j.singular ? "singular" : fmt(j.cond), prev?.jacobian ? `was ${prev.jacobian.singular ? "singular" : fmt(prev.jacobian.cond)}` : "") : ""}
        ${j ? chip("Suggested solver", j.suggested_solver === "stiff" ? "stiff (BDF)" : "non-stiff (Dopri5)", "", j.solver_reason) : ""}
      </div>
      ${changed ? `<div class="st-diff">States changed: <b>${esc(prev!.states.join(", "))}</b> → <b>${esc(r.states.join(", "))}</b>. Simulate to use them.</div>` : ""}
      ${r.warnings.filter((w) => /state/.test(w)).map((w) => `<div class="st-warn">${esc(w)}</div>`).join("")}
      ${r.jacobian_error ? `<div class="st-warn">Jacobian analysis failed: ${esc(r.jacobian_error)}</div>` : ""}
      ${this.debuggerHtml(r)}
      <div class="st-grid">
        <div class="st-col">
          <section class="st-card">
            <h4>State selection</h4>
            <div class="st-states">${r.state_reasons.map((s) => `<div><b><a class="dbg-var" data-trace-var="${esc(s.state)}">${esc(s.state)}</a></b> <span class="muted">${esc(s.reason)}</span></div>`).join("") || "<div class='muted'>No states.</div>"}</div>
            ${r.state_sets.map((s) => this.setHtml(s)).join("")}
            <div class="st-ov">
              <div class="st-ov-title">Overrides ${ov.size ? "" : "<span class='muted'>(none: pick a tier for a candidate above, or add any variable)</span>"}</div>
              ${[...ov].map(([n, t]) => `<div class="st-ov-row"><code>${esc(n)}</code> = ${t} <button data-rm="${esc(n)}" title="Remove">✕</button></div>`).join("")}
              <div class="st-ov-add"><input class="st-ov-name" placeholder="variable or glob" list="st-vars"/><select class="st-ov-tier">${TIERS.map((t) => `<option${t === "prefer" ? " selected" : ""}>${t}</option>`).join("")}</select><button class="st-ov-addbtn">Add</button></div>
              <datalist id="st-vars">${r.variables.filter((v) => v.role === "state" || v.role === "algebraic" || v.role === "alias" || v.role === "dummy derivative").slice(0, 2000).map((v) => `<option value="${esc(v.name)}">`).join("")}</datalist>
              <div class="st-ov-actions">
                <button class="st-copy" ${ov.size ? "" : "disabled"} title="Copy the equivalent stateSelect modifications">Copy as modifier</button>
                <button class="st-clear" ${ov.size ? "" : "disabled"}>Clear</button>
                <button class="st-sim primary" title="Simulate with these overrides">▶ Simulate with overrides</button>
              </div>
            </div>
          </section>
          <section class="st-card">
            <h4>Blocks <span class="muted">(BLT order; click for equations)</span></h4>
            <div class="st-blocks">${this.blocksHtml(r)}</div>
          </section>
        </div>
        <div class="st-col">
          <section class="st-card">
            <h4>Incidence <span class="muted">(${r.incidence.rows.length} × ${r.incidence.cols.length}, ${r.incidence.entries.length} nonzeros, BLT order)</span></h4>
            <canvas class="st-inc"></canvas>
          </section>
          <section class="st-card">
            <h4>State Jacobian ∂f/∂x ${j ? `<span class="muted">at t = ${fmt(j.time)} (${j.nnz} nonzeros)</span>` : ""}</h4>
            ${j && j.states.length ? `<canvas class="st-jac"></canvas><div class="st-legend"><span class="neg"></span> negative <span class="pos"></span> positive; shade: log₁₀|value|</div>` : "<div class='muted'>No states.</div>"}
          </section>
          <section class="st-card">
            <h4>Eigenvalues <span class="muted">${j ? `(${j.modes.length}; hover for time constant and participating states${prev?.jacobian ? "; hollow: before the last change" : ""})` : ""}</span></h4>
            ${j && j.modes.length ? `<canvas class="st-eig"></canvas>` : "<div class='muted'>None.</div>"}
            ${j ? this.loopsHtml(j) : ""}
            ${j ? j.notes.map((n) => `<div class="muted">${esc(n)}</div>`).join("") : ""}
          </section>
        </div>
      </div>`;
    this.wire();
    this.wireDebugger();
    requestAnimationFrame(() => this.drawAll());
  }

  private setHtml(s: StateSet): string {
    const ov = this.overrides();
    const check = (label: string, c: SetCheck | null) =>
      c ? `<span class="${c.fails ? "st-bad" : "muted"}">${label} t = ${fmt(c.time)}: cond ${fmtCond(c.cond)} (best ${fmtCond(c.best_cond)})${c.fails ? ` — fails; best dummies there: ${esc(c.best.join(", "))}` : ""}</span>` : "";
    // Long sets (multibody loops): the states and the first dummies, unless expanded.
    const long = s.candidates.length > 12 && !this.expandedSets.has(s.index);
    const shown = long ? s.candidates.filter((c, i) => c.chosen === "state" || i < 6 || ov.has(c.var)) : s.candidates;
    const more = long ? `<tr><td></td><td colspan="3"><button data-expand="${s.index}">show all ${s.candidates.length} candidates</button></td></tr>` : "";
    const rows = shown
      .map((c) => {
        const cur = ov.get(c.var);
        return `<tr class="${c.chosen}">
          <td>${c.chosen === "state" ? "● state" : "○ dummy"}</td>
          <td><code><a class="dbg-var" data-trace-var="${esc(c.var)}">${esc(c.var)}</a></code>${c.flags.length ? ` <span class="muted">(${c.flags.join(", ")})</span>` : ""}</td>
          <td>${esc(c.state_select)}${c.overridden ? " <span class='st-ovmark' title='overridden'>✎</span>" : ""}</td>
          <td><select data-var="${esc(c.var)}" title="Override stateSelect"><option value="">model</option>${TIERS.map((t) => `<option${cur === t ? " selected" : ""}>${t}</option>`).join("")}</select>
          <button data-pin="${esc(c.var)}" title="Pin as state (prefer)">📌</button><button data-forbid="${esc(c.var)}" title="Forbid as state (avoid)">⊘</button></td>
        </tr>`;
      })
      .join("") + more;
    return `<div class="st-set${s.worst?.fails ? " failing" : ""}">
      <div class="st-set-head">Set ${s.index} <span class="muted">level ${s.level}, ${s.eqs.length} equation(s), ${s.states} state(s) of ${s.candidates.length}${s.state_dependent ? ", state-dependent" : ""}</span></div>
      <table>${rows}</table>
      <div class="st-reason">${esc(s.reason)}</div>
      <div>${check("start", s.at_start)} ${s.worst && s.worst.time !== s.at_start?.time ? check("worst", s.worst) : ""}</div>
    </div>`;
  }

  private blocksHtml(r: StructureReport): string {
    const shown = r.blocks.length > 3000 ? r.blocks.filter((b) => b.kind !== "assign" || b.index === this.selectedBlock).slice(0, 3000) : r.blocks;
    const omitted = r.blocks.length - shown.length;
    return (
      shown
        .map((b) => {
          const head =
            b.kind === "assign"
              ? `<code>${esc(b.vars[0] ?? "?")}</code>`
              : `${b.kind} loop, ${b.size} unknown${b.size === 1 ? "" : "s"}${b.torn ? `, torn: ${b.tears.length} tear(s), ${b.inner} inner, ${b.residuals} residual(s)` : ""}`;
          const open = this.selectedBlock === b.index;
          const body = open
            ? `<div class="st-eqs">${b.eqs
                .map((e) => {
                  const q = r.equations[e.eq];
                  return `<div><span class="st-lhs">${e.var ? `<a class="dbg-var" data-trace-var="${esc(e.var)}" title="Trace ${esc(e.var)}">${esc(e.var)}</a> :=` : "residual"}</span> <code><a class="dbg-eqlink" data-trace-eq="${e.eq}" title="Show the transformations of this equation">${esc(q.text)}</a></code>${q.differentiated ? ` <span class="muted">(differentiated ${q.differentiated}×)</span>` : ""}<div class="muted st-origin">${esc(q.origin)}</div></div>`;
                })
                .join("")}${b.tears.length ? `<div class="muted">tears: ${esc(b.tears.join(", "))}</div>` : ""}</div>`
            : "";
          return `<div class="st-block${open ? " open" : ""}" data-block="${b.index}"><span class="st-sw" style="background:var(${KIND_VAR[b.kind] ?? "--muted"})"></span><span class="st-bi">${b.index}</span> ${head}${body}</div>`;
        })
        .join("") + (omitted ? `<div class="muted">${omitted} assignments not listed</div>` : "")
    );
  }

  private loopsHtml(j: Jacobian): string {
    if (!j.loops.length) return "";
    return `<h4>Algebraic loops <span class="muted">(Jacobian w.r.t. tear variables)</span></h4><table class="st-loops">${j.loops
      .map((l) => `<tr data-block="${l.block}"><td>block ${l.block}</td><td>${l.size} unknown${l.size === 1 ? "" : "s"}</td><td><code>${esc(l.tears.join(", "))}</code></td><td class="${l.singular || (l.cond ?? 0) > 1e8 ? "st-bad" : ""}">cond ${l.singular ? "singular" : fmtCond(l.cond)}</td></tr>`)
      .join("")}</table>`;
  }

  private wire() {
    const h = this.host;
    const q = <T extends HTMLElement>(s: string) => h.querySelector<T>(s)!;
    q<HTMLSelectElement>(".st-at").onchange = (e) => {
      const t = q<HTMLInputElement>(".st-t");
      t.disabled = (e.target as HTMLSelectElement).value === "start";
      if (t.disabled) {
        this.at = null;
        void this.analyse(true);
      } else t.focus();
    };
    const run = () => {
      const sel = q<HTMLSelectElement>(".st-at").value;
      const t = parseFloat(q<HTMLInputElement>(".st-t").value);
      this.at = sel === "t" && isFinite(t) ? t : null;
      void this.analyse(true);
    };
    q<HTMLButtonElement>(".st-run").onclick = run;
    q<HTMLInputElement>(".st-t").onkeydown = (e) => {
      if (e.key === "Enter") run();
    };
    const setOv = (name: string, tier: Tier | "") => {
      const ov = this.overrides();
      if (tier) ov.set(name, tier);
      else ov.delete(name);
      persist();
      void this.analyse(true);
    };
    h.querySelectorAll<HTMLSelectElement>("select[data-var]").forEach((s) => (s.onchange = () => setOv(s.dataset.var!, s.value as Tier | "")));
    h.querySelectorAll<HTMLButtonElement>("button[data-pin]").forEach((b) => (b.onclick = () => setOv(b.dataset.pin!, "prefer")));
    h.querySelectorAll<HTMLButtonElement>("button[data-forbid]").forEach((b) => (b.onclick = () => setOv(b.dataset.forbid!, "avoid")));
    h.querySelectorAll<HTMLButtonElement>("button[data-expand]").forEach(
      (b) =>
        (b.onclick = () => {
          this.expandedSets.add(+b.dataset.expand!);
          this.render();
        }),
    );
    h.querySelectorAll<HTMLButtonElement>("button[data-rm]").forEach((b) => (b.onclick = () => setOv(b.dataset.rm!, "")));
    q<HTMLButtonElement>(".st-ov-addbtn").onclick = () => {
      const n = q<HTMLInputElement>(".st-ov-name").value.trim();
      if (n) setOv(n, q<HTMLSelectElement>(".st-ov-tier").value as Tier);
    };
    q<HTMLButtonElement>(".st-clear").onclick = () => {
      this.overrides().clear();
      persist();
      void this.analyse(true);
    };
    q<HTMLButtonElement>(".st-copy").onclick = async () => {
      const text = asModifiers(stateOverridesFor(this.model));
      try {
        await navigator.clipboard.writeText(text);
        this.ctx.log(`Copied modifiers:\n${text}`, "ok");
      } catch {
        this.ctx.log(`Modifiers (copy them from here):\n${text}`);
      }
    };
    q<HTMLButtonElement>(".st-sim").onclick = () => this.ctx.simulate();
    h.querySelectorAll<HTMLElement>("[data-block]").forEach(
      (d) =>
        (d.onclick = (e) => {
          if ((e.target as HTMLElement).closest(".st-eqs")) return;
          const b = +d.dataset.block!;
          this.selectedBlock = this.selectedBlock === b ? null : b;
          const list = h.querySelector(".st-blocks")!;
          const top = list.scrollTop;
          list.innerHTML = this.blocksHtml(this.report!);
          list.scrollTop = top;
          this.wire();
          this.drawIncidence();
          list.querySelector(".st-block.open")?.scrollIntoView({ block: "nearest" });
        }),
    );
  }

  private drawAll() {
    this.drawIncidence();
    this.drawJacobian();
    this.drawEigen();
  }

  // ---------------------------------------------------------------- tooltips
  private showTip(e: MouseEvent, html: string) {
    this.tip.innerHTML = html;
    this.tip.classList.remove("hidden");
    const x = Math.min(e.clientX + 14, window.innerWidth - this.tip.offsetWidth - 8);
    const y = Math.min(e.clientY + 14, window.innerHeight - this.tip.offsetHeight - 8);
    this.tip.style.left = `${x}px`;
    this.tip.style.top = `${y}px`;
  }
  private hideTip() {
    this.tip.classList.add("hidden");
  }

  // ---------------------------------------------------------------- incidence
  private drawIncidence() {
    const c = this.host.querySelector<HTMLCanvasElement>(".st-inc");
    const r = this.report;
    if (!c || !r) return;
    const inc = r.incidence;
    const n = Math.max(inc.rows.length, 1);
    const w = Math.min(c.parentElement!.clientWidth - 4, 520, Math.max(200, n * 28));
    const g = sizeCanvas(c, w, w);
    const s = w / n;
    g.fillStyle = css("--panel");
    g.fillRect(0, 0, w, w);
    // blocks: tinted squares on the diagonal
    const blk = new Int32Array(n);
    for (let b = 0; b + 1 < inc.block_starts.length; b++) {
      const a = inc.block_starts[b], e = inc.block_starts[b + 1];
      const k = r.blocks[b]?.kind ?? "assign";
      blk.fill(b, a, e);
      if (e - a > 1 || b === this.selectedBlock) {
        g.fillStyle = kindColor(k) + "22";
        g.fillRect(a * s, a * s, (e - a) * s, (e - a) * s);
        g.strokeStyle = kindColor(k);
        g.lineWidth = b === this.selectedBlock ? 2 : 1;
        g.strokeRect(a * s + 0.5, a * s + 0.5, Math.max((e - a) * s - 1, 1), Math.max((e - a) * s - 1, 1));
      }
    }
    // inside a block: the block's colour; below the diagonal (an earlier block's unknown): grey
    const px = Math.max(s, 1);
    const below = css("--muted");
    for (const [row, col] of inc.entries) {
      g.fillStyle = blk[row] === blk[col] ? kindColor(r.blocks[blk[row]]?.kind ?? "assign") : below;
      g.fillRect(col * s, row * s, px, px);
    }
    c.onmousemove = (e) => {
      const rect = c.getBoundingClientRect();
      const col = Math.floor(((e.clientX - rect.left) / w) * n);
      const row = Math.floor(((e.clientY - rect.top) / w) * n);
      if (row < 0 || col < 0 || row >= inc.rows.length || col >= inc.cols.length) return this.hideTip();
      const eq = r.equations[inc.rows[row]];
      const hit = inc.entries.some(([a, b]) => a === row && b === col);
      this.showTip(e, `<div>row ${row}: <code>${esc(eq.text)}</code></div><div>column ${col}: <b>${esc(inc.cols[col])}</b>${hit ? " (occurs)" : ""}</div><div class="muted">block ${eq.block ?? "?"}</div>`);
    };
    c.onmouseleave = () => this.hideTip();
    c.onclick = (e) => {
      const rect = c.getBoundingClientRect();
      const row = Math.floor(((e.clientY - rect.top) / w) * n);
      const eq = r.equations[inc.rows[row]];
      if (eq?.block !== null && eq?.block !== undefined) {
        this.selectedBlock = eq.block;
        const list = this.host.querySelector(".st-blocks")!;
        list.innerHTML = this.blocksHtml(r);
        this.wire();
        this.drawIncidence();
        list.querySelector(".st-block.open")?.scrollIntoView({ block: "nearest" });
      }
    };
  }

  // ---------------------------------------------------------------- Jacobian heatmap
  private drawJacobian() {
    const c = this.host.querySelector<HTMLCanvasElement>(".st-jac");
    const j = this.report?.jacobian;
    if (!c || !j) return;
    const n = j.states.length;
    const w = Math.min(c.parentElement!.clientWidth - 4, 520, Math.max(160, n * 48));
    const g = sizeCanvas(c, w, w);
    const s = w / n;
    g.fillStyle = css("--panel");
    g.fillRect(0, 0, w, w);
    let lo = Infinity, hi = -Infinity;
    for (const [, , v] of j.entries) {
      const l = Math.log10(Math.abs(v));
      lo = Math.min(lo, l);
      hi = Math.max(hi, l);
    }
    const span = Math.max(hi - lo, 1e-9);
    const val = new Map<number, number>();
    for (const [a, b, v] of j.entries) {
      val.set(a * n + b, v);
      const t = 0.25 + 0.75 * ((Math.log10(Math.abs(v)) - lo) / span);
      g.fillStyle = v > 0 ? `rgba(201, 42, 42, ${t})` : `rgba(25, 113, 194, ${t})`;
      g.fillRect(b * s, a * s, Math.max(s, 1), Math.max(s, 1));
    }
    if (s >= 6) {
      g.strokeStyle = css("--grid");
      g.lineWidth = 1;
      for (let i = 0; i <= n; i++) {
        g.beginPath();
        g.moveTo(i * s, 0);
        g.lineTo(i * s, w);
        g.moveTo(0, i * s);
        g.lineTo(w, i * s);
        g.stroke();
      }
    }
    c.onmousemove = (e) => {
      const rect = c.getBoundingClientRect();
      const col = Math.floor(((e.clientX - rect.left) / w) * n);
      const row = Math.floor(((e.clientY - rect.top) / w) * n);
      if (row < 0 || col < 0 || row >= n || col >= n) return this.hideTip();
      const v = val.get(row * n + col) ?? 0;
      this.showTip(e, `<div>∂ der(<b>${esc(j.states[row])}</b>) / ∂ <b>${esc(j.states[col])}</b></div><div><code>${v === 0 ? "0" : v.toPrecision(6)}</code></div>`);
    };
    c.onmouseleave = () => this.hideTip();
  }

  // ---------------------------------------------------------------- eigenvalues
  private drawEigen() {
    const c = this.host.querySelector<HTMLCanvasElement>(".st-eig");
    const j = this.report?.jacobian;
    if (!c || !j || !j.modes.length) return;
    const prev = this.previous?.jacobian?.modes ?? [];
    const w = Math.min(c.parentElement!.clientWidth - 4, 520);
    const hgt = Math.round(w * 0.7);
    const g = sizeCanvas(c, w, hgt);
    const all = [...j.modes, ...prev];
    const mag = Math.max(...all.map((m) => Math.max(Math.abs(m.re), Math.abs(m.im))), 1e-12);
    const small = Math.min(...all.map((m) => Math.hypot(m.re, m.im)).filter((x) => x > 0), mag);
    // symmetric log axes when the eigenvalues span more than two decades
    const logAxes = mag / small > 100;
    const c0 = small / 10;
    const tr = (v: number) => (logAxes ? Math.sign(v) * Math.log10(1 + Math.abs(v) / c0) : v);
    const R = tr(mag) * 1.1;
    const pad = 28;
    const X = (re: number) => pad + ((tr(re) + R) / (2 * R)) * (w - 2 * pad);
    const Y = (im: number) => hgt - pad - ((tr(im) + R) / (2 * R)) * (hgt - 2 * pad);
    g.fillStyle = css("--panel");
    g.fillRect(0, 0, w, hgt);
    // stable half-plane tint
    g.fillStyle = "rgba(43, 138, 62, 0.06)";
    g.fillRect(pad, pad, X(0) - pad, hgt - 2 * pad);
    g.strokeStyle = css("--line");
    g.strokeRect(pad, pad, w - 2 * pad, hgt - 2 * pad);
    g.strokeStyle = css("--muted");
    g.beginPath();
    g.moveTo(X(0), pad);
    g.lineTo(X(0), hgt - pad);
    g.moveTo(pad, Y(0));
    g.lineTo(w - pad, Y(0));
    g.stroke();
    g.fillStyle = css("--muted");
    g.font = "11px system-ui, sans-serif";
    g.fillText(logAxes ? "Re (symlog)" : "Re", w - pad - 60, Y(0) - 4);
    g.fillText(logAxes ? "Im (symlog)" : "Im", X(0) + 4, pad + 10);
    g.fillText(`±${fmt(mag)}`, w - pad - 40, hgt - pad + 14);
    for (const m of prev) {
      g.strokeStyle = css("--muted");
      g.beginPath();
      g.arc(X(m.re), Y(m.im), 5, 0, 2 * Math.PI);
      g.stroke();
    }
    for (const m of j.modes) {
      g.fillStyle = m.re > 0 ? css("--err") : css("--accent");
      g.beginPath();
      g.arc(X(m.re), Y(m.im), 3.5, 0, 2 * Math.PI);
      g.fill();
    }
    c.onmousemove = (e) => {
      const rect = c.getBoundingClientRect();
      const mx = e.clientX - rect.left, my = e.clientY - rect.top;
      let best: Mode | null = null, bd = 100;
      for (const m of j.modes) {
        const d = Math.hypot(X(m.re) - mx, Y(m.im) - my);
        if (d < bd) {
          bd = d;
          best = m;
        }
      }
      if (!best) return this.hideTip();
      const m = best;
      const lam = m.im === 0 ? fmt(m.re, 5) : `${fmt(m.re, 5)} ${m.im < 0 ? "−" : "+"} ${fmt(Math.abs(m.im), 5)}i`;
      const parts = m.participation.filter((p) => p[1] >= 0.01).map(([k, f]) => `<div><b>${esc(j.states[k])}</b> ${(100 * f).toFixed(0)} %</div>`).join("");
      this.showTip(
        e,
        `<div>λ = <code>${lam}</code></div>
         <div>${m.time_constant !== null ? `τ = ${fmt(m.time_constant)} s` : m.im === 0 ? "integrator (λ = 0)" : "undamped"}${m.frequency !== null ? `, f = ${fmt(m.frequency)} Hz` : ""}${m.damping !== null && m.im !== 0 ? `, ζ = ${fmt(m.damping)}` : ""}</div>
         ${parts ? `<div class="muted">participating states</div>${parts}` : ""}`,
      );
    };
    c.onmouseleave = () => this.hideTip();
  }
}

function chip(label: string, value: string, sub = "", title = "") {
  return `<div class="st-chip" title="${esc(title)}"><div class="muted">${label}</div><div class="v">${value}</div>${sub ? `<div class="muted sub">${sub}</div>` : ""}</div>`;
}
