// MSL accuracy scoreboard: renders docs/conformance/msl-4.1.0.json (written by
// tools/refcompare.py, copied into public/conformance/ by scripts/copy-conformance.mjs).
import "./scoreboard.css";

type Status = "pass" | "differ" | "error" | "timeout";
interface Failed { signal: string; outside: number; worst: number | "inf" }
interface Result {
  model: string;
  status: Status;
  time?: number;
  signals?: number;
  failed?: Failed[];
  missing?: string[];
  error?: string;
}
interface Meta {
  generated_at?: string;
  commit?: string | null;
  msl_version?: string | null;
  tolerance?: number;
  timeout?: number;
}
interface Data { meta?: Meta; summary: Record<string, number>; results: Result[] }

const DATA_URL = "./conformance/msl-4.1.0.json";
const REPO = "https://github.com/ferax564/DAEdalus";
const STATUSES: Status[] = ["pass", "differ", "error", "timeout"];
const LABEL: Record<Status, string> = { pass: "Pass", differ: "Differ", error: "Error", timeout: "Timeout" };
const ICON: Record<Status, string> = { pass: "✓", differ: "≈", error: "✕", timeout: "⏱" };
const RANK: Record<Status, number> = { pass: 0, differ: 1, timeout: 2, error: 3 };

const $ = <T extends HTMLElement = HTMLElement>(id: string) => document.getElementById(id) as T;
function el<K extends keyof HTMLElementTagNameMap>(tag: K, cls?: string, text?: string): HTMLElementTagNameMap[K] {
  const e = document.createElement(tag);
  if (cls) e.className = cls;
  if (text !== undefined) e.textContent = text;
  return e;
}
const pct = (n: number, d: number) => (d ? (100 * n) / d : 0);
const fmtPct = (n: number, d: number) => `${pct(n, d).toFixed(1)}%`;

// ---- package grouping -------------------------------------------------------

/** Depth-2 package, split to depth 3 when it has at least two sub-packages
 * (other than Examples) with three or more examples each. */
function packageGrouper(results: Result[]): (model: string) => string {
  const sub = new Map<string, Map<string, number>>();
  for (const r of results) {
    const p = r.model.split(".");
    const top = p.slice(0, 2).join(".");
    if (!sub.has(top)) sub.set(top, new Map());
    if (p.length > 3 && p[2] !== "Examples") {
      const m = sub.get(top)!;
      m.set(p[2], (m.get(p[2]) ?? 0) + 1);
    }
  }
  const split = new Set<string>();
  for (const [top, m] of sub) if ([...m.values()].filter((n) => n >= 3).length >= 2) split.add(top);
  return (model) => {
    const p = model.split(".");
    const top = p.slice(0, 2).join(".");
    return split.has(top) && p.length > 3 && p[2] !== "Examples" ? p.slice(0, 3).join(".") : top;
  };
}

// ---- tooltip ------------------------------------------------------------------

const tip = $("tip");
function attachTip(target: HTMLElement, text: () => string) {
  const show = (x: number, y: number) => {
    tip.textContent = text();
    tip.classList.remove("hidden");
    const w = tip.offsetWidth, h = tip.offsetHeight;
    tip.style.left = `${Math.max(8, Math.min(window.innerWidth - w - 8, x - w / 2))}px`;
    tip.style.top = `${y - h - 12 < 8 ? y + 18 : y - h - 12}px`;
  };
  target.addEventListener("pointermove", (e) => show(e.clientX, e.clientY));
  target.addEventListener("pointerleave", () => tip.classList.add("hidden"));
}

/** A stacked status bar; each segment has its own hover text. */
function statusBar(counts: Record<Status, number>, total: number, cls = "bar"): HTMLElement {
  const bar = el("div", cls);
  bar.setAttribute("role", "img");
  bar.setAttribute("aria-label", STATUSES.map((s) => `${LABEL[s]} ${counts[s]}`).join(", "));
  for (const s of STATUSES) {
    if (!counts[s]) continue;
    const seg = el("span", `seg s-${s}`);
    seg.style.flexGrow = String(counts[s]);
    attachTip(seg, () => `${LABEL[s]}: ${counts[s]} of ${total} (${fmtPct(counts[s], total)})`);
    bar.append(seg);
  }
  return bar;
}

