// DAEdalus workbench: package browser, icon/diagram/text views, simulation and plotting.
import "./styles.css";
import { api } from "./rpc";
import type { ClassEntry, ClassInfo, ComponentInfo, SimSettings, SimSummary, VarSummary } from "./types";
import { DiagramView, type ViewKind } from "./diagram";
import { TextView } from "./editor";
import { PlotWindow, displayUnit, interp } from "./plot";
import { bundledLibrary, readPicked } from "./library";
import { drawIcon, newCtx, SVG_NS, systemExtent, IDENTITY, el } from "./graphics";
import * as experiments from "./experiments";

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;

// ---------------------------------------------------------------- state
const classes = new Map<string, ClassInfo>();
const pendingInfo = new Map<string, Promise<ClassInfo | null>>();
const src = {
  get(path: string): Promise<ClassInfo | null> {
    const c = classes.get(path);
    if (c) return Promise.resolve(c);
    let p = pendingInfo.get(path);
    if (!p) {
      p = api.classInfo(path).then((info) => {
        if (info) classes.set(path, info);
        pendingInfo.delete(path);
        return info;
      });
      pendingInfo.set(path, p);
    }
    return p;
  },
};
let tree: ClassEntry[] = [];
const expanded = new Set<string>(["Daedalus", "Daedalus.Examples"]);
let current: ClassInfo | null = null;
let view: ViewKind | "text" | "doc" | "flat" = "diagram";
let mode: "modeling" | "simulation" = "modeling";
let summary: SimSummary | null = null;
/** Model the variables in `summary` belong to. */
let summaryPath = "";
/** Whether the variable browser lists a previous model's results after another class is opened. */
let showPrev = false;
const overrides = new Map<string, number>();
const docs = new Map<string, string>();

// ---------------------------------------------------------------- messages
function log(msg: string, cls = "") {
  const d = document.createElement("div");
  if (cls) d.className = cls;
  d.textContent = msg;
  $("log").appendChild(d);
  $("log").scrollTop = $("log").scrollHeight;
}
$("msg-clear").onclick = () => ($("log").innerHTML = "");

// ---------------------------------------------------------------- views
const diagram = new DiagramView($("view-graphic"), src, classes);
const text = new TextView($("view-text"));
const plot = new PlotWindow($("plot"));

diagram.onSelect = (c) => showInspector(c);
diagram.onOpen = (p) => openClass(p);

let editTimer = 0;
text.onChange = (t) => {
  if (!current?.source) return;
  const uri = current.source.uri;
  docs.set(uri, t);
  clearTimeout(editTimer);
  editTimer = window.setTimeout(async () => {
    const errs = await api.setDocuments([[uri, t]]);
    classes.clear();
    if (errs[uri]) for (const e of errs[uri].slice(0, 5)) log(e, "err");
    await refreshTree();
    if (current) {
      const keep = view;
      const info = await src.get(current.path);
      if (info) current = info;
      view = keep;
    }
  }, 500);
};

function setActive(container: HTMLElement, attr: string, value: string) {
  container.querySelectorAll<HTMLButtonElement>(`button[${attr}]`).forEach((b) => b.classList.toggle("active", b.getAttribute(attr) === value));
}

async function showView() {
  setActive($("view-tabs"), "data-view", view);
  for (const id of ["view-graphic", "view-text", "view-doc", "view-flat"]) $(id).classList.add("hidden");
  if (!current) return;
  if (view === "icon" || view === "diagram") {
    $("view-graphic").classList.remove("hidden");
    await diagram.show(current, view);
  } else if (view === "text") {
    $("view-text").classList.remove("hidden");
    const uri = current.source?.uri;
    if (uri) {
      let t = docs.get(uri);
      if (t === undefined) {
        t = (await api.documentText(uri)) ?? "";
        docs.set(uri, t);
      }
      // the source offsets are byte offsets; convert to a character offset
      const start = current.source!.start;
      const chars = new TextDecoder().decode(new TextEncoder().encode(t).slice(0, start)).length;
      text.setText(t, chars);
    }
  } else if (view === "doc") {
    $("view-doc").classList.remove("hidden");
    const d = current.documentation;
    $("view-doc").innerHTML = `<h2>${escapeHtml(current.path)}</h2><p class="muted">${escapeHtml(current.description)}</p>` + (d ?? "<p class='muted'>No documentation.</p>");
  } else if (view === "flat") {
    $("view-flat").classList.remove("hidden");
    $("view-flat").textContent = "Flattening…";
    $("view-flat").textContent = await api.flatten(current.path);
  }
}

