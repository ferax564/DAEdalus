// End-to-end check of the plotting upgrades and dashboards: click-to-plot from the diagram
// (Modeling plot strip and Simulation page), matrix layout with synchronized time axes,
// plot expressions and signal operators, window statistics, live plotting, and the
// dashboard (sliders changing parameters during a paced live run, gauges, displays,
// mini plots, saved layouts, share links). Also checks that the Structure view, Analysis
// mode, Experiments and the Performance tab still work. Needs Playwright and a dev server:
//   npx vite --port 5537 &
//   PLAYWRIGHT=/path/to/playwright/index.mjs node scripts/e2e-plotting.mjs http://localhost:5537/ [screenshot dir]
// Exits non-zero when a check fails.
import { mkdirSync } from "node:fs";

const { chromium } = await import(process.env.PLAYWRIGHT ?? "playwright");
const base = process.argv[2] ?? "http://localhost:5537/";
const out = process.argv[3] ?? "plotting-shots";
mkdirSync(out, { recursive: true });
const fails = [];
const check = (cond, msg) => {
  console.log(`${cond ? "ok  " : "FAIL"} ${msg}`);
  if (!cond) fails.push(msg);
};

const browser = await chromium.launch({ executablePath: process.env.CHROMIUM ?? "/opt/pw-browsers/chromium" });
const ctx = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
await ctx.grantPermissions(["clipboard-read", "clipboard-write"]);
const errors = [];
const watch = (p) => {
  p.on("pageerror", (e) => errors.push(`pageerror: ${e.message}`));
  p.on("console", (c) => c.type() === "error" && errors.push(`console: ${c.text()}`));
};
const idle = (p) => p.waitForFunction(() => !document.querySelector("#btn-sim").disabled && !window.daedalus?.live() && window.daedalus?.timings?.interactive, null, { timeout: 300000 });
const shot = (p, name) => p.screenshot({ path: `${out}/${name}.png` });
const series = (p) => p.evaluate(() => window.daedalus.plot.subplots.map((sp) => ({ xName: sp.xName ?? null, names: sp.series.map((s) => s.name), n: sp.series.map((s) => s.y.length) })));

const p = await ctx.newPage();
watch(p);
const MSL = "Modelica.Mechanics.Rotational.Examples.First";
await p.goto(`${base}#${MSL}`);
await p.waitForFunction(() => window.daedalus?.timings?.interactive, null, { timeout: 300000 });
check((await p.evaluate(() => location.hash)) === `#${MSL}`, "model opened from the hash");

// ------------------------------------------------------------ live plotting (default on)
await p.evaluate(() => window.daedalus.setMode("simulation"));
await p.fill("#set-n", "200000");
const before = await p.evaluate(() => ({ ...window.daedalus.liveStats }));
await p.click("#btn-sim");
await idle(p);
await p.fill("#set-n", "");
let after = await p.evaluate(() => ({ ...window.daedalus.liveStats }));
check(after.runs === before.runs + 1, "Simulate with Live on streams the run");
check(after.chunks - before.chunks >= 1, `live slices posted (${after.chunks - before.chunks})`);
const sum1 = await p.evaluate(() => ({ mode: window.daedalus.mode(), vars: window.daedalus.api && document.querySelectorAll("#variables .node").length }));
check(sum1.mode === "simulation", "results shown in Simulation mode");

// The live path gives the same values as the plain one.
const same = await p.evaluate(async () => {
  const api = window.daedalus.api;
  const a = await api.simulate("Modelica.Mechanics.Rotational.Examples.First", {});
  const va = await api.values(["inertia3.w"]);
  let chunks = 0;
  const b = await api.simulateLive("Modelica.Mechanics.Rotational.Examples.First", {}, { names: ["inertia3.w"] }, () => chunks++);
  const vb = await api.values(["inertia3.w"]);
  const x = va.cols[0];
  const y = vb.cols[0];
  let diff = 0;
  for (let i = 0; i < x.length; i++) diff = Math.max(diff, Math.abs(x[i] - y[i]));
  return { ok: a.ok && b.ok, n: [x.length, y.length], diff, chunks, vars: b.vars.length === a.vars.length };
});
check(same.ok && same.n[0] === same.n[1] && same.diff === 0 && same.vars, `live and plain runs identical (${JSON.stringify(same)})`);

