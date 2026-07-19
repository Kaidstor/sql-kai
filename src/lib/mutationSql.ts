// Staged edits → SQL statements. Pure functions, no store access: the store
// stages changes, these turn them into the DDL/DML that Apply executes.
import { quoteIdent, quoteLit, relIdent } from "./sql";
import type { StructureTabState, TableTabState } from "./store";
import type { ColumnInfo, TablePage } from "./types";

/** SQL literal for a nullable cell value. */
const lit = (v: string | null) => (v === null ? "NULL" : quoteLit(v));

/** Staged structure changes → DDL statements. Edits reference the ORIGINAL
 *  column name (renames run last), edits of dropped columns are skipped. */
export function buildStructureDdl(st: StructureTabState): string[] {
  const rel = relIdent(st.schema, st.table);
  const alter = (rest: string) => `ALTER TABLE ${rel} ${rest}`;
  const drops = new Set(st.colDrops);
  const stmts: string[] = [];
  for (const [col, patch] of Object.entries(st.colEdits)) {
    if (drops.has(col)) continue;
    const c = quoteIdent(col);
    if (patch.type) stmts.push(alter(`ALTER COLUMN ${c} TYPE ${patch.type}`));
    if (patch.nullable !== undefined) {
      stmts.push(
        alter(`ALTER COLUMN ${c} ${patch.nullable ? "DROP NOT NULL" : "SET NOT NULL"}`),
      );
    }
    if (patch.default !== undefined) {
      stmts.push(
        alter(
          patch.default
            ? `ALTER COLUMN ${c} SET DEFAULT ${patch.default}`
            : `ALTER COLUMN ${c} DROP DEFAULT`,
        ),
      );
    }
    if (patch.comment !== undefined) {
      stmts.push(
        `COMMENT ON COLUMN ${rel}.${c} IS ${patch.comment ? quoteLit(patch.comment) : "NULL"}`,
      );
    }
    if (patch.name) stmts.push(alter(`RENAME COLUMN ${c} TO ${quoteIdent(patch.name)}`));
  }
  for (const col of st.colDrops) {
    stmts.push(alter(`DROP COLUMN ${quoteIdent(col)}`));
  }
  for (const a of st.colAdds) {
    stmts.push(
      alter(
        `ADD COLUMN ${quoteIdent(a.name)} ${a.type}` +
          (a.nullable ? "" : " NOT NULL") +
          (a.def ? ` DEFAULT ${a.def}` : ""),
      ),
    );
  }
  return stmts;
}

export type TableDml =
  | { stmts: string[]; updates: number; deletes: number }
  | { error: string };

/** Staged table edits → INSERT/UPDATE/DELETE statements. UPDATE/DELETE match
 *  rows by primary key (always the ORIGINAL row values, even if a pk cell was
 *  edited); INSERTs don't need one. */
export function buildTableDml(
  st: TableTabState,
  data: TablePage,
  cols: ColumnInfo[] | undefined,
): TableDml {
  const rel = relIdent(st.schema, st.table);
  const stmts: string[] = [];
  let updates = 0;
  let deletes = 0;

  if (Object.keys(st.edits).length > 0 || st.deletes.length > 0) {
    const pk = (cols ?? []).filter((c) => c.isPk);
    if (pk.length === 0) {
      return { error: "Table has no primary key — editing is disabled" };
    }
    const colIdx = new Map(data.result.columns.map((c, i) => [c, i] as const));
    const pkIdx: { name: string; idx: number }[] = [];
    for (const c of pk) {
      const idx = colIdx.get(c.name);
      if (idx === undefined) {
        return { error: `Primary key column "${c.name}" is missing from the page` };
      }
      pkIdx.push({ name: c.name, idx });
    }
    const cond = (ri: number) =>
      pkIdx
        .map(({ name, idx }) => {
          const v = data.result.rows[ri][idx];
          return v === null
            ? `${quoteIdent(name)} IS NULL`
            : `${quoteIdent(name)} = ${quoteLit(v)}`;
        })
        .join(" AND ");
    const deleted = new Set(st.deletes);
    for (const [riStr, rowEdits] of Object.entries(st.edits)) {
      const ri = Number(riStr);
      if (deleted.has(ri) || !data.result.rows[ri]) continue;
      const sets = Object.entries(rowEdits).map(([ciStr, v]) => {
        const name = data.result.columns[Number(ciStr)];
        return `${quoteIdent(name)} = ${lit(v)}`;
      });
      stmts.push(`UPDATE ${rel} SET ${sets.join(", ")} WHERE ${cond(ri)}`);
      updates += 1;
    }
    for (const ri of deleted) {
      if (!data.result.rows[ri]) continue;
      stmts.push(`DELETE FROM ${rel} WHERE ${cond(ri)}`);
      deletes += 1;
    }
  }

  for (const row of st.inserts) {
    const names: string[] = [];
    const vals: string[] = [];
    row.values.forEach((v, ci) => {
      if (v === undefined) return; // generated — let the DB default it
      names.push(quoteIdent(data.result.columns[ci]));
      vals.push(lit(v));
    });
    stmts.push(
      names.length > 0
        ? `INSERT INTO ${rel} (${names.join(", ")}) VALUES (${vals.join(", ")})`
        : `INSERT INTO ${rel} DEFAULT VALUES`,
    );
  }

  return { stmts, updates, deletes };
}
