// Modelica text view (CodeMirror 6) with editor intelligence from the compiler's `ide`
// module (via the worker): diagnostics, completion, hover, go to definition (F12 or
// Ctrl/Cmd-click), find usages (Shift+F12), rename (F2), outline (Ctrl+Shift+O) and
// Modelica auto-indent.
import { EditorState, Prec, StateEffect, StateField, type Extension } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection, hoverTooltip, showPanel, type Panel } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { StreamLanguage, syntaxHighlighting, HighlightStyle, bracketMatching, foldGutter, indentOnInput, indentService, type IndentContext } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { autocompletion, completionKeymap, type Completion, type CompletionContext, type CompletionResult } from "@codemirror/autocomplete";
import { linter, lintGutter, type Diagnostic } from "@codemirror/lint";
import { modelica } from "@codemirror/legacy-modes/mode/modelica";
import type { IdeCompletion, IdeDiagnostic, IdeHover, IdeLocation, IdeRename, IdeSymbol } from "./types";

// Colours come from the page's CSS variables, so the editor follows the light/dark theme
// and highlights a selection the same way as the rest of the workbench.
const workbenchTheme = EditorView.theme({
  "&": { height: "100%", color: "var(--fg)", backgroundColor: "var(--panel)" },
  ".cm-scroller": { fontFamily: "var(--mono)", fontSize: "12.5px" },
  ".cm-content": { caretColor: "var(--fg)" },
  ".cm-cursor, .cm-dropCursor": { borderLeftColor: "var(--fg)" },
  "&.cm-focused": { outline: "none" },
  ".cm-selectionBackground, &.cm-focused > .cm-scroller > .cm-selectionLayer .cm-selectionBackground, .cm-content ::selection": {
    backgroundColor: "var(--sel)",
  },
  ".cm-selectionMatch": { backgroundColor: "var(--sel-match)" },
  ".cm-activeLine": { backgroundColor: "var(--active-line)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--active-line)", color: "var(--fg)" },
  ".cm-gutters": { backgroundColor: "var(--panel)", color: "var(--muted)", borderRight: "1px solid var(--line)" },
  "&.cm-focused .cm-matchingBracket": { backgroundColor: "var(--sel-match)", outline: "1px solid var(--line)" },
  "&.cm-focused .cm-nonmatchingBracket": { color: "var(--err)" },
  ".cm-searchMatch": { backgroundColor: "var(--sel-match)", outline: "1px solid var(--accent)" },
  ".cm-searchMatch.cm-searchMatch-selected": { backgroundColor: "var(--sel)" },
  ".cm-panels": { backgroundColor: "var(--bg)", color: "var(--fg)" },
  ".cm-panels.cm-panels-bottom": { borderTop: "1px solid var(--line)" },
  ".cm-panels.cm-panels-top": { borderBottom: "1px solid var(--line)" },
  ".cm-foldPlaceholder": { backgroundColor: "var(--accent-soft)", border: "1px solid var(--line)", color: "var(--muted)" },
  ".cm-tooltip": { backgroundColor: "var(--bg)", color: "var(--fg)", border: "1px solid var(--line)", borderRadius: "4px" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--sel)", color: "var(--fg)" },
  ".cm-completionDetail": { color: "var(--muted)", fontStyle: "normal", marginLeft: "1em" },
  ".cm-completionInfo": { maxWidth: "360px" },
  ".cm-tooltip-lint": { padding: "0" },
  ".cm-diagnostic": { fontFamily: "var(--sans, inherit)", fontSize: "12px" },
});

const modelicaHighlight = HighlightStyle.define([
  { tag: t.keyword, color: "var(--syn-keyword)" },
  { tag: [t.standard(t.variableName), t.typeName], color: "var(--syn-type)" },
  { tag: t.string, color: "var(--syn-string)" },
  { tag: t.number, color: "var(--syn-number)" },
  { tag: t.atom, color: "var(--syn-atom)" },
  { tag: t.comment, color: "var(--syn-comment)", fontStyle: "italic" },
  { tag: t.invalid, color: "var(--err)" },
]);

