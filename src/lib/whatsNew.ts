// "What's new" after an app update.
//
// Trigger — the version changed since the last launch. Content — the latest
// releases from the public GitLab API (at least HISTORY entries; everything
// unseen is flagged fresh). Offline fallback: the notes the updater stashed
// right before the relaunch ({version, notes} in localStorage, see updater.ts).
import { getVersion } from "@tauri-apps/api/app";

const PENDING_KEY = "sqlt.pendingWhatsNew";
const SEEN_KEY = "sqlt.lastSeenVersion";
/** URL-encoded project path — same project the updater endpoint points at. */
const GITLAB_PROJECT = "kaidstor%2Fsql-kai";

/** The releases page — for the "All releases" jump from the dialog. */
export const RELEASES_PAGE_URL =
  "https://gitlab.com/kaidstor/sql-kai/-/releases";

/** How many versions the dialog shows even when fewer are new. */
const HISTORY = 3;

export interface ReleaseNote {
  version: string;
  notes: string;
  /** Released after the version the user last launched. */
  fresh?: boolean;
}

/** Stashes the notes of the update being installed until the relaunch. */
export function stashPendingNotes(version: string, notes?: string) {
  try {
    localStorage.setItem(
      PENDING_KEY,
      JSON.stringify({ version, notes: notes ?? "" }),
    );
  } catch {
    // best-effort
  }
}

/** "1.2.3" → numeric component compare; prereleases are not used. */
const cmp = (a: string, b: string): number => {
  const pa = a.split(".").map(Number);
  const pb = b.split(".").map(Number);
  for (let i = 0; i < Math.max(pa.length, pb.length); i++) {
    const d = (pa[i] ?? 0) - (pb[i] ?? 0);
    if (d) return d;
  }
  return 0;
};

/** Releases ≤ current from GitLab, newest first. */
async function fetchReleases(current: string): Promise<ReleaseNote[]> {
  const res = await fetch(
    `https://gitlab.com/api/v4/projects/${GITLAB_PROJECT}/releases?per_page=20`,
  );
  if (!res.ok) return [];
  const releases: { tag_name: string; description?: string }[] =
    await res.json();
  return releases
    .map((r) => ({
      version: r.tag_name.replace(/^v/, ""),
      notes: r.description ?? "",
    }))
    .filter((r) => cmp(r.version, current) <= 0)
    .sort((a, b) => cmp(b.version, a.version));
}

/**
 * Releases for the "What's new" dialog (empty — don't show it). Call once at
 * startup: marks the current version as seen.
 */
export async function collectWhatsNew(): Promise<ReleaseNote[]> {
  try {
    const current = await getVersion();
    const seen = localStorage.getItem(SEEN_KEY);
    localStorage.setItem(SEEN_KEY, current);

    let pending: ReleaseNote | null = null;
    const raw = localStorage.getItem(PENDING_KEY);
    if (raw) {
      localStorage.removeItem(PENDING_KEY);
      pending = JSON.parse(raw) as ReleaseNote;
    }

    // First launch (fresh install) — nothing to tell.
    if (!seen || seen === current) return [];

    const history = await fetchReleases(current).catch(() => []);
    if (history.length > 0) {
      const isFresh = (v: string) => cmp(v, seen) > 0;
      const freshCount = history.filter((r) => isFresh(r.version)).length;
      return history
        .slice(0, Math.max(HISTORY, freshCount))
        .map((r) => ({ ...r, fresh: isFresh(r.version) }));
    }

    // Offline: at least the stashed notes of the version that just landed.
    if (pending && pending.version === current && pending.notes.trim()) {
      return [{ ...pending, fresh: true }];
    }
    return [];
  } catch {
    return [];
  }
}
