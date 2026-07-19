// The grid's scrolling body: data rows, cell selection/editing wiring and the
// pending-INSERT rows interleaved under their duplicate sources. The parent
// owns the scroller ref and scroll handler (header sync + position memory).
import { Fragment, type CSSProperties, type ReactNode, type RefObject, type UIEvent } from "react";
import type { StatementResult } from "../../lib/types";
import { cn } from "../ui";
import { CellInput } from "./CellInput";
import { InsertRowTr } from "./InsertRowTr";
import type { GridEditing } from "./types";
import type { useColumnLayout } from "./useColumnLayout";
import type { GridEditingState } from "./useGridEditing";
import type { useGridSelection } from "./useGridSelection";

interface Props {
  result: StatementResult;
  layout: ReturnType<typeof useColumnLayout>;
  sel: ReturnType<typeof useGridSelection>;
  editing?: GridEditing;
  ed: GridEditingState;
  /** Column nullability aligned with result.columns — shows the ⊗ (set NULL)
   *  button in the cell editor. */
  columnNullable?: (boolean | undefined)[];
  /** Column indices that are foreign keys — ⌘-click follows the reference. */
  fkColumns?: ReadonlySet<number>;
  onFollowFk?: (row: number, col: number) => void;
  /** Right-click on a cell (ci ≥ 0) or the row gutter (ci = -1). */
  onCellContext: (ri: number, ci: number) => void;
  focusGrid: () => void;
  scrollRef: RefObject<HTMLDivElement | null>;
  onScroll: (e: UIEvent<HTMLDivElement>) => void;
  tableStyle?: CSSProperties;
  tableClass: string;
  colgroup: ReactNode;
}

