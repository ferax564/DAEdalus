// Promise-based calls into the simulation worker.
import type { BundleInfo, BundleProgress, ClassDiff, ClassEntry, ClassInfo, LibReport, LibSource, PackageManifest, ParamValue, SimSettings, SimSummary } from "./types";

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