$("view-tabs").addEventListener("click", (e) => {
  const v = (e.target as HTMLElement).getAttribute("data-view");
  if (v) {
    view = v as typeof view;
    showView();
  }
});

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

// ---------------------------------------------------------------- inspector
async function showInspector(c: ComponentInfo | null) {
  const host = $("inspector");
  if (!c) {
    host.innerHTML = `<p class="muted">Select a component in the diagram.</p>`;
    return;
  }
  const t = c.type_path ? await src.get(c.type_path) : null;
  const rows: string[] = [];
  const params = new Map<string, { value: string; desc: string; modified: boolean }>();
  if (t) {
    const chain = [t];
    for (const e of t.extends) {
      const b = await src.get(e);
      if (b) chain.push(b);
    }
    for (const k of t.components) {
      if (k.variability === "parameter" && !k.protected) params.set(k.name, { value: k.binding ?? "", desc: k.description, modified: false });
    }
  }
  for (const [k, v] of c.modifiers) {
    const p = params.get(k.split(".")[0]);
    if (p && !k.includes(".")) {
      p.value = v;
      p.modified = true;
    } else params.set(k, { value: v, desc: "", modified: true });
  }
  for (const [k, p] of params) {
    rows.push(`<tr title="${escapeHtml(p.desc)}"><td class="k">${escapeHtml(k)}</td><td class="v"${p.modified ? ' style="font-weight:600"' : ""}>${escapeHtml(p.value) || "<span class='muted'>—</span>"}</td></tr>`);
  }
  host.innerHTML = `<h4>${escapeHtml(c.name)}</h4>
    <div class="muted">${escapeHtml(c.type_name)}</div>
    <p>${escapeHtml(c.description)}</p>
    <table>${rows.join("") || "<tr><td class='muted'>No parameters</td></tr>"}</table>
    ${c.type_path ? `<p><button id="open-type">Open ${escapeHtml(c.type_path.split(".").pop()!)}</button></p>` : ""}`;
  const b = document.getElementById("open-type");
  if (b) b.onclick = () => openClass(c.type_path!);
}

// ---------------------------------------------------------------- package browser
const KIND_GLYPH: Record<string, string> = { package: "▣", model: "◆", block: "▶", connector: "●", record: "≡", type: "T", function: "ƒ", class: "◇" };

function renderTree() {
  const filter = ($("filter") as HTMLInputElement).value.trim().toLowerCase();
  const host = $("browser");
  host.innerHTML = "";
  const icons: [SVGSVGElement, string][] = [];
  const matches = (e: ClassEntry): boolean => !filter || e.name.toLowerCase().includes(filter) || e.children.some(matches);
  const build = (entries: ClassEntry[], parent: HTMLElement) => {
    for (const e of entries) {
      if (!matches(e)) continue;
      const row = document.createElement("div");
      row.className = "node" + (current?.path === e.path ? " selected" : "");
      const open = expanded.has(e.path) || (!!filter && e.children.length > 0);
      const tw = document.createElement("span");
      tw.className = "tw";
      tw.textContent = e.children.length ? (open ? "▾" : "▸") : "";
      tw.onclick = (ev) => {
        ev.stopPropagation();
        if (expanded.has(e.path)) expanded.delete(e.path);
        else expanded.add(e.path);
        renderTree();
      };
      const ico = document.createElementNS(SVG_NS, "svg");
      ico.classList.add("ico");
      ico.setAttribute("viewBox", "-110 -110 220 220");
      const glyph = document.createElementNS(SVG_NS, "text");
      glyph.setAttribute("x", "0");
      glyph.setAttribute("y", "40");
      glyph.setAttribute("text-anchor", "middle");
      glyph.setAttribute("font-size", "120");
      glyph.setAttribute("fill", "currentColor");
      glyph.textContent = KIND_GLYPH[e.kind] ?? "?";
      ico.appendChild(glyph);
      icons.push([ico, e.path]);
      const nm = document.createElement("span");
      nm.className = "nm";
      nm.textContent = e.name;
      row.title = `${e.kind} ${e.path}${e.description ? `\n${e.description}` : ""}`;
      row.append(tw, ico, nm);
      if (e.runnable) {
        const r = document.createElement("span");
        r.className = "run";
        r.textContent = "▶";
        r.title = "Has an experiment: can be simulated";
        row.appendChild(r);
      }
      row.onclick = () => openClass(e.path);
      row.ondblclick = () => {
        if (expanded.has(e.path)) expanded.delete(e.path);
        else expanded.add(e.path);
        renderTree();
      };
      parent.appendChild(row);
      if (open && e.children.length) {
        const kids = document.createElement("div");
        kids.className = "kids";
        parent.appendChild(kids);
        build(e.children, kids);
      }
    }
  };
  build(tree, host);
  void renderIcons(icons);
}

