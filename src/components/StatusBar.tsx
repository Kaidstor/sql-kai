import { Cable, ChevronUp, Lock, Plug, Settings, ShieldCheck } from "lucide-react";
import { type MouseEvent, useRef, useState } from "react";
import { copyTextConcealed } from "../lib/clipboard";
import { accentColor } from "../lib/colors";
import { connectedProfiles, profileAddr } from "../lib/profile";
import { useApp } from "../lib/store";
import { ColorDot, Popover, cn } from "./ui";

/** Beekeeper-style bottom-left switcher between active connections. */
function ConnectionSwitcher() {
  const { profiles, sessions, activeProfileId, selectProfile, setPalette } =
    useApp();
  const [open, setOpen] = useState(false);

  const connected = connectedProfiles(profiles, sessions);
  const active = profiles.find((p) => p.id === activeProfileId);
  const activeColor = accentColor(active?.color);

  return (
    <Popover
      open={open}
      onClose={() => setOpen(false)}
      side="top"
      panelClassName="w-72 p-1"
      trigger={
        <button
          onClick={() => setOpen((v) => !v)}
          className={cn(
            "flex items-center gap-1.5 rounded px-1.5 py-0.5 text-[11px]",
            "hover:bg-zinc-800/80 transition-colors",
            active && sessions[active.id] ? "text-zinc-200" : "text-zinc-500",
          )}
          title="Active connections (Ctrl+1…9) · ⌘⌥O to open"
        >
          <Cable
            size={11}
            style={activeColor ? { color: activeColor } : undefined}
            className={activeColor ? undefined : "text-emerald-500"}
          />
          {active && sessions[active.id] ? active.name : "not connected"}
          {connected.length > 1 && (
            <span className="text-zinc-600">+{connected.length - 1}</span>
          )}
          <ChevronUp size={10} className="text-zinc-600" />
        </button>
      }
    >
      {connected.map((p, i) => {
        const color = accentColor(p.color);
        return (
          <div
            key={p.id}
            onClick={() => {
              setOpen(false);
              selectProfile(p.id);
            }}
            className={cn(
              "flex cursor-pointer items-center gap-2 rounded px-2 py-1.5",
              p.id === activeProfileId ? "bg-zinc-800" : "hover:bg-zinc-800/60",
            )}
          >
            <ColorDot color={color} />
            <div className="min-w-0 flex-1">
              <div className="flex items-center gap-1.5 text-[12px] text-zinc-200">
                <span className="truncate">{p.name}</span>
                {p.group?.trim() && (
                  <span className="shrink-0 rounded border border-zinc-700 bg-zinc-800 px-1 py-px text-[9px] text-zinc-400">
                    {p.group.trim()}
                  </span>
                )}
              </div>
              <div className="truncate font-mono text-[10px] text-zinc-500">
                {profileAddr(p)}
              </div>
            </div>
            {i < 9 && <span className="shrink-0 text-[10px] text-zinc-600">⌃{i + 1}</span>}
          </div>
        );
      })}
      {connected.length === 0 && (
        <div className="px-2 py-2 text-[11px] text-zinc-600">
          no active connections
        </div>
      )}
      <div className="mx-1 my-1 border-t border-zinc-800" />
      <div
        onClick={() => {
          setOpen(false);
          setPalette("connections");
        }}
        className="flex cursor-pointer items-center gap-2 rounded px-2 py-1.5 text-[12px] text-zinc-300 hover:bg-zinc-800/60"
      >
        <Plug size={12} className="text-zinc-500" />
        Open connection…
        <span className="ml-auto text-[10px] text-zinc-600">⌘⌥O</span>
      </div>
    </Popover>
  );
}

export function StatusBar() {
  const { activeProfileId, profiles, sessions, toast, lockVault, setSettingsOpen } =
    useApp();
  const [copied, setCopied] = useState(false);
  const copiedTimer = useRef<number | undefined>(undefined);
  const profile = profiles.find((p) => p.id === activeProfileId);
  const session = activeProfileId ? sessions[activeProfileId] : undefined;
  const color = accentColor(profile && session ? profile.color : null);

  const statusText = [
    profile && session && profile.name,
    session?.serverVersion && `PostgreSQL ${session.serverVersion}`,
    session?.tunnelPort && `ssh tunnel :${session.tunnelPort}`,
    toast?.message,
  ]
    .filter(Boolean)
    .join("\n");

  const copyStatus = async (e: MouseEvent<HTMLElement>) => {
    // Ignore clicks on interactive children (switcher, popover, lock).
    if ((e.target as HTMLElement).closest("button, [data-nocopy]")) return;
    if (!statusText || !(await copyTextConcealed(statusText))) return;
    setCopied(true);
    window.clearTimeout(copiedTimer.current);
    copiedTimer.current = window.setTimeout(() => setCopied(false), 1200);
  };

  return (
    <footer
      onClick={(e) => void copyStatus(e)}
      className="flex items-center gap-3 h-6 px-1.5 border-t border-zinc-800 bg-zinc-925 text-[11px] text-zinc-500 shrink-0 cursor-default"
      style={
        color
          ? {
              background: `color-mix(in srgb, ${color} 16%, var(--color-zinc-925))`,
              borderTopColor: `color-mix(in srgb, ${color} 45%, var(--color-zinc-800))`,
            }
          : undefined
      }
    >
      <span data-nocopy className="contents">
        <ConnectionSwitcher />
      </span>
      {profile && session && (
        <>
          {session.serverVersion && <span>PostgreSQL {session.serverVersion}</span>}
          {session.tunnelPort && (
            <span className="flex items-center gap-1 text-sky-500">
              <ShieldCheck size={11} />
              ssh tunnel :{session.tunnelPort}
            </span>
          )}
        </>
      )}
      {toast && (
        <span
          className={cn(
            "ml-auto truncate max-w-[70%] pr-1.5",
            toast.kind === "error"
              ? "text-red-400"
              : toast.kind === "success"
                ? "text-emerald-400"
                : "text-zinc-300",
          )}
          title={toast.message}
        >
          {toast.message.split("\n")[0]}
        </span>
      )}
      {copied && (
        <span className={cn("text-emerald-500", !toast && "ml-auto")}>
          copied
        </span>
      )}
      <button
        data-nocopy
        onClick={() => setSettingsOpen(true)}
        title="Settings (⌘,)"
        className={cn(
          "flex items-center gap-1 rounded px-1.5 py-0.5 transition-colors",
          "hover:bg-zinc-800/80 hover:text-zinc-300",
          toast || copied ? "ml-1" : "ml-auto",
        )}
      >
        <Settings size={11} />
      </button>
      <button
        onClick={() => void lockVault()}
        title="Lock vault — clears saved secrets and closes all connections"
        className={cn(
          "flex items-center gap-1 rounded px-1.5 py-0.5 transition-colors",
          "hover:bg-zinc-800/80 hover:text-zinc-300",
        )}
      >
        <Lock size={11} />
      </button>
    </footer>
  );
}
