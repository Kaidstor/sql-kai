import { invoke } from "@tauri-apps/api/core";
import type {
  AppSettings,
  CliSessionInfo,
  ColumnInfo,
  EnumTypeInfo,
  ExecResult,
  ExportFormat,
  ExportResult,
  HistoryEntry,
  IndexInfo,
  Profile,
  RelationInfo,
  SavedQuery,
  SessionInfo,
  SortSpec,
  TableColumns,
  TableInfo,
  TablePageResult,
  TablePolicies,
  TriggerInfo,
  TxStatus,
  VaultStatus,
} from "./types";

export interface TrayConnection {
  profileId: string;
  name: string;
}

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

  getSettings: () => invoke<AppSettings>("get_settings"),

  saveSettings: (settings: AppSettings) =>
    invoke<void>("save_settings", { settings }),

  settingsPath: () => invoke<string>("settings_path"),

  /** Бандл-CLI рядом с бинарём приложения (null — сборка без sidecar). */
  cliBinPath: () => invoke<string | null>("cli_bin_path"),

  /** Показать главное окно (создаётся скрытым; фронтенд зовёт после отрисовки). */
  revealWindow: () => invoke<void>("reveal_main_window"),

  logPath: () => invoke<string>("log_path"),

  /** Appends a UI-observed event to the backend diagnostics log. */
  logEvent: (message: string) => invoke<void>("log_event", { message }),

  /** Tail of the diagnostics log for the in-app viewer. */
  getLog: () => invoke<string>("get_log"),

  listHistory: () => invoke<HistoryEntry[]>("list_history"),

  recordHistory: (entry: HistoryEntry) =>
    invoke<HistoryEntry[]>("record_history", { entry }),

  deleteHistoryEntry: (id: string) =>
    invoke<HistoryEntry[]>("delete_history_entry", { id }),

  clearHistory: () => invoke<void>("clear_history"),

  importHistory: (entries: HistoryEntry[]) =>
    invoke<HistoryEntry[]>("import_history", { entries }),

  connectProfile: (profileId: string) =>
    invoke<SessionInfo>("connect_profile", { profileId }),

  disconnectSession: (sessionId: string) =>
    invoke<void>("disconnect_session", { sessionId }),

  listSessions: () => invoke<SessionInfo[]>("list_sessions"),

  listCliSessions: () => invoke<CliSessionInfo[]>("list_cli_sessions"),

  syncTrayConnections: (
    connections: TrayConnection[],
    activeProfileId: string | null,
  ) =>
    invoke<void>("sync_tray_connections", { connections, activeProfileId }),

  testProfile: (
    profile: Profile,
    password: string | null,
    sshPassphrase: string | null,
  ) => invoke<string>("test_profile", { profile, password, sshPassphrase }),

  /** `prodWrite` — подтверждённый write-intent: без него backend исполняет
   *  батч production-сессии внутри BEGIN READ ONLY (отказ придёт кодом
   *  "read_only", см. isReadOnlyRefusal). */
  executeSql: (
    sessionId: string,
    sql: string,
    maxRows: number,
    autoBegin = false,
    parameters?: string[],
    prodWrite = false,
  ) =>
    invoke<ExecResult>("execute_sql", {
      sessionId,
      sql,
      maxRows,
      autoBegin,
      parameters,
      prodWrite,
    }),

  /** How many `$N` values the SQL expects. Asked of the backend scanner, not
   *  guessed with a regex: `$1` inside a string or comment is not one. */
  sqlPlaceholderCount: (sql: string) =>
    invoke<number>("sql_placeholder_count", { sql }),

  /** Last run's values, for prefilling the params dialog; empty when the
   *  vault is locked. */
  queryParameters: (profileId: string, sql: string) =>
    invoke<string[]>("query_parameters", { profileId, sql }),

  rememberQueryParameters: (
    profileId: string,
    sql: string,
    parameters: string[],
  ) =>
    invoke<void>("remember_query_parameters", { profileId, sql, parameters }),

  forgetQueryParameters: (profileId: string, sql: string) =>
    invoke<void>("forget_query_parameters", { profileId, sql }),

  /** Full-result export: re-runs `sql` with no row limit and streams the
   *  rows of statement `statementIndex` into `path`. `autoBegin` mirrors
   *  executeSql's manual-commit wrapping (the backend shifts the index). */
  exportSql: (
    sessionId: string,
    sql: string,
    statementIndex: number,
    format: ExportFormat,
    path: string,
    autoBegin = false,
    parameters?: string[],
    prodWrite = false,
  ) =>
    invoke<ExportResult>("export_sql", {
      sessionId,
      sql,
      statementIndex,
      format,
      path,
      autoBegin,
      parameters,
      prodWrite,
    }),

  openIsolatedSession: (profileId: string) =>
    invoke<SessionInfo>("open_isolated_session", { profileId }),

  cancelQuery: (sessionId: string) =>
    invoke<void>("cancel_query", { sessionId }),

  sessionTxStatus: (sessionId: string) =>
    invoke<TxStatus>("session_tx_status", { sessionId }),

  listTables: (sessionId: string) =>
    invoke<TableInfo[]>("list_tables", { sessionId }),

  listColumns: (sessionId: string, schema: string, table: string) =>
    invoke<ColumnInfo[]>("list_columns", { sessionId, schema, table }),

  listAllColumns: (sessionId: string) =>
    invoke<TableColumns[]>("list_all_columns", { sessionId }),

  saveTextFile: (path: string, contents: string) =>
    invoke<void>("save_text_file", { path, contents }),

  /** Writes an in-memory selection (rows already materialized on the client,
   *  staged edits included) to `path` as XLSX — CSV/JSON are built and saved
   *  as text, but the binary workbook is written on the Rust side. */
  saveRowsXlsx: (
    path: string,
    columns: string[],
    rows: (string | null)[][],
  ) => invoke<void>("save_rows_xlsx", { path, columns, rows }),

  getTableDdl: (sessionId: string, schema: string, table: string) =>
    invoke<string>("get_table_ddl", { sessionId, schema, table }),

  listIndexes: (sessionId: string, schema: string, table: string) =>
    invoke<IndexInfo[]>("list_indexes", { sessionId, schema, table }),

  listRelations: (sessionId: string, schema: string, table: string) =>
    invoke<RelationInfo[]>("list_relations", { sessionId, schema, table }),

  listTriggers: (sessionId: string, schema: string, table: string) =>
    invoke<TriggerInfo[]>("list_triggers", { sessionId, schema, table }),

  listEnums: (sessionId: string) =>
    invoke<EnumTypeInfo[]>("list_enums", { sessionId }),

  listPolicies: (sessionId: string, schema: string, table: string) =>
    invoke<TablePolicies>("list_policies", { sessionId, schema, table }),

  getTablePage: (
    sessionId: string,
    schema: string,
    table: string,
    limit: number,
    offset: number,
    sorts?: readonly SortSpec[],
    filter?: string,
  ) =>
    invoke<TablePageResult>("get_table_page", {
      sessionId,
      schema,
      table,
      limit,
      offset,
      sorts: sorts && sorts.length > 0 ? sorts : null,
      filter: filter?.trim() || null,
    }),
};

