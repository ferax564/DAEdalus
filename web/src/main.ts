// DAEdalus workbench: package browser, icon/diagram/text views, simulation and plotting.
import "./styles.css";
import { api, engineState } from "./rpc";
import { DEFAULT_ENGINE, setEngineToken } from "./engine";
import type { ClassEntry, ClassInfo, ComponentInfo, SimSettings, SimSummary, UnitSummary, VarSummary } from "./types";
import { DiagramView, type ViewKind } from "./diagram";
import { TextView } from "./editor";
import { PlotWindow, displayUnit, interp, pickUnit, type DisplayChoices } from "./plot";
import { bundledLibrary, bundledResources, readPicked, type Picked } from "./library";
import { drawIcon, newCtx, SVG_NS, systemExtent, IDENTITY, el } from "./graphics";
import type { Animation3D, AnimData } from "./anim3d";
import { initAssistant } from "./assistant";
import { initPerformance, profileRequested, redrawPerformance, updatePerformance } from "./performance";
import * as experiments from "./experiments";
import { StructureView, stateOverridesFor } from "./structure";
import { DiagramEditor, DRAG_TYPE, editableUri } from "./diagram-edit";
import { AnalysisPanel } from "./analysis";
import { decorateMethodSelect, showRecommendation } from "./solver-hint";
import { LinearView } from "./linear";
import { PlotTools } from "./plottools";
import { Dashboard, layoutFromHash } from "./dashboard";
import type { LiveChunk } from "./rpc";
import { renderDoc } from "./docview";
import { DiffView } from "./diffview";
import { librarySources, openLibraries, openPackage, reportConflicts, restoreInstalled, savePackage, type LibHost } from "./libmgr";
import { importParams } from "./paramsui";
import { applyThemeChoice, readThemeChoice, type ThemeChoice } from "./theme";
import { applyLogChrome, diffViewEnabled, initialLogChrome, isMoreView, pushLogLine, toggleLogChrome, type LogChrome } from "./workbench-ui";

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
let view: ViewKind | "text" | "doc" | "flat" | "structure" | "linear" | "diff" = "diagram";
let mode: "modeling" | "simulation" | "analysis" = "modeling";
/** False after a failed Check, so More views can mark Structure. */
let lastCheckOk = true;
/** Package browser: examples until the user picks All (`daedalus.browser`). */
let browserMode: "examples" | "all" = "examples";
let summary: SimSummary | null = null;
/** Model the variables in `summary` belong to. */
let summaryPath = "";
/** Whether the variable browser lists a previous model's results after another class is opened. */
let showPrev = false;
const overrides = new Map<string, number>();
const docs = new Map<string, string>();
/** Non-Modelica files loaded with the libraries (shape meshes), keyed by relative path. */
const resources = new Map<string, ArrayBuffer>();
/** Each document's text as it was loaded, before edits in the text view: what the Diff view compares with. */
const baseline = new Map<string, string>();

// ---------------------------------------------------------------- messages
let logChrome: LogChrome = initialLogChrome();
function renderLog() {
  applyLogChrome($("messages"), $("log-status"), logChrome);
}
function log(msg: string, cls = "") {
  const d = document.createElement("div");
  if (cls) d.className = cls;
  d.textContent = msg;
  $("log").appendChild(d);
  $("log").scrollTop = $("log").scrollHeight;
  logChrome = pushLogLine(logChrome, msg, cls);
  renderLog();
}
$("log-status").onclick = () => {
  logChrome = toggleLogChrome(logChrome);
  renderLog();
};
$("msg-clear").onclick = () => {
  $("log").replaceChildren();
  logChrome = { ...logChrome, lastText: "", lastClass: "" };
  renderLog();
};

/** A message with a link that runs `action` (e.g. open the debugger). */
function logLink(msg: string, cls: string, label: string, action: () => void) {
  log(msg, cls);
  const a = document.createElement("a");
  a.className = "log-link";
  a.textContent = label;
  a.onclick = action;
  $("log").lastElementChild?.appendChild(a);
}

/** Open the Structure view's debugger on `model` (after a failed check or simulation). */
async function openDebugger(model: string, simulation: boolean) {
  if (current?.path !== model) await openClass(model);
  view = "structure";
  if (mode !== "modeling") setMode("modeling");
  setActive($("view-tabs"), "data-view", view);
  for (const id of ["view-graphic", "view-text", "view-doc", "view-flat", "view-linear", "view-diff"]) $(id).classList.add("hidden");
  $("view-structure").classList.remove("hidden");
  syncViewChrome();
  await structureView.showInit(model, simulation);
}

/** Show a source position of the debugger: the Text view of the class that holds it. */
async function openLocation(file: string, line: number) {
  const holder = current?.source?.uri === file ? current : ([...classes.values()].find((c) => c.source?.uri === file) ?? null);
  if (!holder) {
    log(`${file}:${line}`);
    return;
  }
  if (holder !== current) await openClass(holder.path);
  view = "text";
  if (mode !== "modeling") setMode("modeling");
  await showView();
  let t = docs.get(file);
  if (t === undefined) {
    t = (await api.documentText(file)) ?? "";
    docs.set(file, t);
  }
  let off = 0;
  for (let k = 1, i = 0; k < line && i >= 0; k++) {
    i = t.indexOf("\n", i);
    if (i >= 0) off = ++i;
  }
  text.setText(t, off);
}

// ---------------------------------------------------------------- views
const diagram = new DiagramView($("view-graphic"), src, classes);
const text = new TextView($("view-text"));
const plot = new PlotWindow($("plot"));
initPerformance(plot);
const linear = new LinearView($("view-linear"));
linear.onLog = (m, c) => log(m, c);
linear.params = () => [...overrides.entries()];

/** Read and write a per-viewer setting (local storage may be unavailable). */
function pref(key: string, def: string): string {
  try {
    return localStorage.getItem(`daedalus.${key}`) ?? def;
  } catch {
    return def;
  }
}
function setPref(key: string, v: string) {
  try {
    localStorage.setItem(`daedalus.${key}`, v);
  } catch {
    /* storage unavailable: the setting lasts for this page */
  }
}

diagram.onSelect = (c) => showInspector(c);
diagram.onOpen = (p) => openClass(p);

// Graphical editing: edits go to the source text in the worker, then every view refreshes.
const editor = new DiagramEditor(diagram, {
  current: () => current,
  view: () => view,
  classInfo: (p) => src.get(p),
  log,
  documentChanged: async (uri, t, open) => {
    // the Diff view compares against the text before the first edit
    if (!baseline.has(uri)) baseline.set(uri, docs.get(uri) ?? "");
    docs.set(uri, t);
    for (const [k, v] of classes) if (v.source?.uri === uri) classes.delete(k);
    expanded.add("MyModels");
    await refreshTree();
    if (open && open !== current?.path && (await src.get(open))) return openClass(open);
    const info = current && (await src.get(current.path));
    if (!info) return;
    current = info;
    if (view === "diagram" || view === "icon") await diagram.show(info, view, true);
    else await showView();
    syncViewChrome();
  },
}, $("view-graphic"));

let editTimer = 0;
text.onChange = (t) => {
  if (!current?.source) return;
  const uri = current.source.uri;
  if (!baseline.has(uri) && docs.has(uri)) baseline.set(uri, docs.get(uri)!);
  docs.set(uri, t);
  syncViewChrome();
  clearTimeout(editTimer);
  editTimer = window.setTimeout(async () => {
    // Syntax errors show in the editor itself (the linter); this refreshes the browser.
    await api.ideUpdate(uri, t);
    classes.clear();
    await refreshTree();
    if (current) {
      const keep = view;
      const info = await src.get(current.path);
      if (info) current = info;
      view = keep;
    }
  }, 500);
};

