// Documentation view: `Documentation(info=..., revisions=...)` rendered safely, Markdown
// with LaTeX equations (marked + KaTeX, bundled, so it works offline), `modelica://`
// links that navigate and images resolved to library resources, and the
// `__Daedalus_metadata` annotation shown and edited as key/value pairs.

import type { ClassInfo } from "./types";

/** What the view needs from the workbench. */
export interface DocHost {
  /** Open a class (a `modelica://Class.Path` link). */
  open(path: string): void;
  /** A resource file (`modelica://Lib/Resources/x.png`) as an object URL, or null. */
  resourceUrl(cls: string, path: string): Promise<string | null>;
  /** Write new metadata into the class's source. */
  saveMetadata(cls: string, entries: [string, string][]): Promise<void>;
}

const ALLOWED = new Set(
  "a abbr address b bdi bdo big blockquote br caption center cite code col colgroup dd del details dfn div dl dt em figcaption figure font h1 h2 h3 h4 h5 h6 hr i img ins kbd li mark ol p pre q s samp small span strike strong sub summary sup table tbody td tfoot th thead tr tt u ul var".split(" "),
);
/** Removed with their content. */
const DROP = new Set("script style iframe object embed applet form input button textarea select option link meta base frame frameset noscript template svg math audio video source canvas".split(" "));
const ATTRS = new Set("href src alt title width height colspan rowspan align valign border cellpadding cellspacing id name start type color face size summary scope headers dir lang style".split(" "));

/** A URL is kept when it is http(s), mailto, modelica, an anchor or relative. */
function safeUrl(u: string, img: boolean): boolean {
  const t = u.trim().replace(/[\u0000-\u001f\s]+/g, "").toLowerCase();
  if (t.startsWith("javascript:") || t.startsWith("vbscript:")) return false;
  if (t.startsWith("data:")) return img && /^data:image\/(png|gif|jpe?g|webp|bmp);/.test(t);
  const m = /^([a-z][a-z0-9+.-]*):/.exec(t);
  return !m || ["http", "https", "mailto", "modelica"].includes(m[1]);
}

