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
import { isMac } from "../lib/platform";
import { profileAddr } from "../lib/profile";
import { useApp } from "../lib/store";
import type { Profile } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "./ContextMenu";
import { CliBadge, cn, IconButton, Input, ProdBadge } from "./ui";

function ProfileCard({ profile }: { profile: Profile }) {
  const sessions = useApp((s) => s.sessions);
  const cliSessions = useApp((s) => s.cliSessions);
  const connecting = useApp((s) => s.connecting);
  const lost = useApp((s) => s.lost);
  const connectError = useApp((s) => s.connectError);
  const connect = useApp((s) => s.connect);
  const disconnect = useApp((s) => s.disconnect);
  const reconnect = useApp((s) => s.reconnect);
  const selectProfile = useApp((s) => s.selectProfile);
  const openDialog = useApp((s) => s.openDialog);
  const deleteProfile = useApp((s) => s.deleteProfile);
  const duplicateProfile = useApp((s) => s.duplicateProfile);
  const cli = cliSessions[profile.id];
  const connected = Boolean(sessions[profile.id]);
  const busy = Boolean(connecting[profile.id]);
  const err = !connected && !busy ? connectError[profile.id] : undefined;
  const lostConn = !connected && !busy && !err && Boolean(lost[profile.id]);
  // single source of truth for how the card looks
  const state: "connected" | "connecting" | "lost" | "refused" | "idle" = busy
    ? "connecting"
    : connected
      ? "connected"
      : err
        ? "refused"
        : lostConn
          ? "lost"
          : "idle";
  const retry = state === "lost" || state === "refused";
  const color = accentColor(profile.color);

  const open = () => {
    if (busy) return;
    // selectProfile also leaves the launcher (store-side); a live or lost
    // profile has a workspace to return to, refused/idle just (re)connects
    if (connected || state === "lost") selectProfile(profile.id);
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
        "group relative flex cursor-pointer flex-col gap-1 rounded-lg border px-3 py-2.5 transition-colors",
        state === "connected"
          ? "border-emerald-500/40 bg-emerald-950/40 hover:border-emerald-500/60"
          : retry
            ? "border-red-900/70 bg-zinc-925 hover:border-red-800 hover:bg-zinc-900"
            : "border-zinc-800 bg-zinc-925 hover:border-zinc-600 hover:bg-zinc-900",
      )}
      style={color ? { boxShadow: `inset 3px 0 0 ${color}` } : undefined}
      onClick={open}
      title={
        busy
          ? "Connecting…"
          : connected
            ? "Open workspace"
            : state === "lost"
              ? "Connection lost — open workspace to reconnect"
              : state === "refused"
                ? `Connection failed — click to retry\n${err}`
                : "Connect"
      }
    >
      <div className="flex items-center gap-2 min-w-0">
        <span
          className={cn(
            "size-2 rounded-full shrink-0",
            state === "connected"
              ? "bg-emerald-500 shadow-[0_0_6px_0] shadow-emerald-500/70"
              : state === "connecting"
                ? "bg-amber-400"
                : retry
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
      <div
        className={cn(
          "pl-4 text-[10px]",
          state === "connected"
            ? "text-emerald-500/80"
            : retry
              ? "text-red-400/80"
              : "text-zinc-600",
        )}
      >
        {busy
          ? "connecting…"
          : connected
            ? "connected"
            : lostConn
              ? "connection lost"
              : err
                ? "connection failed"
                : " "}
      </div>

      <div
        className="absolute right-1.5 top-1.5 hidden items-center gap-0.5 group-hover:flex"
        onClick={(e) => e.stopPropagation()}
      >
        {busy ? (
          <Loader2 size={13} className="m-1 animate-spin text-amber-400" />
        ) : connected ? (
          <IconButton title="Disconnect" onClick={() => void disconnect(profile.id)}>
            <Unplug size={13} />
          </IconButton>
        ) : (
          <IconButton
            title={retry ? "Reconnect" : "Connect"}
            onClick={() => void connect(profile.id)}
          >
            {retry ? <RefreshCw size={13} /> : <Plug size={13} />}
          </IconButton>
        )}
        <IconButton title="Edit" onClick={() => openDialog(profile)}>
          <Pencil size={13} />
        </IconButton>
        <IconButton title="Delete" onClick={confirmDelete}>
          <Trash2 size={13} />
        </IconButton>
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
          icon={retry ? RefreshCw : Plug}
          disabled={busy}
          onClick={() => void connect(profile.id)}
        >
          {retry ? "Reconnect" : "Connect"}
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
  const profiles = useApp((s) => s.profiles);
  const sessions = useApp((s) => s.sessions);
  const lost = useApp((s) => s.lost);
  const activeProfileId = useApp((s) => s.activeProfileId);
  const launcherOpen = useApp((s) => s.launcherOpen);
  const setLauncherOpen = useApp((s) => s.setLauncherOpen);
  const openDialog = useApp((s) => s.openDialog);
  const [filter, setFilter] = useState("");
  const needle = filter.trim().toLowerCase();

  // An explicitly opened launcher can be dismissed back to the workspace;
  // with nothing connected there is nowhere to go back to.
  const canClose = Boolean(
    launcherOpen &&
      activeProfileId &&
      (sessions[activeProfileId] || lost[activeProfileId]),
  );

  useEffect(() => {
    if (!canClose) return;
    const handleKey = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      const s = useApp.getState();
      // overlays on top of the launcher own their Esc
      if (s.palette || s.dialog.open || s.settingsOpen) return;
      setLauncherOpen(false);
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
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
          <IconButton title="Back to workspace (Esc)" onClick={() => setLauncherOpen(false)}>
            <X size={15} />
          </IconButton>
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