/** What the editor asks of the workbench: the compiler's answers and navigation. */
export interface IdeHost {
  /** Document shown, or null when the text is not a document (nothing to ask). */
  uri(): string | null;
  update(uri: string, text: string): Promise<IdeDiagnostic[]>;
  definition(uri: string, text: string, pos: number): Promise<IdeLocation[]>;
  references(uri: string, text: string, pos: number): Promise<IdeLocation[]>;
  rename(uri: string, text: string, pos: number, name: string): Promise<IdeRename>;
  completion(uri: string, text: string, pos: number): Promise<IdeCompletion>;
  hover(uri: string, text: string, pos: number): Promise<IdeHover | null>;
  symbols(uri: string, text: string): Promise<IdeSymbol[]>;
  /** Show a location: select it here, or open its class (another document). */
  goto(loc: IdeLocation): void;
  /** Apply rename edits to documents other than the one shown. */
  applyEdits(edits: IdeRename["edits"]): Promise<void>;
  /** A message for the workbench log. */
  log(msg: string, cls?: string): void;
}

// ---------------------------------------------------------------- auto-indent

const OPEN_LINE = /^\s*(?:(?:encapsulated|partial|expandable|operator|pure|impure|redeclare|replaceable|final|inner|outer)\s+)*(?:model|block|connector|record|package|function|class|type|operator)\s+(?:extends\s+)?[\w']+(?!\s*=)/;
const SECTION = /^\s*(?:equation|algorithm|initial\s+equation|initial\s+algorithm|public|protected)\b/;
const END = /^\s*end\b/;
const MIDDLE = /^\s*(?:else|elseif|elsewhen)\b/;

/** Modelica indentation: classes and `if`/`for`/`when`/`while` blocks nest; section
 * keywords sit at the class's level; `else`/`end` close a level. */
function modelicaIndent(cx: IndentContext, pos: number): number | null {
  const unit = cx.unit;
  const doc = cx.state.doc;
  // the code before `pos` on its line (a line being broken at `pos`), else the previous
  // non-blank line
  const line = cx.lineAt(pos, -1);
  let prev = line.text.slice(0, pos - line.from);
  if (!prev.trim()) {
    let n = doc.lineAt(line.from).number - 1;
    while (n >= 1 && !doc.line(n).text.trim()) n--;
    if (n < 1) return 0;
    prev = doc.line(n).text;
  }
  const prevIndent = /^\s*/.exec(prev)![0].replace(/\t/g, " ".repeat(unit)).length;
  const prevCode = prev.replace(/\/\/.*$/, "").replace(/"(?:[^"\\]|\\.)*"/g, '""').trimEnd();
  let ind = prevIndent;
  if (OPEN_LINE.test(prevCode) && !/;\s*$/.test(prevCode)) ind += unit;
  else if (SECTION.test(prevCode)) ind += unit;
  else if (/\b(?:then|loop)\s*$/.test(prevCode) || (MIDDLE.test(prevCode) && !/;\s*$/.test(prevCode))) ind += unit;
  // open parentheses carry over
  let depth = 0;
  for (const c of prevCode) {
    if (c === "(" || c === "{" || c === "[") depth++;
    else if (c === ")" || c === "}" || c === "]") depth--;
  }
  if (depth > 0) ind += unit;
  const cur = cx.textAfterPos(pos);
  if (END.test(cur) || MIDDLE.test(cur) || /^\s*[)}\]]/.test(cur)) ind -= unit;
  if (SECTION.test(cur)) ind -= unit;
  return Math.max(0, ind);
}

// ---------------------------------------------------------------- panels

const setRefs = StateEffect.define<{ title: string; locs: IdeLocation[] } | null>();
const setRename = StateEffect.define<{ pos: number; name: string; error?: string } | null>();
const setOutline = StateEffect.define<IdeSymbol[] | null>();

function escapeHtml(s: string) {
  return s.replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]!);
}

