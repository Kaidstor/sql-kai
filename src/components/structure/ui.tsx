// Building blocks shared by the Structure-tab sections: section tables and the
// inline-edit cells.
import { Check, X } from "lucide-react";
import { useState, type ReactNode } from "react";
import { cn, IconButton } from "../ui";

/** Double-click-to-edit cell: Enter commits, Esc/blur cancels. */
export function EditableCell({
  value,
  placeholder,
  className,
  title = "Double-click to edit · Enter stages",
  onCommit,
}: {
  value: string;
  placeholder?: string;
  className?: string;
  title?: string;
  onCommit: (next: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);

  // Both states are exactly h-6 with text starting 6px from the cell edge,
  // so toggling edit mode never shifts the row.
  if (!editing) {
    return (
      <div
        title={title}
        onDoubleClick={() => {
          setDraft(value);
          setEditing(true);
        }}
        className={cn(
          "h-6 leading-6 cursor-text truncate rounded px-1.5 -mx-1.5",
          "hover:bg-zinc-800/70 hover:ring-1 hover:ring-zinc-700",
          !value && "italic text-zinc-600",
          className,
        )}
      >
        {value || placeholder || "—"}
      </div>
    );
  }
  return (
    <input
      autoFocus
      spellCheck={false}
      autoCorrect="off"
      autoCapitalize="off"
      value={draft}
      onChange={(e) => setDraft(e.target.value)}
      onBlur={() => setEditing(false)}
      onKeyDown={(e) => {
        if (e.key === "Enter") {
          setEditing(false);
          if (draft !== value) onCommit(draft);
        }
        if (e.key === "Escape") setEditing(false);
      }}
      className={cn(
        "h-6 w-[calc(100%+0.75rem)] -mx-1.5 rounded border border-sky-600/70 bg-zinc-900",
        "px-[5px] font-mono text-[12px] text-zinc-100 focus:outline-none",
      )}
    />
  );
}

export function Th({
  children,
  className,
}: {
  children?: ReactNode;
  className?: string;
}) {
  return (
    <th
      className={cn(
        "px-3 py-2 text-left text-[11px] font-semibold tracking-wide text-zinc-500",
        className,
      )}
    >
      {children}
    </th>
  );
}

export function Td({
  children,
  className,
  colSpan,
}: {
  children?: ReactNode;
  className?: string;
  colSpan?: number;
}) {
  return (
    <td colSpan={colSpan} className={cn("px-3 py-1.5 align-middle", className)}>
      {children}
    </td>
  );
}

/** Full-width monospace table with the sticky header row shared by all
 *  structure sections. */
export function SectionTable({
  head,
  children,
}: {
  head: ReactNode;
  children: ReactNode;
}) {
  return (
    <table className="w-full text-[12px] font-mono">
      <thead className="sticky top-0 z-10 bg-zinc-950">
        <tr className="border-b border-zinc-800">{head}</tr>
      </thead>
      <tbody>{children}</tbody>
    </table>
  );
}

/** Zebra-striped body row. */
export function ZTr({
  index,
  className,
  children,
}: {
  index: number;
  className?: string;
  children: ReactNode;
}) {
  return (
    <tr
      className={cn(
        "border-b border-zinc-800/50",
        index % 2 === 1 && "bg-zinc-900/40",
        className,
      )}
    >
      {children}
    </tr>
  );
}

export function BoolMark({ value }: { value: boolean }) {
  return value ? (
    <Check size={14} className="text-emerald-400" />
  ) : (
    <span className="inline-block size-3.5 rounded border border-zinc-700" />
  );
}

/** Compact ✓/✗ pair for the inline "add …" rows. */
export function AddRowActions({
  canAdd,
  onAdd,
  onCancel,
}: {
  canAdd: boolean;
  onAdd: () => void;
  onCancel: () => void;
}) {
  return (
    <div className="flex items-center gap-0.5">
      <IconButton
        title="Create"
        disabled={!canAdd}
        onClick={onAdd}
        className="text-emerald-400"
      >
        <Check size={13} />
      </IconButton>
      <IconButton title="Cancel" onClick={onCancel}>
        <X size={13} />
      </IconButton>
    </div>
  );
}
