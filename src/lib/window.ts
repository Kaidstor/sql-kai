import { getCurrentWindow } from "@tauri-apps/api/window";
import type { MouseEvent } from "react";

/** Anything clickable in a titlebar: clicks go to the element, not the drag. */
const INTERACTIVE =
  "button, a, input, textarea, select, [role='button'], [data-tab-id]";

// `data-tauri-drag-region` only fires when the target itself carries the
// attribute, so any nested span in a header swallows the drag. Instead we
// catch mousedown on the whole header and decide ourselves: interactive
// element — let the click through, otherwise drag the window (double click
// zooms, macOS-style).
export function dragWindow(e: MouseEvent<HTMLElement>) {
  if (e.button !== 0) return;
  if ((e.target as HTMLElement).closest(INTERACTIVE)) return;
  e.preventDefault();
  const win = getCurrentWindow();
  void (e.detail === 2 ? win.toggleMaximize() : win.startDragging());
}
