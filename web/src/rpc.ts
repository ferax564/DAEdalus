// Promise-based calls into the simulation worker.
import type { BundleInfo, BundleProgress, ClassEntry, ClassInfo, IdeCompletion, IdeDiagnostic, IdeHover, IdeLocation, IdeRename, IdeSymbol, SimSettings, SimSummary } from "./types";

const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
let nextId = 1;
const pending = new Map<number, { resolve: (v: unknown) => void; reject: (e: Error) => void }>();
/** Called with the progress of a library bundle being loaded. */
let onProgress: ((p: BundleProgress) => void) | null = null;

worker.onmessage = (ev) => {
  if (ev.data.progress) {
    onProgress?.(ev.data.progress as BundleProgress);
    return;
  }
  const { id, ok, result, error } = ev.data;
  const p = pending.get(id);
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

export const api = {
  setDocuments: (docs: [string, string][]) => call<Record<string, string[]>>("setDocuments", docs),
  /** Register a library bundle (`<base>manifest.json`) for on-demand parsing; null when the site has none. */
  loadBundle: (base: string, prefix: string, progress?: (p: BundleProgress) => void) => {
    onProgress = progress ?? null;
    return call<BundleInfo | null>("loadBundle", base, prefix);
  },
  libraryStats: () => call<{ files: number; parsed: number }>("libraryStats"),
  removeDocument: (uri: string) => call<void>("removeDocument", uri),
  classTree: () => call<ClassEntry[]>("classTree"),
  classInfo: (path: string) => call<ClassInfo | null>("classInfo", path),
  documentText: (uri: string) => call<string | null>("documentText", uri),
  check: (model: string) => call<{ ok: boolean; error?: string; stats?: Record<string, number>; warnings?: string[]; states?: number }>("check", model),
  flatten: (model: string) => call<string>("flatten", model),
  simulate: (model: string, settings: SimSettings) => call<SimSummary>("simulate", model, settings),
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
};
