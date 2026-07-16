import { replaceAll, replaceNext } from "@codemirror/search";
import type { EditorView } from "@codemirror/view";
import {
  ArrowDown,
  ArrowUp,
  CaseSensitive,
  ChevronRight,
  Regex,
  Replace,
  ReplaceAll,
  WholeWord,
  X,
} from "lucide-react";
import {
  forwardRef,
  useEffect,
  useImperativeHandle,
  useRef,
  useState,
  type ReactNode,
} from "react";
import {
  allMatches,
  applyQuery,
  buildQuery,
  clearQuery,
  type Match,
  selectMatch,
} from "../lib/editorSearch";
import { cn } from "./ui";

export interface SearchHandle {
  /** Open the panel (optionally with the replace row expanded). */
  open: (replace: boolean) => void;
  findNext: () => void;
  findPrevious: () => void;
}

interface Props {
  getView: () => EditorView | null;
  /** Editor text — recomputes the match count when the document changes. */
  sqlText: string;
}

function Toggle({
  active,
  title,
  onClick,
  children,
}: {
  active: boolean;
  title: string;
  onClick: () => void;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      className={cn(
        "flex size-5 items-center justify-center rounded transition-colors",
        active
          ? "bg-sky-600/30 text-sky-300"
          : "text-zinc-500 hover:bg-zinc-700/60 hover:text-zinc-200",
      )}
    >
      {children}
    </button>
  );
}

function NavButton({
  title,
  onClick,
  disabled,
  children,
}: {
  title: string;
  onClick: () => void;
  disabled?: boolean;
  children: ReactNode;
}) {
  return (
    <button
      type="button"
      title={title}
      onClick={onClick}
      disabled={disabled}
      className="flex size-6 items-center justify-center rounded text-zinc-400 transition-colors hover:bg-zinc-700/60 hover:text-zinc-100 disabled:opacity-40 disabled:pointer-events-none"
    >
      {children}
    </button>
  );
}

