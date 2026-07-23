// Table-tab data: page loading and the staged-edit lifecycle (cell edits,
// row deletes, duplicated-row inserts) up to the transactional Apply.
import { api, isSessionLost } from "../../api";
import { buildTableDml } from "../../mutationSql";
import type { Get, Set, StoreContext } from "../context";
import { columnsKey, noTableEdits } from "../helpers";
import type { InsertRow, TableTabState } from "../types";

export interface TableSlice {
  refreshTablePage: (
    tabId: string,
    patch?: Partial<
      Pick<TableTabState, "page" | "pageSize" | "sorts" | "filter">
    >,
  ) => Promise<void>;
  /** Stage a cell value; staging the original value reverts the cell. */
  stageCellEdit: (
    tabId: string,
    row: number,
    col: number,
    value: string | null,
  ) => void;
  /** Stage / unstage DELETE — mirrors `setColumnDropped` on the structure side. */
  setRowsDeleted: (tabId: string, rows: number[], deleted: boolean) => void;
  /** Stages copies of the rows as pending INSERTs; generated keys are cut. */
  duplicateRows: (tabId: string, rows: number[]) => void;
  stageInsertCell: (
    tabId: string,
    index: number,
    col: number,
    value: string | null | undefined,
  ) => void;
  /** Drops one staged INSERT — mirrors `unstageColumnAdd`. */
  unstageInsertRow: (tabId: string, index: number) => void;
  discardTableEdits: (tabId: string) => void;
  /** Applies staged INSERT/UPDATE/DELETE in one transaction (PK for the latter two). */
  applyTableEdits: (tabId: string) => Promise<void>;
  /** Hides the failed-Apply banner; staged cells stay red until re-applied. */
  dismissApplyError: (tabId: string) => void;
}

