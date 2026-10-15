#!/usr/bin/env node
// Packs the Modelica Standard Library into a compressed bundle the workbench loads at
// start-up: public/msl/msl-<version>.<hash>.json.gz plus public/msl/manifest.json.
//
// Usage:
//   node scripts/fetch-msl.mjs                 download the release tarball from GitHub
//   node scripts/fetch-msl.mjs /path/to/msl    use a local checkout (works offline)
//   MSL_DIR=/path/to/msl node scripts/fetch-msl.mjs
//
// Options: --force rebuilds even when the manifest already names this version and the
// bundle exists; MSL_VERSION overrides the release tag (default 4.1.0).
//
// The bundle holds Modelica/, ModelicaServices/ and Complex.mo: every .mo file and
// package.order, as JSON {name, version, files: [[path, text], ...]}, gzip-compressed.
// The content hash in the file name lets the server cache it forever.
import { createHash } from "node:crypto";
import { spawn } from "node:child_process";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const VERSION = process.env.MSL_VERSION || "4.1.0";
const here = path.dirname(fileURLToPath(import.meta.url));
const outDir = path.resolve(here, "..", "public", "msl");
const args = process.argv.slice(2);
const force = args.includes("--force");
const localArg = args.find((a) => !a.startsWith("--"));
const roots = ["Modelica", "ModelicaServices", "Complex.mo"];

function log(msg) {
  console.log(`[fetch-msl] ${msg}`);
}

function upToDate() {
  try {
    const m = JSON.parse(fs.readFileSync(path.join(outDir, "manifest.json"), "utf8"));
    return m.version === VERSION && fs.existsSync(path.join(outDir, m.file));
  } catch {
    return false;
  }
}

async function download() {
  const url = `https://github.com/modelica/ModelicaStandardLibrary/archive/refs/tags/v${VERSION}.tar.gz`;
  const tmp = fs.mkdtempSync(path.join(os.tmpdir(), "msl-"));
  log(`downloading ${url}`);
  const res = await fetch(url);
  if (!res.ok || !res.body) throw new Error(`download failed: HTTP ${res.status}`);
  // Unpack only the library roots; the tarball's top directory is stripped.
  const tar = spawn(
    "tar",
    ["-xz", "-C", tmp, "--strip-components=1", "--wildcards", ...roots.map((r) => `*/${r}`)],
    { stdio: ["pipe", "inherit", "inherit"] },
  );
  const done = new Promise((resolve, reject) => {
    tar.on("error", reject);
    tar.on("close", (code) => (code === 0 ? resolve() : reject(new Error(`tar exited with ${code}`))));
  });
  for await (const chunk of res.body) {
    if (!tar.stdin.write(chunk)) await new Promise((r) => tar.stdin.once("drain", r));
  }
  tar.stdin.end();
  await done;
  return tmp;
}

function collect(dir) {
  const files = [];
  const walk = (rel) => {
    const abs = path.join(dir, rel);
    const st = fs.statSync(abs);
    if (st.isDirectory()) {
      // Only package directories hold library classes (not Resources/).
      if (!fs.existsSync(path.join(abs, "package.mo"))) return;
      for (const e of fs.readdirSync(abs).sort()) walk(rel ? `${rel}/${e}` : e);
    } else if (rel.endsWith(".mo") || rel.endsWith("/package.order")) {
      files.push([rel, fs.readFileSync(abs, "utf8")]);
    }
  };
  for (const r of roots) {
    if (!fs.existsSync(path.join(dir, r))) throw new Error(`${dir} has no ${r}: not an MSL ${VERSION} checkout?`);
    walk(r);
  }
  return files;
}

async function main() {
  if (!force && upToDate()) {
    log(`public/msl already holds MSL ${VERSION} (use --force to rebuild)`);
    return;
  }
  const local = localArg || process.env.MSL_DIR;
  let dir = local ? path.resolve(local) : await download();
  const files = collect(dir);
  const rawBytes = files.reduce((a, [, t]) => a + Buffer.byteLength(t), 0);
  const json = JSON.stringify({ name: "Modelica Standard Library", version: VERSION, license: "BSD-3-Clause", files });
  const gz = gzipSync(Buffer.from(json), { level: 9 });
  const hash = createHash("sha256").update(gz).digest("hex").slice(0, 12);
  const file = `msl-${VERSION}.${hash}.json.gz`;
  fs.mkdirSync(outDir, { recursive: true });
  for (const e of fs.readdirSync(outDir)) if (/^msl-.*\.json\.gz$/.test(e)) fs.rmSync(path.join(outDir, e));
  fs.writeFileSync(path.join(outDir, file), gz);
  const license = path.join(dir, "LICENSE");
  if (fs.existsSync(license)) fs.copyFileSync(license, path.join(outDir, "LICENSE.txt"));
  const manifest = { name: "Modelica Standard Library", version: VERSION, file, bytes: gz.length, raw_bytes: rawBytes, files: files.length };
  fs.writeFileSync(path.join(outDir, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");
  log(`${files.length} files, ${(rawBytes / 1e6).toFixed(1)} MB -> public/msl/${file} (${(gz.length / 1e6).toFixed(2)} MB)`);
  if (!local) fs.rmSync(dir, { recursive: true, force: true });
}

main().catch((e) => {
  console.error(`[fetch-msl] ${e.message}`);
  process.exit(1);
});
