// Web Worker owning the WebAssembly workspace, so translation and simulation never block the UI.
import init, { Workspace, doePlan, doeAnalyze } from "./wasm/daedalus_wasm.js";
import type { BundleInfo, BundleProgress } from "./types";

let ws: Workspace | null = null;
const ready = init().then(() => {
  ws = new Workspace();
});

type Req = { id: number; op: string; args: unknown[] };
/** Text of each document as the editor intelligence last saw it. */
const ideText = new Map<string, string>();

/** Set by an "analysisCancel" request; the running analysis checks it between steps. */
let analysisCancelled = false;

/**
 * The run being advanced in slices (live plotting, dashboards). Requests that arrive while
 * it runs are served between its slices: "liveVars" changes the variables streamed,
 * "liveParams" changes parameters at the time reached, "liveControl" pauses or stops it.
 */
interface Live {
  id: number;
  /** Variables whose new rows are posted after each slice. */
  names: string[];
  /** Rows posted so far, per variable (a variable added later starts from row 0). */
  sent: Map<string, number>;
  /** Rows of the time axis posted so far. */
  sentTime: number;
  /** Simulated seconds per wall-clock second; 0: as fast as possible. */
  pace: number;
  paused: boolean;
  stop: boolean;
  /** Pacing anchor: simulated time at wall-clock time. */
  anchor: { t: number; wall: number };
  /** Simulated time reached. */
  t: number;
}
let live: Live | null = null;

const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Post the rows recorded since the last post. */
function postLive(w: Workspace, run: Live, t: number, stopTime: number) {
  const transfer: Transferable[] = [];
  const cols: Record<string, { from: number; v: Float64Array }> = {};
  for (const n of run.names) {
    const from = run.sent.get(n) ?? 0;
    const v = w.liveValues(n, from);
    if (from === 0 && v.length === 0) continue; // not a variable of this model
    run.sent.set(n, from + v.length);
    cols[n] = { from, v };
    transfer.push(v.buffer);
  }
  const fromT = run.sentTime;
  const newTime = w.liveTime(fromT);
  run.sentTime = fromT + newTime.length;
  transfer.push(newTime.buffer);
  (self as unknown as Worker).postMessage({ live: { id: run.id, t, stop: stopTime, from: fromT, time: newTime, cols, paused: run.paused } }, transfer);
}

/** Run a live simulation to its end (or until stopped); resolves to its summary. */
async function runLive(w: Workspace, id: number, model: string, settings: unknown, o: { names?: string[]; pace?: number }) {
  const st = JSON.parse(w.liveStart(model, JSON.stringify(settings ?? {})));
  if (!st.ok) return { ok: false, error: st.error, model, vars: [], points: 0, log: [], warnings: [], translate_ms: 0, stats: null, model_stats: null, experiment: null };
  const run: Live = {
    id,
    names: o.names ?? [],
    sent: new Map(),
    sentTime: 0,
    pace: o.pace ?? 0,
    paused: false,
    stop: false,
    anchor: { t: st.start_time, wall: performance.now() },
    t: st.start_time,
  };
  live = run;
  let t = st.start_time as number;
  postLive(w, run, t, st.stop_time);
  for (;;) {
    if (live !== run) return { ok: false, error: "superseded by another run", model, vars: [], points: 0, log: [], warnings: [], translate_ms: 0, stats: null, model_stats: null, experiment: null };
    if (run.stop) {
      live = null;
      const s = JSON.parse(w.liveFinish());
      s.log = [...(s.log ?? []), `stopped at t = ${+t.toPrecision(6)}`];
      s.stopped = true;
      return s;
    }
    const slice0 = performance.now();
    if (run.paused) {
      await sleep(40);
      run.anchor = { t, wall: performance.now() };
      continue;
    }
    let target = Infinity;
    if (run.pace > 0) {
      target = run.anchor.t + (run.pace * (slice0 - run.anchor.wall)) / 1000;
      // A model slower than the pace runs as fast as it can, without building up a debt.
      if (target - t > run.pace * 0.25) {
        run.anchor = { t, wall: slice0 };
        target = t + run.pace * 0.05;
      }
    }
    const r = JSON.parse(w.liveAdvance(target, run.pace > 0 ? 25 : 120));
    if (r.error) {
      live = null;
      return { ok: false, error: r.error, model, vars: [], points: 0, log: [], warnings: [], translate_ms: 0, stats: null, model_stats: null, experiment: null };
    }
    t = r.t;
    run.t = t;
    // Post the rows of this slice (the last one too) before the run is finished.
    postLive(w, run, t, st.stop_time);
    if (r.done) {
      live = null;
      return JSON.parse(w.liveFinish());
    }
    if (run.pace > 0) await sleep(Math.max(1, 33 - (performance.now() - slice0)));
    else await sleep(0);
  }
}