export function createTableSlice(_set: Set, get: Get, ctx: StoreContext): TableSlice {
  const { tabOf, patchTab } = ctx;

  return {
    refreshTablePage: async (tabId, patch) => {
      const tab = tabOf(tabId, "table");
      if (!tab) return;
      const session = ctx.sessionFor(tab.profileId);
      if (!session) return;
      // Reloading renumbers rows, so staged edits can't survive it.
      if (
        (Object.keys(tab.state.edits).length > 0 ||
          tab.state.deletes.length > 0) &&
        !(await get().confirmDialog({
          title: "Discard pending changes?",
          message: "Reloading the page drops staged edits and deletes.",
          confirmLabel: "Discard",
          danger: true,
        }))
      ) {
        return;
      }
      const seq = ctx.nextLoadSeq(tabId);
      const next = { ...tab.state, ...patch };
      patchTab<TableTabState>(tabId, {
        ...patch,
        loading: true,
        error: undefined,
        connectionLost: undefined,
      });
      try {
        const data = await api.getTablePage(
          session.sessionId,
          next.schema,
          next.table,
          next.pageSize,
          next.page * next.pageSize,
          next.sorts,
          next.filter,
        );
        // Медленный старый ответ не должен перетирать данные более нового
        // запроса (быстрое листание страниц / смена сортировки).
        if (ctx.staleLoad(tabId, seq)) return;
        // Pending inserts survive a reload (not tied to row indices).
        patchTab<TableTabState>(tabId, {
          data,
          loading: false,
          edits: {},
          deletes: [],
          applyFailed: false,
          applyError: undefined,
        });
      } catch (e) {
        const message = ctx.handleSqlError(tab.profileId, e);
        if (ctx.staleLoad(tabId, seq)) return;
        patchTab<TableTabState>(tabId, {
          error: message,
          connectionLost: isSessionLost(e),
          loading: false,
        });
      }
    },

    stageCellEdit: (tabId, row, col, value) => {
      const tab = tabOf(tabId, "table");
      if (!tab?.state.data) return;
      const st = tab.state;
      const original = st.data?.result.rows[row]?.[col] ?? null;
      const rowEdits = { ...(st.edits[row] ?? {}) };
      if (value === original) delete rowEdits[col];
      else rowEdits[col] = value;
      const edits = { ...st.edits };
      if (Object.keys(rowEdits).length > 0) edits[row] = rowEdits;
      else delete edits[row];
      patchTab<TableTabState>(tabId, { edits });
    },

    setRowsDeleted: (tabId, rows, deleted) => {
      const tab = tabOf(tabId, "table");
      if (!tab) return;
      const next = new Set(tab.state.deletes);
      for (const r of rows) {
        if (deleted) next.add(r);
        else next.delete(r);
      }
      patchTab<TableTabState>(tabId, {
        deletes: [...next].sort((a, b) => a - b),
      });
    },

    duplicateRows: (tabId, rows) => {
      const tab = tabOf(tabId, "table");
      const data = tab?.state.data;
      if (!tab || !data) return;
      const cols =
        get().tableColumns[
          columnsKey({
            profileId: tab.profileId,
            schema: tab.state.schema,
            table: tab.state.table,
          })
        ];
      const colByName = new Map((cols ?? []).map((c) => [c.name, c]));
      // generated keys are cut (undefined) so the DB regenerates them on INSERT
      const cut = new Set<number>();
      data.result.columns.forEach((name, ci) => {
        const c = colByName.get(name);
        if (c?.isPk && c.defaultExpr) cut.add(ci);
      });
      const srcRows = rows.filter((ri) => data.result.rows[ri]);
      if (srcRows.length === 0) return;
      // The whole batch renders under the last duplicated row — copies stay
      // grouped together, which is easier to edit than one-under-each.
      const after = Math.max(...srcRows);
      const copies: InsertRow[] = srcRows.map((ri) => ({
        after,
        values: data.result.rows[ri].map((v, ci) =>
          cut.has(ci) ? undefined : v,
        ),
      }));
      patchTab<TableTabState>(tabId, {
        inserts: [...tab.state.inserts, ...copies],
      });
    },

    stageInsertCell: (tabId, index, col, value) => {
      const tab = tabOf(tabId, "table");
      if (!tab || !tab.state.inserts[index]) return;
      const inserts = tab.state.inserts.map((row, i) =>
        i === index
          ? { ...row, values: row.values.map((v, ci) => (ci === col ? value : v)) }
          : row,
      );
      patchTab<TableTabState>(tabId, { inserts });
    },

    unstageInsertRow: (tabId, index) => {
      const tab = tabOf(tabId, "table");
      if (!tab) return;
      patchTab<TableTabState>(tabId, {
        inserts: tab.state.inserts.filter((_, i) => i !== index),
      });
    },

    discardTableEdits: (tabId) => {
      const tab = tabOf(tabId, "table");
      if (!tab) return;
      const st = tab.state;
      if (
        Object.keys(st.edits).length === 0 &&
        st.deletes.length === 0 &&
        st.inserts.length === 0
      ) {
        return;
      }
      patchTab<TableTabState>(tabId, noTableEdits());
      get().showToast("Pending changes discarded", "info");
    },

    applyTableEdits: async (tabId) => {
      const tab = tabOf(tabId, "table");
      if (!tab) return;
      const st = tab.state;
      if (st.loading) return; // e.g. ⌘S mashed while a previous apply runs
      const data = st.data;
      if (!data) return;
      const session = ctx.sessionFor(tab.profileId);
      if (!session) return;
      const dml = buildTableDml(
        st,
        data,
        get().tableColumns[
          columnsKey({ profileId: tab.profileId, schema: st.schema, table: st.table })
        ],
      );
      if ("error" in dml) {
        get().showToast(dml.error);
        return;
      }
      const { stmts, updates, deletes } = dml;
      if (stmts.length === 0) return;
      const profile = get().profiles.find((p) => p.id === tab.profileId);
      if (profile?.production) {
        const parts = [
          updates > 0 && `${updates} UPDATE`,
          deletes > 0 && `${deletes} DELETE`,
          st.inserts.length > 0 && `${st.inserts.length} INSERT`,
        ]
          .filter(Boolean)
          .join(", ");
        const ok = await get().confirmDialog({
          title: `"${profile.name}" is PRODUCTION`,
          message: `Apply ${parts}?`,
          confirmLabel: "Apply",
          danger: true,
        });
        if (!ok) return;
      }
      patchTab<TableTabState>(tabId, {
        loading: true,
        applyFailed: false,
        applyError: undefined,
      });
      const message = await ctx.executeStatements(
        tab.profileId,
        session.sessionId,
        stmts,
      );
      if (message) {
        patchTab<TableTabState>(tabId, {
          loading: false,
          // the whole tx rolled back — every staged cell is unsaved
          applyFailed: true,
          applyError: message,
        });
        get().showToast(message); // rolled back, edits stay staged
        return;
      }
      const done = [
        updates > 0 && `updated ${updates} row(s)`,
        deletes > 0 && `deleted ${deletes} row(s)`,
        st.inserts.length > 0 && `inserted ${st.inserts.length} row(s)`,
      ]
        .filter(Boolean)
        .join(", ");
      get().showToast(done.charAt(0).toUpperCase() + done.slice(1), "success");
      // No auto-refetch: under a sort, reloading makes just-edited rows jump
      // to their new position. Patch the page in place instead; ⌘R re-syncs.
      // Inserts are the exception — the DB generated their keys/defaults, so
      // only a reload can show them.
      if (st.inserts.length > 0) {
        patchTab<TableTabState>(tabId, noTableEdits());
        await get().refreshTablePage(tabId);
        return;
      }
      const deletedRows = new Set(st.deletes);
      const rows = data.result.rows
        .map((row, ri) =>
          st.edits[ri]
            ? row.map((v, ci) => (st.edits[ri][ci] === undefined ? v : st.edits[ri][ci]))
            : row,
        )
        .filter((_, ri) => !deletedRows.has(ri));
      patchTab<TableTabState>(tabId, {
        data: { ...data, result: { ...data.result, rows } },
        loading: false,
        ...noTableEdits(),
      });
    },

    dismissApplyError: (tabId) =>
      patchTab<TableTabState>(tabId, { applyError: undefined }),
  };
}
