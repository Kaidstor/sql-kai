import {
  Check,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  FileCode2,
  Loader2,
  RefreshCw,
  X,
} from "lucide-react";
import { useEffect } from "react";
import { quoteIdent } from "../lib/sql";
import { columnsKey, useApp, type Tab, type TableTabState } from "../lib/store";
import { ResultsGrid } from "./ResultsGrid";
import { Button, ErrorPre, IconBtn, Select } from "./ui";

function formatApprox(n: number): string {
  if (n < 0) return "~?";
  if (n < 10_000) return `~${n}`;
  if (n < 1_000_000) return `~${(n / 1000).toFixed(1)}k`;
  return `~${(n / 1_000_000).toFixed(1)}M`;
}

/** The exact SELECT the backend runs for this grid (see get_table_page). */
function currentViewSql(state: TableTabState): string {
  const rel = `${quoteIdent(state.schema)}.${quoteIdent(state.table)}`;
  const order = state.orderBy
    ? `\nORDER BY ${quoteIdent(state.orderBy)} ${state.orderDir === "desc" ? "DESC" : "ASC"}`
    : "";
  return `SELECT *\nFROM ${rel}${order}\nLIMIT ${state.pageSize} OFFSET ${state.page * state.pageSize}`;
}

export function TableTab({ tab }: { tab: Tab }) {
  const state = tab.state as TableTabState;
  const {
    sessions,
    tables,
    loadTablePage,
    loadTableColumns,
    tableColumns,
    openQueryTab,
    stageCellEdit,
    toggleRowDeletes,
    duplicateRows,
    stageInsertCell,
    removeInsertRow,
    discardEdits,
    applyEdits,
    dismissApplyError,
  } = useApp();
  const connected = Boolean(sessions[tab.profileId]);

  // Lazy load: restored/reopened tabs fetch when first shown, not in bulk
  // at boot (dozens of parallel page queries froze the app).
  useEffect(() => {
    if (connected && !state.data && !state.loading && !state.error) {
      void loadTablePage(tab.id);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected]);

  const cols = tableColumns[columnsKey(tab.profileId, state.schema, state.table)];

  // Column info gives us the primary key needed to build UPDATE/DELETE.
  // Also refetches after runDdl invalidates the cache.
  useEffect(() => {
    if (!cols) void loadTableColumns(tab.profileId, state.schema, state.table);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cols, tab.profileId, state.schema, state.table]);
  const hasPk = (cols ?? []).some((c) => c.isPk);
  // Views (and matviews) are read-only: no INSERT/UPDATE/DELETE through the grid.
  const relKind = (tables[tab.profileId] ?? []).find(
    (t) => t.schema === state.schema && t.name === state.table,
  )?.kind;
  const isView = relKind === "view" || relKind === "matview";
  const disabledReason = isView
    ? `Read-only: ${relKind}s cannot be edited`
    : !cols
      ? "Column info is still loading — try again in a moment"
      : hasPk
        ? undefined
        : "Read-only: table has no primary key";

  const editCount = Object.values(state.edits).reduce(
    (acc, m) => acc + Object.keys(m).length,
    0,
  );
  const dirty = editCount + state.deletes.length + state.inserts.length;

  const onSort = (column: string) => {
    const dir =
      state.orderBy === column && state.orderDir === "asc" ? "desc" : "asc";
    void loadTablePage(tab.id, { orderBy: column, orderDir: dir, page: 0 });
  };

  const rows = state.data?.result.rows.length ?? 0;
  const lastPage = rows < state.pageSize;

  const columnTypes = state.data?.result.columns.map(
    (name) => cols?.find((c) => c.name === name)?.dataType,
  );

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex min-h-10 items-center gap-1.5 px-2 py-1.5 border-b border-zinc-800 shrink-0 text-[12px]">
        <span className="font-mono text-zinc-300">
          {state.schema}.{state.table}
        </span>
        <IconBtn
          title="Refresh (⌘R)"
          onClick={() => void loadTablePage(tab.id)}
          disabled={state.loading}
        >
          {state.loading ? (
            <Loader2 size={13} className="animate-spin" />
          ) : (
            <RefreshCw size={13} />
          )}
        </IconBtn>
        <IconBtn
          title="Current view as query — open this grid's SQL in a new tab"
          onClick={() =>
            openQueryTab(tab.profileId, currentViewSql(state), state.table)
          }
        >
          <FileCode2 size={13} />
        </IconBtn>

        {dirty > 0 && (
          <div className="flex items-center gap-1.5 pl-1">
            <Button
              variant="primary"
              disabled={state.loading}
              onClick={() => void applyEdits(tab.id)}
              title="⌘S — runs INSERT/UPDATE/DELETE in one transaction"
            >
              <Check size={13} /> Apply {dirty}
            </Button>
            <Button title="Esc" onClick={() => discardEdits(tab.id)}>
              Discard
            </Button>
          </div>
        )}
        {isView ? (
          <span className="pl-1 text-[11px] text-zinc-600">
            read-only · {relKind}
          </span>
        ) : (
          cols &&
          !hasPk && (
            <span className="pl-1 text-[11px] text-zinc-600">
              read-only · no primary key
            </span>
          )
        )}

        <div className="ml-auto flex items-center gap-1.5">
          <span className="text-zinc-500">
            {formatApprox(state.data?.approxRows ?? -1)} rows
            {state.data ? ` · ${state.data.durationMs} ms` : ""}
          </span>
          <Select
            value={state.pageSize}
            onChange={(e) =>
              void loadTablePage(tab.id, {
                pageSize: Number(e.target.value),
                page: 0,
              })
            }
          >
            <option value={100}>100 / page</option>
            <option value={500}>500 / page</option>
            <option value={1000}>1000 / page</option>
          </Select>
          <IconBtn
            title="Previous page"
            disabled={state.page === 0 || state.loading}
            onClick={() => void loadTablePage(tab.id, { page: state.page - 1 })}
          >
            <ChevronLeft size={14} />
          </IconBtn>
          <span className="text-zinc-400 tabular-nums">
            {state.page * state.pageSize + 1}–{state.page * state.pageSize + rows}
          </span>
          <IconBtn
            title="Next page"
            disabled={lastPage || state.loading}
            onClick={() => void loadTablePage(tab.id, { page: state.page + 1 })}
          >
            <ChevronRight size={14} />
          </IconBtn>
        </div>
      </div>

      {state.applyError && (
        <div className="flex shrink-0 items-start gap-2 border-b border-red-900/60 bg-red-950/50 px-3 py-2 text-[12px]">
          <CircleAlert size={14} className="mt-px shrink-0 text-red-400" />
          <div className="min-w-0 flex-1">
            <div className="font-medium text-red-300">
              Save failed — rolled back, {dirty} unsaved{" "}
              {dirty === 1 ? "change" : "changes"} kept
            </div>
            <pre className="selectable mt-0.5 max-h-24 overflow-auto whitespace-pre-wrap font-mono text-[11px] text-red-300/90">
              {state.applyError}
            </pre>
          </div>
          <button
            onClick={() => dismissApplyError(tab.id)}
            title="Dismiss"
            className="shrink-0 rounded p-0.5 text-red-400/70 hover:bg-red-900/40 hover:text-red-300"
          >
            <X size={13} />
          </button>
        </div>
      )}

      <div className="flex-1 min-h-0">
        {state.error ? (
          <ErrorPre>{state.error}</ErrorPre>
        ) : state.data ? (
          <ResultsGrid
            result={state.data.result}
            sortCol={state.orderBy}
            sortDir={state.orderDir}
            onSort={onSort}
            columnTypes={columnTypes}
            insertTarget={{ schema: state.schema, table: state.table }}
            editing={{
              edits: state.edits,
              deletes: state.deletes,
              inserts: state.inserts,
              disabledReason,
              applyFailed: state.applyFailed,
              onEdit: (row, col, value) =>
                stageCellEdit(tab.id, row, col, value),
              onToggleDelete: (rowsToToggle, del) =>
                toggleRowDeletes(tab.id, rowsToToggle, del),
              onDuplicate: (rowsToCopy) => duplicateRows(tab.id, rowsToCopy),
              onInsertEdit: (index, col, value) =>
                stageInsertCell(tab.id, index, col, value),
              onInsertRemove: (index) => removeInsertRow(tab.id, index),
              onApply: () => void applyEdits(tab.id),
              onDiscard: () => discardEdits(tab.id),
            }}
          />
        ) : (
          <div className="h-full flex items-center justify-center text-zinc-600 text-[12px]">
            {state.loading ? "loading…" : "no data"}
          </div>
        )}
      </div>
    </div>
  );
}
