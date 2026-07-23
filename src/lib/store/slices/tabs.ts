// Tab lifecycle: opening (query/table/structure/activity), closing with the
// ⌘⇧T reopen stack, drag reorder, and the small per-tab field setters.
import { api } from "../../api";
import {
  CLOSED_CAP,
  loadClosedTabs,
  persistClosedTabs,
  type ClosedTab,
} from "../../persist";
import type { Get, Set, StoreContext } from "../context";
import {
  nextQueryTitle,
  noStructureEdits,
  noTableEdits,
} from "../helpers";
import type { QueryTabState, Tab } from "../types";

export interface TabsSlice {
  tabs: Tab[];
  activeTabId: string | null;
  /** Recently closed tabs (⌘W), restorable with ⌘⇧T; persisted across runs. */
  closedTabs: ClosedTab[];

  openQueryTab: (
    profileId: string,
    sql?: string,
    title?: string,
    savedQueryId?: string,
  ) => void;
  /** filter (raw WHERE) applies to a fresh tab and refocuses+refilters an
   *  existing one — FK navigation lands on the referenced row(s). */
  openTableTab: (
    profileId: string,
    schema: string,
    table: string,
    filter?: string,
  ) => void;
  openStructureTab: (profileId: string, schema: string, table: string) => void;
  /** Server activity monitor (pg_stat_activity) — one tab per connection. */
  openActivityTab: (profileId: string) => void;
  closeTab: (tabId: string) => void;
  /** Mass close (context menu / ⌘K ⌘W); all go to the reopen stack (capped). */
  closeTabs: (ids: string[]) => void;
  /** ⌘W: closes the palette/dialog if open, otherwise the active tab. */
  closeActiveTab: () => void;
  /** ⌘⇧T: restores the most recently closed tab. */
  reopenClosedTab: () => void;
  /** ⌘N: new query tab on the active connection (or the ⌘⌥O palette). */
  newQueryTab: () => void;
  setActiveTab: (tabId: string) => void;
  /** Ctrl+Tab / ⌘⇧] step through the active connection's tabs (wraps around). */
  cycleTab: (dir: 1 | -1) => void;
  /** Drag reorder: puts dragId before targetId (or after it when `after`). */
  moveTab: (dragId: string, targetId: string, after: boolean) => void;
  setTabSql: (tabId: string, sql: string) => void;
  setTabMaxRows: (tabId: string, maxRows: number) => void;
  setTabEditorPct: (tabId: string, editorPct: number) => void;
}

