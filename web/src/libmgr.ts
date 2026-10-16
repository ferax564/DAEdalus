// Library management in the workbench: the index of open libraries, installing one from
// GitHub (the source archive when the network allows it, else file by file from
// jsDelivr's GitHub CDN, which sends CORS headers) or from a zip the user picked,
// installed libraries kept in IndexedDB for the next visit, and the versions loaded
// with their `uses` requirements, conflicts and missing libraries.

import { api } from "./rpc";
import type { LibIndex, LibReport } from "./types";

export interface LibHost {
  log(msg: string, cls?: string): void;
  /** The class tree changed. */
  refresh(): Promise<void>;
  /** Resources stored in the worker that the page should know too (meshes, images). */
  pullResources(paths: string[]): Promise<void>;
}

/** An installed library as kept in IndexedDB: the zip it came from. */
interface Stored {
  key: string;
  name: string;
  version: string;
  repo?: string;
  ref?: string;
  packages: string[];
  zip: Blob;
  installed: string;
}

const DB = "daedalus-libraries";

function openDb(): Promise<IDBDatabase> {
  return new Promise((resolve, reject) => {
    const r = indexedDB.open(DB, 1);
    r.onupgradeneeded = () => r.result.createObjectStore("libs", { keyPath: "key" });
    r.onsuccess = () => resolve(r.result);
    r.onerror = () => reject(r.error);
  });
}

async function dbAll(): Promise<Stored[]> {
  try {
    const db = await openDb();
    return await new Promise((resolve, reject) => {
      const q = db.transaction("libs").objectStore("libs").getAll();
      q.onsuccess = () => resolve(q.result as Stored[]);
      q.onerror = () => reject(q.error);
    });
  } catch {
    return [];
  }
}

async function dbPut(s: Stored) {
  try {
    const db = await openDb();
    await new Promise<void>((resolve, reject) => {
      const t = db.transaction("libs", "readwrite");
      t.objectStore("libs").put(s);
      t.oncomplete = () => resolve();
      t.onerror = () => reject(t.error);
    });
  } catch {
    /* storage unavailable: the library lasts for this visit only */
  }
}

async function dbDelete(key: string) {
  try {
    const db = await openDb();
    await new Promise<void>((resolve) => {
      const t = db.transaction("libs", "readwrite");
      t.objectStore("libs").delete(key);
      t.oncomplete = () => resolve();
      t.onerror = () => resolve();
    });
  } catch {
    /* ignore */
  }
}

/** Where each installed library came from, for documentation images not in its zip. */
export const librarySources = new Map<string, { repo: string; ref: string }>();
librarySources.set("Modelica", { repo: "modelica/ModelicaStandardLibrary", ref: "v4.1.0" });
librarySources.set("ModelicaServices", { repo: "modelica/ModelicaStandardLibrary", ref: "v4.1.0" });

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

/** Install a library zip in the worker and remember it. */
async function installZip(host: LibHost, zip: Uint8Array, meta: { packages: string[]; version: string; repo?: string; ref?: string }, persist = true) {
  const got = await api.installLibraryZip(zip, meta.packages, meta.version);
  const ok = got.filter((g) => !g.skipped);
  for (const g of got) {
    if (g.skipped) host.log(`${g.name}: ${g.skipped}; reload the page to replace it`, "warn");
    else host.log(`Installed ${g.name} ${g.version ?? ""}: ${g.files} files, ${g.resources} resources${g.uses?.length ? `; uses ${g.uses.map(([n, v]) => `${n} ${v}`).join(", ")}` : ""}`, "ok");
  }
  if (!ok.length) return;
  if (meta.repo) for (const g of ok) librarySources.set(g.name, { repo: meta.repo, ref: meta.ref ?? "main" });
  if (persist) {
    const name = ok.map((g) => g.name).join("+");
    const version = ok[0].version ?? meta.version;
    await dbPut({
      key: `${name}@${version}`,
      name,
      version,
      repo: meta.repo,
      ref: meta.ref,
      packages: meta.packages,
      zip: new Blob([zip], { type: "application/zip" }),
      installed: new Date().toISOString(),
    });
  }
  await host.refresh();
  await reportConflicts(host);
}