// Editor intelligence: the compiler answers in the worker; navigation opens classes here.
/** A range to select once the text view shows a document (go to definition, usages). */
let pendingSelect: { uri: string; from: number; to: number } | null = null;
text.ide = {
  uri: () => (view === "text" ? current?.source?.uri ?? null : null),
  update: (uri, t) => api.ideUpdate(uri, t),
  definition: (uri, t, pos) => api.ideDefinition(uri, t, pos),
  references: (uri, t, pos) => api.ideReferences(uri, t, pos),
  rename: (uri, t, pos, name) => api.ideRename(uri, t, pos, name),
  completion: (uri, t, pos) => api.ideCompletion(uri, t, pos),
  hover: (uri, t, pos) => api.ideHover(uri, t, pos),
  symbols: (uri, t) => api.ideSymbols(uri, t),
  goto: (loc) => {
    const uri = current?.source?.uri;
    if (uri === loc.uri && (!loc.class || loc.class === current?.path)) {
      text.select(loc.from, loc.to);
      return;
    }
    pendingSelect = { uri: loc.uri, from: loc.from, to: loc.to };
    view = "text";
    if (loc.class && loc.class !== current?.path) openClass(loc.class);
    else showView();
  },
  applyEdits: async (edits) => {
    const byUri = new Map<string, typeof edits>();
    for (const e of edits) byUri.set(e.uri, [...(byUri.get(e.uri) ?? []), e]);
    for (const [uri, es] of byUri) {
      let t = docs.get(uri) ?? (await api.documentText(uri)) ?? "";
      for (const e of [...es].sort((a, b) => b.from - a.from)) t = t.slice(0, e.from) + e.text + t.slice(e.to);
      docs.set(uri, t);
      await api.ideUpdate(uri, t);
    }
    classes.clear();
    await refreshTree();
  },
  log: (msg, cls) => log(msg, cls),
};

function setActive(container: HTMLElement, attr: string, value: string) {
  container.querySelectorAll<HTMLButtonElement>(`button[${attr}]`).forEach((b) => {
    const on = b.getAttribute(attr) === value;
    b.classList.toggle("active", on);
    if (b.getAttribute("role") === "tab") b.setAttribute("aria-selected", String(on));
  });
}

const MORE_LABEL: Record<string, string> = { flat: "Flat model", structure: "Structure", linear: "Linear", diff: "Diff" };
/** More views highlights the open extra view, and Diff waits for an edit. */
function syncViewChrome() {
  const more = isMoreView(view);
  $("views-menu-btn").classList.toggle("active", more);
  $("views-menu-label").textContent = more ? MORE_LABEL[view] ?? "More views" : "More views";
  const uri = current?.source?.uri;
  const edited = !!(uri && baseline.has(uri) && docs.has(uri) && docs.get(uri) !== baseline.get(uri));
  const diffBtn = document.querySelector<HTMLButtonElement>('#views-menu-pop button[data-view="diff"]');
  if (diffBtn) diffBtn.disabled = !diffViewEnabled(edited);
  $("views-fail-dot").classList.toggle("hidden", lastCheckOk);
  document.getElementById("structure-fail-dot")?.classList.toggle("hidden", lastCheckOk);
}

async function showView() {
  setActive($("view-tabs"), "data-view", view);
  for (const id of ["view-graphic", "view-text", "view-doc", "view-flat", "view-structure", "view-linear", "view-diff"]) $(id).classList.add("hidden");
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
      const sel = pendingSelect?.uri === uri ? pendingSelect : undefined;
      pendingSelect = null;
      text.setText(t, chars, sel);
      if (sel) text.view.focus();
    }
  } else if (view === "doc") {
    $("view-doc").classList.remove("hidden");
    await renderDoc($("view-doc"), current, docHost);
  } else if (view === "linear") {
    $("view-linear").classList.remove("hidden");
    await linear.show(current.path);
  } else if (view === "flat") {
    $("view-flat").classList.remove("hidden");
    $("view-flat").textContent = "Flattening…";
    $("view-flat").textContent = await api.flatten(current.path);
  } else if (view === "structure") {
    $("view-structure").classList.remove("hidden");
    await structureView.show(current.path);
  } else if (view === "diff") {
    $("view-diff").classList.remove("hidden");
    await diffView.show(current);
  }
  syncViewChrome();
}

// ---------------------------------------------------------------- documentation, diff, edits
/** Replace a document's text: the workbench reparses it and redraws the open class. */
async function applyEdit(uri: string, t: string) {
  if (!baseline.has(uri)) baseline.set(uri, docs.get(uri) ?? (await api.documentText(uri)) ?? "");
  docs.set(uri, t);
  const errs = await api.setDocuments([[uri, t]]);
  classes.clear();
  if (errs[uri]) for (const e of errs[uri].slice(0, 5)) log(e, "err");
  await refreshTree();
  if (current) {
    const info = await src.get(current.path);
    if (info) current = info;
    showInspector(null);
    await showView();
  }
}

const blobUrls = new Map<string, string>();
/** Directory of a library's top-level package (`library/Daedalus`), from its package.mo. */
async function libraryDir(root: string): Promise<string | null> {
  const info = await src.get(root);
  const uri = info?.source?.uri;
  return uri && /(^|\/)package\.mo$/.test(uri) ? uri.replace(/\/?package\.mo$/, "") : null;
}

const docHost = {
  open: (p: string) => void openClass(p),
  async resourceUrl(cls: string, path: string): Promise<string | null> {
    const root = cls.split(".")[0];
    const dir = await libraryDir(root);
    // packages below the top level that are directories hold their own resources
    let base = dir;
    if (dir) {
      const segs = cls.split(".");
      for (let i = 1; i < segs.length; i++) {
        const sub = await src.get(segs.slice(0, i + 1).join("."));
        const u = sub?.source?.uri;
        if (u && u === `${base}/${segs[i]}/package.mo`) base = `${base}/${segs[i]}`;
        else break;
      }
    }
    const key = base ? `${base}/${path}` : null;
    if (key) {
      const hit = blobUrls.get(key);
      if (hit) return hit;
      let buf = resources.get(key);
      if (!buf) {
        const b = await api.resource(key);
        if (b) {
          buf = b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer;
          resources.set(key, buf);
        }
      }
      if (buf) {
        const ext = path.split(".").pop()!.toLowerCase();
        const type = { png: "image/png", jpg: "image/jpeg", jpeg: "image/jpeg", gif: "image/gif", svg: "image/svg+xml", webp: "image/webp", bmp: "image/bmp", pdf: "application/pdf" }[ext] ?? "application/octet-stream";
        const url = URL.createObjectURL(new Blob([buf], { type }));
        blobUrls.set(key, url);
        return url;
      }
    }
    // not stored locally: the library's sources on jsDelivr's GitHub CDN, when online
    const s = librarySources.get(root);
    if (s && navigator.onLine) return `https://cdn.jsdelivr.net/gh/${s.repo}@${s.ref}/${root}/${path.split("/").map(encodeURIComponent).join("/")}`;
    return null;
  },
  async saveMetadata(cls: string, entries: [string, string][]) {
    const e = await api.setMetadata(cls, entries);
    await applyEdit(e.uri, e.text);
    log(`Metadata of ${cls} saved (${entries.length} entries)`, "ok");
  },
};

const diffView = new DiffView($("view-diff"), src, classes, {
  async currentText(uri) {
    return docs.get(uri) ?? (await api.documentText(uri)) ?? "";
  },
  async baselineText(uri) {
    return baseline.get(uri) ?? docs.get(uri) ?? (await api.documentText(uri));
  },
  pickFile: () =>
    new Promise((resolve) => {
      const i = document.createElement("input");
      i.type = "file";
      i.accept = ".mo";
      i.onchange = async () => {
        const f = i.files?.[0];
        resolve(f ? { name: f.name, text: await f.text() } : null);
      };
      i.addEventListener("cancel", () => resolve(null));
      i.click();
    }),
});

const libHost: LibHost = {
  log,
  refresh: async () => {
    classes.clear();
    await refreshTree();
  },
  async pullResources(paths: string[]) {
    for (const p of paths) {
      const b = await api.resource(p);
      if (b) resources.set(p, b.buffer.slice(b.byteOffset, b.byteOffset + b.byteLength) as ArrayBuffer);
    }
  },
};

$("view-tabs").addEventListener("click", (e) => {
  const v = (e.target as HTMLElement).closest("button")?.getAttribute("data-view");
  if (v) {
    view = v as typeof view;
    closeMenus();
    showView();
  }
});

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

