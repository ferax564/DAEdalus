// Linear analysis view: linearize the open model in the worker and show the Bode plot,
// the pole-zero map, the step (or impulse) response and a summary with margins, PID
// suggestions, balanced truncation and LQR.
import { api } from "./rpc";
import type { BodeData, Complex, LinearOptions, LinearPair, LinearResult } from "./types";

const COLORS = { main: "#0b5fff", reduced: "#d9480f", lqr: "#2b8a3e", zero: "#ae3ec9", ref: "#868e96" };

interface Line {
  x: ArrayLike<number>;
  y: ArrayLike<number>;
  color: string;
  label: string;
  dash?: number[];
}

interface Marker {
  x: number;
  y: number;
  shape: "x" | "o" | "tri";
  color: string;
}

interface ChartSpec {
  title: string;
  xLabel: string;
  yLabel: string;
  logX?: boolean;
  lines?: Line[];
  markers?: Marker[];
  /** Same scale on both axes (pole-zero map). */
  equal?: boolean;
  /** Fixed y ticks spacing (phase: 45 or 90 deg). */
  yStep?: number;
  /** Horizontal and vertical reference lines. */
  hRefs?: number[];
  vRefs?: number[];
  /** Zoom factor around the view centre (pole-zero map, mouse wheel). */
  zoom?: number;
}

function cssVar(name: string, fallback: string) {
  return getComputedStyle(document.documentElement).getPropertyValue(name).trim() || fallback;
}

function ticks(lo: number, hi: number, n: number, step?: number): number[] {
  if (!(hi > lo)) return [lo];
  let s = step ?? 0;
  if (!s) {
    const raw = (hi - lo) / n;
    const p = Math.pow(10, Math.floor(Math.log10(raw)));
    s = [1, 2, 2.5, 5, 10].map((m) => m * p).find((v) => v >= raw) ?? 10 * p;
  }
  while ((hi - lo) / s > 3 * n) s *= 2;
  const out: number[] = [];
  for (let v = Math.ceil(lo / s - 1e-9) * s; v <= hi + s * 1e-9; v += s) out.push(Math.abs(v) < s * 1e-9 ? 0 : v);
  return out;
}

export const fmt = (v: number | null | undefined, digits = 4): string => {
  if (v === null || v === undefined || !isFinite(v)) return "–";
  const a = Math.abs(v);
  if (a !== 0 && (a >= 1e5 || a < 1e-3)) return v.toExponential(digits - 1);
  return String(+v.toPrecision(digits));
};

const fmtC = ([re, im]: Complex) => (im === 0 ? fmt(re) : `${fmt(re)} ${im < 0 ? "−" : "+"} ${fmt(Math.abs(im))}j`);

/** A canvas chart with linear or logarithmic x axis, lines and point markers. */
class Chart {
  readonly canvas = document.createElement("canvas");
  spec: ChartSpec | null = null;
  private hover: { x: number; y: number } | null = null;
  onZoom: ((factor: number) => void) | null = null;

  constructor(host: HTMLElement) {
    host.appendChild(this.canvas);
    this.canvas.addEventListener("mousemove", (e) => {
      const r = this.canvas.getBoundingClientRect();
      this.hover = { x: e.clientX - r.left, y: e.clientY - r.top };
      this.draw();
    });
    this.canvas.addEventListener("mouseleave", () => {
      this.hover = null;
      this.draw();
    });
    this.canvas.addEventListener(
      "wheel",
      (e) => {
        if (!this.onZoom) return;
        e.preventDefault();
        this.onZoom(e.deltaY < 0 ? 0.8 : 1.25);
      },
      { passive: false },
    );
  }

