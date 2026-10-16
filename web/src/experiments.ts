// Experiments: every simulation becomes a stored run (memory + IndexedDB), runs are
// overlaid or subtracted in the plot, parameter sweeps fan out over the worker pool, and a
// link in the URL hash reopens a model with its overrides and settings and runs it.
import "./experiments.css";
import { PlotWindow, displayUnit, interp, type Series } from "./plot";
import { pool, CancelledError, type RunOutput } from "./pool";
import type { SimSettings, SimSummary } from "./types";

/** What the workbench (main.ts) lends the experiments UI. */
export interface Host {
  plot: PlotWindow;
  /** The open model, if any. */
  model(): string | null;
  /** Result summary of the last single run and the model it belongs to. */
  summary(): { summary: SimSummary; model: string } | null;
  /** Simulation settings of the toolbar, with the parameter overrides. */
  settings(): SimSettings;
  log(msg: string, cls?: string): void;
  /** Show the plot (simulation mode). */
  showPlot(): void;
  /** Apply overrides and settings to the open model, simulate it and plot `vars`. */
  restore(model: string, params: [string, number][], options: SimSettings, vars: string[]): Promise<void>;
}

export interface Run {
  id: string;
  label: string;
  model: string;
  /** Parameter overrides of this run. */
  params: [string, number][];
  /** Settings without the overrides. */
  options: SimSettings;
  created: number;
  pinned: boolean;
  ok: boolean;
  error?: string | null;
  sweep?: { id: string; index: number };
  /** Pool worker that ran it (sweeps). */
  worker?: number;
  t: Float64Array;
  series: Record<string, Float64Array>;
  units: Record<string, string>;
}

// ---------------------------------------------------------------- storage
const DB_NAME = "daedalus-experiments";
const STORE = "runs";
/** Unpinned runs kept; older ones are dropped first. */
const MAX_RUNS = 300;

let dbp: Promise<IDBDatabase | null> | null = null;
function db(): Promise<IDBDatabase | null> {
  return (dbp ??= new Promise((resolve) => {
    try {
      const req = indexedDB.open(DB_NAME, 1);
      req.onupgradeneeded = () => req.result.createObjectStore(STORE, { keyPath: "id" });
      req.onsuccess = () => resolve(req.result);
      req.onerror = () => resolve(null);
      req.onblocked = () => resolve(null);
    } catch {
      resolve(null);
    }
  }));
}

async function dbDo(fn: (s: IDBObjectStore) => IDBRequest | void, mode: IDBTransactionMode = "readwrite"): Promise<unknown> {
  try {
    const d = await db();
    if (!d) return null;
    return await new Promise((resolve) => {
      const tx = d.transaction(STORE, mode);
      const req = fn(tx.objectStore(STORE));
      tx.oncomplete = () => resolve(req ? req.result : null);
      tx.onerror = tx.onabort = () => resolve(null);
    });
  } catch {
    return null;
  }
}

// ---------------------------------------------------------------- run store
export const runs: Run[] = [];
const listeners: (() => void)[] = [];
let counter = 0;

function changed() {
  for (const f of listeners) f();
}

function newId() {
  return `${Date.now().toString(36)}-${(counter++).toString(36)}-${Math.random().toString(36).slice(2, 6)}`;
}

function nextNumber(): number {
  let n = 0;
  for (const r of runs) {
    const m = /^run (\d+)$/.exec(r.label);
    if (m) n = Math.max(n, +m[1]);
  }
  return n + 1;
}

export function addRun(r: Omit<Run, "id" | "created" | "pinned" | "label"> & Partial<Pick<Run, "label">>): Run {
  const run = { ...r, id: newId(), created: Date.now(), pinned: false, label: r.label ?? `run ${nextNumber()}` } as Run;
  runs.unshift(run);
  void dbDo((s) => s.put(run));
  // Bound the history: the oldest unpinned runs go first.
  const unpinned = runs.filter((x) => !x.pinned);
  for (const old of unpinned.slice(MAX_RUNS)) deleteRun(old.id, false);
  changed();
  return run;
}

export function updateRun(id: string, patch: Partial<Run>) {
  const r = runs.find((x) => x.id === id);
  if (!r) return;
  Object.assign(r, patch);
  void dbDo((s) => s.put(r));
  changed();
}

export function deleteRun(id: string, notify = true) {
  const i = runs.findIndex((x) => x.id === id);
  if (i < 0) return;
  runs.splice(i, 1);
  selected.delete(id);
  void dbDo((s) => s.delete(id));
  if (notify) changed();
}

async function loadRuns() {
  const all = (await dbDo((s) => s.getAll(), "readonly")) as Run[] | null;
  if (!all?.length) return;
  const have = new Set(runs.map((r) => r.id));
  for (const r of all) if (!have.has(r.id)) runs.push(r);
  runs.sort((a, b) => b.created - a.created);
  changed();
}