function tally(rs: Result[]): Record<Status, number> {
  const c: Record<Status, number> = { pass: 0, differ: 0, error: 0, timeout: 0 };
  for (const r of rs) if (r.status in c) c[r.status]++;
  return c;
}

// ---- state ------------------------------------------------------------------

const params = new URLSearchParams(location.search);
const state = {
  q: params.get("q") ?? "",
  status: (params.get("status") ?? "") as Status | "",
  pkg: params.get("pkg") ?? "",
  sort: "model" as "model" | "status" | "time",
  dir: 1,
};
function saveState() {
  const p = new URLSearchParams();
  if (state.q) p.set("q", state.q);
  if (state.status) p.set("status", state.status);
  if (state.pkg) p.set("pkg", state.pkg);
  const s = p.toString();
  history.replaceState(null, "", s ? `?${s}` : location.pathname);
}

// ---- rendering --------------------------------------------------------------

function renderMeta(d: Data) {
  const m = d.meta ?? {};
  const tol = m.tolerance ?? 2e-3;
  const tolS = tol.toExponential(); // 0.002 -> "2e-3"
  $("tol-a").textContent = tolS;
  $("tol-b").textContent = tolS;
  const meta = $("meta");
  meta.textContent = "";
  const items: (string | Node)[] = [`MSL ${m.msl_version ?? "4.1.0"}`, `tolerance ${tolS} tube (csv-compare style)`];
  if (m.generated_at) {
    const t = new Date(m.generated_at);
    const date = isNaN(t.getTime()) ? m.generated_at : t.toLocaleDateString(undefined, { year: "numeric", month: "short", day: "numeric" });
    items.push(`results from ${date}`);
  }
  if (m.commit) {
    const a = el("a", "mono", m.commit.slice(0, 7));
    a.href = `${REPO}/commit/${m.commit}`;
    const f = document.createDocumentFragment();
    f.append("commit ", a);
    items.push(f);
  }
  if (m.timeout) items.push(`${m.timeout} s limit per example`);
  items.forEach((it, i) => {
    if (i) meta.append(el("span", "dot", "·"));
    const s = el("span");
    s.append(it);
    meta.append(s);
  });
}

function renderHero(d: Data) {
  const c = tally(d.results);
  const total = d.results.length;
  $("rate").textContent = fmtPct(c.pass, total);
  $("hero-label").textContent = `${c.pass} of ${total} examples match the reference results`;
  $("overall-bar").replaceWith(Object.assign(statusBar(c, total, "bar big"), { id: "overall-bar" }));
  const ul = $("counts");
  ul.textContent = "";
  for (const s of STATUSES) {
    const li = el("li");
    const b = el("button", `count s-${s}`);
    b.type = "button";
    b.append(el("span", "sw"), el("span", "c-label", `${ICON[s]} ${LABEL[s]}`), el("span", "c-num", String(c[s])));
    b.title = `Show ${LABEL[s].toLowerCase()} examples`;
    b.onclick = () => {
      state.status = state.status === s ? "" : s;
      update();
      $("tbl-h").scrollIntoView({ behavior: "smooth", block: "start" });
    };
    li.append(b);
    ul.append(li);
  }
}

let groupOf: (m: string) => string = (m) => m;

