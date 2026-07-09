// Profiles and their live connections: connect/disconnect/reconnect, the
// session maps, per-profile metadata caches (tables, autocomplete columns,
// sidebar column/FK caches) and the broker's cli-session badges.
import { api, errText } from "../../api";
import { persistClosedTabs, persistWorkspace, removeWorkspace } from "../../persist";
import type {
  CliSessionInfo,
  ColumnInfo,
  Profile,
  RelationInfo,
  SessionInfo,
  TableColumns,
  TableInfo,
} from "../../types";
import type { Get, Set, StoreContext } from "../context";
import { columnsKey, omitBy, without } from "../helpers";
import type { FunctionInfo } from "../types";

export interface ConnectionsSlice {
  profiles: Profile[];
  sessions: Record<string, SessionInfo>; // keyed by profileId
  /** Per-tab isolated connections, keyed by their sessionId. Separate from
   *  `sessions` (which holds one primary per profile). */
  isolatedSessions: Record<string, SessionInfo>;
  connecting: Record<string, boolean>;
  /** Profiles whose session died underneath us (server/tunnel drop) —
   *  drives the red dot and the Reconnect affordances. */
  lost: Record<string, boolean>;
  /** Last connect attempt that failed (server down, refused, auth) — keyed by
   *  profileId, value is the error text. Cleared when a fresh attempt starts. */
  connectError: Record<string, string>;
  tables: Record<string, TableInfo[]>; // keyed by profileId
  /** All columns of all relations for editor autocomplete, keyed by profileId. */
  schemaColumns: Record<string, TableColumns[]>;
  /** User functions for the symbols palette, fetched on first open. */
  schemaFunctions: Record<string, FunctionInfo[]>;
  activeProfileId: string | null;
  /** Broker-owned kai sessions by profileId — the "cli" badges. */
  cliSessions: Record<string, CliSessionInfo>;
  /** Lazy-loaded columns for the sidebar tree, keyed `profileId|schema|table`. */
  tableColumns: Record<string, ColumnInfo[]>;
  /** Lazy-loaded foreign keys per relation (FK navigation), same keying. */
  tableRelations: Record<string, RelationInfo[]>;

  connect: (profileId: string) => Promise<void>;
  disconnect: (profileId: string) => Promise<void>;
  /** Re-dials the profile in place: tabs survive (unlike disconnect→connect),
   *  and tabs that errored with the dead session reload once connected. */
  reconnect: (profileId: string) => Promise<void>;
  selectProfile: (profileId: string) => void;
  refreshTables: (profileId: string) => Promise<void>;
  /** Re-pulls broker cli-sessions (on load and broker://changed events). */
  refreshCliSessions: () => Promise<void>;
  /** Fetches the profile's functions once (symbols palette data). */
  loadSchemaFunctions: (profileId: string) => Promise<void>;
  saveProfile: (
    profile: Profile,
    password: string | null,
    sshPassphrase: string | null,
  ) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;
  duplicateProfile: (id: string) => Promise<void>;
  loadTableColumns: (
    profileId: string,
    schema: string,
    table: string,
  ) => Promise<void>;
  loadTableRelations: (
    profileId: string,
    schema: string,
    table: string,
  ) => Promise<void>;
}

