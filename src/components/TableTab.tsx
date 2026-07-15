import {
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  FileCode2,
  Funnel,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useState } from "react";
import { parseRegclass, quoteIdent, quoteLit, relIdent } from "../lib/sql";
import { columnsKey, useApp, type Tab, type TableTabState } from "../lib/store";
import {
  isViewKind,
  type ColumnInfo,
  type RelationInfo,
  type SortSpec,
} from "../lib/types";
import { ReconnectButton } from "./ReconnectButton";
import { ResultsGrid, type GridEditing } from "./ResultsGrid";
import { TabError } from "./TabError";
import { cn, IconButton, PendingChangesBar, RefreshButton, Select } from "./ui";

function formatApprox(n: number): string {
  if (n < 0) return "~?";
  if (n < 10_000) return `~${n}`;
  if (n < 1_000_000) return `~${(n / 1000).toFixed(1)}k`;
  return `~${(n / 1_000_000).toFixed(1)}M`;
}

/** The SELECT behind this grid (see get_table_page). Columns hidden in the
 *  grid are dropped from the list; with none hidden it stays SELECT *. */
function currentViewSql(state: TableTabState, visible: string[] | null): string {
  const rel = relIdent(state.schema, state.table);
  const select = visible?.length ? visible.map(quoteIdent).join(", ") : "*";
  const where = state.filter.trim() ? `\nWHERE ${state.filter.trim()}` : "";
  const order = state.sorts.length
    ? `\nORDER BY ${state.sorts
        .map((s) => `${quoteIdent(s.column)} ${s.dir === "desc" ? "DESC" : "ASC"}`)
        .join(", ")}`
    : "";
  return `SELECT ${select}\nFROM ${rel}${where}${order}\nLIMIT ${state.pageSize} OFFSET ${state.page * state.pageSize}`;
}