function renderPackages(d: Data) {
  const groups = new Map<string, Result[]>();
  for (const r of d.results) {
    const g = groupOf(r.model);
    if (!groups.has(g)) groups.set(g, []);
    groups.get(g)!.push(r);
  }
  const box = $("packages");
  box.textContent = "";
  for (const g of [...groups.keys()].sort()) {
    const rs = groups.get(g)!;
    const c = tally(rs);
    const row = el("button", "pkg-row");
    row.type = "button";
    row.dataset.pkg = g;
    const name = el("span", "pkg-name");
    name.append(el("span", "pkg-root", "Modelica."), g.replace(/^Modelica\./, ""));
    row.append(name, statusBar(c, rs.length), el("span", "pkg-num", `${c.pass}/${rs.length}`), el("span", "pkg-pct", fmtPct(c.pass, rs.length)));
    row.onclick = () => {
      state.pkg = state.pkg === g ? "" : g;
      update();
    };
    box.append(row);
  }
}

function detail(r: Result): HTMLElement {
  const td = el("td", "detail");
  if (r.status === "pass") {
    td.append(el("span", "muted", r.signals ? `${r.signals} signal${r.signals === 1 ? "" : "s"} inside the tube` : ""));
  } else if (r.status === "differ") {
    const bad = r.failed ?? [];
    const miss = r.missing ?? [];
    const n = bad.length + miss.length;
    const head = el("div", "d-head", `${n} of ${r.signals ?? "?"} signal${(r.signals ?? 0) === 1 ? "" : "s"} ${miss.length && !bad.length ? "missing" : "outside the tube"}`);
    td.append(head);
    const items: HTMLElement[] = [];
    for (const f of bad) {
      const li = el("li");
      const worst = f.worst === "inf" ? "∞" : `${Number(f.worst).toPrecision(3)}×`;
      li.append(el("code", "", f.signal), el("span", "muted", ` ${(100 * f.outside).toFixed(1)}% of points outside, worst ${worst} the tube`));
      items.push(li);
    }
    for (const s of miss) {
      const li = el("li");
      li.append(el("code", "", s), el("span", "muted", " missing from the output"));
      items.push(li);
    }
    const SHOW = 3;
    const ul = el("ul", "sigs");
    ul.append(...items.slice(0, SHOW));
    td.append(ul);
    if (items.length > SHOW) {
      const more = el("details");
      more.append(el("summary", "", `${items.length - SHOW} more`));
      const ul2 = el("ul", "sigs");
      ul2.append(...items.slice(SHOW));
      more.append(ul2);
      td.append(more);
    }
  } else if (r.status === "timeout") {
    td.append(el("span", "muted", `No result within ${r.time ?? "the"} s`));
  } else {
    // refcompare.py keeps the first 300 characters of the message.
    const msg = (r.error ?? "failed").replace(/^error:\s*/, "");
    td.append(el("code", "err", (r.error ?? "").length >= 300 ? `${msg}…` : msg));
  }
  return td;
}

function rowFor(r: Result): HTMLTableRowElement {
  const tr = el("tr");
  const tdm = el("td", "model");
  const a = el("a");
  a.href = `./#${encodeURIComponent(r.model).replace(/%2E/g, ".")}`;
  a.title = "Open in the workbench";
  const parts = r.model.split(".");
  a.append(el("span", "m-pkg", parts.slice(0, -1).join(".") + "."), el("span", "m-name", parts[parts.length - 1]));
  tdm.append(a);
  const tds = el("td", "status");
  tds.append(el("span", `badge s-${r.status}`, `${ICON[r.status]} ${LABEL[r.status]}`));
  const tdt = el("td", "num time", r.time === undefined ? "–" : r.status === "timeout" ? `>${r.time}` : r.time.toFixed(2));
  tr.append(tdm, tds, tdt, detail(r));
  return tr;
}

let data: Data;
const rowCache = new Map<string, HTMLTableRowElement>();
const haystack = new Map<string, string>();

function searchText(r: Result): string {
  return [r.model, r.status, r.error ?? "", ...(r.failed ?? []).map((f) => f.signal), ...(r.missing ?? [])].join("\n").toLowerCase();
}

