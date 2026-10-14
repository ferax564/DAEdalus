// Web Worker owning the WebAssembly workspace, so translation and simulation never block the UI.
import init, { Workspace } from "./wasm/daedalus_wasm.js";

let ws: Workspace | null = null;
const ready = init().then(() => {
  ws = new Workspace();
});

type Req = { id: number; op: string; args: unknown[] };

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
      default:
        throw new Error(`unknown op ${op}`);
    }
    (self as unknown as Worker).postMessage({ id, ok: true, result }, transfer);
  } catch (e) {
    (self as unknown as Worker).postMessage({ id, ok: false, error: String(e) });
  }
};
