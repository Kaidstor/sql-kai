// Structure → Indexes: list, rename (double-click), drop, inline create.
import { X } from "lucide-react";
import { useState } from "react";
import { createIndexSql, INDEX_METHODS, renameIndexSql } from "../../lib/ddl";
import { relIdent } from "../../lib/sql";
import type { StructureTabState, Tab } from "../../lib/store";
import { IconButton, Input, Select } from "../ui";
import {
  AddRowActions,
  BoolMark,
  EditableCell,
  SectionTable,
  Td,
  Th,
  ZTr,
} from "./ui";
import { useStructureDdl } from "./useStructureDdl";

export function IndexesSection({
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

  const [name, setName] = useState("");
  const [columns, setColumns] = useState("");
  const [unique, setUnique] = useState(false);
  const [method, setMethod] = useState("btree");

  const create = async () => {
    const sql = createIndexSql(state.schema, state.table, {
      name,
      columns,
      unique,
      method,
    });
    if (await run(sql)) {
      onCloseAdd();
      setName("");
      setColumns("");
      setUnique(false);
      setMethod("btree");
    }
  };

  return (
    <SectionTable
      head={
        <>
          <Th className="w-[30%]">Name</Th>
          <Th className="w-16">Unique</Th>
          <Th className="w-16">Primary</Th>
          <Th>Columns</Th>
          <Th className="w-16" />
        </>
      }
    >
      {state.indexes?.map((idx, i) => (
        <ZTr key={idx.name} index={i}>
          <Td className="text-zinc-100">
            <EditableCell
              value={idx.name}
              title={`${idx.definition}\n\nDouble-click to rename · Enter runs ALTER INDEX`}
              onCommit={(v) =>
                v.trim() &&
                void run(renameIndexSql(state.schema, idx.name, v.trim()))
              }
            />
          </Td>
          <Td className="text-center">
            <BoolMark value={idx.unique} />
          </Td>
          <Td className="text-center">
            <BoolMark value={idx.primary} />
          </Td>
          <Td className="text-zinc-400">{idx.columns ?? "(expression)"}</Td>
          <Td>
            <IconButton
              title={
                idx.primary
                  ? "Primary key — drop via constraint"
                  : `Drop index ${idx.name}`
              }
              disabled={idx.primary}
              onClick={() =>
                void run(`DROP INDEX ${relIdent(state.schema, idx.name)}`, {
                  title: `Drop index "${idx.name}"?`,
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
              placeholder="index name (auto)"
              value={name}
              onChange={(e) => setName(e.target.value)}
              className="py-0.5 font-mono text-[12px]"
            />
          </Td>
          <Td className="text-center">
            <input
              type="checkbox"
              checked={unique}
              onChange={(e) => setUnique(e.target.checked)}
              className="accent-sky-600"
            />
          </Td>
          <Td />
          <Td>
            <div className="flex items-center gap-1.5">
              <Select value={method} onChange={(e) => setMethod(e.target.value)}>
                {INDEX_METHODS.map((m) => (
                  <option key={m} value={m}>
                    {m}
                  </option>
                ))}
              </Select>
              <Input
                placeholder="col1, col2 or (lower(email))"
                value={columns}
                onChange={(e) => setColumns(e.target.value)}
                onKeyDown={(e) => e.key === "Enter" && columns.trim() && void create()}
                className="py-0.5 font-mono text-[12px]"
              />
            </div>
          </Td>
          <Td>
            <AddRowActions
              canAdd={Boolean(columns.trim())}
              onAdd={() => void create()}
              onCancel={onCloseAdd}
            />
          </Td>
        </tr>
      )}
    </SectionTable>
  );
}
