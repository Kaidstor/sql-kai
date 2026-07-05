import type { Profile } from "./types";

/** Saved-query scope key: profiles in one group share it, ungrouped ones get their own. */
export const queryScopeOf = (p: Profile) => p.group?.trim() || p.id;

/** Human label for a profile's saved-query collection. */
export const scopeLabelOf = (p: Profile) => p.group?.trim() || p.name;

/** Short "host:port/database" address shown in lists. */
export const profileAddr = (p: Profile) => `${p.host}:${p.port}/${p.database}`;