let iconGen = 0;
async function renderIcons(items: [SVGSVGElement, string][]) {
  const gen = ++iconGen;
  for (const [svg, path] of items) {
    if (gen !== iconGen) return;
    const info = await src.get(path);
    if (!info || info.icon.graphics.length === 0) continue;
    const [[x1, y1], [x2, y2]] = systemExtent(info.icon.coordinate_system);
    svg.setAttribute("viewBox", `${Math.min(x1, x2)} ${-Math.max(y1, y2)} ${Math.abs(x2 - x1)} ${Math.abs(y2 - y1)}`);
    svg.innerHTML = "";
    const g = el("g", { transform: "scale(1,-1)" });
    svg.appendChild(g);
    const ctx = newCtx(svg);
    ctx.subst = () => "";
    await drawIcon(g, info, null, IDENTITY, src, ctx, classes, 1);
  }
}

$("filter").addEventListener("input", () => renderTree());

async function refreshTree() {
  tree = await api.classTree();
  renderTree();
}

async function openClass(path: string) {
  const info = await src.get(path);
  if (!info) {
    log(`class ${path} not found`, "err");
    return;
  }
  // Parameter overrides belong to the model they were made on.
  if (current?.path !== info.path) {
    overrides.clear();
    showPrev = false;
  }
  current = info;
  // expand the browser down to the class
  const parts = path.split(".");
  for (let i = 1; i < parts.length; i++) expanded.add(parts.slice(0, i).join("."));
  $("model-title").textContent = `${info.kind} ${info.path}`;
  $("model-title").title = info.description;
  {
    // Placeholders show what the experiment annotation will use when a field is left empty.
    const ex = info.experiment ?? {};
    const st = ex.StopTime, t0 = typeof ex.StartTime === "number" ? ex.StartTime : 0, dt = ex.Interval;
    ($("set-stop") as HTMLInputElement).placeholder = typeof st === "number" ? String(st) : "auto";
    ($("set-n") as HTMLInputElement).placeholder =
      typeof st === "number" && typeof dt === "number" && dt > 0 ? String(Math.round((st - t0) / dt)) : "auto";
  }
  history.replaceState(null, "", `#${encodeURIComponent(path)}`);
  if (view !== "text" && view !== "doc" && view !== "flat") view = info.kind !== "connector" && info.kind !== "package" && info.components.some((c) => c.type_kind !== "type" && !c.protected) ? "diagram" : "icon";
  renderTree();
  renderVariables();
  showInspector(null);
  await showView();
}

// ---------------------------------------------------------------- simulation
function settings(): SimSettings {
  const s: SimSettings = {};
  const stop = parseFloat(($("set-stop") as HTMLInputElement).value);
  if (isFinite(stop)) s.stop_time = stop;
  const n = parseInt(($("set-n") as HTMLInputElement).value, 10);
  if (n > 0) {
    s.intervals = n;
    s.interval = 0;
  }
  const tol = parseFloat(($("set-tol") as HTMLInputElement).value);
  if (isFinite(tol) && tol > 0) s.tolerance = tol;
  const m = ($("set-method") as HTMLSelectElement).value;
  if (m) s.method = m;
  if (overrides.size) s.params = [...overrides.entries()];
  return s;
}

let cache: { time: Float64Array; cols: Map<string, Float64Array> } | null = null;

