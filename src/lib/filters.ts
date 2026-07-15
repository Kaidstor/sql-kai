// Structured filter bar model: conditions ↔ the raw WHERE string that
// get_table_page consumes. Compile is the source of truth; parse exists so
// FK-navigation filters and persisted raw filters round-trip into the builder.
import { quoteIdent, quoteLit } from "./sql";
import type { EnumTypeInfo } from "./types";

export type FilterOp =
  | "eq"
  | "neq"
  | "gt"
  | "gte"
  | "lt"
  | "lte"
  | "like"
  | "ilike"
  | "nlike"
  | "in"
  | "null"
  | "notnull";

export interface FilterOpDef {
  op: FilterOp;
  /** Dropdown label, e.g. "not equals". */
  label: string;
  /** SQL chip shown next to the label, e.g. "<>". */
  sql: string;
  /** false = the value input is disabled (IS NULL / IS NOT NULL). */
  needsValue: boolean;
}

export const FILTER_OPS: FilterOpDef[] = [
  { op: "eq", label: "equals", sql: "=", needsValue: true },
  { op: "neq", label: "not equals", sql: "<>", needsValue: true },
  { op: "gt", label: "greater", sql: ">", needsValue: true },
  { op: "gte", label: "greater or equals", sql: ">=", needsValue: true },
  { op: "lt", label: "less", sql: "<", needsValue: true },
  { op: "lte", label: "less or equals", sql: "<=", needsValue: true },
  { op: "like", label: "like", sql: "LIKE", needsValue: true },
  { op: "ilike", label: "ilike", sql: "ILIKE", needsValue: true },
  { op: "nlike", label: "not like", sql: "NOT LIKE", needsValue: true },
  { op: "in", label: "in", sql: "IN", needsValue: true },
  { op: "null", label: "is null", sql: "IS NULL", needsValue: false },
  { op: "notnull", label: "is not null", sql: "IS NOT NULL", needsValue: false },
];

const OP_BY_KEY = new Map(FILTER_OPS.map((d) => [d.op, d]));

export const opDef = (op: FilterOp): FilterOpDef => OP_BY_KEY.get(op)!;

export interface FilterCond {
  column: string;
  op: FilterOp;
  /** null = ещё не введено (условие неактивно); "" = пустая строка (активно,
   *  compiles to `col = ''`). Для `in` — список через запятую. */
  value: string | null;
}

/** Inactive rows (value-requiring op without a value yet) stay out of WHERE. */
export const condActive = (c: FilterCond): boolean =>
  !opDef(c.op).needsValue || c.value !== null;

/** One condition → SQL predicate; null while the row is inactive. Values are
 *  always string literals — Postgres casts them to the column type. */
export function compileCond(c: FilterCond): string | null {
  if (!c.column) return null;
  const col = quoteIdent(c.column);
  const def = opDef(c.op);
  if (!def.needsValue) return `${col} ${def.sql}`;
  if (c.value === null) return null;
  if (c.op === "in") {
    const items = c.value
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    if (items.length === 0) return null;
    return `${col} IN (${items.map(quoteLit).join(", ")})`;
  }
  return `${col} ${def.sql} ${quoteLit(c.value)}`;
}

/** All active conditions AND-joined ("" = no filter). */
export const compileFilters = (conds: FilterCond[]): string =>
  conds.map(compileCond).filter(Boolean).join(" AND ");

// --- Parsing (round-trip of compiled / FK-navigation filters) ------------------

// Longest-first so ">=" wins over ">" and "IS NOT NULL" over "IS NULL".
const SQL_OPS: [string, FilterOp][] = [
  ["IS NOT NULL", "notnull"],
  ["IS NULL", "null"],
  ["NOT LIKE", "nlike"],
  ["ILIKE", "ilike"],
  ["LIKE", "like"],
  ["IN", "in"],
  ["<>", "neq"],
  ["!=", "neq"],
  [">=", "gte"],
  ["<=", "lte"],
  ["=", "eq"],
  [">", "gt"],
  ["<", "lt"],
];

