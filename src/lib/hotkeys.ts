// Перенастраиваемые хоткеи (settings.json → hotkeys). Комбо хранится как
// "mod+shift+k": mod — ⌘ на mac / Ctrl на прочих платформах, клавиша —
// латинская буква или цифра.

import { isKey, keyDigit } from "./keys";
import { isMac } from "./platform";
import type { AppSettings } from "./types";

/** Действия с настраиваемым биндингом; id — ключ в settings.hotkeys. */
export const HOTKEY_ACTIONS = [
  {
    id: "queriesPalette",
    label: "Saved queries / tables palette",
    default: "mod+k",
  },
] as const;

export type HotkeyActionId = (typeof HOTKEY_ACTIONS)[number]["id"];

/** Текущее комбо действия; кривое значение из settings.json тихо
 *  откатывается к дефолту, чтобы действие не осталось без хоткея. */
export function hotkeyOf(settings: AppSettings, id: HotkeyActionId): string {
  const action = HOTKEY_ACTIONS.find((a) => a.id === id)!;
  const custom = settings.hotkeys?.[id];
  return typeof custom === "string" && parseCombo(custom)
    ? custom
    : action.default;
}

interface ParsedCombo {
  shift: boolean;
  alt: boolean;
  key: string;
}

export function parseCombo(combo: string): ParsedCombo | null {
  const parts = combo.toLowerCase().split("+");
  const key = parts[parts.length - 1];
  if (!/^[a-z0-9]$/.test(key)) return null;
  const mods = new Set(parts.slice(0, -1));
  if (!mods.delete("mod")) return null; // комбо без ⌘/Ctrl не принимаем
  const shift = mods.delete("shift");
  const alt = mods.delete("alt");
  if (mods.size > 0) return null;
  return { shift, alt, key };
}

export function matchesCombo(
  e: Pick<
    KeyboardEvent,
    "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"
  >,
  combo: string,
): boolean {
  const p = parseCombo(combo);
  if (!p) return false;
  if (!(e.metaKey || e.ctrlKey)) return false;
  if (e.shiftKey !== p.shift || e.altKey !== p.alt) return false;
  return /[0-9]/.test(p.key) ? keyDigit(e) === Number(p.key) : isKey(e, p.key);
}

/** Комбо из нажатия в рекордере настроек; null — в хоткей не годится
 *  (нет ⌘/Ctrl или клавиша не буква/цифра). Клавиша берётся по e.code —
 *  раскладка и Alt-символы mac ("˚" на ⌥K) на запись не влияют. */
export function comboFromEvent(
  e: Pick<
    KeyboardEvent,
    "key" | "code" | "metaKey" | "ctrlKey" | "altKey" | "shiftKey"
  >,
): string | null {
  if (!(e.metaKey || e.ctrlKey)) return null;
  const letter = /^Key([A-Z])$/.exec(e.code)?.[1]?.toLowerCase();
  const key = letter ?? (keyDigit(e) !== null ? String(keyDigit(e)) : null);
  if (!key) return null;
  return ["mod", e.altKey && "alt", e.shiftKey && "shift", key]
    .filter(Boolean)
    .join("+");
}

/** Части комбо для <kbd>-плашек шпаргалки: ["⌘", "⇧", "K"]. */
export function comboKeys(combo: string): string[] {
  const p = parseCombo(combo);
  if (!p) return [combo];
  return [
    isMac ? "⌘" : "Ctrl",
    p.alt && (isMac ? "⌥" : "Alt"),
    p.shift && (isMac ? "⇧" : "Shift"),
    p.key.toUpperCase(),
  ].filter((x): x is string => Boolean(x));
}

/** Комбо одной строкой для тултипов: "⌘⇧K" / "Ctrl+Shift+K". */
export function formatCombo(combo: string): string {
  const keys = comboKeys(combo);
  return isMac ? keys.join("") : keys.join("+");
}