  draw() {
    const cv = this.canvas;
    const dpr = window.devicePixelRatio || 1;
    const W = cv.clientWidth, H = cv.clientHeight;
    if (!W || !H) return;
    if (cv.width !== Math.round(W * dpr) || cv.height !== Math.round(H * dpr)) {
      cv.width = Math.round(W * dpr);
      cv.height = Math.round(H * dpr);
    }
    const g = cv.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const fg = cssVar("--fg", "#1d2330"), muted = cssVar("--muted", "#6b7385"), grid = cssVar("--grid", "#eceef2"), line = cssVar("--line", "#d9dde5");
    g.fillStyle = cssVar("--panel", "#fff");
    g.fillRect(0, 0, W, H);
    const s = this.spec;
    if (!s) return;
    g.font = "12px system-ui, sans-serif";
    const L = 58, R = 12, T = 22, B = 34;
    const w = W - L - R, h = H - T - B;
    g.fillStyle = fg;
    g.textAlign = "left";
    g.fillText(s.title, L, 15);
    if (w < 20 || h < 20) return;
    const tx = (v: number) => (s.logX ? Math.log10(v) : v);
    // data ranges
    let x0 = Infinity, x1 = -Infinity, y0 = Infinity, y1 = -Infinity;
    for (const ln of s.lines ?? []) {
      for (let i = 0; i < ln.x.length; i++) {
        const x = tx(ln.x[i]), y = ln.y[i];
        if (!isFinite(x) || !isFinite(y)) continue;
        x0 = Math.min(x0, x);
        x1 = Math.max(x1, x);
        y0 = Math.min(y0, y);
        y1 = Math.max(y1, y);
      }
    }
    for (const m of s.markers ?? []) {
      x0 = Math.min(x0, m.x);
      x1 = Math.max(x1, m.x);
      y0 = Math.min(y0, m.y);
      y1 = Math.max(y1, m.y);
    }
    for (const r of s.hRefs ?? []) {
      if (s.equal) continue;
      y0 = Math.min(y0, r);
      y1 = Math.max(y1, r);
    }
    if (!isFinite(x0)) {
      x0 = -1;
      x1 = 1;
      y0 = -1;
      y1 = 1;
    }
    if (s.equal) {
      // include the origin and keep a square scale
      x0 = Math.min(x0, 0);
      x1 = Math.max(x1, 0);
      y0 = Math.min(y0, 0);
      y1 = Math.max(y1, 0);
      const cx = (x0 + x1) / 2, cy = (y0 + y1) / 2;
      let span = Math.max((x1 - x0) / w, (y1 - y0) / h) * 1.15 * (s.zoom ?? 1);
      if (!(span > 0)) span = 2 / Math.min(w, h);
      x0 = cx - (span * w) / 2;
      x1 = cx + (span * w) / 2;
      y0 = cy - (span * h) / 2;
      y1 = cy + (span * h) / 2;
    } else {
      if (!(y1 > y0)) {
        const d = Math.abs(y0) * 0.1 || 1;
        y0 -= d;
        y1 += d;
      }
      const pad = (y1 - y0) * 0.06;
      y0 -= pad;
      y1 += pad;
      if (!(x1 > x0)) {
        x0 -= 1;
        x1 += 1;
      }
    }
    const px = (x: number) => L + ((x - x0) / (x1 - x0)) * w;
    const py = (y: number) => T + h - ((y - y0) / (y1 - y0)) * h;
    // grid
    g.strokeStyle = grid;
    g.lineWidth = 1;
    g.fillStyle = muted;
    g.textAlign = "center";
    const xt = s.logX ? ticks(Math.floor(x0), Math.ceil(x1), 8, 1).filter((v) => v >= x0 - 1e-9 && v <= x1 + 1e-9) : ticks(x0, x1, Math.max(3, Math.floor(w / 80)));
    if (s.logX) {
      // minor decade lines
      g.beginPath();
      for (let d = Math.floor(x0); d < x1; d++)
        for (let k = 2; k < 10; k++) {
          const v = d + Math.log10(k);
          if (v > x0 && v < x1) {
            g.moveTo(px(v) + 0.5, T);
            g.lineTo(px(v) + 0.5, T + h);
          }
        }
      g.globalAlpha = 0.5;
      g.stroke();
      g.globalAlpha = 1;
    }
    g.beginPath();
    for (const v of xt) {
      g.moveTo(Math.round(px(v)) + 0.5, T);
      g.lineTo(Math.round(px(v)) + 0.5, T + h);
    }
    const yt = ticks(y0, y1, Math.max(3, Math.floor(h / 40)), s.yStep);
    for (const v of yt) {
      g.moveTo(L, Math.round(py(v)) + 0.5);
      g.lineTo(L + w, Math.round(py(v)) + 0.5);
    }
    g.stroke();
    for (const v of xt) g.fillText(s.logX ? (Math.abs(v) <= 3 ? String(+Math.pow(10, v).toPrecision(3)) : `1e${v}`) : fmt(v), px(v), T + h + 14);
    g.textAlign = "right";
    for (const v of yt) g.fillText(fmt(v), L - 5, py(v) + 4);
    g.textAlign = "center";
    g.fillText(s.xLabel, L + w / 2, H - 4);
    g.save();
    g.translate(12, T + h / 2);
    g.rotate(-Math.PI / 2);
    g.fillText(s.yLabel, 0, 0);
    g.restore();
    // reference lines
    g.strokeStyle = COLORS.ref;
    g.setLineDash([4, 3]);
    g.beginPath();
    for (const r of s.hRefs ?? []) {
      if (r < y0 || r > y1) continue;
      g.moveTo(L, py(r));
      g.lineTo(L + w, py(r));
    }
    for (const r of s.vRefs ?? []) {
      if (r < x0 || r > x1) continue;
      g.moveTo(px(r), T);
      g.lineTo(px(r), T + h);
    }
    g.stroke();
    g.setLineDash([]);
    g.strokeStyle = line;
    g.strokeRect(L + 0.5, T + 0.5, w, h);
    // data
    g.save();
    g.beginPath();
    g.rect(L, T, w, h);
    g.clip();
    for (const ln of s.lines ?? []) {
      g.strokeStyle = ln.color;
      g.lineWidth = 1.6;
      g.setLineDash(ln.dash ?? []);
      g.beginPath();
      let pen = false;
      for (let i = 0; i < ln.x.length; i++) {
        const x = tx(ln.x[i]), y = ln.y[i];
        if (!isFinite(x) || !isFinite(y)) {
          pen = false;
          continue;
        }
        const X = px(x), Y = Math.max(-1e4, Math.min(1e4, py(y)));
        if (pen) g.lineTo(X, Y);
        else g.moveTo(X, Y);
        pen = true;
      }
      g.stroke();
    }
    g.setLineDash([]);
    g.lineWidth = 1.8;
    for (const m of s.markers ?? []) {
      const X = px(m.x), Y = py(m.y), r = 5;
      g.strokeStyle = m.color;
      g.beginPath();
      if (m.shape === "x") {
        g.moveTo(X - r, Y - r);
        g.lineTo(X + r, Y + r);
        g.moveTo(X - r, Y + r);
        g.lineTo(X + r, Y - r);
      } else if (m.shape === "o") g.arc(X, Y, r, 0, 2 * Math.PI);
      else {
        g.moveTo(X, Y - r);
        g.lineTo(X + r, Y + r * 0.8);
        g.lineTo(X - r, Y + r * 0.8);
        g.closePath();
      }
      g.stroke();
    }
    g.restore();
    // legend
    const named = (s.lines ?? []).filter((l) => l.label);
    if (named.length > 1) {
      g.textAlign = "left";
      let xx = L + w - 8;
      for (const l of [...named].reverse()) {
        const tw = g.measureText(l.label).width;
        xx -= tw + 26;
        g.strokeStyle = l.color;
        g.setLineDash(l.dash ?? []);
        g.beginPath();
        g.moveTo(xx, 11);
        g.lineTo(xx + 18, 11);
        g.stroke();
        g.setLineDash([]);
        g.fillStyle = fg;
        g.fillText(l.label, xx + 22, 15);
      }
    }
    // hover readout
    if (this.hover && this.hover.x >= L && this.hover.x <= L + w && this.hover.y >= T && this.hover.y <= T + h) {
      const xv = x0 + ((this.hover.x - L) / w) * (x1 - x0);
      const yv = y0 + ((T + h - this.hover.y) / h) * (y1 - y0);
      g.strokeStyle = muted;
      g.setLineDash([2, 3]);
      g.beginPath();
      g.moveTo(this.hover.x + 0.5, T);
      g.lineTo(this.hover.x + 0.5, T + h);
      g.stroke();
      g.setLineDash([]);
      let text = `${s.logX ? fmt(Math.pow(10, xv)) : fmt(xv)}, ${fmt(yv)}`;
      const first = s.lines?.[0];
      if (first && !s.equal) {
        // value of the first line at the cursor
        let best = -1, bd = Infinity;
        for (let i = 0; i < first.x.length; i++) {
          const d = Math.abs(tx(first.x[i]) - xv);
          if (d < bd) {
            bd = d;
            best = i;
          }
        }
        if (best >= 0) {
          text = `${s.xLabel.split(" ")[0]} ${fmt(first.x[best])}: ${fmt(first.y[best])}`;
          g.fillStyle = first.color;
          g.beginPath();
          g.arc(px(tx(first.x[best])), py(first.y[best]), 3, 0, 2 * Math.PI);
          g.fill();
        }
      }
      g.font = "11px system-ui, sans-serif";
      const tw = g.measureText(text).width;
      const bx = Math.min(this.hover.x + 8, L + w - tw - 8);
      g.fillStyle = cssVar("--panel", "#fff");
      g.globalAlpha = 0.9;
      g.fillRect(bx - 3, T + 3, tw + 6, 16);
      g.globalAlpha = 1;
      g.fillStyle = fg;
      g.textAlign = "left";
      g.fillText(text, bx, T + 15);
    }
  }
}

