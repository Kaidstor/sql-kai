import { PostgreSQL, sql, type SQLNamespace } from "@codemirror/lang-sql";
import { Prec } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import CodeMirror from "@uiw/react-codemirror";
import {
  AlignLeft,
  CircleStop,
  ClipboardPaste,
  Copy,
  ListFilter,
  Play,
  Route,
  Scissors,
  Split,
  TextSelect,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { format } from "sql-formatter";
import { copyText, readClipboardText } from "../lib/clipboard";
import { searchExtensions } from "../lib/editorSearch";
import { useApp, type QueryTabState, type Tab } from "../lib/store";
import { editorThemes } from "../lib/editorTheme";
import { themeById } from "../lib/themes";
import type { SortSpec, StatementResult } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "./ContextMenu";
import { ExplainView } from "./ExplainView";
import { ExportMenu } from "./ExportMenu";
import { HistoryMenu } from "./HistoryMenu";
import { ResultsGrid } from "./ResultsGrid";
import { SaveQueryButton, SavedQueriesMenu } from "./SavedQueries";
import { SearchPanel, type SearchHandle } from "./SearchPanel";
import { TabError } from "./TabError";
import { Button, cn, MenuButton, Popover, Select } from "./ui";

/** Sorts fetched rows locally — the query is NOT re-run, so only what was
 *  fetched is ordered. Columns where every non-null value parses as a number
 *  compare numerically; NULLs go last on asc, first on desc (Postgres
 *  default). Sort entries for columns missing from the result are dropped. */
function applyClientSort(
  result: StatementResult,
  sorts: SortSpec[],
): { result: StatementResult; sorts: SortSpec[] } {
  const specs = sorts
    .map((s) => ({ ...s, col: result.columns.indexOf(s.column) }))
    .filter((s) => s.col >= 0);
  if (specs.length === 0) return { result, sorts: [] };
  const numeric = specs.map(({ col }) =>
    result.rows.every(
      (r) => r[col] === null || (r[col] !== "" && !Number.isNaN(Number(r[col]))),
    ),
  );
  const rows = [...result.rows].sort((a, b) => {
    for (let i = 0; i < specs.length; i++) {
      const { col, dir } = specs[i];
      const va = a[col];
      const vb = b[col];
      if (va === vb) continue;
      const sign = dir === "desc" ? -1 : 1;
      if (va === null) return sign;
      if (vb === null) return -sign;
      const cmp = numeric[i] ? Number(va) - Number(vb) : va.localeCompare(vb);
      if (cmp !== 0) return sign * cmp;
    }
    return 0;
  });
  return {
    result: { ...result, rows },
    sorts: specs.map(({ column, dir }) => ({ column, dir })),
  };
}

function ResultBlock({
  result,
  index,
  profileId,
  sessionId,
  autoBegin,
  isolatedTabId,
  resultSql,
}: {
  result: StatementResult;
  /** Index of this statement within the run — targets the full export. */
  index: number;
  /** Profile of the connection — session-lost routing on export. */
  profileId: string;
  /** Connection the full export runs on (null = not connected). */
  sessionId: string | null;
  /** Manual-commit tab: the export re-run wraps writes in BEGIN like Run. */
  autoBegin: boolean;
  /** Query tab id when `sessionId` is the tab's isolated session. */
  isolatedTabId?: string;
  /** SQL that produced the run (see QueryTabState.resultSql). */
  resultSql: string | null;
}) {
  const [sorts, setSorts] = useState<SortSpec[]>([]);
  const [filter, setFilter] = useState("");
  // A new run replaces the result object — a needle typed for the previous
  // result must not silently filter the fresh rows ("0 of 500" mystery).
  useEffect(() => setFilter(""), [result]);
  const sorted = useMemo(() => applyClientSort(result, sorts), [result, sorts]);
  // Client filter: keeps rows where any cell contains the needle,
  // case-insensitive; NULL cells never match. Narrows the view only — the
  // query is NOT re-run, so just the fetched rows are searched. One lowercased
  // haystack per row, built once per sort — a keystroke then only scans
  // strings instead of re-lowercasing every cell. \u0000 separates cells:
  // untypeable, so a needle can never straddle a cell boundary.
  const haystacks = useMemo(
    () =>
      sorted.result.rows.map((row) =>
        row.map((v) => v?.toLowerCase() ?? "").join("\u0000"),
      ),
    [sorted.result],
  );
  const shown = useMemo(() => {
    const q = filter.trim().toLowerCase();
    if (!q) return sorted.result;
    return {
      ...sorted.result,
      rows: sorted.result.rows.filter((_, i) => haystacks[i].includes(q)),
    };
  }, [sorted.result, haystacks, filter]);
  const filtering = Boolean(filter.trim());
  if (result.columns.length === 0) {
    return (
      <div className="px-3 py-1.5 text-[12px] text-emerald-400/90 border-b border-zinc-800">
        OK — {result.rowsAffected ?? 0} row(s) affected
      </div>
    );
  }
  return (
    <div className="flex flex-col min-h-0 flex-1">
      <div className="flex items-center px-3 text-[11px] text-zinc-500 border-b border-zinc-800 shrink-0">
        <span className="py-1">
          {filtering ? (
            <span title="Rows are hidden in the view; the query was not re-run">
              {shown.rows.length} of {result.rows.length} row(s)
            </span>
          ) : (
            `${result.rows.length} row(s)`
          )}
          {result.truncated && (
            <span className="text-amber-400"> · truncated to limit</span>
          )}
          {sorted.sorts.length > 0 && (
            <span title="Rows are reordered in the view; the query was not re-run">
              {" "}
              · sorted locally
            </span>
          )}
        </span>
        <span className="ml-auto flex items-center gap-1">
          <ListFilter
            size={11}
            className={filtering ? "text-sky-400" : "text-zinc-600"}
          />
          <input
            value={filter}
            onChange={(e) => setFilter(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                setFilter("");
                e.currentTarget.blur();
              }
            }}
            placeholder="Filter rows…"
            title="Hide fetched rows where no cell contains the text — the query is not re-run"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            className="w-32 bg-transparent py-0.5 font-mono text-[11px] text-zinc-200 outline-none transition-[width] placeholder:text-zinc-700 focus:w-48"
          />
          {filter && (
            <button
              onClick={() => setFilter("")}
              title="Clear filter"
              className="text-zinc-500 hover:text-zinc-200"
            >
              <X size={10} />
            </button>
          )}
        </span>
        <ExportMenu
          className="ml-1 !py-0.5 text-[11px]"
          result={shown}
          profileId={profileId}
          sessionId={sessionId}
          sql={resultSql}
          statementIndex={index}
          autoBegin={autoBegin}
          isolatedTabId={isolatedTabId}
          fileBase="result"
          rerun
        />
      </div>
      <div className="flex-1 min-h-0">
        <ResultsGrid
          result={shown}
          sorts={sorted.sorts}
          onSortsChange={setSorts}
        />
      </div>
    </div>
  );
}

