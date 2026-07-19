// Results grid orchestrator. The moving parts live in grid/: column layout
// (useColumnLayout), the selection model (useGridSelection), the staged-edit
// state (useGridEditing), copy/export actions (copyActions), the header and
// body markup (GridHeader/GridBody), the inline/full-value editors
// (CellInput/CellDialog), pending-INSERT rows (InsertRowTr) and both context
// menus (menus.tsx). This file wires them to the keyboard/mouse handlers.
import {
  memo,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import type { ClipboardEvent } from "react";
import { readClipboardText } from "../lib/clipboard";
import { useApp } from "../lib/store";
import type { SortSpec, StatementResult } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
} from "./ContextMenu";
import { CellDialog } from "./grid/CellDialog";
import { makeCopyActions } from "./grid/copyActions";
import { GridBody } from "./grid/GridBody";
import { GridHeader } from "./grid/GridHeader";
import { CellMenu, ColumnMenu } from "./grid/menus";
import type { CellRef, GridEditing } from "./grid/types";
import { useColumnLayout } from "./grid/useColumnLayout";
import { useGridEditing } from "./grid/useGridEditing";
import { useGridSelection } from "./grid/useGridSelection";

export type { GridEditing } from "./grid/types";

/** Scroll offsets per result object — same lifetime as the selection
 *  snapshots in useGridSelection: survive the unmount on tab switch, gone
 *  with a new result. */
const scrollPos = new WeakMap<StatementResult, { top: number; left: number }>();

interface Props {
  result: StatementResult;
  /** Active ORDER BY entries in priority order. */
  sorts?: readonly SortSpec[];
  /** Present = sortable; receives the full next sort list (multi-sort aware). */
  onSortsChange?: (sorts: SortSpec[]) => void;
  /** Mirrors the set of hidden column indices to the parent (table tabs drop
   *  them from the generated "current view" SQL). */
  onHiddenColsChange?: (hidden: ReadonlySet<number>) => void;
  editing?: GridEditing;
  /** Declared column types aligned with result.columns (json/jsonb get pretty view). */
  columnTypes?: (string | undefined)[];
  /** Column nullability aligned with result.columns — shows the ⊗ (set NULL)
   *  button in the cell editor. */
  columnNullable?: (boolean | undefined)[];
  /** Source relation of the rows; enables "Copy as INSERT" in the menu. */
  insertTarget?: { schema: string; table: string };
  /** Column indices that are foreign keys — ⌘-click follows the reference. */
  fkColumns?: ReadonlySet<number>;
  onFollowFk?: (row: number, col: number) => void;
}

