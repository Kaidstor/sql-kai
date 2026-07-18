// Right-side AI agent chat (ACP): provider picker, streamed messages, tool
// call cards and permission prompts. One chat per connection profile — the
// store slice (slices/agent.ts) owns the agent process and the protocol.
import {
  CircleStop,
  Loader2,
  RotateCcw,
  ShieldAlert,
  Sparkles,
  X,
} from "lucide-react";
import { memo, useState } from "react";
import { parseMcpTitle } from "../lib/agentTool";
import {
  activeProvider,
  AGENT_PROVIDERS,
  type AgentChatItem,
  type AgentPermission,
} from "../lib/store/slices/agent";
import { useApp } from "../lib/store";
import { Conversation, ConversationItem } from "./agent/Conversation";
import { Markdown } from "./agent/Markdown";
import { ToolCard, ToolStatusIcon } from "./agent/ToolCard";
import { Button, cn, IconButton, Input, Select } from "./ui";

/** Panel width survives close/open within the session (not persisted). */
let savedWidth = 400;

/** mcp__sql-kai__query → «sql-kai · query» в permission-карточке. */
function prettyPermTitle(title: string): string {
  const mcp = parseMcpTitle(title);
  return mcp ? `${mcp.server} · ${mcp.tool}` : title;
}

// memo: стрим дописывает только последний элемент — готовые сообщения
// (включая уже отрендеренный markdown) не перерендериваются
const ItemView = memo(function ItemView({ item }: { item: AgentChatItem }) {
  switch (item.kind) {
    case "user":
      return (
        <div className="selectable ml-8 self-end whitespace-pre-wrap rounded-lg border border-sky-600/25 bg-sky-600/15 px-2.5 py-1.5 text-[12px] text-zinc-100">
          {item.text}
        </div>
      );
    case "assistant":
      return <Markdown text={item.text} />;
    case "thought":
      return (
        <div className="selectable whitespace-pre-wrap text-[11px] italic leading-relaxed text-zinc-500">
          {item.text}
        </div>
      );
    case "plan":
      return (
        <div className="flex flex-col gap-0.5 rounded border border-zinc-800 bg-zinc-900/60 px-2 py-1.5">
          {item.entries.map((e, i) => (
            <div key={i} className="flex items-start gap-1.5 text-[11px] text-zinc-400">
              <span className="mt-0.5 shrink-0">
                <ToolStatusIcon
                  status={e.status === "completed" ? "completed" : e.status}
                />
              </span>
              <span className={cn(e.status === "completed" && "text-zinc-600 line-through")}>
                {e.content}
              </span>
            </div>
          ))}
        </div>
      );
    case "tool":
      return <ToolCard item={item} />;
  }
});

function PermissionCard({
  perm,
  onAnswer,
}: {
  perm: AgentPermission;
  onAnswer: (optionId: string | null) => void;
}) {
  return (
    <div className="shrink-0 border-t border-amber-500/30 bg-amber-500/5 px-2.5 py-2">
      <div className="mb-1 flex items-center gap-1.5 text-[11px] font-medium text-amber-300">
        <ShieldAlert size={12} /> Agent asks for permission
      </div>
      <div className="text-[12px] break-words text-zinc-200">
        {prettyPermTitle(perm.title)}
      </div>
      {perm.detail && (
        <pre className="selectable mt-1 max-h-24 overflow-auto whitespace-pre-wrap rounded border border-zinc-800 bg-zinc-900 p-1.5 font-mono text-[11px] text-zinc-300">
          {perm.detail}
        </pre>
      )}
      <div className="mt-1.5 flex flex-wrap gap-1.5">
        {perm.options.map((o) => (
          <Button
            key={o.optionId}
            variant={o.kind.startsWith("allow") ? "primary" : "ghost"}
            className={cn(
              "!px-2 !py-0.5 text-[11px]",
              o.kind.startsWith("reject") && "text-red-300 hover:!bg-red-500/15",
            )}
            onClick={() => onAnswer(o.optionId)}
          >
            {o.name}
          </Button>
        ))}
      </div>
    </div>
  );
}

