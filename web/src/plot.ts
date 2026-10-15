// Canvas plot window: stacked subplots, min/max decimation per pixel column so a million
// points draw as fast as a thousand, crosshair readout, drag-to-zoom and parametric curves.

export interface Series {
  name: string;
  unit: string;
  x: Float64Array;
  y: Float64Array;
  color: string;
  /** Display transform (e.g. rad/s -> rpm). */
  scale?: number;
  offset?: number;
  displayUnit?: string;
  /** Overlay from a stored run (experiments.ts): kept as is when a new result arrives. */
  run?: string;
  /** Filled band drawn behind the line (e.g. the 5–95 % range of a Monte Carlo run). */
  band?: { lo: Float64Array; hi: Float64Array };
  /** Draw markers at the points instead of a line (measured data). */
  points?: boolean;
  /** Expression over result variables (signal.ts): evaluated again for every new result. */
  expr?: string;
  /** Line dash pattern. */
  dash?: number[];
}

export interface Subplot {
  series: Series[];
  /** Parametric plot: the name of the x variable, if not time. */
  xName?: string;
  xUnit?: string;
  /** Zoomed x range of a parametric subplot (time subplots share one range). */
  xZoom?: [number, number];
  /** Logarithmic y axis (values <= 0 are left out). */
  logY?: boolean;
}

const PALETTE = ["#0b5fff", "#d9480f", "#2b8a3e", "#ae3ec9", "#f08c00", "#1098ad", "#c2255c", "#5c940d", "#364fc7", "#862e9c"];

/** Dymola-style display units for common SI units. */
export function displayUnit(unit: string): { unit: string; scale: number; offset: number } {
  switch (unit) {
    case "rad/s":
      return { unit: "rpm", scale: 60 / (2 * Math.PI), offset: 0 };
    case "rev/min":
      return { unit: "rpm", scale: 1, offset: 0 };
    case "rad":
      return { unit: "deg", scale: 180 / Math.PI, offset: 0 };
    case "Pa":
      return { unit: "bar", scale: 1e-5, offset: 0 };
    case "K":
      return { unit: "degC", scale: 1, offset: -273.15 };
    case "m3/s":
      return { unit: "l/min", scale: 60000, offset: 0 };
    case "J":
      return { unit: "kJ", scale: 1e-3, offset: 0 };
    default:
      return { unit, scale: 1, offset: 0 };
  }
}

function niceTicks(lo: number, hi: number, n: number): number[] {
  if (!(hi > lo)) {
    const d = Math.abs(lo) * 0.1 || 1;
    lo -= d;
    hi += d;
  }
  const raw = (hi - lo) / n;
  const p = Math.pow(10, Math.floor(Math.log10(raw)));
  const step = [1, 2, 2.5, 5, 10].map((m) => m * p).find((s) => s >= raw) ?? 10 * p;
  const out: number[] = [];
  for (let v = Math.ceil(lo / step) * step; v <= hi + step * 1e-9; v += step) out.push(Math.abs(v) < step * 1e-9 ? 0 : v);
  return out;
}

const fmtTick = (v: number) => {
  const a = Math.abs(v);
  if (a !== 0 && (a >= 1e5 || a < 1e-3)) return v.toExponential(1);
  return String(+v.toPrecision(6));
};

/** Rectangle of one subplot cell in CSS pixels. */
interface Cell {
  x: number;
  y: number;
  w: number;
  h: number;
}

export class PlotWindow {
  readonly canvas: HTMLCanvasElement;
  subplots: Subplot[] = [{ series: [] }];
  active = 0;
  /** Matrix layout (rows x cols); null stacks the subplots in one column. Extra subplots
   * add rows. */
  layout: { rows: number; cols: number } | null = null;
  /** Time range shown while a live run streams in (start and stop time), unless zoomed. */
  liveRange: [number, number] | null = null;
  private xRange: [number, number] | null = null;
  private hover: { px: number; py: number } | null = null;
  private dragStart: number | null = null;
  private dragNow: number | null = null;
  /** Subplot the drag started in. */
  private dragSub = 0;
  /** Shift+drag pans the x axis: where it started and the range then. */
  private pan: { px: number; range: [number, number]; sub: number } | null = null;
  private colorIndex = 0;
  onChange: () => void = () => {};
  /** Called after every draw (the visible range may have changed). */
  onView: () => void = () => {};
  /** Vertical marker lines at these times (time subplots). */
  markers: { t: number; color: string }[] = [];
  /** Shown when nothing is plotted. */
  emptyText = "Tick variables in the variable browser to plot them";
  private host: HTMLElement;
  private resizeObs: ResizeObserver;

