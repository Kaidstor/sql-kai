import { History, Trash2, X } from "lucide-react";
import { useState } from "react";
import { sqlPreview } from "../lib/sql";
import { useApp, type QueryTabState, type Tab } from "../lib/store";
import type { HistoryEntry } from "../lib/types";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuTrigger,
} from "./context-menu";
import { MenuBtn, Popover, cn } from "./ui";

function fmtTime(at: number): string {
  const d = new Date(at);
  const sameDay = new Date().toDateString() === d.toDateString();
  return sameDay
    ? d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" })
    : d.toLocaleDateString([], { month: "short", day: "numeric" });
}

function HistoryRow({
  entry,
  onOpen,
}: {
  entry: HistoryEntry;
  onOpen: () => void;
}) {
  const { deleteHistoryEntry, clearHistory } = useApp();
  return (
    <ContextMenu>
      <ContextMenuTrigger className="block">
        <div
          className="flex cursor-pointer items-center gap-2 rounded px-2 py-1 hover:bg-zinc-800"
          onClick={onOpen}
          title={entry.sql}
        >
          <span
            className={cn(
              "size-1.5 shrink-0 rounded-full",
              entry.ok ? "bg-emerald-500" : "bg-red-500",
            )}
          />
          <div className="min-w-0 flex-1">
            <div className="truncate font-mono text-[11px] text-zinc-300">
              {sqlPreview(entry.sql)}
            </div>
            <div className="text-[10px] text-zinc-600">
              {entry.profileName} · {fmtTime(entry.at)}
            </div>
          </div>
        </div>
      </ContextMenuTrigger>
      <ContextMenuContent>
        <ContextMenuItem onClick={() => deleteHistoryEntry(entry.id)}>
          <X size={13} className="text-zinc-500" /> Delete entry
        </ContextMenuItem>
        <ContextMenuSeparator />
        <ContextMenuItem
          onClick={() => {
            if (confirm("Clear the whole query history?")) clearHistory();
          }}
        >
          <Trash2 size={13} className="text-red-400/80" /> Clear all history
        </ContextMenuItem>
      </ContextMenuContent>
    </ContextMenu>
  );
}

/** App-wide query history; click re-opens the query, right-click cleans up. */
export function HistoryMenu({ tab }: { tab: Tab }) {
  const { history, setTabSql, openQueryTab } = useApp();
  const state = tab.state as QueryTabState;
  const [open, setOpen] = useState(false);

  const openEntry = (entry: HistoryEntry) => {
    setOpen(false);
    if (!state.sql.trim() || state.sql === entry.sql) {
      setTabSql(tab.id, entry.sql);
    } else {
      openQueryTab(tab.profileId, entry.sql);
    }
  };

  return (
    <Popover
      open={open}
      onClose={() => setOpen(false)}
      panelClassName="w-96 max-h-96 overflow-y-auto p-1"
      trigger={
        <MenuBtn
          title="Query history (right-click entries to clean up)"
          onClick={() => setOpen((v) => !v)}
        >
          <History size={13} />
          History
        </MenuBtn>
      }
    >
      {history.length === 0 ? (
        <div className="px-2 py-3 text-center text-[11px] italic text-zinc-600">
          no queries yet
        </div>
      ) : (
        history
          .slice(0, 100)
          .map((entry) => (
            <HistoryRow
              key={entry.id}
              entry={entry}
              onOpen={() => openEntry(entry)}
            />
          ))
      )}
    </Popover>
  );
}