/** Record a single simulation from the Simulate button. */
export function recordSimulation(model: string, settings: SimSettings, s: SimSummary, time: Float64Array, cols: Map<string, Float64Array>) {
  const { params, ...options } = settings;
  const units: Record<string, string> = {};
  const series: Record<string, Float64Array> = {};
  for (const [n, y] of cols) {
    if (n === "time") continue;
    series[n] = y;
    units[n] = s.vars.find((v) => v.name === n)?.unit ?? "";
  }
  lastSingle = addRun({ model, params: params ?? [], options, ok: s.ok, error: s.error, t: time, series, units });
}
let lastSingle: Run | null = null;

/** A variable fetched later for the last single run (ticked in the variable browser). */
export function noteSeries(model: string, name: string, unit: string, time: Float64Array, y: Float64Array) {
  const r = lastSingle;
  if (!r || r.model !== model || r.series[name] || !runs.includes(r)) return;
  if (r.t.length !== time.length || r.t[r.t.length - 1] !== time[time.length - 1]) return;
  r.series[name] = y;
  r.units[name] = unit;
  void dbDo((s) => s.put(r));
}

// ---------------------------------------------------------------- comparison
const selected = new Set<string>();

function fmt(v: number): string {
  return String(+v.toPrecision(6));
}

/** Parameters whose values differ between the runs (or all of them for a single run). */
function differing(rs: Run[]): string[] {
  const names = new Set(rs.flatMap((r) => r.params.map((p) => p[0])));
  const val = (r: Run, n: string) => r.params.find((p) => p[0] === n)?.[1];
  return [...names].filter((n) => rs.length < 2 || new Set(rs.map((r) => val(r, n))).size > 1);
}

function legendLabel(r: Run, diff: string[]): string {
  const ps = diff
    .map((n) => {
      const v = r.params.find((p) => p[0] === n)?.[1];
      return v === undefined ? null : `${n}=${fmt(v)}`;
    })
    .filter(Boolean);
  return ps.length ? `${r.label} (${ps.join(", ")})` : r.label;
}

/** Remove every overlay from the plot. */
export function clearOverlays(plot: PlotWindow) {
  for (const sp of plot.subplots) sp.series = sp.series.filter((s) => !s.run);
  if (plot.subplots.length > 1) plot.subplots = plot.subplots.filter((sp) => sp.series.length > 0);
  if (!plot.subplots.length) plot.subplots = [{ series: [] }];
  plot.active = Math.min(plot.active, plot.subplots.length - 1);
  plot.draw();
  plot.onChange();
}

function pushSeries(plot: PlotWindow, s: Omit<Series, "color" | "scale" | "offset" | "displayUnit">) {
  const du = displayUnit(s.unit);
  plot.subplots[plot.active].series.push({ ...s, color: plot.nextColor(), scale: du.scale, offset: du.offset, displayUnit: du.unit });
}

/** Variables to compare: those plotted live in the active subplot, else the first one the runs share. */
function compareVars(plot: PlotWindow, rs: Run[]): string[] {
  const live = plot.subplots[plot.active].series.filter((s) => !s.run).map((s) => s.name);
  const have = live.filter((n) => rs.some((r) => r.series[n]));
  if (have.length) return have;
  const overlaid = [...new Set(plot.subplots[plot.active].series.filter((s) => s.run).map((s) => s.name.split(" · ")[0]))];
  const again = overlaid.filter((n) => rs.some((r) => r.series[n]));
  if (again.length) return again;
  const first = rs.map((r) => Object.keys(r.series)).find((k) => k.length);
  return first ? [first[0]] : [];
}

/** Plot `vars` of every run in `rs` in the active subplot (one curve per run and variable). */
export function overlay(plot: PlotWindow, rs: Run[], vars?: string[]): number {
  const names = vars ?? compareVars(plot, rs);
  for (const sp of plot.subplots) sp.series = sp.series.filter((s) => !s.run);
  // Runs are plotted over time: a parametric subplot gets a time subplot next to it.
  if (plot.subplots[plot.active].xName) plot.addSubplot();
  const diff = differing(rs);
  let n = 0;
  for (const v of names) {
    for (const r of rs) {
      const y = r.series[v];
      if (!y) continue;
      pushSeries(plot, { name: `${v} · ${legendLabel(r, diff)}`, unit: r.units[v] ?? "", x: r.t, y, run: r.id });
      n++;
    }
  }
  plot.draw();
  plot.onChange();
  return n;
}

/** `b - a` on the union of both time grids over their common interval. */
export function difference(ta: Float64Array, ya: Float64Array, tb: Float64Array, yb: Float64Array): { t: Float64Array; y: Float64Array } {
  const lo = Math.max(ta[0], tb[0]);
  const hi = Math.min(ta[ta.length - 1], tb[tb.length - 1]);
  const grid: number[] = [];
  let i = 0;
  let j = 0;
  while (i < ta.length || j < tb.length) {
    const x = j >= tb.length || (i < ta.length && ta[i] <= tb[j]) ? ta[i++] : tb[j++];
    if (x < lo || x > hi) continue;
    if (grid.length && x <= grid[grid.length - 1]) continue;
    grid.push(x);
  }
  const t = Float64Array.from(grid);
  const y = t.map((x) => interp(tb, yb, x) - interp(ta, ya, x));
  return { t, y };
}

