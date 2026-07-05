import {
  ChevronDown,
  ChevronRight,
  Copy,
  CopyPlus,
  Database,
  Eye,
  FileCode2,
  FilePlus2,
  GitFork,
  KeyRound,
  Layers,
  Loader2,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  Table2,
  Trash2,
  Unplug,
  Wrench,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { api, errText } from "../lib/api";
import { copyText } from "../lib/clipboard";
import { accentColor } from "../lib/colors";
import { profileAddr } from "../lib/profile";
import { columnsKey, useApp } from "../lib/store";
import type { Profile, TableInfo } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "./context-menu";
import { cn, IconBtn, Input } from "./ui";

function shortType(t: string): string {
  return t
    .replace("character varying", "varchar")
    .replace("timestamp with time zone", "timestamptz")
    .replace("timestamp without time zone", "timestamp")
    .replace("time with time zone", "timetz")
    .replace("time without time zone", "time")
    .replace("double precision", "float8")
    .replace("boolean", "bool")
    .replace("integer", "int4")
    .replace("bigint", "int8")
    .replace("smallint", "int2");
}

function TableIcon({ kind }: { kind: string }) {
  if (kind === "view" || kind === "matview")
    return <Eye size={12} className="text-violet-400 shrink-0" />;
  return <Table2 size={12} className="text-sky-500 shrink-0" />;
}

function ProfileRow({ profile }: { profile: Profile }) {
  const {
    sessions,
    connecting,
    activeProfileId,
    connect,
    disconnect,
    selectProfile,
    openDialog,
    deleteProfile,
    duplicateProfile,
  } = useApp();
  const connected = Boolean(sessions[profile.id]);
  const busy = Boolean(connecting[profile.id]);
  const active = activeProfileId === profile.id;
  const color = accentColor(profile.color);

  return (
    <ContextMenu>
    <ContextMenuTrigger className="block">
    <div
      className={cn(
        "group flex items-center gap-2 px-2 py-1.5 rounded-md cursor-pointer",
        active ? "bg-zinc-800" : "hover:bg-zinc-800/50",
      )}
      style={color ? { boxShadow: `inset 2px 0 0 ${color}` } : undefined}
      onClick={() => selectProfile(profile.id)}
      onDoubleClick={() => !connected && !busy && void connect(profile.id)}
    >
      <span
        className={cn(
          "size-2 rounded-full shrink-0",
          connected ? "bg-emerald-500" : busy ? "bg-amber-400" : "bg-zinc-600",
        )}
      />
      <div className="min-w-0 flex-1">
        <div className="text-[12px] text-zinc-200 truncate">{profile.name}</div>
        <div className="text-[10px] text-zinc-500 truncate">
          {profile.ssh?.host ? `${profile.ssh.host} ⇢ ` : ""}
          {profileAddr(profile)}
        </div>
      </div>
      <div className="hidden group-hover:flex items-center gap-0.5 shrink-0">
        {busy ? (
          <Loader2 size={13} className="animate-spin text-amber-400" />
        ) : connected ? (
          <IconBtn
            title="Disconnect"
            onClick={(e) => {
              e.stopPropagation();
              void disconnect(profile.id);
            }}
          >
            <Unplug size={13} />
          </IconBtn>
        ) : (
          <IconBtn
            title="Connect"
            onClick={(e) => {
              e.stopPropagation();
              void connect(profile.id);
            }}
          >
            <Plug size={13} />
          </IconBtn>
        )}
        <IconBtn
          title="Edit"
          onClick={(e) => {
            e.stopPropagation();
            openDialog(profile);
          }}
        >
          <Pencil size={13} />
        </IconBtn>
        <IconBtn
          title="Delete"
          onClick={(e) => {
            e.stopPropagation();
            if (confirm(`Delete connection "${profile.name}"?`)) {
              void deleteProfile(profile.id);
            }
          }}
        >
          <Trash2 size={13} />
        </IconBtn>
      </div>
    </div>
    </ContextMenuTrigger>
    <ContextMenuContent>
      {connected ? (
        <ContextMenuItem icon={Unplug} onClick={() => void disconnect(profile.id)}>
          Disconnect
        </ContextMenuItem>
      ) : (
        <ContextMenuItem
          icon={Plug}
          disabled={busy}
          onClick={() => void connect(profile.id)}
        >
          Connect
        </ContextMenuItem>
      )}
      <ContextMenuSeparator />
      <ContextMenuItem
        icon={CopyPlus}
        onClick={() => void duplicateProfile(profile.id)}
        title="Fork: the copy shares the group and its saved queries"
      >
        Duplicate
      </ContextMenuItem>
      <ContextMenuItem icon={Pencil} onClick={() => openDialog(profile)}>
        Edit…
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem
        icon={Trash2}
        iconClassName="text-red-400/80"
        onClick={() => {
          if (confirm(`Delete connection "${profile.name}"?`)) {
            void deleteProfile(profile.id);
          }
        }}
      >
        Delete
      </ContextMenuItem>
    </ContextMenuContent>
    </ContextMenu>
  );
}

function TableNode({ profileId, table }: { profileId: string; table: TableInfo }) {
  const {
    openTableTab,
    openStructureTab,
    loadTableColumns,
    tableColumns,
    sessions,
    showToast,
  } = useApp();
  const [expanded, setExpanded] = useState(false);
  const cols = tableColumns[columnsKey(profileId, table.schema, table.name)];

  const copyDdl = async () => {
    const session = sessions[profileId];
    if (!session) return;
    try {
      const ddl = await api.getTableDdl(
        session.sessionId,
        table.schema,
        table.name,
      );
      if (await copyText(ddl)) showToast("CREATE statement copied", "info");
    } catch (e) {
      showToast(errText(e));
    }
  };

  // Fetches on expand, and refetches after runDdl invalidates the cache.
  useEffect(() => {
    if (expanded && !cols) {
      void loadTableColumns(profileId, table.schema, table.name);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [expanded, cols]);

  const toggle = () => setExpanded((v) => !v);

  return (
    <div>
      <ContextMenu>
        <ContextMenuTrigger className="block">
          <div className="flex w-full items-center rounded pl-2 pr-2 hover:bg-zinc-800/60">
            <button
              className="shrink-0 p-0.5 text-zinc-600 hover:text-zinc-300"
              onClick={toggle}
            >
              {expanded ? <ChevronDown size={11} /> : <ChevronRight size={11} />}
            </button>
            <button
              className="flex min-w-0 flex-1 items-center gap-1.5 py-[3px] text-[12px] text-zinc-300"
              onClick={() => openTableTab(profileId, table.schema, table.name)}
              onDoubleClick={toggle}
              title={`${table.schema}.${table.name} (${table.kind})`}
            >
              <TableIcon kind={table.kind} />
              <span className="truncate">{table.name}</span>
            </button>
          </div>
        </ContextMenuTrigger>
        <ContextMenuContent>
          <ContextMenuItem
            icon={Table2}
            onClick={() => openTableTab(profileId, table.schema, table.name)}
          >
            Open data
          </ContextMenuItem>
          <ContextMenuItem
            icon={Wrench}
            onClick={() => openStructureTab(profileId, table.schema, table.name)}
          >
            Structure
          </ContextMenuItem>
          <ContextMenuSeparator />
          <ContextMenuItem
            icon={Copy}
            onClick={() => void copyText(`${table.schema}.${table.name}`)}
          >
            Copy name
          </ContextMenuItem>
          <ContextMenuItem icon={FileCode2} onClick={() => void copyDdl()}>
            Copy CREATE{" "}
            {table.kind === "view" || table.kind === "matview"
              ? "VIEW"
              : "TABLE"}
          </ContextMenuItem>
        </ContextMenuContent>
      </ContextMenu>

      {expanded && (
        <div className="pb-0.5">
          {!cols && (
            <div className="flex items-center gap-1.5 pl-9 py-0.5 text-[11px] text-zinc-600">
              <Loader2 size={10} className="animate-spin" /> loading…
            </div>
          )}
          {cols?.map((c) => (
            <div
              key={c.name}
              className="flex items-center gap-1 pl-9 pr-2 py-px text-[11px]"
              title={`${c.name} ${c.dataType}${c.nullable ? "" : " NOT NULL"}`}
            >
              {c.isPk && <KeyRound size={9} className="shrink-0 text-amber-400" />}
              <span className="truncate text-zinc-400">{c.name}</span>
              <span className="ml-auto shrink-0 pl-2 font-mono text-[10px] text-zinc-600">
                {shortType(c.dataType)}
              </span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function SchemaTree({ profileId, tables }: { profileId: string; tables: TableInfo[] }) {
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({});
  const [filter, setFilter] = useState("");

  const needle = filter.trim().toLowerCase();
  const grouped = useMemo(() => {
    const map = new Map<string, TableInfo[]>();
    for (const t of tables) {
      if (needle && !t.name.toLowerCase().includes(needle)) continue;
      const list = map.get(t.schema) ?? [];
      list.push(t);
      map.set(t.schema, list);
    }
    return [...map.entries()];
  }, [tables, needle]);

  return (
    <div className="flex flex-col min-h-0 flex-1">
      <div className="px-2 pb-1.5">
        <Input
          placeholder="Filter tables…"
          value={filter}
          onChange={(e) => setFilter(e.target.value)}
          className="py-0.5 text-[12px]"
        />
      </div>
      <div className="overflow-y-auto flex-1 px-1 pb-2">
        {grouped.map(([schema, items]) => {
          const isCollapsed = collapsed[schema] ?? false;
          return (
            <div key={schema}>
              <button
                className="w-full flex items-center gap-1 px-1 py-1 text-[11px] text-zinc-400 hover:text-zinc-200"
                onClick={() =>
                  setCollapsed((c) => ({ ...c, [schema]: !isCollapsed }))
                }
              >
                {isCollapsed ? (
                  <ChevronRight size={12} />
                ) : (
                  <ChevronDown size={12} />
                )}
                <Layers size={11} className="text-zinc-500" />
                <span className="font-medium">{schema}</span>
                <span className="text-zinc-600">({items.length})</span>
              </button>
              {!isCollapsed &&
                items.map((t) => (
                  <TableNode
                    key={`${t.schema}.${t.name}`}
                    profileId={profileId}
                    table={t}
                  />
                ))}
            </div>
          );
        })}
        {grouped.length === 0 && (
          <div className="px-2 py-2 text-[11px] text-zinc-600">no tables</div>
        )}
      </div>
    </div>
  );
}

type ProfileItem =
  | { kind: "solo"; profile: Profile }
  | { kind: "group"; name: string; profiles: Profile[] };

/** Grouped profiles collapse under one header (prod/test variations of one DB). */
function groupProfiles(profiles: Profile[]): ProfileItem[] {
  const items: ProfileItem[] = [];
  const groupIdx = new Map<string, number>();
  for (const p of profiles) {
    const g = p.group?.trim();
    if (!g) {
      items.push({ kind: "solo", profile: p });
      continue;
    }
    const i = groupIdx.get(g);
    if (i === undefined) {
      groupIdx.set(g, items.length);
      items.push({ kind: "group", name: g, profiles: [p] });
    } else {
      (items[i] as Extract<ProfileItem, { kind: "group" }>).profiles.push(p);
    }
  }
  return items;
}

export function Sidebar() {
  const {
    profiles,
    sessions,
    tables,
    activeProfileId,
    openDialog,
    openQueryTab,
    refreshTables,
  } = useApp();
  const activeSession = activeProfileId ? sessions[activeProfileId] : undefined;
  const [collapsedGroups, setCollapsedGroups] = useState<Record<string, boolean>>({});
  const items = useMemo(() => groupProfiles(profiles), [profiles]);

  return (
    <aside className="w-64 shrink-0 border-r border-zinc-800 flex flex-col min-h-0 bg-zinc-925">
      {/* top strip doubles as the window drag area (overlay titlebar) */}
      <div
        data-tauri-drag-region
        className="flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 pl-24 pr-4"
      >
        <div
          data-tauri-drag-region
          className="flex items-center gap-1.5 text-[11px] font-semibold tracking-wider text-zinc-500"
        >
          <Database size={12} /> CONNECTIONS
        </div>
        <IconBtn title="New connection" onClick={() => openDialog()}>
          <Plus size={14} />
        </IconBtn>
      </div>
      <div className="px-2 pt-2 space-y-0.5 max-h-[40%] overflow-y-auto">
        {items.map((item) =>
          item.kind === "solo" ? (
            <ProfileRow key={item.profile.id} profile={item.profile} />
          ) : (
            <div key={`group:${item.name}`}>
              <button
                className="flex w-full items-center gap-1 rounded px-1 py-1 text-[11px] text-zinc-400 hover:text-zinc-200"
                onClick={() =>
                  setCollapsedGroups((c) => ({
                    ...c,
                    [item.name]: !c[item.name],
                  }))
                }
              >
                {collapsedGroups[item.name] ? (
                  <ChevronRight size={12} />
                ) : (
                  <ChevronDown size={12} />
                )}
                <GitFork size={11} className="text-amber-500/80" />
                <span className="truncate font-medium">{item.name}</span>
                <span className="text-zinc-600">({item.profiles.length})</span>
              </button>
              {!collapsedGroups[item.name] && (
                <div className="ml-2.5 space-y-0.5 border-l border-zinc-800 pl-1.5">
                  {item.profiles.map((p) => (
                    <ProfileRow key={p.id} profile={p} />
                  ))}
                </div>
              )}
            </div>
          ),
        )}
        {profiles.length === 0 && (
          <button
            className="w-full px-2 py-3 text-[12px] text-zinc-500 hover:text-zinc-300 border border-dashed border-zinc-700 rounded-md"
            onClick={() => openDialog()}
          >
            + Add your first connection
          </button>
        )}
      </div>

      {activeProfileId && activeSession && (
        <>
          <div className="flex items-center justify-between pl-3 pr-4 pt-3 pb-1.5 border-t border-zinc-800 mt-2">
            <div className="text-[11px] font-semibold tracking-wider text-zinc-500">
              SCHEMA
            </div>
            <div className="flex items-center gap-0.5">
              <IconBtn
                title="New SQL tab"
                onClick={() => openQueryTab(activeProfileId)}
              >
                <FilePlus2 size={13} />
              </IconBtn>
              <IconBtn
                title="Refresh tables"
                onClick={() => void refreshTables(activeProfileId)}
              >
                <RefreshCw size={12} />
              </IconBtn>
            </div>
          </div>
          <SchemaTree
            profileId={activeProfileId}
            tables={tables[activeProfileId] ?? []}
          />
        </>
      )}
    </aside>
  );
}