async function fetchSeries(names: string[]) {
  if (!cache) return;
  const need = names.filter((n) => !cache!.cols.has(n));
  if (need.length) {
    const r = await api.values(need);
    cache.time = r.time;
    need.forEach((n, i) => cache!.cols.set(n, r.cols[i]));
  }
}

async function simulate() {
  if (!current) {
    log("Open a model first.", "warn");
    return;
  }
  const name = current.path;
  log(`Simulating ${name}…`);
  const btn = $("btn-sim") as HTMLButtonElement;
  btn.disabled = true;
  const t0 = performance.now();
  try {
    const s = await api.simulate(name, settings());
    summary = s;
    summaryPath = name;
    if (!s.ok) {
      log(s.error ?? "simulation failed", "err");
      if (!s.vars?.length) return;
    }
    for (const w of s.warnings ?? []) log(`warning: ${w}`, "warn");
    for (const l of s.log ?? []) log(l);
    if (s.stats) {
      const st = s.stats;
      log(
        `${name}: ${st.states} states, ${st.solver.steps} steps (${st.solver.rejected} rejected), ${st.events} events; ` +
          `${s.load_ms ? `parse library ${s.load_ms.toFixed(0)} ms, ` : ""}translate ${s.translate_ms.toFixed(0)} ms, simulate ${(st.init_ms + st.sim_ms).toFixed(0)} ms, total ${(performance.now() - t0).toFixed(0)} ms`,
        "ok",
      );
    }
    cache = { time: new Float64Array(), cols: new Map() };
    const plotted = plot.subplots.flatMap((sp) => sp.series.map((x) => x.name));
    const xNames = plot.subplots.map((sp) => sp.xName).filter((x): x is string => !!x);
    const names = [...new Set([...plotted, ...xNames])].filter((n) => s.vars.some((v) => v.name === n));
    await fetchSeries(names.length ? names : ["time"]);
    plot.update((n) => {
      const y = cache!.cols.get(n);
      if (!y) return null;
      const sp = plot.subplots.find((p) => p.series.some((q) => q.name === n));
      const x = sp?.xName ? cache!.cols.get(sp.xName) : cache!.time;
      return x ? { x, y } : null;
    });
    setMode("simulation");
    renderVariables();
    refreshXSelect();
    experiments.recordSimulation(name, settings(), s, cache.time, cache.cols);
  } catch (e) {
    log(String(e), "err");
  } finally {
    btn.disabled = false;
  }
}

$("btn-sim").onclick = () => simulate();
window.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
    e.preventDefault();
    simulate();
  }
});

$("btn-check").onclick = async () => {
  if (!current) return;
  const r = await api.check(current.path);
  if (r.ok) {
    const st = r.stats ?? {};
    log(
      `Check of ${current.path}: ${r.states} states, ${st.equations_after_alias ?? "?"} equations after alias elimination, ` +
        `${st.algebraic_loops ?? 0} algebraic loops (largest ${st.largest_loop ?? 0}), ${st.dummy_derivatives ?? 0} dummy derivatives`,
      "ok",
    );
    for (const w of r.warnings ?? []) log(`warning: ${w}`, "warn");
  } else log(r.error ?? "check failed", "err");
};

// ---------------------------------------------------------------- variable browser
interface VarNode {
  name: string;
  full: string;
  v?: VarSummary;
  kids: Map<string, VarNode>;
}
const varExpanded = new Set<string>();

