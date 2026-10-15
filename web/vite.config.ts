import { defineConfig, type Plugin } from "vite";
import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

/** `npm run dev` serves the documentation site at /docs/, rendered once at startup. */
function devDocs(): Plugin {
  const out = path.resolve("node_modules/.docs/docs");
  const types: Record<string, string> = { ".html": "text/html", ".css": "text/css", ".webp": "image/webp", ".png": "image/png", ".mp4": "video/mp4" };
  return {
    name: "daedalus-dev-docs",
    apply: "serve",
    configureServer(server) {
      execFileSync(process.execPath, ["scripts/build-docs.mjs", "../docs", out], { stdio: "inherit" });
      server.middlewares.use("/docs", (req, res, next) => {
        let rel = decodeURIComponent((req.url ?? "/").split(/[?#]/)[0]);
        if (rel.endsWith("/")) rel += "index.html";
        const file = path.join(out, rel);
        if (!file.startsWith(out) || !fs.existsSync(file) || !fs.statSync(file).isFile()) return next();
        res.setHeader("Content-Type", types[path.extname(file)] ?? "application/octet-stream");
        fs.createReadStream(file).pipe(res);
      });
    },
  };
}

export default defineConfig({
  base: "./",
  plugins: [devDocs()],
  server: { fs: { allow: [".."] } },
  worker: { format: "es" },
  build: {
    target: "es2022",
    chunkSizeWarningLimit: 4096,
    // Two pages: the workbench and the MSL accuracy scoreboard.
    rollupOptions: { input: { main: "index.html", scoreboard: "scoreboard.html" } },
  },
});
