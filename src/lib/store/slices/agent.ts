// AI agent chat (ACP): one chat per profile, backed by an external agent
// process (Claude Code / Gemini / Codex / custom) spawned through acp.rs.
// The agent reaches the database itself via the sql-kai CLI — the first
// prompt of a session carries the connection context (profile alias, CLI
// usage, read-only note). Chats are ephemeral: they die with the app.
import { appDataDir, join } from "@tauri-apps/api/path";
import {
  AcpAgent,
  AUTH_REQUIRED,
  RpcError,
  withTimeout,
  type PermissionOption,
  type SessionUpdate,
  type ToolCallUpdate,
} from "../../acp";
import { ensureAdapter, NODE_NET_ENV, type NpmAdapter } from "../../agentInstall";
import {
  normalizeToolResult,
  type AgentToolDiff,
  type AgentToolOutput,
} from "../../agentTool";
import { api, errText } from "../../api";
import type { Profile } from "../../types";
import type { Get, Set, StoreContext } from "../context";

export interface AgentProvider {
  id: string;
  label: string;
  /** npm-адаптер: ставится один раз в appData и запускается с диска. */
  npm?: NpmAdapter;
  /** Бинарь, который пользователь ставит сам (cursor-agent). Без npm/cmd —
   *  провайдер custom: команда целиком из настроек. */
  cmd?: string;
  /** Аргументы бинаря адаптера (например `--acp` у gemini). */
  binArgs?: string[];
  /** Shown when the agent answers auth_required (-32000). */
  authHint: string;
}

/** Основные провайдеры — официальные ACP-адаптеры (пины версий бампаются
 *  релизами sql-kai); кастомная команда покрывает всё остальное (любой
 *  агент, говорящий ACP по stdio). */
export const AGENT_PROVIDERS: AgentProvider[] = [
  {
    id: "claude",
    label: "Claude Code",
    npm: {
      pkg: "@agentclientprotocol/claude-agent-acp",
      version: "0.59.0",
      bin: "claude-agent-acp",
    },
    authHint:
      "Log in once in a terminal: run `claude` → /login (or set ANTHROPIC_API_KEY).",
  },
  {
    id: "gemini",
    label: "Gemini CLI",
    npm: { pkg: "@google/gemini-cli", version: "0.50.0", bin: "gemini" },
    binArgs: ["--acp"],
    authHint:
      "Log in once in a terminal: run `gemini` and complete the Google login (or set GEMINI_API_KEY).",
  },
  {
    id: "codex",
    label: "Codex",
    npm: {
      pkg: "@agentclientprotocol/codex-acp",
      version: "1.1.2",
      bin: "codex-acp",
    },
    authHint:
      "Log in once in a terminal: run `codex login` (or set OPENAI_API_KEY).",
  },
  {
    // Первопартийный ACP у Cursor: `cursor-agent acp` (бинарь ставится
    // их инсталлером, npm-пакета нет — берём с PATH).
    id: "cursor",
    label: "Cursor",
    cmd: "cursor-agent",
    binArgs: ["acp"],
    authHint:
      "Log in once in a terminal: run `cursor-agent login` (or set CURSOR_API_KEY). Old CLI builds lack `acp` — run `cursor-agent update`.",
  },
  {
    id: "custom",
    label: "Custom…",
    authHint: "The agent requires authentication — check its own CLI login.",
  },
];

export interface AgentTextItem {
  kind: "user" | "assistant" | "thought";
  text: string;
}

export interface AgentToolItem {
  kind: "tool";
  toolCallId: string;
  title: string;
  toolKind: string;
  status: "pending" | "in_progress" | "completed" | "failed";
  /** Аргументы вызова как их прислал адаптер (SQL, команда, таблица…). */
  rawInput?: unknown;
  /** Нормализованный результат (lib/agentTool): таблица или текст, с капом. */
  output?: AgentToolOutput;
  /** diff-блоки ACP-контента (файловые правки агента). */
  diffs?: AgentToolDiff[];
}

