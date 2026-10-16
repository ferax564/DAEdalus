// Equation and initialization debugger: renders the reports of crates/daedalus-core/src/debug.rs
// (transformation chains, initialization report, structural diagnosis) for the Structure view.
import "./debug.css";

export interface Loc { file: string; line: number; col: number }
export interface SourceRef { text: string; now?: string; note?: string; origin: string; component: string | null; loc: Loc | null; initial: boolean }
export interface Step { kind: string; title: string; equation: string | null; source: SourceRef | null; details: string[] }
export interface StartInfo { expr: string | null; fixed: boolean | null; written_in: string | null; loc: Loc | null; from_alias: string | null }
export interface VarTrace {
  name: string; role: string; unit: string; description: string; equation: number | null; block: number | null;
  block_kind: string | null; tearing_role: string | null; alias_of: string | null; start: StartInfo; steps: Step[];
}
export interface EqTrace { id: number; text: string; block: number | null; solved_for: string | null; steps: Step[] }
export interface Unconnected { connector: string; component: string; flows: string[]; level: "warning" | "note"; reason: string }
export interface Subset { unmatched: string[]; equations: SourceRef[]; variables: string[]; components: string[]; excess: number; truncated: boolean }
export interface StructuralDiag { problem: string; equations: number; unknowns: number; underdetermined: Subset[]; overdetermined: Subset[]; hints: string[] }
export interface ResidualInfo { equation: string; value: number | null; source: SourceRef | null }
export interface UpdateInfo { var: string; value: number | null; step: number | null; relative: number | null; start: number | null }
export interface SolverFailure {
  phase: string; time: number; block: number | null; linear: boolean; tears: string[]; inner: string[]; residual: number | null;
  iterations: number; history: (number | null)[]; residual_norm: number | null; residuals: ResidualInfo[]; updates: UpdateInfo[];
  jacobian_singular: boolean; hints: string[];
}
export interface InitUnknown {
  name: string; kind: string; start_use: "fixed" | "defaulted" | "guess" | "computed"; start_value: number | null;
  start: StartInfo; value: number | null; block: number | null;
}
export interface InitReport {
  model: string; ok: boolean; stage: string; error: string | null; structural: StructuralDiag | null; unknowns: InitUnknown[];
  relevant: number; assignments: number; loops: { index: number; kind: string; tears: string[]; inner: string[]; residuals: string[] }[];
  failure: SolverFailure | null; simulation_failure?: SolverFailure | null; non_finite: string[]; redundant: string[]; unconnected: Unconnected[]; warnings: string[]; ms?: number;
}

export const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
const num = (v: number | null | undefined) => (v === null || v === undefined ? "—" : !isFinite(v) ? String(v) : Math.abs(v) >= 1e5 || (Math.abs(v) < 1e-3 && v !== 0) ? v.toExponential(3) : String(+v.toPrecision(6)));
const base = (f: string) => f.split(/[\\/]/).pop() ?? f;

/** `file:line`, clickable (`data-loc`) to open the source. */
export function locHtml(l: Loc | null | undefined): string {
  if (!l) return "";
  return `<a class="dbg-loc" data-loc-file="${esc(l.file)}" data-loc-line="${l.line}" title="${esc(l.file)}:${l.line}:${l.col}">${esc(base(l.file))}:${l.line}</a>`;
}

/** A variable name that traces itself when clicked. */
export const varLink = (n: string) => `<a class="dbg-var" data-trace-var="${esc(n)}">${esc(n)}</a>`;

function sourceHtml(s: SourceRef | null): string {
  if (!s) return "";
  const comp = s.component && !s.origin.includes(s.component) ? ` <span class="dbg-comp">${esc(s.component)}</span>` : "";
  return `<div class="dbg-src">${esc(s.origin)}${comp} ${locHtml(s.loc)}</div>`;
}

const KIND_LABEL: Record<string, string> = {
  source: "written", alias: "alias", substituted: "substituted", differentiated: "d/dt", "state-selection": "states",
  assign: "solve", inner: "torn", tear: "tear", residual: "residual", newton: "Newton", state: "state", parameter: "param",
  initialization: "init", representative: "→", discrete: "discrete", unused: "unused",
};

