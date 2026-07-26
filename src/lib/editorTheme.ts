// SQL-editor palettes over the app themes (see themes.ts). A separate module
// on purpose: themes.ts is imported at startup (SettingsDialog, applyTheme),
// and these are the only theme pieces that pull CodeMirror — keeping them here
// lets the editor chunk load lazily with QueryTab.
import { EditorView } from "@codemirror/view";
import { vscodeDarkInit, vscodeLightInit } from "@uiw/codemirror-theme-vscode";

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

/** One completion-icon letter badge; `--color-${accent}-400` must exist in
 *  every theme's vars (see themes.ts) or the badge loses its color there. */
const completionIcon = (letter: string, accent: string) => ({
  color: `var(--color-${accent}-400)`,
  backgroundColor: `color-mix(in srgb, var(--color-${accent}-400) 15%, transparent)`,
  "&:after": { content: `'${letter}'` },
});

/** Autocomplete dropdown re-skinned to the app's menu kit (see ContextMenu):
 *  zinc-900 popup, zinc-700 border, rounded rows, zinc highlight on the
 *  selected row, sky on the typed-prefix match. The default CM icons (🔑 and
 *  friends) become letter badges for the types lang-sql emits: keyword,
 *  type (tables + builtin types), property (columns), constant (aliases),
 *  variable (words picked up from the doc). Colors ride the theme CSS
 *  variables, so light themes get their palette for free. */
const completionTheme = EditorView.theme({
  ".cm-tooltip": {
    backgroundColor: "var(--color-zinc-900)",
    border: "1px solid var(--color-zinc-700)",
    borderRadius: "6px",
    color: "var(--color-zinc-200)",
    boxShadow: "0 12px 28px -8px rgb(0 0 0 / 0.5)",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul": {
    fontFamily: "var(--font-mono)",
    fontSize: "12px",
    maxHeight: "16em",
    padding: "4px",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li": {
    display: "flex",
    alignItems: "center",
    gap: "7px",
    padding: "2px 8px 2px 5px",
    borderRadius: "4px",
    lineHeight: "1.5",
    color: "var(--color-zinc-200)",
  },
  ".cm-tooltip.cm-tooltip-autocomplete > ul > li[aria-selected]": {
    background: "color-mix(in srgb, var(--color-zinc-700) 60%, transparent)",
    color: "var(--color-zinc-50)",
  },
  ".cm-completionMatchedText": {
    textDecoration: "none",
    color: "var(--color-sky-400)",
  },
  ".cm-completionDetail": {
    marginLeft: "auto",
    fontStyle: "normal",
    color: "var(--color-zinc-500)",
  },
  ".cm-completionIcon": {
    flex: "none",
    boxSizing: "border-box",
    width: "14px",
    padding: "0",
    fontSize: "10px",
    fontWeight: "700",
    lineHeight: "14px",
    textAlign: "center",
    borderRadius: "3px",
    opacity: "1",
  },
  ".cm-completionIcon-keyword": completionIcon("K", "violet"),
  ".cm-completionIcon-type": completionIcon("T", "sky"),
  ".cm-completionIcon-property": completionIcon("C", "emerald"),
  ".cm-completionIcon-constant": completionIcon("A", "amber"),
  ".cm-completionIcon-variable": completionIcon("V", "zinc"),
  // No completion in the app has an info block today; styled so a future one
  // doesn't come up as an unreadable transparent box hanging off the tooltip.
  ".cm-completionInfo": {
    backgroundColor: "var(--color-zinc-900)",
    border: "1px solid var(--color-zinc-700)",
    borderRadius: "6px",
    padding: "4px 8px",
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
    completionTheme,
  ],
  light: [
    vscodeLightInit({
      settings: { background: "transparent", gutterBackground: "transparent" },
    }),
    searchTheme,
    completionTheme,
  ],
};
