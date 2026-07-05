import { Check, Plus, Undo2, X } from "lucide-react";
import { useEffect, useState, type ReactNode } from "react";
import {
  useApp,
  type NewColumn,
  type StructureSection,
  type StructureTabState,
  type Tab,
} from "../lib/store";
import { quoteIdent as qi } from "../lib/sql";
import {
  Button,
  ErrorPre,
  IconBtn,
  Input,
  PendingChangesBar,
  RefreshBtn,
  cn,
} from "./ui";

const SECTIONS: { key: StructureSection; label: string }[] = [
  { key: "columns", label: "Columns" },
  { key: "indexes", label: "Indexes" },
  { key: "relations", label: "Relations" },
  { key: "triggers", label: "Triggers" },
];

/** Double-click-to-edit cell: Enter stages the change, Esc/blur cancels. */
function EditableCell({
  value,
  placeholder,
  className,
  onCommit,
}: {
  value: string;
  placeholder?: string;
  className?: string;
  onCommit: (next: string) => void;
}) {
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(value);

  // Both states are exactly h-6 with text starting 6px from the cell edge,
  // so toggling edit mode never shifts the row.
  if (!editing) {
    return (
      <div
        title="Double-click to edit · Enter stages"
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

function Th({ children, className }: { children?: ReactNode; className?: string }) {
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

function Td({
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
 *  four structure sections. */
function SectionTable({ head, children }: { head: ReactNode; children: ReactNode }) {
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
function ZTr({
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

function BoolMark({ value }: { value: boolean }) {
  return value ? (
    <Check size={14} className="text-emerald-400" />
  ) : (
    <span className="inline-block size-3.5 rounded border border-zinc-700" />
  );
}

function AddColumnRow({
  onAdd,
  onCancel,
}: {
  onAdd: (c: NewColumn) => void;
  onCancel: () => void;
}) {
  const [name, setName] = useState("");
  const [type, setType] = useState("text");
  const [nullable, setNullable] = useState(true);
  const [def, setDef] = useState("");
  return (
    <tr className="bg-sky-950/20">
      <Td>
        <Input
          autoFocus
          placeholder="column_name"
          value={name}
          onChange={(e) => setName(e.target.value)}
          className="py-0.5 font-mono text-[12px]"
        />
      </Td>
      <Td>
        <Input
          placeholder="text / int8 / timestamptz…"
          value={type}
          onChange={(e) => setType(e.target.value)}
          className="py-0.5 font-mono text-[12px]"
        />
      </Td>
      <Td className="text-center">
        <input
          type="checkbox"
          checked={nullable}
          onChange={(e) => setNullable(e.target.checked)}
          className="accent-sky-600"
        />
      </Td>
      <Td>
        <Input
          placeholder="default expression (raw SQL)"
          value={def}
          onChange={(e) => setDef(e.target.value)}
          className="py-0.5 font-mono text-[12px]"
        />
      </Td>
      <Td colSpan={2}>
        <div className="flex items-center gap-1.5">
          <Button
            variant="primary"
            disabled={!name.trim() || !type.trim()}
            onClick={() => onAdd({ name: name.trim(), type: type.trim(), nullable, def: def.trim() })}
          >
            Add
          </Button>
          <Button onClick={onCancel}>Cancel</Button>
        </div>
      </Td>
      <Td />
    </tr>
  );
}

export function StructureTab({ tab }: { tab: Tab }) {
  const state = tab.state as StructureTabState;
  const {
    sessions,
    setStructureSection,
    loadStructure,
    runDdl,
    stageColumnEdit,
    toggleColumnDrop,
    stageColumnAdd,
    unstageColumnAdd,
    discardStructureEdits,
    applyStructureEdits,
  } = useApp();
  const [adding, setAdding] = useState(false);
  const connected = Boolean(sessions[tab.profileId]);

  // Lazy load: restored/reopened tabs fetch when first shown, not in bulk at boot.
  useEffect(() => {
    if (connected && !state[state.section] && !state.loading && !state.error) {
      void loadStructure(tab.id);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected]);

  const ddl = (sql: string) => void runDdl(tab.id, sql);
  const dirty =
    Object.keys(state.colEdits).length +
    state.colDrops.length +
    state.colAdds.length;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-1 border-b border-zinc-800 px-2 py-1.5">
        <span className="mr-2 font-mono text-[12px] text-zinc-300">
          {state.schema}.{state.table}
        </span>
        {SECTIONS.map((s) => (
          <button
            key={s.key}
            onClick={() => setStructureSection(tab.id, s.key)}
            className={cn(
              "rounded-md px-2.5 py-1 text-[12px] transition-colors",
              state.section === s.key
                ? "bg-zinc-800 text-zinc-100"
                : "text-zinc-500 hover:text-zinc-200",
            )}
          >
            {s.label}
          </button>
        ))}
        {state.section === "columns" && dirty > 0 && (
          <PendingChangesBar
            count={dirty}
            busy={state.loading}
            applyTitle="⌘S — runs all staged DDL in one transaction"
            onApply={() => void applyStructureEdits(tab.id)}
            onDiscard={() => discardStructureEdits(tab.id)}
          />
        )}
        <div className="ml-auto flex items-center gap-0.5">
          {state.section === "columns" && (
            <IconBtn title="Add column" onClick={() => setAdding(true)}>
              <Plus size={14} />
            </IconBtn>
          )}
          <RefreshBtn
            loading={state.loading}
            onClick={() => void loadStructure(tab.id)}
          />
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {state.error && <ErrorPre>{state.error}</ErrorPre>}

        {!state.error && state.section === "columns" && (
          <SectionTable
            head={
              <>
                <Th className="w-[22%]">Name</Th>
                <Th className="w-[18%]">Type</Th>
                <Th className="w-16 text-center!">Nullable</Th>
                <Th>Default</Th>
                <Th className="w-[18%]">Comment</Th>
                <Th className="w-14">Primary</Th>
                <Th className="w-10" />
              </>
            }
          >
              {state.columns?.map((col, i) => {
                const patch = state.colEdits[col.name] ?? {};
                const dropped = state.colDrops.includes(col.name);
                if (dropped) {
                  return (
                    <ZTr key={col.name} index={i} className="bg-red-950/30">
                      <Td className="text-red-400/70 line-through">{col.name}</Td>
                      <Td className="text-red-400/50 line-through">
                        {col.dataType}
                      </Td>
                      <Td className="text-center">
                        <input
                          type="checkbox"
                          checked={col.nullable}
                          readOnly
                          className="pointer-events-none accent-sky-600 opacity-40"
                        />
                      </Td>
                      <Td className="text-red-400/50 line-through">
                        {col.defaultExpr ?? ""}
                      </Td>
                      <Td className="text-red-400/50 line-through">
                        {col.comment ?? ""}
                      </Td>
                      <Td className="text-center">
                        <BoolMark value={col.isPk} />
                      </Td>
                      <Td>
                        <IconBtn
                          title="Restore column"
                          onClick={() => toggleColumnDrop(tab.id, col.name)}
                        >
                          <Undo2 size={13} />
                        </IconBtn>
                      </Td>
                    </ZTr>
                  );
                }
                const stagedCls = "bg-amber-500/15 text-amber-200";
                return (
                  <ZTr key={col.name} index={i}>
                    <Td>
                      <EditableCell
                        value={patch.name ?? col.name}
                        onCommit={(v) =>
                          v.trim() &&
                          stageColumnEdit(tab.id, col.name, { name: v.trim() })
                        }
                        className={cn(
                          "text-zinc-100",
                          patch.name !== undefined && stagedCls,
                        )}
                      />
                    </Td>
                    <Td>
                      <EditableCell
                        value={patch.type ?? col.dataType}
                        onCommit={(v) =>
                          v.trim() &&
                          stageColumnEdit(tab.id, col.name, { type: v.trim() })
                        }
                        className={cn(
                          "text-zinc-400",
                          patch.type !== undefined && stagedCls,
                        )}
                      />
                    </Td>
                    <Td className="text-center">
                      <input
                        type="checkbox"
                        checked={patch.nullable ?? col.nullable}
                        onChange={(e) =>
                          stageColumnEdit(tab.id, col.name, {
                            nullable: e.target.checked,
                          })
                        }
                        className={cn(
                          "accent-sky-600",
                          patch.nullable !== undefined && "accent-amber-500",
                        )}
                      />
                    </Td>
                    <Td>
                      <EditableCell
                        value={patch.default ?? col.defaultExpr ?? ""}
                        placeholder="(NULL)"
                        onCommit={(v) =>
                          stageColumnEdit(tab.id, col.name, { default: v.trim() })
                        }
                        className={cn(
                          "text-zinc-400",
                          patch.default !== undefined && stagedCls,
                        )}
                      />
                    </Td>
                    <Td>
                      <EditableCell
                        value={patch.comment ?? col.comment ?? ""}
                        placeholder="(NULL)"
                        onCommit={(v) =>
                          stageColumnEdit(tab.id, col.name, { comment: v.trim() })
                        }
                        className={cn(
                          "text-zinc-500",
                          patch.comment !== undefined && stagedCls,
                        )}
                      />
                    </Td>
                    <Td className="text-center">
                      <BoolMark value={col.isPk} />
                    </Td>
                    <Td>
                      <IconBtn
                        title={`Drop column ${col.name} (staged)`}
                        onClick={() => toggleColumnDrop(tab.id, col.name)}
                      >
                        <X size={13} />
                      </IconBtn>
                    </Td>
                  </ZTr>
                );
              })}
              {state.colAdds.map((a, i) => (
                <tr key={`+${i}`} className="bg-emerald-950/20">
                  <Td className="text-emerald-300">{a.name}</Td>
                  <Td className="text-emerald-300/70">{a.type}</Td>
                  <Td className="text-center">
                    <input
                      type="checkbox"
                      checked={a.nullable}
                      readOnly
                      className="pointer-events-none accent-emerald-500"
                    />
                  </Td>
                  <Td className="text-emerald-300/70">
                    {a.def || <span className="italic text-zinc-600">(NULL)</span>}
                  </Td>
                  <Td className="italic text-zinc-600">new column</Td>
                  <Td />
                  <Td>
                    <IconBtn
                      title="Remove pending column"
                      onClick={() => unstageColumnAdd(tab.id, i)}
                    >
                      <X size={13} />
                    </IconBtn>
                  </Td>
                </tr>
              ))}
            {adding && (
              <AddColumnRow
                onCancel={() => setAdding(false)}
                onAdd={(c) => {
                  setAdding(false);
                  stageColumnAdd(tab.id, c);
                }}
              />
            )}
          </SectionTable>
        )}

        {!state.error && state.section === "indexes" && (
          <SectionTable
            head={
              <>
                <Th className="w-[30%]">Name</Th>
                <Th className="w-16">Unique</Th>
                <Th className="w-16">Primary</Th>
                <Th>Columns</Th>
                <Th className="w-10" />
              </>
            }
          >
              {state.indexes?.map((idx, i) => (
                <ZTr key={idx.name} index={i}>
                  <Td className="text-zinc-100" >
                    <span title={idx.definition}>{idx.name}</span>
                  </Td>
                  <Td className="text-center">
                    <BoolMark value={idx.unique} />
                  </Td>
                  <Td className="text-center">
                    <BoolMark value={idx.primary} />
                  </Td>
                  <Td className="text-zinc-400">{idx.columns ?? "(expression)"}</Td>
                  <Td>
                    <IconBtn
                      title={idx.primary ? "Primary key — drop via constraint" : `Drop index ${idx.name}`}
                      disabled={idx.primary}
                      onClick={() => {
                        if (confirm(`Drop index "${idx.name}"?`)) {
                          ddl(`DROP INDEX ${qi(state.schema)}.${qi(idx.name)}`);
                        }
                      }}
                    >
                      <X size={13} />
                    </IconBtn>
                  </Td>
                </ZTr>
              ))}
          </SectionTable>
        )}

        {!state.error && state.section === "relations" && (
          <SectionTable
            head={
              <>
                <Th className="w-[26%]">Name</Th>
                <Th>Columns</Th>
                <Th>References</Th>
                <Th>Ref. columns</Th>
                <Th className="w-28">On update</Th>
                <Th className="w-28">On delete</Th>
              </>
            }
          >
            {state.relations?.map((r, i) => (
                <ZTr key={r.name} index={i}>
                  <Td className="text-zinc-100">{r.name}</Td>
                  <Td className="text-zinc-400">{r.columns}</Td>
                  <Td className="text-sky-400">{r.refTable}</Td>
                  <Td className="text-zinc-400">{r.refColumns}</Td>
                  <Td className="text-zinc-500">{r.onUpdate}</Td>
                  <Td className="text-zinc-500">{r.onDelete}</Td>
                </ZTr>
              ))}
          </SectionTable>
        )}

        {!state.error && state.section === "triggers" && (
          <SectionTable
            head={
              <>
                <Th className="w-[26%]">Name</Th>
                <Th className="w-28">Timing</Th>
                <Th className="w-44">Events</Th>
                <Th>Definition</Th>
              </>
            }
          >
            {state.triggers?.map((t, i) => (
                <ZTr key={t.name} index={i}>
                  <Td className="text-zinc-100">{t.name}</Td>
                  <Td className="text-zinc-400">{t.timing}</Td>
                  <Td className="text-zinc-400">{t.events}</Td>
                  <Td className="max-w-0 truncate text-zinc-500" >
                    <span title={t.definition}>{t.definition}</span>
                  </Td>
                </ZTr>
              ))}
          </SectionTable>
        )}

        {!state.error &&
          !state.loading &&
          state.section !== "columns" &&
          state[state.section]?.length === 0 && (
            <div className="flex h-32 items-center justify-center text-[13px] text-zinc-600">
              No {state.section}
            </div>
          )}
      </div>
    </div>
  );
}
