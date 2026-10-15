// Performance panel: what a profiled simulation spent its time on and why it took the
// steps it took (Dymola's Simulation Analysis, Amesim's Performance Analyzer). Shows the
// step size over time with the rejected steps, the states limiting the step, the cost of
// each block of the equations, the relations that switched (chattering highlighted) and
// the event log. Data comes from `Workspace::profile` (crates/daedalus-core/src/profile.rs).
import "./performance.css";
import { api } from "./rpc";
import { PlotWindow } from "./plot";

export interface Profile {
  model: string;
  method: string;
  states: string[];
  timing: { total_ms: number; init_ms: number; integration_ms: number; events_ms: number; output_ms: number };
  solver: { steps: number; rejected: number; newton_failures: number; rhs_evals: number; jac_evals: number; lu_factorizations: number };
  events_total: number;
  state_events: number;
  newton_solves: number;
  newton_iterations: number;
  steps: { t: number[]; h: number[]; order: number[]; err: (number | null)[]; dominant: number[] };
  steps_total: number;
  rejected: { t: number; h: number | null; reason: "error" | "newton" }[];
  rejected_total: number;
  limiting: { index: number; name: string; steps: number; share: number; rms_error: number; max_error: number }[];
  blocks: {
    kind: string;
    index: number;
    label: string;
    origin: string;
    vars: string[];
    eqs: string[];
    calls: number;
    iterations: number;
    failures: number;
    time_ms: number;
    share: number;
  }[];
  events: {
    t: number;
    kind: "state" | "time";
    tags: string[];
    relations: { index: number; value: boolean }[];
    text: string;
    origin: string;
    whens: string[];
    changes: { name: string; before: number | null; after: number | null }[];
    n_changes: number;
  }[];
  relations: {
    index: number;
    text: string;
    origin: string;
    time_event: boolean;
    count: number;
    rising: number;
    first: number;
    last: number;
    min_interval: number | null;
    chattering: { count: number; from: number; to: number } | null;
  }[];
  chatter_rule: { count: number; max_gap: number };
  hints: string[];
}

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

function h<K extends keyof HTMLElementTagNameMap>(tag: K, attrs: Record<string, string> = {}, ...kids: (Node | string)[]) {
  const e = document.createElement(tag);
  for (const [k, v] of Object.entries(attrs)) {
    if (k === "class") e.className = v;
    else e.setAttribute(k, v);
  }
  for (const k of kids) e.append(k);
  return e;
}

const num = (n: number) => n.toLocaleString("en-US");
const ms = (x: number) => (x >= 100 ? x.toFixed(0) : x >= 1 ? x.toFixed(1) : x.toFixed(3));
const pct = (x: number) => `${(100 * x).toFixed(x < 0.1 ? 1 : 0)}%`;
const fmt = (v: number | null) => {
  if (v === null || v === undefined) return "–";
  const a = Math.abs(v);
  if (a !== 0 && (a >= 1e5 || a < 1e-3)) return v.toExponential(3);
  return String(+v.toPrecision(7));
};

/** Colours of the dominant states (the rest are grey). */
const STATE_COLORS = ["#0b5fff", "#d9480f", "#2b8a3e", "#ae3ec9", "#1098ad", "#c2255c", "#5c940d", "#e8590c"];

let mainPlot: PlotWindow | null = null;
let stepPlot: PlotWindow | null = null;
let profile: Profile | null = null;
let selectedEvent = -1;
let eventLimit = 500;

/** Whether the next simulation records a profile (the "Profile" checkbox). */
export function profileRequested(): boolean {
  return ($("set-profile") as HTMLInputElement | null)?.checked ?? false;
}

