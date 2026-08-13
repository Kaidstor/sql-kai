// UI chrome: overlays (palette, dialogs, viewers), toast, theme/settings,
// sidebar and launcher visibility.
import { api, errText } from "../../api";
import type { HotkeyActionId } from "../../hotkeys";
import { applyTheme } from "../../themes";
import type { AppSettings, Profile } from "../../types";
import type { Get, Set, StoreContext } from "../context";
import type { ConfirmRequest, PaletteKind, Toast } from "../types";

export interface UiSlice {
  dialog: { open: boolean; profile?: Profile };
  toast: Toast | null;
  /** Pending confirm dialog; null when closed (see confirmDialog). */
  confirm: ConfirmRequest | null;
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
  sidebarOpen: boolean;
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
  /** Перебиндить хоткей действия (null — вернуть дефолт); действует сразу
   *  и сохраняется в settings.json. */
  setHotkey: (id: HotkeyActionId, combo: string | null) => Promise<void>;
  setSaveDialogFor: (tabId: string | null) => void;
  /** revealPath — путь сохранённого файла: тост получает кнопку «Reveal in
   *  Finder» и живёт дольше, чтобы до неё успели дотянуться. */
  showToast: (
    message: string,
    kind?: Toast["kind"],
    revealPath?: string,
  ) => void;
  /** Opens the in-app confirm dialog; resolves true on confirm, false on
   *  cancel/Esc/backdrop. Replaces window.confirm(), which doesn't block
   *  in the Tauri webview. */
  confirmDialog: (req: ConfirmRequest) => Promise<boolean>;
  /** ConfirmDialog's buttons report the outcome here. */
  resolveConfirm: (ok: boolean) => void;
  openDialog: (profile?: Profile) => void;
  closeDialog: () => void;
}

const TOAST_MS = 6000;
// Тост с кнопкой «Reveal in Finder» — нажимать её нужно успеть.
const REVEAL_TOAST_MS = 20000;

let toastTimer: ReturnType<typeof setTimeout> | undefined;
// Resolver of the open confirmDialog promise — lives outside the store so the
// state stays serializable.
let confirmResolve: ((ok: boolean) => void) | null = null;
// Element focused when the dialog was requested — restored on close so
// Esc/Enter don't strand the user outside the editor they came from.
let confirmReturnFocus: HTMLElement | null = null;

export function createUiSlice(set: Set, get: Get, _ctx: StoreContext): UiSlice {
  return {
    dialog: { open: false },
    toast: null,
    confirm: null,
    palette: null,
    saveDialogFor: null,
    settings: {},
    settingsOpen: false,
    logViewerOpen: false,
    sidebarOpen: true,
    launcherOpen: false,

    setPalette: (palette) => set({ palette }),

    setSettingsOpen: (settingsOpen) => set({ settingsOpen }),

    setLogViewerOpen: (logViewerOpen) => set({ logViewerOpen }),

    toggleSidebar: () => set((s) => ({ sidebarOpen: !s.sidebarOpen })),

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

    setHotkey: async (id, combo) => {
      const hotkeys = { ...get().settings.hotkeys };
      if (combo) hotkeys[id] = combo;
      else delete hotkeys[id];
      const settings = { ...get().settings, hotkeys };
      set({ settings });
      try {
        await api.saveSettings(settings);
      } catch (e) {
        // биндинг действует в этой сессии; не сохранилось только на диск
        get().showToast(`Settings not saved: ${errText(e)}`);
      }
    },

    setSaveDialogFor: (tabId) => set({ saveDialogFor: tabId }),

    showToast: (message, kind = "error", revealPath) => {
      set({ toast: { message, kind, revealPath } });
      if (toastTimer) clearTimeout(toastTimer);
      toastTimer = setTimeout(
        () => set({ toast: null }),
        revealPath ? REVEAL_TOAST_MS : TOAST_MS,
      );
    },

    confirmDialog: (req) => {
      confirmResolve?.(false); // a newer request cancels the one still open
      // superseding an open dialog keeps the original focus target — the
      // current activeElement would be the dying dialog's button
      if (!get().confirm) {
        confirmReturnFocus =
          document.activeElement instanceof HTMLElement
            ? document.activeElement
            : null;
      }
      return new Promise<boolean>((resolve) => {
        confirmResolve = resolve;
        set({ confirm: req });
      });
    },

    resolveConfirm: (ok) => {
      set({ confirm: null });
      const resolve = confirmResolve;
      confirmResolve = null;
      const target = confirmReturnFocus;
      confirmReturnFocus = null;
      if (target?.isConnected) target.focus();
      resolve?.(ok);
    },

    openDialog: (profile) => set({ dialog: { open: true, profile } }),
    closeDialog: () => set({ dialog: { open: false } }),
  };
}