export function createConnectionsSlice(
  set: Set,
  get: Get,
  ctx: StoreContext,
): ConnectionsSlice {
  /** Disconnects every isolated session of a profile and detaches it from its
   *  tab — used on profile disconnect/reconnect where the tunnel goes away. */
  const dropProfileIsolatedSessions = (profileId: string) => {
    const s = get();
    const own = Object.values(s.isolatedSessions).filter(
      (iso) => iso.profileId === profileId,
    );
    if (own.length === 0) return;
    const ids = new Set(own.map((iso) => iso.sessionId));
    for (const iso of own) api.disconnectSession(iso.sessionId).catch(() => {});
    set((st) => ({
      isolatedSessions: Object.fromEntries(
        Object.entries(st.isolatedSessions).filter(([id]) => !ids.has(id)),
      ),
      tabs: st.tabs.map((t) =>
        t.state.kind === "query" && t.state.sessionId && ids.has(t.state.sessionId)
          ? { ...t, state: { ...t.state, sessionId: undefined } }
          : t,
      ),
    }));
  };

  return {
    profiles: [],
    sessions: {},
    isolatedSessions: {},
    connecting: {},
    lost: {},
    connectError: {},
    tables: {},
    schemaColumns: {},
    schemaFunctions: {},
    activeProfileId: null,
    cliSessions: {},
    tableColumns: {},
    tableRelations: {},

    connect: async (profileId) => {
      // повторный вход (двойной Enter в палитре/лаунчере) — открыл бы вторую
      // сессию и ssh-туннель, а первая повисла бы навсегда
      if (get().connecting[profileId]) return;
      // a fresh attempt clears the last failure
      set((s) => ({
        connecting: { ...s.connecting, [profileId]: true },
        connectError: without(s.connectError, profileId),
      }));
      try {
        const info = await api.connectProfile(profileId);
        set((s) => {
          return {
            sessions: { ...s.sessions, [profileId]: info },
            activeProfileId: profileId,
            launcherOpen: false,
            lost: without(s.lost, profileId),
            // Tabs that errored with the dead session: clear the error (and the
            // stale payload) in the same commit the session appears, so their
            // lazy-load effect refetches on this very render.
            tabs: s.tabs.map((t) => {
              if (t.profileId !== profileId || !t.state.connectionLost) {
                return t;
              }
              if (t.state.kind === "table") {
                return {
                  ...t,
                  state: {
                    ...t.state,
                    error: undefined,
                    connectionLost: undefined,
                    data: undefined,
                  },
                };
              }
              if (t.state.kind === "structure") {
                return {
                  ...t,
                  state: {
                    ...t.state,
                    error: undefined,
                    connectionLost: undefined,
                    columns: undefined,
                    indexes: undefined,
                    relations: undefined,
                    triggers: undefined,
                  },
                };
              }
              return {
                ...t,
                state: { ...t.state, error: undefined, connectionLost: undefined },
              };
            }),
          };
        });
        await get().refreshTables(profileId);
        if (
          !get().tabs.some((t) => t.profileId === profileId) &&
          !ctx.restoreProfileTabs(profileId)
        ) {
          get().openQueryTab(profileId);
        }
      } catch (e) {
        const message = errText(e);
        // Persist the failure so the launcher card can show "connection failed"
        // instead of it vanishing with the toast.
        set((s) => ({
          connectError: { ...s.connectError, [profileId]: message },
        }));
        get().showToast(message);
      } finally {
        set((s) => ({ connecting: { ...s.connecting, [profileId]: false } }));
      }
    },

    disconnect: async (profileId) => {
      persistWorkspace(get(), profileId); // snapshot before tabs are dropped
      dropProfileIsolatedSessions(profileId); // tear down its isolated tabs' conns
      const session = get().sessions[profileId];
      if (session) {
        try {
          await api.disconnectSession(session.sessionId);
        } catch {
          // session may already be gone server-side
        }
      }
      set((s) => {
        const byProfile = (key: string) => key.startsWith(`${profileId}|`);
        const tabs = s.tabs.filter((t) => t.profileId !== profileId);
        return {
          sessions: without(s.sessions, profileId),
          lost: without(s.lost, profileId),
          connectError: without(s.connectError, profileId),
          tables: without(s.tables, profileId),
          schemaColumns: without(s.schemaColumns, profileId),
          schemaFunctions: without(s.schemaFunctions, profileId),
          tableColumns: omitBy(s.tableColumns, byProfile),
          tableRelations: omitBy(s.tableRelations, byProfile),
          tabs,
          // never jump to another connection's tab — the bar only shows the
          // active connection, which just lost all of its tabs
          activeTabId: tabs.some((t) => t.id === s.activeTabId)
            ? s.activeTabId
            : null,
        };
      });
    },

    reconnect: async (profileId) => {
      if (get().connecting[profileId]) return;
      const session = get().sessions[profileId];
      if (session) {
        // drop the old session but keep the tabs (unlike disconnect)
        set((s) => ({ sessions: without(s.sessions, profileId) }));
        try {
          await api.disconnectSession(session.sessionId);
        } catch {
          // it may already be gone server-side
        }
      }
      // isolated connections died with the tunnel; clear them so isolated tabs
      // reopen lazily on the fresh connection
      dropProfileIsolatedSessions(profileId);
      await get().connect(profileId);
    },

    selectProfile: (profileId) =>
      set((s) => {
        // the tabs bar shows only this connection's tabs — bring its most
        // recent one forward (none when the connection has no tabs yet)
        const activeTab = s.tabs.find((t) => t.id === s.activeTabId);
        let activeTabId = s.activeTabId;
        if (!activeTab || activeTab.profileId !== profileId) {
          const own = s.tabs.filter((t) => t.profileId === profileId);
          activeTabId = own[own.length - 1]?.id ?? null;
        }
        return { activeProfileId: profileId, activeTabId, launcherOpen: false };
      }),

    refreshTables: async (profileId) => {
      const session = get().sessions[profileId];
      if (!session) return;
      // The table list and the autocomplete column dump are independent — fetch
      // them concurrently (allSettled so the non-fatal columns fetch can fail on
      // its own without touching the tables error path).
      const [tablesR, colsR] = await Promise.allSettled([
        api.listTables(session.sessionId),
        api.listAllColumns(session.sessionId),
      ]);
      if (tablesR.status === "fulfilled") {
        set((s) => ({ tables: { ...s.tables, [profileId]: tablesR.value } }));
      } else {
        get().showToast(ctx.handleSqlError(profileId, tablesR.reason));
      }
      // Editor autocomplete data; non-fatal — on failure completion just
      // stays keyword-only.
      if (colsR.status === "fulfilled") {
        set((s) => ({
          schemaColumns: { ...s.schemaColumns, [profileId]: colsR.value },
        }));
      }
    },

    refreshCliSessions: async () => {
      try {
        const list = await api.listCliSessions();
        const cliSessions: Record<string, CliSessionInfo> = {};
        for (const s of list) cliSessions[s.profileId] = s;
        set({ cliSessions });
      } catch {
        // брокера нет (не-unix сборка) — бейджи просто не показываются
      }
    },

    loadSchemaFunctions: async (profileId) => {
      if (get().schemaFunctions[profileId]) return;
      const session = get().sessions[profileId];
      if (!session) return;
      const sql = `SELECT n.nspname, p.proname, pg_get_function_identity_arguments(p.oid)
  FROM pg_proc p JOIN pg_namespace n ON n.oid = p.pronamespace
 WHERE n.nspname NOT IN ('pg_catalog', 'information_schema')
 ORDER BY 1, 2`;
      try {
        const exec = await api.executeSql(session.sessionId, sql, 5000);
        const funcs: FunctionInfo[] = (exec.results[0]?.rows ?? []).map((r) => ({
          schema: r[0] ?? "",
          name: r[1] ?? "",
          args: r[2] ?? "",
        }));
        set((s) => ({
          schemaFunctions: { ...s.schemaFunctions, [profileId]: funcs },
        }));
      } catch {
        // non-fatal: the palette just shows tables/columns only
      }
    },

    saveProfile: async (profile, password, sshPassphrase) => {
      const saved = await api.saveProfile(profile, password, sshPassphrase);
      set((s) => {
        const exists = s.profiles.some((p) => p.id === saved.id);
        return {
          profiles: exists
            ? s.profiles.map((p) => (p.id === saved.id ? saved : p))
            : [...s.profiles, saved],
          dialog: { open: false },
        };
      });
    },

    deleteProfile: async (id) => {
      try {
        await api.deleteProfile(id);
      } catch (e) {
        get().showToast(errText(e));
        return;
      }
      removeWorkspace(id);
      set((s) => {
        // backend delete_profile already dropped its (isolated) sessions
        const isolatedSessions = Object.fromEntries(
          Object.entries(s.isolatedSessions).filter(
            ([, iso]) => iso.profileId !== id,
          ),
        );
        const tabs = s.tabs.filter((t) => t.profileId !== id);
        return {
          profiles: s.profiles.filter((p) => p.id !== id),
          sessions: without(s.sessions, id),
          isolatedSessions,
          lost: without(s.lost, id),
          connectError: without(s.connectError, id),
          tabs,
          closedTabs: s.closedTabs.filter((c) => c.tab.profileId !== id),
          activeProfileId: s.activeProfileId === id ? null : s.activeProfileId,
          activeTabId: tabs.some((t) => t.id === s.activeTabId)
            ? s.activeTabId
            : null,
        };
      });
      persistClosedTabs(get().closedTabs);
    },

    duplicateProfile: async (id) => {
      try {
        await api.duplicateProfile(id);
        // reload the whole list — the original may have gained a group
        const profiles = await api.listProfiles();
        set({ profiles });
        get().showToast("Connection duplicated", "info");
      } catch (e) {
        get().showToast(errText(e));
      }
    },

    loadTableColumns: async (profileId, schema, table) => {
      const key = columnsKey(profileId, schema, table);
      if (get().tableColumns[key]) return;
      const session = get().sessions[profileId];
      if (!session) return;
      try {
        const cols = await api.listColumns(session.sessionId, schema, table);
        set((s) => ({ tableColumns: { ...s.tableColumns, [key]: cols } }));
      } catch (e) {
        get().showToast(ctx.handleSqlError(profileId, e));
      }
    },

    loadTableRelations: async (profileId, schema, table) => {
      const key = columnsKey(profileId, schema, table);
      if (get().tableRelations[key]) return;
      const session = get().sessions[profileId];
      if (!session) return;
      try {
        const rels = await api.listRelations(session.sessionId, schema, table);
        set((s) => ({ tableRelations: { ...s.tableRelations, [key]: rels } }));
      } catch {
        // non-fatal: FK navigation just stays off for this table
      }
    },
  };
}