function renderVariables() {
  const host = $("variables");
  host.innerHTML = "";
  if (!summary) return;
  if (current && summaryPath !== current.path) {
    // Like a result file kept loaded: say which model these variables come from.
    const note = document.createElement("div");
    note.className = "var-note";
    note.textContent = `Simulate ${current.path.split(".").pop()} to list its variables.`;
    host.appendChild(note);
    // The previous model's results stay reachable, folded away, instead of passing as this model's.
    const prev = document.createElement("button");
    prev.className = "var-prev";
    prev.textContent = showPrev ? `Hide results of ${summaryPath}` : `Show results of ${summaryPath}`;
    prev.onclick = () => {
      showPrev = !showPrev;
      renderVariables();
    };
    host.appendChild(prev);
    if (!showPrev) return;
  }
  const filter = ($("var-filter") as HTMLInputElement).value.trim().toLowerCase();
  const root: VarNode = { name: "", full: "", kids: new Map() };
  for (const v of summary.vars) {
    if (v.name === "time") continue;
    if (filter && !v.name.toLowerCase().includes(filter)) continue;
    // der(a.b.x) is listed under a.b as der(x)
    const m = /^((?:der\()+)(.*?)(\)+)$/.exec(v.name);
    const parts = m ? (() => { const q = m[2].split("."); q[q.length - 1] = `${m[1]}${q[q.length - 1]}${m[3]}`; return q; })() : v.name.split(".");
    let n = root;
    let full = "";
    for (const p of parts) {
      full = full ? `${full}.${p}` : p;
      let k = n.kids.get(p);
      if (!k) {
        k = { name: p, full, kids: new Map() };
        n.kids.set(p, k);
      }
      n = k;
    }
    n.v = v;
    n.full = v.name;
  }
  const t = cache?.time;
  const build = (n: VarNode, parent: HTMLElement) => {
    const kids = [...n.kids.values()].sort((a, b) => Number(!!b.kids.size) - Number(!!a.kids.size) || a.name.localeCompare(b.name));
    for (const k of kids) {
      const row = document.createElement("div");
      row.className = "node";
      const tw = document.createElement("span");
      tw.className = "tw";
      const open = varExpanded.has(k.full) || !!filter;
      tw.textContent = k.kids.size ? (open ? "▾" : "▸") : "";
      row.appendChild(tw);
      if (k.v && k.v.kind !== "param") {
        const cb = document.createElement("input");
        cb.type = "checkbox";
        cb.checked = plot.has(k.full);
        // Plotted in another subplot: shown as indeterminate.
        cb.indeterminate = !cb.checked && plot.hasAnywhere(k.full);
        cb.onchange = () => togglePlot(k.full, k.v!);
        row.appendChild(cb);
      }
      const nm = document.createElement("span");
      nm.className = "nm";
      nm.textContent = k.name;
      row.appendChild(nm);
      if (k.v) {
        row.title = `${k.full}${k.v.unit ? ` [${k.v.unit}]` : ""}${k.v.description ? `\n${k.v.description}` : ""}`;
        if (k.v.kind === "param") {
          const inp = document.createElement("input");
          inp.className = "par";
          inp.value = String(overrides.get(k.full) ?? k.v.value ?? "");
          inp.title = `${inp.value}\nChange and press Enter to re-simulate with this value`;
          if (!k.v.editable) {
            // Computed from other parameters (or protected): overriding it would do nothing.
            inp.readOnly = true;
            inp.classList.add("ro");
            inp.title = `${inp.value}\nComputed from other parameters`;
          } else if (current && summaryPath !== current.path) {
            // A previous model's result: Enter would re-simulate the open model instead.
            inp.readOnly = true;
            inp.classList.add("ro");
            inp.title = `${inp.value}\nParameter of ${summaryPath}`;
          }
          inp.onkeydown = (e) => {
            if (e.key === "Enter" && !inp.readOnly) {
              const x = parseFloat(inp.value);
              if (isFinite(x)) {
                overrides.set(k.full, x);
                simulate();
              }
            }
          };
          row.appendChild(inp);
        } else {
          const val = document.createElement("span");
          val.className = "val";
          const y = cache?.cols.get(k.full);
          if (y && t && y.length) {
            const du = displayUnit(k.v.unit);
            val.textContent = `${(+((y[y.length - 1] * du.scale + du.offset).toPrecision(5))).toString()} ${du.unit}`;
          } else val.textContent = k.v.unit ? `[${displayUnit(k.v.unit).unit}]` : "";
          row.appendChild(val);
        }
      }
      tw.onclick = () => {
        if (varExpanded.has(k.full)) varExpanded.delete(k.full);
        else varExpanded.add(k.full);
        renderVariables();
      };
      parent.appendChild(row);
      if (k.kids.size && open) {
        const d = document.createElement("div");
        d.className = "kids";
        parent.appendChild(d);
        build(k, d);
      }
    }
  };
  build(root, host);
}
$("var-filter").addEventListener("input", () => renderVariables());

async function togglePlot(name: string, v: VarSummary) {
  await fetchSeries([name]);
  const sp = plot.subplots[plot.active];
  if (sp.xName) await fetchSeries([sp.xName]);
  const y = cache!.cols.get(name)!;
  const x = sp.xName ? cache!.cols.get(sp.xName)! : cache!.time;
  plot.toggle({ name, unit: v.unit, x, y });
  renderVariables();
  experiments.noteSeries(summaryPath, name, v.unit, cache!.time, y);
}