// ---------------------------------------------------------------- inspector
let inspectorGen = 0;
async function showInspector(c: ComponentInfo | null) {
  const gen = ++inspectorGen;
  const host = $("inspector");
  if (!c) {
    host.innerHTML = `<p class="muted">Select a component in the diagram.</p>`;
    return;
  }
  const editable = editableUri(current?.source?.uri);
  const localComponent = editable && c.declared_in === current?.path;
  const head = `<h4>${escapeHtml(c.name)}</h4>
    <div class="muted">${escapeHtml(c.type_name)}</div>
    <p>${escapeHtml(c.description)}</p>`;
  if (localComponent && c.type_path) {
    const listed = await api.parameters(c.type_path);
    if (gen !== inspectorGen) return;
    const mods = new Map(c.modifiers.map(([k, v]) => [k, v]));
    host.innerHTML = head;
    const table = document.createElement("table");
    if (!listed.length) table.innerHTML = "<tr><td class='muted'>No parameters</td></tr>";
    for (const p of listed) {
      const tr = document.createElement("tr");
      tr.title = p.description;
      const name = document.createElement("td");
      name.className = "k";
      name.textContent = p.name;
      const value = document.createElement("td");
      const input = document.createElement("input");
      input.className = "par";
      input.value = mods.get(p.name) ?? "";
      input.placeholder = p.default ?? "";
      input.title = p.description || p.name;
      input.onchange = () => {
        const v = input.value.trim();
        const old = (mods.get(p.name) ?? "").trim();
        if (v === old) return;
        void editor.apply([{ op: "set_modifier", name: c.name, modifier: p.name, value: v || null }]).then((r) => {
          if (r) mods.set(p.name, v);
        });
      };
      value.append(input);
      const unit = document.createElement("td");
      unit.className = "muted";
      unit.textContent = p.unit;
      const desc = document.createElement("td");
      desc.className = "muted";
      desc.textContent = p.description;
      tr.append(name, value, unit, desc);
      table.append(tr);
    }
    host.append(table, inspectorActions(c));
    return;
  }
  const t = c.type_path ? await src.get(c.type_path) : null;
  if (gen !== inspectorGen) return;
  const rows: string[] = [];
  const params = new Map<string, { value: string; desc: string; modified: boolean }>();
  if (t) {
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
  host.innerHTML = `${head}
    <table>${rows.join("") || "<tr><td class='muted'>No parameters</td></tr>"}</table>`;
  host.append(inspectorActions(c));
  if (!editable) {
    const dup = document.createElement("p");
    const b = document.createElement("button");
    b.textContent = "Duplicate to my models";
    b.title = "Copy this read-only class into MyModels.mo and edit the copy";
    b.onclick = () => void editor.duplicate();
    dup.append(b);
    host.append(dup);
  }
}

function inspectorActions(c: ComponentInfo): HTMLElement {
  const p = document.createElement("p");
  if (c.type_path) {
    const open = document.createElement("button");
    open.id = "open-type";
    open.textContent = `Open ${c.type_path.split(".").pop()!}`;
    open.onclick = () => openClass(c.type_path!);
    p.append(open, " ");
  }
  const ip = document.createElement("button");
  ip.id = "import-params";
  ip.title = `Set parameters of ${c.name} from a CSV (name,value[,unit]) or SSP .ssv file`;
  ip.textContent = "Import parameters…";
  if (current) {
    const model = current.path;
    ip.onclick = () => importParams(model, c, { applyOps: async (ops) => !!(await editor.apply(ops)), log });
  }
  p.append(ip);
  return p;
}

// ---------------------------------------------------------------- package browser
const KIND_GLYPH: Record<string, string> = { package: "▣", model: "◆", block: "▶", connector: "●", record: "≡", type: "T", function: "ƒ", class: "◇" };

/** Examples mode: runnable classes, their packages, and the whole Daedalus.Examples branch. */
let exampleSet: Set<string> | null = null;
let exampleSetTree: ClassEntry[] | null = null;
function examplesInTree(): Set<string> {
  if (exampleSet && exampleSetTree === tree) return exampleSet;
  const keep = new Set<string>();
  const walk = (e: ClassEntry, underExamples: boolean): boolean => {
    const inExamples = underExamples || e.path === "Daedalus.Examples";
    let child = false;
    for (const c of e.children) if (walk(c, inExamples)) child = true;
    const hit = inExamples || e.runnable || child;
    if (hit) keep.add(e.path);
    return hit;
  };
  for (const e of tree) walk(e, false);
  exampleSet = keep;
  exampleSetTree = tree;
  return keep;
}

function renderTree() {
  const filter = ($("filter") as HTMLInputElement).value.trim().toLowerCase();
  const host = $("browser");
  host.innerHTML = "";
  const icons: [SVGSVGElement, string][] = [];
  const allowed = browserMode === "all" ? null : examplesInTree();
  const inBrowser = (e: ClassEntry) => !allowed || allowed.has(e.path);
  const matches = (e: ClassEntry): boolean => inBrowser(e) && (!filter || e.name.toLowerCase().includes(filter) || e.children.some(matches));
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
      // drag onto an editable diagram to add a component
      row.draggable = true;
      row.ondragstart = (ev) => ev.dataTransfer?.setData(DRAG_TYPE, e.path);
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
function syncBrowserChip() {
  $("browser-examples").classList.toggle("active", browserMode === "examples");
  $("browser-all").classList.toggle("active", browserMode === "all");
}
document.querySelectorAll<HTMLButtonElement>("#browser-pane [data-browser]").forEach((b) => {
  b.onclick = () => {
    browserMode = b.dataset.browser === "all" ? "all" : "examples";
    setPref("browser", browserMode);
    syncBrowserChip();
    renderTree();
  };
});
browserMode = pref("browser", "examples") === "all" ? "all" : "examples";
syncBrowserChip();

async function refreshTree() {
  tree = await api.classTree();
  renderTree();
}

// ---------------------------------------------------------------- navigation
// Every class opened gets its own browser history entry, so Back/Forward (browser buttons,
// Alt+←/→, mouse side buttons, or the arrows next to the class name) walk through them.
let navIdx = 0;
let navLen = 1;
/** Bumped by every openClass call, so a slower, older load cannot overwrite a newer one. */
let openGen = 0;
// Entries ahead of this one survive a reload; the tab's session storage remembers how many.
function saveNavLen() {
  try {
    sessionStorage.setItem("daedalus.navLen", String(navLen));
  } catch {
    /* storage unavailable: Forward is just not offered after a reload */
  }
}
function updateNav() {
  ($("nav-back") as HTMLButtonElement).disabled = navIdx <= 0;
  ($("nav-fwd") as HTMLButtonElement).disabled = navIdx >= navLen - 1;
}
$("nav-back").onclick = () => history.back();
$("nav-fwd").onclick = () => history.forward();
window.addEventListener("popstate", (e) => {
  const st = e.state as { idx?: number } | null;
  if (typeof st?.idx === "number") navIdx = st.idx;
  else {
    // The hash was edited by hand: that made a new entry.
    navIdx++;
    navLen = navIdx + 1;
    saveNavLen();
    history.replaceState({ idx: navIdx }, "");
  }
  updateNav();
  const sh = experiments.parseShare(location.hash);
  const h = sh.model;
  if (sh.run && h) void restoreRun(h, sh.params, sh.options, sh.plot);
  else if (h && h !== current?.path) openClass(h, false);
});

async function openClass(path: string, push = true) {
  const gen = ++openGen;
  const info = await src.get(path);
  if (gen !== openGen) return;
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
  const hash = `#${encodeURIComponent(path)}`;
  if (push && location.hash !== hash) {
    navIdx++;
    navLen = navIdx + 1;
    saveNavLen();
    history.pushState({ idx: navIdx }, "", hash);
  } else {
    history.replaceState({ idx: navIdx }, "", hash);
  }
  updateNav();
  if (view !== "text" && view !== "doc" && view !== "flat" && view !== "structure" && view !== "linear" && view !== "diff") view = info.kind !== "connector" && info.kind !== "package" && (info.components.some((c) => c.type_kind !== "type" && !c.protected) || (editableUri(info.source?.uri) && ["model", "block", "class"].includes(info.kind))) ? "diagram" : "icon";
  renderTree();
  renderVariables();
  showInspector(null);
  dashboard.setModel(info.path, pendingDash?.model === info.path ? pendingDash.layout : null);
  pendingDash = null;
  tools.closeMenu();
  if (mode === "modeling") placePlot();
  syncSidePanes();
  updateCoach();
  if (mode === "analysis") analysis.refresh();
  await showView();
}

function updateCoach() {
  const show =
    pref("coach", "on") !== "off" &&
    !!current?.path.startsWith("Daedalus.Examples.") &&
    !(summary && summaryPath === current.path);
  $("coach").classList.toggle("hidden", !show);
}
$("coach-dismiss").onclick = () => {
  setPref("coach", "off");
  updateCoach();
};

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
  const so = stateOverridesFor(current?.path);
  if (so.length) s.stateOverrides = so;
  if (profileRequested()) s.profile = true;
  return s;
}

const structureView = new StructureView($("view-structure"), { settings, log, simulate: () => simulate(), openLocation: (f, l) => void openLocation(f, l) });

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

// ---------------------------------------------------------------- live runs
/** Rows of a run streaming in from the worker (grown in place). */
class LiveData {
  private tbuf = new Float64Array(1024);
  n = 0;
  start = NaN;
  stop = NaN;
  t = NaN;
  private cols = new Map<string, { a: Float64Array; n: number }>();
  private static grow(a: Float64Array, need: number) {
    if (need <= a.length) return a;
    const b = new Float64Array(Math.max(need, 2 * a.length));
    b.set(a);
    return b;
  }
  add(c: LiveChunk) {
    this.tbuf = LiveData.grow(this.tbuf, c.from + c.time.length);
    this.tbuf.set(c.time, c.from);
    this.n = c.from + c.time.length;
    if (this.n && isNaN(this.start)) this.start = this.tbuf[0];
    this.stop = c.stop;
    this.t = c.t;
    for (const [name, { from, v }] of Object.entries(c.cols)) {
      const e = this.cols.get(name) ?? { a: new Float64Array(1024), n: 0 };
      e.a = LiveData.grow(e.a, from + v.length);
      e.a.set(v, from);
      e.n = from + v.length;
      this.cols.set(name, e);
    }
  }
  time() {
    return this.tbuf.subarray(0, this.n);
  }
  col(name: string): Float64Array | undefined {
    const e = this.cols.get(name);
    return e ? e.a.subarray(0, Math.min(e.n, this.n)) : undefined;
  }
}
let liveData: LiveData | null = null;
const liveBox = $("plot-live") as HTMLInputElement;
liveBox.checked = pref("livePlot", "1") === "1";
liveBox.onchange = () => setPref("livePlot", liveBox.checked ? "1" : "0");

/** Data of a plotted series from a result (`time`, `col`), expressions evaluated. */
function seriesData(name: string, s: { expr?: string } | undefined, time: Float64Array, col: (n: string) => Float64Array | undefined) {
  if (s?.expr) return tools.evalSeries(s as Parameters<typeof tools.evalSeries>[0]);
  const y = col(name);
  if (!y) return null;
  const sp = plot.subplots.find((p) => p.series.some((q) => q.name === name));
  const x = sp?.xName ? col(sp.xName) : time;
  return x ? { x, y } : null;
}

/** The time axis and trajectories shown: a live run's rows so far, or the last result's. */
const shownTime = () => liveData?.time() ?? cache?.time ?? null;
const shownCol = (n: string) => (liveData ? liveData.col(n) : cache?.cols.get(n));

let liveDraw = false;
/** Counts of live slices received and drawn (for automated checks). */
const liveStats = { chunks: 0, draws: 0, runs: 0, lastPoints: 0 };
function onLiveChunk(c: LiveChunk) {
  if (!liveData) return;
  liveData.add(c);
  liveStats.chunks++;
  liveStats.lastPoints = liveData.n;
  if (liveDraw) return;
  liveDraw = true;
  requestAnimationFrame(() => {
    liveDraw = false;
    const d = liveData;
    if (!d) return;
    plot.liveRange = isFinite(d.start) && d.stop > d.start ? [d.start, d.stop] : null;
    plot.setLive((n, s) => seriesData(n, s, d.time(), (k) => d.col(k)));
    liveStats.draws++;
    dashboard.data(d.time(), (k) => d.col(k));
    const st = `t = ${+d.t.toPrecision(4)} / ${+d.stop.toPrecision(4)} s${c.paused ? " (paused)" : ""}`;
    $("live-status").textContent = st;
    dashboard.setStatus(st);
  });
}

/** Simulate the open model. A live run (the Live box, or `opts.pace` from the dashboard)
 * streams its rows into the plot while it runs; `opts.pace` paces it against wall time. */
async function simulate(opts: { pace?: number; names?: string[] } = {}) {
  if (!current) {
    log("Open a model first.", "warn");
    return;
  }
  if (liveData) {
    log("A live run is going on: stop it first.", "warn");
    return;
  }
  const name = current.path;
  log(`Simulating ${name}${opts.pace ? ` live at ${opts.pace}× real time` : ""}…`);
  const btn = $("btn-sim") as HTMLButtonElement;
  btn.disabled = true;
  const t0 = performance.now();
  try {
    const set = settings();
    const live = !!opts.pace || (liveBox.checked && !set.profile);
    let s: SimSummary & { stopped?: boolean };
    if (live) {
      const plotted = plot.subplots.flatMap((sp) => [...sp.series.filter((x) => !x.run && !x.expr).map((x) => x.name), ...(sp.xName && sp.xName !== "frequency" ? [sp.xName] : [])]);
      const names = [...new Set([...plotted, ...tools.exprNames(), ...dashboard.outputNames(), ...(opts.names ?? [])])];
      liveData = new LiveData();
      liveStats.runs++;
      $("live-stop").classList.remove("hidden");
      $("live-status").textContent = "starting…";
      if (!(mode === "modeling" && stripShown())) setMode("simulation");
      try {
        s = await api.simulateLive(name, set, { names, pace: opts.pace }, onLiveChunk);
      } finally {
        liveData = null;
        plot.liveRange = null;
        $("live-stop").classList.add("hidden");
        $("live-status").textContent = "";
      }
    } else s = await api.simulate(name, set);
    summary = s;
    summaryPath = name;
    updateCoach();
    void updatePerformance(!!set.profile);
    if (!s.ok) {
      const err = s.error ?? "simulation failed";
      const atInit = !s.vars?.length || /initiali[sz]ation|structurally singular|overdetermined|underdetermined/i.test(err);
      logLink(err, "err", atInit ? "Debug initialization ▸" : "Debug the failing solve ▸", () => void openDebugger(name, !atInit));
      if (!s.vars?.length) return;
    }
    for (const w of s.warnings ?? []) log(`warning: ${w}`, "warn");
    for (const l of s.log ?? []) log(l);
    if (s.stats) {
      const st = s.stats;
      log(
        `${name}: ${st.states} states, ${st.method ? `${st.method}, ` : ""}${st.solver.steps} steps (${st.solver.rejected} rejected), ${st.events} events; ` +
          `${s.load_ms ? `parse library ${s.load_ms.toFixed(0)} ms, ` : ""}${s.cached ? "translation reused" : `translate ${s.translate_ms.toFixed(0)} ms`}, simulate ${(st.init_ms + st.sim_ms).toFixed(0)} ms, total ${(performance.now() - t0).toFixed(0)} ms${s.engine ? " (local engine)" : ""}`,
        "ok",
      );
    }
    showRecommendation($("solver-hint"), $("set-method") as HTMLSelectElement, s.stats, log);
    cache = { time: new Float64Array(), cols: new Map() };
    const plotted = plot.subplots.flatMap((sp) => sp.series.filter((x) => !x.expr).map((x) => x.name));
    const xNames = plot.subplots.map((sp) => sp.xName).filter((x): x is string => !!x);
    const names = [...new Set([...plotted, ...xNames, ...tools.exprNames(), ...dashboard.outputNames()])].filter((n) => s.vars.some((v) => v.name === n));
    await fetchSeries(names.length ? names : ["time"]);
    plot.update((n, ser) => seriesData(n, ser, cache!.time, (k) => cache!.cols.get(k)));
    for (const sp of plot.subplots) {
      for (const q of sp.series) q.choices = choicesOf(s.vars.find((v) => v.name === q.name));
      if (sp.xName) sp.xChoices = choicesOf(s.vars.find((v) => v.name === sp.xName));
    }
    plot.applyUnits();
    refreshUnitSelects();
    dashboard.data(cache.time, (k) => cache!.cols.get(k));
    if (s.stopped) log(`${name}: stopped by the user`, "warn");
    if (!(mode === "modeling" && stripShown())) setMode("simulation");
    else placePlot();
    renderVariables();
    refreshXSelect();
    experiments.recordSimulation(name, settings(), s, cache.time, cache.cols);
    await loadAnimation(s, name);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    if (msg.startsWith("worker restarted")) log("Simulation worker stopped; it was restarted. Run Simulate again.", "warn");
    else log(msg, "err");
  } finally {
    btn.disabled = false;
  }
}

$("btn-sim").onclick = () => simulate();
$("live-stop").onclick = () => void api.liveControl({ stop: true });

// ---------------------------------------------------------------- local engine
// `daedalus serve` on this computer: simulations, analyses and experiment runs go there.
// Opt-in (a page on the public site calling 127.0.0.1 makes some browsers ask first); the
// choice is remembered, and `?engine=URL` connects to another address.
const ENGINE_KEY = "daedalus.engine";
const ENGINE_TOKEN_KEY = "daedalus.engine.token";
const engineBtn = $("btn-engine") as HTMLButtonElement;
/** Shown on the Setup control. A missed engine is not written into the message log. */
let engineMiss = "";
function showEngine() {
  const st = engineState.status;
  engineBtn.classList.toggle("active", !!st);
  engineBtn.classList.toggle("miss", !st && !!engineMiss);
  engineBtn.textContent = st ? `Local engine · ${st.cores} cores` : engineMiss ? "Local engine · unavailable" : "Local engine";
  engineBtn.title = st
    ? "Simulate on this computer with the local engine: native speed and every core"
    : engineMiss || "Simulate on this computer with the local engine (run `daedalus serve`): native speed and every core";
  $("setup-miss").classList.toggle("hidden", !!st || !engineMiss);
  $("setup-miss").title = engineMiss;
}
function rememberedToken(): string | null {
  const q = new URLSearchParams(location.search).get("token");
  if (q) return q;
  try {
    return localStorage.getItem(ENGINE_TOKEN_KEY);
  } catch {
    return null;
  }
}
async function connectEngine(url: string, quiet = false) {
  let token = rememberedToken();
  if (!token && !quiet) token = window.prompt("Paste the token printed by daedalus serve")?.trim() || null;
  if (!token) {
    if (!quiet) engineMiss = "Local engine needs the token printed by `daedalus serve`.";
    showEngine();
    return;
  }
  setEngineToken(token);
  engineBtn.disabled = true;
  try {
    const st = await api.engine(url);
    try {
      localStorage.setItem(ENGINE_KEY, url);
      localStorage.setItem(ENGINE_TOKEN_KEY, token);
    } catch {}
    if (st) {
      engineMiss = "";
      log(`Local engine connected at ${url} (${st.cores} cores, version ${st.version}): simulations run natively.`, "ok");
    }
  } catch (e) {
    engineMiss = `No local engine at ${url}. Start it with \`daedalus serve\` (${String(e)}). Simulations stay in the browser.`;
  } finally {
    engineBtn.disabled = false;
    showEngine();
  }
}
engineState.onLost = (reason) => {
  engineMiss = `Local engine lost (${reason}). Simulations run in the browser again.`;
  showEngine();
  log(engineMiss, "warn");
};
engineBtn.onclick = async () => {
  if (engineState.status) {
    await api.engine(null);
    setEngineToken(null);
    try {
      localStorage.removeItem(ENGINE_KEY);
      localStorage.removeItem(ENGINE_TOKEN_KEY);
    } catch {}
    engineMiss = "";
    log("Local engine disconnected; simulations run in the browser.");
    showEngine();
  } else {
    await connectEngine(new URLSearchParams(location.search).get("engine") ?? DEFAULT_ENGINE);
  }
};
{
  let saved: string | null = null;
  try {
    saved = localStorage.getItem(ENGINE_KEY);
  } catch {}
  const asked = new URLSearchParams(location.search).get("engine");
  if (asked || saved) void connectEngine((asked || saved)!, !asked);
}
decorateMethodSelect($("set-method") as HTMLSelectElement);
window.addEventListener("keydown", (e) => {
  if ((e.ctrlKey || e.metaKey) && e.key === "Enter") {
    e.preventDefault();
    simulate();
  }
});

$("btn-check").onclick = async () => {
  if (!current) return;
  const btn = $("btn-check") as HTMLButtonElement;
  btn.disabled = true;
  try {
    const r = await api.check(current.path);
    lastCheckOk = r.ok;
    syncViewChrome();
    if (r.ok) {
      const st = r.stats ?? {};
      log(
        `Check of ${current.path}: ${r.states} states, ${st.equations_after_alias ?? "?"} equations after alias elimination, ` +
          `${st.algebraic_loops ?? 0} algebraic loops (largest ${st.largest_loop ?? 0}), ${st.dummy_derivatives ?? 0} dummy derivatives`,
        "ok",
      );
      for (const w of r.warnings ?? []) log(`warning: ${w}`, "warn");
    } else {
      const path = current.path;
      logLink(r.error ?? "check failed", "err", "Diagnose ▸", () => void openDebugger(path, false));
    }
    logUnits(r.units);
  } catch (e) {
    const msg = e instanceof Error ? e.message : String(e);
    if (msg.startsWith("worker restarted")) log("Simulation worker stopped; it was restarted. Run Check again.", "warn");
    else log(msg, "err");
  } finally {
    btn.disabled = false;
  }
};

/** Unit issues of a Check, as warnings in the messages panel. */
function logUnits(u: UnitSummary | undefined) {
  if (!u) return;
  const shown = 50;
  for (const i of u.issues.slice(0, shown)) log(`unit warning: ${i.message}`, "warn unit");
  if (u.issues.length > shown) log(`… ${u.issues.length - shown} more unit warnings`, "warn unit");
  log(
    `Units: ${u.issues.length} issue${u.issues.length === 1 ? "" : "s"}; ${u.declared} variables with a declared unit, ` +
      `${u.inferred} inferred, ${u.unknown} unknown` +
      (u.unparsed.length ? `; unparsed unit strings: ${u.unparsed.join(", ")}` : ""),
    u.issues.length ? "warn" : "ok",
  );
}

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
        row.title = `${k.full}${k.v.unit ? ` [${k.v.unit}]${k.v.unit_inferred ? " (inferred)" : ""}` : ""}${k.v.description ? `\n${k.v.description}` : ""}`;
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
            const du = varDisplay(k.v);
            val.textContent = `${(+((y[y.length - 1] * du.scale + du.offset).toPrecision(5))).toString()} ${du.unit}`;
          } else val.textContent = k.v.unit ? `[${varDisplay(k.v).unit}]` : "";
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
  if (liveData) {
    // A run is streaming in: plot its rows so far and stream this variable too.
    const d = liveData;
    await api.liveVars([name]);
    plot.toggle({ name, unit: v.unit, x: d.time(), y: d.col(name) ?? new Float64Array() });
    renderVariables();
    return;
  }
  if (!cache) return;
  await fetchSeries([name]);
  const sp = plot.subplots[plot.active];
  if (sp.xName) await fetchSeries([sp.xName]);
  const y = cache!.cols.get(name)!;
  const x = sp.xName ? cache!.cols.get(sp.xName)! : cache!.time;
  plot.toggle({ name, unit: v.unit, x, y, choices: choicesOf(v) });
  renderVariables();
  experiments.noteSeries(summaryPath, name, v.unit, cache!.time, y);
}

