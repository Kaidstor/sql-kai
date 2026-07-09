import { X } from "lucide-react";
import { useRef, useState } from "react";

/** Inline cell editor. Blur stages the draft (clicking away mustn't lose the
 *  input); Enter stages and refocuses the grid; Esc cancels and refocuses.
 *  Keeping the draft local means typing doesn't re-render the whole grid.
 *  The input is borderless — the host cell carries the editing highlight. */
export function CellInput({
  initial,
  onStage,
  onNull,
  onClose,
}: {
  initial: string;
  onStage: (value: string) => void;
  /** Stages NULL via the ⊗ button; present only for nullable columns. */
  onNull?: () => void;
  /** refocus=true hands focus back to the grid (Enter/Esc, not blur). */
  onClose: (refocus: boolean) => void;
}) {
  const [draft, setDraft] = useState(initial);
  // Enter/Esc refocus the grid, which fires blur before the input unmounts —
  // this flag keeps that blur from staging a value already handled (or, for
  // Esc, explicitly cancelled).
  const skipBlur = useRef(false);
  return (
    <span className="flex h-[18px] w-full items-center gap-1">
      <input
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onClick={(e) => e.stopPropagation()}
        onBlur={() => {
          if (skipBlur.current) return;
          onStage(draft);
          onClose(false);
        }}
        onKeyDown={(e) => {
          // Don't let Esc bubble up and discard ALL edits.
          e.stopPropagation();
          if (e.key === "Enter") {
            skipBlur.current = true;
            onStage(draft);
            onClose(true);
          }
          if (e.key === "Escape") {
            skipBlur.current = true;
            onClose(true);
          }
        }}
        className="w-full min-w-0 flex-1 bg-transparent p-0 font-mono text-[12px] text-zinc-100 outline-none"
      />
      {onNull && (
        <button
          title="Set NULL"
          // preventDefault keeps focus on the input so blur doesn't stage
          // the draft before the click lands
          onMouseDown={(e) => e.preventDefault()}
          onClick={(e) => {
            e.stopPropagation();
            skipBlur.current = true;
            onNull();
            onClose(true);
          }}
          className="flex h-3.5 w-3.5 shrink-0 items-center justify-center rounded-full bg-zinc-400 text-zinc-950 hover:bg-zinc-100"
        >
          <X size={9} strokeWidth={3} />
        </button>
      )}
    </span>
  );
}
