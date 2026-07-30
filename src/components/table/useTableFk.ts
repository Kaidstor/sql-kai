import { useEffect, useMemo } from "react";
import { columnsKey, fkByColumn, useApp, type RelRef } from "../../lib/store";

/** Indices of the columns covered by a foreign key — the grid marks them
 *  clickable (⌘-click → previewFk). Loads the table's relations on first use;
 *  a failure just leaves the set empty (FK navigation stays off). */
export function useTableFk(
  ref: RelRef,
  connected: boolean,
  columns: string[] | undefined,
): ReadonlySet<number> {
  const loadTableRelations = useApp((s) => s.loadTableRelations);
  const rels = useApp((s) => s.tableRelations[columnsKey(ref)]);

  useEffect(() => {
    if (connected && !rels) void loadTableRelations(ref);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected, rels, ref]);

  return useMemo(() => {
    const byCol = fkByColumn(rels);
    const set = new Set<number>();
    columns?.forEach((name, i) => {
      if (byCol.has(name)) set.add(i);
    });
    return set;
  }, [rels, columns]);
}
