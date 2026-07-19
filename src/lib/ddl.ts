// Pure DDL builders for the Structure tab's index/FK/trigger/policy actions.
// These run immediately (confirm → execute_sql), unlike the staged column DDL
// in mutationSql.ts.
import { quoteIdent, relIdent } from "./sql";

const PLAIN_IDENT = /^[a-zA-Z_][a-zA-Z0-9_$]*$/;

/** Comma-separated list where plain identifiers get quoted and anything else
 *  (expressions like `lower(name)`) passes through raw. */
export const identList = (s: string): string =>
  s
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean)
    .map((part) => (PLAIN_IDENT.test(part) ? quoteIdent(part) : part))
    .join(", ");

// --- Indexes -------------------------------------------------------------------

export interface NewIndex {
  /** "" = let Postgres pick the name. */
  name: string;
  /** Columns and/or expressions, comma-separated. */
  columns: string;
  unique: boolean;
  /** btree / hash / gin / gist / brin; btree omits USING. */
  method: string;
}

export const INDEX_METHODS = ["btree", "hash", "gin", "gist", "brin"];

export function createIndexSql(schema: string, table: string, ix: NewIndex): string {
  const parts = ["CREATE"];
  if (ix.unique) parts.push("UNIQUE");
  parts.push("INDEX");
  if (ix.name.trim()) parts.push(quoteIdent(ix.name.trim()));
  parts.push("ON", relIdent(schema, table));
  if (ix.method !== "btree") parts.push("USING", ix.method);
  parts.push(`(${identList(ix.columns)})`);
  return parts.join(" ");
}

export const renameIndexSql = (schema: string, name: string, to: string): string =>
  `ALTER INDEX ${relIdent(schema, name)} RENAME TO ${quoteIdent(to)}`;

// --- Foreign keys ---------------------------------------------------------------

export interface NewForeignKey {
  /** "" = let Postgres pick the name. */
  name: string;
  columns: string;
  refSchema: string;
  refTable: string;
  /** "" = the referenced table's primary key. */
  refColumns: string;
  onUpdate: string;
  onDelete: string;
}

export const FK_ACTIONS = [
  "NO ACTION",
  "RESTRICT",
  "CASCADE",
  "SET NULL",
  "SET DEFAULT",
];

export function createFkSql(
  schema: string,
  table: string,
  fk: NewForeignKey,
): string {
  const parts = [`ALTER TABLE ${relIdent(schema, table)} ADD`];
  if (fk.name.trim()) parts.push(`CONSTRAINT ${quoteIdent(fk.name.trim())}`);
  parts.push(`FOREIGN KEY (${identList(fk.columns)})`);
  parts.push(`REFERENCES ${relIdent(fk.refSchema, fk.refTable)}`);
  if (fk.refColumns.trim()) parts.push(`(${identList(fk.refColumns)})`);
  if (fk.onUpdate !== "NO ACTION") parts.push(`ON UPDATE ${fk.onUpdate}`);
  if (fk.onDelete !== "NO ACTION") parts.push(`ON DELETE ${fk.onDelete}`);
  return parts.join(" ");
}

export const dropConstraintSql = (
  schema: string,
  table: string,
  name: string,
): string =>
  `ALTER TABLE ${relIdent(schema, table)} DROP CONSTRAINT ${quoteIdent(name)}`;

export const renameConstraintSql = (
  schema: string,
  table: string,
  name: string,
  to: string,
): string =>
  `ALTER TABLE ${relIdent(schema, table)} RENAME CONSTRAINT ${quoteIdent(name)} TO ${quoteIdent(to)}`;

// --- Triggers -------------------------------------------------------------------

export interface NewTrigger {
  name: string;
  /** BEFORE / AFTER / INSTEAD OF. */
  timing: string;
  /** Non-empty subset of INSERT / UPDATE / DELETE / TRUNCATE. */
  events: string[];
  /** ROW / STATEMENT. */
  level: string;
  /** Function name, "()" appended when missing. */
  fn: string;
}

export const TRIGGER_TIMINGS = ["BEFORE", "AFTER", "INSTEAD OF"];
export const TRIGGER_EVENTS = ["INSERT", "UPDATE", "DELETE", "TRUNCATE"];

