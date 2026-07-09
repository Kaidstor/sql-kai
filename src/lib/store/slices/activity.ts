// Activity tab: pg_stat_activity polling and its display options.
import { api, isSessionLost } from "../../api";
import type { Get, Set, StoreContext } from "../context";
import type { ActivityRow, ActivityTabState } from "../types";

export interface ActivitySlice {
  refreshActivity: (tabId: string) => Promise<void>;
  setActivityOptions: (
    tabId: string,
    patch: Partial<Pick<ActivityTabState, "refreshSec" | "showIdle">>,
  ) => void;
}

export function createActivitySlice(
  _set: Set,
  get: Get,
  ctx: StoreContext,
): ActivitySlice {
  const { tabOf, patchTab } = ctx;

  return {
    refreshActivity: async (tabId) => {
      const tab = tabOf(tabId, "activity");
      if (!tab || tab.state.loading) return;
      const session = get().sessions[tab.profileId];
      if (!session) return;
      const sql = `SELECT a.pid,
       a.datname,
       a.state,
       a.usename,
       a.application_name,
       a.client_addr::text,
       coalesce(a.wait_event_type || ': ' || a.wait_event, ''),
       array_to_string(pg_blocking_pids(a.pid), ', '),
       round(extract(epoch FROM (now() - a.query_start)))::bigint,
       round(extract(epoch FROM (now() - a.xact_start)))::bigint,
       a.query
  FROM pg_stat_activity a
 WHERE a.pid <> pg_backend_pid()
   AND a.backend_type = 'client backend'
   ${tab.state.showIdle ? "" : "AND coalesce(a.state, '') <> 'idle'"}
 ORDER BY (a.state = 'active') DESC, a.query_start ASC NULLS LAST`;
      patchTab<ActivityTabState>(tabId, { loading: true });
      try {
        const exec = await api.executeSql(session.sessionId, sql, 500);
        const rows: ActivityRow[] = (exec.results[0]?.rows ?? []).map((r) => ({
          pid: r[0] ?? "",
          db: r[1] ?? "",
          state: r[2] ?? "",
          user: r[3] ?? "",
          app: r[4] ?? "",
          client: r[5] ?? "",
          wait: r[6] ?? "",
          blockedBy: r[7] ?? "",
          querySec: r[8] !== null ? Number(r[8]) : null,
          xactSec: r[9] !== null ? Number(r[9]) : null,
          query: r[10] ?? "",
        }));
        patchTab<ActivityTabState>(tabId, {
          rows,
          loading: false,
          error: undefined,
          connectionLost: undefined,
        });
      } catch (e) {
        patchTab<ActivityTabState>(tabId, {
          loading: false,
          error: ctx.handleSqlError(tab.profileId, e),
          connectionLost: isSessionLost(e),
        });
      }
    },

    setActivityOptions: (tabId, patch) => {
      const tab = tabOf(tabId, "activity");
      if (!tab) return;
      patchTab<ActivityTabState>(tabId, patch);
      // toggling idle changes the SQL — refetch right away
      if (patch.showIdle !== undefined) void get().refreshActivity(tabId);
    },
  };
}
