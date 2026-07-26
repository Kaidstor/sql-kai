import { Activity, Plus, SquareTerminal, Wrench, X } from "lucide-react";
import { useMemo, useRef, useState, type PointerEvent } from "react";
import { PROD_RED, tint } from "../lib/colors";
import { isMac } from "../lib/platform";
import { isQueryTabDirty, useApp } from "../lib/store";
import { dragWindow } from "../lib/window";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "./ContextMenu";
import { cn, RelIcon } from "./ui";

// Pointer-based tab reorder. HTML5 DnD is not an option here: Tauri's
// dragDropEnabled (needed for native file drop) makes wry swallow drag
// events in WKWebView, so draggable elements never receive dragover/drop.
export function TabsBar() {
  const tabs = useApp((s) => s.tabs);
  const activeTabId = useApp((s) => s.activeTabId);
  const setActiveTab = useApp((s) => s.setActiveTab);
  const closeTab = useApp((s) => s.closeTab);
  const closeTabs = useApp((s) => s.closeTabs);
  const moveTab = useApp((s) => s.moveTab);
  const activeProfileId = useApp((s) => s.activeProfileId);
  const profiles = useApp((s) => s.profiles);
  const sessions = useApp((s) => s.sessions);
  const openQueryTab = useApp((s) => s.openQueryTab);
  const queries = useApp((s) => s.queries);
  const sidebarOpen = useApp((s) => s.sidebarOpen);
  const tables = useApp((s) => s.tables);
  const production = Boolean(
    profiles.find((p) => p.id === activeProfileId)?.production,
  );
  // Only the active connection's tabs are shown; other connections keep
  // theirs in the background until selected again.
  const visible = tabs.filter((t) => t.profileId === activeProfileId);
  // Table-tab icons follow the catalog kind (view vs table), so a lookup map —
  // the bar re-renders on every pointermove while a tab is being dragged.
  const kinds = useMemo(() => {
    const m = new Map<string, string>();
    for (const t of tables[activeProfileId ?? ""] ?? []) {
      m.set(`${t.schema}.${t.name}`, t.kind);
    }
    return m;
  }, [tables, activeProfileId]);
  /** Tab under the last right-click (null = empty bar area). */
  const [menuTabId, setMenuTabId] = useState<string | null>(null);

  const barRef = useRef<HTMLDivElement>(null);
  const drag = useRef<{
    id: string;
    startX: number;
    startY: number;
    active: boolean; // true once the movement threshold is passed
  } | null>(null);
  // Suppresses the click that fires right after a reorder drag ends.
  const justDragged = useRef(false);
  // Throttle moveTab to actual midpoint crossings — pointermove is continuous.
  const lastMove = useRef("");
  const [draggingId, setDraggingId] = useState<string | null>(null);

  const handlePointerDown = (e: PointerEvent, tabId: string) => {
    if (e.button !== 0) return;
    if ((e.target as Element).closest("button")) return; // the close ✕
    justDragged.current = false;
    drag.current = { id: tabId, startX: e.clientX, startY: e.clientY, active: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const handlePointerMove = (e: PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    if (!d.active) {
      // a few px of slack keeps plain clicks from becoming drags
      if (
        Math.abs(e.clientX - d.startX) < 5 &&
        Math.abs(e.clientY - d.startY) < 5
      ) {
        return;
      }
      d.active = true;
      setDraggingId(d.id);
      setActiveTab(d.id);
    }
    // Live reorder: pointer capture retargets events to the pressed tab, so
    // hit-test the siblings' rects manually.
    const els = barRef.current?.querySelectorAll<HTMLElement>("[data-tab-id]");
    for (const el of els ?? []) {
      const id = el.dataset.tabId;
      if (!id || id === d.id) continue;
      const r = el.getBoundingClientRect();
      if (e.clientX < r.left || e.clientX > r.right) continue;
      const after = e.clientX > r.left + r.width / 2;
      const key = `${id}|${after}`;
      if (lastMove.current !== key) {
        lastMove.current = key;
        moveTab(d.id, id, after);
      }
      break;
    }
  };

  const endDrag = () => {
    if (drag.current?.active) {
      justDragged.current = true;
      setDraggingId(null);
    }
    drag.current = null;
    lastMove.current = "";
  };

  const menuIdx = menuTabId ? visible.findIndex((t) => t.id === menuTabId) : -1;

  return (
    <ContextMenu>
    <ContextMenuTrigger
      ref={barRef}
      onMouseDown={dragWindow}
      onContextMenu={(e) => {
        if (!(e.target as Element).closest?.("[data-tab-id]")) setMenuTabId(null);
      }}
      className={cn(
        "flex h-10 items-stretch border-b border-zinc-800 bg-zinc-925 overflow-x-auto shrink-0",
        // sidebar hidden (⌘B) — clear the mac traffic lights (overlay titlebar)
        !sidebarOpen && isMac && "pl-20",
      )}
      // production connection — the whole bar shifts to red
      style={
        production
          ? {
              background: tint(PROD_RED, 7, "var(--color-zinc-925)"),
              borderBottomColor: tint(PROD_RED, 35, "var(--color-zinc-800)"),
            }
          : undefined
      }
    >
      {visible.map((tab) => (
        <div
          key={tab.id}
          data-tab-id={tab.id}
          onClick={() => {
            if (justDragged.current) {
              justDragged.current = false;
              return;
            }
            setActiveTab(tab.id);
          }}
          onAuxClick={(e) => e.button === 1 && closeTab(tab.id)}
          onContextMenu={() => setMenuTabId(tab.id)}
          onPointerDown={(e) => handlePointerDown(e, tab.id)}
          onPointerMove={handlePointerMove}
          onPointerUp={endDrag}
          onPointerCancel={endDrag}
          className={cn(
            "group flex items-center gap-1.5 px-3 text-[12px] cursor-pointer",
            "border-r border-zinc-800 whitespace-nowrap select-none",
            tab.id === activeTabId
              ? "bg-zinc-950 text-zinc-100 shadow-[inset_0_2px_0_var(--color-sky-600)]"
              : "text-zinc-500 hover:text-zinc-300 hover:bg-zinc-900",
            tab.id === draggingId && "opacity-40",
          )}
        >
          {tab.state.kind === "query" ? (
            <SquareTerminal size={12} className="text-emerald-500/80 shrink-0" />
          ) : tab.state.kind === "structure" ? (
            <Wrench size={12} className="text-amber-500/80 shrink-0" />
          ) : tab.state.kind === "activity" ? (
            <Activity size={12} className="text-rose-400/80 shrink-0" />
          ) : (
            <RelIcon
              kind={kinds.get(`${tab.state.schema}.${tab.state.table}`)}
              className="opacity-80"
            />
          )}
          {tab.title}
          {/* Dirty mark sits in the close button's slot; hover swaps it for ✕. */}
          <button
            className={cn(
              "rounded p-0.5 hover:bg-zinc-700",
              !isQueryTabDirty(tab, queries) &&
                "opacity-0 group-hover:opacity-100",
            )}
            onClick={(e) => {
              e.stopPropagation();
              closeTab(tab.id);
            }}
          >
            {isQueryTabDirty(tab, queries) ? (
              <>
                <span className="block group-hover:hidden size-[11px] p-[3px]">
                  <span className="block size-full rounded-full bg-current" />
                </span>
                <X size={11} className="hidden group-hover:block" />
              </>
            ) : (
              <X size={11} />
            )}
          </button>
        </div>
      ))}
      {activeProfileId && sessions[activeProfileId] && (
        <button
          className="px-2 text-zinc-500 hover:text-zinc-200"
          title="New SQL tab ⌘T"
          onClick={() => openQueryTab(activeProfileId)}
        >
          <Plus size={14} />
        </button>
      )}
    </ContextMenuTrigger>
    <ContextMenuContent>
      <ContextMenuItem
        icon={X}
        disabled={menuIdx < 0}
        onClick={() => menuTabId && closeTab(menuTabId)}
      >
        Close
        <ContextMenuShortcut>⌘W</ContextMenuShortcut>
      </ContextMenuItem>
      <ContextMenuItem
        icon={X}
        disabled={menuIdx < 0 || visible.length <= 1}
        onClick={() =>
          closeTabs(visible.filter((t) => t.id !== menuTabId).map((t) => t.id))
        }
      >
        Close Others
      </ContextMenuItem>
      <ContextMenuItem
        icon={X}
        disabled={menuIdx < 0 || menuIdx >= visible.length - 1}
        onClick={() => closeTabs(visible.slice(menuIdx + 1).map((t) => t.id))}
      >
        Close Tabs to the Right
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem
        icon={X}
        iconClassName="text-red-400/80"
        disabled={visible.length === 0}
        onClick={() => closeTabs(visible.map((t) => t.id))}
      >
        Close All Tabs
        <ContextMenuShortcut>⌘K ⌘W</ContextMenuShortcut>
      </ContextMenuItem>
    </ContextMenuContent>
    </ContextMenu>
  );
}