export function createTriggerSql(
  schema: string,
  table: string,
  t: NewTrigger,
): string {
  const fn = t.fn.trim();
  const call = fn.endsWith(")") ? fn : `${fn}()`;
  return (
    `CREATE TRIGGER ${quoteIdent(t.name.trim())} ${t.timing} ${t.events.join(" OR ")} ` +
    `ON ${relIdent(schema, table)} FOR EACH ${t.level} EXECUTE FUNCTION ${call}`
  );
}

export const dropTriggerSql = (schema: string, table: string, name: string): string =>
  `DROP TRIGGER ${quoteIdent(name)} ON ${relIdent(schema, table)}`;

export const renameTriggerSql = (
  schema: string,
  table: string,
  name: string,
  to: string,
): string =>
  `ALTER TRIGGER ${quoteIdent(name)} ON ${relIdent(schema, table)} RENAME TO ${quoteIdent(to)}`;

export const setTriggerEnabledSql = (
  schema: string,
  table: string,
  name: string,
  enabled: boolean,
): string =>
  `ALTER TABLE ${relIdent(schema, table)} ${enabled ? "ENABLE" : "DISABLE"} TRIGGER ${quoteIdent(name)}`;

// --- Row-level security policies -------------------------------------------------

export interface NewPolicy {
  name: string;
  /** ALL / SELECT / INSERT / UPDATE / DELETE. */
  command: string;
  permissive: boolean;
  /** Comma-separated roles; "" = PUBLIC. */
  roles: string;
  /** Raw USING expression ("" = omit). */
  using: string;
  /** Raw WITH CHECK expression ("" = omit). */
  check: string;
}

export const POLICY_COMMANDS = ["ALL", "SELECT", "INSERT", "UPDATE", "DELETE"];

/** PUBLIC / CURRENT_USER / SESSION_USER are keywords, not role names. */
const roleList = (roles: string): string =>
  roles
    .split(",")
    .map((r) => r.trim())
    .filter(Boolean)
    .map((r) =>
      ["public", "current_user", "session_user", "current_role"].includes(
        r.toLowerCase(),
      )
        ? r.toUpperCase()
        : quoteIdent(r),
    )
    .join(", ");

export function createPolicySql(
  schema: string,
  table: string,
  p: NewPolicy,
): string {
  const parts = [`CREATE POLICY ${quoteIdent(p.name.trim())} ON ${relIdent(schema, table)}`];
  if (!p.permissive) parts.push("AS RESTRICTIVE");
  if (p.command !== "ALL") parts.push(`FOR ${p.command}`);
  if (p.roles.trim()) parts.push(`TO ${roleList(p.roles)}`);
  if (p.using.trim()) parts.push(`USING (${p.using.trim()})`);
  if (p.check.trim()) parts.push(`WITH CHECK (${p.check.trim()})`);
  return parts.join(" ");
}

export const dropPolicySql = (schema: string, table: string, name: string): string =>
  `DROP POLICY ${quoteIdent(name)} ON ${relIdent(schema, table)}`;

export const renamePolicySql = (
  schema: string,
  table: string,
  name: string,
  to: string,
): string =>
  `ALTER POLICY ${quoteIdent(name)} ON ${relIdent(schema, table)} RENAME TO ${quoteIdent(to)}`;

/** ALTER POLICY can change roles / USING / WITH CHECK (not name or command). */
export function alterPolicySql(
  schema: string,
  table: string,
  name: string,
  patch: { roles?: string; using?: string; check?: string },
): string | null {
  const parts: string[] = [];
  if (patch.roles !== undefined) {
    parts.push(`TO ${patch.roles.trim() ? roleList(patch.roles) : "PUBLIC"}`);
  }
  if (patch.using !== undefined && patch.using.trim()) {
    parts.push(`USING (${patch.using.trim()})`);
  }
  if (patch.check !== undefined && patch.check.trim()) {
    parts.push(`WITH CHECK (${patch.check.trim()})`);
  }
  if (parts.length === 0) return null;
  return `ALTER POLICY ${quoteIdent(name)} ON ${relIdent(schema, table)} ${parts.join(" ")}`;
}

export const setRlsSql = (schema: string, table: string, enabled: boolean): string =>
  `ALTER TABLE ${relIdent(schema, table)} ${enabled ? "ENABLE" : "DISABLE"} ROW LEVEL SECURITY`;
