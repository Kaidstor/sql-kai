import { Check, X } from "lucide-react";
import { useEffect, useState } from "react";
import { api } from "../lib/api";
import {
  HOTKEY_ACTIONS,
  comboFromEvent,
  formatCombo,
  hotkeyOf,
} from "../lib/hotkeys";
import { isMac } from "../lib/platform";
import { useApp } from "../lib/store";
import { THEMES, themeById, type Theme } from "../lib/themes";
import { IconButton, Overlay, cn } from "./ui";

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

/** Кнопка-рекордер: клик — «Press keys…», следующее нажатие с ⌘/Ctrl
 *  становится биндингом. Esc отменяет запись, Reset возвращает дефолт. */
function HotkeyRow({
  label,
  combo,
  isDefault,
  onChange,
  onReset,
}: {
  label: string;
  combo: string;
  isDefault: boolean;
  onChange: (combo: string) => void;
  onReset: () => void;
}) {
  const [recording, setRecording] = useState(false);
  return (
    <div className="flex items-center justify-between gap-3 text-[12px]">
      <span className="text-zinc-300">{label}</span>
      <span className="flex items-center gap-1.5">
        {!isDefault && (
          <button
            type="button"
            onClick={onReset}
            className="text-[11px] text-zinc-500 hover:text-zinc-300"
          >
            reset
          </button>
        )}
        <button
          type="button"
          onClick={() => setRecording(true)}
          onBlur={() => setRecording(false)}
          onKeyDown={(e) => {
            if (!recording) return;
            // не отдавать нажатие глобальным хоткеям и Esc оверлея
            e.stopPropagation();
            if (e.key === "Escape") {
              setRecording(false);
              return;
            }
            if (["Shift", "Alt", "Control", "Meta"].includes(e.key)) return;
            e.preventDefault();
            const next = comboFromEvent(e.nativeEvent);
            if (next) {
              onChange(next);
              setRecording(false);
            }
          }}
          className={cn(
            "min-w-16 rounded border px-2 py-1 font-sans text-[11px] leading-none",
            recording
              ? "border-sky-600 bg-sky-950/40 text-sky-300"
              : "border-zinc-700 bg-zinc-800/80 text-zinc-200 hover:border-zinc-600",
          )}
        >
          {recording ? "Press keys…" : formatCombo(combo)}
        </button>
      </span>
    </div>
  );
}

export function SettingsDialog() {
  const settingsOpen = useApp((s) => s.settingsOpen);
  const setSettingsOpen = useApp((s) => s.setSettingsOpen);
  const setLogViewerOpen = useApp((s) => s.setLogViewerOpen);
  const settings = useApp((s) => s.settings);
  const setTheme = useApp((s) => s.setTheme);
  const setHotkey = useApp((s) => s.setHotkey);
  const [path, setPath] = useState<string | null>(null);
  const [logPath, setLogPath] = useState<string | null>(null);

  useEffect(() => {
    if (!settingsOpen) return;
    api
      .settingsPath()
      .then(setPath)
      .catch(() => setPath(null));
    api
      .logPath()
      .then(setLogPath)
      .catch(() => setLogPath(null));
  }, [settingsOpen]);

  if (!settingsOpen) return null;
  const current = themeById(settings.theme);

  return (
    <Overlay onClose={() => setSettingsOpen(false)} closeOnEsc className="items-center bg-black/60">
      <div className="w-130 max-h-[90vh] overflow-y-auto rounded-lg border border-zinc-700 bg-zinc-900 shadow-2xl">
        <div className="flex items-center justify-between px-4 py-3 border-b border-zinc-800">
          <div className="text-[13px] font-semibold text-zinc-100">Settings</div>
          <IconButton onClick={() => setSettingsOpen(false)}>
            <X size={15} />
          </IconButton>
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

        <div className="border-t border-zinc-800 p-4">
          <div className="mb-2 text-[11px] text-zinc-400">Hotkeys</div>
          <div className="flex flex-col gap-2">
            {HOTKEY_ACTIONS.map((a) => (
              <HotkeyRow
                key={a.id}
                label={a.label}
                combo={hotkeyOf(settings, a.id)}
                isDefault={hotkeyOf(settings, a.id) === a.default}
                onChange={(combo) => void setHotkey(a.id, combo)}
                onReset={() => void setHotkey(a.id, null)}
              />
            ))}
          </div>
          <div className="mt-2 text-[11px] text-zinc-600">
            Click the binding, then press the new combination (must include{" "}
            {isMac ? "⌘" : "Ctrl"}).
          </div>
        </div>

        <div className="border-t border-zinc-800 px-4 py-3 text-[11px] text-zinc-500">
          Settings live in{" "}
          <span className="selectable font-mono text-zinc-400">
            {path ?? "settings.json"}
          </span>
          {" "}— copy the file to another machine to carry them over.
          <br />
          Connection diagnostics (why a session dropped) are logged to{" "}
          <span className="selectable font-mono text-zinc-400">
            {logPath ?? "sql-kai.log"}
          </span>{" "}
          —{" "}
          <button
            type="button"
            className="text-sky-400 hover:underline"
            onClick={() => {
              setSettingsOpen(false);
              setLogViewerOpen(true);
            }}
          >
            view
          </button>
          .
        </div>
      </div>
    </Overlay>
  );
}
