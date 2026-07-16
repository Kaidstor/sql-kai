// Searchable dropdown (column pickers, ref-table pickers): a trigger pill
// opening a fuzzy-filtered list with keyboard navigation.
import { ChevronDown, Search } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { fuzzyScore } from "../lib/fuzzy";
import { cn, Popover } from "./ui";

export interface ComboOption {
  value: string;
  /** Dim right-aligned hint (e.g. the column's type). */
  hint?: string;
}

export function Combobox({
  value,
  options,
  onSelect,
  placeholder = "Search…",
  triggerClassName,
  emptyLabel = "—",
}: {
  value: string;
  options: ComboOption[];
  onSelect: (value: string) => void;
  placeholder?: string;
  triggerClassName?: string;
  /** Trigger text while no value is selected. */
  emptyLabel?: string;
}) {
  const [open, setOpen] = useState(false);
  const [query, setQuery] = useState("");
  const [active, setActive] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const filtered = query.trim()
    ? options
        .map((o) => ({
          o,
          score: fuzzyScore(query, `${o.value} ${o.hint ?? ""}`),
        }))
        .filter((x): x is { o: ComboOption; score: number } => x.score !== null)
        .sort((a, b) => b.score - a.score)
        .map((x) => x.o)
    : options;

  // Keep the highlighted row valid and visible as the query narrows the list.
  useEffect(() => {
    if (active >= filtered.length) setActive(0);
  }, [filtered.length, active]);
  useEffect(() => {
    listRef.current
      ?.querySelector(`[data-idx="${active}"]`)
      ?.scrollIntoView({ block: "nearest" });
  }, [active]);

  const close = () => {
    setOpen(false);
    setQuery("");
    setActive(0);
  };
  const pick = (v: string) => {
    close();
    if (v !== value) onSelect(v);
  };

  return (
    <Popover
      open={open}
      onClose={close}
      panelClassName="w-84"
      trigger={
        <button
          onClick={() => (open ? close() : setOpen(true))}
          className={cn(
            "flex items-center gap-1.5 rounded-md bg-zinc-800/80 px-2 py-1",
            "font-mono text-[12px] text-zinc-200 transition-colors hover:bg-zinc-700/80",
            !value && "text-zinc-600",
            triggerClassName,
          )}
        >
          <span className="truncate">{value || emptyLabel}</span>
          <ChevronDown size={11} className="shrink-0 text-zinc-500" />
        </button>
      }
    >
      <div className="flex items-center gap-1.5 border-b border-zinc-800 px-2.5 py-1.5">
        <Search size={12} className="shrink-0 text-zinc-600" />
        <input
          autoFocus
          value={query}
          onChange={(e) => {
            setQuery(e.target.value);
            setActive(0);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown") {
              e.preventDefault();
              setActive((a) => Math.min(a + 1, filtered.length - 1));
            } else if (e.key === "ArrowUp") {
              e.preventDefault();
              setActive((a) => Math.max(a - 1, 0));
            } else if (e.key === "Enter" && filtered[active]) {
              pick(filtered[active].value);
            }
          }}
          placeholder={placeholder}
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          className="min-w-0 flex-1 bg-transparent text-[12px] text-zinc-100 outline-none placeholder:text-zinc-600"
        />
      </div>
      <div ref={listRef} className="max-h-56 overflow-y-auto py-1">
        {filtered.length === 0 && (
          <div className="px-2.5 py-1.5 text-[12px] text-zinc-600">
            no matches
          </div>
        )}
        {filtered.map((o, i) => (
          <button
            key={o.value}
            data-idx={i}
            onClick={() => pick(o.value)}
            onMouseMove={() => setActive(i)}
            className={cn(
              "flex w-full items-center gap-2 px-2.5 py-1.5 text-left font-mono text-[12px]",
              i === active ? "bg-zinc-800 text-zinc-100" : "text-zinc-300",
              o.value === value && "text-sky-400",
            )}
          >
            <span className="min-w-0 flex-1 truncate">{o.value}</span>
            {o.hint && (
              <span className="shrink-0 text-[10px] text-zinc-600">{o.hint}</span>
            )}
          </button>
        ))}
      </div>
    </Popover>
  );
}
