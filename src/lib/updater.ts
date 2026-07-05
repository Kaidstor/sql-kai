import { relaunch } from "@tauri-apps/plugin-process";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { create } from "zustand";

type UpdaterStore = {
  checking: boolean;
  /** Available update, null when up to date (or not checked yet). */
  update: Update | null;
  downloading: boolean;
  /** Download progress 0–100, null while total size is unknown. */
  progress: number | null;
  error: string | null;
  checkForUpdates: () => Promise<void>;
  /** Download the pending update, install it and relaunch the app. */
  install: () => Promise<void>;
};

let lastCheckTime = 0;
const CHECK_INTERVAL = 60 * 60 * 1000; // re-check on window focus at most hourly

export const useUpdater = create<UpdaterStore>((set, get) => ({
  checking: false,
  update: null,
  downloading: false,
  progress: null,
  error: null,

  checkForUpdates: async () => {
    if (get().checking || get().downloading) return;
    lastCheckTime = Date.now();
    set({ checking: true, error: null });
    try {
      const update = await check();
      set({ checking: false, update });
    } catch (e) {
      // Expected in dev / before the first release is published — keep quiet.
      set({ checking: false, update: null, error: String(e) });
    }
  },

  install: async () => {
    const { update, downloading } = get();
    if (!update || downloading) return;
    set({ downloading: true, progress: null, error: null });
    try {
      let total: number | undefined;
      let received = 0;
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            total = event.data.contentLength;
            break;
          case "Progress":
            received += event.data.chunkLength;
            if (total) {
              set({ progress: Math.min(100, Math.round((received / total) * 100)) });
            }
            break;
          case "Finished":
            set({ progress: 100 });
            break;
        }
      });
      await relaunch();
    } catch (e) {
      set({ downloading: false, progress: null, error: String(e) });
    }
  },
}));

/** Check now and re-check on window focus (hourly). Returns a cleanup fn. */
export function initUpdater(): () => void {
  const onFocus = () => {
    if (Date.now() - lastCheckTime > CHECK_INTERVAL) {
      void useUpdater.getState().checkForUpdates();
    }
  };
  onFocus();
  window.addEventListener("focus", onFocus);
  return () => window.removeEventListener("focus", onFocus);
}
