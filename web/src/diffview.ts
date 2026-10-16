// Diff view: a class against another version of it (the version as loaded, or a file
// the user picks): a structural summary, a text diff, and the diagram with added,
// removed, changed and moved components and connections highlighted.

import { api } from "./rpc";
import type { AnnRecord, ClassDiff, ClassInfo } from "./types";
import { DiagramView } from "./diagram";
import { type ClassSource, el, placementMatrix, points } from "./graphics";

export interface DiffHost {
  /** Text of the class's document now (with unsaved edits). */
  currentText(uri: string): Promise<string>;
  /** Text of the document as it was loaded (or last saved). */
  baselineText(uri: string): Promise<string | null>;
  /** Pick a file and return its name and text. */
  pickFile(): Promise<{ name: string; text: string } | null>;
}

type Pane = "summary" | "text" | "diagram";

function esc(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

export class DiffView {
  private pane: Pane = "summary";
  private against: "loaded" | "file" = "loaded";
  private file: { name: string; text: string } | null = null;
  private diff: ClassDiff | null = null;
  private diagram: DiagramView | null = null;
  private body: HTMLElement;
  private status: HTMLElement;
  private info: ClassInfo | null = null;

  constructor(
    host: HTMLElement,
    private src: ClassSource,
    private classes: Map<string, ClassInfo>,
    private dh: DiffHost,
  ) {
    host.innerHTML = `<div class="diff-bar">
        <label>Compare with <select class="diff-against"><option value="loaded">version as loaded</option><option value="file">a file…</option></select></label>
        <span class="seg diff-panes"><button data-pane="summary" class="active">Summary</button><button data-pane="text">Text</button><button data-pane="diagram">Diagram</button></span>
        <span class="diff-status muted"></span>
      </div><div class="diff-body"></div>`;
    this.body = host.querySelector(".diff-body")!;
    this.status = host.querySelector(".diff-status")!;
    const sel = host.querySelector<HTMLSelectElement>(".diff-against")!;
    sel.onchange = async () => {
      if (sel.value === "file") {
        const f = await this.dh.pickFile();
        if (!f) {
          sel.value = this.against;
          return;
        }
        this.file = f;
        this.against = "file";
        sel.options[1].textContent = `file ${f.name}`;
      } else this.against = "loaded";
      if (this.info) await this.show(this.info);
    };
    host.querySelector(".diff-panes")!.addEventListener("click", (e) => {
      const p = (e.target as HTMLElement).getAttribute("data-pane") as Pane | null;
      if (!p) return;
      this.pane = p;
      host.querySelectorAll<HTMLButtonElement>(".diff-panes button").forEach((b) => b.classList.toggle("active", b.dataset.pane === p));
      void this.render();
    });
  }

  /** Diff `info` against the chosen version and show it. */
  async show(info: ClassInfo) {
    this.info = info;
    const uri = info.source?.uri;
    if (!uri) {
      this.body.innerHTML = `<p class="muted">${esc(info.path)} has no source text.</p>`;
      return;
    }
    const now = await this.dh.currentText(uri);
    let old: string | null;
    let label: string;
    if (this.against === "file" && this.file) {
      old = this.file.text;
      label = this.file.name;
    } else {
      old = await this.dh.baselineText(uri);
      label = "the version as loaded";
    }
    if (old === null) {
      this.body.innerHTML = `<p class="muted">No earlier version of ${esc(uri)}.</p>`;
      return;
    }
    try {
      this.diff = await api.diffSources(old, now, info.path, info.path);
    } catch (e) {
      this.diff = null;
      this.body.innerHTML = `<p class="err">${esc(String(e))}</p>`;
      return;
    }
    const d = this.diff;
    const n = d.components.length + d.connections.length + d.equations.length + d.extends.length + d.other.length;
    const lines = d.text.filter((l) => l.op !== " ").length;
    this.status.textContent = n || lines ? `${n} structural changes, ${lines} lines changed against ${label}` : `no differences from ${label}`;
    await this.render();
  }

  private async render() {
    const d = this.diff;
    if (!d) return;
    if (this.pane === "summary") this.renderSummary(d);
    else if (this.pane === "text") this.renderText(d);
    else await this.renderDiagram(d);
  }

  private renderSummary(d: ClassDiff) {
    const badge = (c: string) => `<span class="dbadge ${c}">${c === "added" ? "+" : c === "removed" ? "−" : "~"}</span>`;
    const parts: string[] = [];
    if (d.old_class !== d.new_class) parts.push(`<p>Class renamed: ${esc(d.old_class)} → ${esc(d.new_class)}</p>`);
    if (d.extends.length) parts.push(`<h4>Extends</h4><ul>${d.extends.map(([c, b]) => `<li>${badge(c)} ${esc(b)}</li>`).join("")}</ul>`);
    const comps = d.components.filter((c) => !c.parameter);
    const pars = d.components.filter((c) => c.parameter);
    for (const [title, list] of [["Components", comps], ["Parameters", pars]] as const) {
      if (!list.length) continue;
      parts.push(
        `<h4>${title}</h4><ul>${list
          .map(
            (c) =>
              `<li>${badge(c.change)} <b>${esc(c.name)}</b> <span class="muted">${esc(c.new_type ?? c.old_type ?? "")}</span>${c.moved ? ' <span class="dtag">moved</span>' : ""}` +
              (c.details.length ? `<ul class="ddetails">${c.details.map((x) => `<li>${esc(x)}</li>`).join("")}</ul>` : "") +
              `</li>`,
          )
          .join("")}</ul>`,
      );
    }
    if (d.connections.length)
      parts.push(
        `<h4>Connections</h4><ul>${d.connections.map((c) => `<li>${badge(c.change === "rerouted" ? "modified" : c.change)} connect(${esc(c.from)}, ${esc(c.to)})${c.change === "rerouted" ? ' <span class="dtag">rerouted</span>' : ""}</li>`).join("")}</ul>`,
      );
    if (d.equations.length)
      parts.push(`<h4>Equations</h4><ul class="deqs">${d.equations.map((e) => `<li>${badge(e.change)} ${e.initial ? "<i>initial</i> " : ""}<code>${esc(e.text)}</code></li>`).join("")}</ul>`);
    if (d.other.length) parts.push(`<h4>Other</h4><ul>${d.other.map((o) => `<li>${badge("modified")} ${esc(o)}</li>`).join("")}</ul>`);
    this.body.innerHTML = `<div class="diff-summary">${parts.join("") || "<p class='muted'>No structural differences.</p>"}</div>`;
  }

  private renderText(d: ClassDiff) {
    // changed lines with three lines of context; long unchanged runs fold
    const keep = new Array(d.text.length).fill(false);
    d.text.forEach((l, i) => {
      if (l.op !== " ") for (let k = Math.max(0, i - 3); k <= Math.min(d.text.length - 1, i + 3); k++) keep[k] = true;
    });
    const rows: string[] = [];
    let folded = 0;
    const flush = () => {
      if (folded) rows.push(`<tr class="dfold"><td></td><td></td><td>⋯ ${folded} unchanged lines</td></tr>`);
      folded = 0;
    };
    d.text.forEach((l, i) => {
      if (!keep[i]) {
        folded++;
        return;
      }
      flush();
      const cls = l.op === "+" ? "dadd" : l.op === "-" ? "ddel" : "";
      rows.push(`<tr class="${cls}"><td class="ln">${l.old ?? ""}</td><td class="ln">${l.new ?? ""}</td><td><span class="dop">${l.op}</span>${esc(l.text)}</td></tr>`);
    });
    flush();
    this.body.innerHTML = rows.length > 1 || d.text.some((l) => l.op !== " ") ? `<table class="diff-text">${rows.join("")}</table>` : "<p class='muted'>The texts are identical.</p>";
  }

  private async renderDiagram(d: ClassDiff) {
    this.body.innerHTML = `<div class="diff-legend"><span class="dbadge added">+</span> added <span class="dbadge modified">~</span> changed or moved (dashed: old place) <span class="dbadge removed">−</span> removed</div><div class="diff-diagram"></div>`;
    const hostEl = this.body.querySelector<HTMLElement>(".diff-diagram")!;
    this.diagram = new DiagramView(hostEl, this.src, this.classes);
    const info = this.info!;
    await this.diagram.show(info, "diagram");
    const dv = this.diagram;
    const ov = dv.addOverlay();
    const ghost = (pl: AnnRecord | null, cls: string, label: string) => {
      const tr = (pl?.transformation ?? null) as AnnRecord | null;
      const m = tr ? placementMatrix(tr, [[-100, -100], [100, 100]]) : null;
      if (!m) return;
      const pts = [[-100, -100], [100, -100], [100, 100], [-100, 100]].map(([x, y]) => [m[0] * x + m[2] * y + m[4], m[1] * x + m[3] * y + m[5]]);
      ov.appendChild(el("path", { d: "M" + pts.map((p) => p.join(" ")).join("L") + "Z", class: `dghost ${cls}` }));
      const cx = (pts[0][0] + pts[2][0]) / 2;
      const top = Math.max(...pts.map((p) => p[1]));
      const t = el("text", { x: cx, y: -top - 2, class: `dlabel ${cls}`, transform: "scale(1,-1)", "text-anchor": "middle" });
      t.textContent = label;
      ov.appendChild(t);
    };
    for (const c of d.components) {
      const g = dv.componentElement(c.name);
      if (c.change === "added") g?.classList.add("diff-added");
      else if (c.change === "modified") {
        g?.classList.add("diff-changed");
        if (c.moved) ghost(c.old_placement, "modified", `${c.name} (old place)`);
      } else ghost(c.old_placement, "removed", `${c.name} (removed)`);
    }
    for (const c of d.connections) {
      const e = dv.connectionElement(c.from, c.to);
      if (c.change === "added") e?.classList.add("diff-added");
      else if (c.change === "rerouted") e?.classList.add("diff-changed");
      if (c.change !== "added") {
        const pts = points((c.old_line?.points ?? null) as never);
        if (pts.length >= 2) ov.appendChild(el("path", { d: "M" + pts.map((p) => p.join(" ")).join("L"), class: `dline ${c.change === "removed" ? "removed" : "modified"}` }));
      }
    }
  }
}