/** Add the profiling checkbox to the simulation setup and the Performance tab to the plot pane. */
export function initPerformance(plot: PlotWindow) {
  mainPlot = plot;
  const box = h("input", { type: "checkbox", id: "set-profile" }) as HTMLInputElement;
  const cb = h("label", { title: "Record a performance profile and event log with the next simulation (costs some speed)" }, box, "Profile");
  try {
    box.checked = localStorage.getItem("daedalus.profile") === "1";
  } catch {
    /* storage unavailable */
  }
  box.onchange = () => {
    try {
      localStorage.setItem("daedalus.profile", box.checked ? "1" : "0");
    } catch {
      /* storage unavailable */
    }
  };
  $("btn-sim").before(cb);

  // The Performance tab joins the Plot | Animation switch of the results pane.
  const pane = $("plot-pane");
  const views = $("sim-views");
  const tPerf = h("button", { class: "perf-tab hidden", "data-ptab": "perf", id: "perf-tab", title: "Performance profile and event log of the last simulation" }, "Performance");
  views.append(tPerf);
  const view = h("div", { id: "perf-view", class: "perf hidden" });
  pane.append(view);
  tPerf.onclick = () => showTab("perf");
  // Plot and Animation (handled in main.ts) leave the performance view.
  views.addEventListener("click", (e) => {
    if ((e.target as HTMLElement).hasAttribute("data-sim")) showTab("plot");
  });
}

function showTab(which: "plot" | "perf") {
  const pane = $("plot-pane");
  pane.classList.toggle("perf-mode", which === "perf");
  $("perf-tab").classList.toggle("active", which === "perf");
  if (which === "perf") $("sim-views").querySelectorAll("button[data-sim]").forEach((b) => b.classList.remove("active"));
  else if (!$("sim-views").querySelector("button[data-sim].active")) $("sim-views").querySelector("button[data-sim]")?.classList.add("active");
  $("perf-view").classList.toggle("hidden", which !== "perf");
  if (which === "perf") requestAnimationFrame(() => stepPlot?.draw());
  else requestAnimationFrame(() => mainPlot?.draw());
}

/** After a simulation: fetch and show its profile, or hide the tab when it was not profiled. */
export async function updatePerformance(requested: boolean) {
  profile = requested ? await api.profile() : null;
  const tab = $("perf-tab");
  tab.classList.toggle("hidden", !profile);
  if (mainPlot) {
    mainPlot.markers = [];
    mainPlot.draw();
  }
  if (!profile) {
    showTab("plot");
    $("perf-view").replaceChildren();
    stepPlot = null;
    return;
  }
  selectedEvent = -1;
  eventLimit = 500;
  render(profile);
}

function section(title: string, ...kids: (Node | string)[]) {
  return h("section", { class: "perf-sec" }, h("h3", {}, title), ...kids);
}

function render(p: Profile) {
  const view = $("perf-view");
  view.replaceChildren();
  view.append(summary(p), stepSection(p), limitingSection(p), blocksSection(p), relationsSection(p), eventsSection(p));
  requestAnimationFrame(() => stepPlot?.draw());
}

