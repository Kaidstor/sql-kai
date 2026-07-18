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
  disconnectSession: vi.fn(async () => {}),
}));

vi.mock("../../api", () => ({
  api: {
    connectProfile: mocks.connectProfile,
    listTables: mocks.listTables,
    listAllColumns: mocks.listAllColumns,
    disconnectSession: mocks.disconnectSession,
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

/** Slice wired over a plain state object with `n` live connections, first active. */
const setup = (n: number, activeIdx = 0) => {
  const state = { tabs: [], activeTabId: null, launcherOpen: false } as unknown as AppStore;
  const set = ((
    update: Partial<AppStore> | ((snapshot: AppStore) => Partial<AppStore>),
  ) => {
    Object.assign(state, typeof update === "function" ? update(state) : update);
  }) as Set;
  Object.assign(
    state,
    createConnectionsSlice(set, () => state, {} as unknown as StoreContext),
  );
  state.profiles = Array.from({ length: n }, (_, i) => ({
    id: `p${i}`,
    name: `P${i}`,
    host: "localhost",
    port: 5432,
    database: "db",
    user: "u",
  }));
  state.sessions = Object.fromEntries(
    state.profiles.map((p) => [
      p.id,
      { sessionId: `s-${p.id}`, profileId: p.id, serverVersion: "17" },
    ]),
  );
  state.activeProfileId = `p${activeIdx}`;
  return state;
};

describe("cycleProfile", () => {
  it("wraps forward and backward through live connections", () => {
    const state = setup(3);
    state.cycleProfile(1);
    expect(state.activeProfileId).toBe("p1");
    state.cycleProfile(1);
    state.cycleProfile(1);
    expect(state.activeProfileId).toBe("p0"); // wrapped past the end
    state.cycleProfile(-1);
    expect(state.activeProfileId).toBe("p2"); // wrapped before the start
  });

  it("is a no-op with fewer than two live connections", () => {
    const state = setup(1);
    state.cycleProfile(1);
    expect(state.activeProfileId).toBe("p0");
  });
});

describe("disconnect", () => {
  const queryTab = (id: string, profileId: string) =>
    ({ id, profileId, title: id, state: { kind: "query" } }) as AppStore["tabs"][number];

  it("promotes the neighbouring live connection when the active one goes", async () => {
    const state = setup(3, 1);
    await state.disconnect("p1");
    expect(state.sessions.p1).toBeUndefined();
    expect(state.activeProfileId).toBe("p2"); // took p1's slot in switcher order
  });

  it("clamps to the last live connection when the active one was last", async () => {
    const state = setup(2, 1);
    await state.disconnect("p1");
    expect(state.activeProfileId).toBe("p0");
  });

  it("clears the active profile when the only connection goes", async () => {
    const state = setup(1);
    await state.disconnect("p0");
    expect(state.activeProfileId).toBeNull();
    expect(state.activeTabId).toBeNull();
  });

  it("keeps the active profile when another connection disconnects", async () => {
    const state = setup(3, 0);
    state.tabs = [queryTab("t0", "p0")];
    state.activeTabId = "t0";
    await state.disconnect("p2");
    expect(state.activeProfileId).toBe("p0");
    expect(state.activeTabId).toBe("t0");
  });

  it("brings the promoted connection's most recent tab forward", async () => {
    const state = setup(2, 0);
    state.tabs = [queryTab("t0", "p0"), queryTab("t1", "p1"), queryTab("t2", "p1")];
    state.activeTabId = "t0";
    await state.disconnect("p0");
    expect(state.activeProfileId).toBe("p1");
    expect(state.activeTabId).toBe("t2");
    expect(state.tabs.map((t) => t.id)).toEqual(["t1", "t2"]);
  });
});