/** Plot run B minus run A (B is the newer one) in a new subplot. */
export function diff(plot: PlotWindow, a: Run, b: Run, vars?: string[]): number {
  if (a.created > b.created) [a, b] = [b, a];
  const names = (vars ?? compareVars(plot, [a, b])).filter((v) => a.series[v] && b.series[v]);
  if (!names.length) return 0;
  const sp = plot.subplots[plot.active];
  if (sp.series.length) plot.addSubplot();
  const d = differing([a, b]);
  for (const v of names) {
    const r = difference(a.t, a.series[v], b.t, b.series[v]);
    pushSeries(plot, { name: `${v} · ${legendLabel(b, d)} − ${legendLabel(a, d)}`, unit: a.units[v] ?? "", x: r.t, y: r.y, run: `${b.id}-${a.id}` });
  }
  plot.draw();
  plot.onChange();
  return names.length;
}

// ---------------------------------------------------------------- share links
export interface Share {
  model: string;
  params: [string, number][];
  options: SimSettings;
  plot: string[];
  /** The link asks for a run (it has a query part). */
  run: boolean;
}

/** `#Model.Name` or `#Model.Name?p.k=2&stop=3&tol=1e-6&n=500&method=Bdf&plot=y,z`. */
export function parseShare(hash: string): Share {
  const h = hash.startsWith("#") ? hash.slice(1) : hash;
  const q = h.indexOf("?");
  const model = decodeURIComponent(q >= 0 ? h.slice(0, q) : h);
  const out: Share = { model, params: [], options: {}, plot: [], run: q >= 0 };
  if (q < 0) return out;
  const sp = new URLSearchParams(h.slice(q + 1));
  for (const [k, v] of sp) {
    const x = parseFloat(v);
    if (k.startsWith("p.")) {
      if (isFinite(x)) out.params.push([k.slice(2), x]);
    } else if (k === "stop" && isFinite(x)) out.options.stop_time = x;
    else if (k === "start" && isFinite(x)) out.options.start_time = x;
    else if (k === "tol" && x > 0) out.options.tolerance = x;
    else if (k === "n" && x > 0) {
      out.options.intervals = Math.round(x);
      out.options.interval = 0;
    } else if (k === "method" && v) out.options.method = v;
    else if (k === "plot") out.plot = v.split(",").filter(Boolean);
  }
  return out;
}

export function shareHash(model: string, params: [string, number][], options: SimSettings, plotVars: string[]): string {
  const sp = new URLSearchParams();
  for (const [n, v] of params) sp.append(`p.${n}`, String(v));
  if (options.start_time !== undefined) sp.set("start", String(options.start_time));
  if (options.stop_time !== undefined) sp.set("stop", String(options.stop_time));
  if (options.tolerance !== undefined) sp.set("tol", String(options.tolerance));
  if (options.intervals !== undefined) sp.set("n", String(options.intervals));
  if (options.method) sp.set("method", options.method);
  if (plotVars.length) sp.set("plot", plotVars.join(","));
  // `.` and `,` read better unescaped and are safe in a fragment.
  return `#${encodeURIComponent(model)}?${sp.toString().replace(/%2C/g, ",")}`;
}

export function shareUrl(model: string, params: [string, number][], options: SimSettings, plotVars: string[]): string {
  return `${location.origin}${location.pathname}${location.search}${shareHash(model, params, options, plotVars)}`;
}

// ---------------------------------------------------------------- sweeps
export type Spacing = "list" | "lin" | "log";

/** Values of one swept parameter (as `daedalus sweep -s`: list, lin:a:b:n, log:a:b:n). */
export function sweepValues(spacing: Spacing, list: string, a: number, b: number, n: number): number[] {
  if (spacing === "list") return list.split(/[,\s;]+/).filter(Boolean).map(Number).filter((x) => isFinite(x));
  n = Math.max(1, Math.floor(n));
  if (!isFinite(a) || !isFinite(b)) return [];
  if (spacing === "log" && !(a > 0 && b > 0)) return [];
  const at = (i: number) => (n === 1 ? 0 : i / (n - 1));
  return Array.from({ length: n }, (_, i) =>
    i === n - 1 && n > 1 ? b : spacing === "lin" ? a + (b - a) * at(i) : Math.pow(10, Math.log10(a) + (Math.log10(b) - Math.log10(a)) * at(i)),
  );
}

/** Parameter values of every run: full grid (last parameter fastest) or grouped. */
export function expandSweep(params: { name: string; values: number[] }[], grouped: boolean): [string, number][][] {
  if (!params.length) return [];
  if (grouped) {
    const n = params[0].values.length;
    if (params.some((p) => p.values.length !== n)) throw new Error("grouped sweep: every parameter needs the same number of values");
    return Array.from({ length: n }, (_, i) => params.map((p) => [p.name, p.values[i]] as [string, number]));
  }
  let out: [string, number][][] = [[]];
  for (const p of params) out = out.flatMap((pt) => p.values.map((v) => [...pt, [p.name, v] as [string, number]]));
  return out;
}