/** Names in a "tear variables: a, b" style detail become links. */
function detailHtml(d: string): string {
  const m = /^(tear variables|inner \(explicit\) variables|states|dummy derivatives): (.*)$/.exec(d);
  if (!m) return esc(d);
  return `${esc(m[1])}: ${m[2].split(", ").map((n) => (n ? varLink(n) : "")).join(", ")}`;
}

export function stepsHtml(steps: Step[]): string {
  return `<ol class="dbg-steps">${steps
    .map(
      (s) => `<li class="dbg-step k-${esc(s.kind)}">
        <span class="dbg-kind">${esc(KIND_LABEL[s.kind] ?? s.kind)}</span>
        <div class="dbg-body">
          <div class="dbg-title">${esc(s.title)}</div>
          ${s.equation ? `<code class="dbg-eq">${esc(s.equation)}</code>` : ""}
          ${sourceHtml(s.source)}
          ${s.details.length ? `<ul class="dbg-details">${s.details.map((d) => `<li>${detailHtml(d)}</li>`).join("")}</ul>` : ""}
        </div>
      </li>`,
    )
    .join("")}</ol>`;
}

function startHtml(s: StartInfo): string {
  if (!s.expr && s.fixed === null) return `<span class="muted">no start value (0)</span>`;
  const where = s.written_in !== null ? ` set in ${s.written_in ? `<b>${esc(s.written_in)}</b>` : "the model"}` : "";
  return `start = <code>${esc(s.expr ?? "0")}</code>${s.fixed !== null ? `, fixed = ${s.fixed}` : ""}${where} ${locHtml(s.loc)}${s.from_alias ? ` <span class="muted">(from alias ${varLink(s.from_alias)})</span>` : ""}`;
}

export function varTraceHtml(t: VarTrace): string {
  const facts = [
    `<span class="dbg-role r-${esc(t.role.replace(/ /g, "-"))}">${esc(t.role)}</span>`,
    t.unit ? `<span class="muted">[${esc(t.unit)}]</span>` : "",
    t.alias_of ? `alias of ${varLink(t.alias_of.replace(/^-/, ""))}${t.alias_of.startsWith("-") ? " (negated)" : ""}` : "",
    t.block !== null ? `<a class="dbg-blk" data-goto-block="${t.block}">block ${t.block}</a> (${esc(t.block_kind ?? "?")}, ${esc(t.tearing_role ?? "?")})` : "",
  ].filter(Boolean);
  return `<div class="dbg-head"><code class="dbg-name">${esc(t.name)}</code> ${facts.join(" · ")}</div>
    ${t.description ? `<div class="muted">${esc(t.description)}</div>` : ""}
    <div class="dbg-start">${startHtml(t.start)}</div>
    ${stepsHtml(t.steps)}`;
}

export function eqTraceHtml(t: EqTrace): string {
  return `<div class="dbg-head">equation ${t.id} ${t.block !== null ? `· <a class="dbg-blk" data-goto-block="${t.block}">block ${t.block}</a>` : ""} ${t.solved_for ? `· solved for ${varLink(t.solved_for)}` : "· residual"}</div>
    <code class="dbg-eq">${esc(t.text)}</code>
    ${stepsHtml(t.steps)}`;
}

