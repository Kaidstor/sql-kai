import { Plus, SquareTerminal, Table2, Wrench, X } from "lucide-react";
import { useRef, useState, type PointerEvent } from "react";
import { useApp } from "../lib/store";
import {
  ContextMenu,
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuSeparator,
  ContextMenuShortcut,
  ContextMenuTrigger,
} from "./context-menu";
import { cn } from "./ui";

// Pointer-based tab reorder. HTML5 DnD is not an option here: Tauri's
// dragDropEnabled (needed for native file drop) makes wry swallow drag
// events in WKWebView, so draggable elements never receive dragover/drop.
export function TabsBar() {
  const {
    tabs,
    activeTabId,
    setActiveTab,
    closeTab,
    closeTabs,
    moveTab,
    activeProfileId,
    sessions,
    openQueryTab,
  } = useApp();
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

  const onPointerDown = (e: PointerEvent, tabId: string) => {
    if (e.button !== 0) return;
    if ((e.target as Element).closest("button")) return; // the close ✕
    justDragged.current = false;
    drag.current = { id: tabId, startX: e.clientX, startY: e.clientY, active: false };
    e.currentTarget.setPointerCapture(e.pointerId);
  };

  const onPointerMove = (e: PointerEvent) => {
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

  const menuIdx = menuTabId ? tabs.findIndex((t) => t.id === menuTabId) : -1;

  return (
    <ContextMenu>
    <ContextMenuTrigger
      ref={barRef}
      data-tauri-drag-region
      onContextMenu={(e) => {
        if (!(e.target as Element).closest?.("[data-tab-id]")) setMenuTabId(null);
      }}
      className="flex h-10 items-stretch border-b border-zinc-800 bg-zinc-925 overflow-x-auto shrink-0"
    >
      {tabs.map((tab) => (
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
          onPointerDown={(e) => onPointerDown(e, tab.id)}
          onPointerMove={onPointerMove}
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
          ) : (
            <Table2 size={12} className="text-sky-500/80 shrink-0" />
          )}
          {tab.title}
          <button
            className="rounded p-0.5 opacity-0 group-hover:opacity-100 hover:bg-zinc-700"
            onClick={(e) => {
              e.stopPropagation();
              closeTab(tab.id);
            }}
          >
            <X size={11} />
          </button>
        </div>
      ))}
      {activeProfileId && sessions[activeProfileId] && (
        <button
          className="px-2 text-zinc-500 hover:text-zinc-200"
          title="New SQL tab ⌘N"
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
        disabled={menuIdx < 0 || tabs.length <= 1}
        onClick={() =>
          closeTabs(tabs.filter((t) => t.id !== menuTabId).map((t) => t.id))
        }
      >
        Close Others
      </ContextMenuItem>
      <ContextMenuItem
        icon={X}
        disabled={menuIdx < 0 || menuIdx >= tabs.length - 1}
        onClick={() => closeTabs(tabs.slice(menuIdx + 1).map((t) => t.id))}
      >
        Close Tabs to the Right
      </ContextMenuItem>
      <ContextMenuSeparator />
      <ContextMenuItem
        icon={X}
        iconClassName="text-red-400/80"
        disabled={tabs.length === 0}
        onClick={() => closeTabs(tabs.map((t) => t.id))}
      >
        Close All Tabs
        <ContextMenuShortcut>⌘K ⌘W</ContextMenuShortcut>
      </ContextMenuItem>
    </ContextMenuContent>
    </ContextMenu>
  );
}
