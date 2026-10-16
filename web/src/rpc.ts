// Promise-based calls into the simulation worker.
import type { BundleInfo, BundleProgress, ClassDiff, ClassEntry, ClassInfo, IdeCompletion, IdeDiagnostic, IdeHover, IdeLocation, IdeRename, IdeSymbol, LibReport, LibSource, LinearOptions, LinearResult, PackageManifest, ParamValue, SimSettings, SimSummary, UnitSummary } from "./types";
import type { Profile } from "./performance";

/** A call that answers JSON `{"error": ...}` on failure: thrown as an Error. */
async function checked<T>(p: Promise<T | { error: string }>): Promise<T> {
  const r = await p;
  if (r && typeof r === "object" && !Array.isArray(r) && "error" in r && typeof (r as { error: unknown }).error === "string") throw new Error((r as { error: string }).error);
  return r as T;
}

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
let nextId = 1;
const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
/** Called with the progress of a library bundle being loaded. */
let onProgress: ((p: BundleProgress) => void) | null = null;

/** Called with the progress of a running analysis. */
let onAnalysis: ((p: { done: number; total: number }) => void) | null = null;

/** Rows of a live run (see worker.ts): the new time rows from row `from`, and per variable
 * its new values from its own `from`. */
export interface LiveChunk {
  id: number;
  t: number;
  stop: number;
  from: number;
  time: Float64Array;
  cols: Record<string, { from: number; v: Float64Array }>;
  paused: boolean;
}
const liveHandlers = new Map<number, (c: LiveChunk) => void>();

worker.onmessage = (ev) => {
  if (ev.data.live) {
    liveHandlers.get(ev.data.live.id)?.(ev.data.live as LiveChunk);
    return;
  }
  if (ev.data.analysis) {
    onAnalysis?.(ev.data.analysis);
    return;
  }
  if (ev.data.progress) {
    onProgress?.(ev.data.progress as BundleProgress);
    return;
  }
  const { id, ok, result, error } = ev.data;
  const p = pending.get(id);
  liveHandlers.delete(id);
  if (!p) return;
  pending.delete(id);
  if (ok) p.resolve(result);
  else p.reject(new Error(error));
};

function call<T>(op: string, ...args: unknown[]): Promise<T> {
  const id = nextId++;
  return new Promise<T>((resolve, reject) => {
    pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
    worker.postMessage({ id, op, args });
  });
}

/**
 * The documents and library bundle loaded into the worker, so that other workers (the
 * experiment pool, see pool.ts) can load the same sources. `seq` increases with every change.
 */
export const sources = {
  seq: 0,
  /** Latest text of each document (null: removed) and the change it came with. */
  docs: new Map<string, { text: string | null; seq: number }>(),
  bundle: null as { base: string; prefix: string; seq: number } | null,
};

