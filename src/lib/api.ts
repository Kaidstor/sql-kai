import { invoke } from "@tauri-apps/api/core";
import type {
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

export const api = {
  vaultStatus: () => invoke<VaultStatus>("vault_status"),

  vaultSetup: (password: string) => invoke<void>("vault_setup", { password }),

  vaultUnlock: (password: string) => invoke<void>("vault_unlock", { password }),

  vaultUnlockBiometric: () => invoke<void>("vault_unlock_biometric"),

  vaultEnableBiometric: () => invoke<void>("vault_enable_biometric"),

  vaultDisableBiometric: () => invoke<void>("vault_disable_biometric"),

  vaultLock: () => invoke<void>("vault_lock"),

  listProfiles: () => invoke<Profile[]>("list_profiles"),

  saveProfile: (
    profile: Profile,
    password: string | null,
    sshPassphrase: string | null,
  ) => invoke<Profile>("save_profile", { profile, password, sshPassphrase }),

  deleteProfile: (id: string) => invoke<void>("delete_profile", { id }),

  duplicateProfile: (id: string) =>
    invoke<Profile>("duplicate_profile", { id }),

  listQueries: () => invoke<SavedQuery[]>("list_queries"),

  saveQuery: (query: SavedQuery) => invoke<SavedQuery>("save_query", { query }),

  deleteQuery: (id: string) => invoke<void>("delete_query", { id }),

  listHistory: () => invoke<HistoryEntry[]>("list_history"),

  recordHistory: (entry: HistoryEntry) =>
    invoke<HistoryEntry[]>("record_history", { entry }),

  deleteHistoryEntry: (id: string) =>
    invoke<HistoryEntry[]>("delete_history_entry", { id }),

  clearHistory: () => invoke<void>("clear_history"),

  importHistory: (entries: HistoryEntry[]) =>
    invoke<HistoryEntry[]>("import_history", { entries }),

  connect: (profileId: string) =>
    invoke<SessionInfo>("connect_profile", { profileId }),

  disconnect: (sessionId: string) =>
    invoke<void>("disconnect_session", { sessionId }),

  listSessions: () => invoke<SessionInfo[]>("list_sessions"),

  testProfile: (
    profile: Profile,
    password: string | null,
    sshPassphrase: string | null,
  ) => invoke<string>("test_profile", { profile, password, sshPassphrase }),

  executeSql: (sessionId: string, sql: string, maxRows: number) =>
    invoke<ExecResult>("execute_sql", { sessionId, sql, maxRows }),

  cancelQuery: (sessionId: string) =>
    invoke<void>("cancel_query", { sessionId }),

  getTables: (sessionId: string) =>
    invoke<TableInfo[]>("get_tables", { sessionId }),

  getColumns: (sessionId: string, schema: string, table: string) =>
    invoke<ColumnInfo[]>("get_columns", { sessionId, schema, table }),

  getAllColumns: (sessionId: string) =>
    invoke<TableColumns[]>("get_all_columns", { sessionId }),

  getTableDdl: (sessionId: string, schema: string, table: string) =>
    invoke<string>("get_table_ddl", { sessionId, schema, table }),

  getIndexes: (sessionId: string, schema: string, table: string) =>
    invoke<IndexInfo[]>("get_indexes", { sessionId, schema, table }),

  getRelations: (sessionId: string, schema: string, table: string) =>
    invoke<RelationInfo[]>("get_relations", { sessionId, schema, table }),

  getTriggers: (sessionId: string, schema: string, table: string) =>
    invoke<TriggerInfo[]>("get_triggers", { sessionId, schema, table }),

  getTablePage: (
    sessionId: string,
    schema: string,
    table: string,
    limit: number,
    offset: number,
    orderBy?: string,
    orderDir?: "asc" | "desc",
  ) =>
    invoke<TablePage>("get_table_page", {
      sessionId,
      schema,
      table,
      limit,
      offset,
      orderBy: orderBy ?? null,
      orderDir: orderDir ?? null,
    }),
};

export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  return String(e);
}

// Hot-swapping this module would leave stale references in the store —
// do a clean reload instead (sessions are re-adopted via list_sessions).
if (import.meta.hot) {
  import.meta.hot.accept(() => window.location.reload());
}
