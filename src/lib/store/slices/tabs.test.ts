import { describe, expect, it, vi } from "vitest";
import type { Set, StoreContext } from "../context";
import type { AppStore, Tab } from "../types";

// The slice reads the reopen stack once at creation; keep persistence inert.
vi.mock("../../persist", () => ({
  CLOSED_CAP: 10,
  loadClosedTabs: () => [],
  persistClosedTabs: () => {},
}));

vi.mock("../../api", () => ({ api: {} }));

import { createTabsSlice } from "./tabs";

/** Slice over a plain state with `n` tabs on profile "a", one foreign tab on "b". */
const setup = (n: number, activeIdx = 0) => {
  const state = {} as unknown as AppStore;
  const set = ((
    update: Partial<AppStore> | ((snapshot: AppStore) => Partial<AppStore>),
  ) => {
    Object.assign(state, typeof update === "function" ? update(state) : update);
  }) as Set;
  Object.assign(
    state,
    createTabsSlice(set, () => state, {} as unknown as StoreContext),
  );
  const own: Tab[] = Array.from({ length: n }, (_, i) => ({
    id: `a${i}`,
    profileId: "a",
    title: `A${i}`,
    state: { kind: "query", sql: "", running: false, maxRows: 1000 },
  }));
  state.tabs = [
    ...own,
    { id: "b0", profileId: "b", title: "B0", state: { kind: "query", sql: "", running: false, maxRows: 1000 } },
  ];
  state.activeProfileId = "a";
  state.activeTabId = `a${activeIdx}`;
  return state;
};

describe("cycleTab", () => {
  it("wraps forward and backward within the active connection's tabs", () => {
    const state = setup(3);
    state.cycleTab(1);
    expect(state.activeTabId).toBe("a1");
    state.cycleTab(1);
    state.cycleTab(1);
    expect(state.activeTabId).toBe("a0"); // wrapped past the last tab
    state.cycleTab(-1);
    expect(state.activeTabId).toBe("a2"); // wrapped before the first tab
  });

  it("never lands on another connection's tab", () => {
    const state = setup(2, 1);
    state.cycleTab(1);
    expect(state.activeTabId).toBe("a0"); // b0 skipped, wraps to first own tab
  });

  it("is a no-op when the active connection has no tabs", () => {
    const state = setup(0);
    state.activeTabId = null;
    state.cycleTab(1);
    expect(state.activeTabId).toBeNull();
  });
});
