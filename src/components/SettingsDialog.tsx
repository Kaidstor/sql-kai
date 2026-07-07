import { Check, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import { useApp } from "../lib/store";
import { THEMES, themeById, type Theme } from "../lib/themes";
import { IconBtn, Overlay, cn } from "./ui";

function ThemeCard({
  theme,
  active,
  onPick,
}: {
  theme: Theme;
  active: boolean;
  onPick: () => void;
}) {
  return (
    <button
      onClick={onPick}
      className={cn(
        "flex flex-col gap-1.5 rounded-md border p-2 text-left transition-colors",
        active
          ? "border-sky-600 bg-zinc-800/40"
          : "border-zinc-700 hover:border-zinc-600 hover:bg-zinc-800/30",
      )}
    >
      <span
        className="flex h-10 w-full items-center gap-1.5 rounded border border-black/20 px-2"
        style={{ background: theme.preview.bg }}
      >
        <span
          className="size-2.5 shrink-0 rounded-full"
          style={{ background: theme.preview.accent }}
        />
        <span
          className="h-1.5 flex-1 rounded-sm"
          style={{ background: theme.preview.panel }}
        />
        <span
          className="h-1.5 w-8 rounded-sm opacity-80"
          style={{ background: theme.preview.text }}
        />
      </span>
      <span className="flex items-center gap-1 text-[12px] text-zinc-200">
        {theme.label}
        {active && <Check size={12} className="ml-auto text-sky-400" />}
      </span>
    </button>
  );
}

export function SettingsDialog() {
  const { settingsOpen, setSettingsOpen, settings, setTheme } = useApp();
  const [path, setPath] = useState<string | null>(null);

  useEffect(() => {
    if (!settingsOpen) return;
    api
      .settingsPath()
      .then(setPath)
      .catch(() => setPath(null));
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setSettingsOpen(false);
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [settingsOpen, setSettingsOpen]);

  if (!settingsOpen) return null;
  const current = themeById(settings.theme);

  return (
    <Overlay onClose={() => setSettingsOpen(false)} className="items-center bg-black/60">
      <div className="w-130 max-h-[90vh] overflow-y-auto rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-800">
          <div className="text-[13px] font-semibold text-zinc-100">Settings</div>
          <IconBtn onClick={() => setSettingsOpen(false)}>
            <X size={15} />
          </IconBtn>
        </div>

        <div className="p-4">
          <div className="mb-2 text-[11px] text-zinc-400">Theme</div>
          <div className="grid grid-cols-2 gap-2">
            {THEMES.map((t) => (
              <ThemeCard
                key={t.id}
                theme={t}
                active={t.id === current.id}
                onPick={() => void setTheme(t.id)}
              />
            ))}
          </div>
        </div>

        <div className="border-t border-zinc-800 px-4 py-3 text-[11px] text-zinc-500">
          Settings live in{" "}
          <span className="selectable font-mono text-zinc-400">
            {path ?? "settings.json"}
          </span>
          {" "}— copy the file to another machine to carry them over.
        </div>
      </div>
    </Overlay>
  );
}
