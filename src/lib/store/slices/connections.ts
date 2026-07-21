// Profiles and their live connections: connect/disconnect/reconnect, the
// session maps, per-profile metadata caches (tables, autocomplete columns,
// sidebar column/FK caches) and the broker's cli-session badges.
import { api, errText } from "../../api";
import { persistClosedTabs, persistWorkspace, removeWorkspace } from "../../persist";
import { connectedProfiles } from "../../profile";
import type {
  CliSessionInfo,
  ColumnInfo,
  EnumTypeInfo,
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
  /** Profiles scheduled for deletion — the launcher card shows an Undo
   *  countdown while the flag is set; the real delete fires after the grace
   *  period unless undoDeleteProfile cancels it. */
  pendingDelete: Record<string, boolean>;
  tables: Record<string, TableInfo[]>; // keyed by profileId
  /** All columns of all relations for editor autocomplete, keyed by profileId. */
  schemaColumns: Record<string, TableColumns[]>;
  /** User functions for the symbols palette, fetched on first open. */
  schemaFunctions: Record<string, FunctionInfo[]>;
  /** Enum types with labels (filter-value suggestions), fetched on first use. */
  schemaEnums: Record<string, EnumTypeInfo[]>;
  activeProfileId: string | null;
  /** Broker-owned sql-kai sessions by profileId — the "cli" badges. */
  cliSessions: Record<string, CliSessionInfo>;
  /** Lazy-loaded columns for the sidebar tree, keyed `profileId|schema|table`. */
  tableColumns: Record<string, ColumnInfo[]>;
  /** Lazy-loaded foreign keys per relation (FK navigation), same keying. */
  tableRelations: Record<string, RelationInfo[]>;

  connect: (profileId: string, activate?: boolean) => Promise<void>;
  disconnect: (profileId: string) => Promise<void>;
  /** Пуш с бэкенда (session://lost): соединение умерло — сразу убрать сессию
   *  и зажечь Reconnect, не дожидаясь, пока следующий запрос наткнётся на
   *  труп. Изолированная сессия просто отвязывается от своего таба. */
  markSessionLost: (sessionId: string, profileId: string) => void;
  /** Re-dials the profile in place: tabs survive (unlike disconnect→connect),
   *  and tabs that errored with the dead session reload once connected. */
  reconnect: (profileId: string, activate?: boolean) => Promise<void>;
  /** Окно получило фокус (⌘Tab обратно в приложение) — молча передёрнуть все
   *  lost-профили в фоне; per-profile кулдаун, чтобы частые ⌘Tab при лежащем
   *  сервере не долбили его и не сыпали тостами об ошибке. */
  autoReconnectLost: () => void;
  selectProfile: (profileId: string) => void;
  /** ⌘`/⌘~ cycle the active connection through the live ones (wraps around). */
  cycleProfile: (dir: 1 | -1) => void;
  refreshTables: (profileId: string) => Promise<void>;
  /** Re-pulls broker cli-sessions (on load and broker://changed events). */
  refreshCliSessions: () => Promise<void>;
  /** Fetches the profile's functions once (symbols palette data). */
  loadSchemaFunctions: (profileId: string) => Promise<void>;
  /** Fetches the profile's enum types once (filter-value suggestions). */
  loadSchemaEnums: (profileId: string) => Promise<void>;
  saveProfile: (
    profile: Profile,
    password: string | null,
    sshPassphrase: string | null,
  ) => Promise<void>;
  /** Marks the profile for deletion and starts the undo grace timer. */
  requestDeleteProfile: (id: string) => void;
  /** Cancels a pending deletion within the grace period. */
  undoDeleteProfile: (id: string) => void;
  deleteProfile: (id: string) => Promise<void>;
  duplicateProfile: (id: string) => Promise<void>;
  /** Re-reads the profile list from disk — sql-kai discover/rm changed it
   *  (triggered by the profiles://changed broker event). */
  reloadProfiles: () => Promise<void>;
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

/** Grace period between "Delete" and the profile actually going away. */
export const UNDO_DELETE_MS = 5000;

// Pending-delete timers live outside the store (like the toast timer):
// the card that started one may unmount before it fires.
const deleteTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** Минимальный интервал между фокус-авто-реконнектами одного профиля. */
const AUTO_RECONNECT_COOLDOWN_MS = 15_000;
const autoReconnectAt = new Map<string, number>();

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
    pendingDelete: {},
    tables: {},
    schemaColumns: {},
    schemaFunctions: {},
    schemaEnums: {},
    activeProfileId: null,
    cliSessions: {},
    tableColumns: {},
    tableRelations: {},

    connect: async (profileId, activate = true) => {
      // повторный вход (двойной Enter в палитре/лаунчере) — открыл бы вторую
      // сессию и ssh-туннель, а первая повисла бы навсегда
      if (get().connecting[profileId]) return;
      // профиль в grace-периоде undo-удаления: коннект открыл бы туннель,
      // который через секунды снесёт deleteProfile
      if (get().pendingDelete[profileId]) return;
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
            // mirror the backend's last-connected mark without a reload
            profiles: s.profiles.map((p) =>
              p.id === profileId
                ? { ...p, lastConnected: { ...p.lastConnected, gui: Date.now() } }
                : p,
            ),
            ...(activate
              ? { activeProfileId: profileId, launcherOpen: false }
              : {}),
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
          !ctx.restoreProfileTabs(profileId) &&
          activate
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

    markSessionLost: (sessionId, profileId) => {
      const s = get();
      // изолированная сессия: отвязать от таба — следующий Run переоткроет
      if (s.isolatedSessions[sessionId]) {
        set((st) => ({
          isolatedSessions: without(st.isolatedSessions, sessionId),
          tabs: st.tabs.map((t) =>
            t.state.kind === "query" && t.state.sessionId === sessionId
              ? { ...t, state: { ...t.state, sessionId: undefined } }
              : t,
          ),
        }));
        return;
      }
      const session = s.sessions[profileId];
      // сессии уже нет (ошибка запроса успела раньше) или профиль уже
      // переподключён свежей — событие относится к прошлому, молчим
      if (!session || session.sessionId !== sessionId) return;
      set((st) => ({
        sessions: without(st.sessions, profileId),
        lost: { ...st.lost, [profileId]: true },
      }));
      const name = s.profiles.find((p) => p.id === profileId)?.name ?? profileId;
      get().showToast(`"${name}": connection lost`);
      // A CLI session may already be alive for this profile. Refreshing here
      // covers that ordering too (the broker's creation event may have arrived
      // before the GUI wire died).
      void get().refreshCliSessions();
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
        // never jump to another connection's tab — the bar only shows the
        // active connection's
        const nextActive = {
          activeProfileId: s.activeProfileId,
          activeTabId: tabs.some((t) => t.id === s.activeTabId)
            ? s.activeTabId
            : null,
        };
        if (s.activeProfileId === profileId) {
          // the active connection is going away — promote its neighbour in the
          // switcher order (same list as Ctrl+1…9) and bring that connection's
          // most recent tab forward; with no live connections left the
          // launcher takes over via hasWorkspace. launcherOpen stays untouched
          // so disconnecting from the launcher doesn't yank the user out of it.
          const list = connectedProfiles(s.profiles, s.sessions);
          const idx = list.findIndex((p) => p.id === profileId);
          const rest = list.filter((p) => p.id !== profileId);
          const next = rest[Math.min(Math.max(idx, 0), rest.length - 1)];
          const own = tabs.filter((t) => t.profileId === next?.id);
          nextActive.activeProfileId = next?.id ?? null;
          nextActive.activeTabId = own[own.length - 1]?.id ?? null;
        }
        return {
          sessions: without(s.sessions, profileId),
          lost: without(s.lost, profileId),
          connectError: without(s.connectError, profileId),
          tables: without(s.tables, profileId),
          schemaColumns: without(s.schemaColumns, profileId),
          schemaFunctions: without(s.schemaFunctions, profileId),
          schemaEnums: without(s.schemaEnums, profileId),
          tableColumns: omitBy(s.tableColumns, byProfile),
          tableRelations: omitBy(s.tableRelations, byProfile),
          tabs,
          ...nextActive,
        };
      });
    },

    reconnect: async (profileId, activate = true) => {
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
      await get().connect(profileId, activate);
    },

    autoReconnectLost: () => {
      const s = get();
      const now = Date.now();
      for (const profileId of Object.keys(s.lost)) {
        if (s.connecting[profileId] || s.pendingDelete[profileId]) continue;
        if (now - (autoReconnectAt.get(profileId) ?? 0) < AUTO_RECONNECT_COOLDOWN_MS)
          continue;
        autoReconnectAt.set(profileId, now);
        // activate=false: авто-реконнект не должен перетаскивать воркспейс
        // на этот профиль и закрывать лаунчер
        void get().reconnect(profileId, false);
      }
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

    cycleProfile: (dir) => {
      const s = get();
      // same live-connections list (and order) the Ctrl+1…9 hotkeys use
      const list = connectedProfiles(s.profiles, s.sessions);
      if (list.length < 2) return;
      const cur = list.findIndex((p) => p.id === s.activeProfileId);
      const base = cur < 0 ? (dir > 0 ? -1 : 0) : cur;
      const next = (base + dir + list.length) % list.length;
      get().selectProfile(list[next].id);
    },

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

        // GUI and CLI own separate PostgreSQL sessions. When the CLI has
        // successfully reopened a profile whose GUI session was lost, redial
        // the GUI session through the shared SSH ControlMaster as well. Keep it
        // in the background so another active workspace is not stolen.
        const state = get();
        const recovered = list.filter(
          (s) =>
            state.lost[s.profileId] &&
            !state.sessions[s.profileId] &&
            !state.connecting[s.profileId],
        );
        await Promise.all(
          recovered.map((s) => get().reconnect(s.profileId, false)),
        );
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

    loadSchemaEnums: async (profileId) => {
      if (get().schemaEnums[profileId]) return;
      const session = get().sessions[profileId];
      if (!session) return;
      try {
        const enums = await api.listEnums(session.sessionId);
        set((s) => ({
          schemaEnums: { ...s.schemaEnums, [profileId]: enums },
        }));
      } catch {
        // non-fatal: filter-value suggestions just stay off
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

    requestDeleteProfile: (id) => {
      const prev = deleteTimers.get(id);
      if (prev) clearTimeout(prev);
      set((s) => ({ pendingDelete: { ...s.pendingDelete, [id]: true } }));
      deleteTimers.set(
        id,
        setTimeout(() => {
          deleteTimers.delete(id);
          void get().deleteProfile(id);
        }, UNDO_DELETE_MS),
      );
    },

    undoDeleteProfile: (id) => {
      const timer = deleteTimers.get(id);
      if (timer) clearTimeout(timer);
      deleteTimers.delete(id);
      set((s) => ({ pendingDelete: without(s.pendingDelete, id) }));
    },

    deleteProfile: async (id) => {
      try {
        await api.deleteProfile(id);
      } catch (e) {
        set((s) => ({ pendingDelete: without(s.pendingDelete, id) }));
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
          pendingDelete: without(s.pendingDelete, id),
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

    reloadProfiles: async () => {
      try {
        set({ profiles: await api.listProfiles() });
      } catch {
        // не смогли перечитать — список просто останется прежним
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
