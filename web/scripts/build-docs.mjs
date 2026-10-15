// Renders the Markdown pages in docs/ to a static documentation site in
// dist/docs/, served by the workbench at /docs/.
//
//   node scripts/build-docs.mjs [docsDir] [outDir]     (defaults: ../docs, dist/docs)
//
// Links between pages (`examples.md#torque-converter`) become links between
// the generated pages, links to other repository files point at GitHub, and the
// build fails on a link to a missing page, image or video.
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { Marked } from "marked";

const here = path.dirname(fileURLToPath(import.meta.url));
const docsDir = path.resolve(process.argv[2] ?? path.join(here, "../../docs"));
const outDir = path.resolve(process.argv[3] ?? path.join(here, "../dist/docs"));
const REPO = "https://github.com/ferax564/DAEdalus/blob/main/";
const APP = "../";
const LIVE = "https://daedalus.apps.ezkeel.com/";

// The sidebar, in reading order. Pages not listed here are not published.
const NAV = [
  ["Introduction", [
    ["index.md", "Overview"],
    ["getting-started.md", "Getting started"],
    ["whats-new.md", "What's new"],
  ]],
  ["Using DAEdalus", [
    ["workbench.md", "Workbench"],
    ["animation.md", "3-D animation"],
    ["cli.md", "Command line"],
    ["scripting.md", "Scripts (.mos) and steady states"],
    ["python.md", "Python API"],
  ]],
  ["Models", [
    ["examples.md", "Example library"],
    ["multibody.md", "Multibody vehicles"],
    ["msl.md", "Modelica Standard Library"],
  ]],
  ["Analysis and export", [
    ["linearize.md", "Linearization"],
    ["fmi.md", "FMI 2.0 export"],
    ["optimal-control.md", "Optimal control, MPC, MHE"],
  ]],
  ["Internals", [
    ["architecture.md", "Architecture"],
    ["roadmap.md", "Roadmap"],
  ]],
];
const pages = NAV.flatMap(([, items]) => items.map(([file, title]) => ({ file, title })));
const published = new Set(pages.map((p) => p.file));

const slugify = (s) =>
  s.toLowerCase().replace(/<[^>]+>/g, "").replace(/&[a-z]+;/g, "").replace(/[^\p{L}\p{N}\s-]/gu, "").trim().replace(/\s+/g, "-");
const escapeHtml = (s) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
const htmlName = (file) => file.replace(/\.md$/, ".html");

const problems = [];
const assets = new Set();

