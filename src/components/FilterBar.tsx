// Structured WHERE builder of the table tab: rows of column / operator /
// value conditions compiled into the raw filter string (get_table_page
// consumes it as-is). Unparseable filters (typed SQL, complex FK filters)
// fall back to the raw-SQL input, toggleable via the braces button.
import { Braces, ChevronDown, Plus, X } from "lucide-react";
import { useEffect, useMemo, useRef, useState } from "react";
import {
  FILTER_OPS,
  compileFilters,
  enumLabelsFor,
  isTextType,
  opDef,
  parseFilter,
  type FilterCond,
  type FilterOp,
} from "../lib/filters";
import { fuzzyScore } from "../lib/fuzzy";
import { useApp } from "../lib/store";
import type { ColumnInfo } from "../lib/types";
import { Combobox } from "./Combobox";
import { cn, IconButton, Popover } from "./ui";

function OpSelect({
  value,
  onSelect,
}: {
  value: FilterOp;
  onSelect: (op: FilterOp) => void;
}) {
  const [open, setOpen] = useState(false);
  return (
    <Popover
      open={open}
      onClose={() => setOpen(false)}
      panelClassName="w-52 py-1"
      trigger={
        <button
          onClick={() => setOpen((v) => !v)}
          className={cn(
            "flex shrink-0 items-center gap-1.5 rounded-md bg-zinc-800/80 px-2 py-1",
            "text-[12px] text-zinc-200 transition-colors hover:bg-zinc-700/80",
          )}
        >
          {opDef(value).label}
          <ChevronDown size={11} className="shrink-0 text-zinc-500" />
        </button>
      }
    >
      {FILTER_OPS.map((d) => (
        <button
          key={d.op}
          onClick={() => {
            setOpen(false);
            if (d.op !== value) onSelect(d.op);
          }}
          className={cn(
            "flex w-full items-center gap-3 px-2.5 py-1.5 text-left text-[12px]",
            "hover:bg-zinc-800",
            d.op === value ? "text-sky-400" : "text-zinc-300",
          )}
        >
          <span className="min-w-0 flex-1">{d.label}</span>
          <span className="shrink-0 rounded bg-zinc-800 px-1.5 py-0.5 font-mono text-[10px] text-zinc-400">
            {d.sql}
          </span>
        </button>
      ))}
    </Popover>
  );
}

interface Suggestion {
  kind: "value" | "null" | "empty";
  label: string;
}

/** Filter-value input with a suggestion dropdown: enum labels of the column
 *  plus NULL / EMPTY STRING (see the parent for how picks are applied). */