function summary(p: Profile) {
  const t = p.timing;
  const parts: [string, number, string][] = [
    ["initialization", t.init_ms, "#868e96"],
    ["integration", t.integration_ms, "#0b5fff"],
    ["events", t.events_ms, "#d9480f"],
    ["output", t.output_ms, "#2b8a3e"],
  ];
  const total = Math.max(t.total_ms, 1e-9);
  const bar = h("div", { class: "perf-split" });
  const legend = h("div", { class: "perf-legend" });
  for (const [name, v, c] of parts) {
    const seg = h("span", { title: `${name}: ${ms(v)} ms (${pct(v / total)})` });
    seg.style.width = `${(100 * v) / total}%`;
    seg.style.background = c;
    bar.append(seg);
    const sw = h("i");
    sw.style.background = c;
    legend.append(h("span", {}, sw, `${name} ${ms(v)} ms (${pct(v / total)})`));
  }
  const s = p.solver;
  const nNewton = p.rejected.filter((r) => r.reason === "newton").length;
  const kv = (k: string, v: string) => h("div", { class: "kv" }, h("span", { class: "k" }, k), h("span", { class: "v" }, v));
  const stats = h(
    "div",
    { class: "perf-stats" },
    kv("total", `${ms(t.total_ms)} ms`),
    kv("method", p.method),
    kv("steps", num(s.steps)),
    kv("rejected", `${num(p.rejected_total)} (${num(nNewton)} Newton)`),
    kv("f-evals", num(s.rhs_evals)),
    kv("Jacobians", num(s.jac_evals)),
    kv("events", `${num(p.events_total)} (${num(p.state_events)} state)`),
    kv("nonlinear solves", `${num(p.newton_solves)} / ${num(p.newton_iterations)} it`),
  );
  const hints = h("ul", { class: "perf-hints" });
  for (const x of p.hints) hints.append(h("li", {}, x));
  if (!p.hints.length) hints.append(h("li", { class: "muted" }, "Nothing stands out."));
  return section(`Summary · ${p.model}`, bar, legend, stats, hints);
}

function stepSection(p: Profile) {
  const host = h("div", { class: "perf-plot" });
  const strip = h("canvas", { class: "perf-strip", title: "State with the largest error term in each step" }) as HTMLCanvasElement;
  const sec = section("Step size", host, strip);
  stepPlot = new PlotWindow(host);
  stepPlot.emptyText = "No steps recorded";
  const t = Float64Array.from(p.steps.t);
  const hs = Float64Array.from(p.steps.h);
  const rj = p.rejected.filter((r) => r.h !== null && r.h > 0);
  stepPlot.subplots = [
    {
      logY: true,
      series: [
        { name: "step size h", unit: "s", x: t, y: hs, color: "#0b5fff", displayUnit: "s" },
        ...(rj.length
          ? [{ name: `rejected (${num(p.rejected_total)})`, unit: "s", x: Float64Array.from(rj.map((r) => r.t)), y: Float64Array.from(rj.map((r) => r.h!)), color: "#e03131", points: true, displayUnit: "s" }]
          : []),
      ],
    },
  ];
  if (p.rejected_total > rj.length) {
    sec.append(h("p", { class: "muted" }, `${num(p.rejected_total - rj.length)} rejected steps without a reported size (integrator without diagnostics) are not drawn.`));
  }
  // Dominant state per step, as a strip under the plot over the whole run.
  const color = new Map<number, string>();
  p.limiting.slice(0, STATE_COLORS.length).forEach((l, k) => color.set(l.index, STATE_COLORS[k]));
  const drawStrip = () => {
    const dpr = window.devicePixelRatio || 1;
    const W = strip.clientWidth;
    const H = strip.clientHeight;
    if (!W) return;
    strip.width = W * dpr;
    strip.height = H * dpr;
    const g = strip.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, W, H);
    const n = p.steps.t.length;
    if (!n) return;
    const left = 64;
    const width = Math.max(10, W - 64 - 16);
    const t0 = n ? p.steps.t[0] - p.steps.h[0] : 0;
    const t1 = p.steps.t[n - 1];
    const X = (x: number) => left + ((x - t0) / Math.max(t1 - t0, 1e-300)) * width;
    for (let i = 0; i < n; i++) {
      const d = p.steps.dominant[i];
      g.fillStyle = d < 0 ? "transparent" : (color.get(d) ?? "#adb5bd");
      const a = X(p.steps.t[i] - p.steps.h[i]);
      g.fillRect(a, 0, Math.max(1, X(p.steps.t[i]) - a), H);
    }
  };
  new ResizeObserver(drawStrip).observe(strip);
  return sec;
}

