import { describe, expect, it } from "vitest";
import {
  alterPolicySql,
  createFkSql,
  createIndexSql,
  createPolicySql,
  createTriggerSql,
  dropConstraintSql,
  dropPolicySql,
  dropTriggerSql,
  identList,
  renameConstraintSql,
  renameIndexSql,
  renamePolicySql,
  renameTriggerSql,
  setRlsSql,
  setTriggerEnabledSql,
} from "./ddl";

describe("identList", () => {
  it("quotes plain identifiers, keeps expressions raw", () => {
    expect(identList("a, b")).toBe('"a", "b"');
    expect(identList("lower(name), a")).toBe('lower(name), "a"');
    expect(identList(" a , ")).toBe('"a"');
  });
});

describe("indexes", () => {
  it("builds CREATE INDEX", () => {
    expect(
      createIndexSql("public", "users", {
        name: "",
        columns: "email",
        unique: true,
        method: "btree",
      }),
    ).toBe('CREATE UNIQUE INDEX ON "public"."users" ("email")');
    expect(
      createIndexSql("app", "docs", {
        name: "docs_body_gin",
        columns: "to_tsvector('simple', body)",
        unique: false,
        method: "gin",
      }),
    ).toBe(
      'CREATE INDEX "docs_body_gin" ON "app"."docs" USING gin (to_tsvector(\'simple\', body))',
    );
  });

  it("builds ALTER INDEX RENAME", () => {
    expect(renameIndexSql("public", "old_ix", "new_ix")).toBe(
      'ALTER INDEX "public"."old_ix" RENAME TO "new_ix"',
    );
  });
});

describe("foreign keys", () => {
  it("builds ADD CONSTRAINT ... FOREIGN KEY", () => {
    expect(
      createFkSql("public", "orders", {
        name: "orders_user_fk",
        columns: "user_id",
        refSchema: "public",
        refTable: "users",
        refColumns: "id",
        onUpdate: "NO ACTION",
        onDelete: "CASCADE",
      }),
    ).toBe(
      'ALTER TABLE "public"."orders" ADD CONSTRAINT "orders_user_fk" ' +
        'FOREIGN KEY ("user_id") REFERENCES "public"."users" ("id") ON DELETE CASCADE',
    );
  });

  it("omits optional name and ref columns", () => {
    expect(
      createFkSql("public", "orders", {
        name: "",
        columns: "user_id",
        refSchema: "public",
        refTable: "users",
        refColumns: "",
        onUpdate: "NO ACTION",
        onDelete: "NO ACTION",
      }),
    ).toBe(
      'ALTER TABLE "public"."orders" ADD FOREIGN KEY ("user_id") REFERENCES "public"."users"',
    );
  });

  it("drop / rename constraint", () => {
    expect(dropConstraintSql("public", "orders", "fk")).toBe(
      'ALTER TABLE "public"."orders" DROP CONSTRAINT "fk"',
    );
    expect(renameConstraintSql("public", "orders", "a", "b")).toBe(
      'ALTER TABLE "public"."orders" RENAME CONSTRAINT "a" TO "b"',
    );
  });
});

describe("triggers", () => {
  it("builds CREATE TRIGGER and appends () to the function", () => {
    expect(
      createTriggerSql("public", "users", {
        name: "trg",
        timing: "BEFORE",
        events: ["INSERT", "UPDATE"],
        level: "ROW",
        fn: "touch_updated_at",
      }),
    ).toBe(
      'CREATE TRIGGER "trg" BEFORE INSERT OR UPDATE ON "public"."users" ' +
        "FOR EACH ROW EXECUTE FUNCTION touch_updated_at()",
    );
  });

  it("drop / rename / enable / disable", () => {
    expect(dropTriggerSql("public", "users", "trg")).toBe(
      'DROP TRIGGER "trg" ON "public"."users"',
    );
    expect(renameTriggerSql("public", "users", "a", "b")).toBe(
      'ALTER TRIGGER "a" ON "public"."users" RENAME TO "b"',
    );
    expect(setTriggerEnabledSql("public", "users", "trg", false)).toBe(
      'ALTER TABLE "public"."users" DISABLE TRIGGER "trg"',
    );
    expect(setTriggerEnabledSql("public", "users", "trg", true)).toBe(
      'ALTER TABLE "public"."users" ENABLE TRIGGER "trg"',
    );
  });
});

describe("policies", () => {
  it("builds CREATE POLICY with all clauses", () => {
    expect(
      createPolicySql("public", "docs", {
        name: "docs_own",
        command: "SELECT",
        permissive: true,
        roles: "app_user, public",
        using: "owner_id = current_user_id()",
        check: "",
      }),
    ).toBe(
      'CREATE POLICY "docs_own" ON "public"."docs" FOR SELECT ' +
        'TO "app_user", PUBLIC USING (owner_id = current_user_id())',
    );
  });

  it("restrictive ALL policy with check", () => {
    expect(
      createPolicySql("public", "docs", {
        name: "p",
        command: "ALL",
        permissive: false,
        roles: "",
        using: "true",
        check: "true",
      }),
    ).toBe(
      'CREATE POLICY "p" ON "public"."docs" AS RESTRICTIVE USING (true) WITH CHECK (true)',
    );
  });

  it("alter / drop / rename / rls toggle", () => {
    expect(
      alterPolicySql("public", "docs", "p", { using: "owner = 'me'" }),
    ).toBe(`ALTER POLICY "p" ON "public"."docs" USING (owner = 'me')`);
    expect(alterPolicySql("public", "docs", "p", { roles: "" })).toBe(
      'ALTER POLICY "p" ON "public"."docs" TO PUBLIC',
    );
    expect(alterPolicySql("public", "docs", "p", {})).toBeNull();
    expect(dropPolicySql("public", "docs", "p")).toBe(
      'DROP POLICY "p" ON "public"."docs"',
    );
    expect(renamePolicySql("public", "docs", "a", "b")).toBe(
      'ALTER POLICY "a" ON "public"."docs" RENAME TO "b"',
    );
    expect(setRlsSql("public", "docs", true)).toBe(
      'ALTER TABLE "public"."docs" ENABLE ROW LEVEL SECURITY',
    );
  });
});