export function AgentPanel() {
  const activeProfileId = useApp((s) => s.activeProfileId);
  const profile = useApp((s) =>
    s.profiles.find((p) => p.id === s.activeProfileId),
  );
  const chat = useApp((s) =>
    s.activeProfileId ? s.agentChats[s.activeProfileId] : undefined,
  );
  const settings = useApp((s) => s.settings);
  const toggleAgentPanel = useApp((s) => s.toggleAgentPanel);
  const sendAgentPrompt = useApp((s) => s.sendAgentPrompt);
  const cancelAgentPrompt = useApp((s) => s.cancelAgentPrompt);
  const answerAgentPermission = useApp((s) => s.answerAgentPermission);
  const resetAgentChat = useApp((s) => s.resetAgentChat);
  const setAgentProvider = useApp((s) => s.setAgentProvider);
  const setAgentCustomCmd = useApp((s) => s.setAgentCustomCmd);

  const [draft, setDraft] = useState("");
  const [width, setWidth] = useState(savedWidth);
  const [customDraft, setCustomDraft] = useState(settings.agentCustomCmd ?? "");

  const provider = activeProvider(settings);
  const busy = chat?.status === "running" || chat?.status === "starting";

  const doSend = () => {
    if (!activeProfileId || busy) return;
    const text = draft.trim();
    if (!text) return;
    setDraft("");
    void sendAgentPrompt(activeProfileId, text);
  };

  const resizeTo = (clientX: number) => {
    const w = Math.max(300, Math.min(680, window.innerWidth - clientX));
    savedWidth = w;
    setWidth(w);
  };

  return (
    <aside
      style={{ width }}
      className="relative flex shrink-0 flex-col border-l border-zinc-800 bg-zinc-950 min-h-0"
    >
      <div
        className="absolute inset-y-0 left-0 z-10 w-1 cursor-col-resize hover:bg-sky-500/60"
        onPointerDown={(e) => {
          e.preventDefault();
          e.currentTarget.setPointerCapture(e.pointerId);
        }}
        onPointerMove={(e) => {
          if (e.currentTarget.hasPointerCapture(e.pointerId)) resizeTo(e.clientX);
        }}
      />

      <div className="flex shrink-0 items-center gap-1.5 border-b border-zinc-800 px-2 py-1.5">
        <Sparkles size={13} className="shrink-0 text-violet-400" />
        <span className="text-[12px] font-medium text-zinc-200">Agent</span>
        <Select
          value={provider.id}
          onChange={(e) => void setAgentProvider(e.target.value)}
          title="ACP agent provider — switching resets the current chat"
          className="ml-1"
        >
          {AGENT_PROVIDERS.map((p) => (
            <option key={p.id} value={p.id}>
              {p.label}
            </option>
          ))}
        </Select>
        <div className="ml-auto flex items-center">
          <IconButton
            title="New chat (stops the agent)"
            disabled={!chat}
            onClick={() => activeProfileId && resetAgentChat(activeProfileId)}
          >
            <RotateCcw size={13} />
          </IconButton>
          <IconButton title="Close · ⌘J" onClick={toggleAgentPanel}>
            <X size={14} />
          </IconButton>
        </div>
      </div>

      {provider.id === "custom" && (
        <div className="shrink-0 border-b border-zinc-800 px-2 py-1.5">
          <Input
            placeholder="my-agent --acp   (ACP over stdio)"
            value={customDraft}
            onChange={(e) => setCustomDraft(e.target.value)}
            onBlur={() => void setAgentCustomCmd(customDraft)}
            onKeyDown={(e) => {
              if (e.key === "Enter") void setAgentCustomCmd(customDraft);
            }}
            className="font-mono text-[11px]"
          />
        </div>
      )}

      <Conversation>
        {(!chat || chat.items.length === 0) && !chat?.error && (
          <ConversationItem className="my-auto items-center gap-2 px-4 text-center">
            <Sparkles size={20} className="text-violet-400/70" />
            <div className="text-[13px] text-zinc-300">Chat with your database</div>
            <div className="text-[11px] leading-relaxed text-zinc-500">
              The agent explores the schema on its own and runs queries through
              the sql-kai CLI — read-only unless you say otherwise. It runs on
              your {provider.label} account; make sure that CLI is logged in.
            </div>
            {profile && (
              <div className="text-[11px] text-zinc-600">
                connection: <span className="text-zinc-400">{profile.name}</span>
              </div>
            )}
          </ConversationItem>
        )}
        {chat?.items.map((item) => (
          // сообщение пользователя — якорь нового хода: прилипает к верху
          // вьюпорта, ответ стримится в место под ним
          <ConversationItem
            key={item.id}
            id={String(item.id)}
            anchor={item.kind === "user"}
          >
            <ItemView item={item} />
          </ConversationItem>
        ))}
        {chat?.status === "starting" && (
          <ConversationItem className="gap-1 text-[11px] text-zinc-500">
            <div className="flex items-center gap-1.5">
              <Loader2 size={11} className="animate-spin" />
              {chat.startNote ? `installing ${provider.label} adapter…` : `starting ${provider.label}…`}
            </div>
            {chat.startNote && (
              <div
                className="truncate font-mono text-[10px] text-zinc-600"
                title={chat.startNote}
              >
                {chat.startNote}
              </div>
            )}
          </ConversationItem>
        )}
        {chat?.status === "running" && !chat.permission && (
          <ConversationItem className="flex-row items-center gap-1.5 text-[11px] text-zinc-500">
            <Loader2 size={11} className="animate-spin" /> thinking…
          </ConversationItem>
        )}
        {chat?.error && (
          <ConversationItem>
            <pre className="selectable whitespace-pre-wrap rounded-md border border-red-900/60 bg-red-950/40 p-2 font-mono text-[11px] text-red-300">
              {chat.error}
            </pre>
          </ConversationItem>
        )}
      </Conversation>

      {chat?.permission && activeProfileId && (
        <PermissionCard
          perm={chat.permission}
          onAnswer={(id) => answerAgentPermission(activeProfileId, id)}
        />
      )}

      <div className="shrink-0 border-t border-zinc-800 p-2">
        <textarea
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey) {
              e.preventDefault();
              doSend();
            }
          }}
          rows={3}
          spellCheck={false}
          disabled={!activeProfileId}
          placeholder={
            profile ? `Ask about ${profile.name}…` : "No active connection"
          }
          className={cn(
            "w-full resize-none rounded-md border border-zinc-700 bg-zinc-900 px-2 py-1.5",
            "text-[12px] text-zinc-100 placeholder:text-zinc-600",
            "focus:border-sky-600 focus:outline-none",
          )}
        />
        <div className="mt-1 flex items-center gap-2">
          <span className="text-[10px] text-zinc-600">⏎ send · ⇧⏎ newline</span>
          <div className="ml-auto">
            {chat?.status === "running" ? (
              <Button
                variant="danger"
                className="!px-2 !py-0.5"
                onClick={() => activeProfileId && cancelAgentPrompt(activeProfileId)}
              >
                <CircleStop size={12} /> Stop
              </Button>
            ) : (
              <Button
                variant="primary"
                className="!px-2.5 !py-0.5"
                disabled={!draft.trim() || busy || !activeProfileId}
                onClick={doSend}
              >
                Send
              </Button>
            )}
          </div>
        </div>
      </div>
    </aside>
  );
}