function progress(p: BundleProgress) {
  (self as unknown as Worker).postMessage({ progress: p });
}

async function gunzip(bytes: Uint8Array): Promise<Uint8Array> {
  // A server may already have removed the gzip layer (Content-Encoding).
  if (bytes[0] !== 0x1f || bytes[1] !== 0x8b) return bytes;
  const stream = new Blob([bytes]).stream().pipeThrough(new DecompressionStream("gzip"));
  return new Uint8Array(await new Response(stream).arrayBuffer());
}

/**
 * Fetch a library bundle made by scripts/fetch-msl.mjs (`<base>manifest.json` names the
 * content-hashed bundle) and register its files without parsing them. Registration runs
 * in slices so other requests (the Daedalus library, simulations) keep being served.
 * Resolves to null when the site has no bundle.
 */
async function loadBundle(base: string, prefix: string): Promise<BundleInfo | null> {
  const t0 = performance.now();
  const mres = await fetch(`${base}manifest.json`, { cache: "no-cache" });
  if (!mres.ok || !(mres.headers.get("content-type") ?? "").includes("json")) return null;
  const man = (await mres.json()) as { name: string; version: string; file: string; bytes: number };
  const res = await fetch(`${base}${man.file}`);
  if (!res.ok || !res.body) throw new Error(`${man.file}: HTTP ${res.status}`);
  const total = man.bytes;
  const chunks: Uint8Array[] = [];
  let got = 0;
  let last = 0;
  const reader = res.body.getReader();
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    chunks.push(value);
    got += value.length;
    if (performance.now() - last > 100) {
      last = performance.now();
      progress({ phase: "download", done: Math.min(got, total), total });
    }
  }
  const bytes = new Uint8Array(got);
  let off = 0;
  for (const c of chunks) {
    bytes.set(c, off);
    off += c.length;
  }
  const t1 = performance.now();
  progress({ phase: "unpack", done: 0, total: 1 });
  const data = JSON.parse(new TextDecoder().decode(await gunzip(bytes))) as { files: [string, string][] };
  const t2 = performance.now();
  await ready;
  const w = ws!;
  let slice = performance.now();
  for (let i = 0; i < data.files.length; i++) {
    const [rel, text] = data.files[i];
    w.addLibraryFile(prefix, rel, text);
    if (performance.now() - slice > 30) {
      progress({ phase: "register", done: i + 1, total: data.files.length });
      await new Promise((r) => setTimeout(r, 0));
      slice = performance.now();
    }
  }
  return {
    name: man.name,
    version: man.version,
    files: data.files.length,
    bytes: got,
    download_ms: t1 - t0,
    unpack_ms: t2 - t1,
    register_ms: performance.now() - t2,
  };
}

