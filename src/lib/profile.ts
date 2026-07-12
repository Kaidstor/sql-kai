import type { Profile, SessionInfo } from "./types";

/** Profiles with a live session, in sidebar order — the same list drives the
 *  status-bar switcher and the Ctrl+1…9 hotkeys, so the numbering matches. */
export const connectedProfiles = (
  profiles: Profile[],
  sessions: Record<string, SessionInfo | undefined>,
) => profiles.filter((p) => sessions[p.id]);

/** Saved-query scope key: profiles in one group share it, ungrouped ones get their own. */
export const queryScopeOf = (p: Profile) => p.group?.trim() || p.id;

/** Human label for a profile's saved-query collection. */
export const scopeLabelOf = (p: Profile) => p.group?.trim() || p.name;

/** Short "host:port/database" address shown in lists. */
export const profileAddr = (p: Profile) => `${p.host}:${p.port}/${p.database}`;

/** The profile's most recent "last connected" mark with its source, if any. */
export const lastConnectedOf = (
  p: Profile,
): { at: number; via: "gui" | "cli" } | null => {
  const gui = p.lastConnected?.gui ?? 0;
  const cli = p.lastConnected?.cli ?? 0;
  if (!gui && !cli) return null;
  return cli > gui ? { at: cli, via: "cli" } : { at: gui, via: "gui" };
};

/** Compact relative time for last-connected marks: "just now", "5m ago", "2d ago". */
export const timeAgo = (ms: number): string => {
  const s = Math.max(0, Math.floor((Date.now() - ms) / 1000));
  if (s < 60) return "just now";
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ago`;
  const h = Math.floor(m / 60);
  if (h < 24) return `${h}h ago`;
  const d = Math.floor(h / 24);
  if (d < 30) return `${d}d ago`;
  const mo = Math.floor(d / 30);
  if (mo < 12) return `${mo}mo ago`;
  return `${Math.floor(d / 365)}y ago`;
};