// Live with a long run: the plot draws while it runs.
await p.evaluate(() => window.daedalus.togglePlot("inertia3.w"));
await p.fill("#set-stop", "200");
await p.fill("#set-n", "400000");
const c0 = await p.evaluate(() => window.daedalus.liveStats.draws);
await p.click("#btn-sim");
await p.waitForFunction(() => window.daedalus.live() && window.daedalus.live().n > 1000, null, { timeout: 60000 });
const mid = await p.evaluate(() => ({ n: window.daedalus.plot.subplots[0].series[0].y.length, stopShown: !document.querySelector("#live-stop").classList.contains("hidden"), status: document.querySelector("#live-status").textContent }));
await shot(p, "01-live-plot-running");
check(mid.n > 0 && mid.stopShown, `plot updates during the run (${JSON.stringify(mid)})`);
await p.click("#live-stop");
await idle(p);
const stopped = await p.evaluate(() => ({ draws: window.daedalus.liveStats.draws, n: window.daedalus.plot.subplots[0].series[0].y.length, last: document.querySelector("#log").lastChild?.textContent ?? "" }));
check(stopped.draws > c0, `live draws (${stopped.draws - c0})`);
check(stopped.n > 0, `stopped run keeps its rows (${stopped.n})`);
await p.fill("#set-stop", "");
await p.fill("#set-n", "");
await p.click("#btn-sim");
await idle(p);

// ------------------------------------------------------------ click-to-plot, Simulation page
await p.evaluate(() => window.daedalus.plot.clear());
const simDiag = await p.evaluate(() => !document.querySelector("#sim-diagram").classList.contains("hidden"));
if (!simDiag) await p.click("#sim-diagram-btn");
await p.waitForSelector("#sim-diagram svg .component[data-name='inertia1']", { timeout: 30000 });
await p.click("#sim-diagram svg .component[data-name='inertia1'] .hit", { force: true });
await p.waitForSelector(".var-menu");
const menu = await p.evaluate(() => [...document.querySelectorAll(".var-menu .vm-name")].map((e) => e.textContent));
check(menu.includes("w") && menu.includes("phi") && menu.some((n) => n.startsWith("flange_a.")), `component menu lists main variables (${menu.slice(0, 8).join(", ")}…)`);
await shot(p, "02-sim-diagram-component-menu");
await p.click(".var-menu .vm-row:has(.vm-name:text-is('w')) input");
await p.waitForFunction(() => window.daedalus.plot.subplots[0].series.some((s) => s.name === "inertia1.w"), null, { timeout: 10000 }).catch(() => {});
check((await series(p))[0].names.includes("inertia1.w"), "clicking a menu entry plots inertia1.w");
await p.keyboard.press("Escape");
// a connection: across and flow variables of both connectors
const hit = await p.$("#sim-diagram svg .connection-hit");
check(!!hit, "connections are clickable");
await p.evaluate(() => {
  const h = document.querySelector("#sim-diagram svg .connection-hit[data-from^='inertia1']") ?? document.querySelector("#sim-diagram svg .connection-hit");
  h.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 600, clientY: 400 }));
});
await p.waitForSelector(".var-menu");
const cm = await p.evaluate(() => ({
  title: document.querySelector(".var-menu .vm-title").textContent,
  rows: [...document.querySelectorAll(".var-menu .vm-row")].map((r) => [r.querySelector(".vm-name").textContent, r.querySelector(".vm-tag")?.textContent]),
}));
check(cm.rows.some(([, t]) => t === "flow") && cm.rows.some(([, t]) => t === "across"), `connection menu tags across and flow variables (${cm.title}: ${JSON.stringify(cm.rows.slice(0, 4))})`);
await shot(p, "03-sim-diagram-connection-menu");
const flowRow = cm.rows.findIndex(([, t]) => t === "flow");
await p.click(`.var-menu .vm-row:nth-child(${flowRow + 1}) input`);
await p.waitForFunction((n) => window.daedalus.plot.subplots[0].series.some((s) => s.name === n), cm.rows[flowRow][0], { timeout: 10000 }).catch(() => {});
check((await series(p))[0].names.includes(cm.rows[flowRow][0]), `flow variable ${cm.rows[flowRow][0]} plotted`);
await p.keyboard.press("Escape");