self.onmessage = async (ev: MessageEvent<Req>) => {
  const { id, op, args } = ev.data;
  try {
    await ready;
    const w = ws!;
    let result: unknown;
    const transfer: Transferable[] = [];
    switch (op) {
      case "setDocuments": {
        const docs = args[0] as [string, string][];
        const errors: Record<string, string[]> = {};
        for (const [uri, text] of docs) {
          ideText.delete(uri);
          const e = JSON.parse(w.setDocument(uri, text)) as string[];
          if (e.length) errors[uri] = e;
        }
        result = errors;
        break;
      }
      case "loadBundle":
        result = await loadBundle(args[0] as string, args[1] as string);
        break;
      case "libraryStats":
        result = JSON.parse(w.libraryStats());
        break;
      case "removeDocument":
        ideText.delete(args[0] as string);
        w.removeDocument(args[0] as string);
        break;
      case "classTree":
        result = JSON.parse(w.classTree());
        break;
      case "classInfo":
        result = JSON.parse(w.classInfo(args[0] as string));
        break;
      case "documentText":
        result = w.documentText(args[0] as string) ?? null;
        break;
      case "ideUpdate": {
        const [uri, text] = args as [string, string];
        ideText.set(uri, text);
        result = JSON.parse(w.ideUpdate(uri, text));
        break;
      }
      case "ide": {
        // [request, uri, text, offset, extra]: bring the document up to date, then ask
        const [req, uri, text, offset, extra] = args as [string, string, string, number, string?];
        if (ideText.get(uri) !== text) {
          w.ideUpdate(uri, text);
          ideText.set(uri, text);
        }
        const r =
          req === "definition" ? w.ideDefinition(uri, offset)
          : req === "references" ? w.ideReferences(uri, offset)
          : req === "rename" ? w.ideRename(uri, offset, extra ?? "")
          : req === "completion" ? w.ideCompletion(uri, offset)
          : req === "hover" ? w.ideHover(uri, offset)
          : req === "symbols" ? w.ideSymbols(uri)
          : "null";
        result = JSON.parse(r);
        break;
      }
      case "check":
        result = JSON.parse(w.check(args[0] as string));
        break;
      case "flatten":
        result = w.flatten(args[0] as string);
        break;
      case "simulate":
        live = null;
        result = JSON.parse(w.simulate(args[0] as string, JSON.stringify(args[1] ?? {})));
        break;
      case "linearAnalysis":
        result = JSON.parse(w.linearAnalysis(args[0] as string, JSON.stringify(args[1] ?? {})));
        break;
      case "simulateLive":
        result = await runLive(w, id, args[0] as string, args[1], (args[2] ?? {}) as { names?: string[]; pace?: number });
        break;
      case "liveVars":
        if (live) for (const n of args[0] as string[]) if (!live.names.includes(n)) live.names.push(n);
        result = !!live;
        break;
      case "liveParams": {
        // Served between two slices of the run: applied at the time reached.
        result = live ? JSON.parse(w.liveSetParams(JSON.stringify(args[0]))) : { ok: false, error: "no live run" };
        break;
      }
      case "liveControl": {
        const c = args[0] as { pause?: boolean; stop?: boolean; pace?: number };
        if (live) {
          if (c.pause !== undefined) live.paused = c.pause;
          if (c.stop) live.stop = true;
          if (c.pace !== undefined) {
            live.pace = c.pace;
            live.anchor = { t: live.t, wall: performance.now() };
          }
        }
        result = !!live;
        break;
      }
      case "values": {
        const names = args[0] as string[];
        const time = w.time();
        const cols = names.map((n) => w.values(n));
        transfer.push(time.buffer, ...cols.map((c) => c.buffer));
        result = { time, cols };
        break;
      }
      case "resultMat": {
        const b = w.resultMat();
        transfer.push(b.buffer);
        result = b;
        break;
      }
      case "run": {
        // One run of an experiment (pool.ts): reuses the translation between calls and
        // returns the requested trajectories with the summary.
        const summary = JSON.parse(w.simulateRun(args[0] as string, JSON.stringify(args[1] ?? {})));
        const names = (args[2] as string[]) ?? [];
        const time = w.time();
        const cols = names.map((n) => w.values(n));
        transfer.push(time.buffer, ...cols.map((c) => c.buffer));
        // Quantities of interest of a design of experiments, evaluated here.
        const qois = args[3] as string[] | null;
        const qoi = qois && summary.ok ? (JSON.parse(w.qoiValues(JSON.stringify(qois))) as (number | null)[]) : undefined;
        result = { summary, time, cols, qoi };
        break;
      }
      case "doePlan":
        result = JSON.parse(doePlan(JSON.stringify(args[0])));
        break;
      case "doeAnalyze":
        result = JSON.parse(doeAnalyze(args[0] as string, JSON.stringify(args[1]), JSON.stringify(args[2]), JSON.stringify(args[3]), JSON.stringify(args[4])));
        break;
      case "resultCsv":
        result = w.resultCsv(args[0] as string);
        break;
      case "setResource":
        w.setResource(args[0] as string, new Uint8Array(args[1] as ArrayBuffer));
        break;
      case "resource": {
        const b = w.resource(args[0] as string);
        if (b) transfer.push(b.buffer);
        result = b ?? null;
        break;
      }
      case "diffSources":
        result = JSON.parse(w.diffSources(args[0] as string, args[1] as string, args[2] as string, args[3] as string));
        break;
      case "setMetadata":
        result = JSON.parse(w.setMetadata(args[0] as string, JSON.stringify(args[1])));
        break;
      case "paramsParse":
        result = JSON.parse(w.paramsParse(args[0] as string, args[1] as string));
        break;
      case "paramsFor":
        result = JSON.parse(w.paramsFor(args[0] as string, args[1] as string, JSON.stringify(args[2]), args[3] as string));
        break;
      case "applyParams":
        result = JSON.parse(w.applyParams(args[0] as string, args[1] as string, JSON.stringify(args[2])));
        break;
      case "libraryIndex":
        result = w.libraryIndex();
        break;
      case "parseSpec":
        result = JSON.parse(w.parseSpec(args[0] as string));
        break;
      case "libraryReport":
        result = JSON.parse(w.libraryReport());
        break;
      case "installLibraryZip":
        result = JSON.parse(w.installLibraryZip(args[0] as Uint8Array, JSON.stringify(args[1]), args[2] as string));
        break;
      case "findLibraries":
        result = JSON.parse(w.findLibraries(JSON.stringify(args[0])));
        break;
      case "zipTexts": {
        const b = w.zipTexts(JSON.stringify(args[0]));
        transfer.push(b.buffer);
        result = b;
        break;
      }
      case "exportPackage": {
        const b = w.exportPackage(args[0] as string);
        transfer.push(b.buffer);
        result = b;
        break;
      }
      case "openPackage":
        result = JSON.parse(w.openPackage(args[0] as Uint8Array, args[1] as string));
        break;
      case "structure":
        result = JSON.parse(w.structure(args[0] as string, JSON.stringify(args[1] ?? {})));
        break;
      case "debug":
        result = JSON.parse(w.debug(args[0] as string, JSON.stringify(args[1] ?? {})));
        break;
      case "profile":
        result = JSON.parse(w.profile());
        break;
      case "editClass":
        result = JSON.parse(w.editClass(args[0] as string, JSON.stringify(args[1])));
        break;
      case "duplicateClass":
        result = JSON.parse(w.duplicateClass(args[0] as string, args[1] as string, args[2] as string));
        break;
      case "newModel":
        result = JSON.parse(w.newModel(args[0] as string, args[1] as string, args[2] as string));
        break;
      case "parameters":
        result = JSON.parse(w.parameters(args[0] as string));
        break;
      case "analysisRun": {
        // Runs in slices so progress is posted and a cancel request is seen between them.
        const [model, kind, spec, settings] = args as [string, string, unknown, unknown];
        analysisCancelled = false;
        const st = JSON.parse(w.analysisStart(model, kind, JSON.stringify(spec), JSON.stringify(settings ?? {})));
        if (!st.ok) throw new Error(st.error);
        const post = (done: number, total: number) => (self as unknown as Worker).postMessage({ analysis: { done, total } });
        post(st.done, st.total);
        let cancelled = false;
        for (;;) {
          const r = JSON.parse(w.analysisStep(120));
          if (r.error) throw new Error(r.error);
          post(r.progress[0], r.progress[1]);
          if (r.done) break;
          await new Promise((res) => setTimeout(res, 0));
          if (analysisCancelled) {
            w.analysisCancel();
            cancelled = true;
            break;
          }
        }
        result = cancelled ? { cancelled: true } : JSON.parse(w.analysisResult());
        break;
      }
      case "analysisCancel":
        analysisCancelled = true;
        break;
      default:
        throw new Error(`unknown op ${op}`);
    }
    (self as unknown as Worker).postMessage({ id, ok: true, result }, transfer);
  } catch (e) {
    (self as unknown as Worker).postMessage({ id, ok: false, error: String(e) });
  }
};