/** Log version conflicts and missing libraries of what is loaded now. */
export async function reportConflicts(host: LibHost) {
  const r = await api.libraryReport();
  for (const c of r.resolution.conflicts) host.log(`version conflict: ${c}`, "warn");
  for (const m of r.resolution.missing) host.log(`${m}: install it from Libraries`, "warn");
}

/** Re-install the libraries kept from earlier visits. */
export async function restoreInstalled(host: LibHost) {
  const all = await dbAll();
  for (const s of all) {
    try {
      const zip = new Uint8Array(await s.zip.arrayBuffer());
      const got = await api.installLibraryZip(zip, s.packages, s.version);
      if (s.repo) for (const g of got) librarySources.set(g.name, { repo: s.repo, ref: s.ref ?? "main" });
      host.log(`${s.name} ${s.version} (installed ${s.installed.slice(0, 10)}) ready`, "ok");
    } catch (e) {
      host.log(`${s.name} ${s.version}: ${e}`, "err");
    }
  }
  if (all.length) await host.refresh();
}

/** Fetch with a time limit. */
async function get(url: string, ms = 20000): Promise<Response> {
  const c = new AbortController();
  const t = setTimeout(() => c.abort(), ms);
  try {
    return await fetch(url, { signal: c.signal });
  } finally {
    clearTimeout(t);
  }
}

/**
 * Download a library: its GitHub source archive when the browser may read it (it
 * usually may not: codeload sends no CORS headers), else its Modelica files one by
 * one from jsDelivr's GitHub CDN. Returns a zip.
 */
