// Pure helpers shared by the slices — no store access, no side effects.
import type { StructureTabState, Tab, TableTabState } from "./types";

/** Next free "Query N" title, counting open and reopenable tabs. */
export function nextQueryTitle(s: {
  tabs: Tab[];
  closedTabs: { tab: Tab }[];
}): string {
  let max = 0;
  const titles = [
    ...s.tabs.map((t) => t.title),
    ...s.closedTabs.map((c) => c.tab.title),
  ];
  for (const title of titles) {
    const m = /^Query (\d+)$/.exec(title);
    if (m) max = Math.max(max, Number(m[1]));
  }
  return `Query ${max + 1}`;
}

/** Immutably merges a patch into one tab's state (caller vouches for the kind). */
export function patchState<S extends Tab["state"]>(
  tabs: Tab[],
  id: string,
  patch: Partial<S>,
): Tab[] {
  return tabs.map((t) =>
    t.id === id ? { ...t, state: { ...(t.state as S), ...patch } } : t,
  );
}

export const columnsKey = (profileId: string, schema: string, table: string) =>
  // разделитель `|` и между schema/table — `a`+`b.c` и `a.b`+`c` не должны
  // коллидировать в одном ключе кэша
  `${profileId}|${schema}|${table}`;

/** Immutable single-key removal — replaces the `{ ...map }; delete map[id]`
 *  boilerplate that recurred ~15× (session/metadata teardown). */
export function without<V>(map: Record<string, V>, key: string): Record<string, V> {
  const next = { ...map };
  delete next[key];
  return next;
}

/** Immutable predicate removal — for the profile-prefixed metadata caches
 *  (`<profileId>|<schema>.<table>` keys). */
export function omitBy<V>(
  map: Record<string, V>,
  drop: (key: string) => boolean,
): Record<string, V> {
  return Object.fromEntries(Object.entries(map).filter(([k]) => !drop(k)));
}

/** Blank table staging — for new tabs, discard, and post-apply resets. */
export const noTableEdits = (): Pick<
  TableTabState,
  "edits" | "deletes" | "inserts" | "applyFailed" | "applyError"
> => ({
  edits: {},
  deletes: [],
  inserts: [],
  applyFailed: false,
  applyError: undefined,
});

/** Blank structure staging. */
export const noStructureEdits = (): Pick<
  StructureTabState,
  "colEdits" | "colDrops" | "colAdds"
> => ({ colEdits: {}, colDrops: [], colAdds: [] });