export class LinearView {
  onLog: (msg: string, cls?: string) => void = () => {};
  /** Parameter overrides of the simulation setup. */
  params: () => [string, number][] = () => [];
  private model = "";
  private result: LinearResult | null = null;
  private busy = false;
  private pzZoom = 1;
  private els: Record<string, HTMLElement> = {};
  private mag!: Chart;
  private phase!: Chart;
  private pz!: Chart;
  private time!: Chart;
  private nyquist = false;
  /** Outputs picked with "Other output" for this model. */
  private extras: string[] = [];
  private impulse = false;

  constructor(private host: HTMLElement) {
    host.innerHTML = `
      <div class="lin-bar">
        <label>Input <select data-k="input"></select></label>
        <label>Output <select data-k="output"></select></label>
        <label title="Any continuous variable (or state) as the output">Other output <input data-k="other" list="lin-candidates" placeholder="variable…" size="16" /></label>
        <datalist id="lin-candidates"></datalist>
        <label title="Linearize after simulating to this time (default: the start time)">at t <input data-k="at" type="number" step="any" placeholder="start" style="width:5.5em" /></label>
        <label title="Balanced truncation to this many states">Reduce to <input data-k="reduce" type="number" min="1" placeholder="–" style="width:3.5em" /></label>
        <label title="LQR weights: Q = q I, R = r I">LQR q <input data-k="q" type="text" placeholder="–" size="4" /> r <input data-k="r" type="text" placeholder="–" size="4" /></label>
        <button data-k="run" class="primary">Analyze</button>
        <span data-k="status" class="muted"></span>
      </div>
      <div class="lin-grid">
        <div class="lin-cell lin-bode">
          <div class="lin-head"><span class="seg"><button data-k="tab-bode" class="active">Bode</button><button data-k="tab-nyq">Nyquist</button></span></div>
          <div data-k="bode" class="lin-stack"></div>
        </div>
        <div class="lin-cell"><div class="lin-head"><span class="muted">× poles, ○ zeros${""}, △ LQR closed loop · wheel to zoom</span><button data-k="pz-fit">Fit</button></div><div data-k="pz" class="lin-plot"></div></div>
        <div class="lin-cell"><div class="lin-head"><span class="seg"><button data-k="tab-step" class="active">Step</button><button data-k="tab-imp">Impulse</button></span></div><div data-k="time" class="lin-plot"></div></div>
        <div class="lin-cell lin-summary" data-k="summary"><p class="muted">Pick an input and an output, then Analyze.</p></div>
      </div>`;
    host.querySelectorAll<HTMLElement>("[data-k]").forEach((e) => (this.els[e.dataset.k!] = e));
    const bode = this.els.bode;
    const m1 = document.createElement("div");
    const m2 = document.createElement("div");
    m1.className = m2.className = "lin-plot";
    bode.append(m1, m2);
    this.mag = new Chart(m1);
    this.phase = new Chart(m2);
    this.pz = new Chart(this.els.pz);
    this.time = new Chart(this.els.time);
    this.pz.onZoom = (f) => {
      this.pzZoom *= f;
      this.render();
    };
    this.els["pz-fit"].onclick = () => {
      this.pzZoom = 1;
      this.render();
    };
    this.els.run.onclick = () => this.run();
    const seg = (on: string, off: string, set: (v: boolean) => void) => {
      this.els[on].onclick = () => {
        set(true);
        this.els[on].classList.add("active");
        this.els[off].classList.remove("active");
        this.render();
      };
      this.els[off].onclick = () => {
        set(false);
        this.els[off].classList.add("active");
        this.els[on].classList.remove("active");
        this.render();
      };
    };
    seg("tab-nyq", "tab-bode", (v) => (this.nyquist = v));
    seg("tab-imp", "tab-step", (v) => (this.impulse = v));
    for (const k of ["input", "output"]) this.els[k].addEventListener("change", () => this.run());
    this.els.other.addEventListener("change", () => this.run());
    for (const k of ["at", "reduce", "q", "r"]) this.els[k].addEventListener("keydown", (e) => (e as KeyboardEvent).key === "Enter" && this.run());
    new ResizeObserver(() => this.render()).observe(host);
  }

