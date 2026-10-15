// Copies the MSL conformance results (docs/conformance/*.json, written by
// tools/refcompare.py) into public/conformance/ so the scoreboard page can load them.
// Run by `npm run build` and `npm run dev`; fails when the data is missing so a
// build never ships a scoreboard without its data.
import { copyFileSync, mkdirSync, readdirSync, readFileSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const src = resolve(here, "../../docs/conformance");
const dst = resolve(here, "../public/conformance");

let files;
try {
  files = readdirSync(src).filter((f) => f.endsWith(".json"));
} catch (e) {
  console.error(`copy-conformance: cannot read ${src}: ${e.message}`);
  process.exit(1);
}
if (!files.length) {
  console.error(`copy-conformance: no .json files in ${src}`);
  process.exit(1);
}
mkdirSync(dst, { recursive: true });
for (const f of files) {
  const data = JSON.parse(readFileSync(join(src, f), "utf8"));
  if (!Array.isArray(data.results)) {
    console.error(`copy-conformance: ${f} has no results array`);
    process.exit(1);
  }
  copyFileSync(join(src, f), join(dst, f));
  console.log(`copy-conformance: ${f} (${data.results.length} models)`);
}
