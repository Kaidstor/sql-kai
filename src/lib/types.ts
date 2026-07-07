export interface SshConfig {
  host: string;
  user?: string | null;
  port?: number | null;
  keyPath?: string | null;
  /** Seconds between keepalive pings when idle (ssh ServerAliveInterval).
   *  null = app default (15), 0 = off. */
  keepaliveInterval?: number | null;
}

export interface Profile {
  id: string;
  name: string;
  host: string;
  port: number;
  database: string;
  user: string;
  ssh?: SshConfig | null;
  /** Profiles sharing a group (e.g. prod/test of one service) share saved queries. */
  group?: string | null;
  /** Accent color name for telling connections apart (see lib/colors.ts). */
  color?: string | null;
  /** Production database: the UI asks before running data-modifying SQL. */
  production?: boolean;
  hasPassword?: boolean;
  hasSshPassphrase?: boolean;
}

/** Contents of settings.json — portable app preferences. The backend only
 *  round-trips this object, so unknown keys survive older builds. */
export interface AppSettings {
  theme?: string;
  [key: string]: unknown;
}

export interface VaultStatus {
  /** A vault file exists (a master password was set up before). */
  exists: boolean;
  /** The DEK is decrypted in memory for this session. */
  unlocked: boolean;
  /** This platform can offer Touch ID at all (macOS). */
  biometricsSupported: boolean;
  /** A DEK copy is enrolled in the biometric keychain for this vault. */
  biometricsEnrolled: boolean;
}

export interface HistoryEntry {
  id: string;
  profileId: string;
  profileName: string;
  sql: string;
  at: number; // epoch ms
  ok: boolean;
}

export interface SavedQuery {
  id: string;
  name: string;
  sql: string;
  /** null/undefined = global collection; otherwise group name or profile id. */
  scope?: string | null;
}

export interface SessionInfo {
  sessionId: string;
  profileId: string;
  serverVersion: string;
  tunnelPort?: number | null;
}

export interface StatementResult {
  columns: string[];
  rows: (string | null)[][];
  rowsAffected?: number | null;
  truncated: boolean;
}

/** One ORDER BY entry of a table grid (multi-sort is a list of these). */
export interface SortSpec {
  column: string;
  dir: "asc" | "desc";
}

export interface ExecResult {
  results: StatementResult[];
  durationMs: number;
}

export interface TableInfo {
  schema: string;
  name: string;
  kind: string;
}

/** Column names of one relation, for editor schema autocomplete. */
export interface TableColumns {
  schema: string;
  table: string;
  columns: string[];
}

export interface ColumnInfo {
  name: string;
  dataType: string;
  nullable: boolean;
  isPk: boolean;
  defaultExpr?: string | null;
  comment?: string | null;
}

export interface IndexInfo {
  name: string;
  unique: boolean;
  primary: boolean;
  columns?: string | null;
  definition: string;
}

export interface RelationInfo {
  name: string;
  columns?: string | null;
  refTable: string;
  refColumns?: string | null;
  onUpdate: string;
  onDelete: string;
}

export interface TriggerInfo {
  name: string;
  timing: string;
  events: string;
  definition: string;
}

export interface TablePage {
  result: StatementResult;
  durationMs: number;
  approxRows: number;
}
