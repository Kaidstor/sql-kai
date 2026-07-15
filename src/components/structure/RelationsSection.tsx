// Structure → Relations: foreign keys — list, rename, drop, inline create
// with a searchable referenced-table picker.
import { X } from "lucide-react";
import { useMemo, useState } from "react";
import {
  createFkSql,
  dropConstraintSql,
  FK_ACTIONS,
  renameConstraintSql,
} from "../../lib/ddl";
import { parseRegclass } from "../../lib/sql";
import { useApp, type StructureTabState, type Tab } from "../../lib/store";
import { Combobox } from "../Combobox";
import { IconButton, Input, Select } from "../ui";
import {
  AddRowActions,
  EditableCell,
  SectionTable,
  Td,
  Th,
  useStructureDdl,
  ZTr,
} from "./shared";

export function RelationsSection({
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
  const tables = useApp((s) => s.tables[tab.profileId]);

  const [name, setName] = useState("");
  const [columns, setColumns] = useState("");
  const [refTable, setRefTable] = useState("");
  const [refColumns, setRefColumns] = useState("");
  const [onUpdate, setOnUpdate] = useState("NO ACTION");
  const [onDelete, setOnDelete] = useState("NO ACTION");

  const tableOptions = useMemo(
    () =>
      (tables ?? [])
        .filter((t) => t.kind === "table")
        .map((t) => ({
          value: t.schema === "public" ? t.name : `${t.schema}.${t.name}`,
          hint: t.schema === "public" ? undefined : t.schema,
        })),
    [tables],
  );

  const create = async () => {
    const target = parseRegclass(refTable);
    const sql = createFkSql(state.schema, state.table, {
      name,
      columns,
      refSchema: target.schema,
      refTable: target.table,
      refColumns,
      onUpdate,
      onDelete,
    });
    if (await run(sql)) {
      onCloseAdd();
      setName("");
      setColumns("");
      setRefTable("");
      setRefColumns("");
      setOnUpdate("NO ACTION");
      setOnDelete("NO ACTION");
    }
  };

  return (
    <SectionTable
      head={
        <>
          <Th className="w-[24%]">Name</Th>
          <Th>Columns</Th>
          <Th>References</Th>
          <Th>Ref. columns</Th>
          <Th className="w-28">On update</Th>
          <Th className="w-28">On delete</Th>
          <Th className="w-16" />
        </>
      }
    >
      {state.relations?.map((r, i) => (
        <ZTr key={r.name} index={i}>
          <Td className="text-zinc-100">
            <EditableCell
              value={r.name}
              title="Double-click to rename · Enter runs ALTER TABLE … RENAME CONSTRAINT"
              onCommit={(v) =>
                v.trim() &&
                void run(
                  renameConstraintSql(state.schema, state.table, r.name, v.trim()),
                )
              }
            />
          </Td>
          <Td className="text-zinc-400">{r.columns}</Td>
          <Td className="text-sky-400">{r.refTable}</Td>
          <Td className="text-zinc-400">{r.refColumns}</Td>
          <Td className="text-zinc-500">{r.onUpdate}</Td>
          <Td className="text-zinc-500">{r.onDelete}</Td>
          <Td>
            <IconButton
              title={`Drop foreign key ${r.name}`}
              onClick={() =>
                void run(dropConstraintSql(state.schema, state.table, r.name), {
                  title: `Drop foreign key "${r.name}"?`,
                  danger: true,
                })
              }
            >
              <X size={13} />
            </IconButton>
          </Td>
        </ZTr>
      ))}
      {adding && (
        <tr className="bg-sky-950/20">
          <Td>
            <Input
              autoFocus
              placeholder="constraint name (auto)"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="py-0.5 font-mono text-[12px]"
            />
          </Td>
          <Td>
            <Input
              placeholder="column(s)"
              value={columns}
              onChange={(e) => setColumns(e.target.value)}
              className="py-0.5 font-mono text-[12px]"
            />
          </Td>
          <Td>
            <Combobox
              value={refTable}
              options={tableOptions}
              placeholder="Search table…"
              emptyLabel="table"
              onSelect={setRefTable}
            />
          </Td>
          <Td>
            <Input
              placeholder="(primary key)"
              value={refColumns}
              onChange={(e) => setRefColumns(e.target.value)}
              className="py-0.5 font-mono text-[12px]"
            />
          </Td>
          <Td>
            <Select value={onUpdate} onChange={(e) => setOnUpdate(e.target.value)}>
              {FK_ACTIONS.map((a) => (
                <option key={a} value={a}>
                  {a}
                </option>
              ))}
            </Select>
          </Td>
          <Td>
            <Select value={onDelete} onChange={(e) => setOnDelete(e.target.value)}>
              {FK_ACTIONS.map((a) => (
                <option key={a} value={a}>
                  {a}
                </option>
              ))}
            </Select>
          </Td>
          <Td>
            <AddRowActions
              canAdd={Boolean(columns.trim() && refTable)}
              onAdd={() => void create()}
              onCancel={onCloseAdd}
            />
          </Td>
        </tr>
      )}
    </SectionTable>
  );
}
