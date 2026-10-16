import { expect, test, vi } from "vitest";

class FakeWorker {
  static all: FakeWorker[] = [];
  onmessage: ((ev: MessageEvent) => void) | null = null;
  onerror: ((ev: ErrorEvent) => void) | null = null;
  onmessageerror: (() => void) | null = null;
  constructor(_url: string | URL, _opts?: WorkerOptions) {
    FakeWorker.all.push(this);
  }
  postMessage(_data: unknown) {}
  terminate() {}
}

vi.stubGlobal("Worker", FakeWorker);

const { api } = await import("./rpc");

test("a worker error rejects api.simulate and starts a new worker", async () => {
  const pending = api.simulate("M", { stop_time: 1 } as never);
  const first = FakeWorker.all[0];
  expect(first.onerror).toBeTypeOf("function");
  first.onerror?.({ message: "oom", preventDefault() {} } as ErrorEvent);
  await expect(pending).rejects.toThrow(/worker restarted/);
  expect(FakeWorker.all.length).toBeGreaterThan(1);
});