async function download(spec: string, progress: (s: string) => void): Promise<{ zip: Uint8Array; packages: string[]; version: string; repo: string; ref: string }> {
  const p = await api.parseSpec(spec);
  const s = p.source;
  const version = s.version ?? s.git_ref.replace(/^v/, "");
  // GitHub's archives (codeload) send no CORS headers a page may use, so the files
  // come from jsDelivr's GitHub CDN, which does.
  progress(`Listing ${s.repo}@${s.git_ref} on jsDelivr…`);
  const lr = await get(p.cdn_listing);
  if (!lr.ok) throw new Error(`jsDelivr cannot list ${s.repo}@${s.git_ref} (HTTP ${lr.status})`);
  const listing = (await lr.json()) as { files: { name: string; size: number }[] };
  const paths = listing.files.map((f) => f.name.replace(/^\//, ""));
  const libs = (await api.findLibraries(paths)).filter(([n]) => !s.packages.length || s.packages.includes(n));
  if (!libs.length) throw new Error(`no Modelica library in ${s.repo}@${s.git_ref}`);
  const want = paths.filter((f) => (f.endsWith(".mo") || f.endsWith("package.order")) && libs.some(([, pre]) => (pre.endsWith(".mo") ? f === pre : f.startsWith(pre))));
  const files: [string, string][] = [];
  let done = 0;
  let failed = 0;
  const queue = [...want];
  const worker = async () => {
    for (;;) {
      const f = queue.shift();
      if (!f) return;
      try {
        const r = await get(`${p.cdn_base}/${f.split("/").map(encodeURIComponent).join("/")}`);
        if (r.ok) files.push([f, await r.text()]);
        else failed++;
      } catch {
        failed++;
      }
      done++;
      if (done % 20 === 0 || done === want.length) progress(`Fetching ${s.repo}@${s.git_ref}: ${done} / ${want.length} files${failed ? `, ${failed} failed` : ""}`);
    }
  };
  await Promise.all(Array.from({ length: 12 }, worker));
  if (failed) throw new Error(`${failed} of ${want.length} files could not be fetched from jsDelivr`);
  return { zip: await api.zipTexts(files), packages: s.packages, version, repo: s.repo, ref: s.git_ref };
}

/** The Libraries dialog. */
export async function openLibraries(host: LibHost) {
  const index = JSON.parse(await api.libraryIndex()) as LibIndex;
  const dlg = document.createElement("dialog");
  dlg.className = "lib-dialog";
  dlg.innerHTML = `<form method="dialog" class="dlg-head"><h3>Libraries</h3><button value="close" title="Close">×</button></form>
    <section class="lib-loaded"><h4>Loaded</h4><div class="lib-report muted">…</div></section>
    <section><h4>Install</h4>
      <div class="lib-row"><input class="lib-spec" placeholder="Name@version from the index, or https://github.com/owner/repo@tag" />
        <button class="lib-go primary">Install</button>
        <label class="file">From .zip…<input class="lib-zip" type="file" accept=".zip" /></label></div>
      <div class="lib-progress-msg muted"></div>
      <table class="lib-index"><thead><tr><th>Library</th><th>Version</th><th></th><th>Description</th></tr></thead><tbody></tbody></table>
      <p class="muted small">Libraries download from GitHub through jsDelivr's CDN. If that is blocked, download the release zip
      from the library's GitHub page and use <b>From .zip…</b>. Installed libraries are kept in this browser.</p>
    </section>
    <section><h4>Installed in this browser</h4><div class="lib-installed muted">…</div></section>`;
  document.body.appendChild(dlg);
  dlg.addEventListener("close", () => dlg.remove());
  const msg = dlg.querySelector<HTMLElement>(".lib-progress-msg")!;
  const say = (s: string, cls = "muted") => {
    msg.textContent = s;
    msg.className = `lib-progress-msg ${cls}`;
  };

  const renderReport = async () => {
    const r: LibReport = await api.libraryReport();
    const rows = r.loaded
      .filter((l) => !l.origin.startsWith("user/"))
      .map((l) => {
        const req = r.requirements.filter((q) => q.by === l.name).map((q) => `${esc(q.name)} ${esc(q.version)}`);
        return `<tr><td><b>${esc(l.name)}</b></td><td>${esc(l.version ?? "–")}</td><td class="muted">${req.length ? `uses ${req.join(", ")}` : ""}</td></tr>`;
      });
    const issues = [
      ...r.resolution.conflicts.map((c) => `<li class="err">Conflict: ${esc(c)}</li>`),
      ...r.resolution.missing.map((m) => `<li class="warn">Missing: ${esc(m)}</li>`),
      ...r.resolution.notes.map((n) => `<li class="muted">${esc(n)}</li>`),
    ];
    dlg.querySelector(".lib-report")!.innerHTML = `<table class="lib-table">${rows.join("")}</table>${issues.length ? `<ul class="lib-issues">${issues.join("")}</ul>` : `<p class="ok small">All uses(...) requirements are satisfied.</p>`}`;
    dlg.querySelector(".lib-report")!.classList.remove("muted");
  };
  const renderInstalled = async () => {
    const all = await dbAll();
    const el = dlg.querySelector<HTMLElement>(".lib-installed")!;
    if (!all.length) {
      el.textContent = "None.";
      return;
    }
    el.classList.remove("muted");
    el.innerHTML = `<table class="lib-table">${all
      .map((s) => `<tr><td><b>${esc(s.name)}</b></td><td>${esc(s.version)}</td><td class="muted">${esc(s.repo ?? "zip")} · ${(s.zip.size / 1e6).toFixed(1)} MB</td><td><button data-key="${esc(s.key)}">Remove</button></td></tr>`)
      .join("")}</table>`;
    el.querySelectorAll<HTMLButtonElement>("button[data-key]").forEach(
      (b) =>
        (b.onclick = async () => {
          await dbDelete(b.dataset.key!);
          say(`Removed ${b.dataset.key}; it stays loaded until the page is reloaded.`);
          await renderInstalled();
        }),
    );
  };

  const tbody = dlg.querySelector("tbody")!;
  for (const l of index.libraries) {
    const tr = document.createElement("tr");
    const vs = [...l.versions].sort((a, b) => (a.version < b.version ? 1 : -1));
    tr.innerHTML = `<td><a href="https://github.com/${esc(l.repo)}" target="_blank" rel="noopener noreferrer">${esc(l.name)}</a></td>
      <td><select>${vs.map((v) => `<option value="${esc(v.version)}">${esc(v.version)}</option>`).join("")}</select></td>
      <td><button>Install</button></td><td class="muted">${esc(l.description)}${l.versions[0]?.uses ? ` <span class="small">(uses ${Object.entries(l.versions[0].uses).map(([k, v]) => `${esc(k)} ${esc(v)}`).join(", ")})</span>` : ""}</td>`;
    const sel = tr.querySelector("select")!;
    tr.querySelector("button")!.onclick = () => install(`${l.name}@${sel.value}`);
    tbody.appendChild(tr);
  }

  const install = async (spec: string) => {
    dlg.querySelectorAll<HTMLButtonElement>("button").forEach((b) => (b.disabled = true));
    try {
      const d = await download(spec, (s) => say(s));
      say(`Installing ${spec}…`);
      await installZip(host, d.zip, d);
      say(`Installed ${spec}.`, "ok");
    } catch (e) {
      say(`${spec}: ${e instanceof Error ? e.message : e}. Download the zip from GitHub and use "From .zip…".`, "err");
    } finally {
      dlg.querySelectorAll<HTMLButtonElement>("button").forEach((b) => (b.disabled = false));
      await renderReport();
      await renderInstalled();
    }
  };
  dlg.querySelector<HTMLButtonElement>(".lib-go")!.onclick = () => {
    const v = dlg.querySelector<HTMLInputElement>(".lib-spec")!.value.trim();
    if (v) void install(v);
  };
  dlg.querySelector<HTMLInputElement>(".lib-zip")!.onchange = async (e) => {
    const f = (e.target as HTMLInputElement).files?.[0];
    if (!f) return;
    say(`Installing ${f.name}…`);
    try {
      // a version in the file name (`Buildings-12.1.0.zip`, `repo-v1.2.zip`) when the package names none
      const m = /(\d+(?:\.\d+)+)/.exec(f.name);
      await installZip(host, new Uint8Array(await f.arrayBuffer()), { packages: [], version: m?.[1] ?? "" });
      say(`Installed ${f.name}.`, "ok");
    } catch (err) {
      say(`${f.name}: ${err instanceof Error ? err.message : err}`, "err");
    }
    await renderReport();
    await renderInstalled();
  };
  dlg.showModal();
  await Promise.all([renderReport(), renderInstalled()]);
}

/** Save the model with the files and resources it needs as a zip. */
export async function savePackage(model: string, host: LibHost) {
  const bytes = await api.exportPackage(model);
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([bytes], { type: "application/zip" }));
  a.download = `${model.split(".").pop()}.zip`;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 2000);
  host.log(`Saved ${a.download} (${(bytes.length / 1e3).toFixed(0)} kB): ${model} with the files and resources it needs`, "ok");
}