export interface AgentPlanItem {
  kind: "plan";
  entries: { content: string; status: string }[];
}

export type AgentChatItemBody = AgentTextItem | AgentToolItem | AgentPlanItem;
export type AgentChatItem = AgentChatItemBody & { id: number };

export interface AgentPermission {
  title: string;
  toolKind?: string;
  /** Команда (execute) или SQL (sql-kai-тулы) из rawInput, когда есть. */
  detail?: string;
  options: PermissionOption[];
}

export interface AgentChat {
  providerId: string;
  /** starting: процесс/сессия поднимаются; running: идёт prompt turn. */
  status: "starting" | "ready" | "running" | "error";
  items: AgentChatItem[];
  permission: AgentPermission | null;
  error?: string;
  /** Что происходит внутри "starting" (npm-прогресс установки адаптера). */
  startNote?: string;
  /** Хвост stderr процесса — показывается при смерти агента. */
  stderr: string[];
}

export interface AgentSlice {
  /** Правая панель агента (⌘J). */
  agentOpen: boolean;
  /** Чаты по profileId (эфемерные, не персистятся). */
  agentChats: Record<string, AgentChat>;

  toggleAgentPanel: () => void;
  /** Персистит выбор провайдера; живой чат активного профиля сбрасывается. */
  setAgentProvider: (id: string) => Promise<void>;
  setAgentCustomCmd: (cmd: string) => Promise<void>;
  sendAgentPrompt: (profileId: string, text: string) => Promise<void>;
  /** Останавливает текущий prompt turn (session/cancel). */
  cancelAgentPrompt: (profileId: string) => void;
  /** Ответ на session/request_permission; null = cancelled. */
  answerAgentPermission: (profileId: string, optionId: string | null) => void;
  /** Убивает процесс агента и очищает чат профиля. */
  resetAgentChat: (profileId: string) => void;
}

/** Живые процессы агентов по profileId — вне стора (несериализуемые). */
const liveAgents = new Map<
  string,
  {
    agent: AcpAgent;
    sessionId: string;
    contextSent: boolean;
    generation: symbol;
    /** Сессии передан MCP-сервер sql-kai (structured tools). */
    mcp: boolean;
  }
>();
/** Spawned process whose initialize/session-new handshake is still running. */
const startingAgents = new Map<string, AcpAgent>();
/** Generation of the process currently allowed to publish events for a
 * profile. Killing an agent is asynchronous, so its final stdout/exit events
 * may arrive after a replacement has already started. */
const agentGenerations = new Map<string, symbol>();
/** Открытые permission-запросы: резолвер ответа пользователя. */
const permResolvers = new Map<string, (optionId: string | null) => void>();

let nextItemId = 1;

class AgentSupersededError extends Error {}

/** Контекст подключения, добавляемый к первому сообщению сессии.
 *  `mcp` — true, когда сессии передан MCP-сервер sql-kai (structured tools). */
