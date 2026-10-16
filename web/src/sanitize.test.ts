import { expect, test } from "vitest";
import { safeStyle, sanitize } from "./docview";

function html(fragment: DocumentFragment): HTMLElement {
  const host = document.createElement("div");
  host.appendChild(fragment);
  return host;
}

test("sanitize drops script elements and their text", () => {
  const el = html(sanitize(`<p>ok</p><script>alert(1)</script>`));
  expect(el.querySelector("script")).toBeNull();
  expect(el.textContent).toContain("ok");
  expect(el.textContent).not.toContain("alert");
});

test("sanitize drops event-handler attributes", () => {
  const el = html(sanitize(`<img src="x.png" onerror="alert(1)" alt="a">`));
  const img = el.querySelector("img");
  expect(img?.getAttribute("onerror")).toBeNull();
  expect(img?.getAttribute("alt")).toBe("a");
});

test("sanitize drops javascript: URLs", () => {
  const el = html(sanitize(`<a href="javascript:alert(1)">x</a>`));
  expect(el.querySelector("a")?.getAttribute("href") ?? "").toBe("");
});

test("sanitize drops CSS url()", () => {
  const el = html(sanitize(`<p style="color: red; background: url(https://evil.example/x)">t</p>`));
  const style = el.querySelector("p")?.getAttribute("style") ?? "";
  expect(style).not.toMatch(/url/i);
  expect(style).toMatch(/color/i);
});

test("safeStyle rejects a hex-escaped url the url() filter would miss", () => {
  const bypass = "background:\\75 rl(https://evil.example/x)";
  expect(safeStyle(bypass)).toBe("");
  const el = html(sanitize(`<p style="${bypass}">t</p>`));
  const style = el.querySelector("p")?.getAttribute("style") ?? "";
  expect(style).not.toMatch(/url|\\/i);
  expect(style).not.toContain("75");
});
