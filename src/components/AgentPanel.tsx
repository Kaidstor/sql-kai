// Right-side AI agent chat (ACP): provider picker, streamed messages, tool
// call cards and permission prompts. One chat per connection profile — the
// store slice (slices/agent.ts) owns the agent process and the protocol.
import {
  CircleStop,
  History,
  Loader2,
  MessagesSquare,
  RotateCcw,
  ShieldAlert,
  SlidersHorizontal,
  Sparkles,
  X,
} from "lucide-react";
import { memo, useEffect, useMemo, useState } from "react";
import {
  configSelectValues,
  type ConfigSelectGroup,
  type ConfigSelectValue,
  type SessionConfigOption,
} from "../lib/acp";
import { parseMcpTitle } from "../lib/agentTool";
import { resetOnVaultLock } from "../lib/moduleCaches";
import {
  deleteAgentChat,
  loadAgentChats,
  loadAgentConfigOptions,
  type PersistedAgentChat,
} from "../lib/persist";
import {
  activeProvider,
  AGENT_PROVIDERS,
  prefApplicable,
  useApp,
  type AgentChatItem,
  type AgentPermission,
} from "../lib/store";
import { Conversation, ConversationItem } from "./agent/Conversation";
import { Markdown } from "./agent/Markdown";
import { ToolCard, ToolStatusIcon } from "./agent/ToolCard";
import {
  Button,
  cn,
  Field,
  fmtTime,
  IconButton,
  Input,
  Popover,
  Select,
} from "./ui";

/** Panel width survives close/open within the session (not persisted).
 *  Module-level, so the vault lock resets it like the rest of the session
 *  state (see lib/moduleCaches). */
const DEFAULT_PANEL_WIDTH = 400;
let savedWidth = DEFAULT_PANEL_WIDTH;
resetOnVaultLock(() => {
  savedWidth = DEFAULT_PANEL_WIDTH;
});

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

function SavedChatRow({
  chat,
  onOpen,
  onDelete,
}: {
  chat: PersistedAgentChat;
  onOpen: () => void;
  onDelete: () => void;
}) {
  return (
    <div
      className="group flex cursor-pointer items-center gap-1.5 rounded px-2 py-1 text-left hover:bg-zinc-800"
      onClick={onOpen}
      title={chat.title}
    >
      <MessagesSquare size={12} className="shrink-0 text-zinc-500" />
      <div className="min-w-0 flex-1">
        <div className="truncate text-[11px] text-zinc-300">{chat.title}</div>
        <div className="text-[10px] text-zinc-600">{fmtTime(chat.updatedAt)}</div>
      </div>
      <IconButton
        title="Delete saved chat"
        className="invisible group-hover:visible"
        onClick={(e) => {
          e.stopPropagation();
          onDelete();
        }}
      >
        <X size={12} />
      </IconButton>
    </div>
  );
}

