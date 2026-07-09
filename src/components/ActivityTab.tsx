import { CircleStop, Loader2, OctagonX } from "lucide-react";
import { useEffect } from "react";
import { api, errText } from "../lib/api";
import { fmtDuration } from "../lib/format";
import {
  useApp,
  type ActivityRow,
  type ActivityTabState,
  type Tab,
} from "../lib/store";
import { TabError } from "./TabError";
import { IconBtn, RefreshBtn, Select, cn } from "./ui";

function stateColor(state: string): string {
  if (state === "active") return "text-emerald-400";
  if (state.startsWith("idle in transaction")) return "text-amber-400";
  if (state === "idle") return "text-zinc-500";
  return "text-zinc-400";
}

export function ActivityTab({ tab }: { tab: Tab }) {
  const state = tab.state as ActivityTabState;
  const sessions = useApp((s) => s.sessions);
  const loadActivity = useApp((s) => s.loadActivity);
  const setActivityOptions = useApp((s) => s.setActivityOptions);
  const showToast = useApp((s) => s.showToast);
  const session = sessions[tab.profileId];
  const connected = Boolean(session);

  // First load + auto-refresh. The tab only renders while active, so the
  // interval dies with the unmount when the user switches away.
  useEffect(() => {
    if (!connected) return;
    void loadActivity(tab.id);
    if (state.refreshSec <= 0) return;
    const timer = setInterval(
      () => void loadActivity(tab.id),
      state.refreshSec * 1000,
    );
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected, state.refreshSec, state.showIdle]);

  /** pg_cancel_backend (soft) / pg_terminate_backend (kills the connection). */
  const signal = async (row: ActivityRow, terminate: boolean) => {
    if (!session) return;
    if (
      terminate &&
      !confirm(
        `Terminate backend ${row.pid}? Its connection will be closed and the transaction rolled back.`,
      )
    ) {
      return;
    }
    const fn = terminate ? "pg_terminate_backend" : "pg_cancel_backend";
    try {
      await api.executeSql(
        session.sessionId,
        `SELECT ${fn}(${Number(row.pid)})`,
        10,
      );
      showToast(
        terminate ? `Terminated backend ${row.pid}` : `Cancel sent to ${row.pid}`,
        "info",
      );
    } catch (e) {
      showToast(errText(e));
    }
    void loadActivity(tab.id);
  };

  const rows = state.rows ?? [];
  const blocked = rows.filter((r) => r.blockedBy).length;

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-10 shrink-0 items-center gap-2 border-b border-zinc-800 px-2 py-1.5 text-[12px]">
        <span className="font-medium text-zinc-300">pg_stat_activity</span>
        <RefreshBtn
          loading={state.loading}
          onClick={() => void loadActivity(tab.id)}
        />
        <label className="flex cursor-pointer items-center gap-1.5 pl-1 text-[11px] text-zinc-400">
          <input
            type="checkbox"
            checked={state.showIdle}
            onChange={(e) =>
              setActivityOptions(tab.id, { showIdle: e.target.checked })
            }
            className="accent-sky-600"
          />
          show idle
        </label>
        <div className="ml-auto flex items-center gap-2">
          <span className="text-[11px] text-zinc-500">
            {rows.length} backend(s)
            {blocked > 0 && (
              <span className="text-red-400"> · {blocked} blocked</span>
            )}
          </span>
          <Select
            value={state.refreshSec}
            onChange={(e) =>
              setActivityOptions(tab.id, { refreshSec: Number(e.target.value) })
            }
            title="Auto-refresh"
          >
            <option value={0}>manual</option>
            <option value={2}>every 2s</option>
            <option value={5}>every 5s</option>
            <option value={15}>every 15s</option>
          </Select>
        </div>
      </div>

      {state.error ? (
        <TabError profileId={tab.profileId} error={state.error} lost={state.connectionLost} />
      ) : !state.rows && state.loading ? (
        <div className="flex flex-1 items-center justify-center gap-2 text-[12px] text-zinc-600">
          <Loader2 size={13} className="animate-spin" /> loading…
        </div>
      ) : (
        <div className="min-h-0 flex-1 overflow-auto">
          <table className="w-full border-separate border-spacing-0 font-mono text-[12px]">
            <thead className="sticky top-0 z-10">
              <tr>
                {["pid", "state", "duration", "user", "db", "app", "client", "wait / blocked by", "query", ""].map(
                  (h) => (
                    <th
                      key={h}
                      className="border-b border-r border-zinc-800 bg-zinc-900 px-2 py-1 text-left font-medium whitespace-nowrap text-zinc-400"
                    >
                      {h}
                    </th>
                  ),
                )}
              </tr>
            </thead>
            <tbody>
              {rows.map((r) => (
                <tr
                  key={r.pid}
                  className={cn(
                    "group",
                    r.blockedBy ? "bg-red-950/30" : "hover:bg-zinc-800/40",
                  )}
                >
                  <td className="border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-300">
                    {r.pid}
                  </td>
                  <td
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 whitespace-nowrap",
                      stateColor(r.state),
                    )}
                  >
                    {r.state || "—"}
                  </td>
                  <td
                    title={
                      r.xactSec !== null
                        ? `query ${fmtDuration(r.querySec)} · tx ${fmtDuration(r.xactSec)}`
                        : undefined
                    }
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 text-right tabular-nums whitespace-nowrap",
                      (r.querySec ?? 0) >= 60 && r.state === "active"
                        ? "text-amber-400"
                        : "text-zinc-400",
                    )}
                  >
                    {fmtDuration(r.querySec)}
                  </td>
                  <td className="border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-400">
                    {r.user}
                  </td>
                  <td className="border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-400">
                    {r.db}
                  </td>
                  <td
                    title={r.app}
                    className="max-w-40 truncate border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-500"
                  >
                    {r.app}
                  </td>
                  <td className="border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-500">
                    {r.client}
                  </td>
                  <td className="border-b border-r border-zinc-800/70 px-2 py-0.5 whitespace-nowrap">
                    {r.blockedBy ? (
                      <span
                        className="font-semibold text-red-400"
                        title={`Waiting on lock(s) held by PID(s) ${r.blockedBy}`}
                      >
                        ⛔ {r.blockedBy}
                      </span>
                    ) : (
                      <span className="text-zinc-500">{r.wait}</span>
                    )}
                  </td>
                  <td
                    title={r.query}
                    className="max-w-140 truncate border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-300"
                  >
                    {r.query}
                  </td>
                  <td className="border-b border-zinc-800/70 px-1 py-0.5 whitespace-nowrap">
                    <span className="invisible flex items-center group-hover:visible">
                      <IconBtn
                        title="Cancel query (pg_cancel_backend)"
                        onClick={() => void signal(r, false)}
                      >
                        <CircleStop size={13} className="text-amber-400" />
                      </IconBtn>
                      <IconBtn
                        title="Terminate backend (pg_terminate_backend)"
                        onClick={() => void signal(r, true)}
                      >
                        <OctagonX size={13} className="text-red-400" />
                      </IconBtn>
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {rows.length === 0 && (
            <div className="px-3 py-3 text-[12px] text-zinc-600">
              no {state.showIdle ? "" : "non-idle "}backends
            </div>
          )}
        </div>
      )}
    </div>
  );
}