// ------------------------------------------------------------ matrix layout, synchronized zoom
await p.selectOption("#plot-layout", { label: "2 × 2" });
let sp = await series(p);
check(sp.length === 4, `2 × 2 layout has 4 subplots (${sp.length})`);
await p.evaluate(() => {
  const d = window.daedalus;
  d.plot.active = 1;
});
await p.evaluate(() => window.daedalus.togglePlot("inertia2.w"));
await p.evaluate(() => (window.daedalus.plot.active = 3));
await p.evaluate(() => window.daedalus.togglePlot("inertia3.w"));
// drag-zoom in the bottom-right subplot
const box = await p.$eval("#plot canvas", (c) => {
  const r = c.getBoundingClientRect();
  return { x: r.x, y: r.y, w: r.width, h: r.height };
});
await p.mouse.move(box.x + box.w * 0.62, box.y + box.h * 0.8);
await p.mouse.down();
await p.mouse.move(box.x + box.w * 0.75, box.y + box.h * 0.8, { steps: 4 });
await p.mouse.up();
const zoomed = await p.evaluate(() => window.daedalus.plot.visibleRange());
check(zoomed[0] > 0.05 && zoomed[1] < 0.95, `drag zoom narrows the shared time axis (${zoomed.map((v) => v.toFixed(3))})`);
// wheel zoom out and shift-drag pan, applied to every time subplot
await p.mouse.move(box.x + box.w * 0.3, box.y + box.h * 0.3);
await p.mouse.wheel(0, 200);
const wheeled = await p.evaluate(() => window.daedalus.plot.visibleRange());
check(wheeled[1] - wheeled[0] > zoomed[1] - zoomed[0], "wheel zooms out");
await p.keyboard.down("Shift");
await p.mouse.down();
await p.mouse.move(box.x + box.w * 0.2, box.y + box.h * 0.3, { steps: 4 });
await p.mouse.up();
await p.keyboard.up("Shift");
const panned = await p.evaluate(() => window.daedalus.plot.visibleRange());
check(Math.abs(panned[1] - panned[0] - (wheeled[1] - wheeled[0])) < 1e-9 && panned[0] > wheeled[0], `shift-drag pans (${panned.map((v) => v.toFixed(3))})`);
await p.mouse.move(box.x + box.w * 0.3, box.y + box.h * 0.75);
await shot(p, "04-matrix-layout-synced-zoom");
await p.mouse.dblclick(box.x + box.w * 0.3, box.y + box.h * 0.75);

