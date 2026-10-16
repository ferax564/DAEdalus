// Interactive dashboard: sliders bound to free parameters, gauges, numeric displays and
// mini-plots bound to result variables. "Run live" paces the simulation against wall time
// in the worker; a slider moved during the run changes its parameter at the time reached,
// without restarting. Layouts are kept per model (localStorage) and travel in share links.
import "./dashboard.css";
import { displayUnit } from "./plot";
import { h } from "./plottools";
import type { VarSummary } from "./types";

export type Widget =
  | { kind: "slider"; name: string; min: number; max: number }
  | { kind: "gauge"; name: string; min: number; max: number }
  | { kind: "number"; name: string }
  | { kind: "plot"; names: string[] };

export interface Layout {
  widgets: Widget[];
  /** Simulated seconds per wall-clock second. */
  speed: number;
}

export interface DashHost {
  model(): string | null;
  /** Variables of the open model's last result (null: not simulated yet). */
  vars(): VarSummary[] | null;
  /** Parameter overrides of the open model (the next run uses them). */
  overrides: Map<string, number>;
  /** Simulate live, paced at `speed`, streaming `names`. */
  runLive(speed: number, names: string[]): Promise<void>;
  /** Whether a live run is going on. */
  running(): boolean;
  /** Load the trajectories of `names` from the last result (then `data` is called). */
  fetch(names: string[]): Promise<void>;
  /** Change parameters of the running live run. */
  setParams(ps: [string, number][]): Promise<{ ok: boolean; error?: string; t?: number }>;
  control(c: { pause?: boolean; stop?: boolean; pace?: number }): Promise<unknown>;
  log(msg: string, cls?: string): void;
}

const KEY = (model: string) => `daedalus.dashboard.${model}`;
const SPEEDS = [0.1, 0.25, 0.5, 1, 2, 5, 10, 50, 200];
const PALETTE = ["#0b5fff", "#d9480f", "#2b8a3e", "#ae3ec9", "#f08c00", "#1098ad"];

const fmt = (v: number) => {
  if (!isFinite(v)) return "—";
  const a = Math.abs(v);
  if (a !== 0 && (a >= 1e5 || a < 1e-3)) return v.toExponential(3);
  return String(+v.toPrecision(5));
};