function connectionContext(p: Profile, mcp: boolean): string {
  const alias = JSON.stringify(p.name); // кавычки на случай пробелов в имени
  const viaMcp = [
    "Use the MCP tools of the `sql-kai` server to work with the database:",
    "  query / tables / columns / ddl / indexes — SQL and schema discovery;",
    "  open_table / open_query — show a table or a prepared query to the user as a tab in the sql-kai GUI;",
    "  selection — what the user currently sees in the GUI: active tab (table/filter or query SQL) and the rows/columns/cells they selected, with data. Call it whenever the user refers to what's on their screen (\"this row\", \"the selected rows\", \"эта колонка\").",
    "The sql-kai CLI is also available in the shell as a fallback:",
    `  sql-kai q ${alias} -c "SELECT ..." --json`,
  ];
  const viaCli = [
    "Run SQL through the locally installed sql-kai CLI (it owns connection, ssh tunnel and credentials):",
    `  sql-kai q ${alias} -c "SELECT ..." --json`,
    "Schema discovery:",
    `  sql-kai tables ${alias}`,
    `  sql-kai columns ${alias} [schema.]table`,
    `  sql-kai ddl ${alias} [schema.]table`,
    `  sql-kai indexes ${alias} [schema.]table`,
  ];
  return [
    "You are a database assistant embedded in sql-kai, a Postgres GUI client.",
    `The user is looking at the database profile ${alias} — ${p.database} at ${p.host}:${p.port}, user ${p.user}.` +
      (p.production ? " This is a PRODUCTION database: be extra careful." : ""),
    "",
    ...(mcp ? viaMcp : viaCli),
    "Notes:",
    "- The session is READ-ONLY by default. Enable writes only when the user explicitly asks to modify data, and show them the statement first.",
    "- Row counts are capped by default; prefer JSON output.",
    "- If the sql-kai command is not found, ask the user to run “Install CLI…” from the sql-kai application menu.",
  ].join("\n");
}

/** Разбор кастомной команды: split по пробелам (без shell-квотинга). */
export function parseCustomCmd(cmd: string): { cmd: string; args: string[] } | null {
  const parts = cmd.trim().split(/\s+/).filter(Boolean);
  if (parts.length === 0) return null;
  return { cmd: parts[0], args: parts.slice(1) };
}

export function activeProvider(settings: {
  agentProvider?: string;
}): AgentProvider {
  return (
    AGENT_PROVIDERS.find((p) => p.id === settings.agentProvider) ??
    AGENT_PROVIDERS[0]
  );
}

