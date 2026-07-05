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
