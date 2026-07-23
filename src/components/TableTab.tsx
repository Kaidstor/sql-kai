import {
  ArrowUpRight,
  ChevronLeft,
  ChevronRight,
  CircleAlert,
  FileCode2,
  Funnel,
  Loader2,
  X,
} from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, errText } from "../lib/api";
import { parseRegclass, quoteIdent, quoteLit, relIdent } from "../lib/sql";
import {
  columnsKey,
  useApp,
  type RelRef,
  type Tab,
  type TableTabState,
} from "../lib/store";
import {
  isViewKind,
  type ColumnInfo,
  type RelationInfo,
  type SortSpec,
  type StatementResult,
} from "../lib/types";
import { ExportMenu } from "./ExportMenu";
import { FilterBar } from "./FilterBar";
import { ReconnectButton } from "./ReconnectButton";
import { useLazyTabLoad } from "./useLazyTabLoad";
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
  const loadTableRelations = useApp((s) => s.loadTableRelations);
  const tableRelations = useApp((s) => s.tableRelations);
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

  // Foreign keys enable ⌘-click navigation to the referenced row.
  const rels = tableRelations[columnsKey(ref)];
  useEffect(() => {
    if (connected && !rels) void loadTableRelations(ref);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected, rels, ref]);

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

  // FK preview (Drizzle-Studio-style): ⌘-клик показывает записи по ссылке
  // в нижней панели — чаще всего нужна одна строка глазами, а не переход;
  // переход остаётся кнопкой в шапке панели.
  const [fkPreview, setFkPreview] = useState<{
    target: { schema: string; table: string };
    filter: string;
    result: StatementResult | null;
    loading: boolean;
    error?: string;
  } | null>(null);
  const previewSeq = useRef(0);

  // Смена страницы/фильтра/таблицы делает превью неактуальным.
  useEffect(() => {
    previewSeq.current++;
    setFkPreview(null);
  }, [state.schema, state.table, state.filter, state.page]);

  /** Показывает записи, на которые ссылается FK-ячейка, в нижней панели. */
  const previewFk = useCallback(
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
      const filter = parts.join(" AND ");
      const session = useApp.getState().sessions[tab.profileId];
      if (!session) return;
      const seq = ++previewSeq.current;
      setFkPreview({ target, filter, result: null, loading: true });
      void api
        .executeSql(
          session.sessionId,
          `SELECT * FROM ${relIdent(target.schema, target.table)} WHERE ${filter} LIMIT 50`,
          50,
        )
        .then((exec) => {
          if (previewSeq.current !== seq) return;
          setFkPreview((p) =>
            p ? { ...p, result: exec.results[0] ?? null, loading: false } : p,
          );
        })
        .catch((e) => {
          if (previewSeq.current !== seq) return;
          setFkPreview((p) =>
            p ? { ...p, loading: false, error: errText(e) } : p,
          );
        });
    },
    [state.data, fkByCol, tab.profileId],
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
            onFollowFk={previewFk}
            editing={editing}
          />
        ) : (
          <div className="h-full flex items-center justify-center text-zinc-600 text-[12px]">
            {state.loading ? "loading…" : "no data"}
          </div>
        )}
      </div>

      {fkPreview && (
        <div className="flex h-56 shrink-0 flex-col border-t border-zinc-800">
          <div className="flex shrink-0 items-center gap-2 border-b border-zinc-800 bg-zinc-925 px-2 py-1 text-[11px]">
            <span className="shrink-0 font-mono font-medium text-zinc-200">
              {fkPreview.target.schema === "public"
                ? fkPreview.target.table
                : `${fkPreview.target.schema}.${fkPreview.target.table}`}
            </span>
            <span
              className="truncate font-mono text-zinc-500"
              title={fkPreview.filter}
            >
              {fkPreview.filter}
            </span>
            {fkPreview.result && (
              <span className="shrink-0 text-zinc-600">
                {fkPreview.result.rows.length}
                {fkPreview.result.truncated ? "+" : ""} row(s)
              </span>
            )}
            <div className="ml-auto flex shrink-0 items-center gap-1">
              <button
                onClick={() => {
                  openTableTab(
                    tab.profileId,
                    fkPreview.target.schema,
                    fkPreview.target.table,
                    fkPreview.filter,
                  );
                  setFkPreview(null);
                }}
                className="flex items-center gap-1 rounded px-1.5 py-0.5 text-zinc-300 transition-colors hover:bg-zinc-800 hover:text-zinc-100"
                title="Open the referenced table as a tab with this filter"
              >
                <ArrowUpRight size={12} />
                Open table
              </button>
              <IconButton title="Close preview" onClick={() => setFkPreview(null)}>
                <X size={13} />
              </IconButton>
            </div>
          </div>
          <div className="min-h-0 flex-1">
            {fkPreview.loading ? (
              <div className="flex h-full items-center justify-center gap-2 text-[12px] text-zinc-600">
                <Loader2 size={13} className="animate-spin" /> loading…
              </div>
            ) : fkPreview.error ? (
              <div className="selectable overflow-auto p-3 font-mono text-[12px] whitespace-pre-wrap text-red-400">
                {fkPreview.error}
              </div>
            ) : fkPreview.result && fkPreview.result.rows.length > 0 ? (
              <ResultsGrid result={fkPreview.result} />
            ) : (
              <div className="flex h-full items-center justify-center text-[12px] text-zinc-600">
                no referenced rows
              </div>
            )}
          </div>
        </div>
      )}
    </div>
  );
}