function refreshXSelect() {
  const sel = $("plot-x") as HTMLSelectElement;
  const cur = plot.subplots[plot.active]?.xName ?? "";
  sel.innerHTML = `<option value="">time</option>` + (summary?.vars ?? []).filter((v) => v.kind !== "param").map((v) => `<option${v.name === cur ? " selected" : ""}>${escapeHtml(v.name)}</option>`).join("");
}
// ---------------------------------------------------------------- display units
/** Display units of a variable: the result's unit table entry for its unit and displayUnit. */
function choicesOf(v: VarSummary | undefined): DisplayChoices | undefined {
  if (!v?.unit) return undefined;
  return summary?.display?.[`${v.unit}|${v.display_unit ?? ""}`];
}

/** How a variable is shown outside the plot: its default display unit in the chosen system. */
function varDisplay(v: VarSummary): { unit: string; scale: number; offset: number } {
  const c = choicesOf(v);
  return c?.options.length ? pickUnit(c, plot.imperial) : displayUnit(v.unit);
}

function fillUnitSelect(sel: HTMLSelectElement, c: DisplayChoices | undefined, picked: string | undefined) {
  if (!c?.options.length) {
    sel.innerHTML = `<option value="">—</option>`;
    sel.disabled = true;
    return;
  }
  sel.disabled = false;
  const auto = pickUnit(c, plot.imperial).unit;
  sel.innerHTML =
    `<option value="">auto (${escapeHtml(auto)})</option>` +
    c.options
      .map((o) => `<option value="${escapeHtml(o.unit)}"${o.unit === picked ? " selected" : ""}>${escapeHtml(o.unit)}${o.imperial ? " ·imp" : ""}</option>`)
      .join("");
}