/** Non-empty text of the editor's primary selection, trimmed — undefined when
 *  nothing is selected (or only whitespace), so Run falls back to the whole
 *  editor. A selection may span several statements; that runs fine. */
function selectionText(view: EditorView): string | undefined {
  const { from, to } = view.state.selection.main;
  if (from === to) return undefined;
  const text = view.state.sliceDoc(from, to).trim();
  return text || undefined;
}

/** Editor selection per tab id — the editor unmounts on a tab switch, which
 *  used to silently drop "Run selection"; restored on remount. */
const editorSelections = new Map<string, { anchor: number; head: number }>();

/** Formats the selection (if any) or the whole document as PostgreSQL. */
function formatInView(view: EditorView) {
  const { from, to } = view.state.selection.main;
  const sel = selectionText(view) !== undefined;
  const src = sel ? view.state.sliceDoc(from, to) : view.state.doc.toString();
  if (!src.trim()) return;
  let pretty: string;
  try {
    pretty = format(src, {
      language: "postgresql",
      tabWidth: 2,
      keywordCase: "upper",
    });
  } catch (e) {
    useApp
      .getState()
      .showToast(`Format failed: ${e instanceof Error ? e.message : String(e)}`);
    return;
  }
  const range = sel ? { from, to } : { from: 0, to: view.state.doc.length };
  view.dispatch({
    changes: { ...range, insert: pretty },
    // a formatted selection stays selected, so ⌘⏎ still runs the same SQL
    selection: sel
      ? { anchor: range.from, head: range.from + pretty.length }
      : { anchor: Math.min(view.state.selection.main.head, pretty.length) },
  });
  view.focus();
}

