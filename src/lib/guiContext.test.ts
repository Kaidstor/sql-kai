import { describe, expect, it } from "vitest";
import { buildGuiContext } from "./guiContext";
import type { Tab } from "./store/types";
import type { StatementResult } from "./types";

const result: StatementResult = {
  columns: ["id", "name", "email"],
  rows: [
    ["1", "alice", "a@x.io"],
    ["2", "bob", "b@x.io"],
    ["3", null, "c@x.io"],
  ],
  rowsAffected: null,
  truncated: false,
};

function tableTab(overrides?: Partial<Tab>): Tab {
  return {
    id: "t1",
    profileId: "p1",
    title: "users",
    state: {
      kind: "table",
      schema: "public",
      table: "users",
      page: 2,
      pageSize: 100,
      sorts: [{ column: "id", dir: "desc" }],
      filter: "email LIKE '%x.io'",
      data: { result, durationMs: 5, approxRows: 300 },
      loading: false,
      edits: {},
      deletes: [],
      inserts: [],
    },
    ...overrides,
  };
}

const none = () => null;

describe("buildGuiContext", () => {
  it("нет активной вкладки / чужой профиль — note вместо данных", () => {
    expect(buildGuiContext({ tabs: [], activeTabId: null }, "p1", none)).toEqual(
      { note: "no active tab in the GUI" },
    );
    const ctx = buildGuiContext(
      { tabs: [tableTab()], activeTabId: "t1" },
      "other-profile",
      none,
    );
    expect(ctx).toHaveProperty("note");
  });

  it("таблица: вкладка с фильтром/сортировкой, без выделения — kind none", () => {
    const ctx = buildGuiContext(
      { tabs: [tableTab()], activeTabId: "t1" },
      "p1",
      none,
    ) as { tab: Record<string, unknown>; selection: { kind: string } };
    expect(ctx.tab).toMatchObject({
      kind: "table",
      schema: "public",
      table: "users",
      filter: "email LIKE '%x.io'",
      page: 2,
      approxRows: 300,
      loadedRows: 3,
    });
    expect(ctx.selection.kind).toBe("none");
  });

  it("выделенные строки уходят с данными и 1-based номерами", () => {
    const ctx = buildGuiContext(
      { tabs: [tableTab()], activeTabId: "t1" },
      "p1",
      () => ({ rows: [0, 2], cols: [], cellRect: null }),
    ) as { selection: Record<string, unknown> };
    expect(ctx.selection).toEqual({
      kind: "rows",
      rowNumbers: [1, 3],
      columns: ["id", "name", "email"],
      rows: [
        ["1", "alice", "a@x.io"],
        ["3", null, "c@x.io"],
      ],
    });
  });

  it("выделенная колонка — имена и значения только её", () => {
    const ctx = buildGuiContext(
      { tabs: [tableTab()], activeTabId: "t1" },
      "p1",
      () => ({ rows: [], cols: [1], cellRect: null }),
    ) as { selection: Record<string, unknown> };
    expect(ctx.selection).toEqual({
      kind: "columns",
      columns: ["name"],
      rows: [["alice"], ["bob"], [null]],
    });
  });

  it("прямоугольник ячеек — срез строк и колонок", () => {
    const ctx = buildGuiContext(
      { tabs: [tableTab()], activeTabId: "t1" },
      "p1",
      () => ({
        rows: [],
        cols: [],
        cellRect: { r1: 1, r2: 2, c1: 1, c2: 2 },
      }),
    ) as { selection: Record<string, unknown> };
    expect(ctx.selection).toEqual({
      kind: "cells",
      rowNumbers: [2, 3],
      columns: ["name", "email"],
      rows: [
        ["bob", "b@x.io"],
        [null, "c@x.io"],
      ],
    });
  });

  it("query-вкладка: sql и выделение по statement-гридам", () => {
    const tab: Tab = {
      id: "q1",
      profileId: "p1",
      title: "Query 1",
      state: {
        kind: "query",
        sql: "SELECT 1; SELECT 2",
        resultSql: "SELECT 1; SELECT 2",
        result: { results: [result, result], durationMs: 3 },
        running: false,
        maxRows: 1000,
      },
    };
    // выделение только во втором statement'е
    let call = 0;
    const ctx = buildGuiContext({ tabs: [tab], activeTabId: "q1" }, "p1", () =>
      call++ === 0 ? null : { rows: [1], cols: [], cellRect: null },
    ) as { tab: Record<string, unknown>; statement?: number; selection: { kind: string } };
    expect(ctx.tab).toMatchObject({ kind: "query", statements: 2 });
    expect(ctx.statement).toBe(1);
    expect(ctx.selection.kind).toBe("rows");
  });

  it("кап больших выделений строк", () => {
    const big: StatementResult = {
      columns: ["n"],
      rows: Array.from({ length: 300 }, (_, i) => [String(i)]),
      rowsAffected: null,
      truncated: false,
    };
    const t = tableTab();
    if (t.state.kind === "table" && t.state.data) t.state.data.result = big;
    const ctx = buildGuiContext({ tabs: [t], activeTabId: "t1" }, "p1", () => ({
      rows: Array.from({ length: 300 }, (_, i) => i),
      cols: [],
      cellRect: null,
    })) as { selection: { rows: unknown[]; capped?: number; totalSelected?: number } };
    expect(ctx.selection.rows).toHaveLength(100);
    expect(ctx.selection.capped).toBe(100);
    expect(ctx.selection.totalSelected).toBe(300);
  });
});