export type Stat = "final" | "max" | "min" | "mean" | "initial";

export function stat(t: Float64Array, y: Float64Array, s: Stat): number {
  if (!y?.length) return NaN;
  switch (s) {
    case "initial":
      return y[0];
    case "final":
      return y[y.length - 1];
    case "max":
      return y.reduce((a, b) => Math.max(a, b), -Infinity);
    case "min":
      return y.reduce((a, b) => Math.min(a, b), Infinity);
    case "mean": {
      const span = t[t.length - 1] - t[0];
      if (!(span > 0)) return y[0];
      let acc = 0;
      for (let i = 1; i < t.length; i++) acc += 0.5 * (y[i] + y[i - 1]) * (t[i] - t[i - 1]);
      return acc / span;
    }
  }
}

interface SweepState {
  id: string;
  model: string;
  label: string;
  params: string[];
  vars: string[];
  total: number;
  done: number;
  failed: number;
  runs: Run[];
  workers: Set<number>;
  started: number;
  finished?: number;
  cancelled: boolean;
}

let sweepNo = 0;
let active: SweepState | null = null;
export let lastSweep: SweepState | null = null;

/** Run a sweep over the pool. `onProgress` is called after each run. */
export async function runSweep(
  host: Host,
  model: string,
  points: [string, number][][],
  vars: string[],
  onProgress: (s: SweepState) => void,
): Promise<SweepState> {
  const base = host.settings();
  const { params: baseParams, ...options } = base;
  const st: SweepState = {
    id: newId(),
    model,
    label: `sweep ${++sweepNo}`,
    params: points[0]?.map((p) => p[0]) ?? [],
    vars,
    total: points.length,
    done: 0,
    failed: 0,
    runs: [],
    workers: new Set(),
    started: performance.now(),
    cancelled: false,
  };
  active = lastSweep = st;
  const p = pool();
  p.peak = 0;
  const units: Record<string, string> = {};
  const sum = host.summary();
  for (const v of vars) units[v] = sum?.summary.vars.find((x) => x.name === v)?.unit ?? "";
  const width = String(points.length).length;
  await Promise.all(
    points.map(async (pt, i) => {
      // The sweep's values replace any override of the same parameter.
      const params = [...(baseParams ?? []).filter(([n]) => !pt.some(([m]) => m === n)), ...pt];
      let out: RunOutput | null = null;
      let error: string | null = null;
      try {
        out = await p.run(model, { ...options, params }, vars);
        if (!out.summary.ok) error = out.summary.error ?? "failed";
      } catch (e) {
        if (e instanceof CancelledError) return;
        error = String(e);
      }
      if (st.cancelled) return;
      const series: Record<string, Float64Array> = {};
      if (out) for (const [n, y] of out.series) series[n] = y;
      const run = addRun({
        label: `${st.label} #${String(i + 1).padStart(width, "0")}`,
        model,
        params,
        options,
        ok: !error,
        error,
        sweep: { id: st.id, index: i },
        worker: out?.worker,
        t: out?.time ?? new Float64Array(),
        series,
        units,
      });
      st.runs.push(run);
      if (out) st.workers.add(out.worker);
      st.done++;
      if (error) st.failed++;
      onProgress(st);
    }),
  );
  st.finished = performance.now();
  st.runs.sort((a, b) => a.sweep!.index - b.sweep!.index);
  if (active === st) active = null;
  return st;
}

export function cancelSweep() {
  if (!active) return;
  active.cancelled = true;
  pool().cancel();
}

// ---------------------------------------------------------------- UI
function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else e.setAttribute(k, v);
  }
  e.append(...kids);
  return e;
}

const esc = (s: string) => s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);

let host: Host;

function runTitle(r: Run): string {
  const lines = [
    `${r.label} — ${r.model}`,
    new Date(r.created).toLocaleString(),
    r.ok ? "ok" : `failed: ${r.error ?? ""}`,
    ...r.params.map(([n, v]) => `${n} = ${fmt(v)}`),
  ];
  const o = r.options;
  const opts = [o.stop_time !== undefined ? `stop ${o.stop_time}` : "", o.tolerance ? `tol ${o.tolerance}` : "", o.method ?? "", o.intervals ? `${o.intervals} intervals` : ""].filter(Boolean);
  if (opts.length) lines.push(opts.join(", "));
  const vs = Object.keys(r.series);
  lines.push(vs.length ? `kept: ${vs.slice(0, 8).join(", ")}${vs.length > 8 ? ", …" : ""}` : "no variables kept");
  if (r.worker !== undefined) lines.push(`pool worker ${r.worker + 1}`);
  return lines.join("\n");
}

