import { useEffect } from "react";

/** First fetch for a tab, deferred until it is actually mounted.
 *
 *  Why not load everything at boot: restoring a workspace with a dozen tabs
 *  fired a dozen parallel page queries and froze the app. Tabs render only
 *  while active, so "on mount + on (re)connect" is the right trigger.
 *
 *  Why the effect intentionally depends on `connected` alone: every tab
 *  component is keyed by tab id, so one mounted instance serves exactly one
 *  tab — `load` and the emptiness check close over that tab's state and are
 *  re-read on every render anyway. Adding them to the dep list would re-run
 *  the effect on each state change and refetch mid-edit.
 *
 *  @param connected  the profile has a live session
 *  @param loaded     data (or an error) is already there — nothing to fetch
 *  @param load       the fetch to run; called at most once per connect
 */
export function useLazyTabLoad(
  connected: boolean,
  loaded: boolean,
  load: () => void,
) {
  useEffect(() => {
    if (connected && !loaded) load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected]);
}
