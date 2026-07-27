use tokio_postgres::Client;

use super::exec::{cell, cell_bool, query_rows, query_scalar};
use crate::error::AppError;

pub fn quote_ident(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// A backslash forces the E'…' form: with `standard_conforming_strings = off`
/// (one server config or `ALTER ROLE SET` away) a plain '…' reads backslashes as
/// escapes, which changes the value or breaks the literal. Catalog queries pass
/// their own identifiers here, but MCP `parameters` pass values from a model —
/// the one caller whose input nobody vetted.
pub fn quote_literal(s: &str) -> String {
    let escaped = s.replace('\'', "''");
    if escaped.contains('\\') {
        format!("E'{}'", escaped.replace('\\', "\\\\"))
    } else {
        format!("'{escaped}'")
    }
}

/// quote_literal()'d qualified name, ready for a `::regclass` cast.
pub fn regclass_literal(schema: &str, table: &str) -> String {
    quote_literal(&format!("{}.{}", quote_ident(schema), quote_ident(table)))
}

pub const TABLES_SQL: &str = "\
SELECT n.nspname, c.relname, c.relkind::text
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r','p','v','m','f')
  AND n.nspname NOT IN ('pg_catalog','information_schema')
  AND n.nspname NOT LIKE 'pg_toast%'
  AND n.nspname NOT LIKE 'pg_temp%'
ORDER BY n.nspname, c.relname";

/// TABLES_SQL + a 4th column: approximate row count from pg_class.reltuples.
/// '?' = never analyzed (reltuples = -1); partitioned parents sum their
/// direct partitions; views get NULL.
pub const TABLES_COUNTS_SQL: &str = "\
SELECT n.nspname, c.relname, c.relkind::text,
       CASE
         WHEN c.relkind = 'p' THEN COALESCE((
             SELECT sum(GREATEST(ch.reltuples, 0))::bigint
             FROM pg_inherits i JOIN pg_class ch ON ch.oid = i.inhrelid
             WHERE i.inhparent = c.oid), 0)::text
         WHEN c.relkind IN ('r','m','f') THEN
           CASE WHEN c.reltuples < 0 THEN '?' ELSE c.reltuples::bigint::text END
       END
FROM pg_class c
JOIN pg_namespace n ON n.oid = c.relnamespace
WHERE c.relkind IN ('r','p','v','m','f')
  AND n.nspname NOT IN ('pg_catalog','information_schema')
  AND n.nspname NOT LIKE 'pg_toast%'
  AND n.nspname NOT LIKE 'pg_temp%'
ORDER BY n.nspname, c.relname";

/// `regclass` — a quote_literal()'d qualified table name, e.g. `'"public"."t"'`.
pub fn columns_sql(regclass: &str) -> String {
    format!(
        "SELECT a.attname,
                format_type(a.atttypid, a.atttypmod),
                (NOT a.attnotnull)::text,
                COALESCE((SELECT true FROM pg_index i
                          WHERE i.indrelid = a.attrelid
                            AND a.attnum = ANY(i.indkey)
                            AND i.indisprimary), false)::text,
                pg_get_expr(d.adbin, d.adrelid),
                col_description(a.attrelid, a.attnum)
         FROM pg_attribute a
         LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
         WHERE a.attrelid = {regclass}::regclass AND a.attnum > 0 AND NOT a.attisdropped
         ORDER BY a.attnum"
    )
}

pub fn indexes_sql(regclass: &str) -> String {
    format!(
        "SELECT c.relname,
                i.indisunique::text,
                i.indisprimary::text,
                (SELECT string_agg(a.attname, ', ' ORDER BY k.ord)
                   FROM unnest(i.indkey) WITH ORDINALITY AS k(attnum, ord)
                   JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = k.attnum
                  WHERE k.attnum > 0),
                pg_get_indexdef(i.indexrelid)
         FROM pg_index i
         JOIN pg_class c ON c.oid = i.indexrelid
         WHERE i.indrelid = {regclass}::regclass
         ORDER BY i.indisprimary DESC, c.relname"
    )
}

pub fn relations_sql(regclass: &str) -> String {
    format!(
        "SELECT con.conname,
                (SELECT string_agg(a.attname, ', ' ORDER BY k.ord)
                   FROM unnest(con.conkey) WITH ORDINALITY AS k(attnum, ord)
                   JOIN pg_attribute a ON a.attrelid = con.conrelid AND a.attnum = k.attnum),
                con.confrelid::regclass::text,
                (SELECT string_agg(a.attname, ', ' ORDER BY k.ord)
                   FROM unnest(con.confkey) WITH ORDINALITY AS k(attnum, ord)
                   JOIN pg_attribute a ON a.attrelid = con.confrelid AND a.attnum = k.attnum),
                CASE con.confupdtype WHEN 'a' THEN 'NO ACTION' WHEN 'r' THEN 'RESTRICT'
                     WHEN 'c' THEN 'CASCADE' WHEN 'n' THEN 'SET NULL' WHEN 'd' THEN 'SET DEFAULT'
                     ELSE con.confupdtype::text END,
                CASE con.confdeltype WHEN 'a' THEN 'NO ACTION' WHEN 'r' THEN 'RESTRICT'
                     WHEN 'c' THEN 'CASCADE' WHEN 'n' THEN 'SET NULL' WHEN 'd' THEN 'SET DEFAULT'
                     ELSE con.confdeltype::text END
         FROM pg_constraint con
         WHERE con.conrelid = {regclass}::regclass AND con.contype = 'f'
         ORDER BY con.conname"
    )
}

pub fn triggers_sql(regclass: &str) -> String {
    format!(
        "SELECT t.tgname,
                CASE WHEN (t.tgtype & 2) > 0 THEN 'BEFORE'
                     WHEN (t.tgtype & 64) > 0 THEN 'INSTEAD OF'
                     ELSE 'AFTER' END,
                concat_ws(' OR ',
                  CASE WHEN (t.tgtype & 4) > 0 THEN 'INSERT' END,
                  CASE WHEN (t.tgtype & 8) > 0 THEN 'DELETE' END,
                  CASE WHEN (t.tgtype & 16) > 0 THEN 'UPDATE' END,
                  CASE WHEN (t.tgtype & 32) > 0 THEN 'TRUNCATE' END),
                pg_get_triggerdef(t.oid),
                (t.tgenabled <> 'D')::text
         FROM pg_trigger t
         WHERE t.tgrelid = {regclass}::regclass AND NOT t.tgisinternal
         ORDER BY t.tgname"
    )
}

/// Every enum type with its labels, one row per label (ordered by
/// enumsortorder — the frontend groups consecutive rows into types).
pub const ENUMS_SQL: &str = "\
SELECT n.nspname, t.typname, e.enumlabel
FROM pg_enum e
JOIN pg_type t ON t.oid = e.enumtypid
JOIN pg_namespace n ON n.oid = t.typnamespace
ORDER BY n.nspname, t.typname, e.enumsortorder";

pub fn policies_sql(regclass: &str) -> String {
    format!(
        "SELECT pol.polname,
                CASE pol.polcmd WHEN 'r' THEN 'SELECT' WHEN 'a' THEN 'INSERT'
                     WHEN 'w' THEN 'UPDATE' WHEN 'd' THEN 'DELETE' ELSE 'ALL' END,
                pol.polpermissive::text,
                (SELECT string_agg(r.rolname, ', ' ORDER BY r.rolname)
                   FROM pg_roles r WHERE r.oid = ANY(pol.polroles)),
                pg_get_expr(pol.polqual, pol.polrelid),
                pg_get_expr(pol.polwithcheck, pol.polrelid)
         FROM pg_policy pol
         WHERE pol.polrelid = {regclass}::regclass
         ORDER BY pol.polname"
    )
}

/// Row-level-security switches of the table itself (policies apply only
/// while relrowsecurity is on).
pub fn rls_sql(regclass: &str) -> String {
    format!(
        "SELECT c.relrowsecurity::text, c.relforcerowsecurity::text
         FROM pg_class c WHERE c.oid = {regclass}::regclass"
    )
}

/// Filters of the whole-database dump ([`schema_dump_sql`]).
#[derive(Default)]
pub struct SchemaOptions {
    /// Dump this schema only (a system one is allowed); None = every
    /// non-system schema.
    pub schema: Option<String>,
    /// Dump this relation only, by unqualified name (`schema` narrows which
    /// schemas it is looked up in). Types, routines and sequences are then cut
    /// down to the ones this relation actually uses.
    pub table: Option<String>,
    /// Keep what is normally noise for a schema overview: system schemas,
    /// extension-owned objects and leaf partitions.
    pub internal: bool,
    /// Include view bodies and routine sources.
    pub definitions: bool,
    /// Include COMMENT ON texts.
    pub comments: bool,
}

impl SchemaOptions {
    /// Namespace predicate for a `pg_namespace` alias. Timescale keeps its
    /// chunks in `_timescaledb_*`, which dwarfs the user schema, so it is
    /// filtered out next to the `pg_*` ones.
    fn nsp(&self, alias: &str) -> String {
        match (&self.schema, self.internal) {
            (Some(s), _) => format!("{alias}.nspname = {}", quote_literal(s)),
            (None, true) => "true".to_string(),
            (None, false) => format!(
                "{alias}.nspname <> 'information_schema' \
                 AND {alias}.nspname NOT LIKE 'pg\\_%' \
                 AND {alias}.nspname NOT LIKE '\\_timescaledb\\_%'"
            ),
        }
    }

    /// Drops objects an extension owns (`pg_depend.deptype = 'e'`). On a
    /// postgis/timescale database this is the difference between a few dozen
    /// user objects and thousands of catalog rows. `class` is a caller-side
    /// constant ('pg_class' / 'pg_type' / 'pg_proc'), never user input.
    ///
    /// A named relation ([`SchemaOptions::table`]) is never noise: asking for
    /// `--table geometry_columns` and getting an empty dump because postgis
    /// owns it would be a lie about the database.
    fn no_ext(&self, class: &str, oid: &str) -> String {
        if self.internal || self.table.is_some() {
            return "true".to_string();
        }
        format!(
            "NOT EXISTS (SELECT 1 FROM pg_depend dep \
             WHERE dep.classid = '{class}'::regclass AND dep.objid = {oid} \
               AND dep.deptype = 'e')"
        )
    }

    /// Selects `expr` only when the caller asked for comments/definitions —
    /// keeps the result-set shape (and thus the column indexes) constant.
    fn opt_expr(&self, on: bool, expr: &str) -> String {
        if on {
            expr.to_string()
        } else {
            "NULL::text".to_string()
        }
    }

    /// Shared WHERE body for every relation-scoped query: relkind, namespace,
    /// extension and privilege filters. Visibility is left to Postgres itself
    /// (`has_*_privilege`) instead of hand-rolled ACL checks.
    fn rel_filter(&self) -> String {
        let mut parts = vec![
            "c.relkind IN ('r','p','v','m','f')".to_string(),
            self.nsp("n"),
            self.no_ext("pg_class", "c.oid"),
            "has_schema_privilege(n.oid, 'USAGE')".to_string(),
            // A least-privilege role often gets column grants only
            // (`GRANT SELECT (code) ON t`), and for those has_table_privilege is
            // false — the table would vanish from the dump while constraints on
            // other tables kept referencing it. has_any_column_privilege covers
            // exactly that case.
            "(has_table_privilege(c.oid, 'SELECT, INSERT, UPDATE, DELETE, \
             TRUNCATE, REFERENCES, TRIGGER') \
             OR has_any_column_privilege(c.oid, 'SELECT, INSERT, UPDATE, REFERENCES'))"
                .to_string(),
        ];
        if let Some(t) = &self.table {
            parts.push(format!("c.relname = {}", quote_literal(t)));
        }
        if !self.internal && self.table.is_none() {
            // Leaf partitions repeat the parent's columns/indexes verbatim;
            // the parent reports how many of them there are. Named explicitly,
            // a partition is what the caller asked for — see `no_ext`.
            parts.push("NOT c.relispartition".to_string());
        }
        parts.join("\n           AND ")
    }

    /// The relation(s) [`SchemaOptions::table`] resolves to, as a subquery over
    /// pg_class — plural because the same name may live in several schemas and
    /// the dump would rather show both than silently pick one.
    fn target_oids(&self) -> String {
        format!(
            "SELECT tc.oid FROM pg_class tc \
             JOIN pg_namespace tn ON tn.oid = tc.relnamespace \
             WHERE tc.relname = {name} AND {nsp}",
            name = quote_literal(self.table.as_deref().unwrap_or_default()),
            nsp = self.nsp("tn"),
        )
    }

    /// Narrows a type/routine/sequence query to what the named relation uses.
    /// Without a relation the whole schema is in scope and the predicate is a
    /// no-op, so callers can splice it in unconditionally.
    fn used_by_table(&self, kind: UsedBy) -> String {
        if self.table.is_none() {
            return "true".to_string();
        }
        let target = self.target_oids();
        match kind {
            // An array column stores the array type's oid; the enum/domain
            // behind `status[]` is its typelem, so matching atttypid alone
            // dropped exactly the types worth printing.
            UsedBy::Type(alias) => format!(
                "{alias}.oid IN (SELECT CASE WHEN bt.typelem <> 0 AND bt.typlen = -1 \
                                             THEN bt.typelem ELSE ua.atttypid END \
                                   FROM pg_attribute ua \
                                   JOIN pg_type bt ON bt.oid = ua.atttypid \
                                  WHERE ua.attnum > 0 AND NOT ua.attisdropped \
                                    AND ua.attrelid IN ({target}))"
            ),
            UsedBy::Routine(alias) => format!(
                "{alias}.oid IN (SELECT ut.tgfoid FROM pg_trigger ut \
                                  WHERE NOT ut.tgisinternal AND ut.tgrelid IN ({target}))"
            ),
            // Mirror image of the whole-database rule below: there the dump
            // prints the sequences no table owns, here only the ones this table
            // does — a column shows `nextval(…)` but not its start/increment.
            UsedBy::Sequence(alias) => format!(
                "EXISTS (SELECT 1 FROM pg_depend dep \
                          WHERE dep.classid = 'pg_class'::regclass AND dep.objid = {alias}.oid \
                            AND dep.refclassid = 'pg_class'::regclass \
                            AND dep.deptype IN ('a','i') \
                            AND dep.refobjid IN ({target}))"
            ),
        }
    }
}

/// Which catalog a [`SchemaOptions::used_by_table`] predicate is written for;
/// the payload is the query's alias for that catalog.
enum UsedBy<'a> {
    Type(&'a str),
    Routine(&'a str),
    Sequence(&'a str),
}

/// Number of result sets [`schema_dump_sql`] produces, in this order:
/// 0 relations, 1 columns, 2 constraints, 3 indexes, 4 triggers, 5 enum
/// labels, 6 routines, 7 policies, 8 sequences, 9 domains + composite types.
pub const SCHEMA_DUMP_PARTS: usize = 10;

/** Whole-database schema as one simple-query batch: a single round-trip and a
 *  single catalog snapshot instead of the per-table walk (tables → columns →
 *  indexes → ddl) an agent would otherwise do. The cost is therefore constant
 *  in the number of tables, which is the entire point of the command.
 *
 *  With [`SchemaOptions::table`] the same batch answers "everything about this
 *  one relation" — same SQL, narrower filters, so there is no second code path
 *  for the single-table case.
 *
 *  Requires PostgreSQL 12 or newer (`pg_attribute.attgenerated`); the caller
 *  checks the server version, because a batch this size fails as a whole.
 *
 *  Column layout per result set (all values arrive as text):
 *  - relations: nspname, relname, relkind, partkeydef, partition parent,
 *    partition bound, partition count, rowsecurity, comment, viewdef
 *  - columns: nspname, relname, attname, type, notnull, default, identity,
 *    generated, comment
 *  - constraints: nspname, relname, conname, contype, definition
 *  - indexes: nspname, relname, index name, unique, definition
 *  - triggers: nspname, relname, tgname, timing, events, function, enabled
 *  - enums: nspname, typname, label, comment (one row per label, sorted;
 *    label is NULL for an enum with no labels yet)
 *  - routines: nspname, proname, prokind, arguments, result, language,
 *    volatility, definition, comment
 *  - policies: nspname, relname, polname, command, permissive, roles, using,
 *    with check
 *  - sequences: nspname, relname, type, start, increment, min, max, cycle,
 *    comment
 *  - types: nspname, typname, typtype, base type (domains), constraints
 *    (domains) / attributes (composites), comment
 */
pub fn schema_dump_sql(o: &SchemaOptions) -> String {
    let rel = o.rel_filter();

    // relpartbound (`FOR VALUES FROM … TO …` / `DEFAULT`) is what makes leaf
    // partitions worth printing at all: without it nothing in the dump says
    // which partition holds which range.
    let relations = format!(
        "SELECT n.nspname, c.relname, c.relkind::text,
                pg_get_partkeydef(c.oid),
                CASE WHEN c.relispartition THEN (
                  SELECT pn.nspname || '.' || pc.relname
                    FROM pg_inherits i
                    JOIN pg_class pc ON pc.oid = i.inhparent
                    JOIN pg_namespace pn ON pn.oid = pc.relnamespace
                   WHERE i.inhrelid = c.oid LIMIT 1) END,
                CASE WHEN c.relispartition THEN pg_get_expr(c.relpartbound, c.oid) END,
                CASE WHEN c.relkind = 'p' THEN (
                  SELECT count(*)::text FROM pg_inherits i WHERE i.inhparent = c.oid) END,
                c.relrowsecurity::text,
                {comment},
                {viewdef}
           FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE {rel}
          ORDER BY n.nspname,
                   CASE c.relkind WHEN 'f' THEN 1 WHEN 'v' THEN 2 WHEN 'm' THEN 3 ELSE 0 END,
                   c.relname",
        comment = o.opt_expr(o.comments, "obj_description(c.oid, 'pg_class')"),
        viewdef = o.opt_expr(
            o.definitions,
            "CASE WHEN c.relkind IN ('v','m') THEN pg_get_viewdef(c.oid, true) END"
        ),
    );

    let columns = format!(
        "SELECT n.nspname, c.relname, a.attname,
                format_type(a.atttypid, a.atttypmod),
                a.attnotnull::text,
                pg_get_expr(d.adbin, d.adrelid),
                a.attidentity::text,
                a.attgenerated::text,
                {comment}
           FROM pg_attribute a
           JOIN pg_class c ON c.oid = a.attrelid
           JOIN pg_namespace n ON n.oid = c.relnamespace
           LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
          WHERE a.attnum > 0 AND NOT a.attisdropped
            AND {rel}
          ORDER BY n.nspname, c.relname, a.attnum",
        comment = o.opt_expr(o.comments, "col_description(a.attrelid, a.attnum)"),
    );

    // NOT NULL ('n', PG18+) is already rendered from pg_attribute.attnotnull.
    let constraints = format!(
        "SELECT n.nspname, c.relname, con.conname, con.contype::text,
                pg_get_constraintdef(con.oid, true)
           FROM pg_constraint con
           JOIN pg_class c ON c.oid = con.conrelid
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE con.contype IN ('p','u','f','c','x')
            AND {rel}
          ORDER BY n.nspname, c.relname,
                   CASE con.contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'f' THEN 2 ELSE 3 END,
                   con.conname"
    );

    // Constraint-backed indexes are skipped: they are printed as constraints.
    // The contype filter is what makes "backed by" precise — only p/u/x own
    // their index. A FOREIGN KEY merely points `conindid` at the *referenced*
    // table's index, so a laxer test dropped a plain unique index the moment
    // any FK referenced it (`conrelid = indrelid` still let a self-referencing
    // FK through), and nothing else in the dump mentioned that uniqueness.
    let indexes = format!(
        "SELECT n.nspname, c.relname, ic.relname, i.indisunique::text,
                pg_get_indexdef(i.indexrelid, 0, true)
           FROM pg_index i
           JOIN pg_class c ON c.oid = i.indrelid
           JOIN pg_class ic ON ic.oid = i.indexrelid
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE NOT EXISTS (SELECT 1 FROM pg_constraint co
                             WHERE co.conindid = i.indexrelid
                               AND co.contype IN ('p','u','x'))
            AND {rel}
          ORDER BY n.nspname, c.relname, ic.relname"
    );

    let triggers = format!(
        "SELECT n.nspname, c.relname, t.tgname,
                CASE WHEN (t.tgtype & 2) > 0 THEN 'BEFORE'
                     WHEN (t.tgtype & 64) > 0 THEN 'INSTEAD OF'
                     ELSE 'AFTER' END,
                concat_ws(' OR ',
                  CASE WHEN (t.tgtype & 4) > 0 THEN 'INSERT' END,
                  CASE WHEN (t.tgtype & 8) > 0 THEN 'DELETE' END,
                  CASE WHEN (t.tgtype & 16) > 0 THEN 'UPDATE' END,
                  CASE WHEN (t.tgtype & 32) > 0 THEN 'TRUNCATE' END),
                tn.nspname || '.' || tp.proname,
                (t.tgenabled <> 'D')::text
           FROM pg_trigger t
           JOIN pg_class c ON c.oid = t.tgrelid
           JOIN pg_namespace n ON n.oid = c.relnamespace
           JOIN pg_proc tp ON tp.oid = t.tgfoid
           JOIN pg_namespace tn ON tn.oid = tp.pronamespace
          WHERE NOT t.tgisinternal
            AND {rel}
          ORDER BY n.nspname, c.relname, t.tgname"
    );

    // LEFT JOIN, not JOIN: `CREATE TYPE … AS ENUM ()` is a legal migration
    // step, and an inner join made such a type vanish from the dump entirely.
    let enums = format!(
        "SELECT n.nspname, t.typname, e.enumlabel, {comment}
           FROM pg_type t
           JOIN pg_namespace n ON n.oid = t.typnamespace
           LEFT JOIN pg_enum e ON e.enumtypid = t.oid
          WHERE t.typtype = 'e'
            AND {nsp}
            AND {no_ext}
            AND {used}
            AND has_schema_privilege(n.oid, 'USAGE')
            AND has_type_privilege(t.oid, 'USAGE')
          ORDER BY n.nspname, t.typname, e.enumsortorder",
        comment = o.opt_expr(o.comments, "obj_description(t.oid, 'pg_type')"),
        nsp = o.nsp("n"),
        no_ext = o.no_ext("pg_type", "t.oid"),
        used = o.used_by_table(UsedBy::Type("t")),
    );

    // prokind 'a'/'w' (aggregates, window functions) are left out on purpose —
    // pg_get_functiondef errors on them and they are rarely user-authored.
    let routines = format!(
        "SELECT n.nspname, p.proname, p.prokind::text,
                pg_get_function_arguments(p.oid),
                pg_get_function_result(p.oid),
                l.lanname,
                CASE p.provolatile WHEN 'i' THEN 'immutable' WHEN 's' THEN 'stable'
                     ELSE 'volatile' END,
                {definition},
                {comment}
           FROM pg_proc p
           JOIN pg_namespace n ON n.oid = p.pronamespace
           JOIN pg_language l ON l.oid = p.prolang
          WHERE p.prokind IN ('f','p')
            AND {nsp}
            AND {no_ext}
            AND {used}
            AND has_schema_privilege(n.oid, 'USAGE')
            AND has_function_privilege(p.oid, 'EXECUTE')
          ORDER BY n.nspname, p.proname, pg_get_function_identity_arguments(p.oid)",
        definition = o.opt_expr(o.definitions, "pg_get_functiondef(p.oid)"),
        comment = o.opt_expr(o.comments, "obj_description(p.oid, 'pg_proc')"),
        nsp = o.nsp("n"),
        no_ext = o.no_ext("pg_proc", "p.oid"),
        used = o.used_by_table(UsedBy::Routine("p")),
    );

    // Without the policies themselves `rowsecurity` is a dead end: the dump
    // says reads are filtered but not by what, and the agent has to fall back
    // to the per-table walk this command exists to replace. polroles = {0} is
    // PUBLIC, which has no pg_roles row to aggregate.
    let policies = format!(
        "SELECT n.nspname, c.relname, pol.polname,
                CASE pol.polcmd WHEN 'r' THEN 'SELECT' WHEN 'a' THEN 'INSERT'
                     WHEN 'w' THEN 'UPDATE' WHEN 'd' THEN 'DELETE' ELSE 'ALL' END,
                pol.polpermissive::text,
                CASE WHEN pol.polroles = '{{0}}'::oid[] THEN 'public'
                     ELSE (SELECT string_agg(r.rolname, ', ' ORDER BY r.rolname)
                             FROM pg_roles r WHERE r.oid = ANY(pol.polroles)) END,
                pg_get_expr(pol.polqual, pol.polrelid),
                pg_get_expr(pol.polwithcheck, pol.polrelid)
           FROM pg_policy pol
           JOIN pg_class c ON c.oid = pol.polrelid
           JOIN pg_namespace n ON n.oid = c.relnamespace
          WHERE {rel}
          ORDER BY n.nspname, c.relname, pol.polname"
    );

    // Standalone sequences only: the ones behind identity/serial columns are
    // already visible on the column itself, while a `nextval('invoice_seq')`
    // called from application code had nothing to point at in the dump. Scoped
    // to one table it is the other way round (see `used_by_table`).
    // has_sequence_privilege has to sit inside a CASE — the planner may run it
    // before the relkind test, and on a non-sequence it errors out.
    let owned = match &o.table {
        Some(_) => o.used_by_table(UsedBy::Sequence("c")),
        None => "NOT EXISTS (SELECT 1 FROM pg_depend dep \
                              WHERE dep.classid = 'pg_class'::regclass AND dep.objid = c.oid \
                                AND dep.refclassid = 'pg_class'::regclass \
                                AND dep.deptype IN ('a','i'))"
            .to_string(),
    };
    let sequences = format!(
        "SELECT n.nspname, c.relname,
                format_type(s.seqtypid, NULL),
                s.seqstart::text, s.seqincrement::text,
                s.seqmin::text, s.seqmax::text, s.seqcycle::text,
                {comment}
           FROM pg_class c
           JOIN pg_namespace n ON n.oid = c.relnamespace
           JOIN pg_sequence s ON s.seqrelid = c.oid
          WHERE c.relkind = 'S'
            AND {nsp}
            AND {no_ext}
            AND has_schema_privilege(n.oid, 'USAGE')
            AND CASE WHEN c.relkind = 'S'
                     THEN has_sequence_privilege(c.oid, 'SELECT, USAGE, UPDATE')
                     ELSE false END
            AND {owned}
          ORDER BY n.nspname, c.relname",
        comment = o.opt_expr(o.comments, "obj_description(c.oid, 'pg_class')"),
        nsp = o.nsp("n"),
        no_ext = o.no_ext("pg_class", "c.oid"),
    );

    // Domains and composite types, the two remaining kinds of user-defined
    // type next to enums. Every table has a row type of typtype = 'c' too, so
    // a standalone composite is recognized by relkind = 'c' on typrelid.
    // Domain NOT NULL comes from typnotnull; PG17+ also stores it as a
    // constraint row, hence contype = 'c' — otherwise it printed twice.
    let types = format!(
        "SELECT n.nspname, t.typname, t.typtype::text,
                CASE WHEN t.typtype = 'd' THEN format_type(t.typbasetype, t.typtypmod) END,
                CASE WHEN t.typtype = 'd' THEN
                       concat_ws(' ',
                         CASE WHEN t.typnotnull THEN 'NOT NULL' END,
                         CASE WHEN t.typdefault IS NOT NULL THEN 'DEFAULT ' || t.typdefault END,
                         (SELECT string_agg(pg_get_constraintdef(co.oid, true), ' '
                                            ORDER BY co.conname)
                            FROM pg_constraint co
                           WHERE co.contypid = t.oid AND co.contype = 'c'))
                     ELSE (SELECT string_agg(a.attname || ' ' ||
                                             format_type(a.atttypid, a.atttypmod),
                                             ', ' ORDER BY a.attnum)
                             FROM pg_attribute a
                            WHERE a.attrelid = t.typrelid AND a.attnum > 0
                              AND NOT a.attisdropped) END,
                {comment}
           FROM pg_type t
           JOIN pg_namespace n ON n.oid = t.typnamespace
          WHERE (t.typtype = 'd'
                 OR (t.typtype = 'c'
                     AND EXISTS (SELECT 1 FROM pg_class rc
                                  WHERE rc.oid = t.typrelid AND rc.relkind = 'c')))
            AND {nsp}
            AND {no_ext}
            AND {used}
            AND has_schema_privilege(n.oid, 'USAGE')
            AND has_type_privilege(t.oid, 'USAGE')
          ORDER BY n.nspname, t.typtype, t.typname",
        comment = o.opt_expr(o.comments, "obj_description(t.oid, 'pg_type')"),
        nsp = o.nsp("n"),
        no_ext = o.no_ext("pg_type", "t.oid"),
        used = o.used_by_table(UsedBy::Type("t")),
    );

    [
        relations,
        columns,
        constraints,
        indexes,
        triggers,
        enums,
        routines,
        policies,
        sequences,
        types,
    ]
    .join(";\n")
}

/** Postgres has no SHOW CREATE TABLE — assemble the DDL from the catalogs:
 *  columns (types/defaults/identity), constraints, secondary indexes,
 *  partition key and comments. Views return their stored definition. */
pub async fn table_ddl(client: &Client, schema: &str, table: &str) -> Result<String, AppError> {
    let rel = format!("{}::regclass", regclass_literal(schema, table));
    let qualified = format!("{}.{}", quote_ident(schema), quote_ident(table));

    let kind = query_scalar(
        client,
        &format!("SELECT relkind::text FROM pg_class WHERE oid = {rel}"),
    )
    .await?
    .unwrap_or_default();

    if kind == "v" || kind == "m" {
        let body = query_scalar(client, &format!("SELECT pg_get_viewdef({rel}, true)"))
            .await?
            .unwrap_or_default();
        let head = if kind == "m" {
            "CREATE MATERIALIZED VIEW"
        } else {
            "CREATE OR REPLACE VIEW"
        };
        return Ok(format!("{head} {qualified} AS\n{body}"));
    }

    let cols = query_rows(
        client,
        &format!(
            "SELECT a.attname, format_type(a.atttypid, a.atttypmod), a.attnotnull::text, \
             pg_get_expr(d.adbin, d.adrelid), a.attidentity::text, a.attgenerated::text \
             FROM pg_attribute a \
             LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum \
             WHERE a.attrelid = {rel} AND a.attnum > 0 AND NOT a.attisdropped \
             ORDER BY a.attnum"
        ),
    )
    .await?;
    let mut lines: Vec<String> = Vec::new();
    for row in &cols {
        let mut line = format!("  {} {}", quote_ident(&cell(row, 0)), cell(row, 1));
        let default = row[3].as_deref();
        // A generated column keeps its expression in the default slot, so the
        // fallback arm would emit `DEFAULT (expr)` — a different table. 'v'
        // (virtual, PG18) must not be folded into 's' either: STORED and
        // VIRTUAL differ in storage and in what an index can be built on.
        match (row[5].as_deref(), row[4].as_deref()) {
            (Some(kind @ ("s" | "v")), _) => line.push_str(&format!(
                " GENERATED ALWAYS AS ({}) {}",
                default.unwrap_or_default(),
                if kind == "v" { "VIRTUAL" } else { "STORED" }
            )),
            (_, Some("a")) => line.push_str(" GENERATED ALWAYS AS IDENTITY"),
            (_, Some("d")) => line.push_str(" GENERATED BY DEFAULT AS IDENTITY"),
            _ => {
                if let Some(d) = default {
                    line.push_str(&format!(" DEFAULT {d}"));
                }
            }
        }
        if cell_bool(row, 2) {
            line.push_str(" NOT NULL");
        }
        lines.push(line);
    }

    // NOT NULL lives inline above — 'n' rows (PG18+) would duplicate it.
    let cons = query_rows(
        client,
        &format!(
            "SELECT conname, pg_get_constraintdef(oid, true) FROM pg_constraint \
             WHERE conrelid = {rel} AND contype IN ('p','u','f','c','x') \
             ORDER BY CASE contype WHEN 'p' THEN 0 WHEN 'u' THEN 1 WHEN 'f' THEN 2 ELSE 3 END, conname"
        ),
    )
    .await?;
    for row in &cons {
        lines.push(format!(
            "  CONSTRAINT {} {}",
            quote_ident(&cell(row, 0)),
            cell(row, 1)
        ));
    }

    let mut ddl = format!("CREATE TABLE {qualified} (\n{}\n)", lines.join(",\n"));
    if kind == "p" {
        if let Some(part) =
            query_scalar(client, &format!("SELECT pg_get_partkeydef({rel})")).await?
        {
            ddl.push_str(&format!(" PARTITION BY {part}"));
        }
    }
    ddl.push(';');

    // Only p/u/x constraints own an index (they are already printed above); a
    // FOREIGN KEY just points conindid at the index it references — including
    // one on this very table, when the FK is self-referencing.
    let idx = query_rows(
        client,
        &format!(
            "SELECT pg_get_indexdef(i.indexrelid, 0, true) FROM pg_index i \
             WHERE i.indrelid = {rel} \
             AND NOT EXISTS (SELECT 1 FROM pg_constraint co \
                              WHERE co.conindid = i.indexrelid \
                                AND co.contype IN ('p','u','x')) \
             ORDER BY 1"
        ),
    )
    .await?;
    for row in &idx {
        if let Some(def) = row[0].as_deref() {
            ddl.push_str(&format!("\n{def};"));
        }
    }

    if let Some(c) =
        query_scalar(client, &format!("SELECT obj_description({rel}, 'pg_class')")).await?
    {
        ddl.push_str(&format!(
            "\nCOMMENT ON TABLE {qualified} IS {};",
            quote_literal(&c)
        ));
    }
    let comments = query_rows(
        client,
        &format!(
            "SELECT a.attname, col_description(a.attrelid, a.attnum) \
             FROM pg_attribute a \
             WHERE a.attrelid = {rel} AND a.attnum > 0 AND NOT a.attisdropped \
               AND col_description(a.attrelid, a.attnum) IS NOT NULL \
             ORDER BY a.attnum"
        ),
    )
    .await?;
    for row in &comments {
        if let Some(c) = row[1].as_deref() {
            ddl.push_str(&format!(
                "\nCOMMENT ON COLUMN {qualified}.{} IS {};",
                quote_ident(&cell(row, 0)),
                quote_literal(c)
            ));
        }
    }

    Ok(ddl)
}