function ResultsGridImpl({
  result,
  sorts,
  onSortsChange,
  onHiddenColsChange,
  editing,
  columnTypes,
  columnNullable,
  insertTarget,
  fkColumns,
  onFollowFk,
}: Props) {
  const showToast = useApp((s) => s.showToast);
  /** Column the header context menu is open for (null = cell/row menu). */
  const [menuCol, setMenuCol] = useState<number | null>(null);
  const [menuCell, setMenuCell] = useState<CellRef | null>(null);

  // Hotkeys (⌘C/⌘⏎/⌘S/Esc) live on this container and only fire while focus
  // is inside it — clicks outside the grid naturally deactivate them.
  const gridRef = useRef<HTMLDivElement>(null);
  const focusGrid = () => gridRef.current?.focus();
  /** The vertical scroller (body only — the header sits outside it). */
  const bodyScrollRef = useRef<HTMLDivElement>(null);

  const layout = useColumnLayout(result, onHiddenColsChange);
  const { hiddenCols } = layout;
  const headTableRef = layout.tableRef;

  /** Slides the frozen header to match the body's horizontal scroll. Done with
   *  a transform rather than scrollLeft on the header pane: the pane has no
   *  scrollbar, so its scroll range is shorter than the body's and would clamp
   *  (misaligning the columns) at the far right. */
  const syncHeader = (left: number) => {
    const t = headTableRef.current;
    if (t) t.style.transform = `translateX(${-left}px)`;
  };

  // Narrowing the content (resize/fit/hide column) while scrolled far right
  // clamps the body's scrollLeft; a scroll event isn't guaranteed for that
  // clamp, so re-sync the header whenever the table width changes.
  useLayoutEffect(() => {
    const el = bodyScrollRef.current;
    if (el) syncHeader(el.scrollLeft);
  }, [layout.totalW]);

  // Coming back to the tab lands where the user left off, not at row 1.
  useLayoutEffect(() => {
    const el = bodyScrollRef.current;
    if (!el) return;
    const pos = scrollPos.get(result);
    if (pos) {
      el.scrollTop = pos.top;
      el.scrollLeft = pos.left;
    }
    // Always re-sync: a new result keeps the DOM node (and its transform), but
    // its scrollLeft may clamp to narrower content without firing a scroll.
    syncHeader(el.scrollLeft);
  }, [result]);

  const sel = useGridSelection(result, { hiddenCols, focusGrid });
  const { selected, focused, rect, rectCount, inRect, focusCell, selRows, selColList } = sel;
  const n = selRows.length;

  const ed = useGridEditing({ result, editing, columnTypes, showToast, focusGrid });
  const { dialog, deletedRows, dirty, canEdit } = ed;

  useEffect(() => {
    setMenuCell(null);
    setMenuCol(null);
  }, [result]);

  if (result.columns.length === 0) {
    return null;
  }

  const handleCellContext = (ri: number, ci: number) => {
    setMenuCell({ row: ri, col: ci });
    setMenuCol(null);
    // Right-click inside the current cell rectangle (or on an already-selected
    // row) keeps that selection so the menu acts on it — like Excel/Sheets.
    const insideRect = rectCount > 1 && inRect(ri, ci);
    if (insideRect || selected.has(ri)) return;
    // Outside the selection we retarget, Excel-style: a data-cell right-click
    // focuses that one cell (visible ring; the menu acts on it and its row),
    // the row-number gutter (ci < 0) selects the whole clicked row. Either way
    // any stale cell/row/column selection is dropped so its copy/export actions
    // don't leak into the menu.
    if (ci >= 0) {
      focusCell(ri, ci);
      return;
    }
    sel.setSelected(new Set([ri]));
    sel.setAnchor(ri);
    sel.setCellSel(null);
    sel.setFocused(null);
    sel.setSelCols(new Set());
    sel.setColAnchor(null);
  };

  // Rows the context menu's row-scope actions target: the explicit row
  // selection, or — when a lone cell was right-clicked outside any selection —
  // that cell's row, so "Copy row"/"Export row" stay meaningful for one cell.
  const menuRows = n > 0 ? selRows : menuCell ? [menuCell.row] : [];

  const copy = makeCopyActions({
    result,
    shownValue: ed.shownValue,
    menuRows,
    selColList,
    rect,
    rectCount,
    hiddenCols,
    insertTarget,
    showToast,
  });

  // --- Sorting --------------------------------------------------------------
  const sortIdxOf = (name: string) =>
    sorts ? sorts.findIndex((s) => s.column === name) : -1;

  /** Plain click: single-sort toggle. add=true (shift / menu): append the
   *  column to the list, or toggle its direction if already present. */
  const applySort = (name: string, dir?: "asc" | "desc", add = false) => {
    if (!onSortsChange) return;
    const cur = sorts ? [...sorts] : [];
    const i = cur.findIndex((s) => s.column === name);
    if (add) {
      if (i >= 0) {
        cur[i] = {
          column: name,
          dir: dir ?? (cur[i].dir === "asc" ? "desc" : "asc"),
        };
      } else {
        cur.push({ column: name, dir: dir ?? "asc" });
      }
      onSortsChange(cur);
    } else {
      const prev = i >= 0 ? cur[i].dir : undefined;
      onSortsChange([
        { column: name, dir: dir ?? (prev === "asc" ? "desc" : "asc") },
      ]);
    }
  };

  /** Where a TSV paste lands: the selection's top-left cell, or the first
   *  selected row's first column. */
  const pasteStart: CellRef | null = rect
    ? { row: rect.r1, col: rect.c1 }
    : (focused ?? (selRows.length > 0 ? { row: selRows[0], col: 0 } : null));

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "c") {
      // column > cell selection > whole rows (via the row-number gutter)
      if (selColList.length > 0) {
        e.preventDefault();
        copy.copyColumns();
      } else if (rectCount > 1) {
        e.preventDefault();
        copy.copyCells();
      } else if (focused) {
        e.preventDefault();
        copy.copyCellAt(focused.row, focused.col);
      } else if (n > 0) {
        e.preventDefault();
        copy.copyRows(" ", "");
      }
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "v") {
      // ⌘V comes in as keydown: WKWebView never dispatches `paste` events
      // to non-editable elements, so the clipboard is read explicitly
      const t = e.target as HTMLElement;
      if (t.tagName !== "INPUT" && t.tagName !== "TEXTAREA") {
        e.preventDefault();
        void readClipboardText().then(
          (text) => text && ed.applyTsv(text, pasteStart),
        );
      }
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "d") {
      const rows = n > 0 ? selRows : focused ? [focused.row] : [];
      if (rows.length > 0) {
        e.preventDefault();
        if (canEdit) editing?.onDuplicate(rows);
      }
    }
    if (
      (e.key === "Backspace" || e.key === "Delete") &&
      !e.metaKey &&
      !e.ctrlKey &&
      !e.altKey
    ) {
      // stages a DELETE (or restores when everything targeted is already
      // staged) — nothing hits the database until Apply
      const rows = n > 0 ? selRows : focused ? [focused.row] : [];
      if (rows.length > 0 && canEdit) {
        e.preventDefault();
        const allDeleted = rows.every((r) => deletedRows.has(r));
        editing?.onToggleDelete(rows, !allDeleted);
      }
    }
    if ((e.metaKey || e.ctrlKey) && e.key === "Enter" && focused) {
      e.preventDefault();
      ed.openCellDialog(focused.row, focused.col);
    }
    if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === "s") {
      e.preventDefault();
      if (dirty) editing?.onApply?.();
    }
    if (e.key === "Escape") {
      if (sel.selCols.size > 0) {
        sel.setSelCols(new Set());
        sel.setColAnchor(null);
      } else if (rectCount > 1) sel.setCellSel(null);
      else if (n > 0) {
        sel.setSelected(new Set());
        sel.setAnchor(null);
      } else if (dirty) editing?.onDiscard?.();
    }
  };

  /** Fallback path for environments that do fire `paste` here (dev browser). */
  const handlePaste = (e: ClipboardEvent) => {
    if (!editing || !canEdit) return;
    const target = e.target as HTMLElement;
    if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
    const text = e.clipboardData.getData("text/plain");
    if (!text) return;
    e.preventDefault();
    ed.applyTsv(text, pasteStart);
  };

  const rowsLabel = menuRows.length > 1 ? `${menuRows.length} rows` : "row";

  const menuCellStaged = Boolean(
    menuCell && menuCell.col >= 0 && ed.stagedOf(menuCell.row, menuCell.col).has,
  );
  const allSelectedDeleted =
    menuRows.length > 0 && menuRows.every((r) => deletedRows.has(r));

  // Header and body are two separate tables (see the header pane in
  // GridHeader), so both get the same width + colgroup to keep their columns
  // lined up.
  const tableStyle = layout.sized
    ? { tableLayout: "fixed" as const, width: layout.totalW }
    : undefined;
  const tableClass = "border-separate border-spacing-0 text-[12px] font-mono";
  const colgroup = (
    <colgroup>
      <col style={layout.sized ? { width: layout.colWidths[-1] } : undefined} />
      {result.columns.map((_c, i) =>
        hiddenCols.has(i) ? null : (
          <col
            key={i}
            style={layout.sized ? { width: layout.colWidths[i] } : undefined}
          />
        ),
      )}
    </colgroup>
  );
  return (
    <>
    <ContextMenu>
      <ContextMenuTrigger
        ref={gridRef}
        tabIndex={0}
        onKeyDown={handleKeyDown}
        onPaste={handlePaste}
        onMouseDown={(e) => {
          // WebKit extends native text selection on shift+click even under
          // user-select: none; shift here means row/cell range selection
          if (e.shiftKey) {
            e.preventDefault();
            focusGrid(); // preventDefault also cancels the focus transfer
          }
        }}
        onContextMenu={(e) => {
          const el = e.target as Element;
          if (!el.closest?.("td")) setMenuCell(null);
          if (!el.closest?.("th")) setMenuCol(null);
        }}
        className="flex h-full flex-col outline-none"
      >
        <GridHeader
          result={result}
          layout={layout}
          sel={sel}
          sorts={sorts}
          onSortsChange={onSortsChange}
          applySort={applySort}
          sortIdxOf={sortIdxOf}
          tableStyle={tableStyle}
          tableClass={tableClass}
          colgroup={colgroup}
          onMenuTarget={(col) => {
            setMenuCol(col);
            setMenuCell(null);
          }}
        />
        <GridBody
          result={result}
          layout={layout}
          sel={sel}
          editing={editing}
          ed={ed}
          columnNullable={columnNullable}
          fkColumns={fkColumns}
          onFollowFk={onFollowFk}
          onCellContext={handleCellContext}
          focusGrid={focusGrid}
          scrollRef={bodyScrollRef}
          onScroll={(e) => {
            const el = e.currentTarget;
            syncHeader(el.scrollLeft);
            scrollPos.set(result, { top: el.scrollTop, left: el.scrollLeft });
          }}
          tableStyle={tableStyle}
          tableClass={tableClass}
          colgroup={colgroup}
        />
      </ContextMenuTrigger>

      <ContextMenuContent>
        {menuCol !== null ? (
          <ColumnMenu
            menuCol={menuCol}
            result={result}
            sorts={sorts}
            onSortsChange={onSortsChange}
            applySort={applySort}
            sortIdxOf={sortIdxOf}
            selColList={selColList}
            copy={copy}
            layout={layout}
            onHideColumns={(cols) => {
              const next = new Set(hiddenCols);
              cols.forEach((c) => next.add(c));
              layout.changeHiddenCols(next);
              sel.setSelCols(new Set());
              sel.setColAnchor(null);
            }}
          />
        ) : (
          <CellMenu
            menuCell={menuCell}
            menuCellStaged={menuCellStaged}
            result={result}
            rectCount={rectCount}
            selRows={selRows}
            menuRows={menuRows}
            rowsLabel={rowsLabel}
            allSelectedDeleted={allSelectedDeleted}
            insertTarget={insertTarget}
            fkColumns={fkColumns}
            onFollowFk={onFollowFk}
            editing={editing}
            canEdit={canEdit}
            copy={copy}
            openCellDialog={ed.openCellDialog}
            startEdit={ed.startEdit}
            hiddenCount={hiddenCols.size}
            resetLayout={layout.resetLayout}
          />
        )}
      </ContextMenuContent>
    </ContextMenu>

    {dialog && (
      <CellDialog
        dialog={dialog}
        columnName={result.columns[dialog.col]}
        columnType={columnTypes?.[dialog.col]}
        canEdit={canEdit}
        onText={(text) => ed.setDialog({ ...dialog, text })}
        onStage={ed.stageDialog}
        onClose={ed.closeDialog}
        onCopy={(text) => copy.copyAndToast(text, "Cell copied")}
      />
    )}
    </>
  );
}

/** Memoized: the grid is expensive and TableTab re-renders on its own local
 *  state (filter draft, pagination) — with stable props it can skip those. */
export const ResultsGrid = memo(ResultsGridImpl);
