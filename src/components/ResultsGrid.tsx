import {
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from "@tanstack/react-table";
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  Braces,
  Brackets,
  Copy,
  CopyPlus,
  Database,
  Eraser,
  EyeOff,
  MoveHorizontal,
  Pencil,
  RotateCcw,
  Sheet,
  SquarePen,
  Trash2,
  Undo2,
  UnfoldHorizontal,
  X,
} from "lucide-react";
import {
  Fragment,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type MouseEvent,
} from "react";
import { copyText, readClipboardText } from "../lib/clipboard";
import { quoteIdent, quoteLit } from "../lib/sql";
import { useApp, type InsertRow } from "../lib/store";
import type { SortSpec, StatementResult } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "./context-menu";
import { Button, IconBtn, Overlay, cn } from "./ui";

type Row = (string | null)[];

/** Inline cell editor. Blur stages the draft (clicking away mustn't lose the
 *  input); Enter stages and refocuses the grid; Esc cancels and refocuses.
 *  Keeping the draft local means typing doesn't re-render the whole grid.
 *  The input is borderless — the host cell carries the editing highlight. */
function CellInput({
  initial,
  onStage,
  onNull,
  onClose,
}: {
  initial: string;
  onStage: (value: string) => void;
  /** Stages NULL via the ⊗ button; present only for nullable columns. */
  onNull?: () => void;
  /** refocus=true hands focus back to the grid (Enter/Esc, not blur). */
  onClose: (refocus: boolean) => void;
}) {
  const [draft, setDraft] = useState(initial);
  // Enter/Esc refocus the grid, which fires blur before the input unmounts —
  // this flag keeps that blur from staging a value already handled (or, for
  // Esc, explicitly cancelled).
  const skipBlur = useRef(false);
  return (
    <span className="flex h-[18px] w-full items-center gap-1">
      <input
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onClick={(e) => e.stopPropagation()}
        onBlur={() => {
          if (skipBlur.current) return;
          onStage(draft);
          onClose(false);
        }}
        onKeyDown={(e) => {
          // Don't let Esc bubble up and discard ALL edits.
          e.stopPropagation();
          if (e.key === "Enter") {
            skipBlur.current = true;
            onStage(draft);
            onClose(true);
          }
          if (e.key === "Escape") {
            skipBlur.current = true;
            onClose(true);
          }
        }}
        className="w-full min-w-0 flex-1 bg-transparent p-0 font-mono text-[12px] text-zinc-100 outline-none"
      />
      {onNull && (
        <button
          title="Set NULL"
          // preventDefault keeps focus on the input so blur doesn't stage
          // the draft before the click lands
          onMouseDown={(e) => e.preventDefault()}
          onClick={(e) => {
            e.stopPropagation();
            skipBlur.current = true;
            onNull();
            onClose(true);
          }}
          className="flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full bg-zinc-400 text-zinc-950 hover:bg-zinc-100"
        >
          <X size={9} strokeWidth={3} />
        </button>
      )}
    </span>
  );
}

/** Staged-edit wiring; when present the grid becomes editable (dblclick a cell). */
export interface GridEditing {
  edits: Record<number, Record<number, string | null>>;
  deletes: readonly number[];
  /** Pending INSERT rows shown under the data (⌘D duplicate). */
  inserts: readonly InsertRow[];
  /** Set when editing is unavailable (e.g. no primary key) — shown on attempt. */
  disabledReason?: string;
  /** Last Apply failed and rolled back — staged cells render red, not amber. */
  applyFailed?: boolean;
  onEdit: (row: number, col: number, value: string | null) => void;
  onToggleDelete: (rows: number[], del: boolean) => void;
  onDuplicate: (rows: number[]) => void;
  onInsertEdit: (index: number, col: number, value: string | null | undefined) => void;
  onInsertRemove: (index: number) => void;
  /** ⌘S while the grid is focused and changes are pending. */
  onApply?: () => void;
  /** Esc while the grid is focused and changes are pending. */
  onDiscard?: () => void;
}

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
}