export function TableTab({ tab }: { tab: Tab }) {
  const state = tab.state as TableTabState;
  const sessions = useApp((s) => s.sessions);
  const tables = useApp((s) => s.tables);
  const refreshTablePage = useApp((s) => s.refreshTablePage);
  const loadTableColumns = useApp((s) => s.loadTableColumns);
  const tableColumns = useApp((s) => s.tableColumns);
  const loadTableRelations = useApp((s) => s.loadTableRelations);
  const tableRelations = useApp((s) => s.tableRelations);
  const openQueryTab = useApp((s) => s.openQueryTab);
  const openTableTab = useApp((s) => s.openTableTab);
  const stageCellEdit = useApp((s) => s.stageCellEdit);
  const setRowsDeleted = useApp((s) => s.setRowsDeleted);
  const duplicateRows = useApp((s) => s.duplicateRows);
  const stageInsertCell = useApp((s) => s.stageInsertCell);
  const removeInsertRow = useApp((s) => s.removeInsertRow);
  const discardEdits = useApp((s) => s.discardEdits);
  const applyEdits = useApp((s) => s.applyEdits);
  const dismissApplyError = useApp((s) => s.dismissApplyError);
  const connected = Boolean(sessions[tab.profileId]);
  // A drop mid-session must not blank rows the user was looking at: keep
  // the cached page under a Reconnect banner. Errors without cached data
  // (or unrelated to the connection) still take over the tab body.
  const staleData = Boolean(state.error && state.data && state.connectionLost);
  // Hidden grid columns, mirrored from ResultsGrid — the "current view as
  // query" SQL leaves them out. Indices refer to state.data.result.columns.
  const [hiddenCols, setHiddenCols] = useState<ReadonlySet<number>>(new Set());
  // WHERE bar: draft until Enter applies it; auto-shown while a filter is on.
  const [showFilter, setShowFilter] = useState(Boolean(state.filter));
  const [filterDraft, setFilterDraft] = useState(state.filter);

  // External filter changes (FK navigation onto this tab) resync the bar.
  useEffect(() => {
    setFilterDraft(state.filter);
    if (state.filter) setShowFilter(true);
  }, [state.filter]);

  // Lazy load: restored/reopened tabs fetch when first shown, not in bulk
  // at boot (dozens of parallel page queries froze the app).
  useEffect(() => {
    if (connected && !state.data && !state.loading && !state.error) {
      void refreshTablePage(tab.id);
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

  // Foreign keys enable ⌘-click navigation to the referenced row.
  const rels =
    tableRelations[columnsKey(tab.profileId, state.schema, state.table)];
  useEffect(() => {
    if (connected && !rels) {
      void loadTableRelations(tab.profileId, state.schema, state.table);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected, rels, tab.profileId, state.schema, state.table]);

  /** First FK covering each column name (string_agg output is ", "-joined). */
  const fkByCol = useMemo(() => {
    const map = new Map<string, RelationInfo>();
    for (const r of rels ?? []) {
      for (const c of r.columns?.split(", ") ?? []) {
        if (!map.has(c)) map.set(c, r);
      }
    }
    return map;
  }, [rels]);

  const fkColumns = useMemo(() => {
    const set = new Set<number>();
    state.data?.result.columns.forEach((name, i) => {
      if (fkByCol.has(name)) set.add(i);
    });
    return set;
  }, [state.data, fkByCol]);

  /** Opens the referenced table filtered to the row the FK points at. */
  const followFk = useCallback(
    (ri: number, ci: number) => {
      const res = state.data?.result;
      if (!res) return;
      const rel = fkByCol.get(res.columns[ci]);
      if (!rel) return;
      const from = rel.columns?.split(", ") ?? [];
      const to = (rel.refColumns ?? rel.columns)?.split(", ") ?? [];
      const target = parseRegclass(rel.refTable);
      const parts = to.map((refCol, i) => {
        const idx = res.columns.indexOf(from[i]);
        const v = idx >= 0 ? (res.rows[ri]?.[idx] ?? null) : null;
        return v === null
          ? `${quoteIdent(refCol)} IS NULL`
          : `${quoteIdent(refCol)} = ${quoteLit(v)}`;
      });
      openTableTab(tab.profileId, target.schema, target.table, parts.join(" AND "));
    },
    [state.data, fkByCol, openTableTab, tab.profileId],
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
      onInsertRemove: (index) => removeInsertRow(tab.id, index),
      onApply: () => void applyEdits(tab.id),
      onDiscard: () => discardEdits(tab.id),
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
      removeInsertRow,
      applyEdits,
      discardEdits,
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
          busy={state.loading}
          onClick={() => void refreshTablePage(tab.id)}
        />
        <IconButton
          title="Current view as query — open this grid's SQL in a new tab"
          onClick={() => {
            const all = state.data?.result.columns ?? [];
            const visible = hiddenCols.size
              ? all.filter((_, i) => !hiddenCols.has(i))
              : null;
            openQueryTab(
              tab.profileId,
              currentViewSql(state, visible),
              state.table,
            );
          }}
        >
          <FileCode2 size={13} />
        </IconButton>
        <IconButton
          title="Filter (WHERE …)"
          className={state.filter ? "text-amber-400" : undefined}
          onClick={() => setShowFilter((v) => !v || Boolean(state.filter))}
        >
          <Funnel size={13} />
        </IconButton>

        {dirty > 0 && (
          <PendingChangesBar
            count={dirty}
            busy={state.loading}
            applyTitle="⌘S — runs INSERT/UPDATE/DELETE in one transaction"
            discardTitle="Esc"
            onApply={() => void applyEdits(tab.id)}
            onDiscard={() => discardEdits(tab.id)}
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

      {showFilter && (
        <div className="flex shrink-0 items-center gap-2 border-b border-zinc-800 px-3 py-1">
          <span className="shrink-0 font-mono text-[11px] font-semibold text-zinc-500">
            WHERE
          </span>
          <input
            autoFocus={!state.filter}
            value={filterDraft}
            onChange={(e) => setFilterDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                void refreshTablePage(tab.id, {
                  filter: filterDraft.trim(),
                  page: 0,
                });
              } else if (e.key === "Escape") {
                setFilterDraft(state.filter);
                if (!state.filter) setShowFilter(false);
              }
            }}
            placeholder="status = 'active' AND created_at > now() - interval '1 day'"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            className={cn(
              "min-w-0 flex-1 bg-transparent font-mono text-[12px] outline-none placeholder:text-zinc-700",
              filterDraft !== state.filter ? "text-amber-200" : "text-zinc-100",
            )}
          />
          {filterDraft !== state.filter && (
            <span className="shrink-0 text-[10px] text-amber-400/80">
              ⏎ apply
            </span>
          )}
          {(state.filter || filterDraft) && (
            <IconButton
              title="Clear filter"
              onClick={() => {
                setFilterDraft("");
                if (state.filter) {
                  void refreshTablePage(tab.id, { filter: "", page: 0 });
                }
              }}
            >
              <X size={12} />
            </IconButton>
          )}
        </div>
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
    </div>
  );
}
