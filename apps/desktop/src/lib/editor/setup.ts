// CodeMirror set up for the app: languages, the app's colours, completion
// from what the step reads, problems as underlines, and the keys people bring
// from RStudio and Jupyter.

import { Compartment, EditorSelection, EditorState, Prec, type Extension } from "@codemirror/state";
import {
  EditorView,
  keymap,
  lineNumbers,
  highlightActiveLine,
  highlightActiveLineGutter,
  highlightSpecialChars,
  drawSelection,
  dropCursor,
  rectangularSelection,
  crosshairCursor,
  placeholder as placeholderText,
} from "@codemirror/view";
import { defaultKeymap, history, historyKeymap, indentWithTab } from "@codemirror/commands";
import { bracketMatching, indentOnInput, syntaxHighlighting, HighlightStyle, StreamLanguage, indentUnit } from "@codemirror/language";
import { autocompletion, closeBrackets, closeBracketsKeymap, completionKeymap, type CompletionContext, type CompletionResult, type CompletionSource } from "@codemirror/autocomplete";
import { search, searchKeymap, highlightSelectionMatches } from "@codemirror/search";
import { lintGutter, type Diagnostic } from "@codemirror/lint";
import { python, pythonLanguage } from "@codemirror/lang-python";
import { sql, PostgreSQL } from "@codemirror/lang-sql";
import { r as rMode } from "@codemirror/legacy-modes/mode/r";
import { tags as t } from "@lezer/highlight";
import type { Problem } from "../api/types";
import { schemaFor, suggest, type EditorContext, type Lang } from "./complete";

import type { Keys, RunRequest } from "./prefs";
export type { Keys, RunRequest } from "./prefs";

const highlight = HighlightStyle.define([
  { tag: [t.keyword, t.controlKeyword, t.operatorKeyword, t.definitionKeyword, t.moduleKeyword], color: "var(--color-running)", fontWeight: "600" },
  { tag: [t.string, t.special(t.string)], color: "var(--color-fresh)" },
  { tag: [t.number, t.bool, t.null, t.atom], color: "var(--color-stale)" },
  { tag: [t.comment, t.lineComment, t.blockComment], color: "var(--color-ink-faint)", fontStyle: "italic" },
  { tag: [t.function(t.variableName), t.function(t.propertyName)], color: "var(--color-ink)", fontWeight: "600" },
  { tag: [t.definition(t.variableName), t.className], color: "var(--color-review)" },
  { tag: [t.operator, t.punctuation, t.bracket], color: "var(--color-ink-soft)" },
  { tag: t.invalid, color: "var(--color-failed)" },
]);

const theme = EditorView.theme({
  "&": {
    fontSize: "0.8125rem",
    backgroundColor: "var(--color-panel)",
    border: "1px solid var(--color-rule-strong)",
    borderRadius: "5px",
    color: "var(--color-ink)",
  },
  "&.cm-focused": { outline: "none", borderColor: "var(--color-running)", boxShadow: "0 0 0 2px var(--color-running-wash)" },
  "&[data-invalid=true]": { borderColor: "var(--color-failed)" },
  ".cm-scroller": { fontFamily: "var(--font-mono)", lineHeight: "1.55", fontVariantLigatures: "none" },
  ".cm-content": { padding: "6px 0", caretColor: "var(--color-ink)" },
  ".cm-gutters": { backgroundColor: "var(--color-paper)", color: "var(--color-ink-faint)", border: "none", borderRight: "1px solid var(--color-rule)", borderRadius: "5px 0 0 5px" },
  ".cm-activeLine": { backgroundColor: "var(--color-active-line)" },
  ".cm-activeLineGutter": { backgroundColor: "var(--color-rule)", color: "var(--color-ink)" },
  "&.cm-focused .cm-selectionBackground, .cm-selectionBackground, ::selection": { backgroundColor: "var(--color-select) !important" },
  ".cm-matchingBracket": { backgroundColor: "var(--color-running-wash)", outline: "1px solid var(--color-running)" },
  ".cm-selectionMatch": { backgroundColor: "var(--color-running-wash)" },
  ".cm-tooltip": { border: "1px solid var(--color-rule-strong)", borderRadius: "5px", backgroundColor: "var(--color-panel)", fontFamily: "var(--font-sans)" },
  ".cm-tooltip-autocomplete > ul > li": { fontFamily: "var(--font-mono)", padding: "2px 8px" },
  ".cm-tooltip-autocomplete > ul > li[aria-selected]": { backgroundColor: "var(--color-select)", color: "var(--color-ink)" },
  ".cm-completionDetail": { color: "var(--color-ink-faint)", fontStyle: "normal", marginLeft: "1em" },
  ".cm-diagnostic-error": { borderLeftColor: "var(--color-failed)" },
  ".cm-diagnostic-warning": { borderLeftColor: "var(--color-stale)" },
  ".cm-lintRange-error": { backgroundImage: "none", textDecoration: "underline wavy var(--color-failed)", textUnderlineOffset: "3px" },
  ".cm-lintRange-warning": { backgroundImage: "none", textDecoration: "underline wavy var(--color-stale)", textUnderlineOffset: "3px" },
  ".cm-placeholder": { color: "var(--color-ink-faint)" },
  ".cm-panels": { backgroundColor: "var(--color-paper)", borderColor: "var(--color-rule)" },
  ".cm-panel.cm-search": { fontFamily: "var(--font-sans)", fontSize: "0.8125rem" },
  ".cm-button": { backgroundImage: "none", backgroundColor: "var(--color-panel)", border: "1px solid var(--color-rule-strong)", borderRadius: "4px" },
  ".cm-textfield": { border: "1px solid var(--color-rule-strong)", borderRadius: "4px" },
});