function renderRuns() {
  const list = document.getElementById("runs-list");
  if (!list) return;
  list.innerHTML = "";
  // Pinned first, then newest first; the runs of one sweep in reverse run order.
  const sorted = [...runs].sort(
    (a, b) => Number(b.pinned) - Number(a.pinned) || (a.sweep && a.sweep.id === b.sweep?.id ? b.sweep.index - a.sweep.index : b.created - a.created),
  );
  if (!sorted.length) list.append(h("div", { class: "muted runs-empty" }, "Every simulation is kept here. Select runs to overlay or subtract them."));
  for (const r of sorted) {
    const row = h("div", { class: `run-row${r.ok ? "" : " failed"}${selected.has(r.id) ? " sel" : ""}`, title: runTitle(r), "data-run": r.id });
    const cb = h("input", { type: "checkbox" });
    cb.checked = selected.has(r.id);
    cb.onchange = () => {
      if (cb.checked) selected.add(r.id);
      else selected.delete(r.id);
      renderRuns();
    };
    const label = h("span", { class: "run-label" }, r.label);
    const ps = r.params.map(([n, v]) => `${n.split(".").pop()}=${fmt(v)}`).join(" ");
    const sub = h("span", { class: "run-sub" }, `${r.model.split(".").pop()}${ps ? ` · ${ps}` : ""}`);
    const text = h("span", { class: "run-text" }, label, sub);
    const rename = () => {
      const inp = h("input", { class: "run-rename", value: r.label });
      text.replaceChildren(inp);
      inp.focus();
      inp.select();
      let doneEdit = false;
      const commit = (keep: boolean) => {
        if (doneEdit) return;
        doneEdit = true;
        const v = inp.value.trim();
        if (keep && v && v !== r.label) updateRun(r.id, { label: v });
        else renderRuns();
      };
      inp.onkeydown = (e) => {
        if (e.key === "Enter") commit(true);
        if (e.key === "Escape") commit(false);
      };
      inp.onblur = () => commit(true);
    };
    label.ondblclick = rename;
    const pin = h("button", { class: `icon${r.pinned ? " on" : ""}`, title: r.pinned ? "Unpin" : "Pin (kept when old runs are dropped)" }, r.pinned ? "★" : "☆");
    pin.onclick = () => updateRun(r.id, { pinned: !r.pinned });
    const ren = h("button", { class: "icon", title: "Rename" }, "✎");
    ren.onclick = rename;
    const link = h("button", { class: "icon", title: "Copy a link that reopens and runs this" }, "🔗");
    link.onclick = () => copyLink(r.model, r.params, r.options, Object.keys(r.series));
    const del = h("button", { class: "icon", title: "Delete" }, "×");
    del.onclick = () => deleteRun(r.id);
    row.append(cb, text, pin, ren, link, del);
    list.append(row);
  }
  const n = selected.size;
  const info = document.getElementById("runs-info");
  if (info) info.textContent = n ? `${n} selected` : `${runs.length} runs`;
  document.getElementById("runs-pane")?.classList.toggle("collapsed", runs.length === 0);
  (document.getElementById("runs-overlay") as HTMLButtonElement | null)?.toggleAttribute("disabled", n < 1);
  (document.getElementById("runs-diff") as HTMLButtonElement | null)?.toggleAttribute("disabled", n !== 2);
}

function selectedRuns(): Run[] {
  return runs.filter((r) => selected.has(r.id)).sort((a, b) => a.created - b.created);
}

async function copyLink(model: string, params: [string, number][], options: SimSettings, vars: string[]) {
  const url = shareUrl(model, params, options, vars);
  host.log(`Link: ${url}`, "ok");
  try {
    await navigator.clipboard.writeText(url);
    host.log("Link copied to the clipboard.");
  } catch {
    // no clipboard access (permissions, insecure context): the link is in the log
  }
  return url;
}

function buildRunsPane() {
  const right = document.getElementById("right")!;
  const pane = h("section", { class: "pane", id: "runs-pane" });
  const sweepBtn = h("button", { id: "runs-sweep", title: "Parameter sweep over the worker pool" }, "Sweep…");
  sweepBtn.onclick = () => openSweepDialog();
  pane.classList.add("collapsed");
  pane.append(h("div", { class: "pane-title" }, "Runs", h("span", { id: "runs-info", class: "runs-info" }), sweepBtn));
  const ov = h("button", { id: "runs-overlay", title: "Plot the variables of the active subplot for every selected run" }, "Overlay");
  ov.onclick = () => {
    const rs = selectedRuns();
    host.showPlot();
    const n = overlay(host.plot, rs);
    if (!n) host.log("The selected runs kept none of the plotted variables.", "warn");
  };
  const df = h("button", { id: "runs-diff", title: "Plot the newer selected run minus the older one (interpolated on a common time grid)" }, "Diff B − A");
  df.onclick = () => {
    const [a, b] = selectedRuns();
    host.showPlot();
    if (!diff(host.plot, a, b)) host.log("The two runs share none of the plotted variables.", "warn");
  };
  const clr = h("button", { id: "runs-clear", title: "Remove run overlays from the plot" }, "Clear overlays");
  clr.onclick = () => clearOverlays(host.plot);
  const share = h("button", { id: "runs-share", title: "Copy a link that reopens this model with its overrides and settings, runs it and plots the plotted variables" }, "Share link");
  share.onclick = () => {
    const m = host.model();
    if (!m) return;
    const { params, ...options } = host.settings();
    const vars = [...new Set(host.plot.subplots.flatMap((sp) => sp.series.filter((s) => !s.run).map((s) => s.name)))];
    void copyLink(m, params ?? [], options, vars);
  };
  const sel = h("button", { id: "runs-none", title: "Select none" }, "None");
  sel.onclick = () => {
    selected.clear();
    renderRuns();
  };
  pane.append(h("div", { class: "runs-tools" }, ov, df, clr, sel, share));
  pane.append(h("div", { id: "runs-list", class: "runs-list" }));
  right.append(pane);
}

