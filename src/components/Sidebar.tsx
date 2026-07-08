import {
  Activity,
  ChevronDown,
  ChevronRight,
  Copy,
  Eye,
  FileCode2,
  FilePlus2,
  KeyRound,
  LayoutGrid,
  Layers,
  Loader2,
  RefreshCw,
  Table2,
  Wrench,
} from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import { api, errText } from "../lib/api";
import { copyText } from "../lib/clipboard";
import { columnsKey, useApp } from "../lib/store";
import type { TableInfo } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "./context-menu";
import { ReconnectButton } from "./ReconnectButton";
import { CliBadge, cn, IconBtn, Input, ProdBadge } from "./ui";

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

/** Workspace sidebar: the active connection's schema tree. The connection
 *  list itself lives on the Launcher ("All connections" in the header). */
export function Sidebar() {
  const {
    profiles,
    sessions,
    cliSessions,
    tables,
    lost,
    connecting,
    activeProfileId,
    openQueryTab,
    openActivityTab,
    refreshTables,
    setLauncherOpen,
  } = useApp();
  const profile = profiles.find((p) => p.id === activeProfileId);
  const cliSession = activeProfileId ? cliSessions[activeProfileId] : undefined;
  const activeSession = activeProfileId ? sessions[activeProfileId] : undefined;
  // A lost session keeps its cached schema on screen (with a Reconnect
  // banner) — the tree only disappears on an explicit disconnect.
  const activeLost = Boolean(
    activeProfileId && !activeSession && lost[activeProfileId],
  );
  const reconnecting = Boolean(activeProfileId && connecting[activeProfileId]);

  if (!activeProfileId || !(activeSession || activeLost)) return null;

  return (
    <aside className="w-64 shrink-0 border-r border-zinc-800 flex flex-col min-h-0 bg-zinc-925">
      {/* top strip doubles as the window drag area (overlay titlebar) */}
      <div
        data-tauri-drag-region
        className="flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 pl-24 pr-3"
      >
        <div
          data-tauri-drag-region
          className="flex min-w-0 items-center gap-2"
        >
          <span
            title={activeLost ? "Connection lost" : undefined}
            className={cn(
              "size-2 shrink-0 rounded-full",
              activeSession
                ? "bg-emerald-500"
                : reconnecting
                  ? "bg-amber-400"
                  : "bg-red-500",
            )}
          />
          <span className="truncate text-[12px] text-zinc-200">
            {profile?.name ?? "?"}
          </span>
          {profile?.production && <ProdBadge />}
          {cliSession && <CliBadge idleSec={cliSession.idleSec} />}
        </div>
        <IconBtn
          title="All connections"
          onClick={() => setLauncherOpen(true)}
        >
          <LayoutGrid size={14} />
        </IconBtn>
      </div>

      <div className="flex items-center justify-between pl-3 pr-4 pt-2 pb-1.5">
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
            title="Server activity (pg_stat_activity)"
            onClick={() => openActivityTab(activeProfileId)}
          >
            <Activity size={13} />
          </IconBtn>
          <IconBtn
            title="Refresh tables"
            onClick={() => void refreshTables(activeProfileId)}
          >
            <RefreshCw size={12} />
          </IconBtn>
        </div>
      </div>
      {activeLost && (
        <ReconnectButton
          profileId={activeProfileId}
          iconSize={11}
          iconClassName="shrink-0"
          label="Connection lost — reconnect"
          className="mx-2 mb-1.5 shrink-0 rounded-md border border-red-900/60 bg-red-950/40 px-2 py-1.5 text-[11px] text-red-300 hover:bg-red-950/70 disabled:opacity-60"
        />
      )}
      <SchemaTree
        profileId={activeProfileId}
        tables={tables[activeProfileId] ?? []}
      />
    </aside>
  );
}
