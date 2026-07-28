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

/** Stdio MCP server passed to the agent in session/new. */
export interface McpServerConfig {
  name: string;
  command: string;
  args: string[];
  env: { name: string; value: string }[];
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

/** Anything spawned through acp.rs and routed by agentId. */
interface ProcSink {
  handleLine(line: string): void;
  handleStderr(line: string): void;
  handleExit(code: number | null): void;
}

const agents = new Map<string, ProcSink>();

// One set of global listeners routing by agentId — per-instance listen()
// would leak handlers on every chat reset.
let listenersReady: Promise<void> | null = null;
function ensureListeners(): Promise<void> {
  return (listenersReady ??= Promise.all([
    listen<{ agentId: string; line: string }>("acp://msg", (e) =>
      agents.get(e.payload.agentId)?.handleLine(e.payload.line),
    ),
    listen<{ agentId: string; line: string }>("acp://stderr", (e) =>
      agents.get(e.payload.agentId)?.handleStderr(e.payload.line),
    ),
    listen<{ agentId: string; code: number | null }>("acp://exit", (e) =>
      agents.get(e.payload.agentId)?.handleExit(e.payload.code),
    ),
  ]).then(() => {}));
}

/** Rejects with a descriptive error when `p` doesn't settle in `ms`. */
export function withTimeout<T>(p: Promise<T>, ms: number, what: string): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const t = setTimeout(
      () => reject(new Error(`${what} timed out after ${Math.round(ms / 1000)}s`)),
      ms,
    );
    p.then(
      (v) => {
        clearTimeout(t);
        resolve(v);
      },
      (e) => {
        clearTimeout(t);
        reject(e);
      },
    );
  });
}

/** Non-ACP child process (npm install …) with line-streamed output.
 * Reuses the acp.rs transport: same spawn/kill commands and events. */
export class RawProc implements ProcSink {
  readonly id = crypto.randomUUID();
  private exited: (code: number | null) => void = () => {};
  /** Resolves with the exit code when the process dies. */
  readonly done: Promise<number | null>;

  private constructor(private onLine: (line: string) => void) {
    this.done = new Promise((resolve) => {
      this.exited = resolve;
    });
  }

  static async spawn(
    cmd: string,
    args: string[],
    cwd: string,
    onLine: (line: string) => void,
  ): Promise<RawProc> {
    await ensureListeners();
    const proc = new RawProc(onLine);
    agents.set(proc.id, proc);
    try {
      await invoke("acp_spawn", { agentId: proc.id, cmd, args, cwd });
    } catch (e) {
      agents.delete(proc.id);
      throw e;
    }
    return proc;
  }

  kill(): void {
    void invoke("acp_kill", { agentId: this.id }).catch(() => {});
  }

  handleLine(line: string): void {
    this.onLine(line);
  }
  handleStderr(line: string): void {
    this.onLine(line);
  }
  handleExit(code: number | null): void {
    agents.delete(this.id);
    this.exited(code);
  }
}

export class AcpAgent implements ProcSink {
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

  /** Spawns the agent process and registers it for event routing. Env не
   *  передаётся: backend сам задаёт PATH и NODE_OPTIONS — произвольный env
   *  из webview был бы инъекцией в разрешённый бинарь. */
  static async spawn(
    cmd: string,
    args: string[],
    cwd: string,
    handlers: AgentHandlers,
  ): Promise<AcpAgent> {
    await ensureListeners();
    const agent = new AcpAgent(handlers);
    agents.set(agent.id, agent);
    try {
      await invoke("acp_spawn", { agentId: agent.id, cmd, args, cwd });
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

  async newSession(cwd: string, mcpServers: McpServerConfig[] = []): Promise<string> {
    const res = await this.request<{ sessionId: string }>("session/new", {
      cwd,
      mcpServers,
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

  /** @internal one stderr line from the process. */
  handleStderr(line: string): void {
    this.handlers.onStderr(line);
  }

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
