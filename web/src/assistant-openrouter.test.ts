import { expect, test } from "vitest";
import { articleText, DEFAULT_MODEL, DOC_PAGES, LUNA_MODELS, redactSecrets, SYSTEM } from "./assistant-openrouter";

test("the model control is the GPT-6 Luna models", () => {
  expect(LUNA_MODELS.map((m) => m.id)).toEqual([
    "openai/gpt-6-luna",
    "openai/gpt-6-luna-pro",
    "openai/gpt-6-luna:batch",
    "openai/gpt-6-luna-pro:batch",
  ]);
  expect(DEFAULT_MODEL).toBe("openai/gpt-6-luna");
  expect(DOC_PAGES.map((p) => p.id)).toContain("examples");
  expect(DOC_PAGES.map((p) => p.id)).toContain("workbench");
});

test("documentation text is the article, with secrets removed", () => {
  const html = `<html><body><aside class="side"><a href="examples.html">Example library</a></aside><article><h1>Examples</h1><p>VehicleEfficiency.NEDCSensitivity takes 108,900 steps and 2.3 s.</p><p>key sk-or-v1-abcdefghijklmnopqrstuvwxyz</p></article></body></html>`;
  const text = redactSecrets(articleText(html));
  expect(text).toContain("VehicleEfficiency.NEDCSensitivity");
  expect(text).toContain("108,900");
  expect(text).not.toContain("sk-or-v1-");
  expect(text).toContain("[redacted]");
});

test("the system prompt is about this workbench and what leaves the machine", () => {
  expect(SYSTEM).toContain("DAEdalus");
  expect(SYSTEM).toContain("Modeling");
  expect(SYSTEM).toContain("Simulation");
  expect(SYSTEM).toContain("Analysis");
  expect(SYSTEM).toContain("OpenRouter");
  expect(SYSTEM).toContain("stays on the machine");
  expect(SYSTEM).toContain("read_documentation");
});