function ValueCell({
  cond,
  colInfo,
  enumLabels,
  onDraft,
  onApply,
  onPickNull,
}: {
  cond: FilterCond;
  colInfo?: ColumnInfo;
  enumLabels: string[] | null;
  onDraft: (value: string) => void;
  /** Enter / suggestion pick. null = plain Enter on an empty input — apply
   *  the bar as-is (the row stays inactive). */
  onApply: (value: string | null) => void;
  /** The NULL suggestion — the parent switches the op to IS [NOT] NULL. */
  onPickNull: () => void;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const disabled = !opDef(cond.op).needsValue;
  const draft = cond.value ?? "";

  const suggestions = useMemo<Suggestion[]>(() => {
    if (disabled) return [];
    const out: Suggestion[] = [];
    for (const label of enumLabels ?? []) {
      if (!draft.trim() || fuzzyScore(draft, label) !== null) {
        out.push({ kind: "value", label });
      }
    }
    // NULL / EMPTY STRING make no sense inside an IN list
    if (cond.op !== "in") {
      if (colInfo?.nullable) out.push({ kind: "null", label: "NULL" });
      if (colInfo && isTextType(colInfo.dataType)) {
        out.push({ kind: "empty", label: "EMPTY STRING" });
      }
    }
    return out;
  }, [disabled, enumLabels, draft, cond.op, colInfo]);

  useEffect(() => {
    if (active >= suggestions.length) setActive(-1);
  }, [suggestions.length, active]);

  const pick = (s: Suggestion) => {
    setOpen(false);
    setActive(-1);
    if (s.kind === "null") {
      onPickNull();
    } else if (s.kind === "empty") {
      onApply("");
    } else if (cond.op === "in" && draft.trim()) {
      // IN accumulates a comma-separated list instead of replacing it
      onApply(`${draft.replace(/,\s*$/, "")}, ${s.label}`);
    } else {
      onApply(s.label);
    }
  };

  return (
    <Popover
      open={open && suggestions.length > 0}
      onClose={() => setOpen(false)}
      panelClassName="min-w-40 max-w-72 py-1"
      trigger={
        <input
          value={draft}
          disabled={disabled}
          onFocus={() => setOpen(true)}
          onChange={(e) => {
            onDraft(e.target.value);
            setOpen(true);
            setActive(-1);
          }}
          onKeyDown={(e) => {
            if (e.key === "ArrowDown" && suggestions.length > 0) {
              e.preventDefault();
              setOpen(true);
              setActive((a) => Math.min(a + 1, suggestions.length - 1));
            } else if (e.key === "ArrowUp" && open) {
              e.preventDefault();
              setActive((a) => Math.max(a - 1, -1));
            } else if (e.key === "Enter") {
              if (open && active >= 0 && suggestions[active]) {
                pick(suggestions[active]);
              } else {
                setOpen(false);
                onApply(cond.value);
              }
            } else if (e.key === "Escape" && open) {
              e.stopPropagation();
              setOpen(false);
              setActive(-1);
            }
          }}
          placeholder={
            disabled
              ? ""
              : cond.value === ""
                ? '"" (empty string)'
                : cond.op === "in"
                  ? "a, b, c…"
                  : "value…"
          }
          spellCheck={false}
          autoCorrect="off"
          autoCapitalize="off"
          className={cn(
            "w-44 rounded-md bg-zinc-800/80 px-2 py-1 font-mono text-[12px]",
            "text-zinc-100 outline-none placeholder:text-zinc-600",
            "focus:ring-1 focus:ring-sky-600/70",
            disabled && "opacity-30",
          )}
        />
      }
    >
      {suggestions.map((s, i) => (
        <button
          key={`${s.kind}:${s.label}`}
          onMouseDown={(e) => e.preventDefault()} // keep input focus
          onClick={() => pick(s)}
          onMouseMove={() => setActive(i)}
          className={cn(
            "flex w-full px-2.5 py-1.5 text-left font-mono text-[12px]",
            i === active ? "bg-zinc-800" : "",
            s.kind === "value" ? "text-zinc-200" : "text-zinc-400",
          )}
        >
          {s.label}
        </button>
      ))}
    </Popover>
  );
}

export function FilterBar({
  tabId,
  profileId,
  filter,
  columns,
  dataColumns,
  onHide,
}: {
  tabId: string;
  profileId: string;
  /** Applied raw WHERE from the tab state. */
  filter: string;
  columns?: ColumnInfo[];
  /** Page columns — fallback list until ColumnInfo loads. */
  dataColumns: string[];
  onHide: () => void;
}) {
  const refreshTablePage = useApp((s) => s.refreshTablePage);
  const loadSchemaEnums = useApp((s) => s.loadSchemaEnums);
  const enums = useApp((s) => s.schemaEnums[profileId]);
  const showToast = useApp((s) => s.showToast);

  useEffect(() => {
    void loadSchemaEnums(profileId);
  }, [profileId, loadSchemaEnums]);

  const defaultColumn = columns?.[0]?.name ?? dataColumns[0] ?? "";
  const blankRow = (): FilterCond => ({
    column: defaultColumn,
    op: "eq",
    value: null,
  });

  const [mode, setMode] = useState<"builder" | "raw">(() =>
    parseFilter(filter) ? "builder" : "raw",
  );
  const [rows, setRows] = useState<FilterCond[]>(() => {
    const parsed = parseFilter(filter);
    return parsed?.length ? parsed : [blankRow()];
  });
  const [rawDraft, setRawDraft] = useState(filter);
  // Applies from this bar must not resync (and drop not-yet-active rows);
  // external changes (FK navigation) must.
  const lastApplied = useRef(filter);

  useEffect(() => {
    if (filter === lastApplied.current) return;
    lastApplied.current = filter;
    const parsed = parseFilter(filter);
    if (parsed) {
      setMode("builder");
      setRows(parsed.length ? parsed : [blankRow()]);
    } else {
      setMode("raw");
    }
    setRawDraft(filter);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filter]);

  // The bar can open before column info arrives — backfill the blank row.
  useEffect(() => {
    if (!defaultColumn) return;
    setRows((rs) => rs.map((r) => (r.column ? r : { ...r, column: defaultColumn })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [defaultColumn]);

  const colByName = useMemo(
    () => new Map((columns ?? []).map((c) => [c.name, c])),
    [columns],
  );
  const colOptions = useMemo(
    () =>
      columns?.length
        ? columns.map((c) => ({ value: c.name, hint: c.dataType }))
        : dataColumns.map((name) => ({ value: name })),
    [columns, dataColumns],
  );

  const applyFilter = (compiled: string) => {
    lastApplied.current = compiled;
    if (compiled !== filter) {
      void refreshTablePage(tabId, { filter: compiled, page: 0 });
    }
  };
  const apply = (next: FilterCond[]) => {
    setRows(next);
    applyFilter(compileFilters(next));
  };
  const setRow = (i: number, patch: Partial<FilterCond>, run: boolean) => {
    const next = rows.map((r, j) => (j === i ? { ...r, ...patch } : r));
    if (run) apply(next);
    else setRows(next);
  };
  const removeRow = (i: number) => {
    const next = rows.filter((_, j) => j !== i);
    apply(next.length ? next : [blankRow()]);
  };

  const dirty =
    mode === "builder" ? compileFilters(rows) !== filter : rawDraft !== filter;

  const toggleMode = () => {
    if (mode === "builder") {
      setRawDraft(filter || compileFilters(rows));
      setMode("raw");
      return;
    }
    const parsed = parseFilter(filter);
    if (!parsed) {
      showToast("Filter is too complex for the builder — keep editing as SQL", "info");
      return;
    }
    setRows(parsed.length ? parsed : [blankRow()]);
    setMode("builder");
  };

  return (
    <div className="flex shrink-0 items-start gap-2 border-b border-zinc-800 px-2 py-1">
      {mode === "builder" ? (
        <div className="flex min-w-0 flex-1 flex-col gap-1">
          {rows.map((r, i) => (
            <div key={i} className="flex items-center gap-1.5">
              <IconButton title="Remove condition" onClick={() => removeRow(i)}>
                <X size={12} />
              </IconButton>
              <span className="w-10 shrink-0 text-right font-mono text-[11px] font-semibold text-zinc-500">
                {i === 0 ? "where" : "and"}
              </span>
              <Combobox
                value={r.column}
                options={colOptions}
                placeholder="Search column…"
                emptyLabel="column"
                onSelect={(column) => setRow(i, { column }, true)}
              />
              <OpSelect
                value={r.op}
                onSelect={(op) =>
                  setRow(
                    i,
                    opDef(op).needsValue ? { op } : { op, value: null },
                    true,
                  )
                }
              />
              <ValueCell
                cond={r}
                colInfo={colByName.get(r.column)}
                enumLabels={enumLabelsFor(
                  colByName.get(r.column)?.dataType ?? "",
                  enums,
                )}
                // erasing the value deactivates the row; explicit empty-string
                // filtering goes through the EMPTY STRING suggestion
                onDraft={(value) => setRow(i, { value: value || null }, false)}
                onApply={(value) => {
                  if (value === null) apply(rows); // empty Enter — flush drafts
                  else setRow(i, { value }, true);
                }}
                onPickNull={() => {
                  // negative ops flip to IS NOT NULL
                  const op =
                    r.op === "neq" || r.op === "nlike" ? "notnull" : "null";
                  setRow(i, { op, value: null }, true);
                }}
              />
              {i === rows.length - 1 && (
                <IconButton
                  title="Add condition (AND)"
                  onClick={() => setRows([...rows, blankRow()])}
                >
                  <Plus size={12} />
                </IconButton>
              )}
            </div>
          ))}
        </div>
      ) : (
        <div className="flex h-[26px] min-w-0 flex-1 items-center gap-2">
          <span className="shrink-0 font-mono text-[11px] font-semibold text-zinc-500">
            WHERE
          </span>
          <input
            autoFocus={!filter}
            value={rawDraft}
            onChange={(e) => setRawDraft(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                applyFilter(rawDraft.trim());
              } else if (e.key === "Escape") {
                setRawDraft(filter);
                if (!filter) onHide();
              }
            }}
            placeholder="status = 'active' AND created_at > now() - interval '1 day'"
            spellCheck={false}
            autoCorrect="off"
            autoCapitalize="off"
            className={cn(
              "min-w-0 flex-1 bg-transparent font-mono text-[12px] outline-none placeholder:text-zinc-700",
              rawDraft !== filter ? "text-amber-200" : "text-zinc-100",
            )}
          />
        </div>
      )}

      <div className="flex h-[26px] shrink-0 items-center gap-1">
        {dirty && (
          <span className="text-[10px] text-amber-400/80">⏎ apply</span>
        )}
        <IconButton
          title={mode === "builder" ? "Edit as raw SQL" : "Back to the builder"}
          className={mode === "raw" ? "text-amber-400" : undefined}
          onClick={toggleMode}
        >
          <Braces size={12} />
        </IconButton>
        {(filter || dirty) && (
          <IconButton
            title="Clear filter"
            onClick={() => {
              setRows([blankRow()]);
              setRawDraft("");
              applyFilter("");
            }}
          >
            <X size={12} />
          </IconButton>
        )}
      </div>
    </div>
  );
}
