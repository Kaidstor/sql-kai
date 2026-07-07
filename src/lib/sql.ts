/** Quote an SQL identifier: `my col` -> `"my col"`. */
export const quoteIdent = (s: string) => `"${s.replaceAll('"', '""')}"`;

/** Quote an SQL string literal: `it's` -> `'it''s'`. */
export const quoteLit = (s: string) => `'${s.replaceAll("'", "''")}'`;

/** One-line SQL preview for list rows. */
export const sqlPreview = (sql: string, max = 90) =>
  sql.replace(/\s+/g, " ").slice(0, max);

// --- Production write-guard ---------------------------------------------------

export interface DangerousStatement {
  /** Short classification, e.g. "UPDATE without WHERE". */
  label: string;
  /** One-line preview of the statement (literals blanked). */
  preview: string;
}

/** Blanks string literals / quoted identifiers and strips comments, so `;`
 *  and keywords can be matched naively afterwards. */
const stripSqlNoise = (sql: string) =>
  sql
    .replace(/'(?:[^']|'')*'/g, "''")
    .replace(/"(?:[^"]|"")*"/g, '""')
    .replace(/--[^\n]*/g, "")
    .replace(/\/\*[\s\S]*?\*\//g, "");

const WRITE_KEYWORDS = /^(INSERT|UPDATE|DELETE|MERGE)\b/;
const DDL_KEYWORDS = /^(DROP|TRUNCATE|ALTER|GRANT|REVOKE)\b/;

/** Statements that modify data or schema — the production guard asks before
 *  running these. Heuristic (regex, not a parser): errs on the side of
 *  flagging, e.g. any writing CTE counts, WHERE anywhere in the statement
 *  counts as "has WHERE". */
export function dangerousStatements(sql: string): DangerousStatement[] {
  const out: DangerousStatement[] = [];
  for (const raw of stripSqlNoise(sql).split(";")) {
    const stmt = raw.trim();
    if (!stmt) continue;
    const upper = stmt.toUpperCase();
    let keyword: string | null = null;
    const direct = WRITE_KEYWORDS.exec(upper) ?? DDL_KEYWORDS.exec(upper);
    if (direct) {
      keyword = direct[1];
    } else if (/^WITH\b/.test(upper)) {
      // data-modifying CTE: WITH … AS (…) INSERT/UPDATE/DELETE …
      keyword = /\b(INSERT|UPDATE|DELETE|MERGE)\b/.exec(upper)?.[1] ?? null;
    }
    if (!keyword) continue;
    const noWhere =
      (keyword === "UPDATE" || keyword === "DELETE") && !/\bWHERE\b/.test(upper);
    out.push({
      label: noWhere ? `${keyword} without WHERE` : keyword,
      preview: sqlPreview(stmt, 70),
    });
  }
  return out;
}
