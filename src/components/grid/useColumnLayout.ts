import {
  useLayoutEffect,
  useRef,
  useState,
  type MouseEvent,
} from "react";
import type { StatementResult } from "../../lib/types";

/**
 * Column layout of the grid: measured widths, drag-resize, content-fit and
 * hidden columns. First data render is auto-layout; actual widths are then
 * measured and the table switches to table-layout:fixed. Fixed widths keep
 * cells from reflowing when an editor mounts and make drag-resize possible.
 * Key -1 is the row-number gutter. Widths survive result swaps with the same
 * column set (page/sort) and re-measure when the columns change.
 */
export function useColumnLayout(
  result: StatementResult,
  onHiddenColsChange?: (hidden: ReadonlySet<number>) => void,
) {
  const tableRef = useRef<HTMLTableElement>(null);
  const [colWidths, setColWidths] = useState<Record<number, number>>({});
  const [hiddenCols, setHiddenCols] = useState<ReadonlySet<number>>(new Set());
  const [sizedFor, setSizedFor] = useState<string | null>(null);
  const colsKey = result.columns.join("\u0000");
  const sized = sizedFor === colsKey;

  const changeHiddenCols = (next: ReadonlySet<number>) => {
    setHiddenCols(next);
    onHiddenColsChange?.(next);
  };

  useLayoutEffect(() => {
    if (sized) return;
    const ths = tableRef.current?.querySelectorAll("thead th");
    if (!ths || ths.length === 0) return;
    const widths: Record<number, number> = {};
    ths.forEach((th, i) => {
      widths[i - 1] = Math.ceil(th.getBoundingClientRect().width);
    });
    setColWidths(widths);
    changeHiddenCols(new Set());
    setSizedFor(colsKey);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sized, colsKey]);

  // table-layout:fixed only kicks in with a non-auto table width, so the
  // table is given the exact sum of its visible columns.
  const totalW = sized
    ? Object.entries(colWidths).reduce((acc, [k, w]) => {
        const idx = Number(k);
        return idx >= 0 && hiddenCols.has(idx) ? acc : acc + w;
      }, 0)
    : undefined;

  const startResize = (ci: number, e: MouseEvent) => {
    e.preventDefault();
    e.stopPropagation();
    const startX = e.clientX;
    const startW = colWidths[ci] ?? 150;
    const move = (ev: globalThis.MouseEvent) => {
      const w = Math.max(40, Math.round(startW + ev.clientX - startX));
      setColWidths((cw) => (cw[ci] === w ? cw : { ...cw, [ci]: w }));
    };
    const up = () => {
      window.removeEventListener("mousemove", move);
      window.removeEventListener("mouseup", up);
    };
    window.addEventListener("mousemove", move);
    window.addEventListener("mouseup", up);
  };

  /** Content-fit width via canvas text metrics (grid font, longest line). */
  const fitCanvas = useRef<HTMLCanvasElement | null>(null);
  const fitWidth = (ci: number): number | null => {
    const t = tableRef.current;
    const ctx = (fitCanvas.current ??= document.createElement("canvas")).getContext("2d");
    if (!t || !ctx) return null;
    const cs = getComputedStyle(t);
    ctx.font = `${cs.fontSize} ${cs.fontFamily}`;
    // header label + the sort button that sits next to it
    let w = ctx.measureText(result.columns[ci] ?? "").width + 22;
    for (const row of result.rows) {
      const v = row[ci];
      if (!v) continue;
      for (const line of v.split("\n")) {
        const lw = ctx.measureText(line).width;
        if (lw > w) w = lw;
      }
    }
    // px-2 cell padding + border; capped like the old max-w-105
    return Math.min(Math.max(Math.ceil(w) + 17, 40), 420);
  };
  const fitColumn = (ci: number) => {
    const w = fitWidth(ci);
    if (w !== null) setColWidths((cw) => ({ ...cw, [ci]: w }));
  };
  const fitAllColumns = () => {
    const next: Record<number, number> = { ...colWidths };
    result.columns.forEach((_, ci) => {
      if (hiddenCols.has(ci)) return;
      const w = fitWidth(ci);
      if (w !== null) next[ci] = w;
    });
    setColWidths(next);
  };
  /** Gives every visible column the width of column `ci`. */
  const matchAllColumns = (ci: number) => {
    const w = colWidths[ci];
    if (!w) return;
    const next: Record<number, number> = { ...colWidths };
    result.columns.forEach((_, i) => {
      if (!hiddenCols.has(i)) next[i] = w;
    });
    setColWidths(next);
  };
  const resetLayout = () => {
    // dropping the size key re-renders auto-layout, which re-measures
    setSizedFor(null);
    setColWidths({});
    changeHiddenCols(new Set());
  };

  return {
    tableRef,
    colWidths,
    hiddenCols,
    sized,
    totalW,
    changeHiddenCols,
    startResize,
    fitColumn,
    fitAllColumns,
    matchAllColumns,
    resetLayout,
  };
}
