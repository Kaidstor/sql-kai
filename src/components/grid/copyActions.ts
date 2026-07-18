// Copy/export actions of the grid. A plain factory over the current render's
// selection — no state of its own, so it stays trivially in sync with the
// component. Every path goes through `shownValue`: ⌘C on an edited cell
// copies what the user sees (the staged value), not the stale DB value.
import { api, errText } from "../../lib/api";
import { copyText } from "../../lib/clipboard";
import { exportedMessage, toCsv, toJson } from "../../lib/export";
import { promptExportPath } from "../../lib/exportFile";
import { quoteIdent, quoteLit, relIdent } from "../../lib/sql";
import type { StatementResult } from "../../lib/types";

export interface CopyDeps {
  result: StatementResult;
  /** Значение так, как оно показано на экране: staged-правка, если есть,
   *  иначе значение из результата. */
  shownValue: (ri: number, ci: number) => string | null;
  /** Selected row indices, sorted and bounded to the result. */
  selRows: number[];
  /** Selected column indices, sorted and bounded to the result. */
  selColList: number[];
  rect: { r1: number; r2: number; c1: number; c2: number } | null;
  rectCount: number;
  hiddenCols: ReadonlySet<number>;
  /** Source relation of the rows; enables "Copy as INSERT". */
  insertTarget?: { schema: string; table: string };
  showToast: (message: string, kind?: "error" | "info" | "success") => void;
}

export type CopyActions = ReturnType<typeof makeCopyActions>;