/** The small Markdown the hover answers use: a code block, paragraphs with `code`. */
function renderMarkdown(md: string): HTMLElement {
  const root = document.createElement("div");
  root.className = "cm-ide-hover";
  const parts = md.split(/```(?:modelica)?\n([\s\S]*?)\n```/);
  parts.forEach((p, i) => {
    if (i % 2 === 1) {
      const pre = document.createElement("pre");
      pre.textContent = p;
      root.appendChild(pre);
    } else {
      for (const para of p.split(/\n\s*\n/)) {
        const s = para.trim();
        if (!s) continue;
        const d = document.createElement("p");
        d.innerHTML = escapeHtml(s).replace(/`([^`]+)`/g, "<code>$1</code>").replace(/\n/g, "<br>");
        root.appendChild(d);
      }
    }
  });
  return root;
}

const completionType: Record<string, string> = {
  model: "class", block: "class", connector: "class", "expandable connector": "class", record: "class", class: "class",
  "operator record": "class", package: "namespace", type: "type", function: "function", "operator function": "function",
  operator: "function", variable: "variable", parameter: "constant", constant: "constant", property: "property",
  enum: "enum", keyword: "keyword",
};

export class TextView {
  readonly view: EditorView;
  private suppress = false;
  onChange: (text: string) => void = () => {};
  ide: IdeHost | null = null;

  constructor(host: HTMLElement) {
    const self = this;
    const refsField = StateField.define<{ title: string; locs: IdeLocation[] } | null>({
      create: () => null,
      update(v, tr) {
        for (const e of tr.effects) if (e.is(setRefs)) v = e.value;
        return v;
      },
      provide: (f) => showPanel.from(f, (v) => (v ? (view: EditorView) => self.refsPanel(view, v) : null)),
    });
    const renameField = StateField.define<{ pos: number; name: string; error?: string } | null>({
      create: () => null,
      update(v, tr) {
        for (const e of tr.effects) if (e.is(setRename)) v = e.value;
        return v;
      },
      provide: (f) => showPanel.from(f, (v) => (v ? (view: EditorView) => self.renamePanel(view, v) : null)),
    });
    const outlineField = StateField.define<IdeSymbol[] | null>({
      create: () => null,
      update(v, tr) {
        for (const e of tr.effects) if (e.is(setOutline)) v = e.value;
        return v;
      },
      provide: (f) => showPanel.from(f, (v) => (v ? (view: EditorView) => self.outlinePanel(view, v) : null)),
    });
    const ext: Extension[] = [
      lineNumbers(),
      lintGutter(),
      foldGutter(),
      history(),
      drawSelection(),
      indentOnInput(),
      Prec.high(indentService.of((cx, pos) => modelicaIndent(cx, pos))),
      EditorState.languageData.of(() => [{ indentOnInput: /^\s*(?:end|else|elseif|elsewhen|equation|algorithm|protected|public|[)}\]])$/ }]),
      bracketMatching(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      StreamLanguage.define(modelica),
      syntaxHighlighting(modelicaHighlight),
      refsField,
      renameField,
      outlineField,
      autocompletion({ override: [(cx) => this.complete(cx)], icons: true, activateOnTyping: true }),
      linter((view) => this.lint(view), { delay: 300 }),
      hoverTooltip((view, pos) => this.hover(view, pos), { hoverTime: 350 }),
      keymap.of([
        { key: "F12", run: (v) => (this.gotoDefinition(v, v.state.selection.main.head), true) },
        { key: "Shift-F12", run: (v) => (this.findUsages(v, v.state.selection.main.head), true) },
        { key: "F2", run: (v) => (this.startRename(v), true) },
        { key: "Mod-Shift-o", run: (v) => (this.toggleOutline(v), true), preventDefault: true },
        {
          key: "Escape",
          run: (v) => {
            const st = v.state;
            if (!st.field(refsField) && !st.field(renameField) && !st.field(outlineField)) return false;
            v.dispatch({ effects: [setRefs.of(null), setRename.of(null), setOutline.of(null)] });
            return true;
          },
        },
        ...completionKeymap,
        ...defaultKeymap,
        ...historyKeymap,
        ...searchKeymap,
        indentWithTab,
      ]),
      EditorView.domEventHandlers({
        mousedown: (e, view) => {
          if (!(e.ctrlKey || e.metaKey) || e.button !== 0) return false;
          const pos = view.posAtCoords({ x: e.clientX, y: e.clientY });
          if (pos == null) return false;
          e.preventDefault();
          this.gotoDefinition(view, pos);
          return true;
        },
      }),
      EditorView.updateListener.of((u) => {
        if (u.docChanged && !this.suppress) this.onChange(u.state.doc.toString());
      }),
      workbenchTheme,
    ];
    this.view = new EditorView({ state: EditorState.create({ doc: "", extensions: ext }), parent: host });
  }

  setText(text: string, scrollToOffset?: number, select?: { from: number; to: number }) {
    this.suppress = true;
    if (this.view.state.doc.toString() !== text) {
      this.view.dispatch({ changes: { from: 0, to: this.view.state.doc.length, insert: text } });
    }
    this.suppress = false;
    const len = this.view.state.doc.length;
    if (select) {
      const from = Math.min(select.from, len), to = Math.min(select.to, len);
      this.view.dispatch({ selection: { anchor: from, head: to }, effects: EditorView.scrollIntoView(from, { y: "center" }) });
    } else if (scrollToOffset !== undefined) {
      const pos = Math.min(scrollToOffset, len);
      this.view.dispatch({ selection: { anchor: pos }, effects: EditorView.scrollIntoView(pos, { y: "start", yMargin: 40 }) });
    }
  }

  /** Select a range of the text shown. */
  select(from: number, to: number) {
    const len = this.view.state.doc.length;
    this.view.dispatch({
      selection: { anchor: Math.min(from, len), head: Math.min(to, len) },
      effects: EditorView.scrollIntoView(Math.min(from, len), { y: "center" }),
    });
    this.view.focus();
  }

  // ------------------------------------------------------------ requests

  private ctx(view: EditorView): { uri: string; text: string } | null {
    const uri = this.ide?.uri();
    if (!uri) return null;
    return { uri, text: view.state.doc.toString() };
  }

  private async lint(view: EditorView): Promise<Diagnostic[]> {
    const c = this.ctx(view);
    if (!c) return [];
    const diags = await this.ide!.update(c.uri, c.text);
    // The text may have changed meanwhile; the linter discards stale answers itself.
    const len = view.state.doc.length;
    return diags.map((d) => ({
      from: Math.min(d.from, len),
      to: Math.min(Math.max(d.to, d.from), len),
      severity: d.severity,
      message: d.message,
      source: "daedalus",
    }));
  }

  private async complete(cx: CompletionContext): Promise<CompletionResult | null> {
    const c = this.ctx(cx.view!);
    if (!c) return null;
    const word = cx.matchBefore(/[\w']+$/);
    const before = cx.state.sliceDoc(Math.max(0, cx.pos - 1), cx.pos);
    if (!cx.explicit && !word && before !== "." && before !== "(") return null;
    if (!cx.explicit && before === "(" && !/[\w']\s*\($/.test(cx.state.sliceDoc(Math.max(0, cx.pos - 40), cx.pos))) return null;
    const r = await this.ide!.completion(c.uri, c.text, cx.pos);
    if (cx.aborted) return null;
    if (!r.items.length) return null;
    const options: Completion[] = r.items.map((i) => ({
      label: i.label,
      type: completionType[i.kind] ?? "variable",
      detail: i.detail,
      info: i.info || undefined,
      boost: i.kind === "keyword" ? -10 : i.kind === "property" || i.kind === "variable" || i.kind === "parameter" ? 5 : 0,
    }));
    return { from: r.from, options, validFor: /^[\w']*$/ };
  }

  private async hover(view: EditorView, pos: number) {
    const c = this.ctx(view);
    if (!c) return null;
    const h = await this.ide!.hover(c.uri, c.text, pos);
    if (!h) return null;
    return { pos: h.from, end: h.to, above: true, create: () => ({ dom: renderMarkdown(h.markdown) }) };
  }

  async gotoDefinition(view: EditorView, pos: number) {
    const c = this.ctx(view);
    if (!c) return;
    const locs = await this.ide!.definition(c.uri, c.text, pos);
    if (!locs.length) {
      this.ide!.log("No definition found (builtin, or not in the loaded libraries).");
      return;
    }
    if (locs.length > 1) view.dispatch({ effects: setRefs.of({ title: "Definitions", locs }) });
    this.ide!.goto(locs[0]);
  }

  async findUsages(view: EditorView, pos: number) {
    const c = this.ctx(view);
    if (!c) return;
    const word = view.state.wordAt(pos);
    const name = word ? view.state.sliceDoc(word.from, word.to) : "";
    const locs = await this.ide!.references(c.uri, c.text, pos);
    view.dispatch({ effects: [setRename.of(null), setOutline.of(null), setRefs.of({ title: `Usages of ${name}`, locs })] });
  }

  startRename(view: EditorView) {
    const pos = view.state.selection.main.head;
    const word = view.state.wordAt(pos);
    if (!word) return;
    view.dispatch({ effects: [setRefs.of(null), setOutline.of(null), setRename.of({ pos, name: view.state.sliceDoc(word.from, word.to) })] });
  }

  async toggleOutline(view: EditorView) {
    const c = this.ctx(view);
    if (!c) return;
    if (this.panelOpen("outline")) {
      view.dispatch({ effects: setOutline.of(null) });
      return;
    }
    const syms = await this.ide!.symbols(c.uri, c.text);
    view.dispatch({ effects: [setRefs.of(null), setRename.of(null), setOutline.of(syms)] });
  }

  private async doRename(view: EditorView, pos: number, name: string) {
    const c = this.ctx(view);
    if (!c) return;
    const r = await this.ide!.rename(c.uri, c.text, pos, name);
    if (!r.ok) {
      view.dispatch({ effects: setRename.of({ pos, name, error: r.error ?? "rename failed" }) });
      return;
    }
    const here = r.edits.filter((e) => e.uri === c.uri);
    const other = r.edits.filter((e) => e.uri !== c.uri);
    view.dispatch({
      changes: here.map((e) => ({ from: e.from, to: e.to, insert: e.text })),
      effects: setRename.of(null),
    });
    if (other.length) await this.ide!.applyEdits(other);
    const files = new Set(r.edits.map((e) => e.uri)).size;
    this.ide!.log(`Renamed to ${name}: ${r.edits.length} occurrence${r.edits.length === 1 ? "" : "s"} in ${files} document${files === 1 ? "" : "s"}.`);
    view.focus();
  }

  // ------------------------------------------------------------ panels

  private panelShell(title: string, view: EditorView): { dom: HTMLElement; body: HTMLElement } {
    const dom = document.createElement("div");
    dom.className = "cm-ide-panel";
    const head = document.createElement("div");
    head.className = "cm-ide-panel-head";
    const h = document.createElement("span");
    h.textContent = title;
    const close = document.createElement("button");
    close.textContent = "×";
    close.title = "Close (Esc)";
    close.onclick = () => view.dispatch({ effects: [setRefs.of(null), setRename.of(null), setOutline.of(null)] });
    head.append(h, close);
    const body = document.createElement("div");
    body.className = "cm-ide-panel-body";
    dom.append(head, body);
    return { dom, body };
  }

  private refsPanel(view: EditorView, v: { title: string; locs: IdeLocation[] }): Panel {
    const { dom, body } = this.panelShell(`${v.title} — ${v.locs.length} result${v.locs.length === 1 ? "" : "s"}`, view);
    dom.classList.add("cm-ide-refs");
    let lastUri = "";
    for (const l of v.locs) {
      if (l.uri !== lastUri) {
        const f = document.createElement("div");
        f.className = "cm-ide-file";
        f.textContent = l.uri;
        body.appendChild(f);
        lastUri = l.uri;
      }
      const row = document.createElement("div");
      row.className = "cm-ide-ref";
      row.innerHTML = `<span class="cm-ide-line">${l.line}</span><span class="cm-ide-cls">${escapeHtml(l.class ?? "")}</span><code>${escapeHtml(l.preview)}</code>`;
      row.onclick = () => this.ide?.goto(l);
      body.appendChild(row);
    }
    if (!v.locs.length) body.innerHTML = `<div class="cm-ide-empty">No usages found in the loaded documents.</div>`;
    return { dom, top: false };
  }

  private renamePanel(view: EditorView, v: { pos: number; name: string; error?: string }): Panel {
    const { dom, body } = this.panelShell(`Rename ${v.name}`, view);
    dom.classList.add("cm-ide-rename");
    const input = document.createElement("input");
    input.value = v.name;
    input.spellcheck = false;
    input.onkeydown = (e) => {
      if (e.key === "Enter") {
        e.preventDefault();
        this.doRename(view, v.pos, input.value.trim());
      } else if (e.key === "Escape") {
        e.preventDefault();
        view.dispatch({ effects: setRename.of(null) });
        view.focus();
      }
    };
    const hint = document.createElement("span");
    hint.className = v.error ? "cm-ide-error" : "cm-ide-hint";
    hint.textContent = v.error ?? "Enter renames every usage in the loaded documents · Esc cancels";
    body.append(input, hint);
    return {
      dom,
      top: true,
      mount: () => {
        input.focus();
        input.select();
      },
    };
  }

  private outlinePanel(view: EditorView, syms: IdeSymbol[]): Panel {
    const { dom, body } = this.panelShell("Outline", view);
    dom.classList.add("cm-ide-outline");
    const add = (s: IdeSymbol, depth: number) => {
      const row = document.createElement("div");
      row.className = "cm-ide-sym";
      row.style.paddingLeft = `${8 + depth * 14}px`;
      row.innerHTML = `<span class="cm-ide-kind">${escapeHtml(s.kind)}</span> ${escapeHtml(s.name)}${s.detail ? ` <span class="cm-ide-cls">${escapeHtml(s.detail)}</span>` : ""}`;
      row.onclick = () => this.select(s.selFrom, s.selTo);
      body.appendChild(row);
      for (const c of s.children) add(c, depth + 1);
    };
    for (const s of syms) add(s, 0);
    return { dom, top: false };
  }

  /** Whether a panel of the given kind is open. */
  panelOpen(kind: "refs" | "rename" | "outline"): boolean {
    return !!this.view.dom.querySelector(`.cm-ide-${kind}`);
  }
}