/** Open a model package (or a library zip): returns the model to open, if it names one. */
export async function openPackage(file: File, host: LibHost): Promise<string | null> {
  const bytes = new Uint8Array(await file.arrayBuffer());
  const prefix = `pkg/${file.name.replace(/\.zip$/i, "")}/`;
  // The manifest's name sits in the zip's central directory, at the end of the file.
  const tail = new TextDecoder("latin1").decode(bytes.subarray(Math.max(0, bytes.length - (4 << 20))));
  if (!tail.includes("daedalus-package.json")) {
    // a library archive (a GitHub release zip): install it
    await installZip(host, bytes, { packages: [], version: /(\d+(?:\.\d+)+)/.exec(file.name)?.[1] ?? "" });
    return null;
  }
  const r = await api.openPackage(bytes, prefix);
  await host.pullResources(r.resources);
  const m = r.manifest;
  host.log(
    `Opened ${file.name}: ${r.files} Modelica files, ${r.resources.length} resources${m && Object.keys(m.uses).length ? `; uses ${Object.entries(m.uses).map(([k, v]) => `${k} ${v}`).join(", ")}` : ""}`,
    "ok",
  );
  if (r.skipped.length) host.log(`${file.name}: ${r.skipped.join(", ")} already loaded; kept the loaded version`, "warn");
  for (const miss of m?.missing ?? []) host.log(`${file.name}: resource ${miss} was missing when it was saved`, "warn");
  await host.refresh();
  await reportConflicts(host);
  return m?.model ?? null;
}