function refreshXSelect() {
  const sel = $("plot-x") as HTMLSelectElement;
  const cur = plot.subplots[plot.active]?.xName ?? "";
  sel.innerHTML = `<option value="">time</option>` + (summary?.vars ?? []).filter((v) => v.kind !== "param").map((v) => `<option${v.name === cur ? " selected" : ""}>${escapeHtml(v.name)}</option>`).join("");
}
plot.onChange = () => {
  refreshXSelect();
  renderVariables();
};
$("plot-x").onchange = async () => {
  const xName = ($("plot-x") as HTMLSelectElement).value || undefined;
  const sp = plot.subplots[plot.active];
  sp.xName = xName;
  sp.xUnit = xName ? summary?.vars.find((v) => v.name === xName)?.unit : undefined;
  if (xName) await fetchSeries([xName]);
  for (const s of sp.series) if (!s.run) s.x = xName ? cache!.cols.get(xName)! : cache!.time;
  plot.draw();
};
$("plot-new").onclick = () => plot.addSubplot();
$("plot-clear").onclick = () => plot.clear();

function download(name: string, data: BlobPart, type: string) {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([data], { type }));
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 1000);
}
$("export-csv").onclick = async () => {
  const names = plot.subplots.flatMap((s) => s.series.map((x) => x.name)).join(",");
  download(`${summary?.model ?? "result"}.csv`, await api.resultCsv(names), "text/csv");
};
$("export-mat").onclick = async () => download("dsres.mat", await api.resultMat(), "application/octet-stream");

// ---------------------------------------------------------------- modes
function setMode(m: typeof mode) {
  mode = m;
  setActive($("topbar"), "data-mode", m);
  $("browser-pane").classList.toggle("hidden", m !== "modeling");
  $("vars-pane").classList.toggle("hidden", m !== "simulation");
  $("model-pane").classList.toggle("hidden", m !== "modeling");
  $("plot-pane").classList.toggle("hidden", m !== "simulation");
  if (m === "simulation") requestAnimationFrame(() => plot.draw());
  else showView();
}
$("topbar").querySelectorAll<HTMLButtonElement>("button[data-mode]").forEach((b) => (b.onclick = () => setMode(b.dataset.mode as typeof mode)));

// ---------------------------------------------------------------- loading sources
async function load(files: [string, string][], label: string) {
  if (!files.length) return;
  const t0 = performance.now();
  const errs = await api.setDocuments(files);
  for (const f of files) docs.delete(f[0]);
  classes.clear();
  const nerr = Object.values(errs).reduce((a, e) => a + e.length, 0);
  log(`Loaded ${files.length} files (${label}) in ${(performance.now() - t0).toFixed(0)} ms${nerr ? `, ${nerr} parse errors` : ""}`, nerr ? "warn" : "ok");
  for (const [uri, e] of Object.entries(errs).slice(0, 5)) log(`${uri}: ${e[0]}`, "err");
  await refreshTree();
}
$("open-files").onchange = async (e) => load(await readPicked((e.target as HTMLInputElement).files!), "files");
$("open-dir").onchange = async (e) => load(await readPicked((e.target as HTMLInputElement).files!), "folder");
document.body.addEventListener("dragover", (e) => e.preventDefault());
document.body.addEventListener("drop", async (e) => {
  e.preventDefault();
  if (e.dataTransfer?.files.length) await load(await readPicked(e.dataTransfer.files), "dropped");
});

// The Modelica Standard Library ships as a bundle next to the page (scripts/fetch-msl.mjs).
// Its files are registered in the worker without being parsed; a class is parsed, with
// what it needs, when it is opened or simulated.
const MSL_ROOTS = ["Modelica", "ModelicaServices", "Complex"];
const isMsl = (path: string) => MSL_ROOTS.includes(path.split(".")[0]);

