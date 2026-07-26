import { beforeEach, describe, expect, it, vi } from "vitest";
import type { Get, Set, StoreContext } from "../context";
import type { AppStore, QueryTabState, Tab } from "../types";

// vi.mock поднимается выше объявлений модуля, поэтому фабрика не может
// сослаться на обычную const — стенд собираем через vi.hoisted.
const api = vi.hoisted(() => ({
  sqlPlaceholderCount: vi.fn(),
  queryParameters: vi.fn(),
  rememberQueryParameters: vi.fn(),
  forgetQueryParameters: vi.fn(),
  executeSql: vi.fn(),
  recordHistory: vi.fn(),
  sessionTxStatus: vi.fn(),
}));

vi.mock("../../api", () => ({
  api,
  errText: (e: unknown) => String(e),
  isSessionLost: () => false,
}));
vi.mock("../../notify", () => ({ notifyQueryDone: () => {} }));
vi.mock("../../export", () => ({ exportedMessage: () => "" }));

import { createQuerySlice } from "./query";

const TAB: Tab = {
  id: "t1",
  profileId: "p1",
  title: "Query 1",
  state: { kind: "query", sql: "", running: false, maxRows: 1000 },
};

/** Query slice over a plain state holding one connected query tab. */
const setup = () => {
  const state = {} as unknown as AppStore;
  const set = ((
    update: Partial<AppStore> | ((s: AppStore) => Partial<AppStore>),
  ) => {
    Object.assign(state, typeof update === "function" ? update(state) : update);
  }) as Set;
  const get = (() => state) as Get;
  const tab: Tab = { ...TAB, state: { ...TAB.state } };
  const qs = tab.state as QueryTabState;
  const ctx = {
    tabOf: () => tab,
    patchTab: (_id: string, patch: Record<string, unknown>) =>
      Object.assign(tab.state, patch),
    sessionFor: () => state.sessions.p1,
  } as unknown as StoreContext;
  Object.assign(state, createQuerySlice(set, get, ctx));
  state.tabs = [tab];
  state.sessions = {
    p1: { sessionId: "s1", profileId: "p1", serverVersion: "16" },
  } as AppStore["sessions"];
  state.isolatedSessions = {};
  state.exporting = {};
  state.profiles = [];
  state.history = [];
  state.showToast = () => {};
  return { state, tab, qs };
};

beforeEach(() => {
  vi.clearAllMocks();
  api.executeSql.mockResolvedValue({ results: [], durationMs: 1 });
  api.recordHistory.mockResolvedValue([]);
  api.sessionTxStatus.mockResolvedValue("idle");
  api.rememberQueryParameters.mockResolvedValue(undefined);
  api.forgetQueryParameters.mockResolvedValue(undefined);
});

describe("runQuery with placeholders", () => {
  it("opens the dialog instead of running, prefilled from the last run", async () => {
    const { state, qs } = setup();
    api.sqlPlaceholderCount.mockResolvedValue(2);
    api.queryParameters.mockResolvedValue(["a@b.c", "active"]);
    qs.sql = "SELECT 1 FROM t WHERE a = $1 AND b = $2";

    await state.runQuery("t1");

    expect(api.executeSql).not.toHaveBeenCalled();
    expect(state.paramsPrompt).toMatchObject({
      count: 2,
      values: ["a@b.c", "active"],
      remembered: true,
      action: { kind: "run" },
    });
  });

  it("asks for every placeholder even when nothing was remembered", async () => {
    const { state, qs } = setup();
    api.sqlPlaceholderCount.mockResolvedValue(3);
    api.queryParameters.mockResolvedValue([]);
    qs.sql = "SELECT $1, $2, $3";

    await state.runQuery("t1");

    expect(state.paramsPrompt?.values).toEqual(["", "", ""]);
    expect(state.paramsPrompt?.remembered).toBe(false);
  });

  it("runs straight through when the SQL names no placeholder", async () => {
    const { state, qs } = setup();
    api.sqlPlaceholderCount.mockResolvedValue(0);
    qs.sql = "SELECT 1";

    await state.runQuery("t1");

    expect(state.paramsPrompt).toBeNull();
    expect(api.executeSql).toHaveBeenCalledTimes(1);
    expect(api.queryParameters).not.toHaveBeenCalled();
  });
});

describe("submitParamsPrompt", () => {
  it("sends the values apart from the SQL and keeps them out of history", async () => {
    const { state, qs } = setup();
    api.sqlPlaceholderCount.mockResolvedValue(1);
    api.queryParameters.mockResolvedValue([]);
    const sql = "UPDATE users SET api_token = $1";
    qs.sql = sql;
    await state.runQuery("t1");

    await state.submitParamsPrompt(["ghp_secret"]);

    expect(api.executeSql).toHaveBeenCalledWith("s1", sql, 1000, false, [
      "ghp_secret",
    ]);
    // Ради этого значения и держат отдельно от текста запроса.
    expect(api.recordHistory.mock.calls[0][0].sql).toBe(sql);
    expect(api.rememberQueryParameters).toHaveBeenCalledWith("p1", sql, [
      "ghp_secret",
    ]);
    expect(qs.resultParams).toEqual(["ghp_secret"]);
    expect(state.paramsPrompt).toBeNull();
  });

  it("does not reopen the dialog for the run it just answered", async () => {
    const { state, qs } = setup();
    api.sqlPlaceholderCount.mockResolvedValue(1);
    api.queryParameters.mockResolvedValue([]);
    qs.sql = "SELECT $1";
    await state.runQuery("t1");

    await state.submitParamsPrompt(["v"]);

    expect(api.executeSql).toHaveBeenCalledTimes(1);
    expect(state.paramsPrompt).toBeNull();
  });
});

describe("forgetParamsPrompt", () => {
  it("drops the saved values and empties the fields, dialog still open", async () => {
    const { state, qs } = setup();
    api.sqlPlaceholderCount.mockResolvedValue(2);
    api.queryParameters.mockResolvedValue(["x", "y"]);
    qs.sql = "SELECT $1, $2";
    await state.runQuery("t1");

    await state.forgetParamsPrompt();

    expect(api.forgetQueryParameters).toHaveBeenCalledWith(
      "p1",
      "SELECT $1, $2",
    );
    expect(state.paramsPrompt).toMatchObject({
      values: ["", ""],
      remembered: false,
    });
  });
});