  /** Show the view for `model`; analyzes it the first time. */
  async show(model: string) {
    if (model !== this.model) {
      this.model = model;
      this.result = null;
      this.pzZoom = 1;
      this.extras = [];
      // reduction and LQR settings belong to the model they were made for
      for (const k of ["other", "reduce", "q", "r"]) (this.els[k] as HTMLInputElement).value = "";
      this.fillSelect(this.els.input as HTMLSelectElement, [], "");
      this.fillSelect(this.els.output as HTMLSelectElement, [], "");
      this.els.summary.innerHTML = `<p class="muted">Linearizing ${model}…</p>`;
      this.render();
      await this.run();
    } else this.render();
  }

  private fillSelect(sel: HTMLSelectElement, groups: [string, string[]][] | string[], value: string) {
    sel.innerHTML = "";
    const add = (parent: HTMLElement, n: string) => {
      const o = document.createElement("option");
      o.value = o.textContent = n;
      parent.appendChild(o);
    };
    for (const g of groups) {
      if (typeof g === "string") add(sel, g);
      else if (g[1].length) {
        const og = document.createElement("optgroup");
        og.label = g[0];
        g[1].forEach((n) => add(og, n));
        sel.appendChild(og);
      }
    }
    if (value) sel.value = value;
  }

  private options(input: string, output: string): LinearOptions {
    const o: LinearOptions = { bode: true, step: true, params: this.params() };
    if (input) o.inputs = [input];
    if (output) o.outputs = [output];
    const at = parseFloat((this.els.at as HTMLInputElement).value);
    if (isFinite(at)) o.at = at;
    const red = parseInt((this.els.reduce as HTMLInputElement).value, 10);
    if (red > 0) o.reduce = red;
    const q = parseFloat((this.els.q as HTMLInputElement).value), r = parseFloat((this.els.r as HTMLInputElement).value);
    if (q > 0 || r > 0) o.lqr = [q > 0 ? q : 1, r > 0 ? r : 1];
    return o;
  }

