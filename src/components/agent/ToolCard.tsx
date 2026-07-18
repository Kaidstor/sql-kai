// Карточка tool-колла в чате агента: свёрнута — иконка, претти-имя
// (mcp__sql-kai__query → «query · sql-kai») и сводка аргументов; раскрыта —
// SQL/аргументы, превью результата (мини-таблица со скроллом) и diff-блоки.
// Данные уже нормализованы в сторе (lib/agentTool) — здесь только рендер.
import {
  Brain,
  Check,
  ChevronRight,
  Circle,
  Columns3,
  Database,
  ExternalLink,
  Eye,
  FileCode2,
  Globe,
  ListTree,
  Loader2,
  Pencil,
  Search,
  Table2,
  Terminal,
  TextCursor,
  Wrench,
  X,
} from "lucide-react";
import { memo, useState } from "react";
import {
  diffLines,
  parseMcpTitle,
  toolInputSummary,
  type AgentToolDiff,
  type AgentToolOutput,
} from "../../lib/agentTool";
import type { StatementResult } from "../../lib/types";
import type { AgentToolItem } from "../../lib/store/slices/agent";
import { cn } from "../ui";

export function ToolStatusIcon({ status }: { status: string }) {
  if (status === "completed") return <Check size={12} className="text-emerald-500" />;
  if (status === "failed") return <X size={12} className="text-red-400" />;
  if (status === "in_progress")
    return <Loader2 size={12} className="animate-spin text-amber-400" />;
  return <Circle size={9} className="text-zinc-600" />;
}

/** Иконки по ACP-виду тула (read/edit/execute/…). */
const KIND_ICONS: Record<string, typeof Wrench> = {
  execute: Terminal,
  read: Eye,
  edit: Pencil,
  search: Search,
  fetch: Globe,
  think: Brain,
};

/** Иконки MCP-тулов sql-kai — узнаваемее, чем общий Wrench. */
const SQLKAI_ICONS: Record<string, typeof Wrench> = {
  query: Database,
  tables: Table2,
  columns: Columns3,
  ddl: FileCode2,
  indexes: ListTree,
  open_table: ExternalLink,
  open_query: ExternalLink,
  selection: TextCursor,
};

const CELL_CAP = 200;

function Cell({ value }: { value: string | null }) {
  if (value === null) {
    return <span className="italic text-zinc-600">null</span>;
  }
  return value.length > CELL_CAP ? (
    <span title={value.slice(0, 2000)}>{value.slice(0, CELL_CAP)}…</span>
  ) : (
    <>{value}</>
  );
}

