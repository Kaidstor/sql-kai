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
