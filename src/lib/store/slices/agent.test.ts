import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { AppStore } from "../types";
import type { Set, StoreContext } from "../context";

interface FakeHandlers {
  onExit: (code: number | null) => void;
  onSessionUpdate: (update: unknown) => void;
}

interface FakeAgentRecord {
  handlers: FakeHandlers;
  killed: boolean;
  prompts: string[];
  finish: (reason?: string) => void;
  emitExit: (code?: number | null) => void;
}

const mocks = vi.hoisted(() => ({
  agents: [] as FakeAgentRecord[],
  saveSettings: vi.fn(async () => {}),
}));

vi.mock("@tauri-apps/api/path", () => ({
  appDataDir: async () => "/tmp/sql-kai",
  join: async (...parts: string[]) => parts.join("/"),
}));

vi.mock("../../api", () => ({
  api: {
    saveSettings: mocks.saveSettings,
    cliBinPath: async () => "/tmp/sql-kai-cli",
  },
  errText: (error: unknown) =>
    error instanceof Error ? error.message : String(error),
}));

vi.mock("../../acp", () => {
  class FakeRpcError extends Error {
    constructor(
      public code: number,
      message: string,
    ) {
      super(message);
    }
  }

  class FakeAcpAgent implements FakeAgentRecord {
    alive = true;
    killed = false;
    prompts: string[] = [];
    private resolvePrompt?: (reason: string) => void;
    private rejectPrompt?: (error: Error) => void;

    constructor(readonly handlers: FakeHandlers) {}

    static async spawn(
      _cmd: string,
      _args: string[],
      _cwd: string,
      handlers: FakeHandlers,
    ) {
      const agent = new FakeAcpAgent(handlers);
      mocks.agents.push(agent);
      return agent;
    }

    async initialize() {}

    async newSession() {
      return `session-${mocks.agents.length}`;
    }

    prompt(_sessionId: string, text: string) {
      this.prompts.push(text);
      return new Promise<string>((resolve, reject) => {
        this.resolvePrompt = resolve;
        this.rejectPrompt = reject;
      });
    }

    cancel() {}

    kill() {
      this.killed = true;
    }

    finish(reason = "end_turn") {
      this.resolvePrompt?.(reason);
    }

    emitExit(code: number | null = 0) {
      this.alive = false;
      this.rejectPrompt?.(new FakeRpcError(0, "agent exited"));
      this.handlers.onExit(code);
    }
  }

  return {
    AcpAgent: FakeAcpAgent,
    RpcError: FakeRpcError,
    AUTH_REQUIRED: -32000,
    withTimeout: <T,>(p: Promise<T>) => p,
  };
});

vi.mock("../../agentInstall", () => ({
  ensureAdapter: async (_id: string, a: { bin: string }) =>
    `/tmp/sql-kai/acp/${a.bin}`,
}));

import { loadAgentChats } from "../../persist";
import { createAgentSlice } from "./agent";

// сохранение диалогов идёт в localStorage (lib/persist) — стаб как в persist.test
const backing = new Map<string, string>();
beforeEach(() => {
  backing.clear();
  globalThis.localStorage = {
    getItem: (k: string) => backing.get(k) ?? null,
    setItem: (k: string, v: string) => void backing.set(k, v),
    removeItem: (k: string) => void backing.delete(k),
    clear: () => backing.clear(),
    key: () => null,
    length: 0,
  } as Storage;
});

let currentState: AppStore | undefined;

function harness() {
  const state = {
    settings: {},
    agentChats: {},
    profiles: [
      {
        id: "a",
        name: "A",
        host: "localhost",
        port: 5432,
        database: "a",
        user: "test",
      },
      {
        id: "b",
        name: "B",
        host: "localhost",
        port: 5432,
        database: "b",
        user: "test",
      },
    ],
    showToast: vi.fn(),
  } as unknown as AppStore;
  const set = ((
    update: Partial<AppStore> | ((snapshot: AppStore) => Partial<AppStore>),
  ) => {
    Object.assign(state, typeof update === "function" ? update(state) : update);
  }) as Set;
  Object.assign(
    state,
    createAgentSlice(set, () => state, {} as StoreContext),
  );
  currentState = state;
  return state;
}

