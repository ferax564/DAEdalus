import { defineConfig } from "vitest/config";

export default defineConfig({
  test: {
    environment: "happy-dom",
    environmentOptions: {
      happyDOM: { settings: { disableJavaScriptEvaluation: true } },
    },
    include: ["src/**/*.test.ts"],
  },
});
