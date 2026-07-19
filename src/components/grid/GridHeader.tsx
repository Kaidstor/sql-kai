// The grid's frozen header: column titles, sort buttons, resize handles and
// whole-column selection. Lives outside the body scroller (see ResultsGrid),
// which slides it horizontally with a transform.
import { ArrowDown, ArrowUp, ArrowUpDown } from "lucide-react";
import type { CSSProperties, ReactNode } from "react";
import type { SortSpec, StatementResult } from "../../lib/types";
import { cn } from "../ui";
import type { useColumnLayout } from "./useColumnLayout";
import type { useGridSelection } from "./useGridSelection";

interface Props {
  result: StatementResult;
  layout: ReturnType<typeof useColumnLayout>;
  sel: ReturnType<typeof useGridSelection>;
  /** Active ORDER BY entries in priority order. */
  sorts?: readonly SortSpec[];
  /** Present = sortable (shows the sort buttons). */
  onSortsChange?: (sorts: SortSpec[]) => void;
  applySort: (name: string, dir?: "asc" | "desc", add?: boolean) => void;
  sortIdxOf: (name: string) => number;
  tableStyle?: CSSProperties;
  tableClass: string;
  colgroup: ReactNode;
  /** Context-menu retarget: a column index opens the column menu, null (the
   *  corner cell) closes both menus. Selection retargeting stays local. */
  onMenuTarget: (col: number | null) => void;
}

export function GridHeader({
  result,
  layout,
  sel,
  sorts,
  onSortsChange,
  applySort,
  sortIdxOf,
  tableStyle,
  tableClass,
  colgroup,
  onMenuTarget,
}: Props) {
  const { hiddenCols } = layout;
  return (
    <div className="shrink-0 overflow-hidden">
      <table ref={layout.tableRef} style={tableStyle} className={tableClass}>
        {colgroup}
        <thead>
        <tr>
          <th
            style={layout.sized ? { width: layout.colWidths[-1] } : undefined}
            onContextMenu={() => onMenuTarget(null)}
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
                  onMenuTarget(i);
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
  );
}
