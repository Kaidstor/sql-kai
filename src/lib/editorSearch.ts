// Search/replace wiring for the SQL editor. We drive @codemirror/search's
// query state and commands from a custom React panel (SearchPanel.tsx) instead
// of CodeMirror's built-in panel, so the UI matches the rest of the app.
import {
  getSearchQuery,
  search,
  SearchQuery,
  setSearchQuery,
} from "@codemirror/search";
import { RangeSetBuilder } from "@codemirror/state";
import {
  Decoration,
  type DecorationSet,
  EditorView,
  ViewPlugin,
  type ViewUpdate,
} from "@codemirror/view";

export interface SearchOptions {
  caseSensitive: boolean;
  wholeWord: boolean;
  regexp: boolean;
}

export interface Match {
  from: number;
  to: number;
}

const matchMark = Decoration.mark({ class: "cm-searchMatch" });
const selectedMatchMark = Decoration.mark({
  class: "cm-searchMatch cm-searchMatch-selected",
});

/** Highlights every match of the active query. @codemirror/search ships its own
 *  highlighter, but it only paints while the built-in panel is open — we use a
 *  custom panel, so this panel-independent copy keeps the matches visible. */
const matchHighlighter = ViewPlugin.fromClass(
  class {
    decorations: DecorationSet;
    constructor(view: EditorView) {
      this.decorations = buildMatchDeco(view);
    }
    update(u: ViewUpdate) {
      if (
        u.docChanged ||
        u.viewportChanged ||
        u.selectionSet ||
        u.transactions.some((tr) =>
          tr.effects.some((e) => e.is(setSearchQuery)),
        )
      ) {
        this.decorations = buildMatchDeco(u.view);
      }
    }
  },
  { decorations: (v) => v.decorations },
);

function buildMatchDeco(view: EditorView): DecorationSet {
  const query = getSearchQuery(view.state);
  if (!query.search || !query.valid) return Decoration.none;
  const builder = new RangeSetBuilder<Decoration>();
  for (const { from, to } of view.visibleRanges) {
    const cursor = query.getCursor(view.state, from, to);
    for (let next = cursor.next(); !next.done; next = cursor.next()) {
      const m = next.value;
      if (m.from >= m.to) continue;
      const selected = view.state.selection.ranges.some(
        (r) => r.from === m.from && r.to === m.to,
      );
      builder.add(m.from, m.to, selected ? selectedMatchMark : matchMark);
    }
  }
  return builder.finish();
}

/** Add to the editor's extensions to enable the search state + highlighting. */
export const searchExtensions = [search(), matchHighlighter];

export function buildQuery(
  text: string,
  replace: string,
  opts: SearchOptions,
): SearchQuery {
  return new SearchQuery({
    search: text,
    replace,
    caseSensitive: opts.caseSensitive,
    wholeWord: opts.wholeWord,
    regexp: opts.regexp,
  });
}

/** Push the query into the editor so matches highlight and the search commands
 *  (findNext/replaceNext/…) operate on it — no panel required. */
export function applyQuery(view: EditorView, query: SearchQuery) {
  view.dispatch({ effects: setSearchQuery.of(query) });
}

/** All non-empty matches of the query across the whole document, in order. */
export function allMatches(view: EditorView, query: SearchQuery): Match[] {
  if (!query.search || !query.valid) return [];
  const out: Match[] = [];
  const cursor = query.getCursor(view.state, 0, view.state.doc.length);
  for (let next = cursor.next(); !next.done; next = cursor.next()) {
    if (next.value.from < next.value.to)
      out.push({ from: next.value.from, to: next.value.to });
  }
  return out;
}

/** Select a match and scroll it into view. Deliberately does NOT focus the
 *  editor: navigation is usually driven from the search panel's inputs, which
 *  must keep focus (the selection still renders and scrolls while blurred). */
export function selectMatch(view: EditorView, m: Match) {
  view.dispatch({
    selection: { anchor: m.from, head: m.to },
    effects: EditorView.scrollIntoView(m.from, { y: "center" }),
  });
}

/** Clear the highlight (e.g. when the panel closes). */
export function clearQuery(view: EditorView) {
  view.dispatch({ effects: setSearchQuery.of(new SearchQuery({ search: "" })) });
}
