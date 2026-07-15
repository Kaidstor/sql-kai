// Agent Client Protocol (ACP, agentclientprotocol.com) client. The agent is
// an external CLI process (Claude Code, Gemini, …) spawned by the Rust side
// (acp.rs); this module speaks newline-delimited JSON-RPC 2.0 with it through
// the acp://msg / acp://stderr / acp://exit events and the acp_send command.
//
// sql-kai is the *client* side of ACP: it renders the chat and answers
// permission requests. The agent reaches the database through the sql-kai
// CLI (broker), not through protocol extensions — so the client declares no
// fs/terminal capabilities and the protocol surface stays small.
import { getVersion } from "@tauri-apps/api/app";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// ---- Protocol types (the subset sql-kai uses) ------------------------------

export interface ContentBlock {
  type: string;
  text?: string;
  [key: string]: unknown;
}

/** Tool-call fields shared by tool_call / tool_call_update / permission. */
export interface ToolCallUpdate {
  toolCallId: string;
  title?: string;
  kind?: string; // read | edit | delete | move | search | execute | think | fetch | other
  status?: "pending" | "in_progress" | "completed" | "failed";
  content?: { type: string; content?: ContentBlock; [key: string]: unknown }[];
  rawInput?: unknown;
  rawOutput?: unknown;
}

export type SessionUpdate =
  | { sessionUpdate: "agent_message_chunk"; content: ContentBlock }
  | { sessionUpdate: "agent_thought_chunk"; content: ContentBlock }
  | { sessionUpdate: "user_message_chunk"; content: ContentBlock }
  | ({ sessionUpdate: "tool_call" } & ToolCallUpdate)
  | ({ sessionUpdate: "tool_call_update" } & ToolCallUpdate)
  | {
      sessionUpdate: "plan";
      entries: { content: string; priority?: string; status?: string }[];
    }
  | { sessionUpdate: "available_commands_update"; [key: string]: unknown }
  | { sessionUpdate: "current_mode_update"; currentModeId: string };

export interface PermissionOption {
  optionId: string;
  name: string;
  kind: "allow_once" | "allow_always" | "reject_once" | "reject_always";
}

export interface PermissionRequest {
  sessionId: string;
  toolCall: ToolCallUpdate;
  options: PermissionOption[];
}

export interface InitializeResult {
  protocolVersion: number;
  agentCapabilities?: { loadSession?: boolean; [key: string]: unknown };
  authMethods?: { id: string; name: string; description?: string | null }[];
}

export type StopReason =
  | "end_turn"
  | "max_tokens"
  | "max_turn_requests"
  | "refusal"
  | "cancelled"
  | string;

export interface AgentHandlers {
  onSessionUpdate: (update: SessionUpdate) => void;
  /** Resolve with the chosen optionId, or null for "cancelled". */
  onPermissionRequest: (req: PermissionRequest) => Promise<string | null>;
  onExit: (code: number | null) => void;
  onStderr: (line: string) => void;
}

// ---- JSON-RPC plumbing ------------------------------------------------------

interface RpcMessage {
  jsonrpc: "2.0";
  id?: number | string;
  method?: string;
  params?: unknown;
  result?: unknown;
  error?: { code: number; message: string; data?: unknown };
}

export class RpcError extends Error {
  constructor(
    public code: number,
    message: string,
    public data?: unknown,
  ) {
    super(message);
  }
}

/** JSON-RPC error code the agent answers with when it needs `authenticate`. */
export const AUTH_REQUIRED = -32000;

const agents = new Map<string, AcpAgent>();

// One set of global listeners routing by agentId — per-instance listen()
// would leak handlers on every chat reset.
let listenersReady: Promise<void> | null = null;
function ensureListeners(): Promise<void> {
  return (listenersReady ??= Promise.all([
    listen<{ agentId: string; line: string }>("acp://msg", (e) =>
      agents.get(e.payload.agentId)?.handleLine(e.payload.line),
    ),
    listen<{ agentId: string; line: string }>("acp://stderr", (e) =>
      agents.get(e.payload.agentId)?.handlers.onStderr(e.payload.line),
    ),
    listen<{ agentId: string; code: number | null }>("acp://exit", (e) =>
      agents.get(e.payload.agentId)?.handleExit(e.payload.code),
    ),
  ]).then(() => {}));
}

export class AcpAgent {
  readonly id = crypto.randomUUID();
  private nextId = 1;
  private pending = new Map<
    number,
    { resolve: (v: unknown) => void; reject: (e: unknown) => void }
  >();
  /** Serializes acp_send calls so messages hit stdin in order. */
  private sendQueue: Promise<void> = Promise.resolve();
  private dead = false;

  private constructor(readonly handlers: AgentHandlers) {}