/** Completion icons as words, not glyphs that need a legend. */
const ICON: Record<string, string> = { column: "property", table: "class", parameter: "constant", function: "function", variable: "variable", method: "method", keyword: "keyword" };

function appSource(lang: Lang, ctx: () => EditorContext): CompletionSource {
  return (c: CompletionContext): CompletionResult | null => {
    const line = c.state.doc.lineAt(c.pos);
    const before = line.text.slice(0, c.pos - line.from);
    const s = suggest(lang, ctx(), before);
    if (!s) return null;
    if (!c.explicit && s.from === before.length && !/["'$@\[.]$/.test(before)) return null;
    return {
      from: line.from + s.from,
      options: s.options.map((o) => ({ label: o.label, type: ICON[o.type], detail: o.detail, apply: o.apply, boost: o.boost })),
      validFor: /^[\w. ]*$/,
    };
  };
}

function language(lang: Lang, ctx: () => EditorContext): Extension {
  const own = appSource(lang, ctx);
  switch (lang) {
    case "python":
      return [python(), pythonLanguage.data.of({ autocomplete: own })];
    case "sql": {
      const s = sql({ dialect: PostgreSQL, schema: schemaFor(ctx()), defaultTable: "input", upperCaseKeywords: true });
      return [s, s.language.data.of({ autocomplete: own })];
    }
    case "r": {
      const lang = StreamLanguage.define(rMode);
      return [lang, lang.data.of({ autocomplete: own }), indentUnit.of("  ")];
    }
    case "formula":
      return [EditorState.languageData.of(() => [{ autocomplete: own }])];
  }
}

/** The selection, or the line under the cursor; with no selection the
 *  cursor then moves to the next line, as in RStudio. */
function takeSelection(view: EditorView): { code: string; fromLine: number } {
  const sel = view.state.selection.main;
  const doc = view.state.doc;
  if (!sel.empty) return { code: view.state.sliceDoc(sel.from, sel.to), fromLine: doc.lineAt(sel.from).number };
  const line = doc.lineAt(sel.head);
  if (line.number < doc.lines) view.dispatch({ selection: { anchor: doc.line(line.number + 1).from }, scrollIntoView: true });
  return { code: line.text, fromLine: line.number };
}

function insert(text: string) {
  return (view: EditorView) => {
    view.dispatch(view.state.changeByRange((r) => ({ changes: { from: r.from, to: r.to, insert: text }, range: EditorSelection.cursor(r.from + text.length) })));
    return true;
  };
}

export function runKeys(lang: Lang, onrun: ((r: RunRequest) => void) | undefined, singleLine: boolean): Extension {
  const keys = [];
  if (onrun) {
    const all = (v: EditorView): RunRequest => ({ what: "all", code: v.state.doc.toString(), fromLine: 1 });
    keys.push(
      { key: "Mod-Enter", run: (v: EditorView) => (onrun({ what: "selection", ...takeSelection(v) }), true) },
      { key: "Mod-Shift-Enter", run: (v: EditorView) => (onrun(all(v)), true) },
      { key: "Shift-Enter", run: (v: EditorView) => (onrun({ ...all(v), what: "step" }), true) },
    );
  }
  if (lang === "r") {
    keys.push({ key: "Alt--", run: insert(" <- ") }, { key: "Mod-Shift-m", run: insert(" |> ") });
  }
  // Formulas are one expression; Enter does not add lines. (Accepting a
  // completion with Enter still works: its keymap has higher precedence.)
  if (singleLine) keys.push({ key: "Enter", run: () => true });
  return keymap.of(keys);
}

function consoleKeys(o: { onenter?: () => void; onhistory?: (step: -1 | 1) => string | null }): Extension {
  const keys = [];
  if (o.onenter) keys.push({ key: "Enter", run: () => (o.onenter!(), true) });
  if (o.onhistory) {
    for (const [key, step] of [["ArrowUp", -1], ["ArrowDown", 1]] as const) {
      keys.push({
        key,
        run: (v: EditorView) => {
          const text = o.onhistory!(step);
          if (text === null) return false;
          v.dispatch({ changes: { from: 0, to: v.state.doc.length, insert: text }, selection: { anchor: text.length } });
          return true;
        },
      });
    }
  }
  // Below completion (highest), so Enter still accepts a suggestion.
  return Prec.high(keymap.of(keys));
}

export interface Options {
  lang: Lang;
  context: () => EditorContext;
  label: string;
  placeholder?: string;
  singleLine?: boolean;
  onrun?: (r: RunRequest) => void;
  onchange: (code: string) => void;
  /** A console's input line: Enter runs it. */
  onenter?: () => void;
  /** A console's input line: Up and Down recall earlier entries. */
  onhistory?: (step: -1 | 1) => string | null;
  keys: Keys;
  keysCompartment: Compartment;
  languageCompartment: Compartment;
}

/** Vim keys are loaded the first time someone turns them on. */
export async function keysExtension(k: Keys): Promise<Extension> {
  if (k !== "vim") return [];
  const { vim } = await import("@replit/codemirror-vim");
  return vim();
}
export { language };

export function extensions(o: Options): Extension[] {
  const multi = !o.singleLine;
  return [
    o.keysCompartment.of([]),
    runKeys(o.lang, o.onrun, !!o.singleLine),
    consoleKeys(o),
    multi ? [lineNumbers(), highlightActiveLineGutter(), highlightActiveLine(), lintGutter()] : [],
    highlightSpecialChars(),
    history(),
    drawSelection(),
    dropCursor(),
    EditorState.allowMultipleSelections.of(true),
    indentOnInput(),
    syntaxHighlighting(highlight),
    bracketMatching(),
    closeBrackets(),
    autocompletion({ icons: false, activateOnTyping: true }),
    rectangularSelection(),
    crosshairCursor(),
    highlightSelectionMatches(),
    search({ top: true }),
    keymap.of([...closeBracketsKeymap, ...defaultKeymap, ...searchKeymap, ...historyKeymap, ...completionKeymap, ...(multi ? [indentWithTab] : [])]),
    o.languageCompartment.of(language(o.lang, o.context)),
    theme,
    o.placeholder ? placeholderText(o.placeholder) : [],
    EditorView.contentAttributes.of({ "aria-label": o.label, spellcheck: "false", autocorrect: "off", autocapitalize: "off" }),
    EditorView.updateListener.of((u) => {
      if (u.docChanged) o.onchange(u.state.doc.toString());
    }),
  ];
}

/** Problems from the linter or the last run, as editor underlines. */
export function diagnostics(doc: EditorState["doc"], problems: Problem[]): Diagnostic[] {
  // Problems without a place are listed under the editor instead.
  return problems.filter((p) => p.line !== null || p.column !== null).map((p) => {
    const lineNo = Math.min(Math.max(p.line ?? 1, 1), doc.lines);
    const line = doc.line(lineNo);
    let from = line.from;
    let to = line.to;
    if (p.column) {
      from = Math.min(line.from + p.column - 1, line.to);
      // Underline the word at the column, or one character.
      const rest = line.text.slice(from - line.from);
      const m = rest.match(/^[\w.$@]+/);
      to = Math.min(from + (m ? m[0].length : 1), line.to);
      if (to === from && from > line.from) from -= 1;
    }
    return {
      from,
      to: Math.max(to, from),
      severity: p.severity === "error" ? "error" : p.severity === "warning" ? "warning" : "info",
      message: p.message,
      source: p.from_last_run ? "last run" : undefined,
    };
  });
}

export { Compartment, EditorState } from "@codemirror/state";
export { EditorView } from "@codemirror/view";
export { setDiagnostics } from "@codemirror/lint";
