import { describe, expect, it } from "vitest";
import {
  compileFilters,
  enumLabelsFor,
  isTextType,
  parseFilter,
  type FilterCond,
} from "./filters";

describe("compileFilters", () => {
  it("compiles the basic operators with quoted literals", () => {
    expect(compileFilters([{ column: "id", op: "eq", value: "3" }])).toBe(
      `"id" = '3'`,
    );
    expect(compileFilters([{ column: "name", op: "neq", value: "it's" }])).toBe(
      `"name" <> 'it''s'`,
    );
    expect(compileFilters([{ column: "n", op: "gte", value: "10" }])).toBe(
      `"n" >= '10'`,
    );
    expect(compileFilters([{ column: "name", op: "ilike", value: "%a%" }])).toBe(
      `"name" ILIKE '%a%'`,
    );
    expect(compileFilters([{ column: "name", op: "nlike", value: "x" }])).toBe(
      `"name" NOT LIKE 'x'`,
    );
  });

  it("compiles null ops without a value", () => {
    expect(compileFilters([{ column: "parent", op: "null", value: null }])).toBe(
      `"parent" IS NULL`,
    );
    expect(
      compileFilters([{ column: "parent", op: "notnull", value: null }]),
    ).toBe(`"parent" IS NOT NULL`);
  });

  it("compiles IN from a comma-separated list", () => {
    expect(compileFilters([{ column: "s", op: "in", value: "a, b ,c" }])).toBe(
      `"s" IN ('a', 'b', 'c')`,
    );
  });

  it("skips inactive rows and AND-joins active ones", () => {
    const conds: FilterCond[] = [
      { column: "a", op: "eq", value: "1" },
      { column: "b", op: "eq", value: null }, // untouched — inactive
      { column: "c", op: "null", value: null },
    ];
    expect(compileFilters(conds)).toBe(`"a" = '1' AND "c" IS NULL`);
  });

  it("empty string is an explicit value", () => {
    expect(compileFilters([{ column: "a", op: "eq", value: "" }])).toBe(
      `"a" = ''`,
    );
  });

  it("quotes weird column names", () => {
    expect(compileFilters([{ column: 'my "col"', op: "eq", value: "1" }])).toBe(
      `"my ""col""" = '1'`,
    );
  });
});

describe("parseFilter", () => {
  it("round-trips compiled filters", () => {
    const conds: FilterCond[] = [
      { column: "id", op: "gte", value: "5" },
      { column: "name", op: "ilike", value: "%x%" },
      { column: "parent", op: "null", value: null },
      { column: "s", op: "in", value: "a, b" },
    ];
    expect(parseFilter(compileFilters(conds))).toEqual(conds);
  });

  it("parses FK-navigation filters", () => {
    expect(parseFilter(`"user_id" = '42'`)).toEqual([
      { column: "user_id", op: "eq", value: "42" },
    ]);
    expect(parseFilter(`"a" = '1' AND "b" IS NULL`)).toEqual([
      { column: "a", op: "eq", value: "1" },
      { column: "b", op: "null", value: null },
    ]);
  });

  it("parses bare identifiers, numbers and escaped quotes", () => {
    expect(parseFilter("id = 3")).toEqual([
      { column: "id", op: "eq", value: "3" },
    ]);
    expect(parseFilter("name <> 'it''s'")).toEqual([
      { column: "name", op: "neq", value: "it's" },
    ]);
    expect(parseFilter("ok = true")).toEqual([
      { column: "ok", op: "eq", value: "true" },
    ]);
  });

  it("empty input is an empty builder, not raw mode", () => {
    expect(parseFilter("")).toEqual([]);
    expect(parseFilter("   ")).toEqual([]);
  });

  it("rejects expressions the builder can't represent", () => {
    expect(parseFilter("a = 1 OR b = 2")).toBeNull();
    expect(parseFilter("created_at > now() - interval '1 day'")).toBeNull();
    expect(parseFilter("(a = 1)")).toBeNull();
    expect(parseFilter("a = ")).toBeNull();
    expect(parseFilter("lower(name) = 'x'")).toBeNull();
  });

  it("does not confuse word operators with identifiers", () => {
    // column named "inbox": IN must not match its prefix
    expect(parseFilter("inbox = 'a'")).toEqual([
      { column: "inbox", op: "eq", value: "a" },
    ]);
  });
});

describe("enumLabelsFor", () => {
  const enums = [
    { schema: "public", name: "status", labels: ["new", "done"] },
    { schema: "app", name: "level", labels: ["low", "high"] },
    { schema: "app", name: "My Type", labels: ["a"] },
  ];

  it("matches bare, qualified and array types", () => {
    expect(enumLabelsFor("status", enums)).toEqual(["new", "done"]);
    expect(enumLabelsFor("app.level", enums)).toEqual(["low", "high"]);
    expect(enumLabelsFor("status[]", enums)).toEqual(["new", "done"]);
    expect(enumLabelsFor('app."My Type"', enums)).toEqual(["a"]);
  });

  it("returns null for non-enums", () => {
    expect(enumLabelsFor("integer", enums)).toBeNull();
    expect(enumLabelsFor("timestamp with time zone", enums)).toBeNull();
    expect(enumLabelsFor("status", [])).toBeNull();
    expect(enumLabelsFor("status", undefined)).toBeNull();
  });
});

describe("isTextType", () => {
  it("text-ish types get the EMPTY STRING suggestion", () => {
    expect(isTextType("text")).toBe(true);
    expect(isTextType("character varying(255)")).toBe(true);
    expect(isTextType("character(1)")).toBe(true);
    expect(isTextType("citext")).toBe(true);
  });

  it("non-text types don't", () => {
    expect(isTextType("integer")).toBe(false);
    expect(isTextType("timestamp with time zone")).toBe(false);
    expect(isTextType("uuid")).toBe(false);
  });
});
