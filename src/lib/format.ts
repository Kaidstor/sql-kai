/** Shared number/time formatters for read-only display (grids, plans, activity). */

/** Coarse human duration from seconds; "" for null/negative (unknown). */
export function fmtDuration(sec: number | null): string {
  if (sec === null || sec < 0) return "";
  if (sec < 60) return `${sec}s`;
  if (sec < 3600) return `${Math.floor(sec / 60)}m ${sec % 60}s`;
  return `${Math.floor(sec / 3600)}h ${Math.floor((sec % 3600) / 60)}m`;
}

/** Milliseconds, promoted to seconds past 1000ms. */
export const fmtMs = (ms: number): string =>
  ms >= 1000 ? `${(ms / 1000).toFixed(2)} s` : `${ms.toFixed(2)} ms`;

/** Compact count: 1.2M / 34k / 1234 (small values keep 2 decimals). */
export const fmtNum = (n: number): string => {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 10_000) return `${(n / 1000).toFixed(0)}k`;
  return String(Math.round(n * 100) / 100);
};