// ------------------------------------------------------------ expressions and operators
await p.evaluate(() => (window.daedalus.plot.active = 2));
await p.fill("#plot-expr", "inertia3.w*60/(2*pi)");
await p.press("#plot-expr", "Enter");
await p.waitForFunction(() => window.daedalus.plot.subplots[2].series.length === 1);
const ex = await p.evaluate(() => {
  const s = window.daedalus.plot.subplots[2].series[0];
  const w = window.daedalus.plot.subplots[3].series[0];
  return { name: s.name, y: s.y.at(-1), w: w.y.at(-1) };
});
check(Math.abs(ex.y - (ex.w * 60) / (2 * Math.PI)) < 1e-9 * Math.max(1, Math.abs(ex.y)), `expression evaluated (${ex.name} = ${ex.y})`);
for (const e of ["der(inertia3.phi)", "abs(inertia2.w - inertia3.w)", "integral(inertia3.w)", "movavg(inertia3.w, 0.05)", "rms(inertia1.w, 0.1)"]) {
  await p.fill("#plot-expr", e);
  await p.press("#plot-expr", "Enter");
}
await p.waitForFunction(() => window.daedalus.plot.subplots[2].series.length === 6);
const chk = await p.evaluate(() => {
  const sp = window.daedalus.plot.subplots[2].series;
  const by = (n) => sp.find((s) => s.name === n);
  const w3 = window.daedalus.plot.subplots[3].series[0].y;
  const der = by("der(inertia3.phi)").y;
  let d = 0;
  for (let i = 0; i < w3.length; i++) d = Math.max(d, Math.abs(der[i] - w3[i]));
  const integ = by("integral(inertia3.w)").y;
  return { derDiff: d, integ: integ.at(-1) };
});
check(chk.derDiff < 1e-9, `der(inertia3.phi) uses the solver's derivative = inertia3.w (max diff ${chk.derDiff})`);
// bad expression reported, not plotted
await p.fill("#plot-expr", "nosuch.var + 1");
await p.press("#plot-expr", "Enter");
await p.waitForTimeout(200);
const lastLog = await p.evaluate(() => document.querySelector("#log").lastChild.textContent);
check(/unknown variable/.test(lastLog), `unknown variable reported (${lastLog})`);
await p.fill("#plot-expr", "");
// FFT: inertia1 is driven by a 5 Hz sine (First: f = 5)
await p.fill("#plot-expr", "fft(inertia1.w)");
await p.press("#plot-expr", "Enter");
await p.waitForFunction(() => window.daedalus.plot.subplots.some((sp) => sp.xName === "frequency"));
const fft = await p.evaluate(() => {
  const sp = window.daedalus.plot.subplots.find((s) => s.xName === "frequency");
  const s = sp.series[0];
  let k = 1;
  for (let i = 1; i < s.y.length; i++) if (s.y[i] > s.y[k]) k = i;
  return { peakHz: s.x[k], n: s.y.length };
});
check(Math.abs(fft.peakHz - 5) < 1.01, `FFT peak near 5 Hz (${fft.peakHz.toFixed(2)} Hz, ${fft.n} bins)`);
// operator menu on the active subplot
await p.evaluate(() => (window.daedalus.plot.active = 1));
await p.selectOption("#plot-ops", "der");
await p.waitForFunction(() => window.daedalus.plot.subplots[1].series.some((s) => s.name === "der(inertia2.w)"));
check(true, "signal operator menu adds der(inertia2.w)");
// window statistics
await p.click("#plot-stats-btn");
await p.waitForSelector("#plot-stats table");
const stats = await p.evaluate(() => [...document.querySelectorAll("#plot-stats tbody tr")].map((r) => [...r.children].map((c) => c.textContent)));
check(stats.length >= 8 && stats.every((r) => r.length === 8), `stats table: ${stats.length} rows`);
await p.evaluate(() => window.daedalus.plot.setRange([0.2, 0.4]));
await p.waitForTimeout(100);
const head = await p.textContent("#plot-stats .stats-head");
check(/0\.2 … 0\.4/.test(head), `stats follow the visible window (${head})`);
await shot(p, "05-expressions-fft-stats");
await p.evaluate(() => window.daedalus.plot.setRange(null));
// expressions survive a new simulation (re-evaluated)
await p.click("#btn-sim");
await idle(p);
const kept = await series(p);
check(kept[2].names.length === 6 && kept.some((s) => s.xName === "frequency" && s.n[0] > 8), "expression and spectrum series re-evaluated after a new run");
await p.click("#plot-stats-btn");

