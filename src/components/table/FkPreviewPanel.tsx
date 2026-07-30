import { ArrowUpRight, Loader2, X } from "lucide-react";
import type { FkPreview } from "../../lib/store";
import { ResultsGrid } from "../ResultsGrid";
import { IconButton } from "../ui";

/** FK preview (Drizzle-Studio-style): ⌘-клик показывает записи по ссылке
 *  в нижней панели — чаще всего нужна одна строка глазами, а не переход;
 *  переход остаётся кнопкой в шапке панели. */
export function FkPreviewPanel({
  preview,
  onOpenTable,
  onClose,
}: {
  preview: FkPreview;
  onOpenTable: () => void;
  onClose: () => void;
}) {
  const { target, filter, result } = preview;
  return (
    <div className="flex h-56 shrink-0 flex-col border-t border-zinc-800">
      <div className="flex shrink-0 items-center gap-2 border-b border-zinc-800 bg-zinc-925 px-2 py-1 text-[11px]">
        <span className="shrink-0 font-mono font-medium text-zinc-200">
          {target.schema === "public"
            ? target.table
            : `${target.schema}.${target.table}`}
        </span>
        <span className="truncate font-mono text-zinc-500" title={filter}>
          {filter}
        </span>
        {result && (
          <span className="shrink-0 text-zinc-600">
            {result.rows.length}
            {result.truncated ? "+" : ""} row(s)
          </span>
        )}
        <div className="ml-auto flex shrink-0 items-center gap-1">
          <button
            onClick={onOpenTable}
            className="flex items-center gap-1 rounded px-1.5 py-0.5 text-zinc-300 transition-colors hover:bg-zinc-800 hover:text-zinc-100"
            title="Open the referenced table as a tab with this filter"
          >
            <ArrowUpRight size={12} />
            Open table
          </button>
          <IconButton title="Close preview" onClick={onClose}>
            <X size={13} />
          </IconButton>
        </div>
      </div>
      <div className="min-h-0 flex-1">
        {preview.loading ? (
          <div className="flex h-full items-center justify-center gap-2 text-[12px] text-zinc-600">
            <Loader2 size={13} className="animate-spin" /> loading…
          </div>
        ) : preview.error ? (
          <div className="selectable overflow-auto p-3 font-mono text-[12px] whitespace-pre-wrap text-red-400">
            {preview.error}
          </div>
        ) : result && result.rows.length > 0 ? (
          <ResultsGrid result={result} />
        ) : (
          <div className="flex h-full items-center justify-center text-[12px] text-zinc-600">
            no referenced rows
          </div>
        )}
      </div>
    </div>
  );
}
