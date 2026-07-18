// Store assembly: one zustand store composed from domain slices (see
// slices/*), sharing cross-slice closures via createStoreContext. This file
// also owns the module-level side effects (workspace auto-persist, HMR guard)
// and re-exports the store's public surface, so consumers keep importing
// from "lib/store".
import { create } from "zustand";
import { persistWorkspace } from "../persist";
import { createStoreContext } from "./context";
import { createActivitySlice } from "./slices/activity";
import { createAgentSlice } from "./slices/agent";
import { createConnectionsSlice } from "./slices/connections";
import { createQuerySlice } from "./slices/query";
import { createSavedSlice } from "./slices/saved";
import { createStructureSlice } from "./slices/structure";
import { createTableSlice } from "./slices/table";
import { createTabsSlice } from "./slices/tabs";
import { createUiSlice } from "./slices/ui";
import { createVaultSlice } from "./slices/vault";
import type { AppStore } from "./types";

export const useApp = create<AppStore>((set, get) => {
  const ctx = createStoreContext(set, get);
  return {
    ...createUiSlice(set, get, ctx),
    ...createVaultSlice(set, get, ctx),
    ...createConnectionsSlice(set, get, ctx),
    ...createTabsSlice(set, get, ctx),
    ...createQuerySlice(set, get, ctx),
    ...createTableSlice(set, get, ctx),
    ...createStructureSlice(set, get, ctx),
    ...createActivitySlice(set, get, ctx),
    ...createSavedSlice(set, get, ctx),
    ...createAgentSlice(set, get, ctx),
  };
});

export { columnsKey } from "./helpers";
export {
  isQueryTabDirty,
  type ActivityRow,
  type ActivityTabState,
  type AppStore,
  type ColumnPatch,
  type FunctionInfo,
  type InsertRow,
  type NewColumn,
  type PaletteKind,
  type QueryTabState,
  type StructureSection,
  type StructureTabState,
  type Tab,
  type TableTabState,
} from "./types";

// Debounced auto-persist of open tabs for every connected profile.
// Skipping disconnected profiles keeps their saved workspace intact
// after `disconnect` removes the tabs from the store.
let persistTimer: ReturnType<typeof setTimeout> | undefined;

function flushPersist() {
  if (persistTimer) {
    clearTimeout(persistTimer);
    persistTimer = undefined;
  }
  const s = useApp.getState();
  for (const profileId of Object.keys(s.sessions)) {
    persistWorkspace(s, profileId);
  }
}

useApp.subscribe(() => {
  if (persistTimer) clearTimeout(persistTimer);
  persistTimer = setTimeout(flushPersist, 400);
});

window.addEventListener("beforeunload", flushPersist);

// Dev-хук для браузерной верификации (скилл verify): доступ к стору из
// консоли/автоматизации, чтобы инжектить состояние без Tauri-бэкенда.
if (import.meta.env.DEV) {
  (window as unknown as { __useApp?: typeof useApp }).__useApp = useApp;
}

// HMR would re-create the store with empty state (init() doesn't re-run) —
// reload instead; live sessions are re-adopted via list_sessions on boot.
if (import.meta.hot) {
  import.meta.hot.accept(() => window.location.reload());
}
