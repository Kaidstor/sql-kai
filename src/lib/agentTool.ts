// Представление tool-коллов агента (ACP) для карточек в панели: претти-имена
// MCP-тулов, однострочная сводка аргументов, нормализация результата
// (таблица / текст / diff) с капом размеров — стор хранит уже готовые к
// рендеру данные, а не сырые payload'ы протокола.
import type { StatementResult } from "./types";

/** Превью не должно раздувать эфемерный чат: текст результата режется. */
const MAX_TEXT = 64 * 1024;
/** Строк на statement в превью — как DEFAULT_MAX_ROWS у MCP-сервера. */
const MAX_ROWS = 200;
/** Кап одной стороны diff'а; больше — рендер всё равно нечитаем. */
const MAX_DIFF = 64 * 1024;

/** `mcp__<server>__<tool>` — внутренний нейминг MCP-тулов Claude Code;
 *  адаптер присылает его как title. Не-MCP заголовки возвращают null. */
export function parseMcpTitle(
  title: string,
): { server: string; tool: string } | null {
  const m = /^mcp__(.+?)__(.+)$/.exec(title);
  return m ? { server: m[1], tool: m[2] } : null;
}

/** Однострочная сводка аргументов вызова (SQL, команда, таблица…) для
 *  свёрнутой карточки — как строка тула в терминальном Claude Code. */
export function toolInputSummary(rawInput: unknown): string | null {
  if (!rawInput || typeof rawInput !== "object") return null;
  const r = rawInput as Record<string, unknown>;
  for (const key of ["sql", "command", "table", "pattern", "path", "url", "query"]) {
    const v = r[key];
    if (typeof v === "string" && v.trim()) {
      const line = v.trim().replace(/\s+/g, " ");
      return line.length > 160 ? `${line.slice(0, 159)}…` : line;
    }
  }
  return null;
}

export interface AgentToolDiff {
  path: string;
  oldText: string | null;
  newText: string;
}

export type AgentToolOutput =
  | {
      kind: "exec";
      results: StatementResult[];
      durationMs?: number;
      maskedColumns?: string[];
      /** Превью укоротило строки сверх того, что срезал сам сервер. */
      capped?: boolean;
    }
  | { kind: "text"; text: string; capped?: boolean };

/** Ответ sql-kai-тулов — JSON ExecResult; всё остальное — не наш формат. */
function parseExecJson(text: string): {
  results: StatementResult[];
  durationMs?: number;
  maskedColumns?: string[];
} | null {
  if (!text.startsWith("{")) return null;
  let v: unknown;
  try {
    v = JSON.parse(text);
  } catch {
    return null;
  }
  if (!v || typeof v !== "object" || !Array.isArray((v as { results?: unknown }).results)) {
    return null;
  }
  const raw = v as {
    results: unknown[];
    durationMs?: unknown;
    maskedColumns?: unknown;
  };
  const results: StatementResult[] = [];
  for (const s of raw.results) {
    if (!s || typeof s !== "object") return null;
    const st = s as Record<string, unknown>;
    if (!Array.isArray(st.columns) || !Array.isArray(st.rows)) return null;
    results.push({
      columns: st.columns.map(String),
      rows: (st.rows as unknown[][]).map((row) =>
        row.map((c) => (c === null || c === undefined ? null : String(c))),
      ),
      rowsAffected:
        typeof st.rowsAffected === "number" ? st.rowsAffected : null,
      truncated: st.truncated === true,
    });
  }
  return {
    results,
    durationMs: typeof raw.durationMs === "number" ? raw.durationMs : undefined,
    maskedColumns: Array.isArray(raw.maskedColumns)
      ? raw.maskedColumns.map(String)
      : undefined,
  };
}

/** Блок content из ACP tool_call/tool_call_update (подмножество). */
export interface AcpToolContent {
  type: string;
  content?: { type: string; text?: string };
  [key: string]: unknown;
}

/** Контент/выход апдейта → данные карточки. ACP шлёт результат блоками
 *  `{type:"content",content:{type:"text",text}}` и `{type:"diff",…}`;
 *  rawOutput — фолбэк (у MCP-тулов это результат `{content:[…]}`). */
