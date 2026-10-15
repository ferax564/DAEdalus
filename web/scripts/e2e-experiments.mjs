// End-to-end check of the experiments UI (runs, sweep over the worker pool, overlay, diff,
// share link). Needs Playwright and a running dev server:
//   npx vite --port 5191 &
//   node scripts/e2e-experiments.mjs http://localhost:5191/ [screenshot dir]
// Exits non-zero when a check fails.
import { chromium } from "playwright";
import { mkdirSync } from "node:fs";

const base = process.argv[2] ?? "http://localhost:5191/";
const out = process.argv[3] ?? "experiments-shots";
mkdirSync(out, { recursive: true });
const MODEL = "Daedalus.Examples.QuarterCar.BumpComparison";
const fails = [];
const check = (cond, msg) => {
  console.log(`${cond ? "ok  " : "FAIL"} ${msg}`);
  if (!cond) fails.push(msg);
};

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM ?? "/opt/pw-browsers/chromium" });
const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
const errors = [];
const watch = (p) => {
  p.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  p.on("console", (c) => c.type() === "error" && errors.push(`console: ${c.text()}`));
};
const idle = (p) => p.waitForFunction(() => !document.querySelector("#btn-sim").disabled && window.daedalus?.timings?.interactive, null, { timeout: 300000 });

const p = await ctx.newPage();
watch(p);
await p.goto(`${base}#${MODEL}`);
await p.waitForFunction(() => window.daedalus?.timings?.interactive, null, { timeout: 120000 });
check((await p.evaluate(() => location.hash)) === `#${MODEL}`, "plain #Model.Name hash still opens the model");

// A single run, as before.
await p.click("#btn-sim");
await idle(p);
await p.evaluate(() => window.daedalus.togglePlot("linear.body.s"));
const single = await p.evaluate(() => window.daedalus.experiments.runs.map((r) => ({ label: r.label, vars: Object.keys(r.series) })));
check(single.length >= 1 && single[0].vars.includes("linear.body.s"), `single simulation stored as a run (${JSON.stringify(single[0])})`);

// 3 x 3 sweep over the pool.
await p.click("#runs-sweep");
await p.waitForSelector("#sweep-dialog[open]");
await p.selectOption("#sw-params tbody tr:nth-child(1) .sw-name", "linear.c_spring");
await p.fill("#sw-params tbody tr:nth-child(1) .sw-list", "2e4, 3e4, 4e4");
await p.click("#sw-add");
await p.selectOption("#sw-params tbody tr:nth-child(2) .sw-name", "bump.h");
await p.fill("#sw-params tbody tr:nth-child(2) .sw-list", "0.03, 0.05, 0.07");
await p.fill("#sw-vars", "linear.body.s, linear.a_body");
check((await p.textContent("#sw-count")).startsWith("9 runs"), `run count: ${await p.textContent("#sw-count")}`);
const t0 = Date.now();
await p.click("#sw-run");
await p.waitForFunction(() => /runs in/.test(document.querySelector("#sw-progress .label")?.textContent ?? ""), null, { timeout: 300000 });
const sweepMs = Date.now() - t0;
const sw = await p.evaluate(() => {
  const e = window.daedalus.experiments;
  const s = e.lastSweep;
  return {
    done: s.done,
    failed: s.failed,
    workers: [...s.workers],
    poolSize: s.runs.length && document.querySelector("#sw-progress .label").textContent,
    runs: s.runs.map((r) => ({ params: r.params, final: r.series["linear.body.s"]?.at(-1), max: Math.max(...(r.series["linear.a_body"] ?? [])), worker: r.worker })),
  };
});
console.log(`sweep took ${sweepMs} ms: ${sw.poolSize}`);
check(sw.done === 9 && sw.failed === 0, `9 runs ok (${sw.done} done, ${sw.failed} failed)`);
check(sw.workers.length >= 2, `runs spread over several workers: ${JSON.stringify(sw.workers)}`);
const peak = await p.evaluate(() => /at most (\d+) at once/.exec(document.querySelector("#sw-progress .label").textContent)?.[1]);
check(+peak >= 2, `runs in flight at once: ${peak}`);
// Each run reflects its parameters: a higher bump gives a larger peak acceleration.
const byH = (h, c) => sw.runs.find((r) => r.params.some(([n, v]) => n === "bump.h" && v === h) && r.params.some(([n, v]) => n === "linear.c_spring" && v === c));
check(byH(0.07, 3e4).max > byH(0.05, 3e4).max && byH(0.05, 3e4).max > byH(0.03, 3e4).max, "peak acceleration grows with bump height");
// Same values as a single Simulate with those overrides.
const ref = await p.evaluate(async () => {
  const api = window.daedalus.api;
  const s = await api.simulate("Daedalus.Examples.QuarterCar.BumpComparison", { params: [["linear.c_spring", 40000], ["bump.h", 0.07]] });
  const v = await api.values(["linear.body.s"]);
  return { ok: s.ok, final: v.cols[0].at(-1) };
});
check(Math.abs(ref.final - byH(0.07, 4e4).final) < 1e-12, `pool run equals a single run with the same overrides (${ref.final} vs ${byH(0.07, 4e4).final})`);
await p.selectOption("#sw-stat", "max");
await p.selectOption("#sw-var", "linear.a_body");
await p.waitForTimeout(400);
const tableRows = await p.$$eval("#sw-summary tbody tr", (r) => r.length);
check(tableRows === 9, `summary table has 9 rows`);
const scalarCurves = await p.evaluate(() => document.querySelectorAll("#sw-scalar canvas").length);
check(scalarCurves === 1, "scalar-vs-parameter plot drawn");
await p.screenshot({ path: `${out}/1_sweep_dialog.png` });
await p.click("#sweep-dialog .sw-head button");