async function waitUntilRunning(state: AppStore, profileId: string) {
  await vi.waitFor(() => {
    expect(state.agentChats[profileId]?.status).toBe("running");
  });
}

afterEach(() => {
  if (currentState) {
    for (const profileId of Object.keys(currentState.agentChats)) {
      currentState.resetAgentChat(profileId);
    }
  }
  currentState = undefined;
  mocks.agents.length = 0;
  mocks.saveSettings.mockClear();
});

describe("agent process lifecycle", () => {
  it("ignores a delayed exit from the process replaced by a new chat", async () => {
    const state = harness();
    const firstTurn = state.sendAgentPrompt("a", "first");
    await waitUntilRunning(state, "a");
    const oldAgent = mocks.agents[0];

    state.resetAgentChat("a");
    const secondTurn = state.sendAgentPrompt("a", "second");
    await vi.waitFor(() => expect(mocks.agents).toHaveLength(2));
    await waitUntilRunning(state, "a");

    oldAgent.emitExit();
    await firstTurn;
    expect(state.agentChats.a.status).toBe("running");
    expect(state.agentChats.a.error).toBeUndefined();

    mocks.agents[1].finish();
    await secondTurn;
    expect(state.agentChats.a.status).toBe("ready");
  });

  it("drops agents and chats for every profile when provider changes", async () => {
    const state = harness();
    const firstTurn = state.sendAgentPrompt("a", "first");
    await waitUntilRunning(state, "a");
    mocks.agents[0].finish();
    await firstTurn;

    const secondTurn = state.sendAgentPrompt("b", "second");
    await waitUntilRunning(state, "b");
    mocks.agents[1].finish();
    await secondTurn;

    await state.setAgentProvider("gemini");

    expect(mocks.agents.every((agent) => agent.killed)).toBe(true);
    expect(state.agentChats).toEqual({});
    expect(state.settings.agentProvider).toBe("gemini");
  });
});

describe("saved chats", () => {
  it("saves on reset and resumes with a transcript digest for the new process", async () => {
    const state = harness();
    const firstTurn = state.sendAgentPrompt("a", "what tables exist?");
    await waitUntilRunning(state, "a");
    mocks.agents[0].handlers.onSessionUpdate({
      sessionUpdate: "agent_message_chunk",
      content: { type: "text", text: "users, orders" },
    });
    mocks.agents[0].finish();
    await firstTurn;

    state.resetAgentChat("a");
    expect(state.agentChats.a).toBeUndefined();
    const saved = loadAgentChats("a");
    expect(saved).toHaveLength(1);
    expect(saved[0].title).toBe("what tables exist?");

    state.restoreAgentChat("a", saved[0].id);
    expect(state.agentChats.a.status).toBe("ready");
    expect(state.agentChats.a.items.map((i) => i.kind)).toEqual([
      "user",
      "assistant",
    ]);

    const secondTurn = state.sendAgentPrompt("a", "and views?");
    await waitUntilRunning(state, "a");
    const prompt = mocks.agents[1].prompts[0];
    expect(prompt).toContain("Earlier conversation");
    expect(prompt).toContain("User: what tables exist?");
    expect(prompt).toContain("Assistant: users, orders");
    expect(prompt).toContain("User request:\nand views?");
    mocks.agents[1].finish();
    await secondTurn;
    expect(state.agentChats.a.status).toBe("ready");
  });
});

describe("agent panel toggle", () => {
  it("opens only over a live connection and always closes", () => {
    const state = harness();

    // нет активного профиля — панель не открывается
    state.toggleAgentPanel();
    expect(state.agentOpen).toBe(false);

    // профиль выбран, но сессии нет (launcher / lost) — тоже нет
    Object.assign(state, { activeProfileId: "a", sessions: {} });
    state.toggleAgentPanel();
    expect(state.agentOpen).toBe(false);

    Object.assign(state, { sessions: { a: { sessionId: "s1" } } });
    state.toggleAgentPanel();
    expect(state.agentOpen).toBe(true);

    // закрытие работает всегда, даже если соединение уже умерло
    Object.assign(state, { sessions: {} });
    state.toggleAgentPanel();
    expect(state.agentOpen).toBe(false);
  });
});
