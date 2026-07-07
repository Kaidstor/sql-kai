import { create } from "zustand";
import { api, errText, isConnectionLost } from "./api";
import {
  CLOSED_CAP,
  dropLegacyHistory,
  loadClosedTabs,
  loadLegacyHistory,
  persistClosedTabs,
  persistWorkspace,
  removeWorkspace,
  restoreWorkspace,
  type ClosedTab,
} from "./persist";
import { structureDdl, tableDml } from "./sqlgen";
import { applyTheme } from "./themes";
import type {
  AppSettings,
  ColumnInfo,
  ExecResult,
  HistoryEntry,
  IndexInfo,
  Profile,
  RelationInfo,
  SavedQuery,
  SessionInfo,
  TableColumns,
  TableInfo,
  TablePage,
  TriggerInfo,
  VaultStatus,
} from "./types";

export interface QueryTabState {
  kind: "query";
  sql: string;
  result?: ExecResult;
  error?: string;
  running: boolean;
  maxRows: number;
  /** Editor pane height, % of the editor+results split; 0 = collapsed (results only). */
  editorPct?: number;
  /** Saved query this tab was opened from / saved as — ⌘S overwrites it. */
  savedQueryId?: string;
}

/** Pending INSERT row; values are aligned with result.columns; undefined =
 *  let the DB fill the column (generated keys cut on duplicate). */
export interface InsertRow {
  values: (string | null | undefined)[];
  /** Page row this insert renders under (last row of its duplicate batch);
   *  bottom when absent. */
  after?: number;
}

export interface TableTabState {
  kind: "table";
  schema: string;
  table: string;
  page: number;
  pageSize: number;
  orderBy?: string;
  orderDir: "asc" | "desc";
  data?: TablePage;
  error?: string;
  loading: boolean;
  /** Staged cell edits: row index → column index → new value (null = SQL NULL). */
  edits: Record<number, Record<number, string | null>>;
  /** Row indices staged for deletion. */
  deletes: number[];
  /** Duplicated rows staged for INSERT (not tied to row indices). */
  inserts: InsertRow[];
  /** Last Apply failed (tx rolled back) — staged cells render red. */
  applyFailed?: boolean;
  /** DB error from the last failed Apply — shown as a persistent banner. */
  applyError?: string;
}

export type StructureSection = "columns" | "indexes" | "relations" | "triggers";

/** Staged field changes for one column; only present keys are applied. */
export interface ColumnPatch {
  name?: string;
  type?: string;
  nullable?: boolean;
  /** Raw default expression; "" stages DROP DEFAULT. */
  default?: string;
  /** "" clears the comment. */
  comment?: string;
}

export interface NewColumn {
  name: string;
  type: string;
  nullable: boolean;
  def: string;
}

export interface StructureTabState {
  kind: "structure";
  schema: string;
  table: string;
  section: StructureSection;
  columns?: ColumnInfo[];
  indexes?: IndexInfo[];
  relations?: RelationInfo[];
  triggers?: TriggerInfo[];
  loading: boolean;
  error?: string;
  /** Staged column DDL (Apply runs it in one transaction), keyed by original name. */
  colEdits: Record<string, ColumnPatch>;
  colDrops: string[];
  colAdds: NewColumn[];
}

export interface Tab {
  id: string;
  profileId: string;
  title: string;
  state: QueryTabState | TableTabState | StructureTabState;
}

/** Query tab has SQL that isn't persisted as a saved query (never saved,
 *  or diverged from the linked one) — drives the • dirty mark on tabs. */
export function isQueryTabDirty(tab: Tab, queries: SavedQuery[]): boolean {
  if (tab.state.kind !== "query") return false;
  const { sql, savedQueryId } = tab.state;
  if (!sql.trim()) return false;
  const saved = savedQueryId && queries.find((q) => q.id === savedQueryId);
  return !saved || saved.sql !== sql;
}

interface Toast {
  message: string;
  kind: "error" | "info" | "success";
}

export type PaletteKind = "connections" | "queries";