export function ResultsGrid({
  result,
  sorts,
  onSortsChange,
  onHiddenColsChange,
  editing,
  columnTypes,
  columnNullable,
  insertTarget,
}: Props) {
  const showToast = useApp((s) => s.showToast);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [anchor, setAnchor] = useState<number | null>(null);
  /** Whole-column selection (header click; shift/⌘ extend), exclusive with
   *  row and cell selection. */
  const [selCols, setSelCols] = useState<ReadonlySet<number>>(new Set());
  const [colAnchor, setColAnchor] = useState<number | null>(null);
  const [menuCell, setMenuCell] = useState<{ row: number; col: number } | null>(
    null,
  );
  const [editCell, setEditCell] = useState<{
    row: number;
    col: number;
    initial: string;
  } | null>(null);
  /** Cell being edited in a pending INSERT row. */
  const [editIns, setEditIns] = useState<{
    index: number;
    col: number;
    initial: string;
  } | null>(null);
  const [focused, setFocused] = useState<{ row: number; col: number } | null>(
    null,
  );
  /** Rectangular cell selection (shift+click extends from the anchor). */
  const [cellSel, setCellSel] = useState<{
    a: { row: number; col: number };
    b: { row: number; col: number };
  } | null>(null);
  const [dialog, setDialog] = useState<{
    row: number;
    col: number;
    text: string;
    isJson: boolean;
  } | null>(null);
  // Hotkeys (⌘C/⌘⏎/⌘S/Esc) live on this container and only fire while focus
  // is inside it — clicks outside the grid naturally deactivate them.
  const gridRef = useRef<HTMLDivElement>(null);
  const focusGrid = () => gridRef.current?.focus();

  // --- Column layout -------------------------------------------------------
  // First data render is auto-layout; actual widths are then measured and the
  // table switches to table-layout:fixed. Fixed widths keep cells from
  // reflowing when an editor mounts and make drag-resize possible. Key -1 is
  // the row-number gutter. Widths survive result swaps with the same column
  // set (page/sort) and re-measure when the columns change.
  const tableRef = useRef<HTMLTableElement>(null);
  const [colWidths, setColWidths] = useState<Record<number, number>>({});
  const [hiddenCols, setHiddenCols] = useState<ReadonlySet<number>>(new Set());
  const [sizedFor, setSizedFor] = useState<string | null>(null);
  /** Column the header context menu is open for (null = cell/row menu). */
  const [menuCol, setMenuCol] = useState<number | null>(null);
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

  /** Focuses a single cell, replacing any row/column selection. */
  const focusCell = (ri: number, ci: number) => {
    setSelected(new Set());
    setAnchor(null);
    setSelCols(new Set());
    setColAnchor(null);
    setFocused({ row: ri, col: ci });
    setCellSel({ a: { row: ri, col: ci }, b: { row: ri, col: ci } });
  };

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
    setMenuCell(null);
    setMenuCol(null);
    setEditCell(null);
    setEditIns(null);
    setFocused(null);
    setCellSel(null);
    setDialog(null);
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

  const columns = useMemo<ColumnDef<Row>[]>(
    () =>
      result.columns.map((name, i) => ({
        id: String(i),
        header: name,
        accessorFn: (row: Row) => row[i],
      })),
    [result.columns],
  );

  const table = useReactTable({
    data: result.rows,
    columns,
    getCoreRowModel: getCoreRowModel(),
  });

  if (result.columns.length === 0) {
    return null;
  }

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

  const onCellContext = (ri: number, ci: number) => {
    setMenuCell({ row: ri, col: ci });
    setMenuCol(null);
    if (!selected.has(ri)) {
      setSelected(new Set([ri]));
      setAnchor(ri);
    }
  };

  const selRows = [...selected]
    .sort((a, b) => a - b)
    .filter((i) => i < result.rows.length);
  const n = selRows.length;

  const copied = (what: string) => showToast(what, "info");

  /** Copies and reports success in the status bar. */
  const copyAndToast = (text: string, message: string) =>
    void copyText(text).then((ok) => ok && copied(message));

  const copyRows = (sep: string, suffix: string) => {
    if (!n) return;
    const text = selRows
      .map((i) => result.rows[i].map((v) => v ?? "").join(sep))
      .join("\n");
    copyAndToast(text, `Copied ${n} row(s) ${suffix}`);
  };

  const copyJson = () => {
    if (!n) return;
    const objs = selRows.map((i) =>
      Object.fromEntries(result.columns.map((c, ci) => [c, result.rows[i][ci]])),
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
    const rel = `${quoteIdent(insertTarget.schema)}.${quoteIdent(insertTarget.table)}`;
    const cols = result.columns.map(quoteIdent).join(", ");
    const tuples = selRows.map(
      (i) =>
        `  (${result.rows[i]
          .map((v) => (v === null ? "NULL" : quoteLit(v)))
          .join(", ")})`,
    );
    copyAndToast(
      `INSERT INTO ${rel} (${cols}) VALUES\n${tuples.join(",\n")};`,
      `Copied ${n} row(s) as INSERT`,
    );
  };

  const copyCellAt = (ri: number, ci: number) =>
    copyAndToast(result.rows[ri]?.[ci] ?? "", "Cell copied");

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
      for (let i = lo; i <= hi; i++) if (!hiddenCols.has(i)) range.add(i);
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
    focusGrid();
  };

  /** Selected columns in display order (bounded to the current result). */
  const selColList = [...selCols]
    .filter((c) => c >= 0 && c < result.columns.length)
    .sort((a, b) => a - b);

  /** TSV of the selected columns across all rows (single column = lines). */
  const copyColumns = () => {
    if (!selColList.length) return;
    const lines = result.rows.map((r) =>
      selColList.map((c) => r[c] ?? "").join("\t"),
    );
    copyAndToast(
      lines.join("\n"),
      `Copied ${selColList.length} column(s) as TSV`,
    );
  };

  const copyColumnValues = (ci: number) => {
    const text = result.rows.map((r) => r[ci] ?? "").join("\n");
    copyAndToast(text, `Copied ${result.rows.length} value(s)`);
  };

  /** Distinct non-NULL values as a quoted SQL IN list. */
  const copyColumnIn = (ci: number) => {
    const seen = new Set<string>();
    for (const r of result.rows) {
      const v = r[ci];
      if (v !== null) seen.add(v);
    }
    const list = [...seen].map(quoteLit).join(", ");
    copyAndToast(`(${list})`, `Copied ${seen.size} value(s) for IN`);
  };

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

  const copyCell = () => {
    if (!menuCell || menuCell.col < 0) return;
    copyCellAt(menuCell.row, menuCell.col);
  };

  /** TSV of the rectangular cell selection. */
  const copyCells = () => {
    if (!rect) return;
    const lines: string[] = [];
    for (let r = rect.r1; r <= rect.r2; r++) {
      const row = result.rows[r];
      if (!row) continue;
      lines.push(
        row
          .slice(rect.c1, rect.c2 + 1)
          .map((v) => v ?? "")
          .join("\t"),
      );
    }
    copyAndToast(lines.join("\n"), `Copied ${rectCount} cell(s) as TSV`);
  };

  const copyAll = () => {
    const text = [
      result.columns.join("\t"),
      ...result.rows.map((r) => r.map((v) => v ?? "").join("\t")),
    ].join("\n");
    copyAndToast(text, `Copied ${result.rows.length} row(s) with header`);
  };

  const dirty = Boolean(
    editing &&
      (Object.keys(editing.edits).length > 0 ||
        editing.deletes.length > 0 ||
        editing.inserts.length > 0),
  );

  const canEdit = Boolean(editing) && !editing?.disabledReason;

  const onKeyDown = (e: React.KeyboardEvent) => {
    if ((e.metaKey || e.ctrlKey) && e.key === "c") {
      // column > cell selection > whole rows (via the row-number gutter)
      if (selColList.length > 0) {
        e.preventDefault();
        copyColumns();
      } else if (rectCount > 1) {
        e.preventDefault();
        copyCells();
      } else if (focused) {
        e.preventDefault();
        copyCellAt(focused.row, focused.col);
      } else if (n > 0) {
        e.preventDefault();
        copyRows(" ", "");
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
      if (selCols.size > 0) {
        setSelCols(new Set());
        setColAnchor(null);
      } else if (rectCount > 1) setCellSel(null);
      else if (n > 0) {
        setSelected(new Set());
        setAnchor(null);
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
    if (pasted > 0) copied(`Pasted ${pasted} cell(s) — staged`);
  };

  /** Fallback path for environments that do fire `paste` here (dev browser). */
  const onPaste = (e: ClipboardEvent) => {
    if (!editing || !canEdit) return;
    const target = e.target as HTMLElement;
    if (target.tagName === "INPUT" || target.tagName === "TEXTAREA") return;
    const text = e.clipboardData.getData("text/plain");
    if (!text) return;
    e.preventDefault();
    applyTsv(text);
  };

  const rowsLabel = n > 1 ? `${n} rows` : "row";

  const menuCellOk = Boolean(menuCell && menuCell.col >= 0);
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

  const renderInsert = (ii: number) => {
    if (!editing) return null;
    const row = inserts[ii];
    return (
      <tr
        key={`+${ii}`}
        className={cn(
          "group",
          editing.applyFailed ? "bg-red-500/10" : "bg-emerald-950/25",
        )}
      >
        <td className="border-b border-r border-zinc-800/70 px-2 py-0.5 text-right text-emerald-400/80">
          <span className="group-hover:hidden">+{ii + 1}</span>
          <button
            className="hidden text-red-400 group-hover:inline"
            title="Remove pending row"
            onClick={() => editing.onInsertRemove(ii)}
          >
            <X size={11} />
          </button>
        </td>
        {result.columns.map((_, ci) => {
          if (hiddenCols.has(ci)) return null;
          const v = row.values[ci];
          const isEd = editIns?.index === ii && editIns?.col === ci;
          return (
            <td
              key={ci}
              title={v ?? undefined}
              onDoubleClick={() => startInsertEdit(ii, ci)}
              className={cn(
                "border-b border-r border-zinc-800/70 px-2 py-0.5 whitespace-pre text-emerald-100/90 max-w-105 truncate",
                isEd &&
                  "bg-zinc-900 shadow-[inset_0_0_0_1.5px_var(--color-amber-400)]",
              )}
            >
              {isEd ? (
                <CellInput
                  initial={editIns.initial}
                  onStage={(val) =>
                    editing.onInsertEdit(
                      ii,
                      ci,
                      // an untouched generated column stays on DEFAULT
                      val === "" && v === undefined ? undefined : val,
                    )
                  }
                  onNull={
                    columnNullable?.[ci]
                      ? () => editing.onInsertEdit(ii, ci, null)
                      : undefined
                  }
                  onClose={(refocus) => {
                    setEditIns(null);
                    if (refocus) focusGrid();
                  }}
                />
              ) : v === undefined ? (
                <span className="italic text-emerald-500/70">auto</span>
              ) : v === null ? (
                <span className="italic text-zinc-600">NULL</span>
              ) : (
                v
              )}
            </td>
          );
        })}
      </tr>
    );
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

  return (
    <>
    <ContextMenu>
      <ContextMenuTrigger
        ref={gridRef}
        tabIndex={0}
        onKeyDown={onKeyDown}
        onPaste={onPaste}
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
        className="block h-full overflow-auto outline-none"
      >
        <table
          ref={tableRef}
          style={sized ? { tableLayout: "fixed", width: totalW } : undefined}
          className="border-separate border-spacing-0 text-[12px] font-mono"
        >
          <thead className="sticky top-0 z-10">
            <tr>
              <th
                style={sized ? { width: colWidths[-1] } : undefined}
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
                const isColSel = selCols.has(i);
                return (
                  <th
                    key={i}
                    style={sized ? { width: colWidths[i] } : undefined}
                    onClick={(e) => clickColumn(i, e)}
                    onContextMenu={() => {
                      setMenuCol(i);
                      setMenuCell(null);
                      // right-click outside the selection retargets it,
                      // mirroring how row selection behaves
                      if (!selCols.has(i)) {
                        setSelCols(new Set([i]));
                        setColAnchor(i);
                      }
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
                      onMouseDown={(e) => startResize(i, e)}
                      onClick={(e) => e.stopPropagation()}
                      onDoubleClick={(e) => {
                        e.stopPropagation();
                        fitColumn(i);
                      }}
                      title="Drag to resize · double-click to fit"
                      className="absolute right-0 top-0 h-full w-[5px] cursor-col-resize hover:bg-sky-500/60"
                    />
                  </th>
                );
              })}
            </tr>
          </thead>
          <tbody>
            {table.getRowModel().rows.map((row, ri) => {
              const isSelected = selected.has(ri);
              const isDeleted = deletedRows.has(ri);
              const dups = insertsAfter.get(ri);
              const tr = (
                <tr
                  key={row.id}
                  onContextMenu={() => onCellContext(ri, -1)}
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
                      selectRow(ri, e);
                      setFocused(null);
                      setCellSel(null);
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
                        rowDrag.current = ri;
                        setSelected(new Set([ri]));
                        setAnchor(ri);
                        setSelCols(new Set());
                        setColAnchor(null);
                        setFocused(null);
                        setCellSel(null);
                      }
                    }}
                    onMouseEnter={() => extendRowDrag(ri)}
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
                  {row.getVisibleCells().map((cell, ci) => {
                    if (hiddenCols.has(ci)) return null;
                    const value = cell.getValue() as string | null;
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
                    return (
                      <td
                        key={cell.id}
                        title={shown ?? undefined}
                        onClick={(e) => {
                          // shift+click grows a cell range from the anchor
                          // (cell selection replaces row selection either way)
                          const anchorCell = cellSel?.a ?? focused;
                          if (e.shiftKey && anchorCell) {
                            setSelected(new Set());
                            setAnchor(null);
                            setCellSel({ a: anchorCell, b: { row: ri, col: ci } });
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
                          dragSel.current = true;
                          focusCell(ri, ci);
                        }}
                        onMouseEnter={() => {
                          // a gutter-started drag keeps extending the row
                          // range even when the cursor drifts onto the cells
                          if (rowDrag.current !== null) {
                            extendRowDrag(ri);
                            return;
                          }
                          if (dragSel.current) {
                            setCellSel((cs) =>
                              cs ? { a: cs.a, b: { row: ri, col: ci } } : cs,
                            );
                          }
                        }}
                        onContextMenu={() => onCellContext(ri, ci)}
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
                          selCols.has(ci) &&
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
                        ) : (
                          shown
                        )}
                      </td>
                    );
                  })}
                </tr>
              );
              return dups ? (
                <Fragment key={row.id}>
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
      </ContextMenuTrigger>

      <ContextMenuContent>
        {menuCol !== null ? (
          <>
            {onSortsChange && (
              <>
                <ContextMenuItem
                  icon={ArrowUp}
                  onClick={() => applySort(result.columns[menuCol], "asc")}
                >
                  Sort ascending
                </ContextMenuItem>
                <ContextMenuItem
                  icon={ArrowDown}
                  onClick={() => applySort(result.columns[menuCol], "desc")}
                >
                  Sort descending
                </ContextMenuItem>
                {sortIdxOf(result.columns[menuCol]) < 0 ? (
                  <ContextMenuItem
                    icon={ArrowUpDown}
                    onClick={() =>
                      applySort(result.columns[menuCol], undefined, true)
                    }
                    title="⇧click on the header arrow does the same"
                  >
                    Add to multi-sort
                  </ContextMenuItem>
                ) : (
                  (sorts?.length ?? 0) > 1 && (
                    <ContextMenuItem
                      icon={X}
                      onClick={() =>
                        onSortsChange(
                          (sorts ?? []).filter(
                            (s) => s.column !== result.columns[menuCol],
                          ),
                        )
                      }
                    >
                      Remove from sort
                    </ContextMenuItem>
                  )
                )}
                {(sorts?.length ?? 0) > 0 && (
                  <ContextMenuItem icon={X} onClick={() => onSortsChange([])}>
                    Clear sort
                  </ContextMenuItem>
                )}
                <ContextMenuSeparator />
              </>
            )}
            {selColList.length > 1 ? (
              <>
                <ContextMenuItem icon={Sheet} onClick={copyColumns}>
                  Copy {selColList.length} columns (TSV)
                  <ContextMenuShortcut>⌘C</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem
                  icon={Copy}
                  onClick={() =>
                    copyAndToast(
                      selColList.map((c) => result.columns[c]).join(", "),
                      "Column names copied",
                    )
                  }
                >
                  Copy column names
                </ContextMenuItem>
              </>
            ) : (
              <>
                <ContextMenuItem
                  icon={Copy}
                  onClick={() =>
                    copyAndToast(result.columns[menuCol], "Column name copied")
                  }
                >
                  Copy column name
                </ContextMenuItem>
                <ContextMenuItem
                  icon={Sheet}
                  disabled={!result.rows.length}
                  onClick={() => copyColumnValues(menuCol)}
                >
                  Copy column values
                </ContextMenuItem>
                <ContextMenuItem
                  icon={Brackets}
                  disabled={!result.rows.length}
                  onClick={() => copyColumnIn(menuCol)}
                >
                  Copy for IN (…)
                </ContextMenuItem>
              </>
            )}
            <ContextMenuSeparator />
            <ContextMenuItem
              icon={MoveHorizontal}
              onClick={() =>
                (selColList.length > 1 ? selColList : [menuCol]).forEach(
                  fitColumn,
                )
              }
            >
              Fit column width{selColList.length > 1 ? ` (${selColList.length})` : ""}
            </ContextMenuItem>
            <ContextMenuItem icon={UnfoldHorizontal} onClick={fitAllColumns}>
              Fit all columns
            </ContextMenuItem>
            <ContextMenuItem
              icon={MoveHorizontal}
              onClick={() => matchAllColumns(menuCol)}
            >
              Resize all to match
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem
              icon={EyeOff}
              onClick={() => {
                const next = new Set(hiddenCols);
                (selColList.length > 1 ? selColList : [menuCol]).forEach((c) =>
                  next.add(c),
                );
                changeHiddenCols(next);
                setSelCols(new Set());
                setColAnchor(null);
              }}
            >
              {selColList.length > 1
                ? `Hide ${selColList.length} columns`
                : `Hide ${result.columns[menuCol]}`}
            </ContextMenuItem>
            <ContextMenuItem icon={RotateCcw} onClick={resetLayout}>
              Reset layout
            </ContextMenuItem>
          </>
        ) : (
          <>
            <ContextMenuItem icon={Copy} disabled={!menuCellOk} onClick={copyCell}>
              Copy cell
              {rectCount <= 1 && <ContextMenuShortcut>⌘C</ContextMenuShortcut>}
            </ContextMenuItem>
            {rectCount > 1 && (
              <ContextMenuItem icon={Sheet} onClick={copyCells}>
                Copy {rectCount} cells (TSV)
                <ContextMenuShortcut>⌘C</ContextMenuShortcut>
              </ContextMenuItem>
            )}
            <ContextMenuItem icon={Copy} disabled={!n} onClick={() => copyRows(" ", "")}>
              Copy {rowsLabel}
            </ContextMenuItem>
            <ContextMenuItem icon={Sheet} disabled={!n} onClick={() => copyRows("\t", "as TSV")}>
              Copy {rowsLabel} as TSV
            </ContextMenuItem>
            <ContextMenuItem icon={Braces} disabled={!n} onClick={copyJson}>
              Copy {rowsLabel} as JSON
            </ContextMenuItem>
            {insertTarget && (
              <ContextMenuItem icon={Database} disabled={!n} onClick={copyInsert}>
                Copy {rowsLabel} as INSERT
              </ContextMenuItem>
            )}
            <ContextMenuSeparator />
            <ContextMenuItem icon={Sheet} disabled={!result.rows.length} onClick={copyAll}>
              Copy all with header (TSV)
            </ContextMenuItem>
            <ContextMenuSeparator />
            <ContextMenuItem
              icon={SquarePen}
              disabled={!menuCellOk}
              onClick={() => menuCell && openCellDialog(menuCell.row, menuCell.col)}
            >
              Open cell in editor
              <ContextMenuShortcut>⌘⏎</ContextMenuShortcut>
            </ContextMenuItem>
            {editing && (
              <>
                <ContextMenuSeparator />
                <ContextMenuItem
                  icon={Pencil}
                  disabled={!canEdit || !menuCellOk}
                  onClick={() => menuCell && startEdit(menuCell.row, menuCell.col)}
                >
                  Edit cell
                </ContextMenuItem>
                <ContextMenuItem
                  icon={Eraser}
                  disabled={!canEdit || !menuCellOk}
                  onClick={() =>
                    menuCell && editing.onEdit(menuCell.row, menuCell.col, null)
                  }
                >
                  Set cell NULL
                </ContextMenuItem>
                {menuCellStaged && (
                  <ContextMenuItem
                    icon={Undo2}
                    onClick={() =>
                      menuCell &&
                      editing.onEdit(
                        menuCell.row,
                        menuCell.col,
                        result.rows[menuCell.row][menuCell.col],
                      )
                    }
                  >
                    Revert cell
                  </ContextMenuItem>
                )}
                <ContextMenuItem
                  icon={CopyPlus}
                  disabled={!canEdit || !n}
                  onClick={() => editing.onDuplicate(selRows)}
                  title="Copies stay pending until Apply; generated keys are regenerated"
                >
                  Duplicate {rowsLabel}
                  <ContextMenuShortcut>⌘D</ContextMenuShortcut>
                </ContextMenuItem>
                <ContextMenuItem
                  icon={allSelectedDeleted ? Undo2 : Trash2}
                  iconClassName={allSelectedDeleted ? undefined : "text-red-400/80"}
                  disabled={!canEdit || !n}
                  onClick={() => editing.onToggleDelete(selRows, !allSelectedDeleted)}
                >
                  {allSelectedDeleted ? "Restore" : "Delete"} {rowsLabel}
                  <ContextMenuShortcut>⌫</ContextMenuShortcut>
                </ContextMenuItem>
              </>
            )}
            {hiddenCols.size > 0 && (
              <>
                <ContextMenuSeparator />
                <ContextMenuItem icon={RotateCcw} onClick={resetLayout}>
                  Reset layout ({hiddenCols.size} hidden)
                </ContextMenuItem>
              </>
            )}
          </>
        )}
      </ContextMenuContent>
    </ContextMenu>

    {dialog && (
      <Overlay onClose={closeDialog} className="items-center bg-black/60">
        <div className="flex w-[44rem] max-w-[92vw] flex-col rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
          <div className="flex items-center gap-2 border-b border-zinc-800 px-4 py-2.5">
            <span className="font-mono text-[12px] text-zinc-100">
              {result.columns[dialog.col]}
            </span>
            {columnTypes?.[dialog.col] && (
              <span className="rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400">
                {columnTypes[dialog.col]}
              </span>
            )}
            <span className="text-[11px] text-zinc-600">
              row {dialog.row + 1}
            </span>
            <div className="ml-auto">
              <IconBtn onClick={closeDialog}>
                <X size={14} />
              </IconBtn>
            </div>
          </div>
          <textarea
            autoFocus
            spellCheck={false}
            value={dialog.text}
            readOnly={!canEdit}
            onChange={(e) => setDialog({ ...dialog, text: e.target.value })}
            onKeyDown={(e) => {
              if (e.key === "Escape") closeDialog();
              if ((e.metaKey || e.ctrlKey) && e.key === "Enter") stageDialog();
            }}
            className="selectable m-3 h-80 resize-y rounded-md border border-zinc-700 bg-zinc-950 p-2.5 font-mono text-[12px] leading-relaxed text-zinc-100 outline-none focus:border-sky-600"
          />
          <div className="flex items-center gap-2 border-t border-zinc-800 px-4 py-2.5">
            {dialog.isJson && (
              <span className="text-[11px] text-zinc-600">
                JSON · prettified{canEdit ? " · stored compact" : ""}
              </span>
            )}
            <div className="ml-auto flex items-center gap-2">
              <Button onClick={() => copyAndToast(dialog.text, "Cell copied")}>
                Copy
              </Button>
              {canEdit && (
                <Button variant="primary" title="⌘⏎" onClick={stageDialog}>
                  Stage change
                </Button>
              )}
              <Button onClick={closeDialog}>Close</Button>
            </div>
          </div>
        </div>
      </Overlay>
    )}
    </>
  );
}