  constructor(host: HTMLElement) {
    this.host = host;
    this.canvas = document.createElement("canvas");
    this.canvas.className = "plot";
    host.appendChild(this.canvas);
    this.resizeObs = new ResizeObserver(() => this.draw());
    this.resizeObs.observe(host);
    // Colours come from CSS variables: redraw when the theme changes.
    window.matchMedia?.("(prefers-color-scheme: dark)").addEventListener?.("change", () => this.draw());
    new MutationObserver(() => this.draw()).observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-theme", "class"],
    });
    const pos = (e: { clientX: number; clientY: number }) => {
      const r = this.canvas.getBoundingClientRect();
      return { px: e.clientX - r.left, py: e.clientY - r.top };
    };
    this.canvas.addEventListener("pointermove", (e) => {
      this.hover = pos(e);
      if (this.pan) {
        const sp = this.subplots[this.pan.sub];
        const { width } = this.area(this.pan.sub);
        const [a, b] = this.pan.range;
        const d = ((this.hover.px - this.pan.px) / width) * (b - a);
        const r: [number, number] = [a - d, b - d];
        if (sp?.xName) sp.xZoom = r;
        else this.xRange = r;
      } else if (this.dragStart !== null) this.dragNow = this.hover.px;
      this.draw();
    });
    this.canvas.addEventListener("pointerleave", () => {
      this.hover = null;
      this.draw();
    });
    this.canvas.addEventListener("pointerdown", (e) => {
      const { px, py } = pos(e);
      const idx = this.subplotAt(px, py);
      if (idx !== null) {
        if (idx !== this.active) {
          this.active = idx;
          this.onChange();
        }
        this.dragSub = idx;
      }
      this.canvas.setPointerCapture(e.pointerId);
      if (e.shiftKey && idx !== null) {
        const sp = this.subplots[idx];
        this.pan = { px, range: sp.xName ? this.parametricRange(sp) : this.currentXRange(), sub: idx };
        return;
      }
      this.dragStart = px;
      this.dragNow = px;
    });
    this.canvas.addEventListener("pointerup", () => {
      if (this.pan) {
        this.pan = null;
        this.draw();
        return;
      }
      if (this.dragStart !== null && this.dragNow !== null && Math.abs(this.dragNow - this.dragStart) > 6) {
        const [a, b] = [this.dragStart, this.dragNow].sort((p, q) => p - q);
        // A parametric subplot zooms its own x axis; time subplots share theirs.
        const sp = this.subplots[this.dragSub];
        const xr = sp?.xName ? this.parametricRange(sp) : this.currentXRange();
        const { left, width } = this.area(this.dragSub);
        const t = (px: number) => xr[0] + ((px - left) / width) * (xr[1] - xr[0]);
        if (sp?.xName) sp.xZoom = [t(a), t(b)];
        else this.xRange = [t(a), t(b)];
      }
      this.dragStart = this.dragNow = null;
      this.draw();
    });
    this.canvas.addEventListener("dblclick", () => {
      const sp = this.subplots[this.active];
      if (sp?.xName && sp.xZoom) sp.xZoom = undefined;
      else this.xRange = null;
      this.draw();
    });
    // Wheel zooms the x axis around the cursor (every time subplot together).
    this.canvas.addEventListener(
      "wheel",
      (e) => {
        const { px, py } = pos(e);
        const idx = this.subplotAt(px, py);
        if (idx === null) return;
        e.preventDefault();
        const sp = this.subplots[idx];
        const xr = sp.xName ? this.parametricRange(sp) : this.currentXRange();
        const { left, width } = this.area(idx);
        const f = Math.min(0.99, Math.max(0, (px - left) / width));
        const at = xr[0] + f * (xr[1] - xr[0]);
        const k = Math.exp(e.deltaY * 0.0015);
        const r: [number, number] = [at - (at - xr[0]) * k, at + (xr[1] - at) * k];
        if (sp.xName) sp.xZoom = r;
        else this.xRange = r;
        this.draw();
      },
      { passive: false },
    );
  }

  /** Draw into another element (the plot moves between the Modeling and Simulation pages). */
  moveTo(host: HTMLElement) {
    if (host === this.host) return;
    this.resizeObs.unobserve(this.host);
    this.host = host;
    host.appendChild(this.canvas);
    this.resizeObs.observe(host);
    this.draw();
  }

  get hostElement(): HTMLElement {
    return this.host;
  }

  nextColor(): string {
    const i = this.colorIndex++;
    if (i < PALETTE.length) return PALETTE[i];
    // Beyond the palette: hues spread by the golden angle, alternating lightness.
    const hue = (i * 137.508) % 360;
    return `hsl(${hue.toFixed(0)}, 70%, ${i % 2 ? 38 : 48}%)`;
  }

  has(name: string): boolean {
    return this.subplots[this.active]?.series.some((s) => s.name === name) ?? false;
  }

  /** Plotted in any subplot. */
  hasAnywhere(name: string): boolean {
    return this.subplots.some((sp) => sp.series.some((s) => s.name === name));
  }

  toggle(s: Omit<Series, "color">) {
    const sp = this.subplots[this.active];
    const i = sp.series.findIndex((x) => x.name === s.name);
    if (i >= 0) sp.series.splice(i, 1);
    else {
      const du = displayUnit(s.unit);
      sp.series.push({ ...s, color: this.nextColor(), scale: du.scale, offset: du.offset, displayUnit: du.unit });
    }
    this.draw();
    this.onChange();
  }

  /** Add a series to subplot `k` (default: the active one); replaces one of the same name. */
  add(s: Omit<Series, "color"> & { color?: string }, k = this.active) {
    const sp = this.subplots[k];
    const i = sp.series.findIndex((x) => x.name === s.name);
    const du = displayUnit(s.unit);
    const full: Series = { scale: du.scale, offset: du.offset, displayUnit: du.unit, ...s, color: s.color ?? (i >= 0 ? sp.series[i].color : this.nextColor()) };
    if (i >= 0) sp.series[i] = full;
    else sp.series.push(full);
    this.draw();
    this.onChange();
  }

  addSubplot() {
    this.subplots.push({ series: [] });
    this.active = this.subplots.length - 1;
    this.draw();
    this.onChange();
  }

  /** Arrange the subplots in `rows` x `cols` (null: one column), adding empty subplots
   * to fill the matrix. */
  setLayout(layout: { rows: number; cols: number } | null) {
    this.layout = layout && layout.rows * layout.cols > 1 ? layout : null;
    if (this.layout) while (this.subplots.length < this.layout.rows * this.layout.cols) this.subplots.push({ series: [] });
    else this.subplots = this.subplots.filter((sp, i) => sp.series.length || i === 0);
    this.active = Math.min(this.active, this.subplots.length - 1);
    this.draw();
    this.onChange();
  }

  clear() {
    this.subplots = [{ series: [] }];
    if (this.layout) while (this.subplots.length < this.layout.rows * this.layout.cols) this.subplots.push({ series: [] });
    this.active = 0;
    this.xRange = null;
    this.colorIndex = 0;
    this.draw();
    this.onChange();
  }

  /** Replace the data of plotted series after a new simulation (same names); series the
   * new result does not have (another model) are dropped, and a zoom outside the new time
   * range is reset. */
  update(lookup: (name: string, s: Series) => { x: Float64Array; y: Float64Array } | null) {
    let dropped = false;
    this.liveRange = null;
    for (const sp of this.subplots) {
      sp.series = sp.series.filter((s) => {
        if (s.run) return true;
        const d = lookup(s.name, s);
        if (!d) {
          dropped = true;
          return false;
        }
        s.x = d.x;
        s.y = d.y;
        return true;
      });
      // A spectrum keeps its frequency range; other parametric plots show their new data.
      if (sp.xName !== "frequency") sp.xZoom = undefined;
    }
    const keep = this.layout ? this.layout.rows * this.layout.cols : 1;
    if (this.subplots.length > keep) this.subplots = this.subplots.filter((sp, i) => sp.series.length > 0 || i < keep);
    if (this.subplots.length === 0) this.subplots = [{ series: [] }];
    this.active = Math.min(this.active, this.subplots.length - 1);
    if (this.xRange) {
      const saved = this.xRange;
      this.xRange = null;
      const full = this.currentXRange();
      if (saved[0] >= full[0] && saved[1] <= full[1] && !dropped) this.xRange = saved;
    }
    this.draw();
    this.onChange();
  }

  /** Data of a run still streaming in: series found by `lookup` get the rows so far,
   * the others are kept as they are. */
  setLive(lookup: (name: string, s: Series) => { x: Float64Array; y: Float64Array } | null) {
    for (const sp of this.subplots) {
      for (const s of sp.series) {
        if (s.run) continue;
        const d = lookup(s.name, s);
        if (d) {
          s.x = d.x;
          s.y = d.y;
        }
      }
    }
    this.draw();
  }

  /** Grid of the matrix layout: rows and columns used for `n` subplots. */
  private grid(): { rows: number; cols: number } {
    const n = this.subplots.length;
    if (!this.layout) return { rows: n, cols: 1 };
    const cols = this.layout.cols;
    return { rows: Math.max(this.layout.rows, Math.ceil(n / cols)), cols };
  }

  private cell(k: number, W = this.canvas.clientWidth, H = this.canvas.clientHeight): Cell {
    const { rows, cols } = this.grid();
    const r = Math.floor(k / cols);
    const c = k % cols;
    return { x: (c * W) / cols, y: (r * H) / rows, w: W / cols, h: H / rows };
  }

  /** Horizontal extent of the plotting area of subplot `k`. */
  private area(k: number) {
    const c = this.cell(k);
    return { left: c.x + 64, width: Math.max(10, c.w - 64 - 16) };
  }

  private subplotAt(px: number, py: number): number | null {
    for (let k = 0; k < this.subplots.length; k++) {
      const c = this.cell(k);
      if (px >= c.x && px < c.x + c.w && py >= c.y && py < c.y + c.h) return k;
    }
    return null;
  }

  /** Time range shown by the time subplots. */
  visibleRange(): [number, number] {
    return this.currentXRange();
  }

  /** Zoom every time subplot to `[a, b]` (null: everything). */
  setRange(r: [number, number] | null) {
    this.xRange = r;
    this.draw();
  }

  private currentXRange(): [number, number] {
    if (this.xRange) return this.xRange;
    if (this.liveRange) return this.liveRange;
    let lo = Infinity;
    let hi = -Infinity;
    for (const sp of this.subplots) {
      if (sp.xName) continue;
      for (const s of sp.series) {
        if (s.x.length) {
          lo = Math.min(lo, s.x[0]);
          hi = Math.max(hi, s.x[s.x.length - 1]);
        }
      }
    }
    return lo < hi ? [lo, hi] : [0, 1];
  }

  draw() {
    const dpr = window.devicePixelRatio || 1;
    const W = this.host.clientWidth;
    const H = this.host.clientHeight;
    if (W === 0 || H === 0) return;
    this.canvas.width = W * dpr;
    this.canvas.height = H * dpr;
    this.canvas.style.width = `${W}px`;
    this.canvas.style.height = `${H}px`;
    const g = this.canvas.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const css = getComputedStyle(this.host);
    const fg = css.getPropertyValue("--fg").trim() || "#222";
    const muted = css.getPropertyValue("--muted").trim() || "#888";
    const grid = css.getPropertyValue("--grid").trim() || "#e6e6e6";
    const accent = css.getPropertyValue("--accent").trim() || "#0b5fff";
    g.clearRect(0, 0, W, H);
    g.font = "11px system-ui, sans-serif";
    const n = this.subplots.length;
    const timeRange = this.currentXRange();
    // The cursor time, shown in every time subplot (they share the axis).
    let hoverT: number | null = null;
    let hoverSub: number | null = null;
    if (this.hover && this.dragStart === null && !this.pan) {
      hoverSub = this.subplotAt(this.hover.px, this.hover.py);
      if (hoverSub !== null && !this.subplots[hoverSub].xName) {
        const { left, width } = this.area(hoverSub);
        if (this.hover.px >= left && this.hover.px <= left + width) hoverT = timeRange[0] + ((this.hover.px - left) / width) * (timeRange[1] - timeRange[0]);
      }
    }
    for (let k = 0; k < n; k++) {
      const sp = this.subplots[k];
      const cell = this.cell(k, W, H);
      const left = cell.x + 64;
      const width = Math.max(10, cell.w - 64 - 16);
      // Legend entries wrap onto as many rows as the width needs.
      const legend: { s: Series; label: string; x: number; row: number }[] = [];
      {
        let lx = left;
        let row = 0;
        for (const s of sp.series) {
          const label = `${s.name}${s.displayUnit ? ` [${s.displayUnit}]` : ""}`;
          const w = g.measureText(label).width + 34;
          if (lx > left && lx + w - 16 > left + width) {
            lx = left;
            row++;
          }
          legend.push({ s, label, x: lx, row });
          lx += w;
        }
      }
      const rows = legend.length ? legend[legend.length - 1].row + 1 : 1;
      const head = 8 + 14 * rows;
      const top = cell.y + head;
      const height = cell.h - head - 26;
      if (height < 20) continue;
      if (n > 1 && k === this.active) {
        g.fillStyle = accent;
        g.fillRect(cell.x, top - head, 3, height + head + 26);
      }
      const xr = sp.xName ? this.parametricRange(sp) : timeRange;
      // Values in axis space (log10 on a logarithmic axis).
      const ty = sp.logY ? (v: number) => (v > 0 ? Math.log10(v) : NaN) : (v: number) => v;
      // y range of visible part
      let ylo = Infinity;
      let yhi = -Infinity;
      for (const s of sp.series) {
        const sc = s.scale ?? 1;
        const of = s.offset ?? 0;
        for (let i = 0; i < s.y.length; i++) {
          const x = s.x[i];
          if ((!sp.xName || sp.xZoom) && (x < xr[0] || x > xr[1])) continue;
          const v = ty(s.y[i] * sc + of);
          if (v < ylo) ylo = v;
          if (v > yhi) yhi = v;
        }
        if (s.band) {
          for (let i = 0; i < s.band.lo.length; i++) {
            const x = s.x[i];
            if (x < xr[0] || x > xr[1]) continue;
            ylo = Math.min(ylo, s.band.lo[i] * sc + of);
            yhi = Math.max(yhi, s.band.hi[i] * sc + of);
          }
        }
      }
      if (!(ylo <= yhi)) {
        ylo = 0;
        yhi = 1;
      }
      if (ylo === yhi) {
        const d = Math.abs(ylo) * 0.05 || 0.5;
        ylo -= d;
        yhi += d;
      }
      const pad = (yhi - ylo) * 0.05;
      ylo -= pad;
      yhi += pad;
      const X = (x: number) => left + ((x - xr[0]) / (xr[1] - xr[0])) * width;
      const Y = (y: number) => top + height - ((y - ylo) / (yhi - ylo)) * height;
      // grid + ticks
      g.strokeStyle = grid;
      g.fillStyle = muted;
      g.lineWidth = 1;
      g.textAlign = "right";
      g.textBaseline = "middle";
      const yTicks = sp.logY
        ? niceTicks(ylo, yhi, Math.max(2, Math.floor(height / 40))).filter((t) => Number.isInteger(t))
        : niceTicks(ylo, yhi, Math.max(2, Math.floor(height / 40)));
      if (sp.logY && yTicks.length === 0) for (let d = Math.ceil(ylo); d <= yhi; d++) yTicks.push(d);
      for (const t of yTicks) {
        const y = Math.round(Y(t)) + 0.5;
        g.beginPath();
        g.moveTo(left, y);
        g.lineTo(left + width, y);
        g.stroke();
        g.fillText(sp.logY ? `1e${t}` : fmtTick(t), left - 6, y);
      }
      g.textAlign = "center";
      g.textBaseline = "top";
      for (const t of niceTicks(xr[0], xr[1], Math.max(2, Math.floor(width / 90)))) {
        const x = Math.round(X(t)) + 0.5;
        g.beginPath();
        g.moveTo(x, top);
        g.lineTo(x, top + height);
        g.stroke();
        g.fillText(fmtTick(t), x, top + height + 4);
      }
      g.strokeStyle = muted;
      g.strokeRect(left + 0.5, top + 0.5, width, height);
      g.textAlign = "right";
      g.fillText(sp.xName ? `${sp.xName}${sp.xUnit ? ` [${sp.xUnit}]` : ""}` : "time [s]", left + width, top + height + 14);
      // series
      g.save();
      g.beginPath();
      g.rect(left, top, width, height);
      g.clip();
      for (const s of sp.series) {
        if (s.band) this.drawBand(g, s, X, Y);
        g.strokeStyle = s.color;
        g.fillStyle = s.color;
        g.lineWidth = 1.5;
        const Yt = sp.logY ? (y: number) => Y(ty(y)) : Y;
        if (s.points) {
          this.drawPoints(g, s, X, Yt);
          continue;
        }
        if (s.dash) g.setLineDash(s.dash);
        this.drawSeries(g, s, xr, X, Yt, !!sp.xName, width);
        g.setLineDash([]);
      }
      if (!sp.xName) {
        for (const m of this.markers) {
          if (m.t < xr[0] || m.t > xr[1]) continue;
          g.strokeStyle = m.color;
          g.lineWidth = 1.5;
          g.beginPath();
          g.moveTo(Math.round(X(m.t)) + 0.5, top);
          g.lineTo(Math.round(X(m.t)) + 0.5, top + height);
          g.stroke();
        }
      }
      g.restore();
      // legend
      g.textAlign = "left";
      g.textBaseline = "alphabetic";
      for (const { s, label, x, row } of legend) {
        const ly = top - head + 14 + 14 * row;
        g.fillStyle = s.color;
        g.fillRect(x, ly - 5, 14, 3);
        g.fillStyle = fg;
        g.fillText(label, x + 18, ly);
      }
      // crosshair readout: the cursor time in every time subplot
      if (hoverT !== null && !sp.xName && hoverT >= xr[0] && hoverT <= xr[1]) {
        const t = hoverT;
        const hx = X(t);
        g.strokeStyle = muted;
        g.setLineDash([3, 3]);
        g.beginPath();
        g.moveTo(Math.round(hx) + 0.5, top);
        g.lineTo(Math.round(hx) + 0.5, top + height);
        g.stroke();
        g.setLineDash([]);
        let ry = top + 14;
        g.textAlign = "left";
        const lines = sp.series.map((s) => {
          const v = interp(s.x, s.y, t) * (s.scale ?? 1) + (s.offset ?? 0);
          return { s, v, text: `${s.name} = ${fmtTick(v)}${s.displayUnit ? ` ${s.displayUnit}` : ""}` };
        });
        const boxW = Math.max(60, ...lines.map((l) => g.measureText(l.text).width)) + 8;
        const boxX = hx + 8 + boxW > left + width ? hx - 8 - boxW : hx + 8;
        for (const { s, v, text } of lines) {
          g.fillStyle = s.color;
          g.beginPath();
          g.arc(hx, Y(ty(v)), 3, 0, 2 * Math.PI);
          g.fill();
          g.fillText(text, boxX, ry);
          ry += 14;
        }
        g.fillStyle = muted;
        g.fillText(`t = ${fmtTick(t)}`, boxX, ry);
      }
    }
    if (this.dragStart !== null && this.dragNow !== null) {
      const c = this.cell(this.dragSub, W, H);
      g.fillStyle = accent;
      g.globalAlpha = 0.14;
      g.fillRect(Math.min(this.dragStart, this.dragNow), c.y, Math.abs(this.dragNow - this.dragStart), c.h);
      g.globalAlpha = 1;
    }
    if (this.subplots.every((s) => s.series.length === 0)) {
      g.fillStyle = muted;
      g.textAlign = "center";
      g.font = "13px system-ui, sans-serif";
      g.fillText(this.emptyText, W / 2, H / 2);
    }
    this.onView();
  }

  private parametricRange(sp: Subplot): [number, number] {
    if (sp.xZoom) return sp.xZoom;
    let lo = Infinity;
    let hi = -Infinity;
    for (const s of sp.series) for (const v of s.x) {
      if (v < lo) lo = v;
      if (v > hi) hi = v;
    }
    if (!(lo < hi)) return [lo - 1, hi + 1];
    const p = (hi - lo) * 0.03;
    return [lo - p, hi + p];
  }

  private drawBand(g: CanvasRenderingContext2D, s: Series, X: (x: number) => number, Y: (y: number) => number) {
    const b = s.band!;
    const sc = s.scale ?? 1;
    const of = s.offset ?? 0;
    const n = Math.min(s.x.length, b.lo.length, b.hi.length);
    if (n === 0) return;
    g.beginPath();
    for (let i = 0; i < n; i++) {
      const px = X(s.x[i]);
      const py = Y(b.hi[i] * sc + of);
      if (i === 0) g.moveTo(px, py);
      else g.lineTo(px, py);
    }
    for (let i = n - 1; i >= 0; i--) g.lineTo(X(s.x[i]), Y(b.lo[i] * sc + of));
    g.closePath();
    g.globalAlpha = 0.18;
    g.fillStyle = s.color;
    g.fill();
    g.globalAlpha = 1;
  }

  private drawPoints(g: CanvasRenderingContext2D, s: Series, X: (x: number) => number, Y: (y: number) => number) {
    const sc = s.scale ?? 1;
    const of = s.offset ?? 0;
    g.fillStyle = s.color;
    for (let i = 0; i < s.x.length; i++) {
      g.beginPath();
      g.arc(X(s.x[i]), Y(s.y[i] * sc + of), 2.5, 0, 2 * Math.PI);
      g.fill();
    }
  }

  private drawSeries(g: CanvasRenderingContext2D, s: Series, xr: [number, number], X: (x: number) => number, Y: (y: number) => number, parametric: boolean, width: number) {
    const n = Math.min(s.x.length, s.y.length);
    if (n === 0) return;
    const sc = s.scale ?? 1;
    const of = s.offset ?? 0;
    g.beginPath();
    if (parametric || n < 4 * width) {
      let started = false;
      for (let i = 0; i < n; i++) {
        if (!parametric && (i + 1 < n && s.x[i + 1] < xr[0])) continue;
        const px = X(s.x[i]);
        const py = Y(s.y[i] * sc + of);
        if (!Number.isFinite(py)) {
          started = false;
          continue;
        }
        if (!started) {
          g.moveTo(px, py);
          started = true;
        } else g.lineTo(px, py);
        if (!parametric && s.x[i] > xr[1]) break;
      }
    } else {
      // min/max per pixel column
      let col = -1;
      let mn = 0;
      let mx = 0;
      let first = 0;
      let last = 0;
      let started = false;
      const flush = () => {
        if (col < 0) return;
        if (!started) {
          g.moveTo(col, Y(first));
          started = true;
        } else g.lineTo(col, Y(first));
        g.lineTo(col, Y(mn));
        g.lineTo(col, Y(mx));
        g.lineTo(col, Y(last));
      };
      for (let i = 0; i < n; i++) {
        const x = s.x[i];
        if (x < xr[0] && !(i + 1 < n && s.x[i + 1] >= xr[0])) continue;
        const v = s.y[i] * sc + of;
        const c = Math.round(X(x));
        if (c !== col) {
          flush();
          col = c;
          mn = mx = first = last = v;
        } else {
          if (v < mn) mn = v;
          if (v > mx) mx = v;
          last = v;
        }
        if (x > xr[1]) break;
      }
      flush();
    }
    g.stroke();
  }
}

