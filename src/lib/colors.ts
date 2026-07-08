/** Accent colors a connection can be tagged with. */
export const ACCENT_VALUES: Record<string, string> = {
  red: "#ed6860",
  orange: "#f09340",
  yellow: "#f5d95c",
  green: "#66d89b",
  blue: "#72cdfa",
  purple: "#8f5bf6",
  pink: "#ee80f1",
};

export const ACCENTS = Object.keys(ACCENT_VALUES);

/** The "this is production" red — the footer/tab-bar chrome signal. */
export const PROD_RED = "#ef4444";

/** Overlay `color` at `pct`% onto a base color token — the accent/prod chrome
 *  tint used by the status bar and the tab bar. */
export const tint = (color: string, pct: number, base: string) =>
  `color-mix(in srgb, ${color} ${pct}%, ${base})`;

/** Profiles saved before the palette change stored Tailwind color names. */
const LEGACY: Record<string, string> = {
  sky: "blue",
  emerald: "green",
  amber: "yellow",
  rose: "red",
  violet: "purple",
};

/** CSS color for an accent, or null for the default theme look. */
export function accentColor(color?: string | null): string | null {
  if (!color) return null;
  return ACCENT_VALUES[color] ?? ACCENT_VALUES[LEGACY[color]] ?? null;
}
