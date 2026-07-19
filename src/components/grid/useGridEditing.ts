// Staged-edit state of the grid: the inline cell editor, the pending-INSERT
// editor, the full-value dialog and the shown-value/paste helpers over them.
// Selection stays in useGridSelection; ResultsGrid wires the two together.
import { useEffect, useMemo, useState } from "react";
import type { StatementResult } from "../../lib/types";
import type { CellDialogState } from "./CellDialog";
import type { InsertEdit } from "./InsertRowTr";
import type { CellRef, GridEditing } from "./types";

export interface UseGridEditingArgs {
  result: StatementResult;
  editing?: GridEditing;
  /** Declared column types aligned with result.columns (json/jsonb get pretty view). */
  columnTypes?: (string | undefined)[];
  showToast: (message: string, kind?: "error" | "info" | "success") => void;
  /** Refocus the grid container so its hotkeys stay live after an editor closes. */
  focusGrid: () => void;
}

export type GridEditingState = ReturnType<typeof useGridEditing>;

export function useGridEditing({
  result,
  editing,
  columnTypes,
  showToast,
  focusGrid,
}: UseGridEditingArgs) {
  const [editCell, setEditCell] = useState<
    (CellRef & { initial: string }) | null
  >(null);
  /** Cell being edited in a pending INSERT row. */
  const [editIns, setEditIns] = useState<InsertEdit | null>(null);
  const [dialog, setDialog] = useState<CellDialogState | null>(null);

  useEffect(() => {
    setEditCell(null);
    setEditIns(null);
    setDialog(null);
  }, [result]);

  const deletedRows = useMemo(
    () => new Set(editing?.deletes ?? []),
    [editing?.deletes],
  );

  const stagedOf = (ri: number, ci: number) => {
    const rowEdits = editing?.edits[ri];
    return rowEdits && Object.prototype.hasOwnProperty.call(rowEdits, ci)
      ? { has: true, value: rowEdits[ci] }
      : { has: false, value: null };
  };

  /** Значение так, как оно показано на экране: staged-правка, если есть,
   *  иначе значение из результата. Все copy/export-пути идут через него —
   *  ⌘C по отредактированной ячейке копирует то, что видит пользователь. */
  const shownValue = (ri: number, ci: number): string | null => {
    const staged = stagedOf(ri, ci);
    return staged.has ? staged.value : (result.rows[ri]?.[ci] ?? null);
  };

  const startEdit = (ri: number, ci: number) => {
    if (!editing) return;
    if (editing.disabledReason) {
      showToast(editing.disabledReason, "info");
      return;
    }
    const staged = stagedOf(ri, ci);
    const value = staged.has ? staged.value : result.rows[ri][ci];
    setEditCell({ row: ri, col: ci, initial: value ?? "" });
  };

  const startInsertEdit = (index: number, ci: number) => {
    if (!editing) return;
    setEditIns({
      index,
      col: ci,
      initial: editing.inserts[index]?.values[ci] ?? "",
    });
  };

  /** Full-value editor; json/jsonb (or json-looking) values get prettified. */
  const openCellDialog = (ri: number, ci: number) => {
    const staged = editing ? stagedOf(ri, ci) : { has: false, value: null };
    const raw = staged.has ? staged.value : (result.rows[ri]?.[ci] ?? null);
    const declaredJson = (columnTypes?.[ci] ?? "").toLowerCase().startsWith("json");
    let text = raw ?? "";
    let isJson = declaredJson;
    if (raw !== null) {
      try {
        const parsed: unknown = JSON.parse(raw);
        if (declaredJson || (typeof parsed === "object" && parsed !== null)) {
          text = JSON.stringify(parsed, null, 2);
          isJson = true;
        }
      } catch {
        // not valid JSON — edit as plain text
      }
    }
    setDialog({ row: ri, col: ci, text, isJson });
  };

  const closeDialog = () => {
    setDialog(null);
    focusGrid(); // keep grid hotkeys (⌘S/Esc) live after the dialog
  };

  const stageDialog = () => {
    if (!dialog || !editing || editing.disabledReason) return;
    let value = dialog.text;
    if (dialog.isJson) {
      try {
        value = JSON.stringify(JSON.parse(dialog.text)); // stored compact
      } catch (e) {
        showToast(`Invalid JSON: ${e instanceof Error ? e.message : e}`);
        return;
      }
    }
    editing.onEdit(dialog.row, dialog.col, value);
    closeDialog();
  };

  const dirty = Boolean(
    editing &&
      (Object.keys(editing.edits).length > 0 ||
        editing.deletes.length > 0 ||
        editing.inserts.length > 0),
  );

  const canEdit = Boolean(editing) && !editing?.disabledReason;

  /** Lays TSV out over the grid as staged edits from `start` (the selection's
   *  top-left cell — computed by the caller, which owns the selection). */
  const applyTsv = (text: string, start: CellRef | null) => {
    if (!editing) return;
    if (editing.disabledReason) {
      showToast(editing.disabledReason, "info");
      return;
    }
    if (!start) {
      showToast("Select a cell to paste into", "info");
      return;
    }
    const lines = text.replace(/\r\n?/g, "\n").split("\n");
    if (lines[lines.length - 1] === "") lines.pop();
    let pasted = 0;
    lines.forEach((line, r) => {
      const ri = start.row + r;
      if (ri >= result.rows.length) return;
      line.split("\t").forEach((value, c) => {
        const ci = start.col + c;
        if (ci >= result.columns.length) return;
        editing.onEdit(ri, ci, value);
        pasted += 1;
      });
    });
    if (pasted > 0) showToast(`Pasted ${pasted} cell(s) — staged`, "info");
  };

  return {
    editCell,
    setEditCell,
    editIns,
    setEditIns,
    dialog,
    setDialog,
    deletedRows,
    stagedOf,
    shownValue,
    startEdit,
    startInsertEdit,
    openCellDialog,
    closeDialog,
    stageDialog,
    dirty,
    canEdit,
    applyTsv,
  };
}