// ------------------------------------------------------------ plot strip on the Modeling page
await p.selectOption("#plot-layout", { label: "Stacked" });
await p.evaluate(() => window.daedalus.plot.clear());
await p.evaluate(() => window.daedalus.togglePlot("inertia1.w"));
await p.evaluate(() => window.daedalus.setMode("modeling"));
await p.waitForSelector("#plot-strip:not(.hidden)");
const strip = await p.evaluate(() => ({
  inStrip: !!document.querySelector("#strip-host canvas.plot"),
  collapsed: document.querySelector("#plot-strip").classList.contains("collapsed"),
  h: document.querySelector("#strip-host").clientHeight,
}));
check(strip.inStrip && !strip.collapsed && strip.h > 100, `plot strip shows the plot below the diagram (${JSON.stringify(strip)})`);
await p.waitForSelector("#view-graphic svg .component[data-name='inertia2']");
await p.click("#view-graphic svg .component[data-name='inertia2'] .hit", { force: true });
await p.waitForSelector(".var-menu");
await p.click(".var-menu .vm-row:has(.vm-name:text-is('w')) input");
await p.waitForFunction(() => window.daedalus.plot.subplots[0].series.some((s) => s.name === "inertia2.w"), null, { timeout: 10000 }).catch(() => {});
await p.keyboard.press("Escape");
check((await series(p))[0].names.includes("inertia2.w"), "component click on the Modeling diagram plots into the strip");
await p.evaluate(() => {
  const h = document.querySelector("#view-graphic svg .connection-hit");
  h.dispatchEvent(new MouseEvent("click", { bubbles: true, clientX: 700, clientY: 300 }));
});
await p.waitForSelector(".var-menu");
check((await p.$$eval(".var-menu .vm-tag.flow", (e) => e.length)) > 0, "connection click on the Modeling diagram lists flow variables");
await p.keyboard.press("Escape");
// resize the strip
const rz = await p.$eval("#strip-resize", (e) => {
  const r = e.getBoundingClientRect();
  return { x: r.x + r.width / 2, y: r.y + 2 };
});
await p.mouse.move(rz.x, rz.y);
await p.mouse.down();
await p.mouse.move(rz.x, rz.y - 120, { steps: 5 });
await p.mouse.up();
const h2 = await p.evaluate(() => document.querySelector("#plot-strip").clientHeight);
check(h2 > strip.h + 60, `strip resized (${h2} px)`);
await shot(p, "06-modeling-plot-strip");
// shared state: the Simulation page shows the same curves
await p.evaluate(() => window.daedalus.setMode("simulation"));
check((await p.evaluate(() => !!document.querySelector("#plot canvas.plot"))) && (await series(p))[0].names.includes("inertia2.w"), "Simulation page shows the curves plotted on the Modeling page");
await p.evaluate(() => window.daedalus.setMode("modeling"));
// collapse, remembered after a reload
await p.click("#strip-toggle");
check(await p.evaluate(() => document.querySelector("#plot-strip").classList.contains("collapsed")), "strip collapses");
await p.reload();
await p.waitForFunction(() => window.daedalus?.timings?.interactive, null, { timeout: 300000 });
await p.click("#btn-sim");
await idle(p);
await p.evaluate(() => window.daedalus.setMode("modeling"));
const rem = await p.evaluate(() => ({ collapsed: document.querySelector("#plot-strip").classList.contains("collapsed"), h: document.querySelector("#plot-strip").style.height }));
check(rem.collapsed, `collapsed state remembered after reload (${JSON.stringify(rem)})`);
await p.click("#strip-toggle");
check(Math.abs(parseInt(rem.h) - h2) <= 2, `strip height remembered (${rem.h})`);
// simulating from the Modeling page with the strip open stays there
await p.click("#btn-sim");
await idle(p);
check((await p.evaluate(() => window.daedalus.mode())) === "modeling", "Simulate from the Modeling page with the strip open stays on it");
await shot(p, "07-modeling-strip-after-simulate");