/** Base64url of the layout, for share links (`&dash=`). */
export function encodeLayout(l: Layout): string {
  const s = btoa(unescape(encodeURIComponent(JSON.stringify({ w: l.widgets, s: l.speed }))));
  return s.replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function decodeLayout(s: string): Layout | null {
  try {
    const j = JSON.parse(decodeURIComponent(escape(atob(s.replace(/-/g, "+").replace(/_/g, "/")))));
    if (!Array.isArray(j.w)) return null;
    return { widgets: j.w.filter(valid), speed: typeof j.s === "number" && j.s > 0 ? j.s : 1 };
  } catch {
    return null;
  }
}

function valid(w: unknown): w is Widget {
  const x = w as Widget;
  if (!x || typeof x !== "object") return false;
  if (x.kind === "plot") return Array.isArray(x.names) && x.names.every((n) => typeof n === "string");
  if (typeof (x as { name: unknown }).name !== "string") return false;
  if (x.kind === "number") return true;
  return (x.kind === "slider" || x.kind === "gauge") && isFinite(x.min) && isFinite(x.max);
}

/** The `dash` parameter of a share link hash, if any. */
export function layoutFromHash(hash: string): Layout | null {
  const q = hash.indexOf("?");
  if (q < 0) return null;
  const d = new URLSearchParams(hash.slice(q + 1)).get("dash");
  return d ? decodeLayout(d) : null;
}

export class Dashboard {
  readonly el: HTMLElement;
  private body: HTMLElement;
  private layout: Layout = { widgets: [], speed: 1 };
  private model: string | null = null;
  private status: HTMLElement;
  /** The last parameter change applied during a run. */
  private note: HTMLElement;
  private runBtn: HTMLButtonElement;
  private pauseBtn: HTMLButtonElement;
  private stopBtn: HTMLButtonElement;
  private speedSel: HTMLSelectElement;
  private paused = false;
  /** Latest data: time axis and columns. */
  private time: Float64Array = new Float64Array();
  private col: (n: string) => Float64Array | undefined = () => undefined;
  private redrawQueued = false;
  /** Pending slider values, sent at most every 50 ms. */
  private pending = new Map<string, number>();
  private sendTimer = 0;
  private widgetEls: { w: Widget; update: () => void }[] = [];

  constructor(host: HTMLElement, private hostApi: DashHost) {
    this.el = host;
    this.speedSel = h("select", { id: "dash-speed", title: "Simulated seconds per wall-clock second" });
    for (const s of SPEEDS) this.speedSel.append(h("option", { value: String(s) }, `${s}×`));
    this.speedSel.onchange = () => {
      this.layout.speed = +this.speedSel.value;
      this.save();
      if (this.hostApi.running()) void this.hostApi.control({ pace: this.layout.speed });
    };
    this.runBtn = h("button", { id: "dash-run", class: "primary", title: "Simulate paced against wall time; move sliders while it runs" }, "▶ Run live");
    this.runBtn.onclick = () => void this.run();
    this.pauseBtn = h("button", { id: "dash-pause", disabled: "" }, "Pause");
    this.pauseBtn.onclick = () => {
      this.paused = !this.paused;
      this.pauseBtn.textContent = this.paused ? "Resume" : "Pause";
      void this.hostApi.control({ pause: this.paused });
    };
    this.stopBtn = h("button", { id: "dash-stop", disabled: "" }, "Stop");
    this.stopBtn.onclick = () => void this.hostApi.control({ stop: true });
    this.status = h("span", { class: "dash-status muted", id: "dash-status" });
    this.note = h("span", { class: "dash-status dash-note", id: "dash-note" });
    const add = h("select", { id: "dash-add", title: "Add a widget" });
    for (const [v, l] of [
      ["", "+ Add…"],
      ["slider", "Slider (parameter)"],
      ["gauge", "Gauge (variable)"],
      ["number", "Display (variable)"],
      ["plot", "Mini plot (variables)"],
    ])
      add.append(h("option", { value: v }, l));
    add.onchange = () => {
      const k = add.value;
      add.value = "";
      if (k) this.addForm(k as Widget["kind"]);
    };
    const share = h("button", { id: "dash-share", title: "Copy a link that opens this model with this dashboard" }, "Share");
    share.onclick = () => void this.share();
    this.body = h("div", { class: "dash-body", id: "dash-body" });
    host.replaceChildren(
      h("div", { class: "dash-head" }, h("span", { class: "dash-title" }, "Dashboard"), add, share),
      h("div", { class: "dash-run" }, this.runBtn, this.pauseBtn, this.stopBtn, h("label", {}, "Speed ", this.speedSel), this.status, this.note),
      this.body,
    );
  }

  /** Link built by main.ts (share hash without the dashboard). */
  shareBase: () => string = () => location.href.split("#")[0] + `#${encodeURIComponent(this.model ?? "")}?`;

  private async share() {
    let base = this.shareBase();
    base += base.includes("?") ? (base.endsWith("?") ? "" : "&") : "?";
    const url = `${base}dash=${encodeLayout(this.layout)}`;
    try {
      await navigator.clipboard.writeText(url);
      this.hostApi.log(`Dashboard link copied: ${url}`, "ok");
    } catch {
      this.hostApi.log(`Dashboard link: ${url}`, "ok");
    }
  }

  get widgets(): Widget[] {
    return this.layout.widgets;
  }

  /** Variables the widgets display (streamed during a live run). */
  outputNames(): string[] {
    const out = new Set<string>();
    for (const w of this.layout.widgets) {
      if (w.kind === "plot") w.names.forEach((n) => out.add(n));
      else if (w.kind !== "slider") out.add(w.name);
    }
    return [...out];
  }

  /** Show the layout of `model` (saved, or `given` from a share link). */
  setModel(model: string | null, given?: Layout | null) {
    if (model === this.model && !given) return;
    this.model = model;
    let l: Layout | null = given ?? null;
    if (!l && model) {
      try {
        const s = localStorage.getItem(KEY(model));
        if (s) {
          const j = JSON.parse(s);
          l = { widgets: (j.widgets ?? []).filter(valid), speed: j.speed > 0 ? j.speed : 1 };
        }
      } catch {
        /* storage unavailable */
      }
    }
    this.layout = l ?? { widgets: [], speed: 1 };
    if (given) this.save();
    this.speedSel.value = String(SPEEDS.includes(this.layout.speed) ? this.layout.speed : 1);
    this.render();
  }

  private save() {
    if (!this.model) return;
    try {
      localStorage.setItem(KEY(this.model), JSON.stringify(this.layout));
    } catch {
      /* storage unavailable: the layout lasts for this page */
    }
  }

  /** A live run started or ended (`stopped`). */
  setRunning(on: boolean) {
    this.runBtn.disabled = on;
    if (on) this.note.textContent = "";
    this.pauseBtn.disabled = !on;
    this.stopBtn.disabled = !on;
    if (!on) {
      this.paused = false;
      this.pauseBtn.textContent = "Pause";
    }
  }

  setStatus(text: string) {
    this.status.textContent = text;
  }

  private async run() {
    const names = this.outputNames();
    this.setRunning(true);
    try {
      await this.hostApi.runLive(this.layout.speed, names);
      this.setStatus("Run finished");
    } finally {
      this.setRunning(false);
    }
  }

  /** New data (a live slice or a finished result). */
  data(time: Float64Array, col: (n: string) => Float64Array | undefined) {
    this.time = time;
    this.col = col;
    if (this.redrawQueued) return;
    this.redrawQueued = true;
    requestAnimationFrame(() => {
      this.redrawQueued = false;
      for (const w of this.widgetEls) w.update();
    });
  }

  private varInfo(name: string): VarSummary | undefined {
    return this.hostApi.vars()?.find((v) => v.name === name);
  }

  private last(name: string): number {
    const c = this.col(name);
    if (c && c.length) return c[c.length - 1];
    const v = this.varInfo(name);
    return v?.value ?? NaN;
  }

  // ---------------------------------------------------------------- adding widgets
  private addForm(kind: Widget["kind"]) {
    const vars = this.hostApi.vars();
    if (!vars) {
      this.hostApi.log("Simulate the model once so the dashboard can list its parameters and variables.", "warn");
      return;
    }
    const cands = kind === "slider" ? vars.filter((v) => v.kind === "param" && v.editable) : vars.filter((v) => v.kind !== "param" && v.name !== "time");
    const list = h("datalist", { id: "dash-cands" });
    for (const v of cands.slice(0, 5000)) list.append(h("option", { value: v.name }));
    const input = h("input", { type: "text", list: "dash-cands", class: "dash-pick", placeholder: kind === "slider" ? "free parameter, e.g. load.J" : kind === "plot" ? "variables, comma-separated" : "variable" });
    const ok = h("button", { class: "primary dash-ok" }, "Add");
    const cancel = h("button", {}, "Cancel");
    const form = h("div", { class: "dash-form" }, h("div", { class: "muted" }, kind === "slider" ? "Slider for a free parameter" : kind === "plot" ? "Mini plot" : kind === "gauge" ? "Gauge" : "Numeric display"), input, list, h("div", { class: "dash-form-btns" }, ok, cancel));
    const done = () => form.remove();
    cancel.onclick = done;
    const add = () => {
      const names = input.value.split(",").map((s) => s.trim()).filter(Boolean);
      const bad = names.filter((n) => !cands.some((v) => v.name === n));
      if (!names.length || bad.length) {
        this.hostApi.log(bad.length ? `Dashboard: ${bad.join(", ")} ${kind === "slider" ? "is not a free parameter of this model" : "is not a result variable"}` : "Dashboard: choose a variable", "warn");
        return;
      }
      done();
      void (async () => {
        if (kind !== "slider") await this.hostApi.fetch(names);
        for (const name of kind === "plot" ? [names.join(",")] : names) this.addWidget(kind, name.split(","));
      })();
    };
    ok.onclick = add;
    input.onkeydown = (e) => e.key === "Enter" && add();
    this.body.prepend(form);
    input.focus();
  }

  /** Add a widget for `names` (one name, or several for a mini plot). */
  addWidget(kind: Widget["kind"], names: string[]) {
    let w: Widget;
    if (kind === "plot") w = { kind, names };
    else if (kind === "number") w = { kind, name: names[0] };
    else {
      const name = names[0];
      let v: number;
      if (kind === "slider") v = this.hostApi.overrides.get(name) ?? this.varInfo(name)?.value ?? 0;
      else {
        const c = this.col(name);
        let lo = Infinity;
        let hi = -Infinity;
        if (c) for (const x of c) {
          if (x < lo) lo = x;
          if (x > hi) hi = x;
        }
        if (lo < hi) {
          const du = displayUnit(this.varInfo(name)?.unit ?? "");
          const a = lo * du.scale + du.offset;
          const b = hi * du.scale + du.offset;
          const pad = 0.1 * (b - a);
          w = { kind, name, min: +(a - pad).toPrecision(3), max: +(b + pad).toPrecision(3) };
          this.push(w);
          return;
        }
        v = this.last(name);
      }
      const range = v === 0 || !isFinite(v) ? [-1, 1] : v > 0 ? [0, 2 * v] : [2 * v, 0];
      w = { kind, name, min: +range[0].toPrecision(3), max: +range[1].toPrecision(3) };
    }
    this.push(w);
  }

  private push(w: Widget) {
    this.layout.widgets.push(w);
    this.save();
    this.render();
  }

  // ---------------------------------------------------------------- rendering
  render() {
    this.widgetEls = [];
    const cards: HTMLElement[] = [];
    if (!this.layout.widgets.length)
      cards.push(
        h(
          "div",
          { class: "dash-empty muted" },
          "Add sliders bound to free parameters and gauges, displays or mini plots bound to variables, then press Run live. Moving a slider during the run changes the parameter at once, without a restart.",
        ),
      );
    this.layout.widgets.forEach((w, i) => cards.push(this.card(w, i)));
    // A form being filled in stays open.
    const form = this.body.querySelector(".dash-form");
    this.body.replaceChildren(...(form ? [form] : []), ...cards);
    for (const w of this.widgetEls) w.update();
  }

  private card(w: Widget, i: number): HTMLElement {
    const title = w.kind === "plot" ? w.names.join(", ") : w.name;
    const unit = w.kind === "plot" ? "" : displayUnit(this.varInfo(w.name)?.unit ?? "").unit;
    const del = h("button", { class: "icon", title: "Remove" }, "✕");
    del.onclick = () => {
      this.layout.widgets.splice(i, 1);
      this.save();
      this.render();
    };
    const card = h("div", { class: `dash-card dash-w-${w.kind}`, "data-name": title }, h("div", { class: "dash-card-head" }, h("span", { class: "dash-name", title }, title), unit ? h("span", { class: "muted" }, ` [${unit}]`) : "", del));
    if (w.kind === "slider") {
      const v0 = this.hostApi.overrides.get(w.name) ?? this.varInfo(w.name)?.value ?? (w.min + w.max) / 2;
      const range = h("input", { type: "range", min: String(w.min), max: String(w.max), step: String((w.max - w.min) / 500 || 0.001), class: "dash-slider" });
      range.value = String(v0);
      const num = h("input", { type: "number", step: "any", class: "dash-num", value: String(v0) });
      const lo = h("input", { type: "number", step: "any", class: "dash-lim", value: String(w.min), title: "Minimum" });
      const hi = h("input", { type: "number", step: "any", class: "dash-lim", value: String(w.max), title: "Maximum" });
      const set = (v: number) => {
        if (!isFinite(v)) return;
        range.value = String(v);
        num.value = String(+v.toPrecision(6));
        this.change(w.name, v);
      };
      range.oninput = () => set(+range.value);
      num.onchange = () => set(+num.value);
      const lim = () => {
        const a = +lo.value;
        const b = +hi.value;
        if (isFinite(a) && isFinite(b) && b > a) {
          w.min = a;
          w.max = b;
          range.min = String(a);
          range.max = String(b);
          range.step = String((b - a) / 500);
          this.save();
        }
      };
      lo.onchange = lim;
      hi.onchange = lim;
      card.append(h("div", { class: "dash-row" }, range, num), h("div", { class: "dash-row dash-lims" }, lo, h("span", { class: "muted" }, "…"), hi));
      this.widgetEls.push({ w, update: () => {} });
    } else if (w.kind === "gauge") {
      const NS = "http://www.w3.org/2000/svg";
      const svg = document.createElementNS(NS, "svg");
      svg.setAttribute("viewBox", "-60 -55 120 70");
      svg.classList.add("dash-gauge");
      const arc = (f0: number, f1: number) => {
        const a0 = Math.PI * (1 - f0);
        const a1 = Math.PI * (1 - f1);
        const r = 45;
        return `M ${r * Math.cos(a0)} ${-r * Math.sin(a0)} A ${r} ${r} 0 0 1 ${r * Math.cos(a1)} ${-r * Math.sin(a1)}`;
      };
      const track = document.createElementNS(NS, "path");
      track.setAttribute("d", arc(0, 1));
      track.setAttribute("class", "g-track");
      const fill = document.createElementNS(NS, "path");
      fill.setAttribute("class", "g-fill");
      const txt = document.createElementNS(NS, "text");
      txt.setAttribute("y", "-4");
      txt.setAttribute("class", "g-val");
      const lim = document.createElementNS(NS, "text");
      lim.setAttribute("y", "12");
      lim.setAttribute("class", "g-lim");
      lim.textContent = `${fmt(w.min)} … ${fmt(w.max)}`;
      svg.append(track, fill, txt, lim);
      card.append(svg);
      const du = displayUnit(this.varInfo(w.name)?.unit ?? "");
      this.widgetEls.push({
        w,
        update: () => {
          const v = this.last(w.name) * du.scale + du.offset;
          const f = Math.max(0, Math.min(1, (v - w.min) / (w.max - w.min)));
          fill.setAttribute("d", isFinite(v) && f > 0.001 ? arc(0, f) : "");
          txt.textContent = fmt(v);
        },
      });
    } else if (w.kind === "number") {
      const val = h("div", { class: "dash-value" });
      card.append(val);
      const du = displayUnit(this.varInfo(w.name)?.unit ?? "");
      this.widgetEls.push({ w, update: () => (val.textContent = fmt(this.last(w.name) * du.scale + du.offset)) });
    } else {
      const c = h("canvas", { class: "dash-plot" });
      card.append(c);
      this.widgetEls.push({ w, update: () => this.miniPlot(c, w.names) });
    }
    return card;
  }

  private miniPlot(c: HTMLCanvasElement, names: string[]) {
    const W = c.clientWidth;
    const H = c.clientHeight;
    if (!W || !H) return;
    const dpr = window.devicePixelRatio || 1;
    c.width = W * dpr;
    c.height = H * dpr;
    const g = c.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    g.clearRect(0, 0, W, H);
    const t = this.time;
    if (!t.length) return;
    const cols = names.map((n) => this.col(n)).filter((x): x is Float64Array => !!x);
    let lo = Infinity;
    let hi = -Infinity;
    for (const y of cols) for (const v of y) {
      if (v < lo) lo = v;
      if (v > hi) hi = v;
    }
    if (!(lo < hi)) {
      lo -= 1;
      hi += 1;
    }
    const t0 = t[0];
    const t1 = Math.max(t[t.length - 1], t0 + 1e-9);
    const X = (x: number) => 2 + ((x - t0) / (t1 - t0)) * (W - 4);
    const Y = (y: number) => H - 3 - ((y - lo) / (hi - lo)) * (H - 6);
    const css = getComputedStyle(c);
    g.strokeStyle = css.getPropertyValue("--grid").trim() || "#ddd";
    g.strokeRect(0.5, 0.5, W - 1, H - 1);
    cols.forEach((y, k) => {
      g.strokeStyle = PALETTE[k % PALETTE.length];
      g.lineWidth = 1.3;
      g.beginPath();
      const n = Math.min(y.length, t.length);
      const stride = Math.max(1, Math.floor(n / (2 * W)));
      for (let i = 0; i < n; i += stride) {
        if (i === 0) g.moveTo(X(t[i]), Y(y[i]));
        else g.lineTo(X(t[i]), Y(y[i]));
      }
      if (n) g.lineTo(X(t[n - 1]), Y(y[n - 1]));
      g.stroke();
    });
    g.fillStyle = css.getPropertyValue("--muted").trim() || "#888";
    g.font = "10px system-ui, sans-serif";
    g.fillText(`${fmt(hi)}`, 4, 11);
    g.fillText(`${fmt(lo)}`, 4, H - 4);
  }

  /** A slider moved: the next run uses the value; a live run gets it at the next slice. */
  private change(name: string, v: number) {
    this.hostApi.overrides.set(name, v);
    if (!this.hostApi.running()) return;
    this.pending.set(name, v);
    if (this.sendTimer) return;
    this.sendTimer = window.setTimeout(async () => {
      this.sendTimer = 0;
      const ps = [...this.pending.entries()];
      this.pending.clear();
      const r = await this.hostApi.setParams(ps);
      if (!r.ok && r.error && r.error !== "no live run") this.hostApi.log(`dashboard: ${r.error}`, "err");
      else if (r.ok) this.note.textContent = `${ps.map(([n, x]) => `${n} = ${fmt(x)}`).join(", ")} applied at t = ${fmt(r.t ?? NaN)} s`;
    }, 50);
  }
}
