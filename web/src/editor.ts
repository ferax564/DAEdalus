// Modelica text view (CodeMirror 6).
import { EditorState, type Extension } from "@codemirror/state";
import { EditorView, keymap, lineNumbers, highlightActiveLine, drawSelection } from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { StreamLanguage, syntaxHighlighting, defaultHighlightStyle, bracketMatching, foldGutter, indentOnInput } from "@codemirror/language";
import { searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { modelica } from "@codemirror/legacy-modes/mode/modelica";

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
      syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
      keymap.of([...defaultKeymap, ...historyKeymap, ...searchKeymap, indentWithTab]),
      EditorView.updateListener.of((u) => {
        if (u.docChanged && !this.suppress) this.onChange(u.state.doc.toString());
      }),
      EditorView.theme({ "&": { height: "100%" }, ".cm-scroller": { fontFamily: "var(--mono)", fontSize: "12.5px" } }),
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