function refreshUnitSelects() {
  const sp = plot.subplots[plot.active];
  fillUnitSelect($("plot-yunit") as HTMLSelectElement, plot.yChoices(sp), sp?.yUnit);
  fillUnitSelect($("plot-xunit") as HTMLSelectElement, plot.xChoices(sp), sp?.xDisplay);
}

$("plot-yunit").onchange = () => {
  const sp = plot.subplots[plot.active];
  if (!sp) return;
  sp.yUnit = ($("plot-yunit") as HTMLSelectElement).value || undefined;
  plot.applyUnits();
  renderVariables();
};
$("plot-xunit").onchange = () => {
  const sp = plot.subplots[plot.active];
  if (!sp) return;
  sp.xDisplay = ($("plot-xunit") as HTMLSelectElement).value || undefined;
  plot.draw();
};
{
  const box = $("plot-imperial") as HTMLInputElement;
  try {
    plot.imperial = localStorage.getItem("daedalus.imperial") === "1";
  } catch {
    /* storage unavailable: SI */
  }
  box.checked = plot.imperial;
  box.onchange = () => {
    plot.imperial = box.checked;
    try {
      localStorage.setItem("daedalus.imperial", box.checked ? "1" : "0");
    } catch {
      /* not persisted */
    }
    plot.applyUnits();
    refreshUnitSelects();
    renderVariables();
  };
}