function update() {
  saveState();
  // status chips
  const counts = tally(data.results.filter((r) => !state.pkg || groupOf(r.model) === state.pkg));
  const total = Object.values(counts).reduce((a, b) => a + b, 0);
  const chips = $("status-filter");
  chips.textContent = "";
  for (const s of ["", ...STATUSES] as (Status | "")[]) {
    const b = el("button", `chip${s ? ` s-${s}` : ""}${state.status === s ? " on" : ""}`);
    b.type = "button";
    b.setAttribute("aria-pressed", String(state.status === s));
    b.append(s ? `${ICON[s]} ${LABEL[s]} ` : "All ", el("span", "chip-n", String(s ? counts[s] : total)));
    b.onclick = () => {
      state.status = s;
      update();
    };
    chips.append(b);
  }
  const pc = $("pkg-chip");
  pc.classList.toggle("hidden", !state.pkg);
  pc.textContent = state.pkg ? `${state.pkg} ✕` : "";
  for (const r of $("packages").querySelectorAll<HTMLElement>(".pkg-row")) r.classList.toggle("on", r.dataset.pkg === state.pkg);

  const q = state.q.trim().toLowerCase();
  const terms = q.split(/\s+/).filter(Boolean);
  const rows = data.results.filter(
    (r) =>
      (!state.status || r.status === state.status) &&
      (!state.pkg || groupOf(r.model) === state.pkg) &&
      terms.every((t) => haystack.get(r.model)!.includes(t)),
  );
  const k = state.sort;
  rows.sort((a, b) => {
    let c = 0;
    if (k === "status") c = RANK[a.status] - RANK[b.status];
    else if (k === "time") c = (a.time ?? 0) - (b.time ?? 0);
    if (!c) c = a.model < b.model ? -1 : a.model > b.model ? 1 : 0;
    return c * state.dir;
  });
  const body = $("rows");
  body.replaceChildren(...rows.map((r) => rowCache.get(r.model) ?? rowCache.set(r.model, rowFor(r)).get(r.model)!));
  $("empty").classList.toggle("hidden", rows.length > 0);
  $("shown").textContent = rows.length === data.results.length ? `${rows.length} examples` : `${rows.length} of ${data.results.length} examples`;
  for (const th of document.querySelectorAll<HTMLElement>("th[data-sort]")) {
    const on = th.dataset.sort === k;
    th.classList.toggle("sorted", on);
    th.setAttribute("aria-sort", on ? (state.dir > 0 ? "ascending" : "descending") : "none");
    th.dataset.dir = on ? (state.dir > 0 ? "▲" : "▼") : "";
  }
}

async function main() {
  try {
    const res = await fetch(DATA_URL, { cache: "no-cache" });
    if (!res.ok) throw new Error(`HTTP ${res.status}`);
    data = (await res.json()) as Data;
  } catch (e) {
    $("meta").textContent = `Could not load ${DATA_URL}: ${(e as Error).message}`;
    $("meta").classList.add("err");
    return;
  }
  for (const r of data.results) haystack.set(r.model, searchText(r));
  groupOf = packageGrouper(data.results);
  if (state.pkg && !data.results.some((r) => groupOf(r.model) === state.pkg)) state.pkg = "";
  if (state.status && !STATUSES.includes(state.status)) state.status = "";
  renderMeta(data);
  renderHero(data);
  renderPackages(data);

  const q = $<HTMLInputElement>("q");
  q.value = state.q;
  let t = 0;
  q.addEventListener("input", () => {
    clearTimeout(t);
    t = window.setTimeout(() => {
      state.q = q.value;
      update();
    }, 80);
  });
  $("pkg-chip").onclick = () => {
    state.pkg = "";
    update();
  };
  for (const th of document.querySelectorAll<HTMLElement>("th[data-sort]")) {
    th.tabIndex = 0;
    const go = () => {
      const k = th.dataset.sort as typeof state.sort;
      state.dir = state.sort === k ? -state.dir : k === "model" ? 1 : -1;
      state.sort = k;
      update();
    };
    th.onclick = go;
    th.onkeydown = (e) => {
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        go();
      }
    };
  }
  update();
}

main();