  /** Spawns the agent process and registers it for event routing. */
  static async spawn(
    cmd: string,
    args: string[],
    env: Record<string, string>,
    cwd: string,
    handlers: AgentHandlers,
  ): Promise<AcpAgent> {
    await ensureListeners();
    const agent = new AcpAgent(handlers);
    agents.set(agent.id, agent);
    try {
      await invoke("acp_spawn", { agentId: agent.id, cmd, args, env, cwd });
    } catch (e) {
      agents.delete(agent.id);
      throw e;
    }
    return agent;
  }

  get alive(): boolean {
    return !this.dead;
  }

  // -- outgoing ---------------------------------------------------------------

  private send(msg: RpcMessage): void {
    const line = JSON.stringify(msg);
    this.sendQueue = this.sendQueue.then(() =>
      invoke<void>("acp_send", { agentId: this.id, line }).catch(() => {
        // dead process — the acp://exit handler rejects pending requests
      }),
    );
  }

  request<T>(method: string, params?: unknown): Promise<T> {
    if (this.dead) return Promise.reject(new RpcError(0, "agent exited"));
    const id = this.nextId++;
    const p = new Promise<T>((resolve, reject) => {
      this.pending.set(id, { resolve: resolve as (v: unknown) => void, reject });
    });
    this.send({ jsonrpc: "2.0", id, method, params });
    return p;
  }

  notify(method: string, params?: unknown): void {
    if (this.dead) return;
    this.send({ jsonrpc: "2.0", method, params });
  }

  // -- protocol helpers ---------------------------------------------------------

  async initialize(): Promise<InitializeResult> {
    const version = await getVersion().catch(() => "dev");
    return this.request<InitializeResult>("initialize", {
      protocolVersion: 1,
      clientCapabilities: {
        fs: { readTextFile: false, writeTextFile: false },
        terminal: false,
      },
      clientInfo: { name: "sql-kai", version },
    });
  }

  authenticate(methodId: string): Promise<unknown> {
    return this.request("authenticate", { methodId });
  }

  async newSession(cwd: string): Promise<string> {
    const res = await this.request<{ sessionId: string }>("session/new", {
      cwd,
      mcpServers: [],
    });
    return res.sessionId;
  }

  async prompt(sessionId: string, text: string): Promise<StopReason> {
    const res = await this.request<{ stopReason: StopReason }>(
      "session/prompt",
      { sessionId, prompt: [{ type: "text", text }] },
    );
    return res.stopReason;
  }

  cancel(sessionId: string): void {
    this.notify("session/cancel", { sessionId });
  }

  /** Kills the process; pending requests reject via the exit event. */
  kill(): void {
    void invoke("acp_kill", { agentId: this.id }).catch(() => {});
  }

  // -- incoming ---------------------------------------------------------------

  /** @internal one stdout line from the process. */
  handleLine(line: string): void {
    let msg: RpcMessage;
    try {
      msg = JSON.parse(line) as RpcMessage;
    } catch {
      // adapters occasionally print banners to stdout — not fatal
      this.handlers.onStderr(line);
      return;
    }
    if (msg.method !== undefined) {
      void this.handleIncoming(msg);
      return;
    }
    if (msg.id === undefined) return;
    const pending = this.pending.get(msg.id as number);
    if (!pending) return;
    this.pending.delete(msg.id as number);
    if (msg.error) {
      pending.reject(new RpcError(msg.error.code, msg.error.message, msg.error.data));
    } else {
      pending.resolve(msg.result);
    }
  }

  /** @internal requests/notifications from the agent. */
  private async handleIncoming(msg: RpcMessage): Promise<void> {
    const respond = (result: unknown) =>
      msg.id !== undefined &&
      this.send({ jsonrpc: "2.0", id: msg.id, result });
    const fail = (code: number, message: string) =>
      msg.id !== undefined &&
      this.send({ jsonrpc: "2.0", id: msg.id, error: { code, message } });

    switch (msg.method) {
      case "session/update": {
        const { update } = msg.params as { update: SessionUpdate };
        this.handlers.onSessionUpdate(update);
        return;
      }
      case "session/request_permission": {
        const req = msg.params as PermissionRequest;
        const optionId = await this.handlers.onPermissionRequest(req);
        respond({
          outcome: optionId
            ? { outcome: "selected", optionId }
            : { outcome: "cancelled" },
        });
        return;
      }
      default:
        // fs/* and terminal/* are not advertised in clientCapabilities;
        // anything else the client doesn't know either
        fail(-32601, `method not supported by sql-kai: ${msg.method}`);
    }
  }

  /** @internal process died: reject in-flight requests, tell the store. */
  handleExit(code: number | null): void {
    this.dead = true;
    agents.delete(this.id);
    for (const [, p] of this.pending) {
      p.reject(new RpcError(0, `agent exited (code ${code ?? "?"})`));
    }
    this.pending.clear();
    this.handlers.onExit(code);
  }
}