// ------------------------------------------------------------ dashboard
await p.evaluate(() => window.daedalus.setMode("simulation"));
if (await p.evaluate(() => document.querySelector("#dashboard").classList.contains("hidden"))) await p.click("#dash-btn");
const addWidget = async (kind, value, n) => {
  await p.selectOption("#dash-add", kind);
  await p.waitForSelector(".dash-pick", { timeout: 10000 }).catch(async (e) => {
    console.log(await p.evaluate(() => ({ log: document.querySelector("#log").textContent.slice(-300), mode: window.daedalus.mode() })));
    throw e;
  });
  await p.fill(".dash-pick", value);
  await p.click(".dash-ok");
  if (n) await p.waitForFunction((k) => window.daedalus.dashboard.widgets.length === k, n);
};
await addWidget("slider", "amplitude", 1);
await addWidget("gauge", "inertia3.w", 2);
await addWidget("number", "inertia1.phi", 3);
await addWidget("plot", "inertia1.w, inertia3.w", 4);
const widgets = await p.evaluate(() => window.daedalus.dashboard.widgets.map((w) => w.kind));
check(JSON.stringify(widgets) === JSON.stringify(["slider", "gauge", "number", "plot"]), `widgets added (${widgets})`);
// a computed parameter cannot be a slider
await p.selectOption("#dash-add", "slider");
await p.fill(".dash-pick", "inertia3.w");
await p.click(".dash-ok");
check((await p.evaluate(() => window.daedalus.dashboard.widgets.length)) === 4, "a variable that is not a free parameter is refused as a slider");
await p.click(".dash-form button:not(.dash-ok)");
// run live at 0.5x real time (1 s of simulated time takes ~2 s) and move the slider at ~half way
await p.selectOption("#dash-speed", "0.5");
await p.evaluate(() => window.daedalus.plot.clear());
await p.evaluate(() => window.daedalus.togglePlot("inertia1.w"));
const tRun = Date.now();
await p.click("#dash-run");
await p.waitForFunction(() => window.daedalus.live() && window.daedalus.live().t > 0.45, null, { timeout: 30000 });
const tMid = await p.evaluate(() => window.daedalus.live().t);
await p.$eval(".dash-slider", (el) => {
  el.value = "0";
  el.dispatchEvent(new Event("input", { bubbles: true }));
});
await p.waitForFunction(() => /amplitude = 0 applied at t = /.test(document.querySelector("#dash-note").textContent), null, { timeout: 5000 }).catch(async (e) => {
  console.log(await p.evaluate(() => ({ log: document.querySelector("#log").textContent.slice(-400), note: document.querySelector("#dash-note").textContent, live: !!window.daedalus.live(), ov: [...window.daedalus.overrides], sl: [...document.querySelectorAll(".dash-slider")].map((e) => [e.value, e.min, e.max]) })), errors);
  throw e;
});
const changedAt = await p.evaluate(() => +/at t = ([\d.e+-]+)/.exec(document.querySelector("#dash-note").textContent)[1]);
await p.waitForTimeout(250);
await shot(p, "08-dashboard-live-run");
await idle(p);
const wall = (Date.now() - tRun) / 1000;
check(wall > 1.5, `paced at 0.5x: 1 s simulated took ${wall.toFixed(1)} s of wall time`);
check(changedAt >= tMid - 1e-9 && changedAt < 0.9, `slider change applied mid-run at t = ${changedAt} (no restart)`);
const effect = await p.evaluate(async () => {
  const d = window.daedalus;
  const v = await d.api.values(["amplitude", "inertia1.w", "torque.tau"]);
  const t = v.time;
  const i = t.findIndex((x) => x > 0.95);
  return { amp: v.cols[0].at(-1), tau: Math.max(...v.cols[2].slice(i).map(Math.abs)), t0: t[0], n: t.length, gauge: document.querySelector(".dash-gauge .g-val").textContent, num: document.querySelector(".dash-value").textContent };
});
check(effect.amp === 0 && effect.tau < 1e-9, `after the change the driving torque is zero (amplitude ${effect.amp}, |tau| ${effect.tau})`);
const gaugeLim = await p.textContent(".dash-gauge .g-lim");
check(gaugeLim !== "-1 … 1", `gauge range taken from the last result (${gaugeLim})`);
check(effect.t0 === 0 && effect.gauge !== "—" && effect.num !== "—", `one run from t = 0 to the end; gauge ${effect.gauge}, display ${effect.num}`);
const mini = await p.evaluate(() => {
  const c = document.querySelector(".dash-plot");
  const g = c.getContext("2d").getImageData(0, 0, c.width, c.height).data;
  let ink = 0;
  for (let i = 3; i < g.length; i += 4) if (g[i] > 0) ink++;
  return ink;
});
check(mini > 100, `mini plot drawn (${mini} px)`);
await shot(p, "09-dashboard-after-run");
// layout saved per model and restored after a reload
await p.reload();
await p.waitForFunction(() => window.daedalus?.timings?.interactive, null, { timeout: 300000 });
const restored = await p.evaluate(() => window.daedalus.dashboard.widgets.map((w) => w.kind));
check(restored.length === 4, `dashboard layout restored from local storage (${restored})`);
// share link carries the dashboard
await p.evaluate(() => window.daedalus.setMode("simulation"));
if (await p.evaluate(() => document.querySelector("#dashboard").classList.contains("hidden"))) await p.click("#dash-btn");
await p.click("#dash-share");
await p.waitForTimeout(200);
const link = await p.evaluate(() => [...document.querySelectorAll("#log div")].map((d) => d.textContent).reverse().find((t) => t.includes("dash=")) ?? "");
const url = link.slice(link.indexOf("http"));
check(/dash=/.test(url), `share link has the dashboard (${url.slice(0, 120)}…)`);
const p2 = await ctx.newPage();
watch(p2);
await p2.evaluate(() => {}).catch(() => {});
await p2.goto(`${base}#Modelica.Mechanics.Rotational.Examples.First`);
await p2.waitForFunction(() => window.daedalus?.timings?.interactive, null, { timeout: 300000 });
await p2.evaluate(() => localStorage.clear());
await p2.goto(url);
await p2.waitForFunction(() => window.daedalus?.timings?.interactive && window.daedalus.dashboard.widgets.length === 4, null, { timeout: 300000 });
await idle(p2);
check(true, "share link opens the model with its dashboard");
await shot(p2, "10-dashboard-from-share-link");
await p2.close();

