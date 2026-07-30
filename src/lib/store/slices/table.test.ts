import { describe, expect, it, vi } from "vitest";
import type { ColumnInfo, RelationInfo, TablePageResult } from "../../types";
import type { Set } from "../context";
import type { AppStore, Tab, TableTabState } from "../types";

const { executeSql } = vi.hoisted(() => ({ executeSql: vi.fn() }));

vi.mock("../../api", () => ({
  api: { executeSql },
  errText: (e: unknown) => (e instanceof Error ? e.message : String(e)),
  isSessionLost: () => false,
  isReadOnlyRefusal: (e: unknown) =>
    typeof e === "object" && e !== null && (e as { code?: string }).code === "read_only",
}));
vi.mock("../../persist", () => ({ restoreWorkspace: () => null }));

import { createStoreContext } from "../context";
import { columnsKey } from "../helpers";
import { createTableSlice } from "./table";

const REFUSAL = { code: "read_only", message: "read-only session" };

const columns: ColumnInfo[] = [
  { name: "id", dataType: "int4", nullable: false, isPk: true },
  { name: "name", dataType: "text", nullable: true, isPk: false },
];

const data: TablePageResult = {
  result: {
    columns: ["id", "name", "org_id"],
    rows: [["1", "ann", "7"]],
    truncated: false,
  },
  durationMs: 0,
  approxRows: 1,
};

const relations: RelationInfo[] = [
  {
    name: "users_org_id_fkey",
    columns: "org_id",
    refTable: "app.orgs",
    refColumns: "id",
    onUpdate: "NO ACTION",
    onDelete: "NO ACTION",
  },
];

/** Store with one production profile, one connected table tab, one staged edit. */
const setup = (confirm = true) => {
  const confirmDialog = vi.fn(async () => confirm);
  const showToast = vi.fn();
  const state = {} as AppStore;
  const set = ((
    update: Partial<AppStore> | ((snapshot: AppStore) => Partial<AppStore>),
  ) => {
    Object.assign(state, typeof update === "function" ? update(state) : update);
  }) as Set;
  const get = () => state;
  const tabState: TableTabState = {
    kind: "table",
    schema: "public",
    table: "users",
    page: 0,
    pageSize: 100,
    sorts: [],
    filter: "",
    loading: false,
    edits: { 0: { 1: "bob" } },
    deletes: [],
    inserts: [],
    data,
  };
  const tab: Tab = { id: "t1", profileId: "p1", title: "users", state: tabState };
  Object.assign(state, {
    tabs: [tab],
    profiles: [{ id: "p1", name: "prod-db", production: true }],
    sessions: { p1: { sessionId: "s1" } },
    tableColumns: {
      [columnsKey({ profileId: "p1", schema: "public", table: "users" })]: columns,
    },
    tableRelations: {
      [columnsKey({ profileId: "p1", schema: "public", table: "users" })]: relations,
    },
    confirmDialog,
    showToast,
  });
  Object.assign(state, createTableSlice(set, get, createStoreContext(set, get)));
  const current = () => (state.tabs[0].state as TableTabState);
  return { state, current, confirmDialog, showToast };
};

describe("applyTableEdits on a production profile", () => {
  it("asks exactly once — the backend read-only refusal is the only prompt", async () => {
    executeSql.mockReset();
    executeSql.mockImplementation((...a: unknown[]) =>
      a[5] ? Promise.resolve({}) : Promise.reject(REFUSAL),
    );
    const { state, current, confirmDialog } = setup();

    await state.applyTableEdits("t1");

    expect(confirmDialog).toHaveBeenCalledTimes(1);
    expect(confirmDialog).toHaveBeenCalledWith(
      expect.objectContaining({
        title: '"prod-db" is PRODUCTION',
        message: "Apply 1 UPDATE?",
      }),
    );
    expect(executeSql).toHaveBeenCalledTimes(2);
    expect(current().edits).toEqual({});
    expect(current().data?.result.rows).toEqual([["1", "bob", "7"]]);
  });

  it("declining leaves the edits staged and unmarked", async () => {
    executeSql.mockReset();
    executeSql.mockImplementation((...a: unknown[]) =>
      a[5] ? Promise.resolve({}) : Promise.reject(REFUSAL),
    );
    const { state, current, showToast } = setup(false);

    await state.applyTableEdits("t1");

    expect(current().edits).toEqual({ 0: { 1: "bob" } });
    expect(current().loading).toBe(false);
    expect(current().applyFailed).toBeFalsy();
    expect(showToast).not.toHaveBeenCalled();
  });
});

describe("previewFk", () => {
  it("selects the referenced rows and keeps the WHERE for \"Open table\"", async () => {
    executeSql.mockReset();
    executeSql.mockResolvedValue({
      results: [{ columns: ["id"], rows: [["7"]], truncated: false }],
    });
    const { state, current } = setup();

    await state.previewFk("t1", 0, 2);

    expect(executeSql.mock.calls[0][1]).toBe(
      'SELECT * FROM "app"."orgs" WHERE "id" = \'7\' LIMIT 50',
    );
    expect(current().fkPreview).toMatchObject({
      target: { schema: "app", table: "orgs" },
      filter: '"id" = \'7\'',
      loading: false,
    });
  });

  it("a page reload drops the panel and the in-flight answer", async () => {
    executeSql.mockReset();
    let resolve!: (v: unknown) => void;
    executeSql.mockReturnValue(new Promise((r) => (resolve = r)));
    const { state, current } = setup();

    const pending = state.previewFk("t1", 0, 2);
    expect(current().fkPreview?.loading).toBe(true);
    state.closeFkPreview("t1");
    resolve({ results: [{ columns: ["id"], rows: [["7"]], truncated: false }] });
    await pending;

    expect(current().fkPreview).toBeUndefined();
  });
});
