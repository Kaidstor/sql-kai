import { describe, expect, it, vi } from "vitest";
import type { Get, Set } from "./context";
import type { AppStore } from "./types";

vi.mock("../api", () => ({
  api: {},
  errText: (e: unknown) => String(e),
  isSessionLost: () => false,
  isReadOnlyRefusal: (e: unknown) =>
    typeof e === "object" &&
    e !== null &&
    (e as { code?: string }).code === "read_only",
}));
vi.mock("../persist", () => ({ restoreWorkspace: () => null }));

import { createStoreContext } from "./context";

const REFUSAL = { code: "read_only", message: "read-only session" };

/** Context over a store with one profile; `confirm` — ответ диалога. */
const setup = (production: boolean, confirm = true) => {
  const confirmDialog = vi.fn(async () => confirm);
  const state = {
    profiles: [{ id: "p1", name: "prod-db", production }],
    confirmDialog,
  } as unknown as AppStore;
  const ctx = createStoreContext((() => {}) as unknown as Set, (() =>
    state) as unknown as Get);
  return { ctx, confirmDialog };
};

describe("runProdGuarded", () => {
  it("runs once without write intent when nothing is refused", async () => {
    const { ctx, confirmDialog } = setup(true);
    const attempt = vi.fn(async (w: boolean) => (w ? "rw" : "ro"));
    await expect(ctx.runProdGuarded("p1", "SELECT 1", attempt)).resolves.toBe(
      "ro",
    );
    expect(attempt).toHaveBeenCalledTimes(1);
    expect(attempt).toHaveBeenCalledWith(false);
    expect(confirmDialog).not.toHaveBeenCalled();
  });

  it("asks once and reruns with prodWrite after a read_only refusal", async () => {
    const { ctx, confirmDialog } = setup(true);
    const attempt = vi.fn(async (w: boolean) => {
      if (!w) throw REFUSAL;
      return "written";
    });
    await expect(
      ctx.runProdGuarded("p1", "UPDATE t SET x = 1", attempt),
    ).resolves.toBe("written");
    expect(confirmDialog).toHaveBeenCalledTimes(1);
    expect(attempt).toHaveBeenNthCalledWith(1, false);
    expect(attempt).toHaveBeenNthCalledWith(2, true);
  });

  it("does not rerun when the dialog is declined", async () => {
    const { ctx } = setup(true, false);
    const attempt = vi.fn(async (w: boolean) => {
      if (!w) throw REFUSAL;
      return "written";
    });
    await expect(
      ctx.runProdGuarded("p1", "DELETE FROM t", attempt),
    ).rejects.toThrow(/not confirmed/);
    expect(attempt).toHaveBeenCalledTimes(1);
  });

  it("rethrows a read_only refusal of a non-production profile", async () => {
    // настоящий read-only сервер (standby) без метки production — не повод
    // предлагать запись
    const { ctx, confirmDialog } = setup(false);
    const attempt = vi.fn(async () => {
      throw REFUSAL;
    });
    await expect(ctx.runProdGuarded("p1", "UPDATE t", attempt)).rejects.toBe(
      REFUSAL,
    );
    expect(confirmDialog).not.toHaveBeenCalled();
  });

  it("rethrows ordinary SQL errors without asking", async () => {
    const { ctx, confirmDialog } = setup(true);
    const boom = { code: "db", message: "syntax error" };
    const attempt = vi.fn(async () => {
      throw boom;
    });
    await expect(ctx.runProdGuarded("p1", "SELEC 1", attempt)).rejects.toBe(
      boom,
    );
    expect(confirmDialog).not.toHaveBeenCalled();
    expect(attempt).toHaveBeenCalledTimes(1);
  });
});