plot.onChange = () => {
  refreshUnitSelects();
  refreshXSelect();
  renderVariables();
};
$("plot-x").onchange = async () => {
  const xName = ($("plot-x") as HTMLSelectElement).value || undefined;
  const sp = plot.subplots[plot.active];
  sp.xName = xName;
  const xv = xName ? summary?.vars.find((v) => v.name === xName) : undefined;
  sp.xUnit = xv?.unit;
  sp.xChoices = choicesOf(xv);
  sp.xDisplay = undefined;
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

// ---------------------------------------------------------------- analysis
const analysis = new AnalysisPanel($("center"), {
  model: () => current?.path ?? null,
  vars: () => (summary && current && summaryPath === current.path ? summary.vars : null),
  settings: () => settings(),
  plotted: () => plot.subplots.flatMap((sp) => sp.series.map((x) => x.name)),
  simulate: () => simulate().then(() => setMode("analysis")),
  applyParams: (ps) => {
    for (const [n, v] of ps) overrides.set(n, v);
    void simulate();
  },
  log,
});

// ---------------------------------------------------------------- 3-D animation
let anim: Animation3D | null = null;
let simView: "plot" | "anim" = "plot";
/** Model whose results the animation shows, so a new model with shapes opens it once. */
let animShown = "";
let animData: AnimData | null = null;

function setSimView(v: typeof simView) {
  simView = v;
  setActive($("sim-views"), "data-sim", v);
  $("plot-body").classList.toggle("hidden", v !== "plot");
  $("plot-tools").classList.toggle("hidden", v !== "plot");
  $("anim").classList.toggle("hidden", v !== "anim");
  for (const id of ["plot-new", "plot-clear", "display-menu"]) $(id).classList.toggle("hidden", v !== "plot");
  $("plot-x").parentElement!.classList.toggle("hidden", v !== "plot");
  if (v === "plot") {
    anim?.stop();
    requestAnimationFrame(() => plot.draw());
  } else requestAnimationFrame(() => anim?.resize());
}
$("sim-views").addEventListener("click", (e) => {
  const v = (e.target as HTMLElement).getAttribute("data-sim");
  if (v) setSimView(v as typeof simView);
});

/** Build the animation from the Visualizers.Advanced.Shape instances in the result, if any. */
async function loadAnimation(s: SimSummary, model: string) {
  const mod = await import("./anim3d");
  const shapes = mod.findShapes(s.vars.map((v) => v.name));
  $("anim-tab").classList.toggle("hidden", shapes.length === 0);
  if (!shapes.length) {
    animData = null;
    if (simView === "anim") setSimView("plot");
    return;
  }
  const t0 = performance.now();
  const types = await mod.resolveShapeTypes(model, shapes, src);
  const params = new Map(s.vars.filter((v) => v.kind === "param").map((v) => [v.name, v.value ?? 0]));
  const lists = shapes.map((p) => mod.shapeVarNames(p));
  const need = [...new Set(lists.flat().filter((n) => !params.has(n)))];
  const r = need.length ? await api.values(need) : { time: await api.values([]).then((x) => x.time), cols: [] };
  const meshes: Record<string, ArrayBuffer> = {};
  for (const t of new Set(types.values())) {
    if (!mod.isFileShape(t)) continue;
    const p = mod.resolveResource(t, resources.keys());
    if (p) meshes[t] = resources.get(p)!;
  }
  const cols = new Map(need.map((n, i) => [n, r.cols[i]]));
  const track = (n: string) => (params.has(n) ? params.get(n)! : mod.compact(cols.get(n) ?? new Float64Array()));
  const g = [1, 2, 3].map((i) => params.get(`world.n[${i}]`) ?? (i === 2 ? -1 : 0));
  animData = {
    model,
    time: r.time,
    gravity: g,
    shapes: shapes.map((p, k) => mod.assembleShape(p, types.get(p) ?? "cylinder", lists[k].map(track))),
    meshes,
  };
  if (!anim) {
    anim = new mod.Animation3D($("anim"));
    anim.onWarning = (m) => log(m, "warn");
    anim.onRecorded = (v, f) => log(`Recorded ${f} (${(v.size / 1e6).toFixed(1)} MB): download it from the animation bar`, "ok");
  }
  if (animShown !== model) {
    animShown = model;
    setSimView("anim");
  }
  anim.setData(animData);
  log(`Animation: ${shapes.length} shapes in ${(performance.now() - t0).toFixed(0)} ms`);
}

// ---------------------------------------------------------------- modes
/** Analysis hides the variable tree until this model has a result, so one empty explanation is enough. */
function syncSidePanes() {
  const resultHere = !!(summary && current && summaryPath === current.path);
  $("browser-pane").classList.toggle("hidden", mode !== "modeling");
  $("vars-pane").classList.toggle("hidden", mode === "modeling" || (mode === "analysis" && !resultHere));
}
function setMode(m: typeof mode) {
  mode = m;
  setActive($("topbar"), "data-mode", m);
  syncSidePanes();
  $("model-pane").classList.toggle("hidden", m !== "modeling");
  $("plot-pane").classList.toggle("hidden", m !== "simulation");
  analysis.setVisible(m === "analysis");
  tools.closeMenu();
  placePlot();
  if (m === "simulation") {
    requestAnimationFrame(() => (simView === "plot" ? plot.draw() : anim?.resize()));
    void refreshSimDiagram();
  } else if (m === "modeling") showView();
}
$("topbar").querySelectorAll<HTMLButtonElement>("button[data-mode]").forEach((b) => (b.onclick = () => setMode(b.dataset.mode as typeof mode)));
// Phones: Packages starts collapsed so the diagram is the first screen. The log has its own one-line collapse.
const narrow = window.matchMedia("(max-width: 900px)");
const collapsible = ["browser-pane", "vars-pane"].map((id) => $(id));
function applyNarrow(on: boolean) {
  for (const pane of collapsible) {
    pane.classList.toggle("collapsed", on);
    pane.querySelector(".pane-toggle")?.setAttribute("aria-expanded", String(!on));
  }
}
applyNarrow(narrow.matches);
narrow.addEventListener("change", () => applyNarrow(narrow.matches));
document.querySelectorAll<HTMLButtonElement>(".pane-toggle").forEach((btn) => {
  btn.onclick = () => {
    if (!narrow.matches) return;
    const pane = btn.closest<HTMLElement>("#browser-pane, #vars-pane, #messages");
    if (!pane) return;
    const collapsed = pane.classList.toggle("collapsed");
    btn.setAttribute("aria-expanded", String(!collapsed));
  };
});
// File, Setup, More views, and Display: a tap outside or Escape closes them.
const MENUS: [string, string][] = [
  ["file-menu-btn", "file-menu-pop"],
  ["setup-menu-btn", "setup-menu-pop"],
  ["views-menu-btn", "views-menu-pop"],
  ["display-menu-btn", "display-menu-pop"],
];
function closeMenus() {
  for (const [btn, pop] of MENUS) {
    $(pop).classList.add("hidden");
    $(btn).setAttribute("aria-expanded", "false");
  }
}
function bindMenu(btnId: string, popId: string) {
  const btn = $(btnId);
  const pop = $(popId);
  btn.onclick = (e) => {
    e.stopPropagation();
    const open = pop.classList.contains("hidden");
    closeMenus();
    if (!open) return;
    pop.classList.remove("hidden");
    btn.setAttribute("aria-expanded", "true");
    pop.querySelector<HTMLElement>("a[href], button:not([disabled]), input, select")?.focus();
  };
}
for (const [btn, pop] of MENUS) bindMenu(btn, pop);
document.addEventListener("click", (e) => {
  const t = e.target as Node;
  if (MENUS.some(([btn, pop]) => $(pop).contains(t) || $(btn).contains(t))) return;
  closeMenus();
});
document.addEventListener("keydown", (e) => {
  if (e.key !== "Escape") return;
  const openBtn = MENUS.find(([, pop]) => !$(pop).classList.contains("hidden"));
  if (!openBtn) return;
  if ($(openBtn[1]).contains(document.activeElement)) $(openBtn[0]).focus();
  closeMenus();
});
function syncSetupLabel() {
  const sel = $("set-method") as HTMLSelectElement;
  const text = sel.value ? sel.selectedOptions[0]?.textContent?.trim() || sel.value : "model default";
  $("setup-label").textContent = `Setup · ${text}`;
}
$("set-method").addEventListener("change", syncSetupLabel);
syncSetupLabel();
$("btn-new-model").onclick = () => void editor.newModel();
function syncThemeButtons(choice: ThemeChoice) {
  document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]").forEach((b) => b.classList.toggle("active", b.dataset.themeChoice === choice));
}
function setTheme(choice: ThemeChoice) {
  setPref("theme", choice);
  applyThemeChoice(document.documentElement, choice);
  syncThemeButtons(choice);
  redrawThemedCanvases();
}
function redrawThemedCanvases() {
  plot.draw();
  analysis.redraw();
  structureView.redraw();
  linear.redraw();
  redrawPerformance();
}
document.querySelectorAll<HTMLButtonElement>("[data-theme-choice]").forEach((b) => {
  b.onclick = () => setTheme(b.dataset.themeChoice as ThemeChoice);
});
{
  const choice = readThemeChoice(pref("theme", "system"));
  applyThemeChoice(document.documentElement, choice);
  syncThemeButtons(choice);
}
window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener?.("change", () => {
  if (!document.documentElement.dataset.theme) redrawThemedCanvases();
});
// The toolbar wraps on narrow windows, so panels fixed under it (the Assistant drawer) follow its real height.
new ResizeObserver(() => document.documentElement.style.setProperty("--topbar-h", `${$("topbar").offsetHeight}px`)).observe($("topbar"));

