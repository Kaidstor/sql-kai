// localStorage persistence: query history, per-profile workspaces (open tabs),
// the closed-tabs stack and saved agent chats. Everything here is best-effort —
// storage errors are swallowed, corrupt snapshots are dropped.
import type { SessionConfigOption } from "./acp";
import type {
  ActivityTabState,
  AgentChatItem,
  AgentChatItemBody,
  QueryTabState,
  StructureTabState,
  Tab,
  TableTabState,
} from "./store";
import type { HistoryEntry } from "./types";

function readJson<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // best-effort
  }
}

// --- Query history (legacy) ---------------------------------------------------
// History now lives on disk (history.json via the Rust store); localStorage is
// read once to migrate what an older build left behind.

const HISTORY_KEY = "sqlt.history";

export const loadLegacyHistory = (): HistoryEntry[] =>
  readJson<HistoryEntry[]>(HISTORY_KEY) ?? [];

/** Call only after the entries made it into the disk store. */
export function dropLegacyHistory() {
  try {
    localStorage.removeItem(HISTORY_KEY);
  } catch {
    // best-effort
  }
}

// --- Workspace (open tabs + raw SQL, per profile) ----------------------------

const wsKey = (profileId: string) => `sqlt.workspace.${profileId}`;

type PersistedTabState =
  | Pick<
      QueryTabState,
      "kind" | "sql" | "maxRows" | "savedQueryId" | "isolated" | "commitMode"
    >
  | (Pick<
      TableTabState,
      "kind" | "schema" | "table" | "page" | "pageSize" | "sorts"
    > &
      Partial<Pick<TableTabState, "filter">> & {
      /** Pre-multi-sort snapshots; migrated into `sorts` on revive. */
      orderBy?: string;
      orderDir?: "asc" | "desc";
    })
  | Pick<StructureTabState, "kind" | "schema" | "table" | "section">
  | (Pick<ActivityTabState, "kind" | "refreshSec" | "includeIdle"> & {
      /** Pre-rename snapshots; migrated into `includeIdle` on revive. */
      showIdle?: boolean;
    });

interface PersistedTab {
  title: string;
  state: PersistedTabState;
}

interface PersistedWorkspace {
  tabs: PersistedTab[];
  activeIndex: number;
}

function snapshotTab(tab: Tab): PersistedTab {
  const st = tab.state;
  const state: PersistedTabState =
    st.kind === "query"
      ? {
          kind: "query",
          sql: st.sql,
          maxRows: st.maxRows,
          savedQueryId: st.savedQueryId,
          isolated: st.isolated,
          commitMode: st.commitMode,
        }
      : st.kind === "table"
        ? {
            kind: "table",
            schema: st.schema,
            table: st.table,
            page: st.page,
            pageSize: st.pageSize,
            sorts: st.sorts,
            filter: st.filter,
          }
        : st.kind === "structure"
          ? {
              kind: "structure",
              schema: st.schema,
              table: st.table,
              section: st.section,
            }
          : {
              kind: "activity",
              refreshSec: st.refreshSec,
              includeIdle: st.includeIdle,
            };
  return { title: tab.title, state };
}

export function persistWorkspace(
  s: { tabs: Tab[]; activeTabId: string | null },
  profileId: string,
) {
  const tabs = s.tabs.filter((t) => t.profileId === profileId);
  const ws: PersistedWorkspace = {
    tabs: tabs.map(snapshotTab),
    activeIndex: tabs.findIndex((t) => t.id === s.activeTabId),
  };
  writeJson(wsKey(profileId), ws);
}

export function removeWorkspace(profileId: string) {
  try {
    localStorage.removeItem(wsKey(profileId));
  } catch {
    // best-effort
  }
}

/** Rebuilds a live tab state from its persisted snapshot (null = corrupt). */
function reviveTab(p: PersistedTab): { title: string; state: Tab["state"] } | null {
  const st = p?.state;
  if (!st || typeof st !== "object") return null;
  if (st.kind === "query") {
    return {
      title: p.title,
      state: {
        kind: "query",
        sql: st.sql ?? "",
        running: false,
        maxRows: st.maxRows || 1000,
        savedQueryId: st.savedQueryId,
        // isolation intent survives; the connection reopens lazily (no sessionId)
        isolated: st.isolated,
        commitMode: st.commitMode,
      },
    };
  }
  if (st.kind === "table" && st.schema && st.table) {
    // pre-multi-sort snapshots carry orderBy/orderDir instead of sorts
    const sorts = Array.isArray(st.sorts)
      ? st.sorts.filter((s) => s && typeof s.column === "string")
      : st.orderBy
        ? [
            {
              column: st.orderBy,
              dir: st.orderDir === "desc" ? ("desc" as const) : ("asc" as const),
            },
          ]
        : [];
    return {
      title: p.title,
      state: {
        kind: "table",
        schema: st.schema,
        table: st.table,
        page: st.page ?? 0,
        pageSize: st.pageSize || 100,
        sorts,
        filter: typeof st.filter === "string" ? st.filter : "",
        loading: false,
        edits: {},
        deletes: [],
        inserts: [],
      },
    };
  }
  if (st.kind === "structure" && st.schema && st.table) {
    return {
      title: p.title,
      state: {
        kind: "structure",
        schema: st.schema,
        table: st.table,
        section: st.section ?? "columns",
        loading: false,
        colEdits: {},
        colDrops: [],
        colAdds: [],
      },
    };
  }
  if (st.kind === "activity") {
    return {
      title: p.title,
      state: {
        kind: "activity",
        loading: false,
        refreshSec: typeof st.refreshSec === "number" ? st.refreshSec : 5,
        // pre-rename snapshots carry the flag as `showIdle`
        includeIdle: Boolean(st.includeIdle ?? st.showIdle),
      },
    };
  }
  return null;
}