  /** Linearize and analyze the selected pair. */
  async run() {
    if (!this.model || this.busy) return;
    this.busy = true;
    const model = this.model;
    (this.els.run as HTMLButtonElement).disabled = true;
    this.els.status.textContent = "Analyzing…";
    try {
      const other = (this.els.other as HTMLInputElement).value.trim();
      let input = (this.els.input as HTMLSelectElement).value;
      let output = other || (this.els.output as HTMLSelectElement).value;
      const t0 = performance.now();
      let r = await api.linearAnalysis(model, this.options(input, output));
      // First run: pick the first input and the first output (or state) and analyze that pair.
      if (!input || !output) {
        input = input || r.inputs[0] || "";
        output = output || r.outputs[0] || r.states[0] || "";
        if (input && output && (r.analysis?.pairs.length !== 1 || !r.ok)) r = await api.linearAnalysis(model, this.options(input, output));
      }
      if (model !== this.model) return;
      this.result = r;
      this.fillSelect(this.els.input as HTMLSelectElement, r.inputs, input);
      const outs: [string, string[]][] = [
        ["Outputs", r.outputs],
        ["States", r.states.filter((s) => !r.outputs.includes(s))],
      ];
      if (other && r.ok && !r.outputs.includes(other) && !r.states.includes(other) && !this.extras.includes(other)) this.extras.push(other);
      outs.push(["Other", this.extras]);
      this.fillSelect(this.els.output as HTMLSelectElement, outs, output);
      if (other) (this.els.other as HTMLInputElement).value = "";
      const dl = this.host.querySelector("datalist")!;
      dl.innerHTML = r.candidates
        .slice(0, 5000)
        .map((c) => `<option value="${c.replace(/"/g, "&quot;")}"></option>`)
        .join("");
      const ms = performance.now() - t0;
      if (!r.ok) {
        this.els.status.textContent = "";
        this.els.summary.innerHTML = `<p class="err">${escapeHtml(r.error ?? "linear analysis failed")}</p>`;
        this.onLog(`Linear analysis of ${model}: ${r.error}`, "err");
      } else {
        const a = r.analysis!;
        this.els.status.textContent = `${a.states.length} states · ${ms.toFixed(0)} ms`;
        this.onLog(`Linear analysis of ${model} at t = ${a.time}: ${a.states.length} states, ${a.stable ? "stable" : "unstable"}; ${ms.toFixed(0)} ms`, "ok");
      }
      this.render();
    } catch (e) {
      this.onLog(String(e), "err");
    } finally {
      this.busy = false;
      (this.els.run as HTMLButtonElement).disabled = false;
      if (this.els.status.textContent === "Analyzing…") this.els.status.textContent = "";
    }
  }

