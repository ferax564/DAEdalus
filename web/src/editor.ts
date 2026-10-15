// Modelica text view (CodeMirror 6).
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { StreamLanguage, syntaxHighlighting, HighlightStyle, bracketMatching, foldGutter, indentOnInput } from "@codemirror/language";
import { tags as t } from "@lezer/highlight";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { modelica } from "@codemirror/legacy-modes/mode/modelica";

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
  ".cm-foldPlaceholder": { backgroundColor: "var(--accent-soft)", border: "1px solid var(--line)", color: "var(--muted)" },
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

export class TextView {
  readonly view: EditorView;
  private suppress = false;
  onChange: (text: string) => void = () => {};

  constructor(host: HTMLElement) {
    const ext: Extension[] = [
      lineNumbers(),
      foldGutter(),
      history(),
      drawSelection(),
      indentOnInput(),
      bracketMatching(),
      highlightActiveLine(),
      highlightSelectionMatches(),
      StreamLanguage.define(modelica),
      syntaxHighlighting(modelicaHighlight),
      keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged && !this.suppress) this.onChange(u.state.doc.toString());
      }),
      workbenchTheme,
    ];
    this.view = new EditorView({ state: EditorState.create({ doc: "", extensions: ext }), parent: host });
  }

  setText(text: string, scrollToOffset?: number) {
    this.suppress = true;
    if (this.view.state.doc.toString() !== text) {
      this.view.dispatch({ changes: { from: 0, to: this.view.state.doc.length, insert: text } });
    }
    this.suppress = false;
    if (scrollToOffset !== undefined) {
      const pos = Math.min(scrollToOffset, this.view.state.doc.length);
      this.view.dispatch({ selection: { anchor: pos }, effects: EditorView.scrollIntoView(pos, { y: "start", yMargin: 40 }) });
    }
  }
}
