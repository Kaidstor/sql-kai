import { beforeEach, describe, expect, it } from "vitest";
import {
  AGENT_CHATS_CAP,
  CLOSED_CAP,
  deleteAgentChat,
  loadAgentChats,
  persistClosedTabs,
  persistWorkspace,
  restoreClosedTabs,
  restoreWorkspace,
  upsertAgentChat,
  type ClosedTab,
} from "./persist";
import type { AgentChatItem, Tab } from "./store";

// persist.ts is best-effort over localStorage — a Map-backed stub is enough
const backing = new Map<string, string>();
beforeEach(() => {
  backing.clear();
  globalThis.localStorage = {
    getItem: (k: string) => backing.get(k) ?? null,
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
    key: () => null,
    length: 0,
  } as Storage;
});

const queryTab = (id: string, profileId: string, sql: string): Tab => ({
  id,
  profileId,
  title: `Query ${id}`,
  state: {
    kind: "query",
    sql,
    running: true, // transient — must not survive persistence
    maxRows: 500,
    isolated: true,
    sessionId: "ephemeral", // ephemeral — must not survive persistence
    commitMode: "manual",
  },
});

const tableTab = (id: string, profileId: string): Tab => ({
  id,
  profileId,
  title: "users",
  state: {
    kind: "table",
    schema: "public",
    table: "users",
    page: 3,
    pageSize: 50,
    sorts: [{ column: "id", dir: "desc" }],
    filter: "id > 10",
    loading: true,
    edits: { 0: { 1: "staged" } }, // staged — must not survive persistence
    deletes: [2],
    inserts: [{ values: ["x"] }],
    error: "boom", // must not survive persistence
  },
});

describe("workspace round-trip", () => {
  it("restores tabs with transient state reset and fresh ids", () => {
    const tabs = [queryTab("q1", "p1", "SELECT 1"), tableTab("t1", "p1")];
    persistWorkspace({ tabs, activeTabId: "t1" }, "p1");

    const ws = restoreWorkspace("p1")!;
    expect(ws.tabs).toHaveLength(2);

    const q = ws.tabs[0];
    expect(q.id).not.toBe("q1"); // fresh id
    expect(q.state).toMatchObject({
      kind: "query",
      sql: "SELECT 1",
      maxRows: 500,
      running: false,
      isolated: true,
      commitMode: "manual",
    });
    expect((q.state as { sessionId?: string }).sessionId).toBeUndefined();

    const t = ws.tabs[1];
    expect(t.state).toMatchObject({
      kind: "table",
      schema: "public",
      table: "users",
      page: 3,
      pageSize: 50,
      sorts: [{ column: "id", dir: "desc" }],
      filter: "id > 10",
      loading: false,
      edits: {},
      deletes: [],
      inserts: [],
    });
    expect((t.state as { error?: string }).error).toBeUndefined();

    expect(ws.activeId).toBe(t.id);
  });

  it("persists only the requested profile's tabs", () => {
    persistWorkspace(
      { tabs: [queryTab("q1", "p1", "a"), queryTab("q2", "p2", "b")], activeTabId: null },
      "p1",
    );
    expect(restoreWorkspace("p1")!.tabs).toHaveLength(1);
    expect(restoreWorkspace("p2")).toBeNull();
  });

  it("migrates legacy orderBy/orderDir snapshots into sorts", () => {
    backing.set(
      "sqlt.workspace.p1",
      JSON.stringify({
        tabs: [
          {
            title: "users",
            state: {
              kind: "table",
              schema: "public",
              table: "users",
              page: 0,
              pageSize: 100,
              orderBy: "name",
              orderDir: "desc",
            },
          },
        ],
        activeIndex: 0,
      }),
    );
    const ws = restoreWorkspace("p1")!;
    expect(ws.tabs[0].state).toMatchObject({
      sorts: [{ column: "name", dir: "desc" }],
    });
  });

  it("drops corrupt entries, collapses exact duplicates, clamps activeIndex", () => {
    const snapshot = {
      title: "Query 1",
      state: { kind: "query", sql: "SELECT 1", maxRows: 1000 },
    };
    backing.set(
      "sqlt.workspace.p1",
      JSON.stringify({
        tabs: [
          snapshot,
          snapshot, // duplicate of a past double-restore
          { title: "broken", state: { kind: "table" } }, // no schema/table
          null,
        ],
        activeIndex: 99,
      }),
    );
    const ws = restoreWorkspace("p1")!;
    expect(ws.tabs).toHaveLength(1);
    expect(ws.activeId).toBe(ws.tabs[0].id);
  });

  it("null when nothing was persisted or nothing revives", () => {
    expect(restoreWorkspace("nope")).toBeNull();
    backing.set("sqlt.workspace.p1", "not json");
    expect(restoreWorkspace("p1")).toBeNull();
  });
});