export function restoreWorkspace(
  profileId: string,
): { tabs: Tab[]; activeId: string | null } | null {
  const ws = readJson<PersistedWorkspace>(wsKey(profileId));
  if (!ws || !Array.isArray(ws.tabs)) return null;
  const tabs: Tab[] = [];
  const seen = new Set<string>();
  for (const p of ws.tabs) {
    // exact duplicates are artifacts of a past double-restore — collapse them
    const key = JSON.stringify(p);
    if (seen.has(key)) continue;
    seen.add(key);
    const revived = reviveTab(p);
    if (revived) {
      tabs.push({ id: crypto.randomUUID(), profileId, ...revived });
    }
  }
  if (tabs.length === 0) return null;
  const active = tabs[Math.min(Math.max(ws.activeIndex ?? 0, 0), tabs.length - 1)];
  return { tabs, activeId: active?.id ?? null };
}

// --- Closed-tabs stack (⌘W → ⌘⇧T), survives app restarts ---------------------

const CLOSED_KEY = "sqlt.closedTabs";
export const CLOSED_CAP = 10;

export interface ClosedTab {
  tab: Tab;
  /** Position the tab had in the tab bar, for ⌘⇧T to put it back. */
  index: number;
}

type PersistedClosedTab = PersistedTab & { profileId: string; index: number };

/** Light snapshot without result/page data. */
export function persistClosedTabs(entries: ClosedTab[]) {
  const light: PersistedClosedTab[] = entries.map(({ tab, index }) => ({
    ...snapshotTab(tab),
    profileId: tab.profileId,
    index,
  }));
  writeJson(CLOSED_KEY, light);
}

/** Restored without result/page data — reopening refetches what's missing. */
export function restoreClosedTabs(): ClosedTab[] {
  const raw = readJson<PersistedClosedTab[]>(CLOSED_KEY);
  if (!Array.isArray(raw)) return [];
  const out: ClosedTab[] = [];
  for (const p of raw) {
    if (!p?.profileId) continue;
    const revived = reviveTab(p);
    if (revived) {
      out.push({
        tab: { id: crypto.randomUUID(), profileId: p.profileId, ...revived },
        index: p.index ?? 0,
      });
    }
  }
  return out.slice(-CLOSED_CAP);
}

// --- Agent session config options (last seen per provider) -------------------
// Кэш для панели: селекторы модели/режима видны до старта первой сессии
// нового чата — реальные значения приедут с session/new.

const agentCfgKey = (providerId: string) => `sqlt.agentConfig.${providerId}`;

export const loadAgentConfigOptions = (
  providerId: string,
): SessionConfigOption[] =>
  readJson<SessionConfigOption[]>(agentCfgKey(providerId)) ?? [];

export function saveAgentConfigOptions(
  providerId: string,
  options: SessionConfigOption[],
) {
  writeJson(agentCfgKey(providerId), options);
}

// --- Agent chats (per profile, resumable from the panel) ----------------------

const agentChatsKey = (profileId: string) => `sqlt.agentChats.${profileId}`;
export const AGENT_CHATS_CAP = 10;

export interface PersistedAgentChat {
  id: string;
  /** First user message, truncated — the label in the chat picker. */
  title: string;
  providerId: string;
  updatedAt: number;
  /** Items without ids (reassigned on restore); tool statuses are settled. */
  items: AgentChatItemBody[];
}

/** Last saved items reference per chatId: the debounced auto-persist upserts on
 *  every store change — an unchanged chat must not be rewritten (or get a fresh
 *  updatedAt reordering the list). */
const savedItemsRef = new Map<string, AgentChatItem[]>();

function snapshotChatItems(items: AgentChatItem[]): AgentChatItemBody[] {
  return items.map((it) => {
    const { id: _id, ...body } = it;
    // an unfinished tool call never completes in a future agent process
    return body.kind === "tool" &&
      (body.status === "pending" || body.status === "in_progress")
      ? { ...body, status: "failed" as const }
      : body;
  });
}

export function loadAgentChats(profileId: string): PersistedAgentChat[] {
  const raw = readJson<PersistedAgentChat[]>(agentChatsKey(profileId));
  if (!Array.isArray(raw)) return [];
  return raw.filter(
    (c) => c && typeof c.id === "string" && Array.isArray(c.items),
  );
}

/** Chats without a user message (nothing but an error banner) are skipped. */
export function upsertAgentChat(
  profileId: string,
  chat: { chatId: string; providerId: string; items: AgentChatItem[] },
) {
  if (savedItemsRef.get(chat.chatId) === chat.items) return;
  const firstUser = chat.items.find((it) => it.kind === "user");
  if (!firstUser || firstUser.kind !== "user") return;
  const rest = loadAgentChats(profileId).filter((c) => c.id !== chat.chatId);
  const list: PersistedAgentChat[] = [
    {
      id: chat.chatId,
      title: firstUser.text.slice(0, 80),
      providerId: chat.providerId,
      updatedAt: Date.now(),
      items: snapshotChatItems(chat.items),
    },
    ...rest,
  ];
  writeJson(agentChatsKey(profileId), list.slice(0, AGENT_CHATS_CAP));
  savedItemsRef.set(chat.chatId, chat.items);
}

export function deleteAgentChat(profileId: string, chatId: string) {
  const rest = loadAgentChats(profileId).filter((c) => c.id !== chatId);
  if (rest.length === 0) removeAgentChats(profileId);
  else writeJson(agentChatsKey(profileId), rest);
}

export function removeAgentChats(profileId: string) {
  try {
    localStorage.removeItem(agentChatsKey(profileId));
  } catch {
    // best-effort
  }
}