  private pair(): LinearPair | null {
    return this.result?.analysis?.pairs[0] ?? null;
  }

  /** Redraw Bode, pole-zero, and response canvases after a theme change. */
  redraw() {
    this.render();
  }

  render() {
    const a = this.result?.analysis ?? null;
    const p = this.pair();
    const b = p?.bode ?? null;
    const rb = p?.reduced_bode ?? null;
    const name = p ? `${p.output} / ${p.input}` : "";
    const red = a?.reduction ? `reduced (${a.reduction.order})` : "";
    if (this.nyquist) {
      this.phase.canvas.parentElement!.classList.add("hidden");
      this.mag.spec = b
        ? {
            title: `Nyquist ${name}`,
            xLabel: "Re",
            yLabel: "Im",
            equal: true,
            lines: [
              { x: b.re, y: b.im, color: COLORS.main, label: "ω > 0" },
              { x: b.re, y: b.im.map((v) => -v), color: COLORS.main, label: "ω < 0", dash: [4, 3] },
              ...(rb ? [{ x: rb.re, y: rb.im, color: COLORS.reduced, label: red, dash: [6, 3] }] : []),
            ],
            markers: [{ x: -1, y: 0, shape: "x", color: COLORS.zero }],
          }
        : null;
    } else {
      this.phase.canvas.parentElement!.classList.remove("hidden");
      const lines = (f: (d: BodeData) => number[]) => [
        ...(b ? [{ x: b.omega, y: f(b), color: COLORS.main, label: name }] : []),
        ...(rb ? [{ x: rb.omega, y: f(rb), color: COLORS.reduced, label: red, dash: [6, 3] }] : []),
      ];
      const mg = p?.margins;
      this.mag.spec = { title: `Magnitude ${name}`, xLabel: "ω [rad/s]", yLabel: "dB", logX: true, lines: lines((d) => d.mag_db), hRefs: [0], vRefs: mg?.gain_crossover ? [Math.log10(mg.gain_crossover)] : [] };
      this.phase.spec = { title: "Phase", xLabel: "ω [rad/s]", yLabel: "deg", logX: true, lines: lines((d) => d.phase_deg), yStep: 45, hRefs: [-180], vRefs: mg?.phase_crossover ? [Math.log10(mg.phase_crossover)] : [] };
    }
    const markers: Marker[] = [];
    for (const [re, im] of a?.poles ?? []) markers.push({ x: re, y: im, shape: "x", color: COLORS.main });
    for (const [re, im] of p?.zeros ?? []) markers.push({ x: re, y: im, shape: "o", color: COLORS.zero });
    for (const [re, im] of a?.lqr?.closed_loop_poles ?? []) markers.push({ x: re, y: im, shape: "tri", color: COLORS.lqr });
    for (const [re, im] of a?.reduction?.poles ?? []) markers.push({ x: re, y: im, shape: "x", color: COLORS.reduced });
    this.pz.spec = a ? { title: "Pole-zero map", xLabel: "Re [1/s]", yLabel: "Im [rad/s]", equal: true, markers, hRefs: [0], vRefs: [0], zoom: this.pzZoom } : null;
    const td = this.impulse ? p?.impulse : p?.step;
    const rs = this.impulse ? null : p?.reduced_step;
    const fv = p?.step_info?.final_value;
    this.time.spec = td
      ? {
          title: `${this.impulse ? "Impulse" : "Step"} response ${p!.output} ← ${p!.input}`,
          xLabel: "t [s]",
          yLabel: p!.output,
          lines: [{ x: td.t, y: td.y, color: COLORS.main, label: "linear model" }, ...(rs ? [{ x: rs.t, y: rs.y, color: COLORS.reduced, label: red, dash: [6, 3] }] : [])],
          hRefs: !this.impulse && fv !== null && fv !== undefined ? [fv] : [],
        }
      : null;
    for (const c of [this.mag, this.phase, this.pz, this.time]) c.draw();
    if (a && p) this.els.summary.innerHTML = this.summaryHtml();
  }

