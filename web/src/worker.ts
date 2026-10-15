// Web Worker owning the WebAssembly workspace, so translation and simulation never block the UI.
import init, { Workspace } from "./wasm/daedalus_wasm.js";
import type { BundleInfo, BundleProgress } from "./types";

let ws: Workspace | null = null;
const ready = init().then(() => {
  ws = new Workspace();
});

type Req = { id: number; op: string; args: unknown[] };

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
      case "check":
        result = JSON.parse(w.check(args[0] as string));
        break;
      case "flatten":
        result = w.flatten(args[0] as string);
        break;
      case "simulate":
        result = JSON.parse(w.simulate(args[0] as string, JSON.stringify(args[1] ?? {})));
        break;
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
      case "resultCsv":
        result = w.resultCsv(args[0] as string);
        break;
      case "profile":
        result = JSON.parse(w.profile());
        break;
      default:
        throw new Error(`unknown op ${op}`);
    }
    (self as unknown as Worker).postMessage({ id, ok: true, result }, transfer);
  } catch (e) {
    (self as unknown as Worker).postMessage({ id, ok: false, error: String(e) });
  }
};