/** Rust-команды кидают AppError, сериализованный как `{code, message}` —
 *  коды см. error.rs (connection_lost / session_gone / read_only / db / …). */
export interface BackendError {
  code: string;
  message: string;
}

function isBackendError(e: unknown): e is BackendError {
  return (
    typeof e === "object" &&
    e !== null &&
    typeof (e as BackendError).message === "string"
  );
}

export function errText(e: unknown): string {
  if (typeof e === "string") return e;
  if (e instanceof Error) return e.message;
  if (isBackendError(e)) return e.message;
  return String(e);
}

/** Код AppError; null для прочих ошибок (строки, JS-исключения). */
export function errCode(e: unknown): string | null {
  return isBackendError(e) && typeof e.code === "string" ? e.code : null;
}

/** Errors that mean the session itself is dead (server/tunnel dropped or the
 *  backend already discarded it) — the fix is a reconnect, not a better query,
 *  so the UI offers one wherever such an error surfaces. Determined by the
 *  backend's error code, not by matching message text. */
export function isSessionLost(e: unknown): boolean {
  const code = errCode(e);
  return code === "connection_lost" || code === "session_gone";
}

/** Backend отказал из-за read-only: гейт prod-сессии до отправки или SQLSTATE
 *  25006 из BEGIN READ ONLY-блока. На production-профиле это сигнал «нужен
 *  write-intent» — спросить пользователя и повторить с prodWrite. */
export function isReadOnlyRefusal(e: unknown): boolean {
  return errCode(e) === "read_only";
}

// Hot-swapping this module would leave stale references in the store —
// do a clean reload instead (sessions are re-adopted via list_sessions).
if (import.meta.hot) {
  import.meta.hot.accept(() => window.location.reload());
}
