// localStorage persistence: query history, per-profile workspaces (open tabs)
// and the closed-tabs stack. Everything here is best-effort — storage errors
// are swallowed, corrupt snapshots are dropped.
import type {
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
  | Pick<QueryTabState, "kind" | "sql" | "maxRows" | "savedQueryId">
  | (Pick<
      TableTabState,
      "kind" | "schema" | "table" | "page" | "pageSize" | "sorts"
    > & {
      /** Pre-multi-sort snapshots; migrated into `sorts` on revive. */
      orderBy?: string;
      orderDir?: "asc" | "desc";
    })
  | Pick<StructureTabState, "kind" | "schema" | "table" | "section">;

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
        }
      : st.kind === "table"
        ? {
            kind: "table",
            schema: st.schema,
            table: st.table,
            page: st.page,
            pageSize: st.pageSize,
            sorts: st.sorts,
          }
        : {
            kind: "structure",
            schema: st.schema,
            table: st.table,
            section: st.section,
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
export function loadClosedTabs(): ClosedTab[] {
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