// ---- sweep dialog
let dialog: HTMLDialogElement | null = null;
let scalarPlot: PlotWindow | null = null;

interface ParamRow {
  name: HTMLSelectElement;
  spacing: HTMLSelectElement;
  list: HTMLInputElement;
  a: HTMLInputElement;
  b: HTMLInputElement;
  n: HTMLInputElement;
}
let rows: ParamRow[] = [];

function editableParams(): { name: string; value: number | null }[] {
  const s = host.summary();
  const m = host.model();
  if (!s || s.model !== m) return [];
  return s.summary.vars.filter((v) => v.kind === "param" && v.editable).map((v) => ({ name: v.name, value: v.value }));
}

function rowValues(r: ParamRow): number[] {
  return sweepValues(r.spacing.value as Spacing, r.list.value, parseFloat(r.a.value), parseFloat(r.b.value), parseInt(r.n.value, 10));
}

function addParamRow(name?: string) {
  const params = editableParams();
  const tbody = dialog!.querySelector("#sw-params tbody")!;
  const sel = h("select", { class: "sw-name" });
  for (const p of params) sel.append(h("option", { value: p.name }, p.name));
  const used = new Set(rows.map((r) => r.name.value));
  sel.value = name ?? params.find((p) => !used.has(p.name))?.name ?? params[0]?.name ?? "";
  const spacing = h("select", { class: "sw-spacing" }, h("option", { value: "list" }, "list"), h("option", { value: "lin" }, "linear"), h("option", { value: "log" }, "log"));
  const list = h("input", { class: "sw-list", placeholder: "1, 2, 3" });
  const a = h("input", { class: "sw-a", type: "number", step: "any", placeholder: "from" });
  const b = h("input", { class: "sw-b", type: "number", step: "any", placeholder: "to" });
  const n = h("input", { class: "sw-n", type: "number", min: "1", value: "3", title: "number of values" });
  const r: ParamRow = { name: sel, spacing, list, a, b, n };
  const prefill = () => {
    const v = params.find((p) => p.name === sel.value)?.value ?? 1;
    const base = v === 0 ? 1 : v;
    list.value = [0.5, 1, 2].map((k) => fmt(k * base)).join(", ");
    a.value = fmt(base / 2);
    b.value = fmt(base * 2);
  };
  prefill();
  sel.onchange = () => {
    prefill();
    updateCount();
  };
  const show = () => {
    const isList = spacing.value === "list";
    list.classList.toggle("hidden", !isList);
    for (const x of [a, b, n]) x.classList.toggle("hidden", isList);
    updateCount();
  };
  spacing.onchange = show;
  for (const x of [list, a, b, n]) x.oninput = updateCount;
  const del = h("button", { class: "icon", title: "Remove" }, "×");
  const tr = h("tr", {}, h("td", {}, sel), h("td", {}, spacing), h("td", { class: "sw-vals" }, list, a, b, n), h("td", {}, del));
  del.onclick = () => {
    rows = rows.filter((x) => x !== r);
    tr.remove();
    updateCount();
  };
  tbody.append(tr);
  rows.push(r);
  show();
}

function sweepPoints(): [string, number][][] {
  const grouped = (dialog!.querySelector("input[name=sw-combine]:checked") as HTMLInputElement)?.value === "grouped";
  const ps = rows.map((r) => ({ name: r.name.value, values: rowValues(r) }));
  if (ps.some((p) => !p.name || !p.values.length)) throw new Error("every parameter needs at least one value");
  if (new Set(ps.map((p) => p.name)).size !== ps.length) throw new Error("a parameter is swept twice");
  return expandSweep(ps, grouped);
}

function updateCount() {
  const el = dialog?.querySelector("#sw-count");
  if (!el) return;
  try {
    const n = sweepPoints().length;
    el.textContent = `${n} run${n === 1 ? "" : "s"} on up to ${Math.min(pool().size, n)} parallel workers`;
    el.classList.remove("err");
  } catch (e) {
    el.textContent = (e as Error).message;
    el.classList.add("err");
  }
}

