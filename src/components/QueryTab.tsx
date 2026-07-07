import { PostgreSQL, sql, type SQLNamespace } from "@codemirror/lang-sql";
import { Prec } from "@codemirror/state";
import { keymap } from "@codemirror/view";
import CodeMirror from "@uiw/react-codemirror";
import { CircleStop, Play } from "lucide-react";
import { useMemo, useRef } from "react";
import { useApp, type QueryTabState, type Tab } from "../lib/store";
import { editorThemes, themeById } from "../lib/themes";
import type { StatementResult } from "../lib/types";
import { HistoryMenu } from "./HistoryMenu";
import { ResultsGrid } from "./ResultsGrid";
import { SaveQueryButton, SavedQueriesMenu } from "./SavedQueries";
import { Button, ErrorPre, Select } from "./ui";

function ResultBlock({ result }: { result: StatementResult }) {
  if (result.columns.length === 0) {
    return (
      <div className="px-3 py-1.5 text-[12px] text-emerald-400/90 border-b border-zinc-800">
        OK — {result.rowsAffected ?? 0} row(s) affected
      </div>
    );
  }
  return (
    <div className="flex flex-col min-h-0 flex-1">
      <div className="px-3 py-1 text-[11px] text-zinc-500 border-b border-zinc-800 shrink-0">
        {result.rows.length} row(s)
        {result.truncated && (
          <span className="text-amber-400"> · truncated to limit</span>
        )}
      </div>
      <div className="flex-1 min-h-0">
        <ResultsGrid result={result} />
      </div>
    </div>
  );
}

const DEFAULT_EDITOR_PCT = 38;
/** Minimum pane height while dragging; below half of it the editor snaps shut. */
const MIN_PANE_PX = 100;

export function QueryTab({ tab }: { tab: Tab }) {
  const state = tab.state as QueryTabState;
  const {
    runQuery,
    cancelQuery,
    setTabSql,
    setTabMaxRows,
    setTabEditorPct,
    sessions,
  } = useApp();
  const connected = Boolean(sessions[tab.profileId]);
  const schemaColumns = useApp((s) => s.schemaColumns[tab.profileId]);
  const lightTheme = useApp((s) => Boolean(themeById(s.settings.theme).light));
  const splitRef = useRef<HTMLDivElement>(null);
  const editorPct = state.editorPct ?? DEFAULT_EDITOR_PCT;

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
      Prec.highest(
        keymap.of([
          {
            key: "Mod-Enter",
            run: () => {
              void useApp.getState().runQuery(tab.id);
              return true;
            },
          },
        ]),
      ),
    ];
  }, [tab.id, schemaColumns]);

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex items-center gap-2 px-2 py-1.5 border-b border-zinc-800 shrink-0">
        {state.running ? (
          <Button variant="danger" onClick={() => void cancelQuery(tab.id)}>
            <CircleStop size={13} /> Cancel
          </Button>
        ) : (
          <Button
            variant="primary"
            disabled={!connected || !state.sql.trim()}
            onClick={() => void runQuery(tab.id)}
            title="⌘⏎"
          >
            <Play size={13} /> Run
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
        <div className="mx-1 h-4 border-l border-zinc-800" />
        <SavedQueriesMenu tab={tab} />
        <SaveQueryButton tab={tab} />
        <HistoryMenu tab={tab} />
        <div className="ml-auto text-[11px] text-zinc-500">
          {state.running
            ? "running…"
            : state.result
              ? `${state.result.durationMs} ms`
              : connected
                ? "⌘⏎ to run"
                : "not connected"}
        </div>
      </div>

      <div ref={splitRef} className="flex-1 min-h-0 flex flex-col">
        <div
          className={
            editorPct === 0
              ? "hidden"
              : "min-h-[100px] shrink-0 overflow-hidden"
          }
          style={{ height: `${editorPct}%` }}
        >
          <CodeMirror
            value={state.sql}
            onChange={(value) => setTabSql(tab.id, value)}
            extensions={extensions}
            theme={lightTheme ? editorThemes.light : editorThemes.dark}
            height="100%"
            style={{ height: "100%" }}
            placeholder="SELECT * FROM …"
            basicSetup={{
              foldGutter: false,
              autocompletion: true,
              highlightActiveLine: true,
            }}
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
          {state.error && <ErrorPre>{state.error}</ErrorPre>}
          {!state.error &&
            state.result?.results.map((result, i) => (
              <ResultBlock key={i} result={result} />
            ))}
          {!state.error && !state.result && !state.running && (
            <div className="flex-1 flex items-center justify-center text-zinc-600 text-[12px]">
              Run a query to see results
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