export function interp(x: Float64Array, y: Float64Array, t: number): number {
  const n = x.length;
  if (n === 0) return NaN;
  if (t <= x[0]) return y[0];
  if (t >= x[n - 1]) return y[n - 1];
  let lo = 0;
  let hi = n - 1;
  while (hi - lo > 1) {
    const mid = (lo + hi) >> 1;
    if (x[mid] <= t) lo = mid;
    else hi = mid;
  }
  const d = x[hi] - x[lo];
  return d > 0 ? y[lo] + ((y[hi] - y[lo]) * (t - x[lo])) / d : y[hi];
}

/** One row of a bar chart: a label and one value per legend entry. */
export interface BarRow {
  label: string;
  values: number[];
  /** Optional half-widths of error bars, per value. */
  errors?: (number | null)[];
}

/** Horizontal grouped bar chart (rankings, Sobol indices, Morris effects). */
export class BarChart {
  readonly canvas: HTMLCanvasElement;
  title = "";
  legend: string[] = [];
  rows: BarRow[] = [];

  constructor(private host: HTMLElement) {
    this.canvas = document.createElement("canvas");
    this.canvas.className = "plot";
    host.appendChild(this.canvas);
    new ResizeObserver(() => this.draw()).observe(host);
  }

  set(title: string, legend: string[], rows: BarRow[]) {
    this.title = title;
    this.legend = legend;
    this.rows = rows;
    this.draw();
  }

