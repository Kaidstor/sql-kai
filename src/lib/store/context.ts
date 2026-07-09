// Cross-slice closures: helpers that more than one slice needs. Created once
// per store in index.ts and handed to every slice factory. Helpers used by a
// single slice stay private to that slice's file.
import type { StoreApi } from "zustand";
import { api, errText, isSessionLost } from "../api";
import { restoreWorkspace } from "../persist";
import { dangerousStatements } from "../sql";
import type { SessionInfo } from "../types";
import { patchState, without } from "./helpers";
import type { AppStore, Tab } from "./types";

export type Set = StoreApi<AppStore>["setState"];
export type Get = StoreApi<AppStore>["getState"];

export interface StoreContext {
  /** Tab by id, narrowed to the given kind; null when missing or another kind. */
  tabOf: <K extends Tab["state"]["kind"]>(
    tabId: string,
    kind: K,
  ) => (Tab & { state: Extract<Tab["state"], { kind: K }> }) | null;
  /** Immutably merges a patch into one tab's state (caller vouches for the kind). */
  patchTab: <S extends Tab["state"]>(tabId: string, patch: Partial<S>) => void;
  /** Session for a profile; toasts "Not connected" and returns null if absent. */
  sessionFor: (profileId: string) => SessionInfo | null;
  /** A failed call whose error means the session died (not a bad query):
   *  drop it so the sidebar flips to "connection lost" and every entry
   *  point offers Reconnect. Tabs stay — reconnect restores them live. */
  noteSessionLost: (profileId: string, e: unknown) => void;
  /** Shared tail for every backend/SQL call's catch block: derive the message,
   *  mark the profile's session lost if the connection dropped, and return the
   *  message for the caller to surface (a tab error or a toast). */
  handleSqlError: (profileId: string, e: unknown) => string;
  /** Production write-guard: true = go ahead (not production, nothing
   *  data-modifying in the SQL, or the user confirmed). */
  confirmProdRun: (profileId: string, sql: string) => boolean;
  /** Монотонные номера загрузок по табам: устаревший ответ (медленная
   *  страница, обогнанная следующим запросом) молча отбрасывается. */
  nextLoadSeq: (tabId: string) => number;
  staleLoad: (tabId: string, seq: number) => boolean;
  /** Restores the persisted workspace for a profile; false if nothing restored.
   *  Tab data is NOT fetched here — tabs load lazily on first focus. */
  restoreProfileTabs: (profileId: string) => boolean;
}

export function createStoreContext(set: Set, get: Get): StoreContext {
  const loadSeq: Record<string, number> = {};

  const noteSessionLost = (profileId: string, e: unknown) => {
    if (!isSessionLost(e)) return;
    const session = get().sessions[profileId];
    if (!session) return; // already marked (or explicitly disconnected)
    const profile = get().profiles.find((p) => p.id === profileId);
    api
      .logEvent(`"${profile?.name ?? profileId}": session lost — ${errText(e)}`)
      .catch(() => {});
    // free the backend half (tunnel included); it may already be gone
    api.disconnectSession(session.sessionId).catch(() => {});
    set((s) => ({
      sessions: without(s.sessions, profileId),
      lost: { ...s.lost, [profileId]: true },
    }));
  };

  return {
    tabOf: (tabId, kind) => {
      const tab = get().tabs.find((t) => t.id === tabId);
      return tab && tab.state.kind === kind
        ? (tab as Tab & { state: Extract<Tab["state"], { kind: typeof kind }> })
        : null;
    },

    patchTab: (tabId, patch) =>
      set((s) => ({ tabs: patchState(s.tabs, tabId, patch) })),

    sessionFor: (profileId) => {
      const session = get().sessions[profileId];
      if (!session) get().showToast("Not connected");
      return session ?? null;
    },

    noteSessionLost,

    handleSqlError: (profileId, e) => {
      noteSessionLost(profileId, e);
      return errText(e);
    },

    confirmProdRun: (profileId, sql) => {
      const profile = get().profiles.find((p) => p.id === profileId);
      if (!profile?.production) return true;
      const dangers = dangerousStatements(sql);
      if (dangers.length === 0) return true;
      const list = dangers.map((d) => `• ${d.label}:  ${d.preview}`).join("\n");
      return confirm(
        `"${profile.name}" is PRODUCTION. About to run:\n\n${list}\n\nContinue?`,
      );
    },

    nextLoadSeq: (tabId) => (loadSeq[tabId] = (loadSeq[tabId] ?? 0) + 1),
    staleLoad: (tabId, seq) => loadSeq[tabId] !== seq,

    restoreProfileTabs: (profileId) => {
      const restored = restoreWorkspace(profileId);
      if (!restored) return false;
      set((s) => ({
        tabs: [...s.tabs, ...restored.tabs],
        // only the visible (active) connection may claim the active tab —
        // re-adopting several sessions must not focus a background one
        activeTabId:
          s.activeProfileId === profileId
            ? (restored.activeId ??
              restored.tabs[restored.tabs.length - 1]?.id ??
              s.activeTabId)
            : s.activeTabId,
      }));
      return true;
    },
  };
}