export function GridBody({
  result,
  layout,
  sel,
  editing,
  ed,
  columnNullable,
  fkColumns,
  onFollowFk,
  onCellContext,
  focusGrid,
  scrollRef,
  onScroll,
  tableStyle,
  tableClass,
  colgroup,
}: Props) {
  const { hiddenCols } = layout;
  const { focused, cellSel, rect, rectCount, inRect, focusCell } = sel;
  const { deletedRows, editCell, editIns } = ed;

  // Pending inserts render right under their duplicate-source row; ones
  // without a source on this page go to the bottom.
  const inserts = editing?.inserts ?? [];
  const insertsAfter = new Map<number, number[]>();
  const bottomInserts: number[] = [];
  inserts.forEach((ins, ii) => {
    if (ins.after !== undefined && ins.after < result.rows.length) {
      const list = insertsAfter.get(ins.after);
      if (list) list.push(ii);
      else insertsAfter.set(ins.after, [ii]);
    } else {
      bottomInserts.push(ii);
    }
  });

  const renderInsert = (ii: number) =>
    editing ? (
      <InsertRowTr
        key={`+${ii}`}
        editing={editing}
        ii={ii}
        result={result}
        hiddenCols={hiddenCols}
        columnNullable={columnNullable}
        editIns={editIns}
        onStartEdit={ed.startInsertEdit}
        onCloseEdit={(refocus) => {
          ed.setEditIns(null);
          if (refocus) focusGrid();
        }}
      />
    ) : null;

  return (
    <div ref={scrollRef} onScroll={onScroll} className="min-h-0 flex-1 overflow-auto">
      <table ref={layout.bodyTableRef} style={tableStyle} className={tableClass}>
        {colgroup}
        <tbody>
        {result.rows.map((row, ri) => {
          const isSelected = sel.selected.has(ri);
          const isDeleted = deletedRows.has(ri);
          // The active cell's row (single focus or a spanned cell range):
          // tint its number gutter faintly, like Excel highlights the row
          // header of the selected cell — lighter than a whole-row select.
          const cellInRow = rect !== null && ri >= rect.r1 && ri <= rect.r2;
          const dups = insertsAfter.get(ri);
          const tr = (
            <tr
              key={ri}
              className={cn(
                isDeleted
                  ? "bg-red-950/40"
                  : isSelected
                    ? "bg-sky-600/15"
                    : "hover:bg-zinc-800/40",
              )}
            >
              <td
                title="Select row"
                onClick={(e) => {
                  // whole-row selection lives on the number gutter only;
                  // it replaces any cell selection
                  sel.selectRow(ri, e);
                  sel.setFocused(null);
                  sel.setCellSel(null);
                }}
                onMouseDown={(e) => {
                  // keep a drag over the gutter from starting a native
                  // text selection; refocus since preventDefault also
                  // cancels the focus transfer
                  if (e.button === 0 && !e.shiftKey) {
                    e.preventDefault();
                    focusGrid();
                  }
                  // plain press selects immediately and arms a row drag:
                  // rows entered while the button is held extend the range
                  if (
                    e.button === 0 &&
                    !e.shiftKey &&
                    !e.metaKey &&
                    !e.ctrlKey
                  ) {
                    sel.rowDrag.current = ri;
                    sel.setSelected(new Set([ri]));
                    sel.setAnchor(ri);
                    sel.setSelCols(new Set());
                    sel.setColAnchor(null);
                    sel.setFocused(null);
                    sel.setCellSel(null);
                  }
                }}
                onMouseEnter={() => sel.extendRowDrag(ri)}
                // Row-gutter right-click targets the row (no specific
                // column). Lives here, not on the <tr>, so a data-cell
                // right-click doesn't also bubble a col=-1 context that
                // would retarget and drop the cell selection.
                onContextMenu={() => onCellContext(ri, -1)}
                className={cn(
                  "cursor-pointer border-b border-r border-zinc-800/70 px-2 py-0.5 text-right",
                  isDeleted
                    ? "text-red-400/70"
                    : isSelected
                      ? "text-sky-400"
                      : cellInRow
                        ? "bg-sky-500/10 text-sky-400/80"
                        : "text-zinc-600 hover:text-zinc-400",
                )}
              >
                {ri + 1}
              </td>
              {result.columns.map((_col, ci) => {
                if (hiddenCols.has(ci)) return null;
                const value = row[ci] ?? null;
                const staged = editing
                  ? ed.stagedOf(ri, ci)
                  : { has: false, value: null };
                const shown = staged.has ? staged.value : value;
                const isEditing =
                  editCell?.row === ri && editCell?.col === ci;
                const isFocused =
                  focused?.row === ri && focused?.col === ci;
                // A staged cell whose last Apply rolled back: render red even
                // when focused, so a single failed cell isn't masked by the
                // blue selection ring.
                const errored =
                  staged.has && !isDeleted && Boolean(editing?.applyFailed);
                const isFk = Boolean(
                  onFollowFk && fkColumns?.has(ci) && shown !== null,
                );
                return (
                  <td
                    key={ci}
                    title={
                      isFk
                        ? `${shown}\n⌘-click — open referenced row`
                        : (shown ?? undefined)
                    }
                    onClick={(e) => {
                      if (isFk && (e.metaKey || e.ctrlKey)) {
                        e.preventDefault();
                        // ячейка-источник остаётся выделенной — иначе после
                        // взгляда на превью непонятно, откуда оно открыто
                        focusCell(ri, ci);
                        onFollowFk?.(ri, ci);
                        return;
                      }
                      // shift+click grows a cell range from the anchor
                      // (cell selection replaces row selection either way)
                      const anchorCell = cellSel?.a ?? focused;
                      if (e.shiftKey && anchorCell) {
                        sel.setSelected(new Set());
                        sel.setAnchor(null);
                        sel.setCellSel({
                          a: anchorCell,
                          b: { row: ri, col: ci },
                        });
                        return;
                      }
                      focusCell(ri, ci);
                    }}
                    onMouseDown={(e) => {
                      if (
                        e.button !== 0 ||
                        e.shiftKey ||
                        e.metaKey ||
                        e.ctrlKey ||
                        (e.target as HTMLElement).tagName === "INPUT"
                      ) {
                        return;
                      }
                      // WKWebView starts a native text selection on drag
                      // even under user-select: none — preventDefault stops
                      // it; skip while a cell editor is open so its blur
                      // still fires and stages the draft
                      if (!editCell && !editIns) {
                        e.preventDefault();
                        focusGrid();
                      }
                      sel.dragSel.current = true;
                      focusCell(ri, ci);
                    }}
                    onMouseEnter={() => {
                      // a gutter-started drag keeps extending the row
                      // range even when the cursor drifts onto the cells
                      if (sel.rowDrag.current !== null) {
                        sel.extendRowDrag(ri);
                        return;
                      }
                      if (sel.dragSel.current) {
                        sel.setCellSel((cs) =>
                          cs ? { a: cs.a, b: { row: ri, col: ci } } : cs,
                        );
                      }
                    }}
                    onContextMenu={() => onCellContext(ri, ci)}
                    onDoubleClick={
                      editing && !isDeleted
                        ? () => ed.startEdit(ri, ci)
                        : undefined
                    }
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 whitespace-pre text-zinc-200 max-w-105 truncate",
                      staged.has &&
                        !isDeleted &&
                        !isEditing &&
                        (errored
                          ? "bg-red-500/15 text-red-300"
                          : "bg-amber-500/15 text-amber-200"),
                      isDeleted && "text-zinc-600 line-through",
                      sel.selCols.has(ci) &&
                        !staged.has &&
                        !isDeleted &&
                        !isEditing &&
                        "bg-sky-500/10",
                      rectCount > 1 &&
                        inRect(ri, ci) &&
                        !isEditing &&
                        "bg-sky-500/15",
                      // Error state owns the ring so a focused failed cell
                      // reads as an error, not just a selection.
                      isFocused &&
                        !isEditing &&
                        (errored
                          ? "bg-red-500/25 shadow-[inset_0_0_0_1px_var(--color-red-500)]"
                          : "bg-sky-500/20 shadow-[inset_0_0_0_1px_var(--color-sky-500)]"),
                      // The editing highlight lives on the cell, not the
                      // input — it replaces any other background/ring.
                      isEditing &&
                        "bg-zinc-900 shadow-[inset_0_0_0_1.5px_var(--color-amber-400)]",
                    )}
                  >
                    {isEditing ? (
                      <CellInput
                        initial={editCell.initial}
                        onStage={(v) => editing?.onEdit(ri, ci, v)}
                        onNull={
                          columnNullable?.[ci]
                            ? () => editing?.onEdit(ri, ci, null)
                            : undefined
                        }
                        onClose={(refocus) => {
                          ed.setEditCell(null);
                          if (refocus) focusGrid();
                        }}
                      />
                    ) : shown === null ? (
                      <span
                        className={cn(
                          "italic",
                          staged.has
                            ? editing?.applyFailed
                              ? "text-red-400"
                              : "text-amber-400"
                            : "text-zinc-600",
                        )}
                      >
                        NULL
                      </span>
                    ) : isFk ? (
                      <span className="underline decoration-dotted decoration-zinc-500 underline-offset-2">
                        {shown}
                      </span>
                    ) : (
                      shown
                    )}
                  </td>
                );
              })}
            </tr>
          );
          return dups ? (
            <Fragment key={ri}>
              {tr}
              {dups.map(renderInsert)}
            </Fragment>
          ) : (
            tr
          );
        })}
        {bottomInserts.map(renderInsert)}
        </tbody>
      </table>
      {result.rows.length === 0 && (
        <div className="px-3 py-2 text-[12px] text-zinc-500">no rows</div>
      )}
    </div>
  );
}