async function loadMsl(): Promise<boolean> {
  const bar = $("lib-progress");
  const fill = bar.querySelector<HTMLElement>(".fill")!;
  const label = bar.querySelector<HTMLElement>(".label")!;
  const PHASE = { download: "Downloading", unpack: "Unpacking", register: "Indexing" };
  bar.classList.remove("hidden");
  label.textContent = "Modelica Standard Library…";
  try {
    // Relative to the page, not to the worker script under assets/.
    const info = await api.loadBundle(new URL("msl/", document.baseURI).href, "msl/", (p) => {
      const frac = p.total ? p.done / p.total : 0;
      // download 0–70 %, unpack 70–75 %, indexing 75–100 %
      const pct = p.phase === "download" ? 70 * frac : p.phase === "unpack" ? 72 : 75 + 25 * frac;
      fill.style.width = `${pct.toFixed(1)}%`;
      label.textContent =
        p.phase === "download" ? `${PHASE[p.phase]} Modelica Standard Library… ${(p.done / 1e6).toFixed(1)} / ${(p.total / 1e6).toFixed(1)} MB` : `${PHASE[p.phase]} Modelica Standard Library…`;
    });
    if (!info) {
      log("Modelica Standard Library not bundled with this site (see web/scripts/fetch-msl.mjs).", "warn");
      return false;
    }
    await refreshTree();
    log(
      `${info.name} ${info.version}: ${info.files} files (${(info.bytes / 1e6).toFixed(1)} MB) ready to browse in ${(info.download_ms + info.unpack_ms + info.register_ms).toFixed(0)} ms ` +
        `(download ${info.download_ms.toFixed(0)}, unpack ${info.unpack_ms.toFixed(0)}, index ${info.register_ms.toFixed(0)}); classes are parsed when opened`,
      "ok",
    );
    timings.mslBrowsable = performance.now();
    return true;
  } catch (e) {
    log(`Modelica Standard Library: ${e}`, "err");
    return false;
  } finally {
    bar.classList.add("hidden");
  }
}

/** Milestones since navigation start, for automated checks. */
const timings: { interactive?: number; mslBrowsable?: number } = {};

async function start() {
  // `#Model.Name`, or a shared run `#Model.Name?p.k=2&stop=3&plot=y` (experiments.ts)
  const share = experiments.parseShare(location.hash);
  const hash = share.model;
  // for automated checks
  const w = window as unknown as { daedalus: Record<string, unknown> };
  w.daedalus = { api, simulate, openClass, plot, timings, togglePlot: (n: string) => togglePlot(n, summary!.vars.find((v) => v.name === n)!) };
  await load(bundledLibrary(), "Daedalus library");
  const msl = loadMsl();
  w.daedalus.msl = msl;
  if (hash && isMsl(hash)) {
    log(`${hash} opens once the Modelica Standard Library is loaded…`);
    await msl;
  }
  await openClass(hash || "Daedalus.Examples.TorqueConverter.Launch");
  timings.interactive = performance.now();
  if (share.run && hash) await restoreRun(hash, share.params, share.options, share.plot);
}
start();
void interp;
window.addEventListener("hashchange", () => {
  const sh = experiments.parseShare(location.hash);
  const h = sh.model;
  if (sh.run && h) restoreRun(h, sh.params, sh.options, sh.plot);
  else if (h && h !== current?.path) openClass(h);
});

// ---------------------------------------------------------------- experiments
/** Open `model` with these overrides and settings, simulate it and plot `vars`. */
async function restoreRun(model: string, params: [string, number][], options: SimSettings, vars: string[]) {
  if (current?.path !== model) await openClass(model);
  if (current?.path !== model) return;
  overrides.clear();
  for (const [n, v] of params) overrides.set(n, v);
  const field = (id: string, v: unknown) => (($(id) as HTMLInputElement).value = v === undefined ? "" : String(v));
  field("set-stop", options.stop_time);
  field("set-n", options.intervals);
  field("set-tol", options.tolerance);
  field("set-method", options.method);
  await simulate();
  for (const n of vars) {
    const v = summary?.vars.find((x) => x.name === n);
    if (v && !plot.has(n)) await togglePlot(n, v);
  }
}
experiments.initExperiments({
  plot,
  model: () => current?.path ?? null,
  summary: () => (summary ? { summary, model: summaryPath } : null),
  settings,
  log,
  showPlot: () => setMode("simulation"),
  restore: restoreRun,
});
((window as unknown as { daedalus?: Record<string, unknown> }).daedalus ??= {}).experiments = experiments;