function cutSelection(view: EditorView) {
  const { from, to } = view.state.selection.main;
  if (from === to) return;
  void copyText(view.state.sliceDoc(from, to));
  view.dispatch({ changes: { from, to, insert: "" } });
  view.focus();
}

function copySelection(view: EditorView) {
  const { from, to } = view.state.selection.main;
  if (from === to) return;
  void copyText(view.state.sliceDoc(from, to));
  view.focus();
}

function pasteClipboard(view: EditorView) {
  void readClipboardText().then((text) => {
    if (!text) return;
    const { from, to } = view.state.selection.main;
    view.dispatch({
      changes: { from, to, insert: text },
      selection: { anchor: from + text.length },
    });
    view.focus();
  });
}

function selectAllInView(view: EditorView) {
  view.dispatch({ selection: { anchor: 0, head: view.state.doc.length } });
  view.focus();
}

/** Keep in sync with the connections list height in Sidebar.tsx (max-h-[40%]). */
const DEFAULT_EDITOR_PCT = 40;
/** Minimum pane height while dragging; below half of it the editor snaps shut. */
const MIN_PANE_PX = 100;

/** Stable reference — an inline object would make @uiw/react-codemirror
 *  reconfigure the whole editor on every render (which closes our search
 *  panel). searchKeymap is off: we bind ⌘F ourselves and drive a custom panel. */
const BASIC_SETUP = {
  foldGutter: false,
  autocompletion: true,
  highlightActiveLine: true,
  searchKeymap: false,
} as const;