interface AppStore {
  profiles: Profile[];
  sessions: Record<string, SessionInfo>; // keyed by profileId
  connecting: Record<string, boolean>;
  /** Profiles whose session died underneath us (server/tunnel drop) —
   *  drives the red dot and the Reconnect affordances. */
  lost: Record<string, boolean>;
  tables: Record<string, TableInfo[]>; // keyed by profileId
  /** All columns of all relations for editor autocomplete, keyed by profileId. */
  schemaColumns: Record<string, TableColumns[]>;
  activeProfileId: string | null;
  tabs: Tab[];
  activeTabId: string | null;
  dialog: { open: boolean; profile?: Profile };
  toast: Toast | null;
  queries: SavedQuery[];
  history: HistoryEntry[];
  palette: PaletteKind | null;
  /** Recently closed tabs (⌘W), restorable with ⌘⇧T; persisted across runs. */
  closedTabs: ClosedTab[];
  /** Query tab whose "save query" dialog is open (⌘S on an unsaved query). */
  saveDialogFor: string | null;
  /** Contents of settings.json (theme etc.) — loaded before the vault gate. */
  settings: AppSettings;
  /** Settings dialog (⌘,). */
  settingsOpen: boolean;
  /** Vault gate: null until checked, then whether it exists / is unlocked. */
  vault: VaultStatus | null;
  /** init() failed before the vault state was known — show a retry screen. */
  vaultError: string | null;

