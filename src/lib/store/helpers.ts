// Pure helpers shared by the slices — no store access, no side effects.
import type { RelationInfo } from "../types";
import type { RelRef, StructureTabState, Tab, TableTabState } from "./types";

/** Smallest free "Query N" title among open tabs (VS Code untitled-style).
 *  The ⌘⇧T reopen stack is deliberately not counted: it persists across runs,
 *  so counting it made the numbering grow forever. A restored closed tab may
 *  therefore collide with a live title — cosmetic only, identity is the id. */
export function nextQueryTitle(s: { tabs: Tab[] }): string {
  const used = new Set<number>();
  for (const t of s.tabs) {
    const m = /^Query (\d+)$/.exec(t.title);
    if (m) used.add(Number(m[1]));
  }
  let n = 1;
  while (used.has(n)) n++;
  return `Query ${n}`;
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

export const columnsKey = ({ profileId, schema, table }: RelRef) =>
  // разделитель `|` и между schema/table — `a`+`b.c` и `a.b`+`c` не должны
  // коллидировать в одном ключе кэша
  `${profileId}|${schema}|${table}`;

/** First FK covering each column name (string_agg output is ", "-joined) —
 *  один и тот же разбор нужен гриду (какие колонки кликабельны) и previewFk
 *  (по какой связи собирать WHERE). */
export function fkByColumn(
  rels: readonly RelationInfo[] | undefined,
): Map<string, RelationInfo> {
  const map = new Map<string, RelationInfo>();
  for (const r of rels ?? []) {
    for (const c of r.columns?.split(", ") ?? []) {
      if (!map.has(c)) map.set(c, r);
    }
  }
  return map;
}

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