export function makeCopyActions({
  result,
  shownValue,
  selRows,
  selColList,
  rect,
  rectCount,
  hiddenCols,
  insertTarget,
  showToast,
}: CopyDeps) {
  const n = selRows.length;

  const toastCopied = (what: string) => showToast(what, "info");

  /** Copies and reports success in the status bar. */
  const copyAndToast = (text: string, message: string) =>
    void copyText(text).then((ok) => ok && toastCopied(message));

  const copyRows = (sep: string, suffix: string) => {
    if (!n) return;
    const text = selRows
      .map((i) =>
        result.columns.map((_, ci) => shownValue(i, ci) ?? "").join(sep),
      )
      .join("\n");
    copyAndToast(text, `Copied ${n} row(s) ${suffix}`);
  };

  const copyJson = () => {
    if (!n) return;
    const objs = selRows.map((i) =>
      Object.fromEntries(result.columns.map((c, ci) => [c, shownValue(i, ci)])),
    );
    copyAndToast(
      JSON.stringify(n === 1 ? objs[0] : objs, null, 2),
      `Copied ${n} row(s) as JSON`,
    );
  };

  /** Multi-row INSERT ready to run elsewhere (e.g. paste into prod). String
   *  literals for every value — Postgres coerces them to the column types. */
  const copyInsert = () => {
    if (!insertTarget || !n) return;
    const rel = relIdent(insertTarget.schema, insertTarget.table);
    const cols = result.columns.map(quoteIdent).join(", ");
    const tuples = selRows.map(
      (i) =>
        `  (${result.columns
          .map((_, ci) => {
            const v = shownValue(i, ci);
            return v === null ? "NULL" : quoteLit(v);
          })
          .join(", ")})`,
    );
    copyAndToast(
      `INSERT INTO ${rel} (${cols}) VALUES\n${tuples.join(",\n")};`,
      `Copied ${n} row(s) as INSERT`,
    );
  };

  const copyCellAt = (ri: number, ci: number) =>
    copyAndToast(shownValue(ri, ci) ?? "", "Cell copied");

  /** TSV of the selected columns across all rows (single column = lines). */
  const copyColumns = () => {
    if (!selColList.length) return;
    const lines = result.rows.map((_, ri) =>
      selColList.map((c) => shownValue(ri, c) ?? "").join("\t"),
    );
    copyAndToast(
      lines.join("\n"),
      `Copied ${selColList.length} column(s) as TSV`,
    );
  };

  const copyColumnValues = (ci: number) => {
    const text = result.rows.map((_, ri) => shownValue(ri, ci) ?? "").join("\n");
    copyAndToast(text, `Copied ${result.rows.length} value(s)`);
  };

  /** Distinct non-NULL values as a quoted SQL IN list. */
  const copyColumnIn = (ci: number) => {
    const seen = new Set<string>();
    for (let ri = 0; ri < result.rows.length; ri++) {
      const v = shownValue(ri, ci);
      if (v !== null) seen.add(v);
    }
    const list = [...seen].map(quoteLit).join(", ");
    copyAndToast(`(${list})`, `Copied ${seen.size} value(s) for IN`);
  };

  /** TSV of the rectangular cell selection. Raw tab-join (no header) — the
   *  plain "paste into a spreadsheet" shape ⌘C is expected to produce. */
  const copyCells = () => {
    if (!rect) return;
    const lines: string[] = [];
    for (let r = rect.r1; r <= rect.r2; r++) {
      if (!result.rows[r]) continue;
      const cells: string[] = [];
      for (let c = rect.c1; c <= rect.c2; c++) {
        cells.push(shownValue(r, c) ?? "");
      }
      lines.push(cells.join("\t"));
    }
    copyAndToast(lines.join("\n"), `Copied ${rectCount} cell(s) as TSV`);
  };

  /** The rectangular cell selection serialized in an export format, keyed by
   *  the selected columns' names — the Export menu's formats scoped to the
   *  picked cells instead of whole rows. Includes a header row (CSV) or object
   *  keys (JSON), unlike the bare `copyCells`. */
  const copyCellsAs = (kind: "csv" | "json") => {
    if (!rect) return;
    const cols: string[] = [];
    for (let c = rect.c1; c <= rect.c2; c++) cols.push(result.columns[c]);
    const rows = [];
    for (let r = rect.r1; r <= rect.r2; r++) {
      if (!result.rows[r]) continue;
      rows.push(cols.map((_, i) => shownValue(r, rect.c1 + i)));
    }
    const text = kind === "csv" ? toCsv(cols, rows) : toJson(cols, rows);
    copyAndToast(
      text,
      `Copied ${rectCount} cell(s) as ${kind === "csv" ? "CSV" : "JSON"}`,
    );
  };

  const copyAll = () => {
    const text = [
      result.columns.join("\t"),
      ...result.rows.map((r, ri) =>
        r.map((_, ci) => shownValue(ri, ci) ?? "").join("\t"),
      ),
    ].join("\n");
    copyAndToast(text, `Copied ${result.rows.length} row(s) with header`);
  };

  // --- Export (CSV / JSON / Excel) ------------------------------------------
  // Follows the on-screen selection so the export matches what the right-click
  // targeted: a cell rectangle → those cells; selected columns → those columns
  // (all rows); selected rows → those rows; nothing → the whole shown result
  // (hidden columns left out, mirroring the grid).
  const selectedData = () => {
    if (rect && rectCount > 1) {
      const cols: number[] = [];
      for (let c = rect.c1; c <= rect.c2; c++) cols.push(c);
      const rowIdxs: number[] = [];
      for (let r = rect.r1; r <= rect.r2; r++) if (result.rows[r]) rowIdxs.push(r);
      return {
        columns: cols.map((c) => result.columns[c]),
        rows: rowIdxs.map((ri) => cols.map((c) => shownValue(ri, c))),
      };
    }
    if (selColList.length > 0) {
      return {
        columns: selColList.map((c) => result.columns[c]),
        rows: result.rows.map((_, ri) =>
          selColList.map((c) => shownValue(ri, c)),
        ),
      };
    }
    const rowIdxs = n > 0 ? selRows : result.rows.map((_, i) => i);
    const visCols = result.columns
      .map((_, i) => i)
      .filter((i) => !hiddenCols.has(i));
    return {
      columns: visCols.map((i) => result.columns[i]),
      rows: rowIdxs.map((ri) => visCols.map((ci) => shownValue(ri, ci))),
    };
  };

  const exportRows = async (kind: "csv" | "json" | "xlsx") => {
    const { columns, rows } = selectedData();
    try {
      const path = await promptExportPath(insertTarget?.table ?? "result", kind);
      if (!path) return;
      if (kind === "xlsx") {
        await api.saveRowsXlsx(path, columns, rows);
      } else {
        await api.saveTextFile(
          path,
          kind === "csv" ? toCsv(columns, rows) : toJson(columns, rows),
        );
      }
      toastCopied(exportedMessage(rows.length, path));
    } catch (e) {
      showToast(errText(e));
    }
  };

  return {
    toastCopied,
    copyAndToast,
    copyRows,
    copyJson,
    copyInsert,
    copyCellAt,
    copyColumns,
    copyColumnValues,
    copyColumnIn,
    copyCells,
    copyCellsAs,
    copyAll,
    exportRows,
  };
}