// ------------------------------------------------------------ existing features still work
await p.evaluate(() => window.daedalus.setMode("modeling"));
await p.click("button[data-view='structure']");
await p.waitForFunction(() => document.querySelector("#view-structure").textContent.length > 200, null, { timeout: 60000 });
check(true, "Structure view renders");
await shot(p, "11-structure-still-works");
await p.click("button[data-view='diagram']");
await p.evaluate(() => window.daedalus.setMode("analysis"));
check(await p.evaluate(() => !!document.querySelector(".an-plot") && document.querySelector(".an-plot").offsetParent !== null), "Analysis mode shows its panel");
await shot(p, "12-analysis-still-works");
await p.evaluate(() => window.daedalus.setMode("simulation"));
check((await p.evaluate(() => window.daedalus.experiments.runs.length)) > 3, "every simulation is still stored as an experiment run");
// Performance tab: a profiled run takes the plain path
await p.check("#set-profile");
await p.click("#btn-sim");
await idle(p);
await p.waitForSelector("#perf-tab:not(.hidden)");
await p.click("#perf-tab");
await p.waitForSelector("#perf-view:not(.hidden) .perf-sec");
await shot(p, "13-performance-still-works");
check(true, "Performance tab works after a profiled run");
await p.uncheck("#set-profile");
await p.click("button[data-sim='plot']");

// ------------------------------------------------------------ Daedalus model with a parametric plot
await p.evaluate(() => window.daedalus.openClass("Daedalus.Examples.TorqueConverter.Launch"));
await p.click("#btn-sim");
await idle(p);
// The model has 3-D shapes: the animation opens first; back to the plot.
await p.click("button[data-sim='plot']");
await p.evaluate(() => window.daedalus.plot.clear());
await p.fill("#plot-expr", "static.engine.rpm - dynamic.engine.rpm");
await p.press("#plot-expr", "Enter");
await p.waitForFunction(() => window.daedalus.plot.subplots[0].series.length === 1);
await shot(p, "14-launch-expression");
check(true, "expression over a library model");

check(errors.length === 0, `no console errors (${errors.slice(0, 5).join(" | ")})`);
await browser.close();
if (fails.length) {
  console.log(`\n${fails.length} check(s) failed`);
  process.exit(1);
}
console.log("\nall checks passed");
