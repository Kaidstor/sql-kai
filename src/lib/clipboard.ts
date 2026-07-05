import { invoke } from "@tauri-apps/api/core";
import {
  readText as tauriReadText,
  writeText as tauriWriteText,
} from "@tauri-apps/plugin-clipboard-manager";

export async function copyText(text: string): Promise<boolean> {
  try {
    await tauriWriteText(text);
    return true;
  } catch {
    // plugin unavailable (e.g. plain browser during dev) — fall back
  }
  try {
    await navigator.clipboard.writeText(text);
    return true;
  } catch {
    // WKWebView may reject clipboard API in some contexts — fall back
  }
  const ta = document.createElement("textarea");
  ta.value = text;
  ta.style.position = "fixed";
  ta.style.opacity = "0";
  document.body.appendChild(ta);
  ta.select();
  const ok = document.execCommand("copy");
  ta.remove();
  return ok;
}

/** Copies with `org.nspasteboard.ConcealedType` so clipboard-history
 *  managers (Maccy, Paste, Raycast…) don't record the item. */
export async function copyTextConcealed(text: string): Promise<boolean> {
  try {
    await invoke("copy_text_concealed", { text });
    return true;
  } catch {
    // plain browser during dev — a regular copy is better than nothing
  }
  return copyText(text);
}

/** WKWebView doesn't fire `paste` events on non-editable elements and
 *  rejects navigator.clipboard.readText — the plugin reads natively. */
export async function readClipboardText(): Promise<string | null> {
  try {
    return await tauriReadText();
  } catch {
    // plugin unavailable — try the web API
  }
  try {
    return await navigator.clipboard.readText();
  } catch {
    return null;
  }
}
