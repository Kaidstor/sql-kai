import {
  flexRender,
  getCoreRowModel,
  useReactTable,
  type ColumnDef,
} from "@tanstack/react-table";
import {
  ArrowDown,
  ArrowUp,
  Braces,
  Copy,
  CopyPlus,
  Database,
  Eraser,
  Pencil,
  Sheet,
  SquarePen,
  Trash2,
  Undo2,
  X,
} from "lucide-react";
import {
  Fragment,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ClipboardEvent,
  type MouseEvent,
} from "react";
import { copyText, readClipboardText } from "../lib/clipboard";
import { quoteIdent, quoteLit } from "../lib/sql";
import { useApp, type InsertRow } from "../lib/store";
import type { StatementResult } from "../lib/types";
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
 *  Keeping the draft local means typing doesn't re-render the whole grid. */
function CellInput({
  initial,
  onStage,
  onClose,
}: {
  initial: string;
  onStage: (value: string) => void;
  /** refocus=true hands focus back to the grid (Enter/Esc, not blur). */
  onClose: (refocus: boolean) => void;
}) {
  const [draft, setDraft] = useState(initial);
  // Enter/Esc refocus the grid, which fires blur before the input unmounts —
  // this flag keeps that blur from staging a value already handled (or, for
  // Esc, explicitly cancelled).
  const skipBlur = useRef(false);
  return (
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
      className="block h-[18px] w-full min-w-24 -mx-1 rounded-sm border border-sky-500 bg-zinc-900 px-1 font-mono text-[12px] text-zinc-100 outline-none"
    />
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
  sortCol?: string;
  sortDir?: "asc" | "desc";
  onSort?: (column: string) => void;
  editing?: GridEditing;
  /** Declared column types aligned with result.columns (json/jsonb get pretty view). */
  columnTypes?: (string | undefined)[];
  /** Source relation of the rows; enables "Copy as INSERT" in the menu. */
  insertTarget?: { schema: string; table: string };
}

export function ResultsGrid({
  result,
  sortCol,
  sortDir,
  onSort,
  editing,
  columnTypes,
  insertTarget,
}: Props) {
  const showToast = useApp((s) => s.showToast);
  const [selected, setSelected] = useState<ReadonlySet<number>>(new Set());
  const [anchor, setAnchor] = useState<number | null>(null);
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
    setMenuCell(null);
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

  const copyRows = (sep: string, suffix: string) => {
    if (!n) return;
    const text = selRows
      .map((i) => result.rows[i].map((v) => v ?? "").join(sep))
      .join("\n");
    void copyText(text).then(
      (ok) => ok && copied(`Copied ${n} row(s) ${suffix}`),
    );
  };

  const copyJson = () => {
    if (!n) return;
    const objs = selRows.map((i) =>
      Object.fromEntries(result.columns.map((c, ci) => [c, result.rows[i][ci]])),
    );
    void copyText(JSON.stringify(n === 1 ? objs[0] : objs, null, 2)).then(
      (ok) => ok && copied(`Copied ${n} row(s) as JSON`),
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
    const sql = `INSERT INTO ${rel} (${cols}) VALUES\n${tuples.join(",\n")};`;
    void copyText(sql).then(
      (ok) => ok && copied(`Copied ${n} row(s) as INSERT`),
    );
  };

  const copyCellAt = (ri: number, ci: number) => {
    const value = result.rows[ri]?.[ci] ?? "";
    void copyText(value).then((ok) => ok && copied("Cell copied"));
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
    void copyText(lines.join("\n")).then(
      (ok) => ok && copied(`Copied ${rectCount} cell(s) as TSV`),
    );
  };

  const copyAll = () => {
    const text = [
      result.columns.join("\t"),
      ...result.rows.map((r) => r.map((v) => v ?? "").join("\t")),
    ].join("\n");
    void copyText(text).then(
      (ok) => ok && copied(`Copied ${result.rows.length} row(s) with header`),
    );
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
      // cell selection wins; whole rows copy only when explicitly
      // selected via the row-number gutter
      if (rectCount > 1) {
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
      if (rectCount > 1) setCellSel(null);
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
          const v = row.values[ci];
          const isEd = editIns?.index === ii && editIns?.col === ci;
          return (
            <td
              key={ci}
              title={v ?? undefined}
              onDoubleClick={() => startInsertEdit(ii, ci)}
              className="border-b border-r border-zinc-800/70 px-2 py-0.5 whitespace-pre text-emerald-100/90 max-w-105 truncate"
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
          if (!(e.target as Element).closest?.("td")) setMenuCell(null);
        }}
        className="block h-full overflow-auto outline-none"
      >
        <table className="border-separate border-spacing-0 text-[12px] font-mono">
          <thead className="sticky top-0 z-10">
            <tr>
              <th className="bg-zinc-900 border-b border-r border-zinc-800 px-2 py-1 text-right text-zinc-600 font-normal min-w-10">
                #
              </th>
              {table.getFlatHeaders().map((header) => {
                const name = result.columns[Number(header.id)];
                const sorted = sortCol === name;
                return (
                  <th
                    key={header.id}
                    onClick={onSort ? () => onSort(name) : undefined}
                    className={cn(
                      "bg-zinc-900 border-b border-r border-zinc-800 px-2 py-1 text-left",
                      "font-medium text-zinc-400 whitespace-nowrap max-w-105",
                      onSort && "cursor-pointer hover:text-zinc-100",
                      sorted && "text-sky-400",
                    )}
                  >
                    <span className="inline-flex items-center gap-1">
                      {flexRender(
                        header.column.columnDef.header,
                        header.getContext(),
                      )}
                      {sorted &&
                        (sortDir === "desc" ? (
                          <ArrowDown size={11} />
                        ) : (
                          <ArrowUp size={11} />
                        ))}
                    </span>
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
                          // cell selection replaces row selection
                          setSelected(new Set());
                          setAnchor(null);
                          // shift+click grows a cell range from the anchor
                          const anchorCell = cellSel?.a ?? focused;
                          if (e.shiftKey && anchorCell) {
                            setCellSel({ a: anchorCell, b: { row: ri, col: ci } });
                            return;
                          }
                          setFocused({ row: ri, col: ci });
                          setCellSel({
                            a: { row: ri, col: ci },
                            b: { row: ri, col: ci },
                          });
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
                          setSelected(new Set());
                          setAnchor(null);
                          setFocused({ row: ri, col: ci });
                          setCellSel({
                            a: { row: ri, col: ci },
                            b: { row: ri, col: ci },
                          });
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
                            (errored
                              ? "bg-red-500/15 text-red-300"
                              : "bg-amber-500/15 text-amber-200"),
                          isDeleted && "text-zinc-600 line-through",
                          rectCount > 1 && inRect(ri, ci) && "bg-sky-500/15",
                          // Error state owns the ring so a focused failed cell
                          // reads as an error, not just a selection.
                          isFocused &&
                            (errored
                              ? "bg-red-500/25 shadow-[inset_0_0_0_1px_var(--color-red-500)]"
                              : "bg-sky-500/20 shadow-[inset_0_0_0_1px_var(--color-sky-500)]"),
                        )}
                      >
                        {isEditing ? (
                          <CellInput
                            initial={editCell.initial}
                            onStage={(v) => editing?.onEdit(ri, ci, v)}
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
        <ContextMenuItem disabled={!menuCellOk} onClick={copyCell}>
          <Copy size={13} className="text-zinc-500" />
          Copy cell
          {rectCount <= 1 && <ContextMenuShortcut>⌘C</ContextMenuShortcut>}
        </ContextMenuItem>
        {rectCount > 1 && (
          <ContextMenuItem onClick={copyCells}>
            <Sheet size={13} className="text-zinc-500" />
            Copy {rectCount} cells (TSV)
            <ContextMenuShortcut>⌘C</ContextMenuShortcut>
          </ContextMenuItem>
        )}
        <ContextMenuItem disabled={!n} onClick={() => copyRows(" ", "")}>
          <Copy size={13} className="text-zinc-500" />
          Copy {rowsLabel}
        </ContextMenuItem>
        <ContextMenuItem disabled={!n} onClick={() => copyRows("\t", "as TSV")}>
          <Sheet size={13} className="text-zinc-500" />
          Copy {rowsLabel} as TSV
        </ContextMenuItem>
        <ContextMenuItem disabled={!n} onClick={copyJson}>
          <Braces size={13} className="text-zinc-500" />
          Copy {rowsLabel} as JSON
        </ContextMenuItem>
        {insertTarget && (
          <ContextMenuItem disabled={!n} onClick={copyInsert}>
            <Database size={13} className="text-zinc-500" />
            Copy {rowsLabel} as INSERT
          </ContextMenuItem>
        )}
        <ContextMenuSeparator />
        <ContextMenuItem disabled={!result.rows.length} onClick={copyAll}>
          <Sheet size={13} className="text-zinc-500" />
          Copy all with header (TSV)
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          disabled={!menuCellOk}
          onClick={() => menuCell && openCellDialog(menuCell.row, menuCell.col)}
        >
          <SquarePen size={13} className="text-zinc-500" />
          Open cell in editor
          <ContextMenuShortcut>⌘⏎</ContextMenuShortcut>
        </ContextMenuItem>
        {editing && (
          <>
            <ContextMenuSeparator />
            <ContextMenuItem
              disabled={!canEdit || !menuCellOk}
              onClick={() => menuCell && startEdit(menuCell.row, menuCell.col)}
            >
              <Pencil size={13} className="text-zinc-500" />
              Edit cell
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!canEdit || !menuCellOk}
              onClick={() =>
                menuCell && editing.onEdit(menuCell.row, menuCell.col, null)
              }
            >
              <Eraser size={13} className="text-zinc-500" />
              Set cell NULL
            </ContextMenuItem>
            {menuCellStaged && (
              <ContextMenuItem
                onClick={() =>
                  menuCell &&
                  editing.onEdit(
                    menuCell.row,
                    menuCell.col,
                    result.rows[menuCell.row][menuCell.col],
                  )
                }
              >
                <Undo2 size={13} className="text-zinc-500" />
                Revert cell
              </ContextMenuItem>
            )}
            <ContextMenuItem
              disabled={!canEdit || !n}
              onClick={() => editing.onDuplicate(selRows)}
              title="Copies stay pending until Apply; generated keys are regenerated"
            >
              <CopyPlus size={13} className="text-zinc-500" />
              Duplicate {rowsLabel}
              <ContextMenuShortcut>⌘D</ContextMenuShortcut>
            </ContextMenuItem>
            <ContextMenuItem
              disabled={!canEdit || !n}
              onClick={() => editing.onToggleDelete(selRows, !allSelectedDeleted)}
            >
              {allSelectedDeleted ? (
                <>
                  <Undo2 size={13} className="text-zinc-500" />
                  Restore {rowsLabel}
                </>
              ) : (
                <>
                  <Trash2 size={13} className="text-red-400/80" />
                  Delete {rowsLabel}
                </>
              )}
              <ContextMenuShortcut>⌫</ContextMenuShortcut>
            </ContextMenuItem>
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
              <Button
                onClick={() =>
                  void copyText(dialog.text).then(
                    (ok) => ok && copied("Cell copied"),
                  )
                }
              >
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
