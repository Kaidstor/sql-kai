import { X } from "lucide-react";
import { isMac } from "../lib/platform";
import { cn, IconButton, Overlay } from "./ui";

const MOD = isMac ? "⌘" : "Ctrl";
const SHIFT = isMac ? "⇧" : "Shift";
const ALT = isMac ? "⌥" : "Alt";

interface Shortcut {
  keys: string[];
  label: string;
}

const SECTIONS: { title: string; items: Shortcut[] }[] = [
  {
    title: "General",
    items: [
      { keys: [MOD, ALT, "O"], label: "Connections palette" },
      { keys: [MOD, "F"], label: "Filter connections (launcher)" },
      { keys: [MOD, "P"], label: "Saved queries palette" },
      { keys: ["Ctrl", "X"], label: "Delete saved query (palette)" },
      { keys: [MOD, SHIFT, "O"], label: "Find table / column / function" },
      { keys: ["Ctrl", "1…9"], label: "Switch connection" },
      { keys: [MOD, "`"], label: "Cycle connections" },
      { keys: ["Ctrl", "⇥"], label: "Next / previous tab" },
      { keys: [MOD, SHIFT, "] ["], label: "Next / previous tab" },
      { keys: [MOD, "T / N"], label: "New query tab" },
      { keys: [MOD, "W"], label: "Close tab" },
      { keys: [MOD, SHIFT, "T"], label: "Reopen closed tab" },
      { keys: [`${MOD}K`, `${MOD}W`], label: "Close all tabs" },
      { keys: [MOD, SHIFT, "W"], label: "Close connection" },
      { keys: [MOD, "R"], label: "Refresh table / structure" },
      { keys: [MOD, "B"], label: "Toggle sidebar" },
      { keys: [MOD, "J"], label: "AI agent panel" },
      { keys: [MOD, ","], label: "Settings" },
      { keys: [MOD, "?"], label: "Keyboard shortcuts" },
    ],
  },
  {
    title: "Query editor",
    items: [
      { keys: [MOD, "⏎"], label: "Run query" },
      { keys: ["Ctrl", "C"], label: "Cancel running query" },
      { keys: ["Ctrl", "Space"], label: "Autocomplete" },
      { keys: [SHIFT, ALT, "F"], label: "Format query / selection" },
      { keys: [MOD, "F"], label: "Find in editor" },
      { keys: [MOD, "S"], label: "Save query" },
    ],
  },
  {
    title: "Data grid",
    items: [
      { keys: [MOD, "C"], label: "Copy cell / rows" },
      { keys: [MOD, "V"], label: "Paste TSV into cells" },
      { keys: [MOD, "D"], label: "Duplicate rows" },
      { keys: ["⌫"], label: "Mark rows deleted / restore" },
      { keys: [MOD, "⏎"], label: "Open cell in editor" },
      { keys: [MOD, "S"], label: "Apply staged changes" },
      { keys: [SHIFT, "Click"], label: "Select range" },
      { keys: [MOD, "Click"], label: "Toggle row in selection" },
      { keys: [SHIFT, "Click ⇅"], label: "Add column to multi-sort" },
      { keys: ["Esc"], label: "Clear selection / discard edits" },
    ],
  },
];

function Keys({ keys }: { keys: string[] }) {
  return (
    <span className="flex shrink-0 items-center gap-1">
      {keys.map((k, i) => (
        <kbd
          key={i}
          className="rounded border border-zinc-700/70 bg-zinc-800/80 px-1.5 py-0.5 font-sans text-[10px] leading-none text-zinc-300"
        >
          {k}
        </kbd>
      ))}
    </span>
  );
}

/** Shortcut cheat-sheet; lives on the empty screen and in the ⌘? overlay. */
export function ShortcutSections({ className }: { className?: string }) {
  return (
    <div className={cn("flex flex-wrap justify-center gap-x-14 gap-y-6", className)}>
      {SECTIONS.map((s) => (
        <div key={s.title} className="w-60">
          <div className="mb-2 text-[10px] font-semibold tracking-wider text-zinc-600">
            {s.title.toUpperCase()}
          </div>
          <div className="flex flex-col gap-1.5">
            {s.items.map((it) => (
              <div
                // label alone is not unique ("Next / previous tab" is listed
                // for two different key combos)
                key={`${it.keys.join("+")} ${it.label}`}
                className="flex items-center justify-between gap-4 text-[12px] text-zinc-500"
              >
                <span className="truncate">{it.label}</span>
                <Keys keys={it.keys} />
              </div>
            ))}
          </div>
        </div>
      ))}
    </div>
  );
}

export function ShortcutsOverlay({
  open,
  onClose,
}: {
  open: boolean;
  onClose: () => void;
}) {
  if (!open) return null;
  return (
    <Overlay onClose={onClose} closeOnEsc className="items-center bg-black/60">
      <div className="max-h-[85vh] max-w-[92vw] overflow-y-auto rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
        <div className="flex items-center gap-2 border-b border-zinc-800 px-5 py-2.5">
          <span className="text-[12px] font-medium text-zinc-200">
            Keyboard shortcuts
          </span>
          <div className="ml-auto">
            <IconButton onClick={onClose}>
              <X size={14} />
            </IconButton>
          </div>
        </div>
        <div className="px-6 py-5">
          <ShortcutSections />
        </div>
      </div>
    </Overlay>
  );
}
