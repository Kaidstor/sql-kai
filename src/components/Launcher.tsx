import {
  CopyPlus,
  Database,
  GitFork,
  Loader2,
  Pencil,
  Plug,
  Plus,
  RefreshCw,
  RotateCcw,
  Trash2,
  Unplug,
  X,
} from "lucide-react";
import { type CSSProperties, Fragment, useEffect, useMemo, useRef, useState } from "react";
import { accentColor } from "../lib/colors";
import { isKey } from "../lib/keys";
import { switchLayout } from "../lib/layout";
import { isMac } from "../lib/platform";
import { lastConnectedOf, profileAddr, timeAgo } from "../lib/profile";
import { UNDO_DELETE_MS, useApp } from "../lib/store";
import type { Profile } from "../lib/types";
import { dragWindow } from "../lib/window";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
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
  const requestDeleteProfile = useApp((s) => s.requestDeleteProfile);
  const undoDeleteProfile = useApp((s) => s.undoDeleteProfile);
  const pendingDelete = useApp((s) => s.pendingDelete);
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
  const last = lastConnectedOf(profile);

  const open = () => {
    if (busy) return;
    // selectProfile also leaves the launcher (store-side); a live or lost
    // profile has a workspace to return to, refused/idle just (re)connects
    if (connected || state === "lost") selectProfile(profile.id);
    else void connect(profile.id);
  };

  // Deletion is scheduled with an undo grace period instead of a confirm():
  // the card flips into a countdown state and a click anywhere cancels it.
  if (pendingDelete[profile.id]) {
    return (
      <div
        role="button"
        tabIndex={0}
        className="relative cursor-pointer overflow-hidden rounded-lg border border-zinc-800 bg-zinc-925 px-3 py-2.5 hover:border-zinc-600 focus-visible:border-sky-500 focus-visible:outline-none"
        onClick={() => undoDeleteProfile(profile.id)}
        onKeyDown={(e) => {
          if (e.key !== "Enter" && e.key !== " ") return;
          e.preventDefault();
          undoDeleteProfile(profile.id);
        }}
        title="Click to undo"
      >
        <div className="flex items-center gap-2 min-w-0">
          <span className="size-2 shrink-0 rounded-full bg-zinc-700" />
          <span className="min-w-0 flex-1 truncate text-[13px] text-zinc-500 line-through">
            {profile.name}
          </span>
        </div>
        <div className="truncate pl-4 pt-1 font-mono text-[11px] text-zinc-600">
          {profile.ssh?.host ? `${profile.ssh.host} ⇢ ` : ""}
          {profileAddr(profile)}
        </div>
        <div className="flex items-center gap-1.5 pl-4 pt-1 text-[10px] text-amber-500/90">
          <RotateCcw size={10} />
          deleted — click to undo
        </div>
        <span
          className="absolute bottom-0 left-0 h-0.5 bg-amber-500/70"
          style={{ animation: `undo-countdown ${UNDO_DELETE_MS}ms linear forwards` }}
        />
      </div>
    );
  }

  return (
    <ContextMenu>
    <ContextMenuTrigger className="block min-w-0">
    <div
      role="button"
      tabIndex={0}
      className={cn(
        "group relative flex cursor-pointer flex-col gap-1 rounded-lg border px-3 py-2.5 transition-colors",
        // keyboard focus takes the connection's accent color so the border and
        // the left stripe read as one highlight; colorless cards fall back to sky
        "focus-visible:border-(--card-accent) focus-visible:outline-none",
        state === "connected"
          ? "border-emerald-500/40 bg-emerald-950/40 hover:border-emerald-500/60"
          : retry
            ? "border-red-900/70 bg-zinc-925 hover:border-red-800 hover:bg-zinc-900"
            : "border-zinc-800 bg-zinc-925 hover:border-zinc-600 hover:bg-zinc-900",
      )}
      style={
        {
          "--card-accent": color ?? "var(--color-sky-500)",
          ...(color ? { boxShadow: `inset 3px 0 0 ${color}` } : null),
        } as CSSProperties
      }
      onClick={open}
      onKeyDown={(e) => {
        // card-level shortcuts replace tabbing into the hover-only action
        // buttons: Enter/Space connect, ⌘E edits, Delete/Backspace deletes
        if ((e.metaKey || e.ctrlKey) && !e.altKey && !e.shiftKey && isKey(e, "e")) {
          e.preventDefault();
          openDialog(profile);
          return;
        }
        if (e.key === "Delete" || e.key === "Backspace") {
          e.preventDefault();
          requestDeleteProfile(profile.id);
          return;
        }
        if (e.key !== "Enter" && e.key !== " ") return;
        e.preventDefault();
        open();
      }}
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
      {/* on hover the absolute action buttons appear top-right — pad the row
          so a long name truncates before running underneath them */}
      <div className="flex items-center gap-2 min-w-0 group-hover:pr-16">
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
                : last
                  ? `last: ${timeAgo(last.at)} · ${last.via}`
                  : "never connected"}
      </div>

      {/* tabIndex -1: the card itself is the single tab stop — these appear on
          hover for the mouse, keyboard users act via the card shortcuts */}
      <div
        className="absolute right-1.5 top-1.5 hidden items-center gap-0.5 group-hover:flex"
        onClick={(e) => e.stopPropagation()}
      >
        {busy ? (
          <Loader2 size={13} className="m-1 animate-spin text-amber-400" />
        ) : connected ? (
          <IconButton tabIndex={-1} title="Disconnect" onClick={() => void disconnect(profile.id)}>
            <Unplug size={13} />
          </IconButton>
        ) : (
          <IconButton
            tabIndex={-1}
            title={retry ? "Reconnect" : "Connect"}
            onClick={() => void connect(profile.id)}
          >
            {retry ? <RefreshCw size={13} /> : <Plug size={13} />}
          </IconButton>
        )}
        <IconButton tabIndex={-1} title="Edit" onClick={() => openDialog(profile)}>
          <Pencil size={13} />
        </IconButton>
        <IconButton tabIndex={-1} title="Delete" onClick={() => requestDeleteProfile(profile.id)}>
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
        <ContextMenuShortcut>{isMac ? "⌘E" : "Ctrl+E"}</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem
        icon={Trash2}
        iconClassName="text-red-400/80"
        onClick={() => requestDeleteProfile(profile.id)}
      >
        Delete
        <ContextMenuShortcut>{isMac ? "⌫" : "Del"}</ContextMenuShortcut>
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
  const filterRef = useRef<HTMLInputElement>(null);
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

  // ⌘F/Ctrl+F jumps back into the filter after focus wandered off to a card
  useEffect(() => {
    const handleKey = (e: KeyboardEvent) => {
      if (!(e.metaKey || e.ctrlKey) || e.altKey || e.shiftKey) return;
      if (!isKey(e, "f")) return;
      const s = useApp.getState();
      // overlays on top of the launcher own the keyboard
      if (s.palette || s.dialog.open || s.settingsOpen || s.logViewerOpen) return;
      e.preventDefault();
      filterRef.current?.focus();
      filterRef.current?.select();
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, []);

  const visible = useMemo(() => {
    if (!needle) return profiles;
    const match = (n: string) =>
      profiles.filter(
        (p) =>
          p.name.toLowerCase().includes(n) ||
          p.group?.trim().toLowerCase().includes(n) ||
          profileAddr(p).toLowerCase().includes(n),
      );
    const hits = match(needle);
    if (hits.length > 0) return hits;
    // wrong-keyboard-layout rescue: "зкщв" finds "prod"
    const switched = switchLayout(needle);
    return switched === needle ? hits : match(switched);
  }, [profiles, needle]);

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
        onMouseDown={dragWindow}
        className={cn(
          "flex h-10 shrink-0 items-center justify-between border-b border-zinc-800 pr-3",
          isMac ? "pl-24" : "pl-4",
        )}
      >
        <div className="flex items-center gap-1.5 text-[11px] font-semibold tracking-wider text-zinc-500">
          <Database size={12} /> CONNECTIONS
        </div>
        {canClose && (
          <IconButton title="Back to workspace (Esc)" onClick={() => setLauncherOpen(false)}>
            <X size={15} />
          </IconButton>
        )}
      </div>

      <div className="flex-1 overflow-y-scroll">
        <div className="mx-auto w-full max-w-3xl px-6 py-8">
          {profiles.length > 0 ? (
            <>
              <div className="flex items-center gap-2 pb-4">
                {/* the button sits BEFORE the input in the DOM (order-first flips
                    them back visually) so Tab from the filter lands on the first
                    connection card, not here; the button stays on Shift+Tab.
                    While filtering it disappears entirely: the user is picking a
                    connection, so Tab should only walk the matching cards */}
                {!needle && (
                  <button
                    className="flex shrink-0 items-center gap-1.5 rounded-md border border-zinc-700 px-2.5 py-1 text-[12px] text-zinc-300 hover:bg-zinc-800 hover:text-zinc-100"
                    onClick={() => openDialog()}
                  >
                    <Plus size={13} /> New connection
                  </button>
                )}
                <Input
                  ref={filterRef}
                  autoFocus
                  className="order-first"
                  placeholder={isMac ? "Filter connections…  ⌘F" : "Filter connections…  Ctrl+F"}
                  value={filter}
                  onChange={(e) => setFilter(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape" && filter) {
                      e.stopPropagation();
                      setFilter("");
                    }
                  }}
                />
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