/** Rewrites a link or asset reference found in `page`. */
function rewrite(href, page, kind) {
  // Links into the live workbench stay absolute in the Markdown (so they work on GitHub)
  // and become relative here, so a self-hosted copy opens its own workbench.
  if (href?.startsWith(LIVE)) return APP + href.slice(LIVE.length);
  if (!href || /^(https?:|mailto:|#|data:)/.test(href)) return href;
  const [p, hash] = href.split("#");
  const target = path.normalize(path.join(path.dirname(page), p));
  if (target.startsWith("..")) return REPO + path.normalize(target).replace(/^(\.\.[\/\\])+/, "") + (hash ? `#${hash}` : "");
  if (p.endsWith(".md")) {
    if (published.has(target)) return htmlName(target) + (hash ? `#${hash}` : "");
    if (fs.existsSync(path.join(docsDir, target))) return REPO + "docs/" + target + (hash ? `#${hash}` : "");
    problems.push(`${page}: link to missing page ${href}`);
    return href;
  }
  if (!fs.existsSync(path.join(docsDir, target))) {
    problems.push(`${page}: ${kind} ${href} does not exist`);
    return href;
  }
  if (kind === "link" && !/\.(webp|png|jpe?g|gif|svg|mp4|webm)$/i.test(target)) return REPO + "docs/" + target;
  assets.add(target);
  return target;
}

function render(page, md) {
  const toc = [];
  const marked = new Marked({ gfm: true });
  marked.use({
    renderer: {
      heading({ tokens, depth }) {
        const text = this.parser.parseInline(tokens);
        const id = slugify(text);
        if (depth === 2 || depth === 3) toc.push({ depth, id, text: text.replace(/<[^>]+>/g, "") });
        if (depth === 1) return `<h1>${text}</h1>\n`;
        return `<h${depth} id="${id}"><a class="anchor" href="#${id}" aria-hidden="true">#</a>${text}</h${depth}>\n`;
      },
      link({ href, title, tokens }) {
        const text = this.parser.parseInline(tokens);
        const h = rewrite(href, page, "link");
        const ext = /^https?:/.test(h) && !h.startsWith(REPO) ? ' rel="noopener"' : "";
        return `<a href="${escapeHtml(h)}"${title ? ` title="${escapeHtml(title)}"` : ""}${ext}>${text}</a>`;
      },
      image({ href, title, text }) {
        const h = rewrite(href, page, "image");
        const cap = title ? `<figcaption>${escapeHtml(title)}</figcaption>` : "";
        return `<figure><a href="${escapeHtml(h)}" class="zoom"><img src="${escapeHtml(h)}" alt="${escapeHtml(text)}" loading="lazy"></a>${cap}</figure>`;
      },
      html({ text }) {
        // Raw HTML (videos, card grids): rewrite src, poster and href attributes.
        return text.replace(/\b(src|poster|href)="([^"]+)"/g, (_, a, v) => `${a}="${escapeHtml(rewrite(v, page, a === "href" ? "link" : "asset"))}"`);
      },
      code({ text, lang }) {
        return `<pre><code${lang ? ` class="language-${escapeHtml(lang)}"` : ""}>${escapeHtml(text)}</code></pre>\n`;
      },
    },
  });
  const body = marked.parse(md).replace(/<table>/g, '<div class="table-wrap"><table>').replace(/<\/table>/g, "</table></div>");
  const title = (md.match(/^#\s+(.+)$/m)?.[1] ?? "DAEdalus").replace(/[`*]/g, "");
  return { body, toc, title };
}

function layout(page, { body, toc, title }) {
  const idx = pages.findIndex((p) => p.file === page);
  const prev = pages[idx - 1];
  const next = pages[idx + 1];
  const nav = NAV.map(([group, items]) =>
    `<div class="nav-group"><div class="nav-title">${group}</div>${items
      .map(([f, t]) => `<a href="${htmlName(f)}"${f === page ? ' class="current" aria-current="page"' : ""}>${t}</a>`)
      .join("")}</div>`).join("");
  const onThisPage = toc.length > 1
    ? `<nav class="toc" aria-label="On this page"><div class="nav-title">On this page</div>${toc
      .map((h) => `<a href="#${h.id}" class="d${h.depth}">${escapeHtml(h.text)}</a>`).join("")}</nav>`
    : "";
  const pager = `<nav class="pager">${prev ? `<a class="prev" href="${htmlName(prev.file)}"><span>Previous</span>${prev.title}</a>` : "<span></span>"}${next ? `<a class="next" href="${htmlName(next.file)}"><span>Next</span>${next.title}</a>` : ""}</nav>`;
  const source = `${REPO}docs/${page}`;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>${escapeHtml(title)} · DAEdalus docs</title>
<link rel="icon" href="data:image/svg+xml,%3Csvg xmlns=%27http://www.w3.org/2000/svg%27 viewBox=%270 0 16 16%27%3E%3Crect width=%2716%27 height=%2716%27 rx=%273%27 fill=%27%231f4e79%27/%3E%3Cpath d=%27M3 12 L6 5 L9 10 L13 3%27 stroke=%27white%27 stroke-width=%271.6%27 fill=%27none%27/%3E%3C/svg%3E">
<link rel="stylesheet" href="docs.css">
</head>
<body>
<header class="top">
  <button class="menu" aria-label="Menu" onclick="document.body.classList.toggle('nav-open')">☰</button>
  <a class="brand" href="index.html"><span class="logo">Δ</span> DAEdalus <span class="dim">docs</span></a>
  <span class="grow"></span>
  <a class="btn" href="https://github.com/ferax564/DAEdalus" rel="noopener">GitHub</a>
  <a class="btn primary" href="${APP}">Open the workbench</a>
</header>
<div class="shell">
  <aside class="side">${nav}</aside>
  <main class="content">
    <article>${body}</article>
    ${pager}
    <p class="edit"><a href="${source}" rel="noopener">Edit this page on GitHub</a></p>
  </main>
  ${onThisPage}
</div>
<div class="lightbox" hidden><img alt=""></div>
<script>
// Any figure image opens full size in a lightbox.
for (const img of document.querySelectorAll("article figure img")) {
  img.classList.add("zoom");
  img.addEventListener("click", (e) => {
    e.preventDefault();
    const lb = document.querySelector(".lightbox");
    lb.querySelector("img").src = img.currentSrc || img.src;
    lb.hidden = false;
  });
}
document.querySelector(".lightbox").addEventListener("click", (e) => { e.currentTarget.hidden = true; });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") document.querySelector(".lightbox").hidden = true; });
</script>
</body>
</html>
`;
}

fs.rmSync(outDir, { recursive: true, force: true });
fs.mkdirSync(outDir, { recursive: true });
for (const { file } of pages) {
  const src = path.join(docsDir, file);
  if (!fs.existsSync(src)) {
    problems.push(`navigation lists ${file}, which does not exist`);
    continue;
  }
  fs.writeFileSync(path.join(outDir, htmlName(file)), layout(file, render(file, fs.readFileSync(src, "utf8"))));
}
for (const a of assets) {
  fs.mkdirSync(path.dirname(path.join(outDir, a)), { recursive: true });
  fs.copyFileSync(path.join(docsDir, a), path.join(outDir, a));
}
fs.copyFileSync(path.join(here, "docs.css"), path.join(outDir, "docs.css"));
if (problems.length) {
  console.error(problems.map((p) => `docs: ${p}`).join("\n"));
  process.exit(1);
}
console.log(`docs: ${pages.length} pages and ${assets.size} images or videos in ${path.relative(process.cwd(), outDir) || "."}`);
