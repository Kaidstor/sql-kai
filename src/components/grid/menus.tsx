// The grid's two context menus: ColumnMenu opens on a header, CellMenu on a
// cell/row. Both render just the ContextMenuContent children — the trigger
// and open-state live in ResultsGrid.
import {
  ArrowDown,
  ArrowUp,
  ArrowUpDown,
  ArrowUpRight,
  Braces,
  Brackets,
  Copy,
  CopyPlus,
  Database,
  Eraser,
  EyeOff,
  FileDown,
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
import type { SortSpec, StatementResult } from "../../lib/types";
import {
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
} from "../context-menu";
import type { CopyActions } from "./copyActions";
import type { CellRef, GridEditing } from "./types";
import type { useColumnLayout } from "./useColumnLayout";

export function ColumnMenu({
  menuCol,
  result,
  sorts,
  onSortsChange,
  applySort,
  sortIdxOf,
  selColList,
  copy,
  layout,
  onHideColumns,
}: {
  menuCol: number;
  result: StatementResult;
  sorts?: readonly SortSpec[];
  onSortsChange?: (sorts: SortSpec[]) => void;
  applySort: (name: string, dir?: "asc" | "desc", add?: boolean) => void;
  sortIdxOf: (name: string) => number;
  selColList: number[];
  copy: CopyActions;
  layout: ReturnType<typeof useColumnLayout>;
  /** Hides the columns and clears the column selection. */
  onHideColumns: (cols: number[]) => void;
}) {
  return (
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
          <ContextMenuItem icon={Sheet} onClick={copy.copyColumns}>
            Copy {selColList.length} columns (TSV)
            <ContextMenuShortcut>⌘C</ContextMenuShortcut>
          </ContextMenuItem>
          <ContextMenuItem
            icon={Copy}
            onClick={() =>
              copy.copyAndToast(
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
              copy.copyAndToast(result.columns[menuCol], "Column name copied")
            }
          >
            Copy column name
          </ContextMenuItem>
          <ContextMenuItem
            icon={Sheet}
            disabled={!result.rows.length}
            onClick={() => copy.copyColumnValues(menuCol)}
          >
            Copy column values
          </ContextMenuItem>
          <ContextMenuItem
            icon={Brackets}
            disabled={!result.rows.length}
            onClick={() => copy.copyColumnIn(menuCol)}
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
            layout.fitColumn,
          )
        }
      >
        Fit column width{selColList.length > 1 ? ` (${selColList.length})` : ""}
      </ContextMenuItem>
      <ContextMenuItem icon={UnfoldHorizontal} onClick={layout.fitAllColumns}>
        Fit all columns
      </ContextMenuItem>
      <ContextMenuItem
        icon={MoveHorizontal}
        onClick={() => layout.matchAllColumns(menuCol)}
      >
        Resize all to match
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem
        icon={EyeOff}
        onClick={() =>
          onHideColumns(selColList.length > 1 ? selColList : [menuCol])
        }
      >
        {selColList.length > 1
          ? `Hide ${selColList.length} columns`
          : `Hide ${result.columns[menuCol]}`}
      </ContextMenuItem>
      <ContextMenuItem icon={RotateCcw} onClick={layout.resetLayout}>
        Reset layout
      </ContextMenuItem>
    </>
  );
}

export function CellMenu({
  menuCell,
  menuCellStaged,
  result,
  rectCount,
  selRows,
  rowsLabel,
  allSelectedDeleted,
  insertTarget,
  fkColumns,
  onFollowFk,
  editing,
  canEdit,
  copy,
  openCellDialog,
  startEdit,
  hiddenCount,
  resetLayout,
}: {
  menuCell: CellRef | null;
  /** The menu cell has a staged edit — offers "Revert cell". */
  menuCellStaged: boolean;
  result: StatementResult;
  rectCount: number;
  selRows: number[];
  rowsLabel: string;
  allSelectedDeleted: boolean;
  insertTarget?: { schema: string; table: string };
  fkColumns?: ReadonlySet<number>;
  onFollowFk?: (row: number, col: number) => void;
  editing?: GridEditing;
  canEdit: boolean;
  copy: CopyActions;
  openCellDialog: (ri: number, ci: number) => void;
  startEdit: (ri: number, ci: number) => void;
  hiddenCount: number;
  resetLayout: () => void;
}) {
  const n = selRows.length;
  const menuCellOk = Boolean(menuCell && menuCell.col >= 0);
  const copyCell = () => {
    if (!menuCell || menuCell.col < 0) return;
    copy.copyCellAt(menuCell.row, menuCell.col);
  };
  return (
    <>
      <ContextMenuItem icon={Copy} disabled={!menuCellOk} onClick={copyCell}>
        Copy cell
        {rectCount <= 1 && <ContextMenuShortcut>⌘C</ContextMenuShortcut>}
      </ContextMenuItem>
      {rectCount > 1 && (
        <ContextMenuItem icon={Sheet} onClick={copy.copyCells}>
          Copy {rectCount} cells (TSV)
          <ContextMenuShortcut>⌘C</ContextMenuShortcut>
        </ContextMenuItem>
      )}
      <ContextMenuItem icon={Copy} disabled={!n} onClick={() => copy.copyRows(" ", "")}>
        Copy {rowsLabel}
      </ContextMenuItem>
      <ContextMenuItem icon={Sheet} disabled={!n} onClick={() => copy.copyRows("\t", "as TSV")}>
        Copy {rowsLabel} as TSV
      </ContextMenuItem>
      <ContextMenuItem icon={Braces} disabled={!n} onClick={copy.copyJson}>
        Copy {rowsLabel} as JSON
      </ContextMenuItem>
      {insertTarget && (
        <ContextMenuItem icon={Database} disabled={!n} onClick={copy.copyInsert}>
          Copy {rowsLabel} as INSERT
        </ContextMenuItem>
      )}
      <ContextMenuSeparator />
      <ContextMenuItem icon={Sheet} disabled={!result.rows.length} onClick={copy.copyAll}>
        Copy all with header (TSV)
      </ContextMenuItem>
      <ContextMenuItem
        icon={Braces}
        disabled={!result.rows.length}
        onClick={copy.copyMarkdown}
      >
        Copy {copy.exportLabel} as Markdown
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem
        icon={FileDown}
        disabled={!result.rows.length}
        onClick={() => void copy.exportRows("csv")}
      >
        Export {copy.exportLabel} as CSV…
      </ContextMenuItem>
      <ContextMenuItem
        icon={FileDown}
        disabled={!result.rows.length}
        onClick={() => void copy.exportRows("json")}
      >
        Export {copy.exportLabel} as JSON…
      </ContextMenuItem>
      <ContextMenuItem
        icon={FileDown}
        disabled={!result.rows.length}
        onClick={() => void copy.exportRows("md")}
      >
        Export {copy.exportLabel} as Markdown…
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
      {onFollowFk && menuCell && fkColumns?.has(menuCell.col) && (
        <ContextMenuItem
          icon={ArrowUpRight}
          disabled={result.rows[menuCell.row]?.[menuCell.col] == null}
          onClick={() => onFollowFk(menuCell.row, menuCell.col)}
        >
          Open referenced row
          <ContextMenuShortcut>⌘click</ContextMenuShortcut>
        </ContextMenuItem>
      )}
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
      {hiddenCount > 0 && (
        <>
          <ContextMenuSeparator />
          <ContextMenuItem icon={RotateCcw} onClick={resetLayout}>
            Reset layout ({hiddenCount} hidden)
          </ContextMenuItem>
        </>
      )}
    </>
  );
}