function ConfigOptionSelect({
  opt,
  onSet,
}: {
  opt: SessionConfigOption;
  onSet: (value: string | boolean) => void;
}) {
  if (opt.type === "boolean") {
    return (
      <Select
        className="w-full"
        value={opt.currentValue ? "on" : "off"}
        title={opt.description ?? undefined}
        onChange={(e) => onSet(e.target.value === "on")}
      >
        <option value="on">On</option>
        <option value="off">Off</option>
      </Select>
    );
  }
  const entries = opt.options as (ConfigSelectValue | ConfigSelectGroup)[];
  return (
    <Select
      className="w-full"
      value={opt.currentValue}
      title={opt.description ?? undefined}
      onChange={(e) => onSet(e.target.value)}
    >
      {entries.map((entry) =>
        "group" in entry ? (
          <optgroup key={entry.group} label={entry.name}>
            {entry.options.map((v) => (
              <option key={v.value} value={v.value}>
                {v.name}
              </option>
            ))}
          </optgroup>
        ) : (
          <option key={entry.value} value={entry.value}>
            {entry.name}
          </option>
        ),
      )}
    </Select>
  );
}

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
  const restoreAgentChat = useApp((s) => s.restoreAgentChat);
  const setAgentProvider = useApp((s) => s.setAgentProvider);
  const setAgentCustomCmd = useApp((s) => s.setAgentCustomCmd);
  const setAgentConfigOption = useApp((s) => s.setAgentConfigOption);

  const [draft, setDraft] = useState("");
  const [width, setWidth] = useState(savedWidth);
  const [customDraft, setCustomDraft] = useState(settings.agentCustomCmd ?? "");
  const [historyOpen, setHistoryOpen] = useState(false);
  const [configOpen, setConfigOpen] = useState(false);
  const [savedChats, setSavedChats] = useState<PersistedAgentChat[]>([]);

  const provider = activeProvider(settings);
  const busy = chat?.status === "running" || chat?.status === "starting";
  const chatEmpty = !chat || chat.items.length === 0;

  // Опции сессии: живые из чата, до старта сессии — кэш прошлой сессии
  // провайдера с наложенным сохранённым выбором (то, что реально применится)
  const liveOptions = chat?.configOptions;
  const sessionConfig = settings.agentSessionConfig;
  const configOptions = useMemo(() => {
    if (liveOptions && liveOptions.length > 0) return liveOptions;
    const prefs = sessionConfig?.[provider.id] ?? {};
    return loadAgentConfigOptions(provider.id).map((o) => {
      const pref = prefs[o.id];
      return pref !== undefined && prefApplicable(o, pref)
        ? ({ ...o, currentValue: pref } as SessionConfigOption)
        : o;
    });
  }, [liveOptions, provider.id, sessionConfig]);

  const modelOpt = configOptions.find(
    (o): o is Extract<SessionConfigOption, { type: "select" }> =>
      o.type === "select" && o.category === "model",
  );
  const modelLabel = modelOpt
    ? (configSelectValues(modelOpt).find((v) => v.value === modelOpt.currentValue)
        ?.name ?? modelOpt.currentValue)
    : null;

  // список читается из localStorage лениво — когда он виден (пустое
  // состояние или поповер), не на каждый стрим-чанк
  useEffect(() => {
    if (chatEmpty || historyOpen) {
      setSavedChats(activeProfileId ? loadAgentChats(activeProfileId) : []);
    }
  }, [activeProfileId, chatEmpty, historyOpen]);

  const openSavedChat = (chatId: string) => {
    if (!activeProfileId || busy) return;
    setHistoryOpen(false);
    restoreAgentChat(activeProfileId, chatId);
  };
  const deleteSavedChat = (chatId: string) => {
    if (!activeProfileId) return;
    deleteAgentChat(activeProfileId, chatId);
    setSavedChats((list) => list.filter((c) => c.id !== chatId));
  };

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
        {configOptions.length > 0 && activeProfileId && (
          <Popover
            open={configOpen}
            onClose={() => setConfigOpen(false)}
            align="right"
            panelClassName="w-60 p-2"
            trigger={
              <button
                title="Session settings — model, permission mode"
                onClick={() => setConfigOpen((v) => !v)}
                className={cn(
                  "flex min-w-0 items-center gap-1 rounded px-1.5 py-1",
                  "text-[11px] text-zinc-400 hover:bg-zinc-800 hover:text-zinc-200",
                )}
              >
                <SlidersHorizontal size={11} className="shrink-0" />
                {modelLabel && <span className="truncate">{modelLabel}</span>}
              </button>
            }
          >
            {configOptions.map((o) => (
              <Field key={o.id} label={o.name} className="mb-2 last:mb-0">
                <ConfigOptionSelect
                  opt={o}
                  onSet={(v) => void setAgentConfigOption(activeProfileId, o.id, v)}
                />
              </Field>
            ))}
          </Popover>
        )}
        <div className="ml-auto flex items-center">
          <Popover
            open={historyOpen}
            onClose={() => setHistoryOpen(false)}
            align="right"
            panelClassName="w-72 max-h-80 overflow-y-auto p-1"
            trigger={
              <IconButton
                title="Saved chats of this connection"
                onClick={() => setHistoryOpen((v) => !v)}
              >
                <History size={13} />
              </IconButton>
            }
          >
            {savedChats.length === 0 ? (
              <div className="px-2 py-3 text-center text-[11px] italic text-zinc-600">
                no saved chats on this connection yet
              </div>
            ) : (
              savedChats.map((c) => (
                <SavedChatRow
                  key={c.id}
                  chat={c}
                  onOpen={() => openSavedChat(c.id)}
                  onDelete={() => deleteSavedChat(c.id)}
                />
              ))
            )}
          </Popover>
          <IconButton
            title="New chat (saves the current one, stops the agent)"
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
            {savedChats.length > 0 && (
              <div className="mt-1.5 w-full max-w-72">
                <div className="mb-1 text-[10px] uppercase tracking-wide text-zinc-600">
                  continue a saved chat
                </div>
                <div className="flex flex-col">
                  {savedChats.slice(0, 5).map((c) => (
                    <SavedChatRow
                      key={c.id}
                      chat={c}
                      onOpen={() => openSavedChat(c.id)}
                      onDelete={() => deleteSavedChat(c.id)}
                    />
                  ))}
                </div>
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
