// Saved queries (shared with the CLI via queries.json) and the run history.
import { api, errText } from "../../api";
import type { HistoryEntry, SavedQuery } from "../../types";
import type { Get, Set, StoreContext } from "../context";
import type { QueryTabState } from "../types";

export interface SavedSlice {
  queries: SavedQuery[];
  history: HistoryEntry[];

  /** scope: null = global collection, otherwise queryScopeOf(profile).
   *  id: overwrite that saved query; empty/omitted = upsert by name+scope. */
  saveQuery: (
    name: string,
    sql: string,
    scope: string | null,
    id?: string,
  ) => Promise<SavedQuery | null>;
  deleteQuery: (id: string) => Promise<void>;
  /** Links a query tab to a saved query: retitles it, ⌘S now overwrites. */
  linkQueryTab: (tabId: string, q: SavedQuery) => void;
  /** Loads a saved query into the active empty query tab or a new one. */
  openSavedQuery: (profileId: string, q: SavedQuery) => void;
  /** ⌘S on a query tab: overwrite the linked saved query or ask for a name. */
  saveQueryTab: (tabId: string) => Promise<void>;
  deleteHistoryEntry: (id: string) => void;
  clearHistory: () => void;
}

export function createSavedSlice(set: Set, get: Get, ctx: StoreContext): SavedSlice {
  return {
    queries: [],
    history: [],

    saveQuery: async (name, sql, scope, id) => {
      try {
        const saved = await api.saveQuery({ id: id ?? "", name, sql, scope });
        set((s) => ({
          queries: [...s.queries.filter((q) => q.id !== saved.id), saved],
        }));
        get().showToast(`Saved "${saved.name}"`, "info");
        return saved;
      } catch (e) {
        get().showToast(errText(e));
        return null;
      }
    },

    deleteQuery: async (id) => {
      try {
        await api.deleteQuery(id);
        set((s) => ({ queries: s.queries.filter((q) => q.id !== id) }));
      } catch (e) {
        get().showToast(errText(e));
      }
    },

    linkQueryTab: (tabId, q) =>
      set((s) => ({
        tabs: s.tabs.map((t) =>
          t.id === tabId && t.state.kind === "query"
            ? { ...t, title: q.name, state: { ...t.state, savedQueryId: q.id } }
            : t,
        ),
      })),

    openSavedQuery: (profileId, q) => {
      const s = get();
      const active = s.tabs.find((t) => t.id === s.activeTabId);
      // don't clobber unsaved SQL — reuse the tab only if it's empty (or same)
      if (
        active &&
        active.profileId === profileId &&
        active.state.kind === "query" &&
        (!active.state.sql.trim() || active.state.sql === q.sql)
      ) {
        ctx.patchTab<QueryTabState>(active.id, { sql: q.sql });
        get().linkQueryTab(active.id, q);
      } else {
        get().openQueryTab(profileId, q.sql, q.name, q.id);
      }
    },

    saveQueryTab: async (tabId) => {
      const tab = ctx.tabOf(tabId, "query");
      if (!tab) return;
      const st = tab.state;
      if (!st.sql.trim()) return;
      const existing = st.savedQueryId
        ? get().queries.find((q) => q.id === st.savedQueryId)
        : undefined;
      if (!existing) {
        // new (or link went stale) — ask for a name first
        set({ saveDialogFor: tabId });
        return;
      }
      const saved = await get().saveQuery(
        existing.name,
        st.sql,
        existing.scope ?? null,
        existing.id,
      );
      if (saved) get().linkQueryTab(tabId, saved);
    },

    deleteHistoryEntry: (id) => {
      set((s) => ({ history: s.history.filter((h) => h.id !== id) }));
      api.deleteHistoryEntry(id).catch((e) => get().showToast(errText(e)));
    },

    clearHistory: () => {
      set({ history: [] });
      api.clearHistory().catch((e) => get().showToast(errText(e)));
    },
  };
}