describe("closed-tabs stack", () => {
  const closed = (n: number): ClosedTab => ({
    tab: queryTab(`q${n}`, "p1", `SELECT ${n}`),
    index: n,
  });

  it("round-trips with profile and bar position", () => {
    persistClosedTabs([closed(1), closed(2)]);
    const out = restoreClosedTabs();
    expect(out).toHaveLength(2);
    expect(out[1].index).toBe(2);
    expect(out[1].tab.profileId).toBe("p1");
    expect(out[1].tab.state).toMatchObject({ kind: "query", sql: "SELECT 2" });
  });

  it("keeps only the newest CLOSED_CAP entries", () => {
    persistClosedTabs(Array.from({ length: CLOSED_CAP + 5 }, (_, i) => closed(i)));
    const out = restoreClosedTabs();
    expect(out).toHaveLength(CLOSED_CAP);
    expect(out[out.length - 1].index).toBe(CLOSED_CAP + 4);
  });

  it("skips entries without a profile", () => {
    backing.set(
      "sqlt.closedTabs",
      JSON.stringify([
        { title: "x", state: { kind: "query", sql: "1" }, index: 0 },
      ]),
    );
    expect(restoreClosedTabs()).toEqual([]);
  });
});

describe("saved agent chats", () => {
  const userItem = (id: number, text: string): AgentChatItem => ({
    id,
    kind: "user",
    text,
  });

  it("round-trips with ids stripped and unfinished tools settled", () => {
    const items: AgentChatItem[] = [
      userItem(7, "how many users?"),
      { id: 8, kind: "assistant", text: "42" },
      {
        id: 9,
        kind: "tool",
        toolCallId: "t1",
        title: "query",
        toolKind: "execute",
        status: "in_progress",
      },
    ];
    upsertAgentChat("p1", { chatId: "c-round", providerId: "claude", items });

    const [saved] = loadAgentChats("p1");
    expect(saved.title).toBe("how many users?");
    expect(saved.providerId).toBe("claude");
    expect(saved.items).toHaveLength(3);
    expect("id" in saved.items[0]).toBe(false);
    expect(saved.items[2]).toMatchObject({ kind: "tool", status: "failed" });
  });

  it("skips chats without a user message", () => {
    upsertAgentChat("p1", {
      chatId: "c-noise",
      providerId: "claude",
      items: [{ id: 1, kind: "assistant", text: "hi" }],
    });
    expect(loadAgentChats("p1")).toEqual([]);
  });

  it("does not rewrite an unchanged items reference", () => {
    const items = [userItem(1, "q")];
    upsertAgentChat("p1", { chatId: "c-ref", providerId: "claude", items });
    // подменяем запись руками: повторный upsert той же ссылки не должен её тронуть
    backing.set(
      "sqlt.agentChats.p1",
      JSON.stringify([
        { id: "c-ref", title: "MANUAL", providerId: "claude", updatedAt: 1, items: [] },
      ]),
    );
    upsertAgentChat("p1", { chatId: "c-ref", providerId: "claude", items });
    expect(loadAgentChats("p1")[0].title).toBe("MANUAL");

    upsertAgentChat("p1", {
      chatId: "c-ref",
      providerId: "claude",
      items: [...items, userItem(2, "more")],
    });
    expect(loadAgentChats("p1")[0].title).toBe("q");
  });

  it("keeps the newest AGENT_CHATS_CAP chats, newest first", () => {
    for (let i = 0; i <= AGENT_CHATS_CAP + 2; i++) {
      upsertAgentChat("p1", {
        chatId: `c-cap-${i}`,
        providerId: "claude",
        items: [userItem(1, `q${i}`)],
      });
    }
    const list = loadAgentChats("p1");
    expect(list).toHaveLength(AGENT_CHATS_CAP);
    expect(list[0].title).toBe(`q${AGENT_CHATS_CAP + 2}`);
  });

  it("deletes one entry; the last delete drops the key", () => {
    upsertAgentChat("p1", {
      chatId: "c-d1",
      providerId: "claude",
      items: [userItem(1, "a")],
    });
    upsertAgentChat("p1", {
      chatId: "c-d2",
      providerId: "claude",
      items: [userItem(1, "b")],
    });
    deleteAgentChat("p1", "c-d2");
    expect(loadAgentChats("p1").map((c) => c.id)).toEqual(["c-d1"]);
    deleteAgentChat("p1", "c-d1");
    expect(backing.has("sqlt.agentChats.p1")).toBe(false);
  });
});