export const api = {
  setDocuments: (docs: [string, string][]) => {
    const seq = ++sources.seq;
    for (const [uri, text] of docs) sources.docs.set(uri, { text, seq });
    return call<Record<string, string[]>>("setDocuments", docs);
  },
  /** Register a library bundle (`<base>manifest.json`) for on-demand parsing; null when the site has none. */
  loadBundle: (base: string, prefix: string, progress?: (p: BundleProgress) => void) => {
    onProgress = progress ?? null;
    sources.bundle = { base, prefix, seq: ++sources.seq };
    return call<BundleInfo | null>("loadBundle", base, prefix);
  },
  libraryStats: () => call<{ files: number; parsed: number }>("libraryStats"),
  removeDocument: (uri: string) => {
    sources.docs.set(uri, { text: null, seq: ++sources.seq });
    return call<void>("removeDocument", uri);
  },
  classTree: () => call<ClassEntry[]>("classTree"),
  classInfo: (path: string) => call<ClassInfo | null>("classInfo", path),
  documentText: (uri: string) => call<string | null>("documentText", uri),
  check: (model: string) => call<{ ok: boolean; error?: string; stats?: Record<string, number>; warnings?: string[]; states?: number; units?: UnitSummary }>("check", model),
  flatten: (model: string) => call<string>("flatten", model),
  simulate: (model: string, settings: SimSettings) => call<SimSummary>("simulate", model, settings),
  /** Linearize and analyze (poles, zeros, Bode, step, reduction, LQR). */
  linearAnalysis: (model: string, options: LinearOptions) => call<LinearResult>("linearAnalysis", model, options),
  /** Simulate in slices, posting the rows of `opts.names` as they come (`onChunk`); with
   * `opts.pace` > 0 paced at that many simulated seconds per second. Resolves to the same
   * summary as `simulate` (with `stopped` when ended by `liveControl({stop})`). */
  simulateLive: (model: string, settings: SimSettings, opts: { names: string[]; pace?: number }, onChunk: (c: LiveChunk) => void) => {
    liveHandlers.set(nextId, onChunk);
    return call<SimSummary & { stopped?: boolean }>("simulateLive", model, settings, opts);
  },
  /** Stream these variables too from the running live run. */
  liveVars: (names: string[]) => call<boolean>("liveVars", names),
  /** Change free Real parameters of the running live run, at the time it reached. */
  liveParams: (params: [string, number][]) => call<{ ok: boolean; error?: string; t?: number }>("liveParams", params),
  liveControl: (c: { pause?: boolean; stop?: boolean; pace?: number }) => call<boolean>("liveControl", c),
  values: (names: string[]) => call<{ time: Float64Array; cols: Float64Array[] }>("values", names),
  resultMat: () => call<Uint8Array>("resultMat"),
  resultCsv: (names: string) => call<string>("resultCsv", names),
  // editor intelligence: offsets are string indices into `text`, the document's current text
  ideUpdate: (uri: string, text: string) => call<IdeDiagnostic[]>("ideUpdate", uri, text),
  ideDefinition: (uri: string, text: string, pos: number) => call<IdeLocation[]>("ide", "definition", uri, text, pos),
  ideReferences: (uri: string, text: string, pos: number) => call<IdeLocation[]>("ide", "references", uri, text, pos),
  ideRename: (uri: string, text: string, pos: number, name: string) => call<IdeRename>("ide", "rename", uri, text, pos, name),
  ideCompletion: (uri: string, text: string, pos: number) => call<IdeCompletion>("ide", "completion", uri, text, pos),
  ideHover: (uri: string, text: string, pos: number) => call<IdeHover | null>("ide", "hover", uri, text, pos),
  ideSymbols: (uri: string, text: string) => call<IdeSymbol[]>("ide", "symbols", uri, text, 0),
  /** Structure report (see structure.ts); `opts.stateOverrides` are `[name, tier]` pairs. */
  structure: (model: string, opts: Record<string, unknown>) => call<unknown>("structure", model, opts),
  /** Performance profile and event log of the last simulation (null unless profiled). */
  profile: () => call<Profile | null>("profile"),
  // graphical editing (see diagram-edit.ts): each returns the edited document's new text
  editClass: (path: string, ops: unknown[]) => call<EditReply>("editClass", path, ops),
  duplicateClass: (path: string, uri: string, pkg: string) => call<EditReply>("duplicateClass", path, uri, pkg),
  newModel: (uri: string, pkg: string, name: string) => call<EditReply>("newModel", uri, pkg, name),
  parameters: (path: string) => call<ParamInfo[]>("parameters", path),
  /** Run an analysis (sensitivity | gsa | calibrate | optimize) in steps; resolves to its
   * JSON result, or `{cancelled: true}` after `analysisCancel`. */
  analysisRun: <T>(model: string, kind: string, spec: unknown, settings: SimSettings, progress?: (p: { done: number; total: number }) => void) => {
    onAnalysis = progress ?? null;
    return call<T | { cancelled: true }>("analysisRun", model, kind, spec, settings);
  },
  analysisCancel: () => call<void>("analysisCancel"),
  // library management, documentation, parameters, diff and packages
  setResource: (path: string, bytes: ArrayBuffer) => call<void>("setResource", path, bytes),
  resource: (path: string) => call<Uint8Array | null>("resource", path),
  diffSources: (oldText: string, newText: string, oldClass: string, newClass: string) =>
    checked(call<ClassDiff | { error: string }>("diffSources", oldText, newText, oldClass, newClass)),
  setMetadata: (cls: string, entries: [string, string][]) => checked(call<{ uri: string; text: string } | { error: string }>("setMetadata", cls, entries)),
  paramsParse: (text: string, fileName: string) => checked(call<ParamValue[] | { error: string }>("paramsParse", text, fileName)),
  paramsFor: (cls: string, component: string, params: ParamValue[], record: string) =>
    checked(call<{ params: ParamValue[]; messages: string[]; modifier: string; record: string } | { error: string }>("paramsFor", cls, component, params, record)),
  applyParams: (cls: string, component: string, params: ParamValue[]) =>
    checked(call<{ uri: string; text: string } | { error: string }>("applyParams", cls, component, params)),
  libraryIndex: () => call<string>("libraryIndex"),
  parseSpec: (spec: string) =>
    checked(call<{ source: LibSource; archives: string[]; cdn_listing: string; cdn_base: string } | { error: string }>("parseSpec", spec)),
  libraryReport: () => call<LibReport>("libraryReport"),
  installLibraryZip: (bytes: Uint8Array, packages: string[], version: string) =>
    checked(
      call<{ name: string; version?: string; files?: number; resources?: number; skipped?: string; uses?: [string, string][]; base?: string }[] | { error: string }>(
        "installLibraryZip",
        bytes,
        packages,
        version,
      ),
    ),
  findLibraries: (paths: string[]) => call<[string, string][]>("findLibraries", paths),
  zipTexts: (files: [string, string][]) => call<Uint8Array>("zipTexts", files),
  exportPackage: (model: string) => call<Uint8Array>("exportPackage", model),
  openPackage: (bytes: Uint8Array, prefix: string) =>
    checked(call<{ manifest: PackageManifest | null; files: number; resources: string[]; skipped: string[] } | { error: string }>("openPackage", bytes, prefix)),
};

export interface EditReply {
  ok: boolean;
  error?: string;
  uri: string;
  text: string;
  /** Names of added components (editClass). */
  added?: string[];
  /** Path of the new class (duplicateClass, newModel). */
  path?: string;
}

export interface ParamInfo {
  name: string;
  type_name: string;
  unit: string;
  description: string;
  default: string | null;
  declared_in: string;
}
