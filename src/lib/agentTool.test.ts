import { describe, expect, it } from "vitest";
import {
  diffLines,
  normalizeToolResult,
  parseMcpTitle,
  toolInputSummary,
} from "./agentTool";

describe("parseMcpTitle", () => {
  it("разбирает mcp__<server>__<tool>", () => {
    expect(parseMcpTitle("mcp__sql-kai__query")).toEqual({
      server: "sql-kai",
      tool: "query",
    });
    expect(parseMcpTitle("mcp__sql-kai__open_table")).toEqual({
      server: "sql-kai",
      tool: "open_table",
    });
  });

  it("не трогает обычные заголовки", () => {
    expect(parseMcpTitle("Bash")).toBeNull();
    expect(parseMcpTitle("ls -la")).toBeNull();
    expect(parseMcpTitle("mcp__broken")).toBeNull();
  });
});

describe("toolInputSummary", () => {
  it("SQL в одну строку с капом длины", () => {
    expect(
      toolInputSummary({ sql: "SELECT *\n  FROM users\n  WHERE id = 1" }),
    ).toBe("SELECT * FROM users WHERE id = 1");
    const long = toolInputSummary({ sql: `SELECT '${"x".repeat(200)}'` });
    expect(long).toHaveLength(160);
    expect(long?.endsWith("…")).toBe(true);
  });

  it("command / table как фолбэки, мусор — null", () => {
    expect(toolInputSummary({ command: "ls -la" })).toBe("ls -la");
    expect(toolInputSummary({ table: "public.users" })).toBe("public.users");
    expect(toolInputSummary({ counts: true })).toBeNull();
    expect(toolInputSummary(undefined)).toBeNull();
    expect(toolInputSummary("just a string")).toBeNull();
  });
});

describe("normalizeToolResult", () => {
  const execJson = JSON.stringify({
    results: [
      {
        columns: ["id", "name"],
        rows: [
          ["1", "alice"],
          ["2", null],
        ],
        rowsAffected: null,
        truncated: false,
      },
    ],
    durationMs: 12,
    maskedColumns: ["password"],
  });

  it("JSON ExecResult из content-блока → таблица", () => {
    const { output } = normalizeToolResult({
      content: [{ type: "content", content: { type: "text", text: execJson } }],
    });
    expect(output?.kind).toBe("exec");
    if (output?.kind !== "exec") return;
    expect(output.results[0].columns).toEqual(["id", "name"]);
    expect(output.results[0].rows[1][1]).toBeNull();
    expect(output.durationMs).toBe(12);
    expect(output.maskedColumns).toEqual(["password"]);
  });

  it("rawOutput в форме MCP-результата — фолбэк, когда content пуст", () => {
    const { output } = normalizeToolResult({
      rawOutput: { content: [{ type: "text", text: execJson }], isError: false },
    });
    expect(output?.kind).toBe("exec");
  });

  it("контент из content-блоков важнее rawOutput", () => {
    const { output } = normalizeToolResult({
      content: [
        { type: "content", content: { type: "text", text: "plain answer" } },
      ],
      rawOutput: { content: [{ type: "text", text: execJson }] },
    });
    expect(output).toEqual({ kind: "text", text: "plain answer" });
  });

  it("не-JSON текст остаётся текстом, длинный — режется", () => {
    const { output } = normalizeToolResult({ rawOutput: "CREATE TABLE t ()" });
    expect(output).toEqual({ kind: "text", text: "CREATE TABLE t ()" });
    const big = normalizeToolResult({ rawOutput: "x".repeat(100_000) }).output;
    expect(big?.kind).toBe("text");
    if (big?.kind !== "text") return;
    expect(big.text).toHaveLength(64 * 1024);
    expect(big.capped).toBe(true);
  });

  it("строки сверх лимита превью режутся с флагом capped", () => {
    const rows = Array.from({ length: 500 }, (_, i) => [String(i)]);
    const json = JSON.stringify({
      results: [{ columns: ["n"], rows, rowsAffected: null, truncated: true }],
      durationMs: 1,
    });
    const { output } = normalizeToolResult({ rawOutput: json });
    expect(output?.kind).toBe("exec");
    if (output?.kind !== "exec") return;
    expect(output.results[0].rows).toHaveLength(200);
    expect(output.capped).toBe(true);
  });

  it("diff-блоки собираются отдельно от текста", () => {
    const { output, diffs } = normalizeToolResult({
      content: [
        { type: "diff", path: "/a.md", oldText: "old", newText: "new" },
        { type: "content", content: { type: "text", text: "done" } },
      ],
    });
    expect(diffs).toEqual([{ path: "/a.md", oldText: "old", newText: "new" }]);
    expect(output).toEqual({ kind: "text", text: "done" });
  });

  it("пустой апдейт — ничего", () => {
    expect(normalizeToolResult({})).toEqual({});
    expect(normalizeToolResult({ content: [] })).toEqual({});
  });
});

describe("diffLines", () => {
  it("общие строки сохраняются, правки помечаются", () => {
    expect(diffLines("a\nb\nc", "a\nx\nc")).toEqual([
      { type: "same", text: "a" },
      { type: "del", text: "b" },
      { type: "add", text: "x" },
      { type: "same", text: "c" },
    ]);
  });

  it("чистое добавление и чистое удаление", () => {
    expect(diffLines("", "a\nb")).toEqual([
      { type: "add", text: "a" },
      { type: "add", text: "b" },
    ]);
    expect(diffLines("a", "")).toEqual([{ type: "del", text: "a" }]);
  });

  it("идентичные тексты — без add/del", () => {
    expect(diffLines("a\nb", "a\nb").every((l) => l.type === "same")).toBe(true);
  });
});
