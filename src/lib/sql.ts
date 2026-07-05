/** Quote an SQL identifier: `my col` -> `"my col"`. */
export const quoteIdent = (s: string) => `"${s.replaceAll('"', '""')}"`;

/** Quote an SQL string literal: `it's` -> `'it''s'`. */
export const quoteLit = (s: string) => `'${s.replaceAll("'", "''")}'`;

/** One-line SQL preview for list rows. */
export const sqlPreview = (sql: string, max = 90) =>
  sql.replace(/\s+/g, " ").slice(0, max);