/** Превью одного statement'а: таблица со скроллом или строка rowsAffected. */
function ResultPreview({
  r,
  capped,
}: {
  r: StatementResult;
  capped?: boolean;
}) {
  if (r.columns.length === 0) {
    return (
      <div className="text-[10px] text-zinc-500">
        ok{r.rowsAffected != null && ` · ${r.rowsAffected} rows affected`}
      </div>
    );
  }
  const cut = r.truncated || capped;
  return (
    <div className="flex flex-col gap-0.5">
      <div className="selectable max-h-56 overflow-auto rounded border border-zinc-800 bg-zinc-950/60">
        <table className="w-max min-w-full border-collapse font-mono text-[10px]">
          <thead className="sticky top-0 bg-zinc-900">
            <tr>
              {r.columns.map((c, i) => (
                <th
                  key={i}
                  className="border-b border-zinc-700 px-1.5 py-0.5 text-left font-medium text-zinc-400"
                >
                  {c}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {r.rows.map((row, ri) => (
              <tr key={ri} className="odd:bg-zinc-900/40">
                {row.map((v, ci) => (
                  <td
                    key={ci}
                    className="whitespace-pre border-b border-zinc-800/50 px-1.5 py-0.5 align-top text-zinc-300"
                  >
                    <Cell value={v} />
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="text-[10px] text-zinc-600">
        {cut ? `first ${r.rows.length} rows` : `${r.rows.length} rows`}
        {r.rowsAffected != null && ` · ${r.rowsAffected} affected`}
      </div>
    </div>
  );
}

function OutputView({ output }: { output: AgentToolOutput }) {
  if (output.kind === "text") {
    return (
      <pre className="selectable max-h-56 overflow-auto whitespace-pre-wrap rounded border border-zinc-800 bg-zinc-950/60 p-1.5 font-mono text-[10px] text-zinc-300">
        {output.text}
        {output.capped && <span className="text-zinc-500">{"\n…(truncated)"}</span>}
      </pre>
    );
  }
  return (
    <div className="flex flex-col gap-1">
      {output.results.map((r, i) => (
        <ResultPreview key={i} r={r} capped={output.capped} />
      ))}
      {(output.durationMs !== undefined || output.maskedColumns) && (
        <div className="text-[10px] text-zinc-600">
          {output.durationMs !== undefined && `${output.durationMs} ms`}
          {output.maskedColumns &&
            ` · masked: ${output.maskedColumns.join(", ")}`}
        </div>
      )}
    </div>
  );
}

function DiffView({ diff }: { diff: AgentToolDiff }) {
  return (
    <div className="flex flex-col gap-0.5">
      {diff.path && (
        <div className="truncate font-mono text-[10px] text-zinc-500" title={diff.path}>
          {diff.path}
        </div>
      )}
      <div className="selectable max-h-64 overflow-auto rounded border border-zinc-800 bg-zinc-950/60 py-0.5 font-mono text-[10px] leading-4">
        {diffLines(diff.oldText ?? "", diff.newText).map((l, i) => (
          <div
            key={i}
            className={cn(
              "whitespace-pre px-1.5",
              l.type === "add" && "bg-emerald-500/10 text-emerald-300",
              l.type === "del" && "bg-red-500/10 text-red-400",
              l.type === "same" && "text-zinc-400",
            )}
          >
            {l.type === "add" ? "+ " : l.type === "del" ? "- " : "  "}
            {l.text}
          </div>
        ))}
      </div>
    </div>
  );
}

/** SQL/аргументы вызова: sql и command — как есть, прочее — компактный JSON. */
function InputView({ rawInput }: { rawInput: unknown }) {
  if (!rawInput || typeof rawInput !== "object") return null;
  const r = rawInput as Record<string, unknown>;
  const text =
    typeof r.sql === "string"
      ? r.sql
      : typeof r.command === "string"
        ? r.command
        : null;
  const rest = Object.fromEntries(
    Object.entries(r).filter(
      ([k, v]) => k !== "sql" && k !== "command" && v !== undefined,
    ),
  );
  const restJson =
    Object.keys(rest).length > 0 ? JSON.stringify(rest, null, 1) : null;
  if (!text && !restJson) return null;
  return (
    <pre className="selectable max-h-40 overflow-auto whitespace-pre-wrap rounded border border-zinc-800 bg-zinc-950/60 p-1.5 font-mono text-[10px] text-zinc-300">
      {text}
      {text && restJson && "\n"}
      {restJson && <span className="text-zinc-500">{restJson}</span>}
    </pre>
  );
}

// memo: стрим-апдейты чата перерендеривают список — готовые карточки
// с тем же item-объектом не трогаем
export const ToolCard = memo(function ToolCard({ item }: { item: AgentToolItem }) {
  const [open, setOpen] = useState(false);
  const mcp = parseMcpTitle(item.title);
  const inputSummary = toolInputSummary(item.rawInput);
  // у execute-вызовов адаптер кладёт команду и в title — не дублируем
  const summary = inputSummary === item.title.trim() ? null : inputSummary;
  const expandable =
    item.rawInput !== undefined ||
    item.output !== undefined ||
    (item.diffs?.length ?? 0) > 0;
  const Icon =
    (mcp && SQLKAI_ICONS[mcp.tool]) ?? KIND_ICONS[item.toolKind] ?? Wrench;

  return (
    <div className="rounded border border-zinc-800 bg-zinc-900/60 text-[11px] text-zinc-400">
      <button
        type="button"
        title={item.title}
        onClick={() => expandable && setOpen((o) => !o)}
        className={cn(
          "flex w-full items-center gap-1.5 px-2 py-1 text-left",
          expandable && "cursor-pointer hover:bg-zinc-800/40",
        )}
      >
        <Icon size={12} className="shrink-0 text-zinc-500" />
        {mcp ? (
          <span className="shrink-0 font-medium text-zinc-300">{mcp.tool}</span>
        ) : (
          <span
            className={cn("truncate text-zinc-300", summary ? "shrink-0 max-w-[45%]" : "min-w-0 flex-1")}
          >
            {item.title}
          </span>
        )}
        {mcp && mcp.server !== "sql-kai" && (
          <span className="shrink-0 rounded bg-zinc-800 px-1 py-px text-[9px] text-zinc-500">
            {mcp.server}
          </span>
        )}
        {summary && (
          <span className="min-w-0 flex-1 truncate font-mono text-[10px] text-zinc-500">
            {summary}
          </span>
        )}
        {!summary && mcp && <span className="min-w-0 flex-1" />}
        <ToolStatusIcon status={item.status} />
        {expandable && (
          <ChevronRight
            size={11}
            className={cn("shrink-0 text-zinc-600 transition-transform", open && "rotate-90")}
          />
        )}
      </button>
      {open && (
        <div className="flex flex-col gap-1.5 border-t border-zinc-800 px-2 py-1.5">
          <InputView rawInput={item.rawInput} />
          {item.diffs?.map((d, i) => <DiffView key={i} diff={d} />)}
          {item.output && <OutputView output={item.output} />}
          {!item.output && item.status === "in_progress" && (
            <div className="text-[10px] text-zinc-600">running…</div>
          )}
        </div>
      )}
    </div>
  );
});
