import { describe, expect, it } from "vitest";
import { buildStructureDdl, buildTableDml } from "./mutationSql";
import type { StructureTabState, TableTabState } from "./store";
import type { ColumnInfo, TablePageResult } from "./types";

// --- buildStructureDdl ---------------------------------------------------------

const structureState = (
  patch: Partial<StructureTabState>,
): StructureTabState => ({
  kind: "structure",
  schema: "public",
  table: "users",
  section: "columns",
  loading: false,
  colEdits: {},
  colDrops: [],
  colAdds: [],
  ...patch,
});

describe("buildStructureDdl", () => {
  it("emits one ALTER per staged field, rename last", () => {
    const stmts = buildStructureDdl(
      structureState({
        colEdits: {
          age: { type: "bigint", nullable: true, name: "years" },
        },
      }),
    );
    expect(stmts).toEqual([
      'ALTER TABLE "public"."users" ALTER COLUMN "age" TYPE bigint',
      'ALTER TABLE "public"."users" ALTER COLUMN "age" DROP NOT NULL',
      'ALTER TABLE "public"."users" RENAME COLUMN "age" TO "years"',
    ]);
  });

  it("SET/DROP DEFAULT and comment set/clear", () => {
    const stmts = buildStructureDdl(
      structureState({
        colEdits: {
          a: { default: "now()", comment: "when" },
          b: { default: "", comment: "" },
        },
      }),
    );
    expect(stmts).toEqual([
      'ALTER TABLE "public"."users" ALTER COLUMN "a" SET DEFAULT now()',
      'COMMENT ON COLUMN "public"."users"."a" IS \'when\'',
      'ALTER TABLE "public"."users" ALTER COLUMN "b" DROP DEFAULT',
      'COMMENT ON COLUMN "public"."users"."b" IS NULL',
    ]);
  });

  it("skips edits of dropped columns and emits drops + adds", () => {
    const stmts = buildStructureDdl(
      structureState({
        colEdits: { legacy: { type: "text" } },
        colDrops: ["legacy"],
        colAdds: [
          { name: "flag", type: "boolean", nullable: false, def: "false" },
          { name: "note", type: "text", nullable: true, def: "" },
        ],
      }),
    );
    expect(stmts).toEqual([
      'ALTER TABLE "public"."users" DROP COLUMN "legacy"',
      'ALTER TABLE "public"."users" ADD COLUMN "flag" boolean NOT NULL DEFAULT false',
      'ALTER TABLE "public"."users" ADD COLUMN "note" text',
    ]);
  });

  it("SET NOT NULL when nullable staged to false", () => {
    const stmts = buildStructureDdl(
      structureState({ colEdits: { a: { nullable: false } } }),
    );
    expect(stmts).toEqual([
      'ALTER TABLE "public"."users" ALTER COLUMN "a" SET NOT NULL',
    ]);
  });
});

// --- buildTableDml -------------------------------------------------------------

const page = (
  columns: string[],
  rows: (string | null)[][],
): TablePageResult => ({
  result: { columns, rows, truncated: false },
  durationMs: 0,
  approxRows: rows.length,
});

const col = (name: string, isPk = false): ColumnInfo => ({
  name,
  dataType: "text",
  nullable: !isPk,
  isPk,
});

const tableState = (patch: Partial<TableTabState>): TableTabState => ({
  kind: "table",
  schema: "public",
  table: "users",
  page: 0,
  pageSize: 100,
  sorts: [],
  filter: "",
  loading: false,
  edits: {},
  deletes: [],
  inserts: [],
  ...patch,
});

describe("buildTableDml", () => {
  const cols = [col("id", true), col("name"), col("note")];
  const data = page(
    ["id", "name", "note"],
    [
      ["1", "ann", null],
      ["2", "bob", "x"],
    ],
  );

  it("refuses UPDATE/DELETE without a primary key", () => {
    const dml = buildTableDml(
      tableState({ edits: { 0: { 1: "new" } } }),
      data,
      [col("id"), col("name")],
    );
    expect(dml).toEqual({
      error: "Table has no primary key — editing is disabled",
    });
  });

  it("refuses when a pk column is missing from the page", () => {
    const dml = buildTableDml(
      tableState({ deletes: [0] }),
      page(["name"], [["ann"]]),
      cols,
    );
    expect(dml).toEqual({
      error: 'Primary key column "id" is missing from the page',
    });
  });

  it("builds UPDATE by original pk value even when the pk cell is edited", () => {
    const dml = buildTableDml(
      tableState({ edits: { 0: { 0: "99", 2: null } } }),
      data,
      cols,
    );
    expect(dml).toEqual({
      stmts: [
        'UPDATE "public"."users" SET "id" = \'99\', "note" = NULL WHERE "id" = \'1\'',
      ],
      updates: 1,
      deletes: 0,
    });
  });

  it("DELETE wins over an edit of the same row; NULL pk compares with IS NULL", () => {
    const nullPkData = page(["id", "name"], [[null, "ann"]]);
    const dml = buildTableDml(
      tableState({ edits: { 0: { 1: "x" } }, deletes: [0] }),
      nullPkData,
      [col("id", true), col("name")],
    );
    expect(dml).toEqual({
      stmts: ['DELETE FROM "public"."users" WHERE "id" IS NULL'],
      updates: 0,
      deletes: 1,
    });
  });

  it("INSERT skips undefined (generated) cells; all-undefined → DEFAULT VALUES", () => {
    const dml = buildTableDml(
      tableState({
        inserts: [
          { values: [undefined, "carl", null] },
          { values: [undefined, undefined, undefined] },
        ],
      }),
      data,
      cols,
    );
    expect(dml).toEqual({
      stmts: [
        'INSERT INTO "public"."users" ("name", "note") VALUES (\'carl\', NULL)',
        'INSERT INTO "public"."users" DEFAULT VALUES',
      ],
      updates: 0,
      deletes: 0,
    });
  });

  it("inserts need no primary key", () => {
    const dml = buildTableDml(
      tableState({ inserts: [{ values: [undefined, "x", null] }] }),
      data,
      [col("id"), col("name"), col("note")],
    );
    expect("stmts" in dml && dml.stmts.length).toBe(1);
  });
});
