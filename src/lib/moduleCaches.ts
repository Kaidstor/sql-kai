// Registry for module-level caches that deliberately live outside the store.
//
// A few UI details (editor selection offsets, panel width) are kept in
// module-scope maps rather than store state: they change on every keystroke /
// drag and would churn the store — and its auto-persist — for nothing. The
// trade-off is that they are global state the store can't see, so `lockVault`
// (which promises to wipe every trace of the unlocked session) would leave
// them behind. Each such cache registers its reset here instead.
//
// Caches keyed by a result object via WeakMap (grid scroll offsets, grid
// selection snapshots) need no entry: locking drops the tabs holding those
// results, so the entries become collectable on their own.

const resets = new Set<() => void>();

/** Registers a module-level cache to be cleared when the vault locks. */
export function resetOnVaultLock(reset: () => void) {
  resets.add(reset);
}

/** Clears every registered cache — called by `lockVault`. */
export function clearModuleCaches() {
  for (const reset of resets) reset();
}
