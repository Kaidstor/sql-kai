// Structure → Policies: RLS switch + policy list with inline edits (roles /
// USING / WITH CHECK via ALTER POLICY), rename, drop and an inline create
// form. Postgres can't clear USING/WITH CHECK via ALTER — those commits are
// rejected with a hint to recreate the policy.
import { Shield, ShieldOff, X } from "lucide-react";
import { useState } from "react";
import {
  alterPolicySql,
  createPolicySql,
  dropPolicySql,
  POLICY_COMMANDS,
  renamePolicySql,
  setRlsSql,
} from "../../lib/ddl";
import { useApp, type StructureTabState, type Tab } from "../../lib/store";
import { Button, IconButton, Input, Select } from "../ui";
import {
  AddRowActions,
  EditableCell,
  SectionTable,
  Td,
  Th,
  ZTr,
} from "./ui";
import { useStructureDdl } from "./useStructureDdl";

export function PoliciesSection({
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
  const showToast = useApp((s) => s.showToast);
  const pol = state.policies;

  const [name, setName] = useState("");
  const [command, setCommand] = useState("ALL");
  const [permissive, setPermissive] = useState(true);
  const [roles, setRoles] = useState("");
  const [usingExpr, setUsingExpr] = useState("");
  const [checkExpr, setCheckExpr] = useState("");

  const create = async () => {
    const sql = createPolicySql(state.schema, state.table, {
      name,
      command,
      permissive,
      roles,
      using: usingExpr,
      check: checkExpr,
    });
    if (await run(sql)) {
      onCloseAdd();
      setName("");
      setCommand("ALL");
      setPermissive(true);
      setRoles("");
      setUsingExpr("");
      setCheckExpr("");
    }
  };

  /** ALTER POLICY edit of one clause; empty exprs can't be applied. */
  const alter = (
    policy: string,
    patch: { roles?: string; using?: string; check?: string },
  ) => {
    const sql = alterPolicySql(state.schema, state.table, policy, patch);
    if (!sql) {
      showToast(
        "USING / WITH CHECK can't be cleared — drop & recreate the policy",
        "info",
      );
      return;
    }
    void run(sql);
  };

  return (
    <div>
      {pol && (
        <div className="flex items-center gap-2 border-b border-zinc-800/50 px-3 py-2 text-[12px]">
          {pol.rlsEnabled ? (
            <Shield size={13} className="text-emerald-400" />
          ) : (
            <ShieldOff size={13} className="text-zinc-600" />
          )}
          <span className={pol.rlsEnabled ? "text-zinc-200" : "text-zinc-500"}>
            Row-level security {pol.rlsEnabled ? "enabled" : "disabled"}
            {pol.rlsForced ? " · forced" : ""}
          </span>
          {pol.rlsEnabled && pol.policies.length === 0 && (
            <span className="text-[11px] text-amber-400/80">
              — no policies: the table is invisible to non-owners
            </span>
          )}
          <Button
            className="ml-auto"
            onClick={() =>
              void run(setRlsSql(state.schema, state.table, !pol.rlsEnabled), {
                title: `${pol.rlsEnabled ? "Disable" : "Enable"} row-level security on ${state.table}?`,
                danger: pol.rlsEnabled,
                label: pol.rlsEnabled ? "Disable" : "Enable",
              })
            }
          >
            {pol.rlsEnabled ? "Disable RLS" : "Enable RLS"}
          </Button>
        </div>
      )}
      <SectionTable
        head={
          <>
            <Th className="w-[18%]">Name</Th>
            <Th className="w-20">Command</Th>
            <Th className="w-24">Type</Th>
            <Th className="w-[14%]">Roles</Th>
            <Th>Using</Th>
            <Th>With check</Th>
            <Th className="w-16" />
          </>
        }
      >
        {pol?.policies.map((p, i) => (
          <ZTr key={p.name} index={i}>
            <Td className="text-zinc-100">
              <EditableCell
                value={p.name}
                title="Double-click to rename · Enter runs ALTER POLICY … RENAME"
                onCommit={(v) =>
                  v.trim() &&
                  void run(
                    renamePolicySql(state.schema, state.table, p.name, v.trim()),
                  )
                }
              />
            </Td>
            <Td className="text-zinc-400">{p.command}</Td>
            <Td className={p.permissive ? "text-zinc-500" : "text-amber-400/90"}>
              {p.permissive ? "permissive" : "restrictive"}
            </Td>
            <Td className="text-zinc-400">
              <EditableCell
                value={p.roles ?? ""}
                placeholder="PUBLIC"
                title="Double-click to edit · Enter runs ALTER POLICY … TO"
                onCommit={(v) => alter(p.name, { roles: v })}
              />
            </Td>
            <Td className="text-zinc-400">
              <EditableCell
                value={p.usingExpr ?? ""}
                title="Double-click to edit · Enter runs ALTER POLICY … USING"
                onCommit={(v) => alter(p.name, { using: v })}
              />
            </Td>
            <Td className="text-zinc-400">
              <EditableCell
                value={p.checkExpr ?? ""}
                title="Double-click to edit · Enter runs ALTER POLICY … WITH CHECK"
                onCommit={(v) => alter(p.name, { check: v })}
              />
            </Td>
            <Td>
              <IconButton
                title={`Drop policy ${p.name}`}
                onClick={() =>
                  void run(dropPolicySql(state.schema, state.table, p.name), {
                    title: `Drop policy "${p.name}"?`,
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
                placeholder="policy_name"
                value={name}
                onChange={(e) => setName(e.target.value)}
                className="py-0.5 font-mono text-[12px]"
              />
            </Td>
            <Td>
              <Select value={command} onChange={(e) => setCommand(e.target.value)}>
                {POLICY_COMMANDS.map((c) => (
                  <option key={c} value={c}>
                    {c}
                  </option>
                ))}
              </Select>
            </Td>
            <Td>
              <Select
                value={permissive ? "permissive" : "restrictive"}
                onChange={(e) => setPermissive(e.target.value === "permissive")}
              >
                <option value="permissive">permissive</option>
                <option value="restrictive">restrictive</option>
              </Select>
            </Td>
            <Td>
              <Input
                placeholder="PUBLIC"
                value={roles}
                onChange={(e) => setRoles(e.target.value)}
                className="py-0.5 font-mono text-[12px]"
              />
            </Td>
            <Td>
              <Input
                placeholder="USING expression"
                value={usingExpr}
                onChange={(e) => setUsingExpr(e.target.value)}
                className="py-0.5 font-mono text-[12px]"
              />
            </Td>
            <Td>
              <Input
                placeholder="WITH CHECK expression"
                value={checkExpr}
                onChange={(e) => setCheckExpr(e.target.value)}
                className="py-0.5 font-mono text-[12px]"
              />
            </Td>
            <Td>
              <AddRowActions
                canAdd={Boolean(name.trim())}
                onAdd={() => void create()}
                onCancel={onCloseAdd}
              />
            </Td>
          </tr>
        )}
      </SectionTable>
    </div>
  );
}
