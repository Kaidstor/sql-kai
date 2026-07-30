// Structure → Triggers: list with enable/disable, rename, drop, "open
// definition as SQL" and an inline create form.
import { FileCode2, X } from "lucide-react";
import { useState } from "react";
import {
  createTriggerSql,
  dropTriggerSql,
  renameTriggerSql,
  setTriggerEnabledSql,
  TRIGGER_EVENTS,
  TRIGGER_TIMINGS,
} from "../../lib/ddl";
import { useApp, type StructureTabState, type Tab } from "../../lib/store";
import { cn, IconButton, Input, Select } from "../ui";
import {
  AddRowActions,
  EditableCell,
  SectionTable,
  Td,
  Th,
  ZTr,
} from "./ui";
import { useStructureDdl } from "./useStructureDdl";

export function TriggersSection({
  tab,
  adding,
  onCloseAdd,
}: {
  tab: Tab;
  adding: boolean;
  onCloseAdd: () => void;
}) {
  const state = tab.state as StructureTabState;
  const run = useStructureDdl(tab.id);
  const openQueryTab = useApp((s) => s.openQueryTab);

  const [name, setName] = useState("");
  const [timing, setTiming] = useState("BEFORE");
  const [events, setEvents] = useState<string[]>([]);
  const [level, setLevel] = useState("ROW");
  const [fn, setFn] = useState("");

  const toggleEvent = (ev: string) =>
    setEvents((cur) =>
      cur.includes(ev) ? cur.filter((e) => e !== ev) : [...cur, ev],
    );

  const create = async () => {
    // Keep the canonical INSERT/UPDATE/DELETE/TRUNCATE order regardless of
    // the click order.
    const ordered = TRIGGER_EVENTS.filter((e) => events.includes(e));
    const sql = createTriggerSql(state.schema, state.table, {
      name,
      timing,
      events: ordered,
      level,
      fn,
    });
    if (await run(sql)) {
      onCloseAdd();
      setName("");
      setTiming("BEFORE");
      setEvents([]);
      setLevel("ROW");
      setFn("");
    }
  };

  return (
    <SectionTable
      head={
        <>
          <Th className="w-[22%]">Name</Th>
          <Th className="w-28">Timing</Th>
          <Th className="w-44">Events</Th>
          <Th className="w-16">Enabled</Th>
          <Th>Definition</Th>
          <Th className="w-16" />
        </>
      }
    >
      {state.triggers?.map((t, i) => (
        <ZTr key={t.name} index={i} className={t.enabled ? undefined : "opacity-60"}>
          <Td className="text-zinc-100">
            <EditableCell
              value={t.name}
              title="Double-click to rename · Enter runs ALTER TRIGGER … RENAME"
              onCommit={(v) =>
                v.trim() &&
                void run(
                  renameTriggerSql(state.schema, state.table, t.name, v.trim()),
                )
              }
            />
          </Td>
          <Td className="text-zinc-400">{t.timing}</Td>
          <Td className="text-zinc-400">{t.events}</Td>
          <Td className="text-center">
            <input
              type="checkbox"
              checked={t.enabled}
              title={t.enabled ? "Disable trigger" : "Enable trigger"}
              onChange={() =>
                void run(
                  setTriggerEnabledSql(
                    state.schema,
                    state.table,
                    t.name,
                    !t.enabled,
                  ),
                  {
                    title: `${t.enabled ? "Disable" : "Enable"} trigger "${t.name}"?`,
                    danger: t.enabled,
                    label: t.enabled ? "Disable" : "Enable",
                  },
                )
              }
              className="accent-sky-600"
            />
          </Td>
          <Td className="max-w-0 truncate text-zinc-500">
            <span title={t.definition}>{t.definition}</span>
          </Td>
          <Td>
            <div className="flex items-center gap-0.5">
              <IconButton
                title="Open definition as SQL (drop & recreate to edit)"
                onClick={() =>
                  openQueryTab(tab.profileId, `${t.definition};`, t.name)
                }
              >
                <FileCode2 size={13} />
              </IconButton>
              <IconButton
                title={`Drop trigger ${t.name}`}
                onClick={() =>
                  void run(dropTriggerSql(state.schema, state.table, t.name), {
                    title: `Drop trigger "${t.name}"?`,
                    danger: true,
                  })
                }
              >
                <X size={13} />
              </IconButton>
            </div>
          </Td>
        </ZTr>
      ))}
      {adding && (
        <tr className="bg-sky-950/20">
          <Td>
            <Input
              autoFocus
              placeholder="trigger_name"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="py-0.5 font-mono text-[12px]"
            />
          </Td>
          <Td>
            <Select value={timing} onChange={(e) => setTiming(e.target.value)}>
              {TRIGGER_TIMINGS.map((t) => (
                <option key={t} value={t}>
                  {t}
                </option>
              ))}
            </Select>
          </Td>
          <Td>
            <div className="flex flex-wrap gap-1">
              {TRIGGER_EVENTS.map((ev) => (
                <button
                  key={ev}
                  onClick={() => toggleEvent(ev)}
                  className={cn(
                    "rounded border px-1.5 py-0.5 text-[10px] transition-colors",
                    events.includes(ev)
                      ? "border-sky-600/60 bg-sky-600/20 text-sky-300"
                      : "border-zinc-700 text-zinc-500 hover:text-zinc-300",
                  )}
                >
                  {ev}
                </button>
              ))}
            </div>
          </Td>
          <Td />
          <Td>
            <div className="flex items-center gap-1.5">
              <Select value={level} onChange={(e) => setLevel(e.target.value)}>
                <option value="ROW">FOR EACH ROW</option>
                <option value="STATEMENT">FOR EACH STATEMENT</option>
              </Select>
              <Input
                placeholder="function_name()"
                value={fn}
                onChange={(e) => setFn(e.target.value)}
                onKeyDown={(e) =>
                  e.key === "Enter" &&
                  name.trim() &&
                  events.length > 0 &&
                  fn.trim() &&
                  void create()
                }
                className="py-0.5 font-mono text-[12px]"
              />
            </div>
          </Td>
          <Td>
            <AddRowActions
              canAdd={Boolean(name.trim() && events.length > 0 && fn.trim())}
              onAdd={() => void create()}
              onCancel={onCloseAdd}
            />
          </Td>
        </tr>
      )}
    </SectionTable>
  );
}