function limitingSection(p: Profile) {
  const list = h("div", { class: "perf-bars" });
  if (!p.limiting.length) list.append(h("p", { class: "muted" }, "The integrator reported no error estimates (fixed-step method)."));
  p.limiting.slice(0, 12).forEach((l, k) => {
    const fill = h("span", { class: "fill" });
    fill.style.width = `${100 * l.share}%`;
    fill.style.background = STATE_COLORS[k] ?? "#adb5bd";
    list.append(
      h(
        "div",
        { class: "bar-row", title: `${l.name}: largest error term in ${num(l.steps)} steps; rms scaled error ${fmt(l.rms_error)}, max ${fmt(l.max_error)}` },
        h("span", { class: "name" }, l.name),
        h("span", { class: "track" }, fill),
        h("span", { class: "val" }, pct(l.share)),
      ),
    );
  });
  return section("States limiting the step", h("p", { class: "muted" }, "Share of steps in which the state had the largest scaled error term; the strip above shows it over time."), list);
}

function blocksSection(p: Profile) {
  const table = h("table", { class: "perf-table" });
  table.append(
    h("thead", {}, h("tr", {}, ...["Block", "Component", "Calls", "Iter/call", "Failures", "Time [ms]", "Share"].map((c) => h("th", {}, c)))),
  );
  const body = h("tbody");
  for (const b of p.blocks) {
    const tr = h("tr", { class: `blk ${b.kind}` });
    const share = h("span", { class: "share" }, h("span", { class: "fill" }));
    (share.firstChild as HTMLElement).style.width = `${Math.min(100, 100 * b.share)}%`;
    const isNewton = b.kind === "linear" || b.kind === "nonlinear";
    tr.append(
      h("td", {}, b.label),
      h("td", { class: "mono" }, b.origin),
      h("td", { class: "r" }, num(b.calls)),
      h("td", { class: "r" }, isNewton ? (b.iterations / Math.max(b.calls, 1)).toFixed(2) : ""),
      h("td", { class: `r${b.failures ? " bad" : ""}` }, isNewton ? num(b.failures) : ""),
      h("td", { class: "r" }, ms(b.time_ms)),
      h("td", {}, share, ` ${pct(b.share)}`),
    );
    if (b.vars.length || b.eqs.length) {
      tr.title = [`unknowns: ${b.vars.join(", ")}`, ...b.eqs].join("\n");
      tr.classList.add("expandable");
      const detail = h("tr", { class: "detail hidden" }, h("td", { colspan: "7" }, h("div", { class: "mono" }, `unknowns: ${b.vars.join(", ")}`), ...b.eqs.map((e) => h("div", { class: "mono" }, e))));
      tr.onclick = () => detail.classList.toggle("hidden");
      body.append(tr, detail);
    } else body.append(tr);
  }
  table.append(body);
  return section("Time per block", h("p", { class: "muted" }, "Algebraic loops (Newton blocks), function loops and the rest of the equations, timed over the whole run. Click a block for its equations."), table);
}

function relationsSection(p: Profile) {
  if (!p.relations.length) return section("Relations", h("p", { class: "muted" }, "No relation switched."));
  const table = h("table", { class: "perf-table" });
  table.append(h("thead", {}, h("tr", {}, ...["Relation", "Component", "Switches", "First", "Last", "Min interval", ""].map((c) => h("th", {}, c)))));
  const body = h("tbody");
  for (const r of p.relations) {
    const c = r.chattering;
    const tr = h(
      "tr",
      { class: c ? "chatter" : "" },
      h("td", { class: "mono" }, r.text),
      h("td", { class: "mono" }, r.origin),
      h("td", { class: "r" }, num(r.count)),
      h("td", { class: "r" }, fmt(r.first)),
      h("td", { class: "r" }, fmt(r.last)),
      h("td", { class: "r" }, fmt(r.min_interval)),
      h("td", {}, c ? h("span", { class: "badge", title: `${num(c.count)} switches between t = ${fmt(c.from)} and ${fmt(c.to)}` }, "chattering") : r.time_event ? h("span", { class: "muted" }, "time") : ""),
    );
    if (c) {
      tr.onclick = () => mark(c.from);
      tr.style.cursor = "pointer";
    }
    body.append(tr);
  }
  table.append(body);
  return section("Relations", table);
}