export function createTabsSlice(set: Set, get: Get, ctx: StoreContext): TabsSlice {
  /** Data/structure tabs are per-relation singletons: focus if open, else create. */
  const openRelationTab = (
    kind: "table" | "structure",
    profileId: string,
    schema: string,
    table: string,
    filter?: string,
  ) => {
    const existing = get().tabs.find(
      (t) =>
        t.profileId === profileId &&
        t.state.kind === kind &&
        t.state.schema === schema &&
        t.state.table === table,
    );
    if (existing) {
      set({ activeTabId: existing.id, activeProfileId: profileId });
      // FK navigation onto an already-open tab retargets its filter
      if (
        filter !== undefined &&
        existing.state.kind === "table" &&
        existing.state.filter !== filter
      ) {
        void get().refreshTablePage(existing.id, { filter, page: 0 });
      }
      return;
    }
    const name = schema === "public" ? table : `${schema}.${table}`;
    const tab: Tab = {
      id: crypto.randomUUID(),
      profileId,
      title: kind === "table" ? name : `${name} ⚙`,
      state:
        kind === "table"
          ? {
              kind: "table",
              schema,
              table,
              page: 0,
              pageSize: 100,
              sorts: [],
              filter: filter ?? "",
              loading: false,
              ...noTableEdits(),
            }
          : {
              kind: "structure",
              schema,
              table,
              section: "columns",
              loading: false,
              ...noStructureEdits(),
            },
    };
    // no explicit fetch: the tab becomes active and loads itself on mount
    set((s) => ({
      tabs: [...s.tabs, tab],
      activeTabId: tab.id,
      activeProfileId: profileId,
    }));
  };

  return {
    tabs: [],
    activeTabId: null,
    closedTabs: loadClosedTabs(),

    openQueryTab: (profileId, sql = "", title, savedQueryId) => {
      const tab: Tab = {
        id: crypto.randomUUID(),
        profileId,
        title: title || nextQueryTitle(get()),
        state: { kind: "query", sql, running: false, maxRows: 1000, savedQueryId },
      };
      set((s) => ({
        tabs: [...s.tabs, tab],
        activeTabId: tab.id,
        activeProfileId: profileId,
      }));
    },

    openTableTab: (profileId, schema, table, filter) =>
      openRelationTab("table", profileId, schema, table, filter),

    openStructureTab: (profileId, schema, table) =>
      openRelationTab("structure", profileId, schema, table),

    openActivityTab: (profileId) => {
      const existing = get().tabs.find(
        (t) => t.profileId === profileId && t.state.kind === "activity",
      );
      if (existing) {
        set({ activeTabId: existing.id, activeProfileId: profileId });
        return;
      }
      const tab: Tab = {
        id: crypto.randomUUID(),
        profileId,
        title: "Activity",
        state: {
          kind: "activity",
          loading: false,
          refreshSec: 5,
          includeIdle: false,
        },
      };
      set((s) => ({
        tabs: [...s.tabs, tab],
        activeTabId: tab.id,
        activeProfileId: profileId,
      }));
    },

    closeTab: (tabId) => get().closeTabs([tabId]),

    closeTabs: (ids) => {
      const idSet = new Set(ids);
      const closing = get().tabs.filter((t) => idSet.has(t.id));
      const count = closing.length;
      if (count === 0) return;
      // Tear down isolated connections owned by the closing tabs (disconnect
      // rolls back any open transaction on them).
      const isoIds = new Set<string>();
      for (const t of closing) {
        if (t.state.kind === "query" && t.state.sessionId) {
          isoIds.add(t.state.sessionId);
        }
      }
      if (isoIds.size > 0) {
        for (const sid of isoIds) api.disconnectSession(sid).catch(() => {});
        set((s) => ({
          isolatedSessions: Object.fromEntries(
            Object.entries(s.isolatedSessions).filter(([id]) => !isoIds.has(id)),
          ),
        }));
      }
      set((s) => {
        const closedEntries = s.tabs
          .map((tab, index) => ({ tab, index }))
          .filter(({ tab }) => idSet.has(tab.id))
          .map(({ tab, index }) => {
            // snapshot with transient bits reset so ⌘⇧T restores a sane tab
            const state: Tab["state"] =
              tab.state.kind === "query"
                ? { ...tab.state, running: false, sessionId: undefined }
                : tab.state.kind === "table"
                  ? { ...tab.state, loading: false, ...noTableEdits() }
                  : tab.state.kind === "structure"
                    ? { ...tab.state, loading: false, ...noStructureEdits() }
                    : { ...tab.state, loading: false };
            return { tab: { ...tab, state }, index };
          });
        const tabs = s.tabs.filter((t) => !idSet.has(t.id));
        let activeTabId = s.activeTabId;
        if (activeTabId !== null && idSet.has(activeTabId)) {
          // nearest surviving neighbour on the SAME connection (only its tabs
          // are visible): first to the right, else to the left
          const oldIdx = s.tabs.findIndex((t) => t.id === activeTabId);
          const profileId = s.tabs[oldIdx].profileId;
          const survives = (t: Tab) =>
            !idSet.has(t.id) && t.profileId === profileId;
          const next =
            s.tabs.slice(oldIdx + 1).find(survives) ??
            s.tabs.slice(0, oldIdx).reverse().find(survives);
          activeTabId = next?.id ?? null;
        }
        return {
          tabs,
          activeTabId,
          closedTabs: [...s.closedTabs, ...closedEntries].slice(-CLOSED_CAP),
        };
      });
      persistClosedTabs(get().closedTabs);
      if (count > 1) get().showToast(`Closed ${count} tabs`, "info");
    },

    closeActiveTab: () => {
      const s = get();
      // ⌘W dismisses whatever overlay is on top before touching tabs
      if (s.logViewerOpen) {
        set({ logViewerOpen: false });
        return;
      }
      if (s.settingsOpen) {
        set({ settingsOpen: false });
        return;
      }
      if (s.palette) {
        set({ palette: null });
        return;
      }
      if (s.saveDialogFor) {
        set({ saveDialogFor: null });
        return;
      }
      if (s.dialog.open) {
        s.closeDialog();
        return;
      }
      if (s.activeTabId) s.closeTab(s.activeTabId);
    },

    reopenClosedTab: () => {
      const stack = [...get().closedTabs];
      while (stack.length > 0) {
        const { tab, index } = stack.pop()!;
        if (!get().profiles.some((p) => p.id === tab.profileId)) continue; // profile gone
        set((s) => {
          const tabs = [...s.tabs];
          tabs.splice(Math.min(index, tabs.length), 0, tab);
          return {
            tabs,
            closedTabs: stack,
            activeTabId: tab.id,
            activeProfileId: tab.profileId,
          };
        });
        persistClosedTabs(stack);
        // the reopened tab is active now — it refetches missing data on mount
        return;
      }
      set({ closedTabs: stack });
      persistClosedTabs(stack);
    },

    newQueryTab: () => {
      const s = get();
      if (s.activeProfileId && s.sessions[s.activeProfileId]) {
        s.openQueryTab(s.activeProfileId);
      } else {
        // nothing connected — offer a connection instead
        set({ palette: "connections" });
      }
    },

    setActiveTab: (tabId) => set({ activeTabId: tabId }),

    cycleTab: (dir) => {
      const s = get();
      // only the active connection's tabs are on the bar
      const visible = s.tabs.filter((t) => t.profileId === s.activeProfileId);
      if (visible.length === 0) return;
      const cur = visible.findIndex((t) => t.id === s.activeTabId);
      // from outside the visible set, step in from the matching end
      const base = cur < 0 ? (dir > 0 ? -1 : 0) : cur;
      const next = (base + dir + visible.length) % visible.length;
      set({ activeTabId: visible[next].id });
    },

    moveTab: (dragId, targetId, after) =>
      set((s) => {
        if (dragId === targetId) return s;
        const from = s.tabs.findIndex((t) => t.id === dragId);
        let to = s.tabs.findIndex((t) => t.id === targetId);
        if (from < 0 || to < 0) return s;
        if (after) to += 1;
        if (to > from) to -= 1; // the removal below shifts everything left
        if (to === from) return s;
        const tabs = [...s.tabs];
        const [moved] = tabs.splice(from, 1);
        tabs.splice(to, 0, moved);
        return { tabs };
      }),

    setTabSql: (tabId, sql) => ctx.patchTab<QueryTabState>(tabId, { sql }),

    setTabMaxRows: (tabId, maxRows) =>
      ctx.patchTab<QueryTabState>(tabId, { maxRows }),

    setTabEditorPct: (tabId, editorPct) =>
      ctx.patchTab<QueryTabState>(tabId, { editorPct }),
  };
}
