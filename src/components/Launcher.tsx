import {
  CopyPlus,
  Database,
  GitFork,
  Loader2,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  Trash2,
  Unplug,
  X,
} from "lucide-react";
import { Fragment, useEffect, useMemo, useState } from "react";
import { accentColor } from "../lib/colors";
import { profileAddr } from "../lib/profile";
import { useApp } from "../lib/store";
import type { Profile } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "./context-menu";
import { CliBadge, cn, IconBtn, Input, ProdBadge } from "./ui";

function ProfileCard({ profile }: { profile: Profile }) {
  const {
    sessions,
    cliSessions,
    connecting,
    lost,
    connect,
    disconnect,
    reconnect,
    selectProfile,
    openDialog,
    deleteProfile,
    duplicateProfile,
  } = useApp();
  const cli = cliSessions[profile.id];
  const connected = Boolean(sessions[profile.id]);
  const busy = Boolean(connecting[profile.id]);
  const lostConn = !connected && !busy && Boolean(lost[profile.id]);
  const color = accentColor(profile.color);

  const open = () => {
    if (busy) return;
    // selectProfile also leaves the launcher (store-side)
    if (connected || lostConn) selectProfile(profile.id);
    else void connect(profile.id);
  };

  const confirmDelete = () => {
    if (confirm(`Delete connection "${profile.name}"?`)) {
      void deleteProfile(profile.id);
    }
  };

  return (
    <ContextMenu>
    <ContextMenuTrigger className="block min-w-0">
    <div
      className={cn(
        "group relative flex cursor-pointer flex-col gap-1 rounded-lg border bg-zinc-925 px-3 py-2.5",
        lostConn
          ? "border-red-900/70 hover:border-red-800"
          : "border-zinc-800 hover:border-zinc-600 hover:bg-zinc-900",
      )}
      style={color ? { boxShadow: `inset 3px 0 0 ${color}` } : undefined}
      onClick={open}
      title={
        busy
          ? "Connecting…"
          : connected
            ? "Open workspace"
            : lostConn
              ? "Connection lost — open workspace to reconnect"
              : "Connect"
      }
    >
      <div className="flex items-center gap-2 min-w-0">
        <span
          className={cn(
            "size-2 rounded-full shrink-0",
            connected
              ? "bg-emerald-500"
              : busy
                ? "bg-amber-400"
                : lostConn
                  ? "bg-red-500"
                  : "bg-zinc-600",
          )}
        />
        <span className="min-w-0 flex-1 truncate text-[13px] text-zinc-100">
          {profile.name}
        </span>
        {/* fades out on hover so the action buttons don't overlap it */}
        {(profile.production || cli) && (
          <span className="flex shrink-0 items-center gap-1 transition-opacity group-hover:opacity-0">
            {cli && <CliBadge idleSec={cli.idleSec} />}
            {profile.production && <ProdBadge />}
          </span>
        )}
      </div>
      <div className="truncate pl-4 font-mono text-[11px] text-zinc-500">
        {profile.ssh?.host ? `${profile.ssh.host} ⇢ ` : ""}
        {profileAddr(profile)}
      </div>
      <div className="pl-4 text-[10px] text-zinc-600">
        {busy
          ? "connecting…"
          : connected
            ? "connected"
            : lostConn
              ? "connection lost"
              : " "}
      </div>

      <div
        className="absolute right-1.5 top-1.5 hidden items-center gap-0.5 group-hover:flex"
        onClick={(e) => e.stopPropagation()}
      >
        {busy ? (
          <Loader2 size={13} className="m-1 animate-spin text-amber-400" />
        ) : connected ? (
          <IconBtn title="Disconnect" onClick={() => void disconnect(profile.id)}>
            <Unplug size={13} />
          </IconBtn>
        ) : (
          <IconBtn
            title={lostConn ? "Reconnect" : "Connect"}
            onClick={() => void connect(profile.id)}
          >
            {lostConn ? <RefreshCw size={13} /> : <Plug size={13} />}
          </IconBtn>
        )}
        <IconBtn title="Edit" onClick={() => openDialog(profile)}>
          <Pencil size={13} />
        </IconBtn>
        <IconBtn title="Delete" onClick={confirmDelete}>
          <Trash2 size={13} />
        </IconBtn>
      </div>
    </div>
    </ContextMenuTrigger>
    <ContextMenuContent>
      {connected ? (
        <>
          <ContextMenuItem icon={RefreshCw} onClick={() => void reconnect(profile.id)}>
            Reconnect
          </ContextMenuItem>
          <ContextMenuItem icon={Unplug} onClick={() => void disconnect(profile.id)}>
            Disconnect
          </ContextMenuItem>
        </>
      ) : (
        <ContextMenuItem
          icon={lostConn ? RefreshCw : Plug}
          disabled={busy}
          onClick={() => void connect(profile.id)}
        >
          {lostConn ? "Reconnect" : "Connect"}
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
        onClick={confirmDelete}
      >
        Delete
      </ContextMenuItem>
    </ContextMenuContent>
    </ContextMenu>
  );
}

/** Full-window connection launcher: shown when nothing is connected, or on
 *  demand ("All connections") over a live workspace. */
export function Launcher() {
  const { profiles, sessions, lost, activeProfileId, launcherOpen, setLauncherOpen, openDialog } =
    useApp();
  const [filter, setFilter] = useState("");
  const needle = filter.trim().toLowerCase();
  const isMac = navigator.userAgent.includes("Mac");

  // An explicitly opened launcher can be dismissed back to the workspace;
  // with nothing connected there is nowhere to go back to.
  const canClose = Boolean(
    launcherOpen &&
      activeProfileId &&
      (sessions[activeProfileId] || lost[activeProfileId]),
  );

  useEffect(() => {
    if (!canClose) return;
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const s = useApp.getState();
      // overlays on top of the launcher own their Esc
      if (s.palette || s.dialog.open || s.settingsOpen) return;
      setLauncherOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [canClose, setLauncherOpen]);

  const visible = useMemo(
    () =>
      needle
        ? profiles.filter(
            (p) =>
              p.name.toLowerCase().includes(needle) ||
              p.group?.trim().toLowerCase().includes(needle) ||
              profileAddr(p).toLowerCase().includes(needle),
          )
        : profiles,
    [profiles, needle],
  );

  // Cluster connections by group so same-group cards sit together: named groups
  // (alphabetical) first, ungrouped last.
  const grouped = useMemo(() => {
    const map = new Map<string, Profile[]>();
    for (const p of visible) {
      const g = p.group?.trim() ?? "";
      const arr = map.get(g);
      if (arr) arr.push(p);
      else map.set(g, [p]);
    }
    const named = [...map.entries()]
      .filter(([g]) => g !== "")
      .sort(([a], [b]) => a.localeCompare(b));
    return { named, ungrouped: map.get("") ?? [] };
  }, [visible]);

  return (
    <div className="flex flex-1 min-w-0 flex-col">
      <div
        data-tauri-drag-region
        className={cn(
          "flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 pr-3",
          isMac ? "pl-24" : "pl-4",
        )}
      >
        <div
          data-tauri-drag-region
          className="flex items-center gap-1.5 text-[11px] font-semibold tracking-wider text-zinc-500"
        >
          <Database size={12} /> CONNECTIONS
        </div>
        {canClose && (
          <IconBtn title="Back to workspace (Esc)" onClick={() => setLauncherOpen(false)}>
            <X size={15} />
          </IconBtn>
        )}
      </div>

      <div className="flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-3xl px-6 py-8">
          {profiles.length > 0 ? (
            <>
              <div className="flex items-center gap-2 pb-4">
                <Input
                  autoFocus
                  placeholder="Filter connections…"
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape" && filter) {
                      e.stopPropagation();
                      setFilter("");
                    }
                  }}
                />
                <button
                  className="flex shrink-0 items-center gap-1.5 rounded-md border border-zinc-700 px-2.5 py-1 text-[12px] text-zinc-300 hover:bg-zinc-800 hover:text-zinc-100"
                  onClick={() => openDialog()}
                >
                  <Plus size={13} /> New connection
                </button>
              </div>
              <div className="grid grid-cols-[repeat(auto-fill,minmax(230px,1fr))] gap-2.5">
                {grouped.named.map(([group, ps]) => (
                  <Fragment key={group}>
                    <div className="col-span-full flex items-center gap-1.5 pt-3 text-[11px] font-medium text-zinc-400 first:pt-0">
                      <GitFork size={11} className="text-amber-500/70" />
                      {group}
                      <span className="text-zinc-600">· {ps.length}</span>
                    </div>
                    {ps.map((p) => (
                      <ProfileCard key={p.id} profile={p} />
                    ))}
                  </Fragment>
                ))}
                {grouped.ungrouped.length > 0 && (
                  <Fragment>
                    {grouped.named.length > 0 && (
                      <div className="col-span-full pt-3 text-[11px] font-medium text-zinc-500">
                        Ungrouped
                      </div>
                    )}
                    {grouped.ungrouped.map((p) => (
                      <ProfileCard key={p.id} profile={p} />
                    ))}
                  </Fragment>
                )}
              </div>
              {visible.length === 0 && (
                <div className="py-8 text-center text-[12px] text-zinc-600">
                  no matches
                </div>
              )}
            </>
          ) : (
            <div className="flex flex-col items-center gap-3 pt-16">
              <div className="text-[15px] text-zinc-500">sql-kai</div>
              <button
                className="rounded-md border border-dashed border-zinc-700 px-6 py-4 text-[13px] text-zinc-400 hover:border-zinc-500 hover:text-zinc-200"
                onClick={() => openDialog()}
              >
                + Add your first connection
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
