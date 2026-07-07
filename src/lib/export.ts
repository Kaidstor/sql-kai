// Result-set serializers for export/copy. All take the visible columns and
// the rows to include (already filtered to the selection when one exists).

type Row = readonly (string | null)[];

/** RFC 4180: quote fields containing the separator, quotes or newlines.
 *  NULL becomes an empty field. */
export function toCsv(columns: readonly string[], rows: readonly Row[]): string {
  const field = (v: string | null) => {
    if (v === null) return "";
    return /[",\n\r]/.test(v) ? `"${v.replaceAll('"', '""')}"` : v;
  };
  return [columns, ...rows]
    .map((row) => row.map((v) => field(v as string | null)).join(","))
    .join("\r\n");
}

/** Array of {column: value} objects, pretty-printed. */
export function toJson(columns: readonly string[], rows: readonly Row[]): string {
  const objs = rows.map((row) =>
    Object.fromEntries(columns.map((c, i) => [c, row[i] ?? null])),
  );
  return JSON.stringify(objs, null, 2);
}

/** GitHub-flavored Markdown table; pipes and newlines are escaped. */
export function toMarkdown(
  columns: readonly string[],
  rows: readonly Row[],
): string {
  const cell = (v: string | null) =>
    v === null ? "" : v.replaceAll("|", "\\|").replaceAll(/\r?\n/g, "<br>");
  const line = (cells: readonly string[]) => `| ${cells.join(" | ")} |`;
  return [
    line(columns.map((c) => cell(c))),
    line(columns.map(() => "---")),
    ...rows.map((row) => line(row.map((v) => cell(v)))),
  ].join("\n");
}
