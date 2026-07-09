// UI chrome: overlays (palette, dialogs, viewers), toast, theme/settings,
// sidebar and launcher visibility.
import { api, errText } from "../../api";
import { applyTheme } from "../../themes";
import type { AppSettings, Profile } from "../../types";
import type { Get, Set, StoreContext } from "../context";
import type { PaletteKind, Toast } from "../types";

export interface UiSlice {
  dialog: { open: boolean; profile?: Profile };
  toast: Toast | null;
  palette: PaletteKind | null;
  /** Query tab whose "save query" dialog is open (⌘S on an unsaved query). */
  saveDialogFor: string | null;
  /** Contents of settings.json (theme etc.) — loaded before the vault gate. */
  settings: AppSettings;
  /** Settings dialog (⌘,). */
  settingsOpen: boolean;
  /** Diagnostics-log viewer (menu → Diagnostics Log). */
  logViewerOpen: boolean;
  /** Sidebar visibility (⌘B). */
  sidebarHidden: boolean;
  /** Launcher explicitly opened over a live workspace ("All connections").
   *  With nothing connected the launcher shows regardless of this flag. */
  launcherOpen: boolean;

  setPalette: (palette: PaletteKind | null) => void;
  setSettingsOpen: (open: boolean) => void;
  setLogViewerOpen: (open: boolean) => void;
  toggleSidebar: () => void;
  setLauncherOpen: (open: boolean) => void;
  /** Applies the theme immediately and persists it to settings.json. */
  setTheme: (id: string) => Promise<void>;
  setSaveDialogFor: (tabId: string | null) => void;
  showToast: (message: string, kind?: Toast["kind"]) => void;
  openDialog: (profile?: Profile) => void;
  closeDialog: () => void;
}

let toastTimer: ReturnType<typeof setTimeout> | undefined;

export function createUiSlice(set: Set, get: Get, _ctx: StoreContext): UiSlice {
  return {
    dialog: { open: false },
    toast: null,
    palette: null,
    saveDialogFor: null,
    settings: {},
    settingsOpen: false,
    logViewerOpen: false,
    sidebarHidden: false,
    launcherOpen: false,

    setPalette: (palette) => set({ palette }),

    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),

    setLogViewerOpen: (logViewerOpen) => set({ logViewerOpen }),

    toggleSidebar: () => set((s) => ({ sidebarHidden: !s.sidebarHidden })),

    setLauncherOpen: (launcherOpen) => set({ launcherOpen }),

    setTheme: async (id) => {
      applyTheme(id);
      const settings = { ...get().settings, theme: id };
      set({ settings });
      try {
        await api.saveSettings(settings);
      } catch (e) {
        // theme is applied for this session; only the persistence failed
        get().showToast(`Settings not saved: ${errText(e)}`);
      }
    },

    setSaveDialogFor: (tabId) => set({ saveDialogFor: tabId }),

    showToast: (message, kind = "error") => {
      set({ toast: { message, kind } });
      if (toastTimer) clearTimeout(toastTimer);
      toastTimer = setTimeout(() => set({ toast: null }), 6000);
    },

    openDialog: (profile) => set({ dialog: { open: true, profile } }),
    closeDialog: () => set({ dialog: { open: false } }),
  };
}