export function createAgentSlice(set: Set, get: Get, _ctx: StoreContext): AgentSlice {
  const patchChat = (profileId: string, patch: Partial<AgentChat>) =>
    set((s) => {
      const chat = s.agentChats[profileId];
      if (!chat) return {};
      return { agentChats: { ...s.agentChats, [profileId]: { ...chat, ...patch } } };
    });

  const pushItem = (profileId: string, item: AgentChatItemBody) =>
    set((s) => {
      const chat = s.agentChats[profileId];
      if (!chat) return {};
      return {
        agentChats: {
          ...s.agentChats,
          [profileId]: {
            ...chat,
            items: [...chat.items, { ...item, id: nextItemId++ } as AgentChatItem],
          },
        },
      };
    });

  /** Стрим-чанки текста клеятся к последнему элементу той же роли. */
  const appendText = (profileId: string, kind: "assistant" | "thought", text: string) =>
    set((s) => {
      const chat = s.agentChats[profileId];
      if (!chat) return {};
      const items = [...chat.items];
      const last = items[items.length - 1];
      if (last && last.kind === kind) {
        items[items.length - 1] = { ...last, text: last.text + text };
      } else {
        items.push({ kind, text, id: nextItemId++ });
      }
      return { agentChats: { ...s.agentChats, [profileId]: { ...chat, items } } };
    });

  const applyUpdate = (profileId: string, update: SessionUpdate) => {
    switch (update.sessionUpdate) {
      case "agent_message_chunk":
        if (update.content.type === "text" && update.content.text) {
          appendText(profileId, "assistant", update.content.text);
        }
        return;
      case "agent_thought_chunk":
        if (update.content.type === "text" && update.content.text) {
          appendText(profileId, "thought", update.content.text);
        }
        return;
      case "tool_call": {
        const { output, diffs } = normalizeToolResult(update);
        pushItem(profileId, {
          kind: "tool",
          toolCallId: update.toolCallId,
          title: update.title ?? update.toolCallId,
          toolKind: update.kind ?? "other",
          status: update.status ?? "pending",
          rawInput: update.rawInput,
          output,
          diffs,
        });
        return;
      }
      case "tool_call_update":
        set((s) => {
          const chat = s.agentChats[profileId];
          if (!chat) return {};
          // результат обычно приезжает частями (in_progress → completed
          // с content/rawOutput) — поля мержатся, а не затираются
          const { output, diffs } = normalizeToolResult(update);
          const items = chat.items.map((it) =>
            it.kind === "tool" && it.toolCallId === update.toolCallId
              ? {
                  ...it,
                  status: update.status ?? it.status,
                  title: update.title ?? it.title,
                  rawInput: update.rawInput ?? it.rawInput,
                  output: output ?? it.output,
                  diffs: diffs ?? it.diffs,
                }
              : it,
          );
          return { agentChats: { ...s.agentChats, [profileId]: { ...chat, items } } };
        });
        return;
      case "plan":
        set((s) => {
          const chat = s.agentChats[profileId];
          if (!chat) return {};
          const entries = update.entries.map((e) => ({
            content: e.content,
            status: e.status ?? "pending",
          }));
          const items = [...chat.items];
          // план присылается целиком — обновляем последний план на месте,
          // чтобы чат не засорялся его копиями
          let lastPlan = -1;
          for (let i = items.length - 1; i >= 0; i--) {
            if (items[i].kind === "plan") {
              lastPlan = i;
              break;
            }
          }
          if (lastPlan >= 0) {
            items[lastPlan] = { id: items[lastPlan].id, kind: "plan", entries };
          } else {
            items.push({ kind: "plan", entries, id: nextItemId++ });
          }
          return { agentChats: { ...s.agentChats, [profileId]: { ...chat, items } } };
        });
        return;
      default:
        // user_message_chunk (своё сообщение уже показано), current_mode_update,
        // available_commands_update и будущие варианты — молча пропускаем
        return;
    }
  };

  /** Описание permission-запроса для карточки в панели. */
  const permissionOf = (toolCall: ToolCallUpdate): Omit<AgentPermission, "options"> => {
    const raw = toolCall.rawInput as
      | { command?: unknown; sql?: unknown }
      | undefined;
    const detail =
      typeof raw?.command === "string"
        ? raw.command
        : typeof raw?.sql === "string"
          ? raw.sql
          : undefined;
    return {
      title: toolCall.title ?? "Tool call",
      toolKind: toolCall.kind,
      detail,
    };
  };

  const dropAgent = (profileId: string) => {
    const live = liveAgents.get(profileId);
    const starting = startingAgents.get(profileId);
    liveAgents.delete(profileId);
    startingAgents.delete(profileId);
    agentGenerations.delete(profileId);
    permResolvers.get(profileId)?.(null);
    permResolvers.delete(profileId);
    live?.agent.kill();
    starting?.kill();
  };

  /** Живой агент профиля; поднимает процесс и ACP-сессию при необходимости. */
  const ensureAgent = async (profileId: string) => {
    const existing = liveAgents.get(profileId);
    if (existing?.agent.alive) return existing;

    const s = get();
    const provider = activeProvider(s.settings);

    patchChat(profileId, { status: "starting", error: undefined, startNote: undefined });
    const generation = Symbol(profileId);
    agentGenerations.set(profileId, generation);
    const isCurrent = () => agentGenerations.get(profileId) === generation;

    let cmd: string;
    let args: string[];
    if (provider.npm) {
      // управляемая установка: один раз в appData, дальше старт с диска
      // без похода в npm registry
      cmd = await ensureAdapter(provider.id, provider.npm, (line) => {
        if (isCurrent()) patchChat(profileId, { startNote: line });
      });
      args = provider.binArgs ?? [];
      if (!isCurrent()) throw new AgentSupersededError();
      patchChat(profileId, { startNote: undefined });
    } else if (provider.cmd) {
      cmd = provider.cmd;
      args = provider.binArgs ?? [];
    } else {
      const parsed = parseCustomCmd(String(s.settings.agentCustomCmd ?? ""));
      if (!parsed) {
        throw new Error("Custom agent command is empty — set it in the panel header.");
      }
      ({ cmd, args } = parsed);
    }

    // рабочая папка агента — карантин в app data, не $HOME пользователя
    const cwd = await join(await appDataDir(), "agent", profileId);
    const agent = await AcpAgent.spawn(cmd, args, NODE_NET_ENV, cwd, {
      onSessionUpdate: (u) => {
        if (isCurrent()) applyUpdate(profileId, u);
      },
      onPermissionRequest: (req) =>
        new Promise<string | null>((resolve) => {
          if (!isCurrent()) {
            resolve(null);
            return;
          }
          permResolvers.get(profileId)?.(null); // новее вытесняет старый
          permResolvers.set(profileId, resolve);
          patchChat(profileId, {
            permission: { ...permissionOf(req.toolCall), options: req.options },
          });
        }),
      onExit: (code) => {
        if (!isCurrent()) return;
        agentGenerations.delete(profileId);
        startingAgents.delete(profileId);
        liveAgents.delete(profileId);
        permResolvers.get(profileId)?.(null);
        permResolvers.delete(profileId);
        const chat = get().agentChats[profileId];
        // штатный reset тоже завершает процесс — ошибку показываем только
        // если чат ещё ждал ответа
        if (chat && (chat.status === "running" || chat.status === "starting")) {
          const tail = chat.stderr.slice(-8).join("\n");
          patchChat(profileId, {
            status: "error",
            permission: null,
            error:
              `Agent exited (code ${code ?? "?"}).` + (tail ? `\n${tail}` : ""),
          });
        }
      },
      onStderr: (line) => {
        if (!isCurrent()) return;
        set((st) => {
          const chat = st.agentChats[profileId];
          if (!chat) return {};
          return {
            agentChats: {
              ...st.agentChats,
              [profileId]: { ...chat, stderr: [...chat.stderr.slice(-30), line] },
            },
          };
        });
      },
    });
    if (!isCurrent()) {
      agent.kill();
      throw new AgentSupersededError();
    }
    startingAgents.set(profileId, agent);

    try {
      // без таймаута зависший адаптер (сеть, авторизация, битый бинарь)
      // оставляет панель крутиться на "starting…" навсегда
      await withTimeout(agent.initialize(), 30_000, `${provider.label}: initialize`);
      // MCP-сервер sql-kai: structured tools вместо разбора вывода CLI
      // (+ open_table/open_query — открытие вкладок в GUI)
      const cliPath = await api.cliBinPath().catch(() => null);
      const mcpServers = cliPath
        ? [{ name: "sql-kai", command: cliPath, args: ["mcp", profileId], env: [] }]
        : [];
      const sessionId = await withTimeout(
        agent.newSession(cwd, mcpServers),
        90_000,
        `${provider.label}: session/new`,
      );
      if (!isCurrent()) throw new AgentSupersededError();
      startingAgents.delete(profileId);
      const live = {
        agent,
        sessionId,
        contextSent: false,
        generation,
        mcp: mcpServers.length > 0,
      };
      liveAgents.set(profileId, live);
      return live;
    } catch (e) {
      if (startingAgents.get(profileId) === agent) {
        startingAgents.delete(profileId);
      }
      agent.kill();
      if (!isCurrent() && !(e instanceof AgentSupersededError)) {
        throw new AgentSupersededError();
      }
      throw e;
    }
  };

  return {
    agentOpen: false,
    agentChats: {},

    toggleAgentPanel: () =>
      set((s) => {
        if (s.agentOpen) return { agentOpen: false };
        // агенту нужен контекст подключения — без живой сессии не открывается
        if (!s.activeProfileId || !s.sessions[s.activeProfileId]) return {};
        return { agentOpen: true };
      }),

    setAgentProvider: async (id) => {
      const settings = { ...get().settings, agentProvider: id };
      // Провайдер — глобальная настройка. Ни один фоновый профиль не должен
      // сохранить процесс старого провайдера, иначе селектор и реальный агент
      // расходятся после переключения подключения.
      const profileIds = new Set([
        ...Object.keys(get().agentChats),
        ...liveAgents.keys(),
        ...startingAgents.keys(),
      ]);
      for (const profileId of profileIds) {
        dropAgent(profileId);
      }
      set({ settings, agentChats: {} });
      try {
        await api.saveSettings(settings);
      } catch (e) {
        get().showToast(`Settings not saved: ${errText(e)}`);
      }
    },

    setAgentCustomCmd: async (cmd) => {
      const settings = { ...get().settings, agentCustomCmd: cmd };
      set({ settings });
      try {
        await api.saveSettings(settings);
      } catch (e) {
        get().showToast(`Settings not saved: ${errText(e)}`);
      }
    },

    sendAgentPrompt: async (profileId, text) => {
      const trimmed = text.trim();
      if (!trimmed) return;
      const s = get();
      const chat = s.agentChats[profileId];
      if (chat && (chat.status === "running" || chat.status === "starting")) return;
      if (!chat) {
        set((st) => ({
          agentChats: {
            ...st.agentChats,
            [profileId]: {
              providerId: activeProvider(st.settings).id,
              status: "starting",
              items: [],
              permission: null,
              stderr: [],
            },
          },
        }));
      }
      pushItem(profileId, { kind: "user", text: trimmed });

      let promptGeneration: symbol | undefined;
      try {
        const live = await ensureAgent(profileId);
        promptGeneration = live.generation;
        const profile = get().profiles.find((p) => p.id === profileId);
        const prompt =
          !live.contextSent && profile
            ? `${connectionContext(profile, live.mcp)}\n\n---\n\nUser request:\n${trimmed}`
            : trimmed;
        live.contextSent = true;
        patchChat(profileId, { status: "running", error: undefined, startNote: undefined });
        const stop = await live.agent.prompt(live.sessionId, prompt);
        if (agentGenerations.get(profileId) !== live.generation) return;
        patchChat(profileId, { status: "ready", permission: null });
        if (stop === "refusal") {
          pushItem(profileId, {
            kind: "thought",
            text: "(the agent refused this request)",
          });
        }
      } catch (e) {
        if (
          e instanceof AgentSupersededError ||
          (promptGeneration !== undefined &&
            agentGenerations.get(profileId) !== promptGeneration)
        ) {
          return;
        }
        const chatNow = get().agentChats[profileId];
        if (!chatNow || chatNow.status === "error") return; // onExit уже оформил
        const hint =
          e instanceof RpcError && e.code === AUTH_REQUIRED
            ? `\n${activeProvider(get().settings).authHint}`
            : "";
        // stderr агента — единственная диагностика при таймауте/падении старта
        const tail = chatNow.stderr.slice(-5).join("\n");
        patchChat(profileId, {
          status: "error",
          permission: null,
          startNote: undefined,
          error: errText(e) + hint + (tail ? `\n${tail}` : ""),
        });
      }
    },

    cancelAgentPrompt: (profileId) => {
      const live = liveAgents.get(profileId);
      if (live?.agent.alive) live.agent.cancel(live.sessionId);
      // спека: при отмене все открытые permission-запросы закрываются cancelled
      permResolvers.get(profileId)?.(null);
      permResolvers.delete(profileId);
      patchChat(profileId, { permission: null });
    },

    answerAgentPermission: (profileId, optionId) => {
      permResolvers.get(profileId)?.(optionId);
      permResolvers.delete(profileId);
      patchChat(profileId, { permission: null });
    },

    resetAgentChat: (profileId) => {
      dropAgent(profileId);
      set((s) => {
        const { [profileId]: _, ...rest } = s.agentChats;
        return { agentChats: rest };
      });
    },
  };
}
