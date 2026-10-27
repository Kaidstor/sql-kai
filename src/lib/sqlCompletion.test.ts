import { CompletionContext } from "@codemirror/autocomplete";
import { EditorState } from "@codemirror/state";
import { PostgreSQL, sql } from "@codemirror/lang-sql";
import { describe, expect, it } from "vitest";
import { nestedSchemaCompletionSource } from "./sqlCompletion";

const config = { dialect: PostgreSQL, schema: { public: { domains: ["id", "name"] } }, defaultSchema: "public" };

describe("SQL column completion", () => {
  it.each([
    "SELECT count(*) FROM domains AS d WHERE d.|",
    "SELECT 1 FROM (SELECT count(*) FROM domains AS d WHERE d.|)",
    "SELECT 1 FROM (SELECT count(*) FROM public.domains d WHERE d.na|)",
    "SELECT 1 FROM (SELECT 1 FROM (SELECT 1 FROM domains d WHERE d.|))",
    "SELECT 1 FROM (SELECT 1 FROM domains d WHERE d.|",
  ])("resolves the table alias in %s", async (query) => {
    const pos = query.indexOf("|");
    const state = EditorState.create({ doc: query.replace("|", ""), extensions: [sql(config)] });
    const result = await nestedSchemaCompletionSource(config)(new CompletionContext(state, pos, true));
    expect(result?.options.map((option) => option.label)).toContain("id");
    expect(result?.from).toBe(query.lastIndexOf("d.") + 2);
  });

  it("uses the nested alias instead of an identically named outer alias", async () => {
    const nestedConfig = { ...config, schema: { public: { domains: ["id"], users: ["email"] } } };
    const doc = "SELECT 1 FROM users d WHERE EXISTS (SELECT 1 FROM domains d WHERE d.)";
    const pos = doc.length - 1;
    const source = nestedSchemaCompletionSource(nestedConfig);
    const state = EditorState.create({
      doc,
      extensions: [sql({ dialect: PostgreSQL }), PostgreSQL.language.data.of({ autocomplete: source })],
    });
    const sources = state.languageDataAt("autocomplete", pos);
    expect(sources).toContain(source);
    const result = await source(new CompletionContext(state, pos, true));
    expect(result?.options.map((option) => option.label)).toEqual(["id"]);
  });
});
