// "What's new" — shown once after an app update: the latest releases with
// the unseen ones badged (see lib/whatsNew).
import { ExternalLink, Sparkles, X } from "lucide-react";
import { openUrl } from "@tauri-apps/plugin-opener";
import { useEffect, useState } from "react";
import {
  RELEASES_PAGE_URL,
  collectWhatsNew,
  type ReleaseNote,
} from "../lib/whatsNew";
import { Button, IconButton, Overlay } from "./ui";

/** Changelog lines: "- item" → bullet, anything else — a paragraph. */
function NoteLines({ text }: { text: string }) {
  return (
    <div className="space-y-1 text-[12px] leading-relaxed text-zinc-300">
      {text.split("\n").map((line, i) => {
        const t = line.trim();
        if (!t) return null;
        const bullet = t.match(/^[-*]\s+(.*)$/);
        return bullet ? (
          <div key={i} className="flex gap-2">
            <span className="text-zinc-600">•</span>
            <span>{bullet[1]}</span>
          </div>
        ) : (
          <p key={i}>{t}</p>
        );
      })}
    </div>
  );
}

export function WhatsNewDialog() {
  const [notes, setNotes] = useState<ReleaseNote[]>([]);

  useEffect(() => {
    void collectWhatsNew().then((r) => {
      if (r.length > 0) setNotes(r);
    });
  }, []);

  if (notes.length === 0) return null;
  const close = () => setNotes([]);

  return (
    <Overlay onClose={close} closeOnEsc className="items-center bg-black/60">
      <div className="w-[30rem] max-w-[92vw] self-center rounded-lg border border-zinc-700 bg-zinc-900 p-4 shadow-2xl">
        <div className="mb-3 flex items-center justify-between">
          <h2 className="flex items-center gap-2 text-[13px] font-semibold text-zinc-100">
            <Sparkles size={14} className="text-sky-400" />
            What's new
          </h2>
          <IconButton onClick={close}>
            <X size={15} />
          </IconButton>
        </div>

        <div className="selectable max-h-[50vh] space-y-4 overflow-y-auto pr-1">
          {notes.map((n) => (
            <div key={n.version}>
              <div className="mb-1 flex items-center gap-2">
                <span className="text-[11px] font-semibold text-sky-400">
                  v{n.version}
                </span>
                {n.fresh && (
                  <span className="rounded border border-sky-500/40 bg-sky-500/10 px-1 py-px text-[9px] font-semibold tracking-wide text-sky-400">
                    NEW
                  </span>
                )}
              </div>
              <NoteLines text={n.notes} />
            </div>
          ))}
        </div>

        <div className="mt-4 flex items-center justify-between">
          <Button
            title="Open the releases page in the browser"
            onClick={() => void openUrl(RELEASES_PAGE_URL)}
          >
            <ExternalLink size={12} /> All releases
          </Button>
          <Button variant="primary" onClick={close}>
            OK
          </Button>
        </div>
      </div>
    </Overlay>
  );
}
