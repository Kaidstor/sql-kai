import { describe, expect, it, vi } from "vitest";
import type { AppStore } from "../types";
import type { Set, StoreContext } from "../context";

const mocks = vi.hoisted(() => ({
  connectProfile: vi.fn(async (profileId: string) => ({
    sessionId: `session-${profileId}`,
    profileId,
    serverVersion: "PostgreSQL 17",
  })),
  listTables: vi.fn(async () => []),
  listAllColumns: vi.fn(async () => []),
}));

vi.mock("../../api", () => ({
  api: {
    connectProfile: mocks.connectProfile,
    listTables: mocks.listTables,
    listAllColumns: mocks.listAllColumns,
  },
  errText: (error: unknown) =>
    error instanceof Error ? error.message : String(error),
}));

import { createConnectionsSlice } from "./connections";

describe("background connection recovery", () => {
  it("does not activate a recovered profile just to create its default tab", async () => {
    const openQueryTab = vi.fn();
    const restoreProfileTabs = vi.fn(() => false);
    const state = {
      tabs: [],
      activeProfileId: "foreground",
      launcherOpen: false,
      openQueryTab,
      showToast: vi.fn(),
    } as unknown as AppStore;
    const set = ((
      update: Partial<AppStore> | ((snapshot: AppStore) => Partial<AppStore>),
    ) => {
      Object.assign(state, typeof update === "function" ? update(state) : update);
    }) as Set;
    const context = {
      restoreProfileTabs,
      handleSqlError: (_profileId: string, error: unknown) => String(error),
    } as unknown as StoreContext;
    Object.assign(state, createConnectionsSlice(set, () => state, context));
    state.profiles = [
      {
        id: "background",
        name: "Background",
        host: "localhost",
        port: 5432,
        database: "db",
        user: "test",
      },
    ];
    state.activeProfileId = "foreground";

    await state.connect("background", false);

    expect(state.sessions.background).toBeDefined();
    expect(restoreProfileTabs).toHaveBeenCalledWith("background");
    expect(openQueryTab).not.toHaveBeenCalled();
    expect(state.activeProfileId).toBe("foreground");
  });
});