  private summaryHtml(): string {
    const a = this.result!.analysis!;
    const p = a.pairs[0];
    const row = (k: string, v: string) => `<tr><th>${k}</th><td>${v}</td></tr>`;
    const m = p.margins;
    const si = p.step_info;
    let h = `<h4>${escapeHtml(p.output)} ← ${escapeHtml(p.input)} <span class="muted">at t = ${fmt(a.time)}, ${a.states.length} states, ${a.stable ? '<span class="ok">stable</span>' : '<span class="err">unstable</span>'}</span></h4><table>`;
    h += row("DC gain", fmt(p.dc_gain));
    h += row("Relative degree", p.relative_degree === null ? "transfer function ≡ 0" : String(p.relative_degree));
    h += row("Zeros", p.zeros.length ? p.zeros.map(fmtC).join(", ") : "none");
    h += row("Gain margin", m.gain_margin_db === null ? "∞" : `${fmt(m.gain_margin_db)} dB at ${fmt(m.phase_crossover)} rad/s`);
    h += row("Phase margin", m.phase_margin_deg === null ? "∞" : `${fmt(m.phase_margin_deg)}° at ${fmt(m.gain_crossover)} rad/s`);
    if (si) {
      h += row("Rise time (10–90 %)", si.rise_time === null ? "–" : `${fmt(si.rise_time)} s`);
      h += row("Settling time (2 %)", si.settling_time === null ? "–" : `${fmt(si.settling_time)} s`);
      h += row("Overshoot", si.overshoot === null ? "–" : `${fmt(si.overshoot, 3)} %`);
    }
    h += "</table>";
    if (p.pid) {
      const f = p.pid.fopdt;
      h += `<h4>PID suggestion <span class="muted">FOPDT fit K = ${fmt(f.k)}, T = ${fmt(f.t)} s, L = ${fmt(f.l)} s</span></h4><table>`;
      for (const g of [p.pid.simc, ...(p.pid.ziegler_nichols ? [p.pid.ziegler_nichols] : [])])
        h += row(escapeHtml(g.method), `K<sub>p</sub> = ${fmt(g.kp)}, T<sub>i</sub> = ${fmt(g.ti)} s${g.td ? `, T<sub>d</sub> = ${fmt(g.td)} s` : ""}`);
      h += "</table>";
      if (p.pid.note) h += `<p class="muted">${escapeHtml(p.pid.note)}</p>`;
    }
    const poles = a.poles.slice(0, 12).map(fmtC).join(", ") + (a.poles.length > 12 ? `, … (${a.poles.length})` : "");
    h += `<h4>Poles</h4><p class="mono">${poles || "none"}</p>`;
    if (a.reduction) {
      const r = a.reduction;
      const max = r.hankel_singular_values[0] || 1;
      h += `<h4>Balanced truncation to ${r.order} states <span class="muted">H∞ error ≤ ${fmt(r.error_bound)}</span></h4><div class="hsv">`;
      r.hankel_singular_values.slice(0, 40).forEach((s, i) => {
        const pct = Math.max(0.03, (Math.log10(Math.max(s / max, 1e-12)) + 12) / 12) * 100;
        h += `<div class="hsv-bar${i < r.order ? " kept" : ""}" style="height:${Math.min(100, pct).toFixed(0)}%" title="σ${i + 1} = ${fmt(s)}"></div>`;
      });
      h += `</div><p class="muted">Hankel singular values (log scale); kept states highlighted.</p>`;
    }
    if (a.reduction_error) h += `<p class="err">Balanced truncation: ${escapeHtml(a.reduction_error)}</p>`;
    if (a.lqr) {
      h += `<h4>LQR <span class="muted">Q = ${a.lqr.q}·I, R = ${a.lqr.r}·I, u = −K x</span></h4><p class="mono">K = [${a.lqr.k.map((r) => r.map((v) => fmt(v)).join(", ")).join("; ")}]</p>`;
      h += `<p class="muted">states: ${a.states.map(escapeHtml).join(", ")}</p>`;
    }
    if (a.lqr_error) h += `<p class="err">LQR: ${escapeHtml(a.lqr_error)}</p>`;
    return h;
  }
}

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}