export const SearchPanel = forwardRef<SearchHandle, Props>(function SearchPanel(
  { getView, sqlText },
  ref,
) {
  const [open, setOpen] = useState(false);
  const [replaceMode, setReplaceMode] = useState(false);
  const [query, setQuery] = useState("");
  const [replace, setReplace] = useState("");
  const [caseSensitive, setCaseSensitive] = useState(false);
  const [wholeWord, setWholeWord] = useState(false);
  const [regexp, setRegexp] = useState(false);
  const [matches, setMatches] = useState<Match[]>([]);
  const [active, setActive] = useState(-1);

  const findInputRef = useRef<HTMLInputElement>(null);
  // Mirror match state into refs so event handlers / imperative calls read the
  // latest without being re-created (and going stale) on every render.
  const matchesRef = useRef<Match[]>(matches);
  matchesRef.current = matches;
  const activeRef = useRef(active);
  activeRef.current = active;

  const opts = { caseSensitive, wholeWord, regexp };
  const cmQuery = buildQuery(query, replace, opts);
  const invalid = query.length > 0 && !cmQuery.valid;

  const setMatchList = (ms: Match[], idx: number) => {
    matchesRef.current = ms;
    activeRef.current = idx;
    setMatches(ms);
    setActive(idx);
  };

  /** Re-highlight and (optionally) jump to the nearest match from the cursor. */
  const recompute = (moveSelection: boolean) => {
    const view = getView();
    if (!view) return;
    const q = buildQuery(query, replace, opts);
    applyQuery(view, q);
    const ms = allMatches(view, q);
    if (ms.length === 0) {
      setMatchList([], -1);
      return;
    }
    const cursor = view.state.selection.main.from;
    let idx = ms.findIndex((m) => m.to > cursor);
    if (idx < 0) idx = 0;
    setMatchList(ms, idx);
    if (moveSelection) selectMatch(view, ms[idx]);
  };

  /** Refresh the count after an edit, keeping the active index on the current
   *  selection — no jump. */
  const recount = () => {
    const view = getView();
    if (!view) return;
    const ms = allMatches(view, buildQuery(query, replace, opts));
    const sel = view.state.selection.main;
    let idx = ms.findIndex((m) => m.from === sel.from && m.to === sel.to);
    if (idx < 0) idx = ms.findIndex((m) => m.to > sel.from);
    if (idx < 0) idx = ms.length ? 0 : -1;
    setMatchList(ms, idx);
  };

  const go = (delta: number) => {
    const view = getView();
    const ms = matchesRef.current;
    if (!view || ms.length === 0) return;
    const idx = (activeRef.current + delta + ms.length) % ms.length;
    setMatchList(ms, idx);
    selectMatch(view, ms[idx]);
  };

  const doReplace = () => {
    const view = getView();
    const ms = matchesRef.current;
    if (!view || activeRef.current < 0 || ms.length === 0) return;
    const m = ms[activeRef.current];
    // Select the active match so CodeMirror's replaceNext acts on it (it also
    // handles $1 group references for regexp queries), then move on.
    view.dispatch({ selection: { anchor: m.from, head: m.to } });
    replaceNext(view);
    recount();
  };

  const doReplaceAll = () => {
    const view = getView();
    if (!view) return;
    applyQuery(view, buildQuery(query, replace, opts));
    replaceAll(view);
    setMatchList([], -1);
  };

  const close = () => {
    const view = getView();
    setOpen(false);
    if (view) {
      clearQuery(view);
      view.focus();
    }
  };

  const openPanel = (withReplace: boolean) => {
    const view = getView();
    if (view) {
      const sel = view.state.selection.main;
      if (sel.from !== sel.to) {
        const text = view.state.sliceDoc(sel.from, sel.to);
        if (text && !text.includes("\n")) setQuery(text);
      }
    }
    if (withReplace) setReplaceMode(true);
    setOpen(true);
    if (open) {
      findInputRef.current?.focus();
      findInputRef.current?.select();
    }
  };

  useImperativeHandle(ref, () => ({
    open: openPanel,
    findNext: () => go(1),
    findPrevious: () => go(-1),
  }));

  // Focus the field and run the first search when the panel opens.
  useEffect(() => {
    if (!open) return;
    findInputRef.current?.focus();
    findInputRef.current?.select();
    recompute(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Query text / options changed → re-highlight and jump to the nearest match.
  useEffect(() => {
    if (!open) return;
    recompute(true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [query, caseSensitive, wholeWord, regexp]);

  // Replace text changed → keep the editor's query.replace current for
  // replaceNext, without moving the selection.
  useEffect(() => {
    if (!open) return;
    const view = getView();
    if (view) applyQuery(view, buildQuery(query, replace, opts));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [replace]);

  // Document edited while the panel is open → refresh the count in place.
  useEffect(() => {
    if (!open) return;
    recount();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sqlText]);

  if (!open) return null;

  const total = matches.length;
  const counter = query
    ? total > 0
      ? `${active + 1}/${total}`
      : "0/0"
    : "";

  return (
    <div className="absolute right-2 top-2 z-20 rounded-md border border-zinc-700 bg-zinc-900/95 p-1 shadow-xl shadow-black/40 backdrop-blur-sm">
      <div className="flex items-start gap-1">
        <button
          type="button"
          title={replaceMode ? "Hide replace" : "Toggle replace"}
          onClick={() => setReplaceMode((v) => !v)}
          className="flex w-4 items-center justify-center self-stretch rounded text-zinc-500 hover:bg-zinc-700/60 hover:text-zinc-200"
        >
          <ChevronRight
            size={14}
            className={cn("transition-transform", replaceMode && "rotate-90")}
          />
        </button>

        <div className="flex flex-col gap-1">
          {/* find row */}
          <div className="flex items-center gap-1">
            <div className="relative">
              <input
                ref={findInputRef}
                value={query}
                onChange={(e) => setQuery(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    go(e.shiftKey ? -1 : 1);
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    close();
                  }
                }}
                placeholder="Find"
                spellCheck={false}
                autoCorrect="off"
                autoCapitalize="off"
                autoComplete="off"
                className={cn(
                  "w-56 rounded border bg-zinc-950 py-1 pl-2 pr-[68px] text-[12px] text-zinc-100 placeholder:text-zinc-600 focus:outline-none",
                  invalid
                    ? "border-red-600 focus:border-red-500"
                    : "border-zinc-700 focus:border-sky-600",
                )}
              />
              <div className="absolute right-1 top-1/2 flex -translate-y-1/2 gap-0.5">
                <Toggle
                  active={caseSensitive}
                  title="Match case"
                  onClick={() => setCaseSensitive((v) => !v)}
                >
                  <CaseSensitive size={14} />
                </Toggle>
                <Toggle
                  active={wholeWord}
                  title="Match whole word"
                  onClick={() => setWholeWord((v) => !v)}
                >
                  <WholeWord size={14} />
                </Toggle>
                <Toggle
                  active={regexp}
                  title="Use regular expression"
                  onClick={() => setRegexp((v) => !v)}
                >
                  <Regex size={14} />
                </Toggle>
              </div>
            </div>
            <span
              className={cn(
                "w-12 shrink-0 text-center text-[11px] tabular-nums",
                query && total === 0 ? "text-red-400" : "text-zinc-500",
              )}
            >
              {counter}
            </span>
            <NavButton
              title="Previous match · ⇧⏎"
              onClick={() => go(-1)}
              disabled={total === 0}
            >
              <ArrowUp size={14} />
            </NavButton>
            <NavButton
              title="Next match · ⏎"
              onClick={() => go(1)}
              disabled={total === 0}
            >
              <ArrowDown size={14} />
            </NavButton>
            <NavButton title="Close · Esc" onClick={close}>
              <X size={14} />
            </NavButton>
          </div>

          {/* replace row */}
          {replaceMode && (
            <div className="flex items-center gap-1">
              <input
                value={replace}
                onChange={(e) => setReplace(e.target.value)}
                onKeyDown={(e) => {
                  if (e.key === "Enter") {
                    e.preventDefault();
                    if (e.metaKey || e.ctrlKey) doReplaceAll();
                    else doReplace();
                  } else if (e.key === "Escape") {
                    e.preventDefault();
                    e.stopPropagation();
                    close();
                  }
                }}
                placeholder="Replace"
                spellCheck={false}
                autoCorrect="off"
                autoCapitalize="off"
                autoComplete="off"
                className="w-56 rounded border border-zinc-700 bg-zinc-950 py-1 px-2 text-[12px] text-zinc-100 placeholder:text-zinc-600 focus:border-sky-600 focus:outline-none"
              />
              <NavButton
                title="Replace · ⏎"
                onClick={doReplace}
                disabled={total === 0}
              >
                <Replace size={14} />
              </NavButton>
              <NavButton
                title="Replace all · ⌘⏎"
                onClick={doReplaceAll}
                disabled={total === 0}
              >
                <ReplaceAll size={14} />
              </NavButton>
            </div>
          )}
        </div>
      </div>
    </div>
  );
});
