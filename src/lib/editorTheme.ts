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
