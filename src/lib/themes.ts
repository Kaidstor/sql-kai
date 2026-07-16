// App themes. The UI is styled with Tailwind's zinc scale plus a handful of
// semantic accents (sky/red/emerald/amber/violet); Tailwind v4 compiles those
// utilities to `var(--color-*)`, so a theme is just a set of CSS-variable
// overrides on <html> — no component changes needed. Light themes invert the
// scale: zinc-950 stays "app background" and zinc-100 stays "primary text".
import { EditorView } from "@codemirror/view";
import { vscodeDarkInit, vscodeLightInit } from "@uiw/codemirror-theme-vscode";

export interface Theme {
  id: string;
  label: string;
  /** Light UI: flips native color-scheme and the SQL editor palette. */
  light?: boolean;
  /** CSS variable overrides; empty = Tailwind's built-in palette. */
  vars: Record<string, string>;
  /** Swatch colors for the theme picker. */
  preview: { bg: string; panel: string; text: string; accent: string };
}

export const THEMES: Theme[] = [
  {
    id: "dark",
    label: "Dark",
    vars: {},
    preview: { bg: "#09090b", panel: "#27272a", text: "#e4e4e7", accent: "#0284c7" },
  },
  {
    id: "github-dark",
    label: "GitHub Dark",
    preview: { bg: "#0d1117", panel: "#21262d", text: "#c9d1d9", accent: "#1f6feb" },
    vars: {
      "--color-zinc-50": "#f0f6fc",
      "--color-zinc-100": "#e6edf3",
      "--color-zinc-200": "#c9d1d9",
      "--color-zinc-300": "#b1bac4",
      "--color-zinc-400": "#8b949e",
      "--color-zinc-500": "#6e7681",
      "--color-zinc-600": "#484f58",
      "--color-zinc-700": "#30363d",
      "--color-zinc-800": "#21262d",
      "--color-zinc-900": "#161b22",
      "--color-zinc-925": "#11161d",
      "--color-zinc-950": "#0d1117",
      "--color-sky-300": "#79c0ff",
      "--color-sky-400": "#58a6ff",
      "--color-sky-500": "#388bfd",
      "--color-sky-600": "#1f6feb",
      "--color-sky-950": "#132339",
      "--color-red-300": "#ffa198",
      "--color-red-400": "#ff7b72",
      "--color-red-500": "#f85149",
      "--color-red-600": "#da3633",
      "--color-red-900": "#67060c",
      "--color-red-950": "#3c0614",
      "--color-emerald-100": "#aff5b4",
      "--color-emerald-300": "#56d364",
      "--color-emerald-400": "#3fb950",
      "--color-emerald-500": "#2ea043",
      "--color-emerald-900": "#033a16",
      "--color-emerald-950": "#04260f",
      "--color-amber-200": "#f2dfa7",
      "--color-amber-400": "#d29922",
      "--color-amber-500": "#bb8009",
      "--color-violet-400": "#a371f7",
    },
  },
  {
    id: "youtrack",
    label: "YouTrack",
    preview: { bg: "#1e1f22", panel: "#393b40", text: "#dfe1e5", accent: "#3574f0" },
    vars: {
      "--color-zinc-50": "#f0f1f2",
      "--color-zinc-100": "#dfe1e5",
      "--color-zinc-200": "#ced0d6",
      "--color-zinc-300": "#b4b8bf",
      "--color-zinc-400": "#9da0a8",
      "--color-zinc-500": "#7f838c",
      "--color-zinc-600": "#5a5d63",
      "--color-zinc-700": "#43454a",
      "--color-zinc-800": "#393b40",
      "--color-zinc-900": "#2b2d30",
      "--color-zinc-925": "#242628",
      "--color-zinc-950": "#1e1f22",
      "--color-sky-300": "#a0bdf8",
      "--color-sky-400": "#7ba4f7",
      "--color-sky-500": "#4d84f5",
      "--color-sky-600": "#3574f0",
      "--color-sky-950": "#25324d",
      "--color-red-300": "#f0a3a3",
      "--color-red-400": "#e57e7e",
      "--color-red-500": "#db5c5c",
      "--color-red-600": "#cc4d4d",
      "--color-red-900": "#5c3535",
      "--color-red-950": "#3d2828",
      "--color-emerald-100": "#d9f2db",
      "--color-emerald-300": "#8fd095",
      "--color-emerald-400": "#73bd79",
      "--color-emerald-500": "#5dac64",
      "--color-emerald-900": "#2a3c2c",
      "--color-emerald-950": "#222e23",
      "--color-amber-200": "#ecd5a2",
      "--color-amber-400": "#d6ae58",
      "--color-amber-500": "#c69a3f",
      "--color-violet-400": "#b18bf0",
    },
  },
  {
    id: "sand",
    label: "Песочная",
    light: true,
    preview: { bg: "#efe6d2", panel: "#f8f3e6", text: "#3d352a", accent: "#3d6a99" },
    vars: {
      "--color-zinc-50": "#1c1812",
      "--color-zinc-100": "#2b251d",
      "--color-zinc-200": "#3d352a",
      "--color-zinc-300": "#514736",
      "--color-zinc-400": "#675c46",
      "--color-zinc-500": "#82745a",
      "--color-zinc-600": "#a29372",
      "--color-zinc-700": "#ccbf9f",
      "--color-zinc-800": "#e0d5bb",
      "--color-zinc-900": "#f8f3e6",
      "--color-zinc-925": "#e8ddc4",
      "--color-zinc-950": "#efe6d2",
      "--color-sky-300": "#2c5480",
      "--color-sky-400": "#35618f",
      "--color-sky-500": "#4a7aad",
      "--color-sky-600": "#3d6a99",
      "--color-sky-950": "#dfe9f2",
      "--color-red-300": "#9a2f27",
      "--color-red-400": "#b0392f",
      "--color-red-500": "#c24438",
      "--color-red-600": "#a83a30",
      "--color-red-900": "#e6beb5",
      "--color-red-950": "#f6e2da",
      "--color-emerald-100": "#1d4a33",
      "--color-emerald-300": "#1d5e3c",
      "--color-emerald-400": "#217547",
      "--color-emerald-500": "#268a54",
      "--color-emerald-900": "#bfe0cc",
      "--color-emerald-950": "#e2f2e8",
      "--color-amber-200": "#5f430e",
      "--color-amber-400": "#7d5a10",
      "--color-amber-500": "#8f6a12",
      "--color-violet-400": "#6a4fa0",
    },
  },
];

