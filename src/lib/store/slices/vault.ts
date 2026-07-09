// Vault gate and startup: init, master-password setup/unlock, Touch ID,
// lock. Owns the post-unlock workspace load (profiles, queries, re-adopted
// sessions) because nothing loads until the vault opens.
import { api, errText } from "../../api";
import {
  dropLegacyHistory,
  loadLegacyHistory,
} from "../../persist";
import { applyTheme } from "../../themes";
import type { SessionInfo, VaultStatus } from "../../types";
import type { Get, Set, StoreContext } from "../context";

export interface VaultSlice {
  /** Vault gate: null until checked, then whether it exists / is unlocked. */
  vault: VaultStatus | null;
  /** init() failed before the vault state was known — show a retry screen. */
  vaultError: string | null;

  init: () => Promise<void>;
  /** First run: set the master password, create + unlock the vault. */
  setupVault: (password: string, enableBiometric: boolean) => Promise<void>;
  /** Unlock an existing vault with the master password. */
  unlockVault: (password: string, enableBiometric: boolean) => Promise<void>;
  /** Unlock via Touch ID; rejects with "cancelled" when dismissed. */
  unlockVaultBiometric: () => Promise<void>;
  /** Lock the vault: drops in-memory secrets and all live sessions. */
  lockVault: () => Promise<void>;
}

export function createVaultSlice(set: Set, get: Get, ctx: StoreContext): VaultSlice {
  /** Loads profiles/queries and re-adopts live sessions. Runs once the vault
   *  is unlocked (from init, setupVault or unlockVault). */
  const loadWorkspace = async () => {
    const [profiles, queries, sessionList] = await Promise.all([
      api.listProfiles(),
      api.listQueries(),
      api.listSessions(),
    ]);
    // Isolated sessions belong to specific tabs, not the profile map, and we
    // don't re-adopt them across a reload — disconnect the orphans and let
    // isolated tabs reopen lazily.
    const primaries = sessionList.filter((s) => !s.isolated);
    for (const s of sessionList) {
      if (s.isolated) api.disconnectSession(s.sessionId).catch(() => {});
    }
    const sessions: Record<string, SessionInfo> = {};
    for (const s of primaries) sessions[s.profileId] = s;
    set((st) => ({
      profiles,
      queries,
      sessions,
      activeProfileId: st.activeProfileId ?? primaries[0]?.profileId ?? null,
    }));
    void get().refreshCliSessions();
    // Re-adopt sessions that survived a webview reload: tables + saved tabs.
    await Promise.all(
      primaries.map(async (s) => {
        await get().refreshTables(s.profileId);
        // guard: StrictMode double-runs init() in dev — restoring twice
        // would duplicate every tab (and persist the doubled set)
        if (!get().tabs.some((t) => t.profileId === s.profileId)) {
          ctx.restoreProfileTabs(s.profileId);
        }
      }),
    );
  };

  /** Post-unlock tail shared by setup/unlock/Touch ID: optional biometric
   *  enrollment, refreshed vault status, workspace load. */
  const finishUnlock = async (enableBiometric: boolean) => {
    if (enableBiometric) {
      try {
        await api.vaultEnableBiometric();
      } catch (e) {
        // Non-fatal: the vault works, only the fast path is missing.
        get().showToast(`Touch ID not enabled: ${errText(e)}`);
      }
    }
    set({ vault: await api.vaultStatus() });
    await loadWorkspace();
  };

  /** Loads history.json, importing what an older build left in localStorage.
   *  History is a convenience — a failure here never blocks startup. */
  const loadHistoryFromDisk = async () => {
    try {
      const legacy = loadLegacyHistory();
      const history = legacy.length
        ? await api.importHistory(legacy)
        : await api.listHistory();
      if (legacy.length) dropLegacyHistory();
      set({ history });
    } catch {
      // stays empty; the next recordHistory repopulates the in-memory list
    }
  };

  return {
    vault: null,
    vaultError: null,

    init: async () => {
      // Settings are not vault-gated — the theme must apply on the unlock
      // screen too. main.tsx already applied the localStorage-cached theme;
      // settings.json is the source of truth and wins once it's read.
      void api
        .getSettings()
        .then((settings) => {
          set({ settings });
          applyTheme(settings.theme);
        })
        .catch(() => {
          // cached theme stays; the next setTheme rewrites the file
        });
      void loadHistoryFromDisk();
      try {
        const vault = await api.vaultStatus();
        set({ vault, vaultError: null });
        // Nothing loads until the vault is unlocked — VaultGate shows setup/unlock.
        if (vault.unlocked) await loadWorkspace();
      } catch (e) {
        // Without the vault state the gate can't render anything meaningful —
        // surface the failure with a retry instead of a blank screen.
        set({ vaultError: errText(e) });
      }
    },

    setupVault: async (password, enableBiometric) => {
      await api.vaultSetup(password);
      await finishUnlock(enableBiometric);
    },

    unlockVault: async (password, enableBiometric) => {
      await api.vaultUnlock(password);
      await finishUnlock(enableBiometric);
    },

    unlockVaultBiometric: async () => {
      await api.vaultUnlockBiometric();
      await finishUnlock(false);
    },

    lockVault: async () => {
      try {
        await api.vaultLock();
      } finally {
        // Wipe every trace of the unlocked session from the UI.
        set((st) => ({
          vault: st.vault ? { ...st.vault, unlocked: false } : st.vault,
          profiles: [],
          sessions: {},
          isolatedSessions: {},
          cliSessions: {},
          tables: {},
          schemaColumns: {},
          schemaFunctions: {},
          tabs: [],
          activeTabId: null,
          activeProfileId: null,
        }));
      }
    },
  };
}
