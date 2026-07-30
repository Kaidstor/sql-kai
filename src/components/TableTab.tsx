import {
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  FileCode2,
  Funnel,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { useLazyTabLoad } from "../hooks/useLazyTabLoad";
import { quoteIdent, relIdent } from "../lib/sql";
import {
  columnsKey,
  useApp,
  type RelRef,
  type Tab,
  type TableTabState,
} from "../lib/store";
import { isViewKind, type ColumnInfo, type SortSpec } from "../lib/types";
import { ExportMenu } from "./ExportMenu";
import { FilterBar } from "./FilterBar";
import { ReconnectButton } from "./ReconnectButton";
import { FkPreviewPanel } from "./table/FkPreviewPanel";
import { useTableFk } from "./table/useTableFk";
import { ResultsGrid, type GridEditing } from "./ResultsGrid";
import { TabError } from "./TabError";
import { IconButton, PendingChangesBar, RefreshButton, Select } from "./ui";

function formatApprox(n: number): string {
  if (n < 0) return "~?";
  if (n < 10_000) return `~${n}`;
  if (n < 1_000_000) return `~${(n / 1000).toFixed(1)}k`;
  return `~${(n / 1_000_000).toFixed(1)}M`;
}

/** The SELECT behind this grid (see get_table_page). Columns hidden in the
 *  grid are dropped from the list; with none hidden it stays SELECT *.
 *  `withPage: false` drops LIMIT/OFFSET — the full result for the export. */
function currentViewSql(
  state: TableTabState,
  visible: string[] | null,
  withPage = true,
): string {
  const rel = relIdent(state.schema, state.table);
  const select = visible?.length ? visible.map(quoteIdent).join(", ") : "*";
  const where = state.filter.trim() ? `\nWHERE ${state.filter.trim()}` : "";
  const order = state.sorts.length
    ? `\nORDER BY ${state.sorts
        .map((s) => `${quoteIdent(s.column)} ${s.dir === "desc" ? "DESC" : "ASC"}`)
        .join(", ")}`
    : "";
  const page = withPage
    ? `\nLIMIT ${state.pageSize} OFFSET ${state.page * state.pageSize}`
    : "";
  return `SELECT ${select}\nFROM ${rel}${where}${order}${page}`;
}

export function TableTab({ tab }: { tab: Tab }) {
  const state = tab.state as TableTabState;
  const sessions = useApp((s) => s.sessions);
  const tables = useApp((s) => s.tables);
  const refreshTablePage = useApp((s) => s.refreshTablePage);
  const loadTableColumns = useApp((s) => s.loadTableColumns);
  const tableColumns = useApp((s) => s.tableColumns);
  const previewFk = useApp((s) => s.previewFk);
  const closeFkPreview = useApp((s) => s.closeFkPreview);
  const openQueryTab = useApp((s) => s.openQueryTab);
  const openTableTab = useApp((s) => s.openTableTab);
  const stageCellEdit = useApp((s) => s.stageCellEdit);
  const setRowsDeleted = useApp((s) => s.setRowsDeleted);
  const duplicateRows = useApp((s) => s.duplicateRows);
  const stageInsertCell = useApp((s) => s.stageInsertCell);
  const unstageInsertRow = useApp((s) => s.unstageInsertRow);
  const discardTableEdits = useApp((s) => s.discardTableEdits);
  const applyTableEdits = useApp((s) => s.applyTableEdits);
  const dismissApplyError = useApp((s) => s.dismissApplyError);
  const connected = Boolean(sessions[tab.profileId]);
  // A drop mid-session must not blank rows the user was looking at: keep
  // the cached page under a Reconnect banner. Errors without cached data
  // (or unrelated to the connection) still take over the tab body.
  const staleData = Boolean(state.error && state.data && state.connectionLost);
  // Hidden grid columns, mirrored from ResultsGrid — the "current view as
  // query" SQL leaves them out. Indices refer to state.data.result.columns.
  const [hiddenCols, setHiddenCols] = useState<ReadonlySet<number>>(new Set());
  // WHERE bar (see FilterBar): auto-shown while a filter is on.
  const [filterOpen, setFilterOpen] = useState(Boolean(state.filter));

  // External filter changes (FK navigation onto this tab) pop the bar open.
  useEffect(() => {
    if (state.filter) setFilterOpen(true);
  }, [state.filter]);

  useLazyTabLoad(
    connected,
    Boolean(state.data || state.loading || state.error),
    () => void refreshTablePage(tab.id),
  );

  const ref = useMemo<RelRef>(
    () => ({ profileId: tab.profileId, schema: state.schema, table: state.table }),
    [tab.profileId, state.schema, state.table],
  );
  const cols = tableColumns[columnsKey(ref)];

  // Column info gives us the primary key needed to build UPDATE/DELETE.
  // Also refetches after runDdl invalidates the cache.
  useEffect(() => {
    if (!cols) void loadTableColumns(ref);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cols, ref]);

  const fkColumns = useTableFk(ref, connected, state.data?.result.columns);
  const fkPreview = state.fkPreview;
  const followFk = useCallback(
    (row: number, col: number) => void previewFk(tab.id, row, col),
    [previewFk, tab.id],
  );

  const hasPk = (cols ?? []).some((c) => c.isPk);
  // Views (and matviews) are read-only: no INSERT/UPDATE/DELETE through the grid.
  const relKind = (tables[tab.profileId] ?? []).find(
    (t) => t.schema === state.schema && t.name === state.table,
  )?.kind;
  const isView = isViewKind(relKind);
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

  const rows = state.data?.result.rows.length ?? 0;
  const lastPage = rows < state.pageSize;

  // Visible column names (null = all) shared by "view as query" and export.
  const visibleColNames = useMemo(() => {
    const all = state.data?.result.columns ?? [];
    return hiddenCols.size ? all.filter((_, i) => !hiddenCols.has(i)) : null;
  }, [state.data, hiddenCols]);

  // Loaded page as displayed: staged cell edits applied and hidden columns
  // dropped — the export menu's copy section mirrors the grid (its own copy
  // paths go through shownValue), not the stale DB values.
  const exportResult = useMemo(() => {
    const res = state.data?.result;
    if (!res) return res;
    const hasEdits = Object.keys(state.edits).length > 0;
    if (!hasEdits && !hiddenCols.size) return res;
    // edits are keyed by ORIGINAL column index — apply before dropping columns
    const rows = hasEdits
      ? res.rows.map((r, ri) => {
          const rowEdits = state.edits[ri];
          if (!rowEdits) return r;
          return r.map((v, ci) => (ci in rowEdits ? rowEdits[ci] : v));
        })
      : res.rows;
    if (!hiddenCols.size) return { ...res, rows };
    const keep = res.columns.map((_, i) => i).filter((i) => !hiddenCols.has(i));
    return {
      ...res,
      columns: keep.map((i) => res.columns[i]),
      rows: rows.map((r) => keep.map((i) => r[i])),
    };
  }, [state.data, state.edits, hiddenCols]);

  // O(1) column lookup by name, shared by the type/nullable projections below
  // (was an O(cols·rows) `cols.find` per result column on every render).
  const colByName = useMemo(() => {
    const m = new Map<string, ColumnInfo>();
    for (const c of cols ?? []) m.set(c.name, c);
    return m;
  }, [cols]);
  const columnTypes = useMemo(
    () =>
      state.data?.result.columns.map((name) => colByName.get(name)?.dataType),
    [state.data, colByName],
  );
  const columnNullable = useMemo(
    () =>
      state.data?.result.columns.map((name) => colByName.get(name)?.nullable),
    [state.data, colByName],
  );

  // Stable props so React.memo(ResultsGrid) can skip re-renders driven by
  // TableTab's own state (filter draft, page chrome) that don't touch the grid.
  const insertTarget = useMemo(
    () => ({ schema: state.schema, table: state.table }),
    [state.schema, state.table],
  );
  const handleSortsChange = useCallback(
    (sorts: SortSpec[]) => void refreshTablePage(tab.id, { sorts, page: 0 }),
    [refreshTablePage, tab.id],
  );
  const editing = useMemo<GridEditing>(
    () => ({
      edits: state.edits,
      deletes: state.deletes,
      inserts: state.inserts,
      disabledReason,
      applyFailed: state.applyFailed,
      onEdit: (row, col, value) => stageCellEdit(tab.id, row, col, value),
      onToggleDelete: (rowsToToggle, del) =>
        setRowsDeleted(tab.id, rowsToToggle, del),
      onDuplicate: (rowsToCopy) => duplicateRows(tab.id, rowsToCopy),
      onInsertEdit: (index, col, value) =>
        stageInsertCell(tab.id, index, col, value),
      onInsertRemove: (index) => unstageInsertRow(tab.id, index),
      onApply: () => void applyTableEdits(tab.id),
      onDiscard: () => discardTableEdits(tab.id),
    }),
    [
      state.edits,
      state.deletes,
      state.inserts,
      disabledReason,
      state.applyFailed,
      tab.id,
      stageCellEdit,
      setRowsDeleted,
      duplicateRows,
      stageInsertCell,
      unstageInsertRow,
      applyTableEdits,
      discardTableEdits,
    ],
  );

  return (
    <div className="flex flex-col h-full min-h-0">
      <div className="flex min-h-10 items-center gap-1.5 px-2 py-1.5 border-b border-zinc-800 shrink-0 text-[12px]">
        <span className="font-mono text-zinc-300">
          {state.schema}.{state.table}
        </span>
        <RefreshButton
          title="Refresh (⌘R)"
          loading={state.loading}
          onClick={() => void refreshTablePage(tab.id)}
        />
        <IconButton
          title="Current view as query — open this grid's SQL in a new tab"
          onClick={() =>
            openQueryTab(
              tab.profileId,
              currentViewSql(state, visibleColNames),
              state.table,
            )
          }
        >
          <FileCode2 size={13} />
        </IconButton>
        <IconButton
          title="Filter (WHERE …)"
          className={state.filter ? "text-amber-400" : undefined}
          onClick={() => setFilterOpen((v) => !v || Boolean(state.filter))}
        >
          <Funnel size={13} />
        </IconButton>

        {dirty > 0 && (
          <PendingChangesBar
            count={dirty}
            loading={state.loading}
            applyTitle="⌘S — runs INSERT/UPDATE/DELETE in one transaction"
            discardTitle="Esc"
            onApply={() => void applyTableEdits(tab.id)}
            onDiscard={() => discardTableEdits(tab.id)}
          />
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
          <ExportMenu
            result={exportResult}
            profileId={tab.profileId}
            sessionId={sessions[tab.profileId]?.sessionId ?? null}
            sql={state.data ? currentViewSql(state, visibleColNames, false) : null}
            fileBase={state.table}
          />
          <Select
            value={state.pageSize}
            onChange={(e) =>
              void refreshTablePage(tab.id, {
                pageSize: Number(e.target.value),
                page: 0,
              })
            }
          >
            <option value={100}>100 / page</option>
            <option value={500}>500 / page</option>
            <option value={1000}>1000 / page</option>
          </Select>
          <IconButton
            title="Previous page"
            disabled={state.page === 0 || state.loading}
            onClick={() => void refreshTablePage(tab.id, { page: state.page - 1 })}
          >
            <ChevronLeft size={14} />
          </IconButton>
          <span className="text-zinc-400 tabular-nums">
            {state.page * state.pageSize + 1}–{state.page * state.pageSize + rows}
          </span>
          <IconButton
            title="Next page"
            disabled={lastPage || state.loading}
            onClick={() => void refreshTablePage(tab.id, { page: state.page + 1 })}
          >
            <ChevronRight size={14} />
          </IconButton>
        </div>
      </div>

      {filterOpen && (
        <FilterBar
          tabId={tab.id}
          profileId={tab.profileId}
          filter={state.filter}
          columns={cols}
          dataColumns={state.data?.result.columns ?? []}
          onHide={() => setFilterOpen(false)}
        />
      )}

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

      {staleData && (
        <div className="flex shrink-0 items-center gap-2 border-b border-red-900/60 bg-red-950/40 px-3 py-1.5 text-[12px] text-red-300">
          <CircleAlert size={13} className="shrink-0" />
          <span className="truncate" title={state.error}>
            Connection lost — showing cached data
          </span>
          <ReconnectButton
            profileId={tab.profileId}
            iconSize={11}
            className="ml-auto shrink-0 rounded border border-zinc-700 bg-zinc-900 px-2 py-0.5 text-[11px] text-zinc-200 hover:bg-zinc-800 disabled:opacity-60"
          />
        </div>
      )}

      <div className="flex-1 min-h-0">
        {state.error && !staleData ? (
          <TabError profileId={tab.profileId} error={state.error} lost={state.connectionLost} />
        ) : state.data ? (
          <ResultsGrid
            result={state.data.result}
            sorts={state.sorts}
            onSortsChange={handleSortsChange}
            onHiddenColsChange={setHiddenCols}
            columnTypes={columnTypes}
            columnNullable={columnNullable}
            insertTarget={insertTarget}
            fkColumns={fkColumns}
            onFollowFk={followFk}
            editing={editing}
          />
        ) : (
          <div className="h-full flex items-center justify-center text-zinc-600 text-[12px]">
            {state.loading ? "loading…" : "no data"}
          </div>
        )}
      </div>

      {fkPreview && (
        <FkPreviewPanel
          preview={fkPreview}
          onOpenTable={() => {
            const { target, filter } = fkPreview;
            openTableTab(tab.profileId, target.schema, target.table, filter);
            closeFkPreview(tab.id);
          }}
          onClose={() => closeFkPreview(tab.id)}
        />
      )}
    </div>
  );
}