// Overlay of the sweep in the plot.
const overlays = await p.evaluate(() => window.daedalus.plot.subplots.flatMap((sp) => sp.series.filter((s) => s.run).map((s) => s.name)));
check(overlays.length === 18, `sweep overlaid: ${overlays.length} curves, e.g. "${overlays[0]}"`);
check(/linear\.body\.s · sweep 1 #1 \(linear\.c_spring=20000, bump\.h=0\.03\)/.test(overlays[0]), "legend names the run and its differing parameters");
await p.waitForTimeout(300);
await p.screenshot({ path: `${out}/2_overlay.png` });

// Diff of two runs (B - A on a common grid).
await p.click("#runs-clear");
await p.evaluate(() => {
  const plot = window.daedalus.plot;
  plot.clear();
});
await p.evaluate(() => window.daedalus.togglePlot("linear.body.s"));
await p.click("#runs-none");
const ids = await p.evaluate(() => {
  const s = window.daedalus.experiments.lastSweep.runs;
  return [s[0].id, s[8].id];
});
for (const id of ids) await p.check(`#runs-list .run-row[data-run="${id}"] input[type=checkbox]`);
check(!(await p.isDisabled("#runs-diff")), "Diff enabled with two runs selected");
await p.click("#runs-diff");
const d = await p.evaluate((ids) => {
  const e = window.daedalus.experiments;
  const [a, b] = ids.map((id) => e.runs.find((r) => r.id === id));
  const sp = window.daedalus.plot.subplots.at(-1);
  const s = sp.series[0];
  // compare the diff at a few times against an independent interpolation
  const lin = (t, y, x) => {
    let i = 1;
    while (i < t.length - 1 && t[i] < x) i++;
    return y[i - 1] + ((y[i] - y[i - 1]) * (x - t[i - 1])) / (t[i] - t[i - 1] || 1);
  };
  let err = 0;
  for (const k of [0.1, 0.5, 0.9]) {
    const i = Math.floor(k * (s.x.length - 1));
    const x = s.x[i];
    err = Math.max(err, Math.abs(s.y[i] - (lin(b.t, b.series["linear.body.s"], x) - lin(a.t, a.series["linear.body.s"], x))));
  }
  return { name: s.name, n: s.x.length, err, final: s.y.at(-1), want: b.series["linear.body.s"].at(-1) - a.series["linear.body.s"].at(-1) };
}, ids);
check(/ − /.test(d.name) && d.err < 1e-9 && Math.abs(d.final - d.want) < 1e-12, `diff B − A matches (${d.name}, max err ${d.err})`);
await p.waitForTimeout(300);
await p.screenshot({ path: `${out}/3_diff.png` });

// Shareable link round trip.
await p.evaluate(() => {
  window.daedalus.plot.clear();
});
await p.evaluate(() => window.daedalus.togglePlot("linear.body.s"));
await p.evaluate(() => {
  document.querySelector("#set-stop").value = "1.5";
});
const link = await p.evaluate(() => {
  const e = window.daedalus.experiments;
  return e.shareUrl("Daedalus.Examples.QuarterCar.BumpComparison", [["bump.h", 0.07], ["linear.c_spring", 40000]], { stop_time: 1.5 }, ["linear.body.s"]);
});
await p.click("#runs-share");
const logged = await p.evaluate(() => [...document.querySelectorAll("#log div")].map((d) => d.textContent).filter((t) => t.startsWith("Link: ")).at(-1));
check(!!logged && logged.includes("stop=1.5") && logged.includes("plot=linear.body.s"), `Share link button: ${logged}`);
console.log(`link: ${link}`);
const q = await ctx.newPage();
watch(q);
await q.goto(link);
await q.waitForFunction(() => window.daedalus?.timings?.interactive, null, { timeout: 120000 });
await idle(q);
await q.waitForFunction(() => window.daedalus.plot.subplots.some((sp) => sp.series.some((s) => s.name === "linear.body.s")), null, { timeout: 60000 });
const shared = await q.evaluate(() => {
  const r = window.daedalus.experiments.runs.find((x) => !x.sweep && x.params.length === 2);
  const s = window.daedalus.plot.subplots[0].series.find((x) => x.name === "linear.body.s");
  return { params: r?.params, stop: s.x.at(-1), final: s.y.at(-1), inputStop: document.querySelector("#set-stop").value };
});
check(JSON.stringify(shared.params) === JSON.stringify([["bump.h", 0.07], ["linear.c_spring", 40000]]), `link applied the overrides ${JSON.stringify(shared.params)}`);
check(shared.stop === 1.5 && shared.inputStop === "1.5", `link applied stop time (${shared.stop})`);
const sweepAt15 = await p.evaluate(() => {
  const r = window.daedalus.experiments.lastSweep.runs[8];
  const t = r.t, y = r.series["linear.body.s"];
  const i = t.findIndex((x) => x >= 1.5);
  return y[i];
});
check(Math.abs(shared.final - sweepAt15) < 1e-6, `shared run matches the sweep run at t = 1.5 (${shared.final} vs ${sweepAt15})`);
await q.waitForTimeout(300);
await q.screenshot({ path: `${out}/4_shared_link.png` });

// Runs survive a reload (IndexedDB).
const before = await p.evaluate(() => window.daedalus.experiments.runs.length);
await p.reload();
await p.waitForFunction(() => window.daedalus?.timings?.interactive, null, { timeout: 120000 });
await p.waitForTimeout(500);
const after = await p.evaluate(() => window.daedalus.experiments.runs.length);
check(after >= before, `runs persisted across reload (${before} -> ${after})`);

// Cancel a sweep, then run another one on the restarted workers.
await p.click("#btn-sim");
await idle(p);
await p.click("#runs-sweep");
await p.waitForSelector("#sweep-dialog[open]");
await p.selectOption("#sw-params tbody tr:nth-child(1) .sw-name", "linear.c_spring");
await p.selectOption("#sw-params tbody tr:nth-child(1) .sw-spacing", "lin");
await p.fill("#sw-params tbody tr:nth-child(1) .sw-a", "1e4");
await p.fill("#sw-params tbody tr:nth-child(1) .sw-b", "5e4");
await p.fill("#sw-params tbody tr:nth-child(1) .sw-n", "200");
await p.click("#sw-run");
await p.waitForFunction(() => window.daedalus.experiments.lastSweep?.done >= 1, null, { timeout: 120000 });
console.log(await p.textContent("#sw-progress .label"));
await p.click("#sw-cancel", { timeout: 5000 });
await p.waitForFunction(() => /Cancelled/.test(document.querySelector("#sw-progress .label")?.textContent ?? ""), null, { timeout: 60000 });
const cancelled = await p.evaluate(() => window.daedalus.experiments.lastSweep.done);
check(cancelled < 200, `sweep cancelled after ${cancelled} of 200 runs`);
await p.fill("#sw-params tbody tr:nth-child(1) .sw-n", "4");
await p.selectOption("#sw-params tbody tr:nth-child(1) .sw-spacing", "log");
await p.click("#sw-run");
await p.waitForFunction(() => /runs in/.test(document.querySelector("#sw-progress .label")?.textContent ?? ""), null, { timeout: 300000 });
const again = await p.evaluate(() => {
  const s = window.daedalus.experiments.lastSweep;
  return { ok: s.done - s.failed, values: s.runs.map((r) => r.params.find((q) => q[0] === "linear.c_spring")[1]) };
});
check(again.ok === 4 && Math.abs(again.values[1] - 1e4 * Math.pow(5, 1 / 3)) < 1e-6, `log sweep after cancel: ${JSON.stringify(again)}`);

check(errors.length === 0, `no console errors ${errors.length ? JSON.stringify(errors.slice(0, 5)) : ""}`);
await browser.close();
if (fails.length) {
  console.log(`${fails.length} checks failed`);
  process.exit(1);
}
console.log("all checks passed");
