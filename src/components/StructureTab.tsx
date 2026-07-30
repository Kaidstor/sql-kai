import { Plus, Undo2, X } from "lucide-react";
import { useState } from "react";
import { useLazyTabLoad } from "../hooks/useLazyTabLoad";
import {
  useApp,
  type NewColumn,
  type StructureSection,
  type StructureTabState,
  type Tab,
} from "../lib/store";
import { TabError } from "./TabError";
import { IndexesSection } from "./structure/IndexesSection";
import { PoliciesSection } from "./structure/PoliciesSection";
import { RelationsSection } from "./structure/RelationsSection";
import { TriggersSection } from "./structure/TriggersSection";
import {
  BoolMark,
  EditableCell,
  SectionTable,
  Td,
  Th,
  ZTr,
} from "./structure/ui";
import {
  Button,
  IconButton,
  Input,
  PendingChangesBar,
  RefreshButton,
  cn,
} from "./ui";

const SECTIONS: { key: StructureSection; label: string; add: string }[] = [
  { key: "columns", label: "Columns", add: "Add column" },
  { key: "indexes", label: "Indexes", add: "Add index" },
  { key: "relations", label: "Relations", add: "Add foreign key" },
  { key: "triggers", label: "Triggers", add: "Add trigger" },
  { key: "policies", label: "Policies", add: "Add policy" },
];

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
  const sessions = useApp((s) => s.sessions);
  const setStructureSection = useApp((s) => s.setStructureSection);
  const refreshStructure = useApp((s) => s.refreshStructure);
  const stageColumnEdit = useApp((s) => s.stageColumnEdit);
  const setColumnDropped = useApp((s) => s.setColumnDropped);
  const stageColumnAdd = useApp((s) => s.stageColumnAdd);
  const unstageColumnAdd = useApp((s) => s.unstageColumnAdd);
  const discardStructureEdits = useApp((s) => s.discardStructureEdits);
  const applyStructureEdits = useApp((s) => s.applyStructureEdits);
  const [adding, setAdding] = useState(false);
  const connected = Boolean(sessions[tab.profileId]);

  useLazyTabLoad(
    connected,
    Boolean(state[state.section] || state.loading || state.error),
    () => void refreshStructure(tab.id),
  );

  const dirty =
    Object.keys(state.colEdits).length +
    state.colDrops.length +
    state.colAdds.length;

  const sectionEmpty =
    state.section === "columns"
      ? false
      : state.section === "policies"
        ? state.policies?.policies.length === 0
        : state[state.section]?.length === 0;

  const sectionProps = { tab, adding, onCloseAdd: () => setAdding(false) };

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex shrink-0 items-center gap-1 border-b border-zinc-800 px-2 py-1.5">
        <span className="mr-2 font-mono text-[12px] text-zinc-300">
          {state.schema}.{state.table}
        </span>
        {SECTIONS.map((s) => (
          <button
            key={s.key}
            onClick={() => {
              setAdding(false);
              setStructureSection(tab.id, s.key);
            }}
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
            loading={state.loading}
            applyTitle="⌘S — runs all staged DDL in one transaction"
            onApply={() => void applyStructureEdits(tab.id)}
            onDiscard={() => discardStructureEdits(tab.id)}
          />
        )}
        <div className="ml-auto flex items-center gap-0.5">
          <IconButton
            title={SECTIONS.find((s) => s.key === state.section)?.add}
            onClick={() => setAdding(true)}
          >
            <Plus size={14} />
          </IconButton>
          <RefreshButton
            loading={state.loading}
            onClick={() => void refreshStructure(tab.id)}
          />
        </div>
      </div>

      <div className="min-h-0 flex-1 overflow-auto">
        {state.error && (
          <TabError profileId={tab.profileId} error={state.error} lost={state.connectionLost} />
        )}

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
                        <IconButton
                          title="Restore column"
                          onClick={() => setColumnDropped(tab.id, col.name, false)}
                        >
                          <Undo2 size={13} />
                        </IconButton>
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
                      <IconButton
                        title={`Drop column ${col.name} (staged)`}
                        onClick={() => setColumnDropped(tab.id, col.name, true)}
                      >
                        <X size={13} />
                      </IconButton>
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
                    <IconButton
                      title="Remove pending column"
                      onClick={() => unstageColumnAdd(tab.id, i)}
                    >
                      <X size={13} />
                    </IconButton>
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
          <IndexesSection {...sectionProps} />
        )}
        {!state.error && state.section === "relations" && (
          <RelationsSection {...sectionProps} />
        )}
        {!state.error && state.section === "triggers" && (
          <TriggersSection {...sectionProps} />
        )}
        {!state.error && state.section === "policies" && (
          <PoliciesSection {...sectionProps} />
        )}

        {!state.error && !state.loading && !adding && sectionEmpty && (
          <div className="flex h-32 items-center justify-center text-[13px] text-zinc-600">
            No {state.section}
          </div>
        )}
      </div>
    </div>
  );
}
