// Tab-state model shared by every slice and by consumers outside the store
// (persist snapshots, mutationSql, components). AppStore is assembled at the
// bottom from the slice interfaces — each slice file owns its own contract.
import type {
  ColumnInfo,
  ExecResult,
  ExplainResult,
  IndexInfo,
  RelationInfo,
  SavedQuery,
  SortSpec,
  TablePageResult,
  TablePolicies,
  TriggerInfo,
} from "../types";
import type { ActivitySlice } from "./slices/activity";
import type { AgentSlice } from "./slices/agent";
import type { ConnectionsSlice } from "./slices/connections";
import type { QuerySlice } from "./slices/query";
import type { SavedSlice } from "./slices/saved";
import type { StructureSlice } from "./slices/structure";
import type { TableSlice } from "./slices/table";
import type { TabsSlice } from "./slices/tabs";
import type { UiSlice } from "./slices/ui";
import type { VaultSlice } from "./slices/vault";

export interface QueryTabState {
  kind: "query";
  sql: string;
  result?: ExecResult;
  /** The SQL that produced `result` (may be an editor selection, not `sql`) —
   *  the full export re-runs exactly this text. */
  resultSql?: string;
  /** Parsed EXPLAIN output; shown instead of results until dismissed. */
  explain?: ExplainResult;
  error?: string;
  /** `error` означает смерть соединения (код connection_lost/session_gone) —
   *  UI предлагает Reconnect вместо правки запроса. */
  connectionLost?: boolean;
  running: boolean;
  maxRows: number;
  /** Editor pane height, % of the editor+results split; 0 = collapsed (results only). */
  editorPct?: number;
  /** Saved query this tab was opened from / saved as — ⌘S overwrites it. */
  savedQueryId?: string;
  /** Runs on its own dedicated connection (own pid & transaction) instead of
   *  the profile's shared session. Persisted intent; the connection itself is
   *  opened lazily. */
  isolated?: boolean;
  /** Dedicated backend session id when isolated & open (ephemeral, keyed into
   *  `isolatedSessions`); absent = not yet opened / needs (re)opening. */
  sessionId?: string;
  /** "manual" holds a transaction open across runs (BEGIN auto-inserted) with
   *  explicit Commit/Rollback; implies isolation. Default "auto". */
  commitMode?: "auto" | "manual";
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
  /** ORDER BY entries in priority order (empty = server default order). */
  sorts: SortSpec[];
  /** Raw WHERE expression ("" = no filter) — filter bar / FK navigation. */
  filter: string;
  data?: TablePageResult;
  error?: string;
  /** См. QueryTabState.connectionLost. */
  connectionLost?: boolean;
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

/** One backend from pg_stat_activity (see the activity slice for the SQL). */
export interface ActivityInfo {
  pid: string;
  db: string;
  state: string;
  user: string;
  app: string;
  client: string;
  wait: string;
  /** PIDs holding locks this backend waits on (pg_blocking_pids). */
  blockedBy: string;
  querySec: number | null;
  xactSec: number | null;
  query: string;
}

export interface ActivityTabState {
  kind: "activity";
  rows?: ActivityInfo[];
  error?: string;
  /** См. QueryTabState.connectionLost. */
  connectionLost?: boolean;
  loading: boolean;
  /** Auto-refresh period, seconds; 0 = manual only. */
  refreshSec: number;
  /** Include idle backends in the listing. */
  includeIdle: boolean;
}

/** One relation inside one connection — the identity behind the sidebar's
 *  column/FK caches and the argument of every lazy metadata loader. Named
 *  rather than positional: `schema` and `table` are both bare strings, so a
 *  `(profileId, schema, table)` triple is trivially swappable at a call site
 *  (`table.schema` / `table.name` in the sidebar) and the compiler wouldn't
 *  notice. */
export interface RelRef {
  profileId: string;
  schema: string;
  table: string;
}

export type StructureSection =
  | "columns"
  | "indexes"
  | "relations"
  | "triggers"
  | "policies";

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
  policies?: TablePolicies;
  loading: boolean;
  error?: string;
  /** См. QueryTabState.connectionLost. */
  connectionLost?: boolean;
  /** Staged column DDL (Apply runs it in one transaction), keyed by original name. */
  colEdits: Record<string, ColumnPatch>;
  colDrops: string[];
  colAdds: NewColumn[];
}

export interface Tab {
  id: string;
  profileId: string;
  title: string;
  state:
    | QueryTabState
    | TableTabState
    | StructureTabState
    | ActivityTabState;
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

export interface Toast {
  message: string;
  kind: "error" | "info" | "success";
}

/** In-app confirm dialog request — window.confirm() doesn't block in the
 *  Tauri webview, so destructive actions go through confirmDialog() instead. */
export interface ConfirmRequest {
  title: string;
  /** Optional body; rendered pre-wrap so SQL previews keep line breaks. */
  message?: string;
  /** Confirm button label, default "Confirm". */
  confirmLabel?: string;
  /** Destructive action: the confirm button turns red. */
  danger?: boolean;
}

export type PaletteKind = "connections" | "queries" | "symbols";

/** One user-defined function for the symbols palette. */
export interface FunctionInfo {
  schema: string;
  name: string;
  /** Identity argument list, e.g. "integer, text". */
  args: string;
}

export type AppStore = UiSlice &
  VaultSlice &
  ConnectionsSlice &
  TabsSlice &
  QuerySlice &
  TableSlice &
  StructureSlice &
  ActivitySlice &
  SavedSlice &
  AgentSlice;
