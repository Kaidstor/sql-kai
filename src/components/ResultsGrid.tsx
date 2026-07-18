// Results grid orchestrator. The moving parts live in grid/: column layout
// (useColumnLayout), the selection model (useGridSelection), copy/export
// actions (copyActions), the inline/full-value editors (CellInput/CellDialog),
// pending-INSERT rows (InsertRowTr) and both context menus (menus.tsx).
// This file wires them to the table markup and the keyboard/mouse handlers.
import { ArrowDown, ArrowUp, ArrowUpDown } from "lucide-react";
import {
  Fragment,
  memo,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
} from "react";
import { readClipboardText } from "../lib/clipboard";
import { useApp } from "../lib/store";
import type { SortSpec, StatementResult } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuTrigger,
} from "./ContextMenu";
import { cn } from "./ui";
import { CellDialog, type CellDialogState } from "./grid/CellDialog";
import { CellInput } from "./grid/CellInput";
import { makeCopyActions } from "./grid/copyActions";
import { InsertRowTr, type InsertEdit } from "./grid/InsertRowTr";
import { CellMenu, ColumnMenu } from "./grid/menus";
import type { CellRef, GridEditing } from "./grid/types";
import { useColumnLayout } from "./grid/useColumnLayout";
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
  const [editCell, setEditCell] = useState<
    (CellRef & { initial: string }) | null
  >(null);
  /** Cell being edited in a pending INSERT row. */
  const [editIns, setEditIns] = useState<InsertEdit | null>(null);
  const [dialog, setDialog] = useState<CellDialogState | null>(null);

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
  const {
    selected,
    focused,
    cellSel,
    rect,
    rectCount,
    inRect,
    focusCell,
    selRows,
    selColList,
  } = sel;
  const n = selRows.length;

  useEffect(() => {
    setMenuCell(null);
    setMenuCol(null);
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

  if (result.columns.length === 0) {
    return null;
  }

  const handleCellContext = (ri: number, ci: number) => {
    setMenuCell({ row: ri, col: ci });
    setMenuCol(null);
    // Right-click inside the current cell rectangle (or on an already-selected
    // row) keeps that selection so the menu acts on it. Anywhere else retargets
    // to the clicked row — like clicking away — clearing any stale cell/column
    // selection so its copy/export actions don't leak into the menu.
    const insideRect = rectCount > 1 && inRect(ri, ci);
    if (insideRect || selected.has(ri)) return;
    sel.setSelected(new Set([ri]));
    sel.setAnchor(ri);
    sel.setCellSel(null);
    sel.setFocused(null);
    sel.setSelCols(new Set());
    sel.setColAnchor(null);
  };

  const copy = makeCopyActions({
    result,
    shownValue,
    selRows,
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

  const dirty = Boolean(
    editing &&
      (Object.keys(editing.edits).length > 0 ||
        editing.deletes.length > 0 ||
        editing.inserts.length > 0),
  );

  const canEdit = Boolean(editing) && !editing?.disabledReason;

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
        void readClipboardText().then((text) => text && applyTsv(text));
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
      openCellDialog(focused.row, focused.col);
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

  /** Lays TSV out over the grid as staged edits, starting at the
   *  selection's top-left cell (or the first selected row's first column). */
  const applyTsv = (text: string) => {
    if (!editing) return;
    if (editing.disabledReason) {
      showToast(editing.disabledReason, "info");
      return;
    }
    const start = rect
      ? { row: rect.r1, col: rect.c1 }
      : (focused ?? (selRows.length > 0 ? { row: selRows[0], col: 0 } : null));
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
    if (pasted > 0) copy.toastCopied(`Pasted ${pasted} cell(s) — staged`);
  };

  /** Fallback path for environments that do fire `paste` here (dev browser). */
  const handlePaste = (e: ClipboardEvent) => {
    if (!editing || !canEdit) return;
    const target = e.target as HTMLElement;
    if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
    const text = e.clipboardData.getData("text/plain");
    if (!text) return;
    e.preventDefault();
    applyTsv(text);
  };

  const rowsLabel = n > 1 ? `${n} rows` : "row";

  const menuCellStaged = Boolean(
    menuCell && menuCell.col >= 0 && stagedOf(menuCell.row, menuCell.col).has,
  );
  const allSelectedDeleted =
    n > 0 && selRows.every((r) => deletedRows.has(r));

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
        onStartEdit={startInsertEdit}
        onCloseEdit={(refocus) => {
          setEditIns(null);
          if (refocus) focusGrid();
        }}
      />
    ) : null;

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

  // Header and body are two separate tables (see the header pane below), so
  // both get the same width + colgroup to keep their columns lined up.
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
        {/* The header lives outside the scroller instead of being a sticky
         *  thead inside it: WebKit (WKWebView) lets rows bleed past a pinned
         *  element in an overflow container on retina, which showed up as a
         *  ghost row at the header's edge while scrolling. */}
        <div className="shrink-0 overflow-hidden">
          <table ref={headTableRef} style={tableStyle} className={tableClass}>
            {colgroup}
            <thead>
            <tr>
              <th
                style={layout.sized ? { width: layout.colWidths[-1] } : undefined}
                onContextMenu={() => {
                  setMenuCell(null);
                  setMenuCol(null);
                }}
                className="bg-zinc-900 border-b border-r border-zinc-800 px-2 py-1 text-right text-zinc-600 font-normal min-w-10"
              >
                #
              </th>
              {result.columns.map((name, i) => {
                if (hiddenCols.has(i)) return null;
                const sortIdx = sortIdxOf(name);
                const sort = sortIdx >= 0 ? sorts?.[sortIdx] : undefined;
                const isColSel = sel.selCols.has(i);
                return (
                  <th
                    key={i}
                    style={layout.sized ? { width: layout.colWidths[i] } : undefined}
                    onClick={(e) => sel.clickColumn(i, e)}
                    onContextMenu={() => {
                      setMenuCol(i);
                      setMenuCell(null);
                      // right-click outside the selection retargets it,
                      // mirroring how row selection behaves
                      if (!sel.selCols.has(i)) {
                        sel.setSelCols(new Set([i]));
                        sel.setColAnchor(i);
                      }
                      // drop any cell/row selection so the column menu's export
                      // acts on the columns, not a stale rectangle
                      sel.setCellSel(null);
                      sel.setSelected(new Set());
                      sel.setAnchor(null);
                    }}
                    title={`${name} — click selects column (⇧ range, ⌘ toggle)`}
                    className={cn(
                      "group/th relative cursor-pointer border-b border-r border-zinc-800 px-2 py-1 text-left",
                      "font-medium whitespace-nowrap max-w-105",
                      isColSel
                        ? "bg-sky-950 text-sky-300"
                        : "bg-zinc-900 text-zinc-400 hover:text-zinc-200",
                    )}
                  >
                    <span className="flex items-center gap-1">
                      <span className="min-w-0 flex-1 truncate">{name}</span>
                      {onSortsChange && (
                        <button
                          onClick={(e) => {
                            e.stopPropagation();
                            applySort(name, undefined, e.shiftKey);
                          }}
                          title={
                            sort
                              ? `Sorted ${sort.dir} — click toggles · ⇧click for multi-sort`
                              : "Sort — ⇧click adds to multi-sort"
                          }
                          className={cn(
                            "flex shrink-0 items-center gap-px rounded p-0.5 hover:bg-zinc-700/60 hover:text-zinc-100",
                            sort
                              ? "text-sky-400"
                              : "text-zinc-500 opacity-0 group-hover/th:opacity-100",
                          )}
                        >
                          {sort ? (
                            sort.dir === "desc" ? (
                              <ArrowDown size={11} />
                            ) : (
                              <ArrowUp size={11} />
                            )
                          ) : (
                            <ArrowUpDown size={11} />
                          )}
                          {sort && (sorts?.length ?? 0) > 1 && (
                            <span className="text-[9px] font-semibold tabular-nums">
                              {sortIdx + 1}
                            </span>
                          )}
                        </button>
                      )}
                    </span>
                    <span
                      onMouseDown={(e) => layout.startResize(i, e)}
                      onClick={(e) => e.stopPropagation()}
                      onDoubleClick={(e) => {
                        e.stopPropagation();
                        layout.fitColumn(i);
                      }}
                      title="Drag to resize · double-click to fit"
                      className="absolute right-0 top-0 h-full w-[5px] cursor-col-resize hover:bg-sky-500/60"
                    />
                  </th>
                );
              })}
            </tr>
            </thead>
          </table>
        </div>

        <div
          ref={bodyScrollRef}
          onScroll={(e) => {
            const el = e.currentTarget;
            syncHeader(el.scrollLeft);
            scrollPos.set(result, { top: el.scrollTop, left: el.scrollLeft });
          }}
          className="min-h-0 flex-1 overflow-auto"
        >
          <table
            ref={layout.bodyTableRef}
            style={tableStyle}
            className={tableClass}
          >
            {colgroup}
            <tbody>
            {result.rows.map((row, ri) => {
              const isSelected = selected.has(ri);
              const isDeleted = deletedRows.has(ri);
              const dups = insertsAfter.get(ri);
              const tr = (
                <tr
                  key={ri}
                  onContextMenu={() => handleCellContext(ri, -1)}
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
                    className={cn(
                      "cursor-pointer border-b border-r border-zinc-800/70 px-2 py-0.5 text-right",
                      isDeleted
                        ? "text-red-400/70"
                        : isSelected
                          ? "text-sky-400"
                          : "text-zinc-600 hover:text-zinc-400",
                    )}
                  >
                    {ri + 1}
                  </td>
                  {result.columns.map((_col, ci) => {
                    if (hiddenCols.has(ci)) return null;
                    const value = row[ci] ?? null;
                    const staged = editing
                      ? stagedOf(ri, ci)
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
                        onContextMenu={() => handleCellContext(ri, ci)}
                        onDoubleClick={
                          editing && !isDeleted
                            ? () => startEdit(ri, ci)
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
                              setEditCell(null);
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
            rowsLabel={rowsLabel}
            allSelectedDeleted={allSelectedDeleted}
            insertTarget={insertTarget}
            fkColumns={fkColumns}
            onFollowFk={onFollowFk}
            editing={editing}
            canEdit={canEdit}
            copy={copy}
            openCellDialog={openCellDialog}
            startEdit={startEdit}
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
        onText={(text) => setDialog({ ...dialog, text })}
        onStage={stageDialog}
        onClose={closeDialog}
        onCopy={(text) => copy.copyAndToast(text, "Cell copied")}
      />
    )}
    </>
  );
}

/** Memoized: the grid is expensive and TableTab re-renders on its own local
 *  state (filter draft, pagination) — with stable props it can skip those. */
export const ResultsGrid = memo(ResultsGridImpl);