// ---------------------------------------------------------------- plot tools
/** Variables of the result the plot shows. */
const shownVars = () => summary?.vars ?? null;
const tools = new PlotTools(
  {
    plot,
    vars: shownVars,
    time: shownTime,
    col: shownCol,
    fetch: async (names) => {
      if (liveData) await api.liveVars(names);
      else await fetchSeries(names);
    },
    toggle: async (name) => {
      const v = summary?.vars.find((x) => x.name === name);
      if (!v) return;
      if (mode === "modeling" && !stripOpen) setStrip(true);
      await togglePlot(name, v);
    },
    log,
    classInfo: (p) => src.get(p),
  },
  $("plot-tools"),
  $("plot-stats"),
);

/** Whether the diagram's clicks plot variables: results of the class shown, not editing. */
function plottable(cls: ClassInfo | null | undefined) {
  return !!summary && !!cls && summaryPath === cls.path && !diagram.svg.classList.contains("editing");
}
diagram.onComponentClick = (c, ev) => {
  if (plottable(current) && view === "diagram") tools.componentMenu(c, ev.clientX, ev.clientY);
};
diagram.onConnection = (from, to, ev) => {
  if (plottable(current) && view === "diagram" && current) void tools.connectionMenu(current, from, to, ev.clientX, ev.clientY);
};

// The plot strip of the Modeling page shows the same plot window as the Simulation page.
let stripOpen = pref("plotStrip", "open") !== "closed";
let stripH = Math.max(80, +pref("plotStripH", "260") || 260);
/** Whether the strip shows the plot right now (Modeling page, results, open). */
const stripShown = () => mode === "modeling" && !!summary && stripOpen;
function placePlot() {
  const strip = $("plot-strip");
  strip.classList.toggle("hidden", !(mode === "modeling" && summary));
  strip.classList.toggle("collapsed", !stripOpen);
  strip.style.height = `${stripH}px`;
  $("strip-toggle").textContent = stripOpen ? "▾ Plot" : "▸ Plot";
  $("strip-note").textContent =
    summary && current && summaryPath !== current.path
      ? `Results of ${summaryPath}`
      : "Click a component or connection in the diagram to plot its variables";
  plot.moveTo(stripShown() ? $("strip-host") : $("plot"));
}
function setStrip(open: boolean) {
  stripOpen = open;
  setPref("plotStrip", open ? "open" : "closed");
  placePlot();
}
$("strip-toggle").onclick = () => setStrip(!stripOpen);
$("strip-new").onclick = () => plot.addSubplot();
$("strip-clear").onclick = () => plot.clear();
$("strip-open").onclick = () => setMode("simulation");
$("strip-resize").addEventListener("pointerdown", (e) => {
  const el = $("strip-resize");
  el.setPointerCapture(e.pointerId);
  const y0 = e.clientY;
  const h0 = stripH;
  const max = $("model-pane").clientHeight - 80;
  const move = (ev: PointerEvent) => {
    stripH = Math.max(80, Math.min(max, h0 - (ev.clientY - y0)));
    $("plot-strip").style.height = `${stripH}px`;
  };
  const up = () => {
    el.removeEventListener("pointermove", move);
    el.removeEventListener("pointerup", up);
    setPref("plotStripH", String(Math.round(stripH)));
  };
  el.addEventListener("pointermove", move);
  el.addEventListener("pointerup", up);
});

// The diagram of the results' model next to the plot (Simulation page).
const simDiagram = new DiagramView($("sim-diagram"), src, classes);
let simDiagramOn = pref("simDiagram", "1") === "1";
let simDiagramShown = "";
simDiagram.onSelect = (c) => showInspector(c);
simDiagram.onComponentClick = (c, ev) => tools.componentMenu(c, ev.clientX, ev.clientY);
simDiagram.onConnection = (from, to, ev) => {
  const info = classes.get(summaryPath);
  if (info) void tools.connectionMenu(info, from, to, ev.clientX, ev.clientY);
};
async function refreshSimDiagram() {
  $("sim-diagram").classList.toggle("hidden", !simDiagramOn);
  $("sim-diagram-btn").classList.toggle("active", simDiagramOn);
  if (!simDiagramOn || mode !== "simulation" || !summaryPath) return;
  const info = await src.get(summaryPath);
  if (!info || (simDiagramShown === summaryPath && simDiagram.rendered?.cls === info)) return;
  simDiagramShown = summaryPath;
  await simDiagram.show(info, info.components.some((c) => c.type_kind !== "type" && !c.protected) ? "diagram" : "icon");
}
$("sim-diagram-btn").onclick = () => {
  simDiagramOn = !simDiagramOn;
  setPref("simDiagram", simDiagramOn ? "1" : "0");
  simDiagramShown = "";
  void refreshSimDiagram();
};