function buildDialog() {
  dialog = h("dialog", { id: "sweep-dialog", class: "sweep-dialog" });
  dialog.innerHTML = `
    <form method="dialog" class="sw-head"><h3>Parameter sweep <span id="sw-model" class="muted"></span></h3><button class="icon" value="close" title="Close">×</button></form>
    <div class="sw-body">
      <div id="sw-setup">
        <table id="sw-params" class="sw-table"><thead><tr><th>Parameter</th><th>Values</th><th></th><th></th></tr></thead><tbody></tbody></table>
        <div class="sw-line"><button id="sw-add">+ Parameter</button>
          <span class="sw-combine"><label><input type="radio" name="sw-combine" value="grid" checked /> Grid (every combination)</label>
          <label><input type="radio" name="sw-combine" value="grouped" /> Grouped (n-th values together)</label></span></div>
        <div class="sw-line"><label class="sw-keep">Keep variables <input id="sw-vars" list="sw-var-list" placeholder="a.b, c" /></label><datalist id="sw-var-list"></datalist></div>
        <div class="sw-line"><span id="sw-count" class="muted"></span><span class="spacer"></span>
          <button id="sw-run" class="primary">▶ Run sweep</button><button id="sw-cancel" disabled>Cancel</button></div>
        <div id="sw-progress" class="sw-progress hidden"><div class="track"><div class="fill"></div></div><div class="label"></div></div>
      </div>
      <div id="sw-results" class="hidden">
        <div class="sw-line"><label>Result <select id="sw-stat"><option value="final">final value</option><option value="max">max</option><option value="min">min</option><option value="mean">time average</option><option value="initial">initial value</option></select></label>
          <label>of <select id="sw-var"></select></label><label>vs <select id="sw-x"></select></label></div>
        <div id="sw-scalar" class="sw-scalar plot-host"></div>
        <div class="sw-table-wrap"><table id="sw-summary" class="sw-table sw-summary"></table></div>
      </div>
    </div>`;
  document.body.append(dialog);
  dialog.querySelector<HTMLButtonElement>("#sw-add")!.onclick = () => {
    addParamRow();
    updateCount();
  };
  dialog.querySelectorAll<HTMLInputElement>("input[name=sw-combine]").forEach((r) => (r.onchange = updateCount));
  dialog.querySelector<HTMLButtonElement>("#sw-run")!.onclick = () => void startSweep();
  dialog.querySelector<HTMLButtonElement>("#sw-cancel")!.onclick = () => cancelSweep();
  for (const id of ["#sw-stat", "#sw-var", "#sw-x"]) dialog.querySelector<HTMLSelectElement>(id)!.onchange = () => lastSweep && renderSweepResults(lastSweep);
}

export function openSweepDialog() {
  const m = host.model();
  if (!m) {
    host.log("Open a model first.", "warn");
    return;
  }
  const s = host.summary();
  if (!s || s.model !== m) {
    host.log(`Simulate ${m} once first: the sweep dialog lists its parameters.`, "warn");
    return;
  }
  if (!dialog) buildDialog();
  const d = dialog!;
  d.querySelector("#sw-model")!.textContent = m;
  const params = editableParams();
  // A new model starts a fresh setup; the same model keeps the last one.
  if (d.dataset.model !== m) {
    d.dataset.model = m;
    rows = [];
    d.querySelector("#sw-params tbody")!.innerHTML = "";
    if (params.length) addParamRow();
    const plotted = [...new Set(host.plot.subplots.flatMap((sp) => sp.series.filter((x) => !x.run).map((x) => x.name)))];
    const series = s.summary.vars.filter((v) => v.kind !== "param").map((v) => v.name);
    (d.querySelector("#sw-vars") as HTMLInputElement).value = (plotted.length ? plotted : series.slice(0, 1)).join(", ");
    d.querySelector("#sw-var-list")!.innerHTML = series.slice(0, 3000).map((n) => `<option value="${esc(n)}">`).join("");
    d.querySelector("#sw-results")!.classList.add("hidden");
  }
  if (!params.length) host.log(`${m} has no editable parameters to sweep.`, "warn");
  updateCount();
  d.showModal();
}

async function startSweep() {
  const d = dialog!;
  const m = d.dataset.model!;
  let points: [string, number][][];
  try {
    points = sweepPoints();
  } catch (e) {
    host.log(String((e as Error).message), "err");
    return;
  }
  const s = host.summary();
  const known = new Set(s?.summary.vars.map((v) => v.name));
  const vars = (d.querySelector("#sw-vars") as HTMLInputElement).value
    .split(",")
    .map((x) => x.trim())
    .filter((x) => x && known.has(x));
  if (!vars.length) {
    host.log("Name at least one variable to keep (Keep variables).", "err");
    return;
  }
  const runBtn = d.querySelector<HTMLButtonElement>("#sw-run")!;
  const cancelBtn = d.querySelector<HTMLButtonElement>("#sw-cancel")!;
  const prog = d.querySelector<HTMLElement>("#sw-progress")!;
  const fill = prog.querySelector<HTMLElement>(".fill")!;
  const label = prog.querySelector<HTMLElement>(".label")!;
  runBtn.disabled = true;
  cancelBtn.disabled = false;
  prog.classList.remove("hidden");
  fill.style.width = "0%";
  label.textContent = `Starting ${points.length} runs on ${Math.min(pool().size, points.length)} workers…`;
  host.log(`Sweep of ${m}: ${points.length} runs on up to ${Math.min(pool().size, points.length)} workers`);
  const st = await runSweep(host, m, points, vars, (st) => {
    fill.style.width = `${((100 * st.done) / st.total).toFixed(1)}%`;
    label.textContent = `${st.done} / ${st.total} runs${st.failed ? `, ${st.failed} failed` : ""} · ${st.workers.size} workers · ${((performance.now() - st.started) / 1000).toFixed(1)} s`;
    renderSweepResults(st);
  });
  runBtn.disabled = false;
  cancelBtn.disabled = true;
  const secs = ((st.finished! - st.started) / 1000).toFixed(1);
  if (st.cancelled) {
    label.textContent = `Cancelled after ${st.done} of ${st.total} runs`;
    host.log(`Sweep cancelled after ${st.done} of ${st.total} runs`, "warn");
  } else {
    label.textContent = `${st.done} runs in ${secs} s on ${st.workers.size} workers (at most ${pool().peak} at once)${st.failed ? `, ${st.failed} failed` : ""}`;
    host.log(`${st.label} of ${m}: ${st.done - st.failed}/${st.total} runs ok in ${secs} s on ${st.workers.size} workers`, st.failed ? "warn" : "ok");
  }
  renderSweepResults(st);
  // Overlay the kept variables of the sweep's runs in the plot.
  selected.clear();
  for (const r of st.runs) if (r.ok) selected.add(r.id);
  renderRuns();
  if (st.runs.some((r) => r.ok)) {
    host.showPlot();
    overlay(host.plot, st.runs.filter((r) => r.ok), vars);
  }
}

