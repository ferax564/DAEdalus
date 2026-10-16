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

function cssBlock(source: string, start: string, end: string): string {
  const i = source.indexOf(start);
  expect(i).toBeGreaterThanOrEqual(0);
  const j = end ? source.indexOf(end, i + start.length) : source.length;
  return source.slice(i, j === -1 ? source.length : j);
}

function hexToken(block: string, name: string): string {
  const m = block.match(new RegExp(`${name}:\\s*(#[0-9a-fA-F]{6})`));
  expect(m, name).toBeTruthy();
  return m![1].toLowerCase();
}

/** Relative luminance of a #rrggbb token, 0 (black) to 1 (white). */
function relLum(hex: string): number {
  const n = parseInt(hex.slice(1), 16);
  const lin = (c: number) => {
    const s = c / 255;
    return s <= 0.04045 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
  };
  const r = lin((n >> 16) & 255);
  const g = lin((n >> 8) & 255);
  const b = lin(n & 255);
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

test("the diagram canvas is a dark sheet in dark theme and gridded in both", () => {
  const light = cssBlock(tokens, ":root", 'html[data-theme="dark"]');
  const dark = cssBlock(tokens, 'html[data-theme="dark"]', "@media");
  const systemDark = cssBlock(tokens, "html:not([data-theme])", "");
  const lightCanvas = hexToken(light, "--canvas");
  const darkCanvas = hexToken(dark, "--canvas");
  const systemCanvas = hexToken(systemDark, "--canvas");
  expect(lightCanvas).toBe("#ffffff");
  expect(relLum(darkCanvas)).toBeLessThan(0.08);
  expect(relLum(systemCanvas)).toBeLessThan(0.08);
  expect(darkCanvas).not.toBe("#ffffff");
  expect(systemCanvas).not.toBe("#ffffff");

  const lightGrid = hexToken(light, "--canvas-grid");
  const darkGrid = hexToken(dark, "--canvas-grid");
  const systemGrid = hexToken(systemDark, "--canvas-grid");
  expect(relLum(lightGrid)).toBeGreaterThan(0.55);
  expect(relLum(lightGrid)).toBeLessThan(relLum(lightCanvas));
  expect(relLum(darkGrid)).toBeGreaterThan(relLum(darkCanvas) + 0.02);
  expect(relLum(systemGrid)).toBeGreaterThan(relLum(systemCanvas) + 0.02);
  expect(relLum(hexToken(dark, "--canvas-ink"))).toBeGreaterThan(0.7);
  expect(relLum(hexToken(light, "--canvas-ink"))).toBeLessThan(0.15);
  expect(relLum(hexToken(systemDark, "--canvas-ink"))).toBeGreaterThan(0.7);

  expect(styles).toContain("background-color: var(--canvas)");
  expect(styles).toContain("var(--canvas-grid)");
  expect(styles).not.toContain("hue-rotate");
  expect(styles).not.toContain("invert(");
  expect(tokens).not.toContain("hue-rotate");
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
