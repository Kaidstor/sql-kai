import { X } from "lucide-react";
import type { StatementResult } from "../../lib/types";
import { cn } from "../ui";
import { CellInput } from "./CellInput";
import { oneLine } from "./text";
import type { GridEditing } from "./types";

/** Cell being edited in a pending INSERT row. */
export interface InsertEdit {
  index: number;
  col: number;
  initial: string;
}

/** One pending INSERT row (⌘D duplicate), rendered under its source row or at
 *  the bottom. Values are editable in place; ✕ on the gutter removes the row. */
export function InsertRowTr({
  editing,
  ii,
  result,
  hiddenCols,
  columnNullable,
  editIns,
  onStartEdit,
  onCloseEdit,
}: {
  editing: GridEditing;
  /** Index into editing.inserts. */
  ii: number;
  result: StatementResult;
  hiddenCols: ReadonlySet<number>;
  columnNullable?: (boolean | undefined)[];
  editIns: InsertEdit | null;
  onStartEdit: (index: number, col: number) => void;
  /** refocus=true hands focus back to the grid (Enter/Esc, not blur). */
  onCloseEdit: (refocus: boolean) => void;
}) {
  const row = editing.inserts[ii];
  return (
    <tr
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
            onDoubleClick={() => onStartEdit(ii, ci)}
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
                onClose={onCloseEdit}
              />
            ) : v === undefined ? (
              <span className="italic text-emerald-500/70">auto</span>
            ) : v === null ? (
              <span className="italic text-zinc-600">NULL</span>
            ) : (
              oneLine(v)
            )}
          </td>
        );
      })}
    </tr>
  );
}
