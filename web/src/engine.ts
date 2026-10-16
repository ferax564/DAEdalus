// Client of the local engine (`daedalus serve`, crates/daedalus-cli/src/serve.rs): a native
// process on the user's machine that runs the same Workspace as the WebAssembly worker, so
// simulations get native speed and every core. Used by worker.ts (the Simulate button and
// analyses) and pool.ts (experiment runs, one engine "space" per pool slot).

export const DEFAULT_ENGINE = "http://127.0.0.1:7878";

export interface EngineStatus {
  engine: string;
  version: string;
  cores: number;
}

async function check(r: Response): Promise<Response> {
  if (!r.ok) throw new Error(`local engine: ${r.status} ${await r.text()}`);
  return r;
}

export async function engineStatus(base: string): Promise<EngineStatus> {
  const r = await check(await fetch(`${base}/status`));
  const s = (await r.json()) as EngineStatus;
  if (s.engine !== "daedalus") throw new Error(`${base} is not a DAEdalus engine`);
  return s;
}

export async function enginePost(base: string, path: string, body: unknown): Promise<Response> {
  return check(
    await fetch(`${base}${path}`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(body),
    }),
  );
}

/** Call a Workspace method in an engine space; JSON replies are parsed. */
export async function engineCall<T>(base: string, space: string, op: string, args: unknown[]): Promise<T> {
  const r = await enginePost(base, `/call/${space}`, { op, args });
  return (r.headers.get("content-type") ?? "").includes("json") ? ((await r.json()) as T) : ((await r.text()) as T);
}

/** `values` from an engine space: the time vector and one column per name (empty when missing). */
export async function engineValues(base: string, space: string, names: string[]): Promise<{ time: Float64Array; cols: Float64Array[] }> {
  const r = await enginePost(base, `/call/${space}`, { op: "values", args: [names] });
  const buf = await r.arrayBuffer();
  const head = new DataView(buf);
  const n = Number(head.getBigUint64(0, true));
  const k = Number(head.getBigUint64(8, true));
  const lens = Array.from({ length: k }, (_, i) => Number(head.getBigUint64(16 + 8 * i, true)));
  let off = 16 + 8 * k;
  // Copy out of the response buffer so each array can be transferred on its own.
  const take = (len: number) => {
    const a = new Float64Array(buf.slice(off, off + 8 * len));
    off += 8 * len;
    return a;
  };
  const time = take(n);
  return { time, cols: lens.map(take) };
}

export async function engineBytes(base: string, space: string, op: string, args: unknown[]): Promise<Uint8Array> {
  const r = await enginePost(base, `/call/${space}`, { op, args });
  return new Uint8Array(await r.arrayBuffer());
}
