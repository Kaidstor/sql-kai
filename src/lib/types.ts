export interface SshConfig {
  host: string;
  user?: string | null;
  port?: number | null;
  keyPath?: string | null;
  /** Seconds between keepalive pings when idle (ssh ServerAliveInterval).
   *  null = app default (5), 0 = off. */
  keepaliveInterval?: number | null;
}

/** TLS settings for a direct (non-tunnel) connection. Absent or
 *  `enabled: false` = plaintext (the default). */
export interface SslConfig {
  enabled: boolean;
  /** PEM file with a CA to trust (self-signed / internal CA / RDS);
   *  empty = system trust store. */
  caCert?: string | null;
  /** PEM client certificate + private key (mTLS) — both or neither. */
  clientCert?: string | null;
  clientKey?: string | null;
  /** Verify the server certificate (chain + hostname); off = encrypt only. */
  rejectUnauthorized: boolean;
}

export interface Profile {
  id: string;
  name: string;
  host: string;
  port: number;
  database: string;
  user: string;
  ssh?: SshConfig | null;
  /** TLS settings for direct (non-tunnel) connections; see SslConfig.
   *  Ignored over an SSH tunnel or to a loopback host. */
  ssl?: SslConfig | null;
  /** Profiles sharing a group (e.g. prod/test of one service) share saved queries. */
  group?: string | null;
  /** Accent color name for telling connections apart (see lib/colors.ts). */
  color?: string | null;
  /** Production database: the UI asks before running data-modifying SQL. */
  production?: boolean;
  hasPassword?: boolean;
  hasSshPassphrase?: boolean;
  /** Last successful connection per client, attached by the backend from
   *  last_connected.json (display-only, not part of the saved profile). */
  lastConnected?: LastConnected | null;
}

/** Epoch-ms marks of a profile's last successful connection per client. */
export interface LastConnected {
  gui?: number | null;
  cli?: number | null;
}

/** Contents of settings.json — portable app preferences. The backend only
 *  round-trips this object, so unknown keys survive older builds. */
export interface AppSettings {
  theme?: string;
  /** ACP agent provider id (see AGENT_PROVIDERS in slices/agent.ts). */
  agentProvider?: string;
  /** Command line of the "custom" agent provider. */
  agentCustomCmd?: string;
  [key: string]: unknown;
}

export interface VaultStatus {
  /** A vault file exists (a master password was set up before). */
  exists: boolean;
  /** The DEK is decrypted in memory for this session. */
  unlocked: boolean;
  /** This platform can offer Touch ID at all (macOS). */
  biometricSupported: boolean;
  /** A DEK copy is enrolled in the biometric keychain for this vault. */
  biometricEnrolled: boolean;
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

/** Heuristic transaction state of a connection (advisory, status-bar badge). */
export type TxStatus = "idle" | "active" | "failed";

export interface SessionInfo {
  sessionId: string;
  profileId: string;
  serverVersion: string;
  tunnelPort?: number | null;
  /** Transaction state of the connection; refreshed after each query run. */
  tx?: TxStatus;
  /** True for a per-tab isolated connection (own pid / transaction). */
  isolated?: boolean;
  /** Backend pid — set for isolated sessions so the tab can display it. */
  pid?: number | null;
}

/** One broker-owned CLI (sql-kai) session — drives the "cli" badges. */
export interface CliSessionInfo {
  profileId: string;
  profileName: string;
  origin: string;
  serverVersion: string;
  tunnelPort?: number | null;
  tx: TxStatus;
  idleSec?: number | null;
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

/** File formats of the full-result export (export_sql). */
export type ExportFormat = "csv" | "json" | "md" | "xlsx";

export interface ExportOutcome {
  rows: number;
  /** XLSX sheet row limit cut the result; other formats never truncate. */
  truncated: boolean;
  durationMs: number;
}

export interface TableInfo {
  schema: string;
  name: string;
  kind: string;
}

/** Relation kinds that have no storage of their own: read-only in the grid,
 *  DDL'd as CREATE VIEW. Undefined kind (catalog not loaded yet) is not a view. */
export function isViewKind(kind: string | undefined): boolean {
  return kind === "view" || kind === "matview";
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
  /** false = ALTER TABLE … DISABLE TRIGGER (tgenabled = 'D'). */
  enabled: boolean;
}

/** One enum type with its labels, for filter-value suggestions. */
export interface EnumTypeInfo {
  schema: string;
  name: string;
  labels: string[];
}

export interface PolicyInfo {
  name: string;
  /** ALL / SELECT / INSERT / UPDATE / DELETE. */
  command: string;
  permissive: boolean;
  /** null = PUBLIC. */
  roles?: string | null;
  usingExpr?: string | null;
  checkExpr?: string | null;
}

/** RLS switches + policies of one table. */
export interface TablePolicies {
  rlsEnabled: boolean;
  rlsForced: boolean;
  policies: PolicyInfo[];
}

export interface TablePage {
  result: StatementResult;
  durationMs: number;
  approxRows: number;
}

/** One node of EXPLAIN (FORMAT JSON) output — keys exactly as Postgres emits
 *  them. Times are per-loop averages; multiply by "Actual Loops" for totals. */
export interface PlanNode {
  "Node Type": string;
  "Subplan Name"?: string;
  "Relation Name"?: string;
  Schema?: string;
  Alias?: string;
  "Index Name"?: string;
  "Join Type"?: string;
  "Startup Cost"?: number;
  "Total Cost"?: number;
  "Plan Rows"?: number;
  "Actual Startup Time"?: number;
  "Actual Total Time"?: number;
  "Actual Rows"?: number;
  "Actual Loops"?: number;
  "Rows Removed by Filter"?: number;
  "Rows Removed by Join Filter"?: number;
  Filter?: string;
  "Index Cond"?: string;
  "Hash Cond"?: string;
  "Merge Cond"?: string;
  "Recheck Cond"?: string;
  "Sort Key"?: string[];
  "Sort Method"?: string;
  Plans?: PlanNode[];
}

export interface ExplainResult {
  Plan: PlanNode;
  "Planning Time"?: number;
  "Execution Time"?: number;
  /** true when produced by EXPLAIN ANALYZE (actual timings present). */
  analyzed: boolean;
}
