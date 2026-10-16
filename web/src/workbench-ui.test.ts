import { expect, test } from "vitest";
// The workbench tsconfig has no Node types; these reads stay in the test runner.
// @ts-expect-error node builtin
import { readFileSync } from "node:fs";
// @ts-expect-error node builtin
import { resolve } from "node:path";
import { applyThemeChoice } from "./theme";
import { PRIMARY_VIEWS, applyLogChrome, initialLogChrome, pushLogLine, toggleLogChrome } from "./workbench-ui";

const html = readFileSync(resolve("index.html"), "utf8");
const tokens = readFileSync(resolve("src/tokens.css"), "utf8");
const styles = readFileSync(resolve("src/styles.css"), "utf8");

test("theme choice sets or clears the html data-theme attribute before paint rules match", () => {
  const root = document.documentElement;
  applyThemeChoice(root, "dark");
  expect(root.dataset.theme).toBe("dark");
  expect(root.style.colorScheme).toBe("dark");
  applyThemeChoice(root, "light");
  expect(root.dataset.theme).toBe("light");
  expect(root.style.colorScheme).toBe("light");
  applyThemeChoice(root, "system");
  expect(root.dataset.theme).toBeUndefined();
  expect(root.style.colorScheme).toBe("");
});

test("index.html applies daedalus.theme in the head, before the module script", () => {
  const head = html.slice(0, html.indexOf('<script type="module"'));
  expect(head).toContain('localStorage.getItem("daedalus.theme")');
  expect(head).toContain('theme === "light" || theme === "dark"');
  expect(head).toContain("document.documentElement.dataset.theme");
  expect(head).toContain("document.documentElement.style.colorScheme");
  expect(tokens).toContain('html[data-theme="light"]');
  expect(tokens).toContain('html[data-theme="dark"]');
  expect(tokens).toContain("html:not([data-theme])");
  expect(styles).not.toContain("invert(");
  expect(tokens).not.toContain("invert(");
  expect(styles).not.toContain("hue-rotate");
});

test("Flat model, Structure, Linear, and Diff sit on the class tab strip", () => {
  const doc = new DOMParser().parseFromString(html.replace(/<script[\s\S]*?<\/script>/gi, ""), "text/html");
  const tablist = doc.querySelector("#view-tabs [role=tablist]");
  expect(tablist).toBeTruthy();
  expect(doc.querySelector("#views-menu-btn")).toBeNull();
  expect(doc.querySelector("#views-menu-pop")).toBeNull();
  const buttons = [...tablist!.querySelectorAll<HTMLButtonElement>("button[data-view]")].filter((b) => b.parentElement === tablist);
  expect(buttons.map((b) => b.getAttribute("data-view"))).toEqual([...PRIMARY_VIEWS]);
  for (const button of buttons) expect(button.getAttribute("role")).toBe("tab");
  expect(tablist!.querySelector('button[data-view="linear"]')?.textContent).toBe("Linear");
  // openDebugger still finds Structure by data-view and can activate it directly.
  expect(tablist!.querySelector('button[data-view="structure"]')).toBeTruthy();
});

test("the log stays one line until an error or a click", () => {
  const footer = document.createElement("footer");
  const status = document.createElement("button");
  let state = initialLogChrome();
  applyLogChrome(footer, status, state);
  expect(footer.classList.contains("collapsed")).toBe(true);
  expect(status.getAttribute("aria-expanded")).toBe("false");

  state = pushLogLine(state, "library indexed", "ok");
  applyLogChrome(footer, status, state);
  expect(state.expanded).toBe(false);
  expect(footer.classList.contains("collapsed")).toBe(true);
  expect(status.textContent).toBe("library indexed");
  expect(status.classList.contains("ok")).toBe(true);

  state = pushLogLine(state, "check failed", "err");
  applyLogChrome(footer, status, state);
  expect(state.expanded).toBe(true);
  expect(footer.classList.contains("collapsed")).toBe(false);
  expect(status.classList.contains("err")).toBe(true);
  expect(status.getAttribute("aria-expanded")).toBe("true");

  state = toggleLogChrome(state);
  applyLogChrome(footer, status, state);
  expect(state.expanded).toBe(false);
  expect(footer.classList.contains("collapsed")).toBe(true);

  state = toggleLogChrome(state);
  state = pushLogLine(state, "still open", "warn");
  applyLogChrome(footer, status, state);
  expect(state.expanded).toBe(true);
  expect(status.classList.contains("warn")).toBe(true);
});