function renderSweepResults(st: SweepState) {
  const d = dialog!;
  d.querySelector("#sw-results")!.classList.remove("hidden");
  const statSel = d.querySelector<HTMLSelectElement>("#sw-stat")!;
  const varSel = d.querySelector<HTMLSelectElement>("#sw-var")!;
  const xSel = d.querySelector<HTMLSelectElement>("#sw-x")!;
  if (varSel.dataset.sweep !== st.id) {
    varSel.dataset.sweep = st.id;
    varSel.innerHTML = st.vars.map((v) => `<option>${esc(v)}</option>`).join("");
    xSel.innerHTML = st.params.map((v) => `<option>${esc(v)}</option>`).join("");
  }
  const s = statSel.value as Stat;
  const v = varSel.value;
  const xp = xSel.value;
  const runsSorted = [...st.runs].sort((a, b) => a.sweep!.index - b.sweep!.index);
  // Summary table: parameters, status and the chosen statistic of every kept variable.
  const head = `<tr><th>Run</th>${st.params.map((p) => `<th>${esc(p)}</th>`).join("")}<th>Status</th>${st.vars.map((x) => `<th>${esc(s)}(${esc(x)})</th>`).join("")}<th>Worker</th></tr>`;
  const body = runsSorted
    .map((r) => {
      const ps = st.params.map((p) => `<td>${fmt(r.params.find((q) => q[0] === p)?.[1] ?? NaN)}</td>`).join("");
      const vs = st.vars.map((x) => `<td>${r.ok && r.series[x] ? fmt(stat(r.t, r.series[x], s)) : ""}</td>`).join("");
      return `<tr class="${r.ok ? "" : "failed"}" title="${esc(r.error ?? "")}"><td>${r.sweep!.index + 1}</td>${ps}<td>${r.ok ? "ok" : "fail"}</td>${vs}<td>${r.worker !== undefined ? r.worker + 1 : ""}</td></tr>`;
    })
    .join("");
  d.querySelector("#sw-summary")!.innerHTML = `<thead>${head}</thead><tbody>${body}</tbody>`;
  // Scalar vs parameter: one curve per combination of the other parameters.
  const host2 = d.querySelector<HTMLElement>("#sw-scalar")!;
  scalarPlot ??= new PlotWindow(host2);
  const groups = new Map<string, { x: number; y: number }[]>();
  for (const r of runsSorted) {
    if (!r.ok || !r.series[v]) continue;
    const x = r.params.find((q) => q[0] === xp)?.[1];
    if (x === undefined) continue;
    const key = st.params
      .filter((p) => p !== xp)
      .map((p) => `${p}=${fmt(r.params.find((q) => q[0] === p)?.[1] ?? NaN)}`)
      .join(", ");
    if (!groups.has(key)) groups.set(key, []);
    groups.get(key)!.push({ x, y: stat(r.t, r.series[v], s) });
  }
  scalarPlot.clear();
  const sp = scalarPlot.subplots[0];
  sp.xName = xp;
  for (const [key, pts] of groups) {
    pts.sort((a, b) => a.x - b.x);
    sp.series.push({
      name: `${s}(${v})${key ? ` · ${key}` : ""}`,
      unit: "",
      x: Float64Array.from(pts.map((p) => p.x)),
      y: Float64Array.from(pts.map((p) => p.y)),
      color: scalarPlot.nextColor(),
    });
  }
  requestAnimationFrame(() => scalarPlot!.draw());
}

/** Build the Runs pane and load stored runs. */
export function initExperiments(h0: Host) {
  host = h0;
  buildRunsPane();
  listeners.push(renderRuns);
  renderRuns();
  void loadRuns();
}
