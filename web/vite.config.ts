import { defineConfig } from "vite";

export default defineConfig({
  base: "./",
  server: { fs: { allow: [".."] } },
  worker: { format: "es" },
  build: {
    target: "es2022",
    chunkSizeWarningLimit: 4096,
    // Two pages: the workbench and the MSL accuracy scoreboard.
    rollupOptions: { input: { main: "index.html", scoreboard: "scoreboard.html" } },
  },
});
