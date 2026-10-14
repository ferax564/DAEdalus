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
}

export interface Subplot {
  series: Series[];
  /** Parametric plot: the name of the x variable, if not time. */
  xName?: string;
  xUnit?: string;
}

const PALETTE = ["#0b5fff", "#d9480f", "#2b8a3e", "#ae3ec9", "#e8590c", "#1098ad", "#c2255c", "#5c940d", "#364fc7", "#862e9c"];

/** Dymola-style display units for common SI units. */
export function displayUnit(unit: string): { unit: string; scale: number; offset: number } {
  switch (unit) {
    case "rad/s":
      return { unit: "rpm", scale: 60 / (2 * Math.PI), offset: 0 };
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

export class PlotWindow {
  readonly canvas: HTMLCanvasElement;
  subplots: Subplot[] = [{ series: [] }];
  active = 0;
  private xRange: [number, number] | null = null;
  private hover: { px: number; py: number } | null = null;
  private dragStart: number | null = null;
  private dragNow: number | null = null;
  private colorIndex = 0;
  onChange: () => void = () => {};

  constructor(private host: HTMLElement) {
    this.canvas = document.createElement("canvas");
    this.canvas.className = "plot";
    host.appendChild(this.canvas);
    new ResizeObserver(() => this.draw()).observe(host);
    this.canvas.addEventListener("pointermove", (e) => {
      const r = this.canvas.getBoundingClientRect();
      this.hover = { px: e.clientX - r.left, py: e.clientY - r.top };
      if (this.dragStart !== null) this.dragNow = this.hover.px;
      this.draw();
    });
    this.canvas.addEventListener("pointerleave", () => {
      this.hover = null;
      this.draw();
    });
    this.canvas.addEventListener("pointerdown", (e) => {
      const r = this.canvas.getBoundingClientRect();
      this.dragStart = e.clientX - r.left;
      this.dragNow = this.dragStart;
      const py = e.clientY - r.top;
      const idx = this.subplotAt(py);
      if (idx !== null) {
        this.active = idx;
        this.onChange();
      }
      this.canvas.setPointerCapture(e.pointerId);
    });
    this.canvas.addEventListener("pointerup", () => {
      if (this.dragStart !== null && this.dragNow !== null && Math.abs(this.dragNow - this.dragStart) > 6) {
        const [a, b] = [this.dragStart, this.dragNow].sort((p, q) => p - q);
        const xr = this.currentXRange();
        const { left, width } = this.plotArea();
        const t = (px: number) => xr[0] + ((px - left) / width) * (xr[1] - xr[0]);
        this.xRange = [t(a), t(b)];
      }
      this.dragStart = this.dragNow = null;
      this.draw();
    });
    this.canvas.addEventListener("dblclick", () => {
      this.xRange = null;
      this.draw();
    });
  }

  nextColor(): string {
    return PALETTE[this.colorIndex++ % PALETTE.length];
  }

  has(name: string): boolean {
    return this.subplots[this.active]?.series.some((s) => s.name === name) ?? false;
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

  addSubplot() {
    this.subplots.push({ series: [] });
    this.active = this.subplots.length - 1;
    this.draw();
    this.onChange();
  }

  clear() {
    this.subplots = [{ series: [] }];
    this.active = 0;
    this.xRange = null;
    this.colorIndex = 0;
    this.draw();
    this.onChange();
  }

  /** Replace the data of plotted series after a new simulation (same names). */
  update(lookup: (name: string) => { x: Float64Array; y: Float64Array } | null) {
    for (const sp of this.subplots) {
      for (const s of sp.series) {
        const d = lookup(s.name);
        if (d) {
          s.x = d.x;
          s.y = d.y;
        }
      }
    }
    this.draw();
  }

  private plotArea() {
    const w = this.canvas.clientWidth;
    return { left: 64, width: Math.max(10, w - 64 - 16) };
  }

  private subplotAt(py: number): number | null {
    const h = this.canvas.clientHeight;
    const n = this.subplots.length;
    const i = Math.floor((py / h) * n);
    return i >= 0 && i < n ? i : null;
  }

  private currentXRange(): [number, number] {
    if (this.xRange) return this.xRange;
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
    const { left, width } = this.plotArea();
    const timeRange = this.currentXRange();
    for (let k = 0; k < n; k++) {
      const sp = this.subplots[k];
      const top = (k * H) / n + 22;
      const height = H / n - 22 - 26;
      if (height < 20) continue;
      if (n > 1 && k === this.active) {
        g.fillStyle = accent;
        g.fillRect(0, top - 22, 3, height + 48);
      }
      const xr = sp.xName ? this.parametricRange(sp) : timeRange;
      // y range of visible part
      let ylo = Infinity;
      let yhi = -Infinity;
      for (const s of sp.series) {
        const sc = s.scale ?? 1;
        const of = s.offset ?? 0;
        for (let i = 0; i < s.y.length; i++) {
          const x = s.x[i];
          if (!sp.xName && (x < xr[0] || x > xr[1])) continue;
          const v = s.y[i] * sc + of;
          if (v < ylo) ylo = v;
          if (v > yhi) yhi = v;
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
      for (const t of niceTicks(ylo, yhi, Math.max(2, Math.floor(height / 40)))) {
        const y = Math.round(Y(t)) + 0.5;
        g.beginPath();
        g.moveTo(left, y);
        g.lineTo(left + width, y);
        g.stroke();
        g.fillText(fmtTick(t), left - 6, y);
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
        g.strokeStyle = s.color;
        g.lineWidth = 1.5;
        this.drawSeries(g, s, xr, X, Y, !!sp.xName);
      }
      g.restore();
      // legend
      g.textAlign = "left";
      g.textBaseline = "alphabetic";
      let lx = left;
      for (const s of sp.series) {
        const label = `${s.name}${s.displayUnit ? ` [${s.displayUnit}]` : ""}`;
        g.fillStyle = s.color;
        g.fillRect(lx, top - 12, 14, 3);
        g.fillStyle = fg;
        g.fillText(label, lx + 18, top - 7);
        lx += g.measureText(label).width + 34;
      }
      // crosshair readout
      if (this.hover && !sp.xName && this.hover.px >= left && this.hover.px <= left + width) {
        const t = xr[0] + ((this.hover.px - left) / width) * (xr[1] - xr[0]);
        g.strokeStyle = muted;
        g.setLineDash([3, 3]);
        g.beginPath();
        g.moveTo(Math.round(this.hover.px) + 0.5, top);
        g.lineTo(Math.round(this.hover.px) + 0.5, top + height);
        g.stroke();
        g.setLineDash([]);
        let ry = top + 14;
        g.textAlign = "left";
        const boxX = this.hover.px + 8 > left + width - 160 ? this.hover.px - 168 : this.hover.px + 8;
        for (const s of sp.series) {
          const v = interp(s.x, s.y, t) * (s.scale ?? 1) + (s.offset ?? 0);
          g.fillStyle = s.color;
          g.beginPath();
          g.arc(this.hover.px, Y(v), 3, 0, 2 * Math.PI);
          g.fill();
          g.fillText(`${fmtTick(v)}`, boxX, ry);
          ry += 14;
        }
        g.fillStyle = muted;
        g.fillText(`t = ${fmtTick(t)}`, boxX, ry);
      }
    }
    if (this.dragStart !== null && this.dragNow !== null) {
      g.fillStyle = "rgba(11,95,255,0.12)";
      g.fillRect(Math.min(this.dragStart, this.dragNow), 0, Math.abs(this.dragNow - this.dragStart), H);
    }
    if (this.subplots.every((s) => s.series.length === 0)) {
      g.fillStyle = muted;
      g.textAlign = "center";
      g.font = "13px system-ui, sans-serif";
      g.fillText("Tick variables in the variable browser to plot them", W / 2, H / 2);
    }
  }

  private parametricRange(sp: Subplot): [number, number] {
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

  private drawSeries(g: CanvasRenderingContext2D, s: Series, xr: [number, number], X: (x: number) => number, Y: (y: number) => number, parametric: boolean) {
    const n = s.x.length;
    if (n === 0) return;
    const sc = s.scale ?? 1;
    const of = s.offset ?? 0;
    g.beginPath();
    if (parametric || n < 4 * this.canvas.clientWidth) {
      let started = false;
      for (let i = 0; i < n; i++) {
        if (!parametric && (i + 1 < n && s.x[i + 1] < xr[0])) continue;
        const px = X(s.x[i]);
        const py = Y(s.y[i] * sc + of);
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
