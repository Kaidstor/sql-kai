import { describe, expect, it } from "vitest";
import {
  countStatements,
  dangerousStatements,
  parseRegclass,
  quoteIdent,
  quoteLit,
  relIdent,
  sqlPreview,
} from "./sql";

describe("quoting", () => {
  it("quotes identifiers, doubling inner quotes", () => {
    expect(quoteIdent("users")).toBe('"users"');
    expect(quoteIdent('my "col"')).toBe('"my ""col"""');
  });

  it("quotes literals, doubling inner apostrophes", () => {
    expect(quoteLit("it's")).toBe("'it''s'");
    expect(quoteLit("")).toBe("''");
  });

  it("qualifies relations", () => {
    expect(relIdent("public", "users")).toBe('"public"."users"');
  });
});

describe("sqlPreview", () => {
  it("flattens whitespace and truncates", () => {
    expect(sqlPreview("SELECT\n  1")).toBe("SELECT 1");
    expect(sqlPreview("x".repeat(200)).length).toBe(90);
  });
});

describe("parseRegclass", () => {
  it("bare name means public", () => {
    expect(parseRegclass("users")).toEqual({ schema: "public", table: "users" });
  });

  it("splits schema-qualified names", () => {
    expect(parseRegclass("app.users")).toEqual({ schema: "app", table: "users" });
  });

  it("unquotes quoted parts, including escaped quotes and dots", () => {
    expect(parseRegclass('"My-Table"')).toEqual({
      schema: "public",
      table: "My-Table",
    });
    expect(parseRegclass('app."My Table"')).toEqual({
      schema: "app",
      table: "My Table",
    });
    expect(parseRegclass('"a.b"."c""d"')).toEqual({ schema: "a.b", table: 'c"d' });
  });
});

describe("countStatements", () => {
  it("counts ;-separated statements, ignoring empties", () => {
    expect(countStatements("SELECT 1")).toBe(1);
    expect(countStatements("SELECT 1; SELECT 2;")).toBe(2);
    expect(countStatements(";;  ;")).toBe(0);
  });

  it("does not split on ; inside literals or comments", () => {
    expect(countStatements("SELECT 'a;b'")).toBe(1);
    expect(countStatements('SELECT ";" FROM "t;t"')).toBe(1);
    expect(countStatements("SELECT 1 -- ; comment\n; SELECT 2")).toBe(2);
    expect(countStatements("/* a; b */ SELECT 1")).toBe(1);
  });
});

describe("dangerousStatements", () => {
  it("reads clean for read-only SQL", () => {
    expect(dangerousStatements("SELECT * FROM users; EXPLAIN SELECT 1")).toEqual(
      [],
    );
  });

  it("flags writes and DDL", () => {
    const labels = dangerousStatements(
      "INSERT INTO t VALUES (1); DROP TABLE t; TRUNCATE t2",
    ).map((d) => d.label);
    expect(labels).toEqual(["INSERT", "DROP", "TRUNCATE"]);
  });

  it("calls out UPDATE/DELETE without WHERE", () => {
    const labels = dangerousStatements(
      "UPDATE t SET x = 1; DELETE FROM t WHERE id = 2",
    ).map((d) => d.label);
    expect(labels).toEqual(["UPDATE without WHERE", "DELETE"]);
  });

  it("catches data-modifying CTEs", () => {
    const [d] = dangerousStatements(
      "WITH gone AS (DELETE FROM t WHERE old RETURNING *) SELECT count(*) FROM gone",
    );
    expect(d.label).toBe("DELETE");
  });

  it("ignores keywords inside literals and comments", () => {
    expect(dangerousStatements("SELECT 'DROP TABLE users'")).toEqual([]);
    expect(dangerousStatements("SELECT 1 -- DELETE FROM t")).toEqual([]);
  });
});
