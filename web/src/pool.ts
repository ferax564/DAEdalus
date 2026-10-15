// A pool of simulation workers for experiments (sweeps, Monte Carlo): N Web Workers, each
// running worker.ts with its own WebAssembly workspace. Every pool worker loads the same
// sources as the main worker (rpc.ts records them); the library bundle download is cached
// by the browser, so only its unpacking is repeated. A worker keeps the last translation,
// so runs of one model that differ only in free parameters translate once per worker.
import { sources } from "./rpc";
import type { SimSettings } from "./types";

/** Summary of one run (see `Workspace::simulateRun` in crates/daedalus-wasm). */
export interface RunSummary {
  ok: boolean;
  error: string | null;
  model: string;
  points?: number;
  log?: string[];
  warnings?: string[];
  translate_ms?: number;
  load_ms?: number;
  /** The translation of an earlier run was reused. */
  cached?: boolean;
  /** Overridden parameters that needed a new translation (not free Real parameters). */
  retranslated?: string[];
  stats?: { solver: { steps: number }; events: number; states: number; sim_ms: number } | null;
}

export interface RunOutput {
  summary: RunSummary;
  time: Float64Array;
  /** The requested variables (missing ones are absent). */
  series: Map<string, Float64Array>;
  /** Index of the pool worker that ran it. */
  worker: number;
  /** Wall time in the pool, including waiting for sources. */
  ms: number;
}

export class CancelledError extends Error {
  constructor() {
    super("cancelled");
  }
}

interface Job {
  model: string;
  options: SimSettings;
  vars: string[];
  resolve: (r: RunOutput) => void;
  reject: (e: Error) => void;
}

class PoolWorker {
  private w: Worker;
  private nextId = 1;
  private pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
  /** Sources sent so far (rpc.ts `sources.seq`). */
  private seq = 0;
  private bundle: Promise<unknown> | null = null;
  private bundleSeq = 0;
  busy = false;

  constructor(readonly index: number) {
    this.w = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
    this.w.onmessage = (ev) => {
      if (ev.data.progress) return;
      const { id, ok, result, error } = ev.data;
      const p = this.pending.get(id);
      if (!p) return;
      this.pending.delete(id);
      if (ok) p.resolve(result);
      else p.reject(new Error(error));
    };
  }

  private call<T>(op: string, ...args: unknown[]): Promise<T> {
    const id = this.nextId++;
    return new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
      this.w.postMessage({ id, op, args });
    });
  }

  /** Send the documents changed since the last sync, and the bundle once. */
  private async sync() {
    if (this.seq < sources.seq) {
      const set: [string, string][] = [];
      const removed: string[] = [];
      for (const [uri, d] of sources.docs) {
        if (d.seq <= this.seq) continue;
        if (d.text === null) removed.push(uri);
        else set.push([uri, d.text]);
      }
      const b = sources.bundle;
      if (b && b.seq > this.bundleSeq) {
        this.bundleSeq = b.seq;
        this.bundle = this.call("loadBundle", b.base, b.prefix).catch(() => null);
      }
      this.seq = sources.seq;
      for (const uri of removed) await this.call("removeDocument", uri);
      if (set.length) await this.call("setDocuments", set);
    }
    // Any model may extend or instantiate library classes, so the bundle must be in first.
    if (this.bundle) await this.bundle;
  }

  async run(job: Job): Promise<RunOutput> {
    const t0 = performance.now();
    await this.sync();
    const r = await this.call<{ summary: RunSummary; time: Float64Array; cols: Float64Array[] }>("run", job.model, job.options, job.vars);
    const series = new Map<string, Float64Array>();
    job.vars.forEach((n, i) => {
      if (r.cols[i]?.length) series.set(n, r.cols[i]);
    });
    return { summary: r.summary, time: r.time, series, worker: this.index, ms: performance.now() - t0 };
  }

  terminate() {
    this.w.terminate();
    for (const p of this.pending.values()) p.reject(new CancelledError());
    this.pending.clear();
  }
}

export class WorkerPool {
  readonly size: number;
  private workers: (PoolWorker | null)[];
  private queue: Job[] = [];
  /** Runs in flight now, and the most at once since the last reset (for checks). */
  running = 0;
  peak = 0;

  constructor(size?: number) {
    const cores = navigator.hardwareConcurrency || 2;
    this.size = Math.max(1, Math.min(size ?? cores - 1, 8));
    this.workers = new Array(this.size).fill(null);
  }

  /**
   * Simulate `model` with `options` (as for Simulate, including `params` overrides) on the
   * next free worker. Resolves with the run summary and the trajectories of `vars`.
   */
  run(model: string, options: SimSettings, vars: string[] = []): Promise<RunOutput> {
    return new Promise((resolve, reject) => {
      this.queue.push({ model, options, vars, resolve, reject });
      this.pump();
    });
  }

  /** Drop queued runs and stop the running ones (their workers are restarted on demand). */
  cancel() {
    const q = this.queue;
    this.queue = [];
    for (const j of q) j.reject(new CancelledError());
    this.workers.forEach((w, i) => {
      if (w?.busy) {
        w.terminate();
        this.workers[i] = null;
      }
    });
    this.running = 0;
  }

  /** Workers started so far. */
  get started(): number {
    return this.workers.filter((w) => w).length;
  }

  private pump() {
    for (let i = 0; i < this.size && this.queue.length; i++) {
      let w = this.workers[i];
      if (w?.busy) continue;
      if (!w) w = this.workers[i] = new PoolWorker(i);
      const job = this.queue.shift()!;
      const worker = w;
      worker.busy = true;
      this.running++;
      this.peak = Math.max(this.peak, this.running);
      worker
        .run(job)
        .then(job.resolve, job.reject)
        .finally(() => {
          if (this.workers[worker.index] === worker) {
            worker.busy = false;
            this.running = Math.max(0, this.running - 1);
          }
          this.pump();
        });
    }
  }
}

/** The shared pool, created on first use. */
let shared: WorkerPool | null = null;
export function pool(): WorkerPool {
  return (shared ??= new WorkerPool());
}
