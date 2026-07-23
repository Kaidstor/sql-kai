import { revealItemInDir } from "@tauri-apps/plugin-opener";
import { FolderOpen, X } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import { IconButton, Overlay } from "./ui";

const POLL_MS = 2000;

/** Live tail of the backend diagnostics log (menu → Diagnostics Log).
 *  Polls while open and stays pinned to the newest lines unless the user
 *  scrolled up to read something. */
export function LogViewer() {
  const logViewerOpen = useApp((s) => s.logViewerOpen);
  const setLogViewerOpen = useApp((s) => s.setLogViewerOpen);
  const [text, setText] = useState("");
  const [path, setPath] = useState<string | null>(null);
  const scrollRef = useRef<HTMLPreElement>(null);
  const stickToBottom = useRef(true);

  useEffect(() => {
    if (!logViewerOpen) return;
    stickToBottom.current = true;
    let cancelled = false;
    const load = () =>
      api
        .getLog()
        .then((t) => !cancelled && setText(t))
        .catch(() => {
          // file may not exist yet — keep whatever is on screen
        });
    void load();
    const timer = setInterval(load, POLL_MS);
    api
      .logPath()
      .then((p) => !cancelled && setPath(p))
      .catch(() => {});
    return () => {
      cancelled = true;
      clearInterval(timer);
    };
  }, [logViewerOpen]);

  // Each refresh keeps the view pinned to the tail (unless scrolled away).
  useEffect(() => {
    const el = scrollRef.current;
    if (el && stickToBottom.current) el.scrollTop = el.scrollHeight;
  }, [text]);

  if (!logViewerOpen) return null;

  return (
    <Overlay
      onClose={() => setLogViewerOpen(false)}
      closeOnEsc
      className="items-center bg-black/60"
    >
      <div className="flex h-[80vh] w-[85vw] max-w-4xl flex-col rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
        <div className="flex items-center justify-between border-b border-zinc-800 px-4 py-3">
          <div className="text-[13px] font-semibold text-zinc-100">
            Diagnostics log
          </div>
          <div className="flex items-center gap-1">
            {path && (
              <IconButton
                title="Reveal in Finder"
                onClick={() => void revealItemInDir(path)}
              >
                <FolderOpen size={14} />
              </IconButton>
            )}
            <IconButton onClick={() => setLogViewerOpen(false)}>
              <X size={15} />
            </IconButton>
          </div>
        </div>

        <pre
          ref={scrollRef}
          onScroll={(e) => {
            const el = e.currentTarget;
            stickToBottom.current =
              el.scrollTop + el.clientHeight >= el.scrollHeight - 8;
          }}
          className="selectable flex-1 overflow-auto whitespace-pre-wrap px-4 py-3 font-mono text-[11px] leading-relaxed text-zinc-300"
        >
          {text ||
            "Log is empty — connection lifecycle events (tunnels, drops, reconnects) will appear here."}
        </pre>

        <div className="flex items-center gap-2 border-t border-zinc-800 px-4 py-2 text-[11px] text-zinc-500">
          <span className="selectable min-w-0 truncate font-mono">
            {path ?? ""}
          </span>
          <span className="ml-auto shrink-0">
            refreshes every {POLL_MS / 1000}s
          </span>
        </div>
      </div>
    </Overlay>
  );
}
