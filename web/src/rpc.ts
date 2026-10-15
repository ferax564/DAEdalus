// Promise-based calls into the simulation worker.
import type { BundleInfo, BundleProgress, ClassEntry, ClassInfo, SimSettings, SimSummary } from "./types";

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
  // graphical editing (see diagram-edit.ts): each returns the edited document's new text
  editClass: (path: string, ops: unknown[]) => call<EditReply>("editClass", path, ops),
  duplicateClass: (path: string, uri: string, pkg: string) => call<EditReply>("duplicateClass", path, uri, pkg),
  newModel: (uri: string, pkg: string, name: string) => call<EditReply>("newModel", uri, pkg, name),
  parameters: (path: string) => call<ParamInfo[]>("parameters", path),
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