function eventsSection(p: Profile) {
  const chattering = new Set(p.relations.filter((r) => r.chattering).map((r) => r.index));
  const filter = h("input", { type: "search", placeholder: "Filter events", class: "perf-filter" }) as HTMLInputElement;
  const kind = h("select", {}, h("option", { value: "" }, "all kinds"), h("option", { value: "state" }, "state events"), h("option", { value: "time" }, "time events")) as HTMLSelectElement;
  const onlyChat = h("input", { type: "checkbox" }) as HTMLInputElement;
  const bar = h("div", { class: "perf-toolbar" }, filter, kind, h("label", {}, onlyChat, "chattering only"));
  const count = h("span", { class: "muted" });
  bar.append(count);
  const table = h("table", { class: "perf-table events" });
  table.append(h("thead", {}, h("tr", {}, ...["#", "Time", "Kind", "Action", "Relation", "Component", "Changes"].map((c) => h("th", {}, c)))));
  const body = h("tbody");
  table.append(body);
  const more = h("button", { class: "hidden" }, "Show more");
  const fill = () => {
    const q = filter.value.trim().toLowerCase();
    const rows: HTMLElement[] = [];
    let shown = 0;
    let matched = 0;
    p.events.forEach((e, i) => {
      if (kind.value && e.kind !== kind.value) return;
      const chat = e.relations.some((r) => chattering.has(r.index));
      if (onlyChat.checked && !chat) return;
      const changes = e.changes.map((c) => `${c.name}: ${fmt(c.before)} → ${fmt(c.after)}`).join(", ") + (e.n_changes > e.changes.length ? ` (+${e.n_changes - e.changes.length})` : "");
      const action = [...e.whens].join("; ");
      if (q && ![e.text, e.origin, action, changes, e.tags.join(" ")].some((s) => s.toLowerCase().includes(q))) return;
      matched++;
      if (shown >= eventLimit) return;
      shown++;
      const rel = e.text ? `${e.text} → ${e.relations[0]?.value}${e.relations.length > 1 ? ` (+${e.relations.length - 1})` : ""}` : "";
      const tr = h(
        "tr",
        { class: `${chat ? "chatter" : ""}${i === selectedEvent ? " sel" : ""}`, "data-i": String(i) },
        h("td", { class: "r muted" }, String(i + 1)),
        h("td", { class: "r mono" }, String(+e.t.toPrecision(10))),
        h("td", {}, e.kind),
        h("td", {}, e.tags.join(", ") + (action ? ` · ${action}` : "")),
        h("td", { class: "mono" }, rel),
        h("td", { class: "mono" }, e.origin),
        h("td", { class: "mono" }, changes),
      );
      tr.onclick = () => {
        selectedEvent = i;
        body.querySelectorAll("tr.sel").forEach((x) => x.classList.remove("sel"));
        tr.classList.add("sel");
        mark(e.t);
      };
      rows.push(tr);
    });
    body.replaceChildren(...rows);
    count.textContent = `${num(matched)} of ${num(p.events_total)} events${matched > shown ? `, first ${num(shown)} shown` : ""}`;
    more.classList.toggle("hidden", matched <= shown);
  };
  more.onclick = () => {
    eventLimit *= 4;
    fill();
  };
  filter.oninput = fill;
  kind.onchange = fill;
  onlyChat.onchange = fill;
  fill();
  return section("Event log", bar, table, more);
}

/** Mark time `t` in the step-size plot and the results plot. */
function mark(t: number) {
  for (const pl of [stepPlot, mainPlot]) {
    if (!pl) continue;
    pl.markers = [{ t, color: "#e03131" }];
    pl.draw();
  }
}