export function normalizeToolResult(u: {
  content?: AcpToolContent[];
  rawOutput?: unknown;
}): { output?: AgentToolOutput; diffs?: AgentToolDiff[] } {
  const texts: string[] = [];
  const diffs: AgentToolDiff[] = [];
  for (const block of u.content ?? []) {
    if (block.type === "diff" && typeof block.newText === "string") {
      diffs.push({
        path: typeof block.path === "string" ? block.path : "",
        oldText:
          typeof block.oldText === "string"
            ? block.oldText.slice(0, MAX_DIFF)
            : null,
        newText: block.newText.slice(0, MAX_DIFF),
      });
    } else if (
      block.type === "content" &&
      block.content?.type === "text" &&
      typeof block.content.text === "string"
    ) {
      texts.push(block.content.text);
    }
  }
  if (texts.length === 0 && u.rawOutput !== undefined) {
    texts.push(...rawOutputTexts(u.rawOutput));
  }
  const joined = texts.join("\n").trim();
  const result: { output?: AgentToolOutput; diffs?: AgentToolDiff[] } = {};
  if (diffs.length > 0) result.diffs = diffs;
  if (!joined) return result;

  const exec = parseExecJson(joined);
  if (exec) {
    let capped = false;
    const results = exec.results.map((r) => {
      if (r.rows.length <= MAX_ROWS) return r;
      capped = true;
      return { ...r, rows: r.rows.slice(0, MAX_ROWS) };
    });
    result.output = { kind: "exec", ...exec, results, ...(capped ? { capped } : {}) };
  } else {
    result.output = {
      kind: "text",
      text: joined.slice(0, MAX_TEXT),
      ...(joined.length > MAX_TEXT ? { capped: true } : {}),
    };
  }
  return result;
}

/** rawOutput произвольной формы → текстовые куски. */
function rawOutputTexts(raw: unknown): string[] {
  if (typeof raw === "string") return [raw];
  if (raw && typeof raw === "object") {
    // MCP-результат: { content: [{ type: "text", text }], isError }
    const content = (raw as { content?: unknown }).content;
    if (Array.isArray(content)) {
      const texts = content
        .filter(
          (b): b is { text: string } =>
            !!b &&
            typeof b === "object" &&
            typeof (b as { text?: unknown }).text === "string",
        )
        .map((b) => b.text);
      if (texts.length > 0) return texts;
    }
    try {
      return [JSON.stringify(raw, null, 1)];
    } catch {
      return [];
    }
  }
  return raw === undefined || raw === null ? [] : [String(raw)];
}

export type DiffLine = { type: "same" | "add" | "del"; text: string };

/** Построчный LCS-дифф для рендера diff-блоков. На больших текстах DP-таблица
 *  неподъёмна — грубый фолбэк «всё старое минус, всё новое плюс». */
export function diffLines(oldText: string, newText: string): DiffLine[] {
  const a = oldText.length ? oldText.split("\n") : [];
  const b = newText.length ? newText.split("\n") : [];
  if (a.length * b.length > 250_000) {
    return [
      ...a.map((text): DiffLine => ({ type: "del", text })),
      ...b.map((text): DiffLine => ({ type: "add", text })),
    ];
  }
  // LCS по строкам: lcs[i][j] — длина LCS хвостов a[i:], b[j:]
  const lcs: number[][] = Array.from({ length: a.length + 1 }, () =>
    new Array<number>(b.length + 1).fill(0),
  );
  for (let i = a.length - 1; i >= 0; i--) {
    for (let j = b.length - 1; j >= 0; j--) {
      lcs[i][j] =
        a[i] === b[j]
          ? lcs[i + 1][j + 1] + 1
          : Math.max(lcs[i + 1][j], lcs[i][j + 1]);
    }
  }
  const out: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length && j < b.length) {
    if (a[i] === b[j]) {
      out.push({ type: "same", text: a[i] });
      i++;
      j++;
    } else if (lcs[i + 1][j] >= lcs[i][j + 1]) {
      out.push({ type: "del", text: a[i] });
      i++;
    } else {
      out.push({ type: "add", text: b[j] });
      j++;
    }
  }
  while (i < a.length) out.push({ type: "del", text: a[i++] });
  while (j < b.length) out.push({ type: "add", text: b[j++] });
  return out;
}