export function structuralHtml(d: StructuralDiag): string {
  const subset = (s: Subset, k: number, under: boolean) => `<div class="dbg-subset ${under ? "under" : "over"}">
      <div class="dbg-subset-head">${under ? "Underdetermined" : "Overdetermined"} part ${k + 1}: ${s.equations.length} equation(s), ${s.variables.length} unknown(s)${s.truncated ? " (truncated)" : ""}</div>
      ${under ? `<div>No equation left for: ${s.unmatched.map((n) => `<code class="dbg-bad">${esc(n)}</code>`).join(", ")}</div>` : s.unmatched.map((e) => `<div>Extra equation: <code class="dbg-bad">${esc(e)}</code></div>`).join("")}
      ${s.components.length ? `<div class="muted">components: ${s.components.map((c) => `<b>${esc(c)}</b>`).join(", ")}</div>` : ""}
      <table class="dbg-table">${s.equations
        .slice(0, 30)
        .map((e) => `<tr><td><code>${esc(e.text)}</code>${e.now ? `<div class="dbg-now">after alias substitution: <code>${esc(e.now)}</code></div>` : ""}${e.note ? `<div class="dbg-note">${esc(e.note)}</div>` : ""}</td><td class="muted">${esc(e.origin)}${e.component && !e.origin.includes(e.component) ? ` [${esc(e.component)}]` : ""}</td><td>${locHtml(e.loc)}</td></tr>`)
        .join("")}</table>
      ${s.equations.length > 30 ? `<div class="muted">… ${s.equations.length - 30} more</div>` : ""}
      ${s.variables.length ? `<div class="muted dbg-vars">unknowns: ${esc(s.variables.slice(0, 40).join(", "))}${s.variables.length > 40 ? " …" : ""}</div>` : ""}
    </div>`;
  return `<div class="dbg-struct">
    <div class="dbg-head">Structural analysis of the ${esc(d.problem)} problem: ${d.equations} equations, ${d.unknowns} unknowns</div>
    ${d.underdetermined.map((s, k) => subset(s, k, true)).join("")}
    ${d.overdetermined.map((s, k) => subset(s, k, false)).join("")}
    ${!d.underdetermined.length && !d.overdetermined.length ? `<div class="muted">The matching found no unmatched equation or unknown.</div>` : ""}
    ${(d.hints ?? []).map((h) => `<div class="dbg-hint">${esc(h)}</div>`).join("")}
  </div>`;
}

export function unconnectedHtml(list: Unconnected[]): string {
  if (!list.length) return "";
  const warn = list.filter((u) => u.level === "warning");
  const notes = list.filter((u) => u.level !== "warning");
  const row = (u: Unconnected) => `<div class="dbg-unc ${u.level}"><code>${esc(u.connector)}</code> <span class="muted">${esc(u.reason)}</span></div>`;
  return `<div class="dbg-sub">Unconnected connectors</div>${warn.map(row).join("")}${notes.length ? `<details><summary class="muted">${notes.length} free end(s) (flow set to zero)</summary>${notes.map(row).join("")}</details>` : ""}`;
}

function failureHtml(f: SolverFailure): string {
  const hist = f.history.filter((x): x is number => x !== null);
  return `<div class="dbg-fail">
    <div class="dbg-head">${esc(f.phase)}: ${f.linear ? "linear" : "nonlinear"} ${f.block !== null ? `block ${f.block}` : "block"} failed at t = ${num(f.time)}</div>
    <div>tear variables: ${f.tears.map(varLink).join(", ")}${f.inner.length ? ` <span class="muted">(+ ${f.inner.length} inner: ${f.inner.map(varLink).join(", ")})</span>` : ""}</div>
    <div class="muted">solver residual ${num(f.residual)}; diagnostic damped Newton re-run: ${f.iterations} iteration(s), residual ${num(f.residual_norm)}${f.jacobian_singular ? ", <b class='dbg-bad'>singular Jacobian</b>" : ""}</div>
    ${hist.length > 1 ? `<div class="dbg-hist" title="max |residual| per iteration">${sparkline(hist)}</div>` : ""}
    <div class="dbg-sub">Residuals at the last iterate</div>
    <table class="dbg-table">${f.residuals
      .slice(0, 12)
      .map((r) => `<tr><td class="num ${Math.abs(r.value ?? Infinity) > 1e-6 ? "dbg-bad" : ""}">${num(r.value)}</td><td><code>${esc(r.equation)}</code>${r.source ? sourceHtml(r.source) : ""}</td></tr>`)
      .join("")}</table>
    <div class="dbg-sub">Largest Newton updates</div>
    <table class="dbg-table"><tr class="muted"><td>variable</td><td>value</td><td>last step</td><td>relative</td><td>start</td></tr>${f.updates
      .slice(0, 12)
      .map((u) => `<tr><td>${varLink(u.var)}</td><td class="num">${num(u.value)}</td><td class="num">${num(u.step)}</td><td class="num">${num(u.relative)}</td><td class="num">${num(u.start)}</td></tr>`)
      .join("")}</table>
    ${f.hints.map((h) => `<div class="dbg-hint">${esc(h)}</div>`).join("")}
  </div>`;
}