  draw() {
    const dpr = window.devicePixelRatio || 1;
    const W = this.host.clientWidth;
    const H = this.host.clientHeight;
    if (W === 0 || H === 0) return;
    this.canvas.width = W * dpr;
    this.canvas.height = H * dpr;
    this.canvas.style.width = `${W}px`;
    this.canvas.style.height = `${H}px`;
    const g = this.canvas.getContext("2d")!;
    g.setTransform(dpr, 0, 0, dpr, 0, 0);
    const css = getComputedStyle(this.host);
    const fg = css.getPropertyValue("--fg").trim() || "#222";
    const muted = css.getPropertyValue("--muted").trim() || "#888";
    const grid = css.getPropertyValue("--grid").trim() || "#e6e6e6";
    g.clearRect(0, 0, W, H);
    g.font = "11px system-ui, sans-serif";
    if (!this.rows.length) {
      g.fillStyle = muted;
      g.textAlign = "center";
      g.fillText(this.title || "No data", W / 2, H / 2);
      return;
    }
    const labelW = Math.min(W * 0.4, Math.max(...this.rows.map((r) => g.measureText(r.label).width)) + 12);
    const left = labelW + 4;
    const right = W - 16;
    const top = 40;
    const bottom = H - 24;
    let lo = 0;
    let hi = 0;
    for (const r of this.rows)
      r.values.forEach((v, i) => {
        if (!isFinite(v)) return;
        const e = r.errors?.[i] ?? 0;
        lo = Math.min(lo, v - (e ?? 0));
        hi = Math.max(hi, v + (e ?? 0));
      });
    if (!(hi > lo)) hi = lo + 1;
    const X = (v: number) => left + ((v - lo) / (hi - lo)) * (right - left);
    // title and legend
    g.fillStyle = fg;
    g.textAlign = "left";
    g.textBaseline = "alphabetic";
    g.font = "600 12px system-ui, sans-serif";
    g.fillText(this.title, 8, 16);
    g.font = "11px system-ui, sans-serif";
    let lx = 8;
    this.legend.forEach((name, i) => {
      g.fillStyle = PALETTE[i % PALETTE.length];
      g.fillRect(lx, 25, 12, 8);
      g.fillStyle = fg;
      g.fillText(name, lx + 16, 33);
      lx += g.measureText(name).width + 34;
    });
    // grid
    g.strokeStyle = grid;
    g.fillStyle = muted;
    g.textAlign = "center";
    g.textBaseline = "top";
    for (const t of niceTicks(lo, hi, Math.max(2, Math.floor((right - left) / 80)))) {
      const x = Math.round(X(t)) + 0.5;
      g.beginPath();
      g.moveTo(x, top);
      g.lineTo(x, bottom);
      g.stroke();
      g.fillText(fmtTick(t), x, bottom + 4);
    }
    const rowH = (bottom - top) / this.rows.length;
    const k = Math.max(1, this.legend.length);
    const barH = Math.min(18, (rowH * 0.8) / k);
    this.rows.forEach((r, ri) => {
      const y0 = top + ri * rowH + (rowH - barH * k) / 2;
      g.fillStyle = fg;
      g.textAlign = "right";
      g.textBaseline = "middle";
      g.fillText(r.label, labelW, top + ri * rowH + rowH / 2);
      r.values.forEach((v, i) => {
        if (!isFinite(v)) return;
        const y = y0 + i * barH;
        const a = X(Math.min(0, v));
        const b = X(Math.max(0, v));
        g.fillStyle = PALETTE[i % PALETTE.length];
        g.fillRect(a, y + 1, Math.max(1, b - a), barH - 2);
        const e = r.errors?.[i];
        if (e && isFinite(e)) {
          g.strokeStyle = fg;
          g.beginPath();
          g.moveTo(X(v - e), y + barH / 2);
          g.lineTo(X(v + e), y + barH / 2);
          g.moveTo(X(v - e), y + 3);
          g.lineTo(X(v - e), y + barH - 3);
          g.moveTo(X(v + e), y + 3);
          g.lineTo(X(v + e), y + barH - 3);
          g.stroke();
        }
      });
    });
    g.strokeStyle = muted;
    g.beginPath();
    g.moveTo(Math.round(X(0)) + 0.5, top);
    g.lineTo(Math.round(X(0)) + 0.5, bottom);
    g.stroke();
  }
}
