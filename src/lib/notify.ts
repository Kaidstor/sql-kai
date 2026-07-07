import {
  isPermissionGranted,
  requestPermission,
  sendNotification,
} from "@tauri-apps/plugin-notification";
import { sqlPreview } from "./sql";

/** Queries at least this long ping a native notification when they finish
 *  while the window is in the background. */
const LONG_QUERY_MS = 10_000;

export async function notifyQueryDone(opts: {
  profileName: string;
  durationMs: number;
  ok: boolean;
  sql: string;
}) {
  if (opts.durationMs < LONG_QUERY_MS || document.hasFocus()) return;
  try {
    let granted = await isPermissionGranted();
    if (!granted) granted = (await requestPermission()) === "granted";
    if (!granted) return;
    const secs = (opts.durationMs / 1000).toFixed(1);
    sendNotification({
      title: opts.ok
        ? `Query finished in ${secs}s`
        : `Query failed after ${secs}s`,
      body: `${opts.profileName} — ${sqlPreview(opts.sql, 80)}`,
    });
  } catch {
    // notifications are best-effort
  }
}