function sparkline(v: number[]): string {
  const w = 220, h = 36;
  const l = v.map((x) => Math.log10(Math.max(x, 1e-300)));
  const lo = Math.min(...l), hi = Math.max(...l);
  const pts = l.map((y, i) => `${((i / Math.max(l.length - 1, 1)) * (w - 4) + 2).toFixed(1)},${(h - 2 - ((y - lo) / Math.max(hi - lo, 1e-9)) * (h - 4)).toFixed(1)}`).join(" ");
  return `<svg width="${w}" height="${h}"><polyline fill="none" stroke="currentColor" stroke-width="1.5" points="${pts}"/></svg><span class="muted"> log₁₀ residual over ${v.length - 1} iterations</span>`;
}

const USE_LABEL: Record<string, string> = { fixed: "fixed", defaulted: "defaulted", guess: "guess", computed: "computed" };
const USE_TITLE: Record<string, string> = {
  fixed: "fixed = true: the start value is an initial equation",
  defaulted: "no fixed attribute, but the initial system needed one more equation: the start value was used",
  guess: "iteration start of a nonlinear block: the start value is the first Newton guess",
  computed: "computed explicitly: the start value does not matter",
};

export function initHtml(r: InitReport, showAll: boolean): string {
  const status = r.ok
    ? `<span class="dbg-ok">✓ initialization succeeds</span>`
    : `<span class="dbg-bad">✗ ${r.stage === "initialization" ? "initialization fails" : r.stage === "translation" ? "the model does not translate" : "the model does not flatten"}</span>`;
  const rows = (showAll ? r.unknowns : r.unknowns.filter((u) => u.start_use !== "computed")).slice(0, 1500);
  const table = r.stage === "initialization" || r.stage === "ok"
    ? `<div class="dbg-sub">Unknowns of the initial system <span class="muted">(${r.unknowns.length}; ${r.relevant} whose start value matters, ${r.assignments} explicit assignments, ${r.loops.length} loop${r.loops.length === 1 ? "" : "s"})</span>
        <label class="dbg-all"><input type="checkbox" class="dbg-showall"${showAll ? " checked" : ""}/> show all</label></div>
      <table class="dbg-table dbg-init"><tr class="muted"><td>use</td><td>variable</td><td>kind</td><td>start</td><td>set in</td><td>${r.ok ? "initial value" : "value at failure"}</td></tr>
      ${rows
        .map(
          (u) => `<tr class="u-${u.start_use}"><td><span class="dbg-use ${u.start_use}" title="${esc(USE_TITLE[u.start_use])}">${USE_LABEL[u.start_use]}</span></td>
          <td>${varLink(u.name)}</td><td class="muted">${esc(u.kind)}</td>
          <td class="num">${num(u.start_value)}${u.start.expr && isNaN(Number(u.start.expr)) ? ` <span class="muted">= ${esc(u.start.expr)}</span>` : ""}${u.start.from_alias ? ` <span class="muted">(via ${esc(u.start.from_alias)})</span>` : ""}</td>
          <td>${u.start.written_in !== null ? (u.start.written_in ? esc(u.start.written_in) : "model") : u.start.expr ? "" : '<span class="muted">default</span>'} ${locHtml(u.start.loc)}</td>
          <td class="num">${num(u.value)}</td></tr>`,
        )
        .join("")}</table>${rows.length < (showAll ? r.unknowns.length : r.relevant) ? `<div class="muted">first ${rows.length} shown</div>` : ""}`
    : "";
  return `<div class="dbg-init-head">${status} ${r.ms !== undefined ? `<span class="muted">${r.ms.toFixed(0)} ms</span>` : ""}</div>
    ${r.error ? `<pre class="dbg-err">${esc(r.error)}</pre>` : ""}
    ${r.structural ? structuralHtml(r.structural) : ""}
    ${r.failure ? failureHtml(r.failure) : ""}
    ${r.simulation_failure ? `<div class="dbg-sub">The simulation stopped in a nonlinear solve</div>${failureHtml(r.simulation_failure)}` : ""}
    ${r.non_finite.length ? `<div class="dbg-hint">not finite after initialization: ${r.non_finite.map(varLink).join(", ")}</div>` : ""}
    ${table}
    ${r.redundant.map((x) => `<div class="dbg-hint">redundant initial equation ignored: <code>${esc(x)}</code></div>`).join("")}
    ${unconnectedHtml(r.unconnected)}`;
}
