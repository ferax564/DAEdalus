import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  server: { fs: { allow: [".."] } },
  worker: { format: "es" },
  build: { target: "es2022", chunkSizeWarningLimit: 4096 },
});
