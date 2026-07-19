// Structure-tab: section loading (columns/indexes/relations/triggers), ad-hoc
// DDL and the staged column-DDL lifecycle up to the transactional Apply.
import { api, isSessionLost } from "../../api";
import { buildStructureDdl } from "../../mutationSql";
import type { Get, Set, StoreContext } from "../context";
import { columnsKey, noStructureEdits, without } from "../helpers";
import type {
  ColumnPatch,
  NewColumn,
  StructureSection,
  StructureTabState,
} from "../types";

export interface StructureSlice {
  setStructureSection: (tabId: string, section: StructureSection) => void;
  refreshStructure: (tabId: string) => Promise<void>;
  /** Runs DDL in the tab's session, refreshes the structure view. Returns success. */
  runDdl: (tabId: string, sql: string) => Promise<boolean>;
  /** Stage a column change; staging the original value reverts the field. */
  stageColumnEdit: (tabId: string, column: string, patch: ColumnPatch) => void;
  toggleColumnDrop: (tabId: string, column: string) => void;
  stageColumnAdd: (tabId: string, col: NewColumn) => void;
  unstageColumnAdd: (tabId: string, index: number) => void;
  discardStructureEdits: (tabId: string) => void;
  /** Applies staged column DDL in one implicit transaction, then reloads. */
  applyStructureEdits: (tabId: string) => Promise<void>;
}

export function createStructureSlice(
  set: Set,
  get: Get,
  ctx: StoreContext,
): StructureSlice {
  const { tabOf, patchTab } = ctx;

  /** Drops cached column/FK info after DDL changed the table. */
  const invalidateColumns = (profileId: string, schema: string, table: string) =>
    set((s) => {
      const key = columnsKey(profileId, schema, table);
      return {
        tableColumns: without(s.tableColumns, key),
        tableRelations: without(s.tableRelations, key),
      };
    });

  return {
    setStructureSection: (tabId, section) => {
      patchTab<StructureTabState>(tabId, { section });
      void get().refreshStructure(tabId);
    },

    refreshStructure: async (tabId) => {
      const tab = tabOf(tabId, "structure");
      if (!tab) return;
      const session = ctx.sessionFor(tab.profileId);
      if (!session) return;
      const seq = ctx.nextLoadSeq(tabId);
      const { schema, table, section } = tab.state;
      const fetch: Record<
        StructureSection,
        () => Promise<Partial<StructureTabState>>
      > = {
        columns: async () => ({
          columns: await api.listColumns(session.sessionId, schema, table),
        }),
        indexes: async () => ({
          indexes: await api.listIndexes(session.sessionId, schema, table),
        }),
        relations: async () => ({
          relations: await api.listRelations(session.sessionId, schema, table),
        }),
        triggers: async () => ({
          triggers: await api.listTriggers(session.sessionId, schema, table),
        }),
        policies: async () => ({
          policies: await api.listPolicies(session.sessionId, schema, table),
        }),
      };
      patchTab<StructureTabState>(tabId, {
        loading: true,
        error: undefined,
        connectionLost: undefined,
      });
      try {
        const loaded = await fetch[section]();
        if (ctx.staleLoad(tabId, seq)) return; // уже запрошена другая секция
        patchTab<StructureTabState>(tabId, { loading: false, ...loaded });
      } catch (e) {
        const message = ctx.handleSqlError(tab.profileId, e);
        if (ctx.staleLoad(tabId, seq)) return;
        patchTab<StructureTabState>(tabId, {
          loading: false,
          error: message,
          connectionLost: isSessionLost(e),
        });
      }
    },

    runDdl: async (tabId, sql) => {
      const tab = tabOf(tabId, "structure");
      if (!tab) return false;
      const session = ctx.sessionFor(tab.profileId);
      if (!session) return false;
      if (!(await ctx.confirmProdRun(tab.profileId, sql))) return false;
      const { schema, table } = tab.state;
      try {
        await api.executeSql(session.sessionId, sql, 10);
      } catch (e) {
        get().showToast(ctx.handleSqlError(tab.profileId, e));
        return false;
      }
      // sidebar column cache is stale now
      invalidateColumns(tab.profileId, schema, table);
      get().showToast("Applied", "success");
      await get().refreshStructure(tabId);
      return true;
    },

    stageColumnEdit: (tabId, column, patch) => {
      const tab = tabOf(tabId, "structure");
      const orig = tab?.state.columns?.find((c) => c.name === column);
      if (!tab || !orig) return;
      // merge, then drop fields staged back to their original values
      const cur = { ...tab.state.colEdits[column], ...patch };
      if (cur.name === orig.name) delete cur.name;
      if (cur.type === orig.dataType) delete cur.type;
      if (cur.nullable === orig.nullable) delete cur.nullable;
      if (cur.default === (orig.defaultExpr ?? "")) delete cur.default;
      if (cur.comment === (orig.comment ?? "")) delete cur.comment;
      const colEdits = { ...tab.state.colEdits };
      if (Object.keys(cur).length > 0) colEdits[column] = cur;
      else delete colEdits[column];
      patchTab<StructureTabState>(tabId, { colEdits });
    },

    toggleColumnDrop: (tabId, column) => {
      const tab = tabOf(tabId, "structure");
      if (!tab) return;
      const colDrops = tab.state.colDrops.includes(column)
        ? tab.state.colDrops.filter((c) => c !== column)
        : [...tab.state.colDrops, column];
      patchTab<StructureTabState>(tabId, { colDrops });
    },

    stageColumnAdd: (tabId, col) => {
      const tab = tabOf(tabId, "structure");
      if (!tab) return;
      patchTab<StructureTabState>(tabId, {
        colAdds: [...tab.state.colAdds, col],
      });
    },

    unstageColumnAdd: (tabId, index) => {
      const tab = tabOf(tabId, "structure");
      if (!tab) return;
      patchTab<StructureTabState>(tabId, {
        colAdds: tab.state.colAdds.filter((_, i) => i !== index),
      });
    },

    discardStructureEdits: (tabId) => {
      const tab = tabOf(tabId, "structure");
      if (!tab) return;
      if (
        Object.keys(tab.state.colEdits).length === 0 &&
        tab.state.colDrops.length === 0 &&
        tab.state.colAdds.length === 0
      ) {
        return;
      }
      patchTab<StructureTabState>(tabId, noStructureEdits());
      get().showToast("Pending changes discarded", "info");
    },

    applyStructureEdits: async (tabId) => {
      const tab = tabOf(tabId, "structure");
      if (!tab || tab.state.loading) return;
      const stmts = buildStructureDdl(tab.state);
      if (stmts.length === 0) return;
      const session = ctx.sessionFor(tab.profileId);
      if (!session) return;
      if (!(await ctx.confirmProdRun(tab.profileId, stmts.join(";\n")))) return;
      patchTab<StructureTabState>(tabId, { loading: true });
      // One simple-query message = one implicit transaction: atomic, and an
      // error auto-rolls-back without leaving the session in an aborted tx.
      try {
        await api.executeSql(session.sessionId, stmts.join(";\n"), 10);
      } catch (e) {
        patchTab<StructureTabState>(tabId, { loading: false });
        // rolled back, edits stay staged
        get().showToast(ctx.handleSqlError(tab.profileId, e));
        return;
      }
      // clear staged + drop the stale sidebar column cache before reloading
      invalidateColumns(tab.profileId, tab.state.schema, tab.state.table);
      patchTab<StructureTabState>(tabId, noStructureEdits());
      get().showToast(`Applied ${stmts.length} change(s)`, "success");
      await get().refreshStructure(tabId);
    },
  };
}
