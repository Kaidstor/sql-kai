import { useEffect, useRef, useState, type MouseEvent } from "react";
import type { StatementResult } from "../../lib/types";
import type { CellRange, CellRef } from "./types";

/**
 * The grid's selection model: whole rows (number gutter), whole columns
 * (header click), a focused cell and a rectangular cell range — the three
 * kinds are mutually exclusive. Also owns the mouse-drag state: cell drags
 * anchor on mousedown and extend over hovered cells; gutter drags extend the
 * row range. Everything resets when the result object is swapped.
 */
export function useGridSelection(
  result: StatementResult,
  opts: { hiddenCols: ReadonlySet<number>; focusGrid: () => void },
) {
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [anchor, setAnchor] = useState<number | null>(null);
  /** Whole-column selection (header click; shift/⌘ extend), exclusive with
   *  row and cell selection. */
  const [selCols, setSelCols] = useState<ReadonlySet<number>>(new Set());
  const [colAnchor, setColAnchor] = useState<number | null>(null);
  const [focused, setFocused] = useState<CellRef | null>(null);
  /** Rectangular cell selection (shift+click extends from the anchor). */
  const [cellSel, setCellSel] = useState<CellRange | null>(null);

  // Mouse-drag cell selection: anchor on mousedown, extend while the button
  // is held over other cells; mouseup anywhere (incl. outside) ends it.
  const dragSel = useRef(false);
  /** Row-drag started on the number gutter: anchor row index, null when idle. */
  const rowDrag = useRef<number | null>(null);
  useEffect(() => {
    const up = () => {
      dragSel.current = false;
      rowDrag.current = null;
    };
    window.addEventListener("mouseup", up);
    return () => window.removeEventListener("mouseup", up);
  }, []);

  useEffect(() => {
    setSelected(new Set());
    setAnchor(null);
    setSelCols(new Set());
    setColAnchor(null);
    setFocused(null);
    setCellSel(null);
  }, [result]);

  const rect = cellSel
    ? {
        r1: Math.min(cellSel.a.row, cellSel.b.row),
        r2: Math.max(cellSel.a.row, cellSel.b.row),
        c1: Math.min(cellSel.a.col, cellSel.b.col),
        c2: Math.max(cellSel.a.col, cellSel.b.col),
      }
    : null;
  const rectCount = rect
    ? (rect.r2 - rect.r1 + 1) * (rect.c2 - rect.c1 + 1)
    : 0;
  const inRect = (r: number, c: number) =>
    rect !== null && r >= rect.r1 && r <= rect.r2 && c >= rect.c1 && c <= rect.c2;

  /** Focuses a single cell, replacing any row/column selection. */
  const focusCell = (ri: number, ci: number) => {
    setSelected(new Set());
    setAnchor(null);
    setSelCols(new Set());
    setColAnchor(null);
    setFocused({ row: ri, col: ci });
    setCellSel({ a: { row: ri, col: ci }, b: { row: ri, col: ci } });
  };

  const selectRow = (ri: number, e: MouseEvent) => {
    setSelCols(new Set());
    setColAnchor(null);
    if (e.shiftKey && anchor !== null) {
      const [from, to] = anchor < ri ? [anchor, ri] : [ri, anchor];
      const range = new Set<number>();
      for (let i = from; i <= to; i++) range.add(i);
      setSelected(range);
    } else if (e.metaKey || e.ctrlKey) {
      const next = new Set(selected);
      if (next.has(ri)) next.delete(ri);
      else next.add(ri);
      setSelected(next);
      setAnchor(ri);
    } else {
      setSelected(new Set([ri]));
      setAnchor(ri);
    }
  };

  /** Extends the gutter row-drag to the hovered row (from the drag anchor). */
  const extendRowDrag = (ri: number) => {
    const from = rowDrag.current;
    if (from === null) return;
    const [lo, hi] = from < ri ? [from, ri] : [ri, from];
    const range = new Set<number>();
    for (let i = lo; i <= hi; i++) range.add(i);
    setSelected(range);
  };

  /** Header click: selects the whole column; shift extends a contiguous
   *  range from the anchor, ⌘/ctrl toggles individual columns. */
  const clickColumn = (ci: number, e: MouseEvent) => {
    setSelected(new Set());
    setAnchor(null);
    setFocused(null);
    setCellSel(null);
    if (e.shiftKey && colAnchor !== null) {
      const [lo, hi] = colAnchor < ci ? [colAnchor, ci] : [ci, colAnchor];
      const range = new Set<number>();
      for (let i = lo; i <= hi; i++) if (!opts.hiddenCols.has(i)) range.add(i);
      setSelCols(range);
    } else if (e.metaKey || e.ctrlKey) {
      const next = new Set(selCols);
      if (next.has(ci)) next.delete(ci);
      else next.add(ci);
      setSelCols(next);
      setColAnchor(ci);
    } else {
      setSelCols(new Set([ci]));
      setColAnchor(ci);
    }
    opts.focusGrid();
  };

  /** Selected row indices in display order (bounded to the current result). */
  const selRows = [...selected]
    .sort((a, b) => a - b)
    .filter((i) => i < result.rows.length);

  /** Selected columns in display order (bounded to the current result). */
  const selColList = [...selCols]
    .filter((c) => c >= 0 && c < result.columns.length)
    .sort((a, b) => a - b);

  return {
    selected,
    setSelected,
    anchor,
    setAnchor,
    selCols,
    setSelCols,
    colAnchor,
    setColAnchor,
    focused,
    setFocused,
    cellSel,
    setCellSel,
    dragSel,
    rowDrag,
    rect,
    rectCount,
    inRect,
    focusCell,
    selectRow,
    extendRowDrag,
    clickColumn,
    selRows,
    selColList,
  };
}
