import { expect, test, vi } from "vitest";

class FakeWorker {
  static all: FakeWorker[] = [];
  posts: { id: number; op: string; args: unknown[] }[] = [];
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onerror: ((ev: ErrorEvent) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  constructor(_url: string | URL, _opts?: WorkerOptions) {
    FakeWorker.all.push(this);
  }
  postMessage(data: { id: number; op: string; args: unknown[] }) {
    this.posts.push(data);
  }
  terminate() {}
}

vi.stubGlobal("Worker", FakeWorker);

const { api } = await import("./rpc");

function crash(worker: FakeWorker, message: string) {
  worker.onerror?.({ message, preventDefault() {} } as ErrorEvent);
}

async function flush() {
  for (let i = 0; i < 6; i++) await Promise.resolve();
}

test("a worker error rejects api.simulate and starts a new worker", async () => {
  const first = FakeWorker.all.at(-1)!;
  const pending = api.simulate("M", { stop_time: 1 } as never);
  await flush();
  expect(first.onerror).toBeTypeOf("function");
  expect(first.posts.at(-1)?.op).toBe("simulate");
  const n = FakeWorker.all.length;
  crash(first, "oom");
  await expect(pending).rejects.toThrow(/worker restarted/);
  expect(FakeWorker.all.length).toBe(n + 1);
});

test("a retry waits until the crashed worker's sources have been replayed", async () => {
  const first = FakeWorker.all.at(-1)!;
  const loading = api.setDocuments([["user/a.mo", "model A end A;"]]);
  await flush();
  const sent = first.posts.find((p) => p.op === "setDocuments");
  expect(sent).toBeTruthy();
  crash(first, "oom");
  await expect(loading).rejects.toThrow(/worker restarted/);

  const next = FakeWorker.all.at(-1)!;
  expect(next).not.toBe(first);
  expect(next.posts.map((p) => p.op)).toEqual(["setDocuments"]);

  const retry = api.simulate("A", { stop_time: 1 } as never);
  await flush();
  expect(next.posts.map((p) => p.op)).toEqual(["setDocuments"]);

  const replay = next.posts[0];
  next.onmessage?.({ data: { id: replay.id, ok: true, result: {} } } as MessageEvent);
  await flush();
  expect(next.posts.map((p) => p.op)).toEqual(["setDocuments", "simulate"]);
  const sim = next.posts[1];
  next.onmessage?.({ data: { id: sim.id, ok: true, result: { ok: true } } } as MessageEvent);
  await expect(retry).resolves.toEqual({ ok: true });
});