/** Inline styles without `url(...)`, `expression(...)` or positioning that escapes the view. */
function safeStyle(s: string): string {
  return s
    .split(";")
    .filter((d) => d.trim() && !/url\s*\(|expression|javascript:|@import|position\s*:\s*(fixed|absolute)|behavior/i.test(d))
    .join(";");
}

/**
 * Sanitize documentation HTML: an allow-list of tags and attributes; scripts, styles,
 * frames, forms and event handlers are removed; unknown tags are unwrapped.
 */
export function sanitize(html: string): DocumentFragment {
  const doc = new DOMParser().parseFromString(`<!doctype html><body>${html}</body>`, "text/html");
  const out = document.createDocumentFragment();
  const walk = (src: Node, dst: Node) => {
    for (const n of Array.from(src.childNodes)) {
      if (n.nodeType === Node.TEXT_NODE) {
        dst.appendChild(document.createTextNode(n.textContent ?? ""));
      } else if (n.nodeType === Node.ELEMENT_NODE) {
        const e = n as Element;
        const tag = e.tagName.toLowerCase();
        if (DROP.has(tag)) continue;
        if (!ALLOWED.has(tag)) {
          walk(e, dst); // unwrap (html, body, head, unknown tags)
          continue;
        }
        const c = document.createElement(tag);
        for (const a of Array.from(e.attributes)) {
          const name = a.name.toLowerCase();
          if (!ATTRS.has(name)) continue;
          let v = a.value;
          if (name === "href" || name === "src") {
            if (!safeUrl(v, tag === "img")) continue;
          }
          if (name === "style") v = safeStyle(v);
          // images load only once wire() has resolved their source
          c.setAttribute(tag === "img" && name === "src" ? "data-src" : name, v);
        }
        walk(e, c);
        dst.appendChild(c);
      }
    }
  };
  walk(doc.body, out);
  return out;
}

/** Whether documentation text is Markdown rather than HTML. */
export function isMarkdown(s: string): boolean {
  const t = s.trim();
  if (/^<html/i.test(t)) return false;
  // HTML documentation starts with a tag (`<html>`, `<p>`); Markdown may hold inline tags
  return !t.startsWith("<");
}

let mdLibs: Promise<[typeof import("marked"), typeof import("katex")["default"]]> | null = null;
function loadMd() {
  mdLibs ??= Promise.all([import("marked"), import("katex").then((m) => m.default), import("katex/dist/katex.min.css")]).then(
    ([m, k]) => [m, k] as [typeof import("marked"), typeof import("katex")["default"]],
  );
  return mdLibs;
}

/**
 * Markdown with `$inline$` and `$$display$$` LaTeX (also `\(...\)` and `\[...\]`),
 * rendered with marked and KaTeX. The Markdown's HTML is sanitized; formulas are put
 * back afterwards as KaTeX output.
 */
export async function renderMarkdown(md: string): Promise<DocumentFragment> {
  const [marked, katex] = await loadMd();
  const math: { tex: string; display: boolean }[] = [];
  const hold = (tex: string, display: boolean) => {
    math.push({ tex, display });
    return `@@MATH${math.length - 1}@@`;
  };
  // code spans and blocks keep their dollars
  const code: string[] = [];
  let src = md.replace(/```[\s\S]*?```|`[^`\n]*`/g, (m) => {
    code.push(m);
    return `@@CODE${code.length - 1}@@`;
  });
  src = src
    .replace(/\$\$([\s\S]+?)\$\$/g, (_, t) => hold(t, true))
    .replace(/\\\[([\s\S]+?)\\\]/g, (_, t) => hold(t, true))
    .replace(/\\\(([\s\S]+?)\\\)/g, (_, t) => hold(t, false))
    .replace(/(^|[^\\$])\$([^\s$](?:[^$\n]*?[^\s$])?)\$(?!\d)/g, (_, pre, t) => pre + hold(t, false))
    .replace(/@@CODE(\d+)@@/g, (_, i) => code[+i]);
  const html = await marked.parse(src, { gfm: true, async: true });
  const frag = sanitize(html);
  // replace the placeholders in text nodes by KaTeX's output
  const walker = document.createTreeWalker(frag, NodeFilter.SHOW_TEXT);
  const nodes: Text[] = [];
  while (walker.nextNode()) if (/@@MATH\d+@@/.test(walker.currentNode.textContent ?? "")) nodes.push(walker.currentNode as Text);
  for (const t of nodes) {
    const parts = (t.textContent ?? "").split(/(@@MATH\d+@@)/);
    const span = document.createDocumentFragment();
    for (const p of parts) {
      const m = /^@@MATH(\d+)@@$/.exec(p);
      if (!m) {
        if (p) span.appendChild(document.createTextNode(p));
        continue;
      }
      const { tex, display } = math[+m[1]];
      const holder = document.createElement(display ? "div" : "span");
      holder.className = display ? "math display" : "math";
      // KaTeX output is trusted markup (`trust: false` keeps \href and friends out)
      holder.innerHTML = katex.renderToString(tex, { displayMode: display, throwOnError: false, trust: false, output: "htmlAndMathml" });
      span.appendChild(holder);
    }
    t.replaceWith(span);
  }
  return frag;
}

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

/** Make links navigate and images load from library resources. */
async function wire(root: HTMLElement, cls: string, host: DocHost) {
  for (const a of Array.from(root.querySelectorAll<HTMLAnchorElement>("a[href]"))) {
    const href = a.getAttribute("href")!;
    const m = /^modelica:\/\/([^/#?]+)(\/[^#?]*)?/i.exec(href);
    if (m && !m[2]) {
      const target = decodeURIComponent(m[1]).replace(/\.$/, "");
      a.classList.add("class-link");
      a.title = target;
      a.addEventListener("click", (e) => {
        e.preventDefault();
        host.open(target);
      });
    } else if (m) {
      const url = await host.resourceUrl(decodeURIComponent(m[1]), decodeURIComponent(m[2].slice(1)));
      if (url) {
        a.href = url;
        a.target = "_blank";
      } else a.removeAttribute("href");
    } else if (/^https?:/i.test(href)) {
      a.target = "_blank";
      a.rel = "noopener noreferrer";
    }
  }
  for (const img of Array.from(root.querySelectorAll<HTMLImageElement>("img[data-src]"))) {
    const src = img.getAttribute("data-src")!;
    img.removeAttribute("data-src");
    const m = /^modelica:\/\/([^/]+)\/(.*)$/i.exec(src);
    if (m) {
      const url = await host.resourceUrl(decodeURIComponent(m[1]), decodeURIComponent(m[2]));
      if (url) {
        // an online fallback that does not load leaves a note instead of a broken image
        img.onerror = () => img.replaceWith(Object.assign(document.createElement("span"), { className: "img-missing", textContent: `[image ${m[2].split("/").pop()} not available offline]`, title: src }));
        img.src = url;
      } else {
        const ph = document.createElement("span");
        ph.className = "img-missing";
        ph.textContent = `[image ${m[2].split("/").pop()} not available offline]`;
        ph.title = src;
        img.replaceWith(ph);
      }
    } else if (/^(https?:|data:)/i.test(src)) {
      img.src = src;
    } else {
      img.replaceWith(Object.assign(document.createElement("span"), { className: "img-missing", textContent: `[image ${src}]` }));
    }
  }
  void cls;
}

/** Render the documentation view of a class into `el`. */
export async function renderDoc(el: HTMLElement, info: ClassInfo, host: DocHost) {
  el.innerHTML = "";
  const head = document.createElement("div");
  head.className = "doc-head";
  head.innerHTML = `<h2>${escapeHtml(info.path)}</h2><p class="muted">${escapeHtml(info.description)}</p>` +
    (info.version || info.uses.length
      ? `<p class="doc-lib">${info.version ? `Version <b>${escapeHtml(info.version)}</b>` : ""}${info.uses.length ? ` · uses ${info.uses.map(([n, v]) => `${escapeHtml(n)} ${escapeHtml(v)}`).join(", ")}` : ""}</p>`
      : "");
  el.appendChild(head);

  el.appendChild(metadataEditor(info, host));

  const body = document.createElement("div");
  body.className = "doc-body";
  const d = info.documentation;
  if (!d) body.innerHTML = "<p class='muted'>No documentation.</p>";
  else if (isMarkdown(d)) body.appendChild(await renderMarkdown(d));
  else body.appendChild(sanitize(d));
  el.appendChild(body);

  if (info.revisions) {
    const det = document.createElement("details");
    det.className = "doc-revisions";
    det.innerHTML = "<summary>Revisions</summary>";
    const rv = document.createElement("div");
    if (isMarkdown(info.revisions)) rv.appendChild(await renderMarkdown(info.revisions));
    else rv.appendChild(sanitize(info.revisions));
    det.appendChild(rv);
    el.appendChild(det);
  }
  await wire(el, info.path, host);
}

/** Key/value table of `__Daedalus_metadata`, editable in place. */
function metadataEditor(info: ClassInfo, host: DocHost): HTMLElement {
  const box = document.createElement("details");
  box.className = "doc-meta";
  box.open = info.metadata.length > 0;
  const sum = document.createElement("summary");
  sum.textContent = `Metadata${info.metadata.length ? ` (${info.metadata.length})` : ""}`;
  box.appendChild(sum);
  const table = document.createElement("table");
  const rows: [HTMLInputElement, HTMLInputElement][] = [];
  const addRow = (k = "", v = "") => {
    const tr = document.createElement("tr");
    const ki = Object.assign(document.createElement("input"), { value: k, placeholder: "key", className: "meta-key" });
    const vi = Object.assign(document.createElement("input"), { value: v, placeholder: "value", className: "meta-val" });
    const del = Object.assign(document.createElement("button"), { textContent: "×", title: "Remove this entry" });
    const pair: [HTMLInputElement, HTMLInputElement] = [ki, vi];
    del.onclick = () => {
      rows.splice(rows.indexOf(pair), 1);
      tr.remove();
    };
    for (const x of [ki, vi, del]) {
      const td = document.createElement("td");
      td.appendChild(x);
      tr.appendChild(td);
    }
    rows.push(pair);
    table.appendChild(tr);
  };
  for (const [k, v] of info.metadata) addRow(k, v);
  if (!info.metadata.length) addRow();
  const bar = document.createElement("div");
  bar.className = "meta-bar";
  const add = Object.assign(document.createElement("button"), { textContent: "Add entry" });
  add.onclick = () => addRow();
  const save = Object.assign(document.createElement("button"), { textContent: "Save to source", className: "primary" });
  const msg = Object.assign(document.createElement("span"), { className: "muted" });
  msg.textContent = "Stored in the class as annotation(__Daedalus_metadata(key=\"value\", ...))";
  save.onclick = async () => {
    const entries = rows.map(([k, v]) => [k.value.trim(), v.value] as [string, string]).filter(([k]) => k);
    const bad = entries.find(([k]) => !/^[A-Za-z_][A-Za-z0-9_]*$/.test(k));
    if (bad) {
      msg.textContent = `"${bad[0]}" is not a valid key (letters, digits and _)`;
      msg.className = "err";
      return;
    }
    try {
      await host.saveMetadata(info.path, entries);
    } catch (e) {
      msg.textContent = String(e);
      msg.className = "err";
    }
  };
  bar.append(add, save, msg);
  box.append(table, bar);
  return box;
}
