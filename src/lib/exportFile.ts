// Save-dialog helper shared by the grid's export actions (copyActions) and
// the Export menu — one place for the default name and filter shape.
import { save } from "@tauri-apps/plugin-dialog";

/** Prompts for an export target path: `base.ext` default, ext-filtered.
 *  Resolves null when the user cancels the dialog. */
export function promptExportPath(base: string, ext: string): Promise<string | null> {
  return save({
    defaultPath: `${base}.${ext}`,
    filters: [
      { name: ext.toUpperCase(), extensions: [ext] },
      { name: "All files", extensions: ["*"] },
    ],
  });
}