export const themeById = (id?: string | null): Theme =>
  THEMES.find((t) => t.id === id) ?? THEMES[0];

const CACHE_KEY = "sqlt.theme";

/** Union of vars any theme touches — cleared before applying so switching to
 *  a theme with fewer overrides (e.g. the default) leaves no leftovers. */
const ALL_VARS = [...new Set(THEMES.flatMap((t) => Object.keys(t.vars)))];

export function applyTheme(id?: string | null) {
  const theme = themeById(id);
  const root = document.documentElement;
  for (const v of ALL_VARS) root.style.removeProperty(v);
  for (const [k, v] of Object.entries(theme.vars)) root.style.setProperty(k, v);
  root.style.colorScheme = theme.light ? "light" : "dark";
  try {
    localStorage.setItem(CACHE_KEY, theme.id);
  } catch {
    // best-effort
  }
}

/** Pre-render apply of the last used theme. settings.json (the source of
 *  truth) loads async in init() — this avoids a flash of the default theme. */
export function applyCachedTheme() {
  try {
    applyTheme(localStorage.getItem(CACHE_KEY));
  } catch {
    applyTheme(null);
  }
}

/** Themes the search-match highlights (from editorSearch.ts) to the app's UI
 *  kit. Colors ride the theme CSS variables, so every app theme — including
 *  light ones — gets its own palette from this one extension. */
const searchTheme = EditorView.theme({
  ".cm-searchMatch": {
    backgroundColor: "color-mix(in srgb, var(--color-amber-400) 28%, transparent)",
    borderRadius: "2px",
  },
  ".cm-searchMatch-selected": {
    backgroundColor: "color-mix(in srgb, var(--color-sky-500) 55%, transparent)",
    outline: "1px solid var(--color-sky-400)",
  },
  ".cm-selectionMatch": {
    backgroundColor: "color-mix(in srgb, var(--color-zinc-400) 22%, transparent)",
  },
});

/** SQL editor palettes with a transparent background so the app theme shows
 *  through; the syntax colors come from the vscode presets. */
export const editorThemes = {
  dark: [
    vscodeDarkInit({
      settings: { background: "transparent", gutterBackground: "transparent" },
    }),
    searchTheme,
  ],
  light: [
    vscodeLightInit({
      settings: { background: "transparent", gutterBackground: "transparent" },
    }),
    searchTheme,
  ],
};
