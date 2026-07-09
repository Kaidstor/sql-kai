import { beforeEach, describe, expect, it } from "vitest";
import {
  CLOSED_CAP,
  loadClosedTabs,
  persistClosedTabs,
  persistWorkspace,
  restoreWorkspace,
  type ClosedTab,
} from "./persist";
import type { Tab } from "./store";

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
    const out = loadClosedTabs();
    expect(out).toHaveLength(2);
    expect(out[1].index).toBe(2);
    expect(out[1].tab.profileId).toBe("p1");
    expect(out[1].tab.state).toMatchObject({ kind: "query", sql: "SELECT 2" });
  });

  it("keeps only the newest CLOSED_CAP entries", () => {
    persistClosedTabs(Array.from({ length: CLOSED_CAP + 5 }, (_, i) => closed(i)));
    const out = loadClosedTabs();
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
    expect(loadClosedTabs()).toEqual([]);
  });
});