// ---------------------------------------------------------------- dashboard
const dashboard = new Dashboard($("dashboard"), {
  model: () => current?.path ?? null,
  vars: () => (summary && current && summaryPath === current.path ? summary.vars : null),
  overrides,
  runLive: (pace, names) => simulate({ pace, names }),
  running: () => !!liveData,
  setParams: (ps) => api.liveParams(ps),
  fetch: async (names) => {
    if (liveData) {
      await api.liveVars(names);
      return;
    }
    if (!cache || !summary || summaryPath !== current?.path) return;
    await fetchSeries(names.filter((n) => summary!.vars.some((v) => v.name === n)));
    dashboard.data(cache.time, (k) => cache!.cols.get(k));
  },
  control: (c) => api.liveControl(c),
  log,
});
dashboard.shareBase = () => {
  const plotted = [...new Set(plot.subplots.flatMap((sp) => sp.series.filter((x) => !x.run && !x.expr).map((x) => x.name)))];
  const st = settings();
  const options: SimSettings = { ...st };
  delete options.params;
  return experiments.shareUrl(current?.path ?? "", [...overrides.entries()], options, plotted);
};
let dashOn = pref("dashboardOpen", "0") === "1";
function showDashboard(on: boolean) {
  dashOn = on;
  setPref("dashboardOpen", on ? "1" : "0");
  $("dashboard").classList.toggle("hidden", !on);
  $("dash-btn").classList.toggle("active", on);
  if (on && mode !== "simulation") setMode("simulation");
}
$("dash-btn").onclick = () => showDashboard(!dashOn);
$("dashboard").classList.toggle("hidden", !dashOn);
$("dash-btn").classList.toggle("active", dashOn);

// ---------------------------------------------------------------- loading sources
async function load(files: [string, string][], label: string, res: [string, ArrayBuffer][] = []) {
  for (const [p, b] of res) {
    resources.set(p, b);
    // the worker keeps a copy for model packages (Save .zip)
    void api.setResource(p, b.slice(0));
  }
  for (const [p, t] of files) baseline.set(p, t);
  if (!files.length) {
    if (res.length) log(`Loaded ${res.length} shape files (${label})`, "ok");
    return;
  }
  const t0 = performance.now();
  const errs = await api.setDocuments(files);
  for (const f of files) docs.delete(f[0]);
  classes.clear();
  const nerr = Object.values(errs).reduce((a, e) => a + e.length, 0);
  log(
    `Loaded ${files.length} files (${label}) in ${(performance.now() - t0).toFixed(0)} ms${res.length ? `, ${res.length} shape files` : ""}${nerr ? `, ${nerr} parse errors` : ""}`,
    nerr ? "warn" : "ok",
  );
  for (const [uri, e] of Object.entries(errs).slice(0, 5)) log(`${uri}: ${e[0]}`, "err");
  await refreshTree();
}
async function loadPicked(p: Picked, label: string) {
  for (const z of p.zips) {
    try {
      const model = await openPackage(z, libHost);
      if (model) await openClass(model);
    } catch (e) {
      log(`${z.name}: ${e instanceof Error ? e.message : e}`, "err");
    }
  }
  if (p.mo.length || p.resources.length) await load(p.mo, label, p.resources);
}
$("btn-libs").onclick = () => openLibraries(libHost);
$("btn-save-zip").onclick = async () => {
  if (!current) return log("Open a model first.", "warn");
  try {
    await savePackage(current.path, libHost);
  } catch (e) {
    log(`Save .zip: ${e instanceof Error ? e.message : e}`, "err");
  }
};
$("open-files").onchange = async (e) => loadPicked(await readPicked((e.target as HTMLInputElement).files!), "files");
$("open-dir").onchange = async (e) => loadPicked(await readPicked((e.target as HTMLInputElement).files!), "folder");
document.body.addEventListener("dragover", (e) => e.preventDefault());
document.body.addEventListener("drop", async (e) => {
  e.preventDefault();
  if (e.dataTransfer?.files.length) await loadPicked(await readPicked(e.dataTransfer.files), "dropped");
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
    bar.setAttribute("aria-valuemin", "0");
    bar.setAttribute("aria-valuemax", "100");
    const info = await api.loadBundle(new URL("msl/", document.baseURI).href, "msl/", (p) => {
      const frac = p.total ? p.done / p.total : 0;
      // download 0–70 %, unpack 70–75 %, indexing 75–100 %
      const pct = p.phase === "download" ? 70 * frac : p.phase === "unpack" ? 72 : 75 + 25 * frac;
      fill.style.width = `${pct.toFixed(1)}%`;
      bar.setAttribute("aria-valuenow", String(Math.round(pct)));
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

/** Dashboard layout from a share link, applied when its model opens. */
let pendingDash: { model: string; layout: ReturnType<typeof layoutFromHash> } | null = null;

async function start() {
  // `#Model.Name`, or a shared run `#Model.Name?p.k=2&stop=3&plot=y` (experiments.ts),
  // with `&dash=` a dashboard layout (dashboard.ts)
  const share = experiments.parseShare(location.hash);
  const hash = share.model;
  {
    const l = layoutFromHash(location.hash);
    if (l && hash) {
      pendingDash = { model: hash, layout: l };
      $("dashboard").classList.remove("hidden");
      $("dash-btn").classList.add("active");
      dashOn = true;
    }
  }
  // for automated checks
  const w = window as unknown as { daedalus: Record<string, unknown> };
  w.daedalus = { api, simulate, openClass, plot, timings, analysis, setMode, togglePlot: (n: string) => togglePlot(n, summary!.vars.find((v) => v.name === n)!), editor, textView: text, anim: () => anim, animData: () => animData, setSimView, tools, dashboard, diagram, simDiagram, overrides, live: () => liveData, liveStats, mode: () => mode, linear, applyEdit, baseline, setView: (v: typeof view) => { view = v; return showView(); } };
  await load(bundledLibrary(), "Daedalus library", await bundledResources());
  initAssistant({
    currentClass: () => current?.path ?? null,
    openClass,
    afterEdit: async () => {
      classes.clear();
      await refreshTree();
      if (current) await openClass(current.path);
    },
    afterSimulate: (model, s) => {
      summary = s;
      summaryPath = model;
      cache = { time: new Float64Array(), cols: new Map() };
      renderVariables();
    },
  });
  await editor.restore();
  const msl = loadMsl();
  w.daedalus.msl = msl;
  // libraries installed on earlier visits, once the MSL they use is registered
  w.daedalus.libs = msl.then(() => restoreInstalled(libHost)).then(() => reportConflicts(libHost));
  if (hash && isMsl(hash)) {
    log(`${hash} opens once the Modelica Standard Library is loaded…`);
    await msl;
  }
  // After a reload the entries behind this one are still in the browser history.
  const st = history.state as { idx?: number } | null;
  if (typeof st?.idx === "number") {
    navIdx = st.idx;
    let saved = 0;
    try {
      saved = Number(sessionStorage.getItem("daedalus.navLen")) || 0;
    } catch {
      /* storage unavailable */
    }
    navLen = Math.max(navIdx + 1, saved);
  }
  await openClass(hash || "Daedalus.Examples.TorqueConverter.Launch", false);
  timings.interactive = performance.now();
  if (share.run && hash) await restoreRun(hash, share.params, share.options, share.plot);
}
start();
void interp;
// ---------------------------------------------------------------- experiments
/** Open `model` with these overrides and settings, simulate it and plot `vars`. */
async function restoreRun(model: string, params: [string, number][], options: SimSettings, vars: string[]) {
  if (current?.path !== model) await openClass(model, false);
  if (current?.path !== model) return;
  overrides.clear();
  for (const [n, v] of params) overrides.set(n, v);
  const field = (id: string, v: unknown) => (($(id) as HTMLInputElement).value = v === undefined ? "" : String(v));
  field("set-stop", options.stop_time);
  field("set-n", options.intervals);
  field("set-tol", options.tolerance);
  field("set-method", options.method);
  syncSetupLabel();
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