export function QueryTab({ tab }: { tab: Tab }) {
  const state = tab.state as QueryTabState;
  const runQuery = useApp((s) => s.runQuery);
  const runExplain = useApp((s) => s.runExplain);
  const clearExplain = useApp((s) => s.clearExplain);
  const cancelQuery = useApp((s) => s.cancelQuery);
  const setTabSql = useApp((s) => s.setTabSql);
  const setTabMaxRows = useApp((s) => s.setTabMaxRows);
  const setTabEditorPct = useApp((s) => s.setTabEditorPct);
  const isolateTab = useApp((s) => s.isolateTab);
  const unisolateTab = useApp((s) => s.unisolateTab);
  const setCommitMode = useApp((s) => s.setCommitMode);
  const commitTx = useApp((s) => s.commitTx);
  const rollbackTx = useApp((s) => s.rollbackTx);
  const sessions = useApp((s) => s.sessions);
  const [explainOpen, setExplainOpen] = useState(false);
  const connected = Boolean(sessions[tab.profileId]);
  // The isolated connection backing this tab (if any) — drives the pid/tx chip.
  const isoSession = useApp((s) =>
    state.sessionId ? s.isolatedSessions[state.sessionId] : undefined,
  );
  const commitMode = state.commitMode ?? "auto";
  const txOpen = isoSession?.tx === "active" || isoSession?.tx === "failed";
  const schemaColumns = useApp((s) => s.schemaColumns[tab.profileId]);
  const lightTheme = useApp((s) => Boolean(themeById(s.settings.theme).light));
  const splitRef = useRef<HTMLDivElement>(null);
  const viewRef = useRef<EditorView | null>(null);
  const searchApi = useRef<SearchHandle>(null);
  // Drives the "Run selection" label — mirrors whether the editor selection
  // holds runnable (non-whitespace) text.
  const [hasSelection, setHasSelection] = useState(false);
  const editorPct = state.editorPct ?? DEFAULT_EDITOR_PCT;

  /** Run the selection if there is one, otherwise the whole editor. */
  const run = () => {
    const view = viewRef.current;
    void runQuery(tab.id, view ? selectionText(view) : undefined);
  };

  // Stable so @uiw/react-codemirror doesn't reconfigure the editor every render.
  const handleChange = useCallback(
    (value: string) => setTabSql(tab.id, value),
    [setTabSql, tab.id],
  );

  const resizeTo = (clientY: number) => {
    const split = splitRef.current;
    if (!split) return;
    const rect = split.getBoundingClientRect();
    const y = clientY - rect.top;
    const px =
      y < MIN_PANE_PX / 2
        ? 0 // snap shut — results take the whole tab
        : Math.max(MIN_PANE_PX, Math.min(y, rect.height - MIN_PANE_PX));
    setTabEditorPct(tab.id, (px / rect.height) * 100);
  };

  const extensions = useMemo(() => {
    // {schema: {table: [columns]}} — tables in `public` complete bare,
    // the rest behind their schema prefix; `table.` lists its columns.
    const namespace: { [schema: string]: { [table: string]: string[] } } = {};
    for (const t of schemaColumns ?? []) {
      (namespace[t.schema] ??= {})[t.table] = t.columns;
    }
    return [
      sql({
        dialect: PostgreSQL,
        schema: namespace as SQLNamespace,
        defaultSchema: "public",
      }),
      EditorView.updateListener.of((u) => {
        if (u.selectionSet || u.docChanged) {
          setHasSelection(selectionText(u.view) !== undefined);
          const m = u.view.state.selection.main;
          editorSelections.set(tab.id, { anchor: m.anchor, head: m.head });
        }
      }),
      ...searchExtensions,
      Prec.highest(
        keymap.of([
          {
            key: "Mod-Enter",
            run: (view) => {
              void useApp.getState().runQuery(tab.id, selectionText(view));
              return true;
            },
          },
          {
            key: "Mod-f",
            run: () => {
              searchApi.current?.open(false);
              return true;
            },
          },
          {
            key: "Mod-g",
            run: () => {
              searchApi.current?.findNext();
              return true;
            },
          },
          {
            key: "Mod-Shift-g",
            run: () => {
              searchApi.current?.findPrevious();
              return true;
            },
          },
        ]),
      ),
      Prec.highest(
        EditorView.domEventHandlers({
          // ⇧⌥F / ⌘⌥F can't be keymap entries: on macOS Option-combos arrive as
          // the typed character ("Ï", layout-dependent) and CodeMirror
          // deliberately skips base-key fallback for them — match the physical
          // key (e.code) instead.
          keydown(e, view) {
            if (
              e.code === "KeyF" &&
              e.altKey &&
              e.shiftKey &&
              !e.metaKey &&
              !e.ctrlKey
            ) {
              formatInView(view);
              return true;
            }
            if (
              e.code === "KeyF" &&
              e.altKey &&
              e.metaKey &&
              !e.shiftKey &&
              !e.ctrlKey
            ) {
              searchApi.current?.open(true);
              return true;
            }
            return false;
          },
          // macOS WebKit selects the word under a right-click in editable
          // text, which flips the context menu to "Run/Format selection";
          // suppress it — `contextmenu` still fires, so the menu opens.
          mousedown(e) {
            return e.button === 2;
          },
        }),
      ),
    ];
  }, [tab.id, schemaColumns]);

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center gap-2 px-2 py-1.5 border-b border-zinc-800 shrink-0">
        {state.running ? (
          <Button
            variant="danger"
            title="Ctrl+C"
            onClick={() => void cancelQuery(tab.id)}
          >
            <CircleStop size={13} /> Cancel
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={!connected || !state.sql.trim()}
            onClick={run}
            title={hasSelection ? "Run selected SQL · ⌘⏎" : "⌘⏎"}
          >
            <Play size={13} /> {hasSelection ? "Run selection" : "Run"}
          </Button>
        )}
        <Select
          value={state.maxRows}
          onChange={(e) => setTabMaxRows(tab.id, Number(e.target.value))}
          title="Max rows fetched per statement"
        >
          <option value={100}>100 rows</option>
          <option value={1000}>1 000 rows</option>
          <option value={5000}>5 000 rows</option>
          <option value={50000}>50 000 rows</option>
        </Select>
        <Popover
          open={explainOpen}
          onClose={() => setExplainOpen(false)}
          panelClassName="w-64 p-1"
          trigger={
            <MenuButton
              disabled={!connected || !state.sql.trim() || state.running}
              className="disabled:opacity-40 disabled:pointer-events-none"
              title="Query plan (EXPLAIN)"
              onClick={() => setExplainOpen((v) => !v)}
            >
              <Route size={12} className="text-violet-400/80" />
              Explain
            </MenuButton>
          }
        >
          {(
            [
              {
                label: "Explain — planner estimate",
                hint: "does not run the query",
                analyze: false,
              },
              {
                label: "Explain Analyze",
                hint: "executes the query, real timings",
                analyze: true,
              },
            ] as const
          ).map((item) => (
            <div
              key={item.label}
              onClick={() => {
                setExplainOpen(false);
                const view = viewRef.current;
                void runExplain(
                  tab.id,
                  item.analyze,
                  view ? selectionText(view) : undefined,
                );
              }}
              className="cursor-pointer rounded px-2 py-1.5 hover:bg-zinc-800/60"
            >
              <div className="text-[12px] text-zinc-200">{item.label}</div>
              <div className="text-[10px] text-zinc-500">{item.hint}</div>
            </div>
          ))}
        </Popover>
        <div className="mx-1 h-4 border-l border-zinc-800" />
        <SavedQueriesMenu tab={tab} />
        <SaveQueryButton tab={tab} />
        <HistoryMenu tab={tab} />

        <div className="ml-auto flex items-center gap-1.5">
          {state.isolated && commitMode === "manual" && txOpen && (
            <>
              <Button
                variant="primary"
                className="!px-2 !py-0.5 !bg-emerald-600 hover:!bg-emerald-500"
                onClick={() => void commitTx(tab.id)}
                title="COMMIT the open transaction"
              >
                Commit
              </Button>
              <Button
                variant="danger"
                className="!px-2 !py-0.5"
                onClick={() => void rollbackTx(tab.id)}
                title="ROLLBACK the open transaction"
              >
                Rollback
              </Button>
            </>
          )}

          {state.isolated ? (
            <>
              <div className="flex overflow-hidden rounded border border-zinc-700 text-[10px]">
                {(["auto", "manual"] as const).map((m) => (
                  <button
                    key={m}
                    onClick={() => void setCommitMode(tab.id, m)}
                    className={cn(
                      "px-1.5 py-0.5 capitalize transition-colors",
                      commitMode === m
                        ? "bg-zinc-700 text-zinc-100"
                        : "text-zinc-500 hover:text-zinc-300",
                    )}
                    title={
                      m === "manual"
                        ? "Manual commit: hold one transaction open across runs; end it with Commit/Rollback"
                        : "Auto commit: every run commits immediately"
                    }
                  >
                    {m}
                  </button>
                ))}
              </div>
              <span
                className={cn(
                  "flex items-center gap-1 rounded border px-1.5 py-0.5 text-[10px]",
                  isoSession?.tx === "failed"
                    ? "border-red-500/40 bg-red-500/10 text-red-400"
                    : isoSession?.tx === "active"
                      ? "border-amber-500/40 bg-amber-500/10 text-amber-400"
                      : "border-violet-500/40 bg-violet-500/10 text-violet-300",
                )}
                title="This tab runs on its own connection — own backend pid and transaction"
              >
                <Split size={10} />
                {isoSession?.pid ? `pid ${isoSession.pid}` : "isolated"}
                {isoSession?.tx === "active" && " · in tx"}
                {isoSession?.tx === "failed" && " · aborted"}
                <button
                  onClick={() => void unisolateTab(tab.id)}
                  title="Merge back onto the shared connection (rolls back any open transaction)"
                  className="ml-0.5 hover:text-zinc-100"
                >
                  <X size={10} />
                </button>
              </span>
            </>
          ) : (
            <button
              onClick={() => void isolateTab(tab.id)}
              disabled={!connected}
              className="flex items-center gap-1 rounded px-1.5 py-0.5 text-[11px] text-zinc-400 transition-colors hover:bg-zinc-800 hover:text-zinc-200 disabled:pointer-events-none disabled:opacity-40"
              title="Run this tab on its own connection (own pid & transaction), like a separate psql session"
            >
              <Split size={11} /> Isolate
            </button>
          )}

          <span className="text-[11px] text-zinc-500">
            {state.running
              ? "running…"
              : hasSelection
                ? "⌘⏎ runs selection"
                : state.result
                  ? `${state.result.durationMs} ms`
                  : connected
                    ? "⌘⏎ to run"
                    : "not connected"}
          </span>
        </div>
      </div>

      <div ref={splitRef} className="flex-1 min-h-0 flex flex-col">
        <div
          className={
            editorPct === 0
              ? "hidden"
              : "relative min-h-[100px] shrink-0 overflow-hidden"
          }
          style={{ height: `${editorPct}%` }}
        >
          <ContextMenu>
            <ContextMenuTrigger className="block h-full">
              <CodeMirror
                value={state.sql}
                onChange={handleChange}
                onCreateEditor={(view) => {
                  viewRef.current = view;
                  // an accidental tab hop keeps the selection (see editorSelections)
                  const saved = editorSelections.get(tab.id);
                  const len = view.state.doc.length;
                  if (saved && saved.anchor <= len && saved.head <= len) {
                    view.dispatch({ selection: saved });
                  }
                }}
                extensions={extensions}
                theme={lightTheme ? editorThemes.light : editorThemes.dark}
                height="100%"
                style={{ height: "100%" }}
                placeholder="SELECT * FROM …"
                basicSetup={BASIC_SETUP}
              />
            </ContextMenuTrigger>
            <ContextMenuContent>
              <ContextMenuItem
                icon={Play}
                disabled={!connected || !state.sql.trim() || state.running}
                onClick={run}
              >
                {hasSelection ? "Run selection" : "Run"}
                <ContextMenuShortcut>⌘⏎</ContextMenuShortcut>
              </ContextMenuItem>
              <ContextMenuItem
                icon={AlignLeft}
                disabled={!state.sql.trim()}
                onClick={() => viewRef.current && formatInView(viewRef.current)}
              >
                {hasSelection ? "Format selection" : "Format query"}
                <ContextMenuShortcut>⇧⌥F</ContextMenuShortcut>
              </ContextMenuItem>
              <ContextMenuSeparator />
              <ContextMenuItem
                icon={Scissors}
                disabled={!hasSelection}
                onClick={() => viewRef.current && cutSelection(viewRef.current)}
              >
                Cut
              </ContextMenuItem>
              <ContextMenuItem
                icon={Copy}
                disabled={!hasSelection}
                onClick={() => viewRef.current && copySelection(viewRef.current)}
              >
                Copy
              </ContextMenuItem>
              <ContextMenuItem
                icon={ClipboardPaste}
                onClick={() => viewRef.current && pasteClipboard(viewRef.current)}
              >
                Paste
              </ContextMenuItem>
              <ContextMenuItem
                icon={TextSelect}
                disabled={!state.sql}
                onClick={() => viewRef.current && selectAllInView(viewRef.current)}
              >
                Select all
              </ContextMenuItem>
            </ContextMenuContent>
          </ContextMenu>
          <SearchPanel
            ref={searchApi}
            getView={() => viewRef.current}
            sqlText={state.sql}
          />
        </div>

        <div
          className="group relative h-1.5 shrink-0 cursor-row-resize touch-none select-none"
          title="Drag to resize · double-click to reset"
          onPointerDown={(e) => {
            e.preventDefault();
            e.currentTarget.setPointerCapture(e.pointerId);
          }}
          onPointerMove={(e) => {
            if (!e.currentTarget.hasPointerCapture(e.pointerId)) return;
            resizeTo(e.clientY);
          }}
          onDoubleClick={() => setTabEditorPct(tab.id, DEFAULT_EDITOR_PCT)}
        >
          <div className="absolute inset-x-0 top-1/2 h-px -translate-y-1/2 bg-zinc-800 group-hover:h-0.5 group-hover:bg-sky-600 group-active:h-0.5 group-active:bg-sky-500" />
        </div>

        <div className="flex-1 min-h-0 flex flex-col overflow-auto">
          {state.error && (
            <TabError profileId={tab.profileId} error={state.error} lost={state.connectionLost} />
          )}
          {!state.error && state.explain && (
            <ExplainView
              explain={state.explain}
              onClose={() => clearExplain(tab.id)}
            />
          )}
          {!state.error &&
            !state.explain &&
            state.result?.results.map((result, i) => (
              <ResultBlock
                key={i}
                result={result}
                index={i}
                profileId={tab.profileId}
                // An isolated tab's export must run on ITS connection — no
                // fallback to the shared session: it would export committed
                // state that need not match the (transactional) grid. Dead
                // isolated session ⇒ export disabled until the next Run
                // lazily reopens it.
                sessionId={
                  state.isolated
                    ? (isoSession?.sessionId ?? null)
                    : (sessions[tab.profileId]?.sessionId ?? null)
                }
                autoBegin={Boolean(state.isolated) && commitMode === "manual"}
                isolatedTabId={state.isolated ? tab.id : undefined}
                resultSql={state.resultSql ?? null}
              />
            ))}
          {!state.error && !state.explain && !state.result && !state.running && (
            <div className="flex-1 flex items-center justify-center text-zinc-600 text-[12px]">
              Run a query to see results
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