const IDENT_RE = /^\s*(?:"((?:[^"]|"")+)"|([a-zA-Z_][a-zA-Z0-9_$]*))/;
const LIT_RE = /^\s*(?:'((?:[^']|'')*)'|(-?\d+(?:\.\d+)?)|(true|false)\b)/i;

/** Simple AND-chain of `col <op> literal` / `col IS [NOT] NULL` / `col IN
 *  (...)` → conditions; anything fancier returns null (builder falls back to
 *  raw SQL mode). */
export function parseFilter(where: string): FilterCond[] | null {
  let rest = where.trim();
  if (!rest) return [];
  const out: FilterCond[] = [];

  const literal = (): string | null => {
    const m = LIT_RE.exec(rest);
    if (!m) return null;
    rest = rest.slice(m[0].length);
    if (m[1] !== undefined) return m[1].replaceAll("''", "'");
    return m[2] ?? m[3];
  };

  for (;;) {
    const im = IDENT_RE.exec(rest);
    if (!im) return null;
    rest = rest.slice(im[0].length);
    const column =
      im[1] !== undefined ? im[1].replaceAll('""', '"') : im[2];

    rest = rest.replace(/^\s+/, "");
    // Word ops need \b so IN doesn't match a column named INDEX; symbol ops
    // need the lookahead so "=" doesn't shadow ">=" leftovers.
    const hit = SQL_OPS.find(([sql]) =>
      new RegExp(
        `^${sql.replace(/[<>=!]/g, "\\$&")}${/[a-z]$/i.test(sql) ? "\\b" : "(?![<>=])"}`,
        "i",
      ).test(rest),
    );
    if (!hit) return null;
    const [sql, op] = hit;
    rest = rest.slice(sql.length);

    if (op === "null" || op === "notnull") {
      out.push({ column, op, value: null });
    } else if (op === "in") {
      if (!/^\s*\(/.test(rest)) return null;
      rest = rest.replace(/^\s*\(/, "");
      const items: string[] = [];
      for (;;) {
        const v = literal();
        if (v === null) return null;
        items.push(v);
        if (/^\s*,/.test(rest)) {
          rest = rest.replace(/^\s*,/, "");
          continue;
        }
        break;
      }
      if (!/^\s*\)/.test(rest)) return null;
      rest = rest.replace(/^\s*\)/, "");
      out.push({ column, op, value: items.join(", ") });
    } else {
      const v = literal();
      if (v === null) return null;
      out.push({ column, op, value: v });
    }

    if (/^\s*$/.test(rest)) return out;
    const and = /^\s+AND\s+/i.exec(rest);
    if (!and) return null;
    rest = rest.slice(and[0].length);
  }
}

// --- Column-type helpers for value suggestions ---------------------------------

/** Labels of the column's enum type (null = not an enum). `dataType` is
 *  format_type() output: `status`, `public.status`, `"My Type"`, `status[]`. */
export function enumLabelsFor(
  dataType: string,
  enums: EnumTypeInfo[] | undefined,
): string[] | null {
  if (!enums || enums.length === 0) return null;
  let t = dataType.trim();
  if (t.endsWith("[]")) t = t.slice(0, -2);
  const unquote = (s: string) =>
    s.startsWith('"') ? s.slice(1, -1).replaceAll('""', '"') : s;
  const dot = t.startsWith('"') ? t.indexOf('.', t.indexOf('"', 1) + 1) : t.indexOf(".");
  if (dot > 0) {
    const schema = unquote(t.slice(0, dot));
    const name = unquote(t.slice(dot + 1));
    return enums.find((e) => e.schema === schema && e.name === name)?.labels ?? null;
  }
  const name = unquote(t);
  return enums.find((e) => e.name === name)?.labels ?? null;
}

/** Text-ish column — the EMPTY STRING suggestion makes sense. */
export const isTextType = (dataType: string): boolean =>
  /^(text|citext|character varying|character|varchar|bpchar|char)\b/.test(
    dataType.trim(),
  );