  init: () => Promise<void>;
  /** First run: set the master password, create + unlock the vault. */
  setupVault: (password: string, enableBiometric: boolean) => Promise<void>;
  /** Unlock an existing vault with the master password. */
  unlockVault: (password: string, enableBiometric: boolean) => Promise<void>;
  /** Unlock via Touch ID; rejects with "cancelled" when dismissed. */
  unlockVaultBiometric: () => Promise<void>;
  /** Lock the vault: drops in-memory secrets and all live sessions. */
  lockVault: () => Promise<void>;
  setPalette: (palette: PaletteKind | null) => void;
  setSettingsOpen: (open: boolean) => void;
  /** Applies the theme immediately and persists it to settings.json. */
  setTheme: (id: string) => Promise<void>;
  duplicateProfile: (id: string) => Promise<void>;
  deleteHistoryEntry: (id: string) => void;
  clearHistory: () => void;
  /** scope: null = global collection, otherwise queryScopeOf(profile).
   *  id: overwrite that saved query; empty/omitted = upsert by name+scope. */
  saveQuery: (
    name: string,
    sql: string,
    scope: string | null,
    id?: string,
  ) => Promise<SavedQuery | null>;
  deleteQuery: (id: string) => Promise<void>;
  setSaveDialogFor: (tabId: string | null) => void;
  /** Links a query tab to a saved query: retitles it, ⌘S now overwrites. */
  linkQueryTab: (tabId: string, q: SavedQuery) => void;
  /** Loads a saved query into the active empty query tab or a new one. */
  openSavedQuery: (profileId: string, q: SavedQuery) => void;
  /** ⌘S on a query tab: overwrite the linked saved query or ask for a name. */
  saveQueryTab: (tabId: string) => Promise<void>;
  /** ⌘W: closes the palette/dialog if open, otherwise the active tab. */
  closeActiveTab: () => void;
  /** Mass close (context menu / ⌘K ⌘W); all go to the reopen stack (capped). */
  closeTabs: (ids: string[]) => void;
  /** ⌘⇧T: restores the most recently closed tab. */
  reopenClosedTab: () => void;
  /** ⌘N: new query tab on the active connection (or the ⌘⌥O palette). */
  newQueryTab: () => void;
  showToast: (message: string, kind?: Toast["kind"]) => void;
  openDialog: (profile?: Profile) => void;
  closeDialog: () => void;
  saveProfile: (
    profile: Profile,
    password: string | null,
    sshPassphrase: string | null,
  ) => Promise<void>;
  deleteProfile: (id: string) => Promise<void>;
  connect: (profileId: string) => Promise<void>;
  disconnect: (profileId: string) => Promise<void>;
  /** Re-dials the profile in place: tabs survive (unlike disconnect→connect),
   *  and tabs that errored with the dead session reload once connected. */
  reconnect: (profileId: string) => Promise<void>;
  selectProfile: (profileId: string) => void;
  refreshTables: (profileId: string) => Promise<void>;
  openQueryTab: (
    profileId: string,
    sql?: string,
    title?: string,
    savedQueryId?: string,
  ) => void;
  openTableTab: (profileId: string, schema: string, table: string) => void;
  openStructureTab: (profileId: string, schema: string, table: string) => void;
  setStructureSection: (tabId: string, section: StructureSection) => void;
  loadStructure: (tabId: string) => Promise<void>;
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
  /** Lazy-loaded columns for the sidebar tree, keyed `profileId|schema.table`. */
  tableColumns: Record<string, ColumnInfo[]>;
  loadTableColumns: (
    profileId: string,
    schema: string,
    table: string,
  ) => Promise<void>;
  closeTab: (tabId: string) => void;
  setActiveTab: (tabId: string) => void;
  /** Drag reorder: puts dragId before targetId (or after it when `after`). */
  moveTab: (dragId: string, targetId: string, after: boolean) => void;
  setTabSql: (tabId: string, sql: string) => void;
  setTabMaxRows: (tabId: string, maxRows: number) => void;
  setTabEditorPct: (tabId: string, editorPct: number) => void;
  runQuery: (tabId: string) => Promise<void>;
  cancelQuery: (tabId: string) => Promise<void>;
  loadTablePage: (
    tabId: string,
    patch?: Partial<
      Pick<TableTabState, "page" | "pageSize" | "orderBy" | "orderDir">
    >,
  ) => Promise<void>;
  /** Stage a cell value; staging the original value reverts the cell. */
  stageCellEdit: (
    tabId: string,
    row: number,
    col: number,
    value: string | null,
  ) => void;
  toggleRowDeletes: (tabId: string, rows: number[], del: boolean) => void;
  /** Stages copies of the rows as pending INSERTs; generated keys are cut. */
  duplicateRows: (tabId: string, rows: number[]) => void;
  stageInsertCell: (
    tabId: string,
    index: number,
    col: number,
    value: string | null | undefined,
  ) => void;
  removeInsertRow: (tabId: string, index: number) => void;
  discardEdits: (tabId: string) => void;
  /** Applies staged INSERT/UPDATE/DELETE in one transaction (PK for the latter two). */
  applyEdits: (tabId: string) => Promise<void>;
  /** Hides the failed-Apply banner; staged cells stay red until re-applied. */
  dismissApplyError: (tabId: string) => void;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;

/** Next free "Query N" title, counting open and reopenable tabs. */
function nextQueryTitle(s: Pick<AppStore, "tabs" | "closedTabs">): string {
  let max = 0;
  const titles = [
    ...s.tabs.map((t) => t.title),
    ...s.closedTabs.map((c) => c.tab.title),
  ];
  for (const title of titles) {
    const m = /^Query (\d+)$/.exec(title);
    if (m) max = Math.max(max, Number(m[1]));
  }
  return `Query ${max + 1}`;
}

/** Immutably merges a patch into one tab's state (caller vouches for the kind). */
function patchState<S extends Tab["state"]>(
  tabs: Tab[],
  id: string,
  patch: Partial<S>,
): Tab[] {
  return tabs.map((t) =>
    t.id === id ? { ...t, state: { ...(t.state as S), ...patch } } : t,
  );
}

export const columnsKey = (profileId: string, schema: string, table: string) =>
  `${profileId}|${schema}.${table}`;

/** Blank table staging — for new tabs, discard, and post-apply resets. */
const noTableEdits = (): Pick<
  TableTabState,
  "edits" | "deletes" | "inserts" | "applyFailed" | "applyError"
> => ({
  edits: {},
  deletes: [],
  inserts: [],
  applyFailed: false,
  applyError: undefined,
});

/** Blank structure staging. */
const noStructureEdits = (): Pick<
  StructureTabState,
  "colEdits" | "colDrops" | "colAdds"
> => ({ colEdits: {}, colDrops: [], colAdds: [] });

export const useApp = create<AppStore>((set, get) => {
  /** Restores the persisted workspace for a profile; false if nothing restored.
   *  Tab data is NOT fetched here — tabs load lazily on first focus. */
  const restoreProfileTabs = (profileId: string): boolean => {
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
  };

  /** Loads profiles/queries and re-adopts live sessions. Runs once the vault
   *  is unlocked (from init, setupVault or unlockVault). */
  const loadWorkspace = async () => {
    const [profiles, queries, sessionList] = await Promise.all([
      api.listProfiles(),
      api.listQueries(),
      api.listSessions(),
    ]);
    const sessions: Record<string, SessionInfo> = {};
    for (const s of sessionList) sessions[s.profileId] = s;
    set((st) => ({
      profiles,
      queries,
      sessions,
      activeProfileId: st.activeProfileId ?? sessionList[0]?.profileId ?? null,
    }));
    // Re-adopt sessions that survived a webview reload: tables + saved tabs.
    await Promise.all(
      sessionList.map(async (s) => {
        await get().refreshTables(s.profileId);
        // guard: StrictMode double-runs init() in dev — restoring twice
        // would duplicate every tab (and persist the doubled set)
        if (!get().tabs.some((t) => t.profileId === s.profileId)) {
          restoreProfileTabs(s.profileId);
        }
      }),
    );
  };

  /** Session for a profile; toasts "Not connected" and returns null if absent. */
  const sessionFor = (profileId: string): SessionInfo | null => {
    const session = get().sessions[profileId];
    if (!session) get().showToast("Not connected");
    return session ?? null;
  };

  /** A failed call whose error means the session died (not a bad query):
   *  drop it so the sidebar flips to "connection lost" and every entry
   *  point offers Reconnect. Tabs stay — reconnect restores them live. */
  const noteSessionLost = (profileId: string, message: string) => {
    if (!isConnectionLost(message)) return;
    const session = get().sessions[profileId];
    if (!session) return; // already marked (or explicitly disconnected)
    // free the backend half (tunnel included); it may already be gone
    api.disconnect(session.sessionId).catch(() => {});
    set((s) => {
      const sessions = { ...s.sessions };
      delete sessions[profileId];
      return { sessions, lost: { ...s.lost, [profileId]: true } };
    });
  };

  /** Tab by id, narrowed to the given kind; null when missing or another kind. */
  const tabOf = <K extends Tab["state"]["kind"]>(tabId: string, kind: K) => {
    const tab = get().tabs.find((t) => t.id === tabId);
    return tab && tab.state.kind === kind
      ? (tab as Tab & { state: Extract<Tab["state"], { kind: K }> })
      : null;
  };

  /** Immutably merges a patch into one tab's state (caller vouches for the kind). */
  const patchTab = <S extends Tab["state"]>(tabId: string, patch: Partial<S>) =>
    set((s) => ({ tabs: patchState<S>(s.tabs, tabId, patch) }));

  /** Post-unlock tail shared by setup/unlock/Touch ID: optional biometric
   *  enrollment, refreshed vault status, workspace load. */
  const finishUnlock = async (enableBiometric: boolean) => {
    if (enableBiometric) {
      try {
        await api.vaultEnableBiometric();
      } catch (e) {
        // Non-fatal: the vault works, only the fast path is missing.
        get().showToast(`Touch ID not enabled: ${errText(e)}`);
      }
    }
    set({ vault: await api.vaultStatus() });
    await loadWorkspace();
  };

  /** Loads history.json, importing what an older build left in localStorage.
   *  History is a convenience — a failure here never blocks startup. */
  const loadHistoryFromDisk = async () => {
    try {
      const legacy = loadLegacyHistory();
      const history = legacy.length
        ? await api.importHistory(legacy)
        : await api.listHistory();
      if (legacy.length) dropLegacyHistory();
      set({ history });
    } catch {
      // stays empty; the next recordHistory repopulates the in-memory list
    }
  };

  /** Drops cached column info after DDL changed the table. */
  const invalidateColumns = (profileId: string, schema: string, table: string) =>
    set((s) => {
      const tableColumns = { ...s.tableColumns };
      delete tableColumns[columnsKey(profileId, schema, table)];
      return { tableColumns };
    });

  /** Data/structure tabs are per-relation singletons: focus if open, else create. */
  const openRelationTab = (
    kind: "table" | "structure",
    profileId: string,
    schema: string,
    table: string,
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
              orderDir: "asc",
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
  profiles: [],
  sessions: {},
  connecting: {},
  lost: {},
  tables: {},
  schemaColumns: {},
  activeProfileId: null,
  tabs: [],
  activeTabId: null,
  dialog: { open: false },
  toast: null,
  queries: [],
  history: [],
  palette: null,
  closedTabs: loadClosedTabs(),
  saveDialogFor: null,
  settings: {},
  settingsOpen: false,
  vault: null,
  vaultError: null,
  tableColumns: {},

  setPalette: (palette) => set({ palette }),

  setSettingsOpen: (settingsOpen) => set({ settingsOpen }),

  setTheme: async (id) => {
    applyTheme(id);
    const settings = { ...get().settings, theme: id };
    set({ settings });
    try {
      await api.saveSettings(settings);
    } catch (e) {
      // theme is applied for this session; only the persistence failed
      get().showToast(`Settings not saved: ${errText(e)}`);
    }
  },

  setSaveDialogFor: (tabId) => set({ saveDialogFor: tabId }),

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

  deleteHistoryEntry: (id) => {
    set((s) => ({ history: s.history.filter((h) => h.id !== id) }));
    api.deleteHistoryEntry(id).catch((e) => get().showToast(errText(e)));
  },

  clearHistory: () => {
    set({ history: [] });
    api.clearHistory().catch((e) => get().showToast(errText(e)));
  },

  init: async () => {
    // Settings are not vault-gated — the theme must apply on the unlock
    // screen too. main.tsx already applied the localStorage-cached theme;
    // settings.json is the source of truth and wins once it's read.
    void api
      .getSettings()
      .then((settings) => {
        set({ settings });
        applyTheme(settings.theme);
      })
      .catch(() => {
        // cached theme stays; the next setTheme rewrites the file
      });
    void loadHistoryFromDisk();
    try {
      const vault = await api.vaultStatus();
      set({ vault, vaultError: null });
      // Nothing loads until the vault is unlocked — VaultGate shows setup/unlock.
      if (vault.unlocked) await loadWorkspace();
    } catch (e) {
      // Without the vault state the gate can't render anything meaningful —
      // surface the failure with a retry instead of a blank screen.
      set({ vaultError: errText(e) });
    }
  },

  setupVault: async (password, enableBiometric) => {
    await api.vaultSetup(password);
    await finishUnlock(enableBiometric);
  },

  unlockVault: async (password, enableBiometric) => {
    await api.vaultUnlock(password);
    await finishUnlock(enableBiometric);
  },

  unlockVaultBiometric: async () => {
    await api.vaultUnlockBiometric();
    await finishUnlock(false);
  },

  lockVault: async () => {
    try {
      await api.vaultLock();
    } finally {
      // Wipe every trace of the unlocked session from the UI.
      set((st) => ({
        vault: st.vault ? { ...st.vault, unlocked: false } : st.vault,
        profiles: [],
        sessions: {},
        tables: {},
        schemaColumns: {},
        tabs: [],
        activeTabId: null,
        activeProfileId: null,
      }));
    }
  },

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
      patchTab<QueryTabState>(active.id, { sql: q.sql });
      get().linkQueryTab(active.id, q);
    } else {
      get().openQueryTab(profileId, q.sql, q.name, q.id);
    }
  },

  saveQueryTab: async (tabId) => {
    const tab = tabOf(tabId, "query");
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

  deleteQuery: async (id) => {
    try {
      await api.deleteQuery(id);
      set((s) => ({ queries: s.queries.filter((q) => q.id !== id) }));
    } catch (e) {
      get().showToast(errText(e));
    }
  },

  showToast: (message, kind = "error") => {
    set({ toast: { message, kind } });
    if (toastTimer) clearTimeout(toastTimer);
    toastTimer = setTimeout(() => set({ toast: null }), 6000);
  },

  openDialog: (profile) => set({ dialog: { open: true, profile } }),
  closeDialog: () => set({ dialog: { open: false } }),

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
      const sessions = { ...s.sessions };
      delete sessions[id];
      const lost = { ...s.lost };
      delete lost[id];
      const tabs = s.tabs.filter((t) => t.profileId !== id);
      return {
        profiles: s.profiles.filter((p) => p.id !== id),
        sessions,
        lost,
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

  connect: async (profileId) => {
    set((s) => ({ connecting: { ...s.connecting, [profileId]: true } }));
    try {
      const info = await api.connect(profileId);
      set((s) => {
        const lost = { ...s.lost };
        delete lost[profileId];
        return {
          sessions: { ...s.sessions, [profileId]: info },
          activeProfileId: profileId,
          lost,
          // Tabs that errored with the dead session: clear the error (and the
          // stale payload) in the same commit the session appears, so their
          // lazy-load effect refetches on this very render.
          tabs: s.tabs.map((t) => {
            if (
              t.profileId !== profileId ||
              !t.state.error ||
              !isConnectionLost(t.state.error)
            ) {
              return t;
            }
            if (t.state.kind === "table") {
              return {
                ...t,
                state: { ...t.state, error: undefined, data: undefined },
              };
            }
            if (t.state.kind === "structure") {
              return {
                ...t,
                state: {
                  ...t.state,
                  error: undefined,
                  columns: undefined,
                  indexes: undefined,
                  relations: undefined,
                  triggers: undefined,
                },
              };
            }
            return { ...t, state: { ...t.state, error: undefined } };
          }),
        };
      });
      await get().refreshTables(profileId);
      if (
        !get().tabs.some((t) => t.profileId === profileId) &&
        !restoreProfileTabs(profileId)
      ) {
        get().openQueryTab(profileId);
      }
    } catch (e) {
      get().showToast(errText(e));
    } finally {
      set((s) => ({ connecting: { ...s.connecting, [profileId]: false } }));
    }
  },

  disconnect: async (profileId) => {
    persistWorkspace(get(), profileId); // snapshot before tabs are dropped
    const session = get().sessions[profileId];
    if (session) {
      try {
        await api.disconnect(session.sessionId);
      } catch {
        // session may already be gone server-side
      }
    }
    set((s) => {
      const sessions = { ...s.sessions };
      delete sessions[profileId];
      const lost = { ...s.lost };
      delete lost[profileId];
      const tables = { ...s.tables };
      delete tables[profileId];
      const schemaColumns = { ...s.schemaColumns };
      delete schemaColumns[profileId];
      const tableColumns = Object.fromEntries(
        Object.entries(s.tableColumns).filter(
          ([key]) => !key.startsWith(`${profileId}|`),
        ),
      );
      const tabs = s.tabs.filter((t) => t.profileId !== profileId);
      return {
        sessions,
        lost,
        tables,
        schemaColumns,
        tableColumns,
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
      set((s) => {
        const sessions = { ...s.sessions };
        delete sessions[profileId];
        return { sessions };
      });
      try {
        await api.disconnect(session.sessionId);
      } catch {
        // it may already be gone server-side
      }
    }
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
      return { activeProfileId: profileId, activeTabId };
    }),

  refreshTables: async (profileId) => {
    const session = get().sessions[profileId];
    if (!session) return;
    try {
      const tables = await api.getTables(session.sessionId);
      set((s) => ({ tables: { ...s.tables, [profileId]: tables } }));
    } catch (e) {
      const message = errText(e);
      get().showToast(message);
      noteSessionLost(profileId, message);
    }
    // Editor autocomplete data; non-fatal — on failure completion just
    // stays keyword-only.
    try {
      const cols = await api.getAllColumns(session.sessionId);
      set((s) => ({
        schemaColumns: { ...s.schemaColumns, [profileId]: cols },
      }));
    } catch {
      // ignore
    }
  },

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

  openTableTab: (profileId, schema, table) =>
    openRelationTab("table", profileId, schema, table),

  openStructureTab: (profileId, schema, table) =>
    openRelationTab("structure", profileId, schema, table),

  setStructureSection: (tabId, section) => {
    patchTab<StructureTabState>(tabId, { section });
    void get().loadStructure(tabId);
  },

  loadStructure: async (tabId) => {
    const tab = tabOf(tabId, "structure");
    if (!tab) return;
    const session = sessionFor(tab.profileId);
    if (!session) return;
    const { schema, table, section } = tab.state;
    const fetch: Record<
      StructureSection,
      () => Promise<Partial<StructureTabState>>
    > = {
      columns: async () => ({
        columns: await api.getColumns(session.sessionId, schema, table),
      }),
      indexes: async () => ({
        indexes: await api.getIndexes(session.sessionId, schema, table),
      }),
      relations: async () => ({
        relations: await api.getRelations(session.sessionId, schema, table),
      }),
      triggers: async () => ({
        triggers: await api.getTriggers(session.sessionId, schema, table),
      }),
    };
    patchTab<StructureTabState>(tabId, { loading: true, error: undefined });
    try {
      patchTab<StructureTabState>(tabId, {
        loading: false,
        ...(await fetch[section]()),
      });
    } catch (e) {
      const message = errText(e);
      patchTab<StructureTabState>(tabId, { loading: false, error: message });
      noteSessionLost(tab.profileId, message);
    }
  },

  runDdl: async (tabId, sql) => {
    const tab = tabOf(tabId, "structure");
    if (!tab) return false;
    const session = sessionFor(tab.profileId);
    if (!session) return false;
    const { schema, table } = tab.state;
    try {
      await api.executeSql(session.sessionId, sql, 10);
    } catch (e) {
      const message = errText(e);
      get().showToast(message);
      noteSessionLost(tab.profileId, message);
      return false;
    }
    // sidebar column cache is stale now
    invalidateColumns(tab.profileId, schema, table);
    get().showToast("Applied", "success");
    await get().loadStructure(tabId);
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
    const stmts = structureDdl(tab.state);
    if (stmts.length === 0) return;
    const session = sessionFor(tab.profileId);
    if (!session) return;
    patchTab<StructureTabState>(tabId, { loading: true });
    // One simple-query message = one implicit transaction: atomic, and an
    // error auto-rolls-back without leaving the session in an aborted tx.
    try {
      await api.executeSql(session.sessionId, stmts.join(";\n"), 10);
    } catch (e) {
      const message = errText(e);
      patchTab<StructureTabState>(tabId, { loading: false });
      get().showToast(message); // rolled back, edits stay staged
      noteSessionLost(tab.profileId, message);
      return;
    }
    // clear staged + drop the stale sidebar column cache before reloading
    invalidateColumns(tab.profileId, tab.state.schema, tab.state.table);
    patchTab<StructureTabState>(tabId, noStructureEdits());
    get().showToast(`Applied ${stmts.length} change(s)`, "success");
    await get().loadStructure(tabId);
  },

  loadTableColumns: async (profileId, schema, table) => {
    const key = columnsKey(profileId, schema, table);
    if (get().tableColumns[key]) return;
    const session = get().sessions[profileId];
    if (!session) return;
    try {
      const cols = await api.getColumns(session.sessionId, schema, table);
      set((s) => ({ tableColumns: { ...s.tableColumns, [key]: cols } }));
    } catch (e) {
      const message = errText(e);
      get().showToast(message);
      noteSessionLost(profileId, message);
    }
  },

  closeTab: (tabId) => get().closeTabs([tabId]),

  closeTabs: (ids) => {
    const idSet = new Set(ids);
    const count = get().tabs.filter((t) => idSet.has(t.id)).length;
    if (count === 0) return;
    set((s) => {
      const closedEntries = s.tabs
        .map((tab, index) => ({ tab, index }))
        .filter(({ tab }) => idSet.has(tab.id))
        .map(({ tab, index }) => {
          // snapshot with transient bits reset so ⌘⇧T restores a sane tab
          const state: Tab["state"] =
            tab.state.kind === "query"
              ? { ...tab.state, running: false }
              : tab.state.kind === "table"
                ? { ...tab.state, loading: false, ...noTableEdits() }
                : { ...tab.state, loading: false, ...noStructureEdits() };
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

  setTabSql: (tabId, sql) => patchTab<QueryTabState>(tabId, { sql }),

  setTabMaxRows: (tabId, maxRows) => patchTab<QueryTabState>(tabId, { maxRows }),

  setTabEditorPct: (tabId, editorPct) =>
    patchTab<QueryTabState>(tabId, { editorPct }),

  runQuery: async (tabId) => {
    const tab = tabOf(tabId, "query");
    if (!tab || tab.state.running) return;
    const session = sessionFor(tab.profileId);
    if (!session) return;
    const sql = tab.state.sql.trim();
    if (!sql) return;
    const pushHistory = (ok: boolean) => {
      const profile = get().profiles.find((p) => p.id === tab.profileId);
      const entry: HistoryEntry = {
        id: crypto.randomUUID(),
        profileId: tab.profileId,
        profileName: profile?.name ?? "?",
        sql,
        at: Date.now(),
        ok,
      };
      // the disk store dedups (a rerun bumps to the top) and caps the list
      api
        .recordHistory(entry)
        .then((history) => set({ history }))
        .catch(() => {
          // keep at least the in-memory trail for this session
          set((s) => ({ history: [entry, ...s.history] }));
        });
    };
    patchTab<QueryTabState>(tabId, { running: true, error: undefined });
    try {
      const result = await api.executeSql(
        session.sessionId,
        sql,
        tab.state.maxRows,
      );
      pushHistory(true);
      patchTab<QueryTabState>(tabId, { result, running: false });
    } catch (e) {
      pushHistory(false);
      const message = errText(e);
      patchTab<QueryTabState>(tabId, {
        error: message,
        result: undefined,
        running: false,
      });
      noteSessionLost(tab.profileId, message);
    }
  },

  cancelQuery: async (tabId) => {
    const tab = get().tabs.find((t) => t.id === tabId);
    if (!tab) return;
    const session = get().sessions[tab.profileId];
    if (!session) return;
    try {
      await api.cancelQuery(session.sessionId);
    } catch (e) {
      get().showToast(errText(e));
    }
  },

  loadTablePage: async (tabId, patch) => {
    const tab = tabOf(tabId, "table");
    if (!tab) return;
    const session = sessionFor(tab.profileId);
    if (!session) return;
    // Reloading renumbers rows, so staged edits can't survive it.
    if (
      (Object.keys(tab.state.edits).length > 0 ||
        tab.state.deletes.length > 0) &&
      !confirm("Discard pending changes?")
    ) {
      return;
    }
    const next = { ...tab.state, ...patch };
    patchTab<TableTabState>(tabId, { ...patch, loading: true, error: undefined });
    try {
      const data = await api.getTablePage(
        session.sessionId,
        next.schema,
        next.table,
        next.pageSize,
        next.page * next.pageSize,
        next.orderBy,
        next.orderDir,
      );
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
      const message = errText(e);
      patchTab<TableTabState>(tabId, { error: message, loading: false });
      noteSessionLost(tab.profileId, message);
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

  toggleRowDeletes: (tabId, rows, del) => {
    const tab = tabOf(tabId, "table");
    if (!tab) return;
    const next = new Set(tab.state.deletes);
    for (const r of rows) {
      if (del) next.add(r);
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
      get().tableColumns[columnsKey(tab.profileId, tab.state.schema, tab.state.table)];
    // generated keys are cut (undefined) so the DB regenerates them on INSERT
    const cut = new Set<number>();
    data.result.columns.forEach((name, ci) => {
      const c = cols?.find((x) => x.name === name);
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

  removeInsertRow: (tabId, index) => {
    const tab = tabOf(tabId, "table");
    if (!tab) return;
    patchTab<TableTabState>(tabId, {
      inserts: tab.state.inserts.filter((_, i) => i !== index),
    });
  },

  discardEdits: (tabId) => {
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

  applyEdits: async (tabId) => {
    const tab = tabOf(tabId, "table");
    if (!tab) return;
    const st = tab.state;
    if (st.loading) return; // e.g. ⌘S mashed while a previous apply runs
    const data = st.data;
    if (!data) return;
    const session = sessionFor(tab.profileId);
    if (!session) return;
    const dml = tableDml(
      st,
      data,
      get().tableColumns[columnsKey(tab.profileId, st.schema, st.table)],
    );
    if ("error" in dml) {
      get().showToast(dml.error);
      return;
    }
    const { stmts, updates, deletes } = dml;
    if (stmts.length === 0) return;
    patchTab<TableTabState>(tabId, {
      loading: true,
      applyFailed: false,
      applyError: undefined,
    });
    // One simple-query message = one implicit transaction: atomic, and an
    // error auto-rolls-back without leaving the session in an aborted tx.
    try {
      await api.executeSql(session.sessionId, stmts.join(";\n"), 10);
    } catch (e) {
      const message = errText(e);
      patchTab<TableTabState>(tabId, {
        loading: false,
        // the whole tx rolled back — every staged cell is unsaved
        applyFailed: true,
        applyError: message,
      });
      get().showToast(message); // rolled back, edits stay staged
      noteSessionLost(tab.profileId, message);
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
      await get().loadTablePage(tabId);
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
});

// Debounced auto-persist of open tabs for every connected profile.
// Skipping disconnected profiles keeps their saved workspace intact
// after `disconnect` removes the tabs from the store.
let persistTimer: ReturnType<typeof setTimeout> | undefined;

function flushPersist() {
  if (persistTimer) {
    clearTimeout(persistTimer);
    persistTimer = undefined;
  }
  const s = useApp.getState();
  for (const profileId of Object.keys(s.sessions)) {
    persistWorkspace(s, profileId);
  }
}

useApp.subscribe(() => {
  if (persistTimer) clearTimeout(persistTimer);
  persistTimer = setTimeout(flushPersist, 400);
});

window.addEventListener("beforeunload", flushPersist);

// HMR would re-create the store with empty state (init() doesn't re-run) —
// reload instead; live sessions are re-adopted via list_sessions on boot.
if (import.meta.hot) {
  import.meta.hot.accept(() => window.location.reload());
}
