import { CircleStop, Loader2, OctagonX } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { api, errText } from "../lib/api";
import { copyText } from "../lib/clipboard";
import { fmtDuration } from "../lib/format";
import {
  useApp,
  type ActivityInfo,
  type ActivityTabState,
  type Tab,
} from "../lib/store";
import { CellDialog, type CellDialogState } from "./grid/CellDialog";
import { TabError } from "./TabError";
import { IconButton, RefreshButton, Select, cn } from "./ui";

function stateColor(state: string): string {
  if (state === "active") return "text-emerald-400";
  if (state.startsWith("idle in transaction")) return "text-amber-400";
  if (state === "idle") return "text-zinc-500";
  return "text-zinc-400";
}

/** Колонки грида: подпись заголовка и текст ячейки — он же уходит в ⌘C
 *  и в read-only просмотр по ⌘⏎ (декорации вроде ⛔/— не копируются). */
const COLS: { label: string; value: (r: ActivityInfo) => string }[] = [
  { label: "pid", value: (r) => r.pid },
  { label: "state", value: (r) => r.state },
  { label: "duration", value: (r) => fmtDuration(r.querySec) },
  { label: "user", value: (r) => r.user },
  { label: "db", value: (r) => r.db },
  { label: "app", value: (r) => r.app },
  { label: "client", value: (r) => r.client },
  { label: "wait / blocked by", value: (r) => r.blockedBy || r.wait },
  { label: "query", value: (r) => r.query },
];

export function ActivityTab({ tab }: { tab: Tab }) {
  const state = tab.state as ActivityTabState;
  const sessions = useApp((s) => s.sessions);
  const refreshActivity = useApp((s) => s.refreshActivity);
  const setActivityOptions = useApp((s) => s.setActivityOptions);
  const showToast = useApp((s) => s.showToast);
  const confirmDialog = useApp((s) => s.confirmDialog);
  const session = sessions[tab.profileId];
  const connected = Boolean(session);

  // Фокус ячейки привязан к pid, а не к индексу строки: авто-refresh каждые
  // 2–15 сек переставляет строки, и индекс уезжал бы на соседний backend.
  const [focus, setFocus] = useState<{ pid: string; col: number } | null>(null);
  const [dialog, setDialog] = useState<CellDialogState | null>(null);
  const gridRef = useRef<HTMLDivElement>(null);

  // First load + auto-refresh. The tab only renders while active, so the
  // interval dies with the unmount when the user switches away.
  useEffect(() => {
    if (!connected) return;
    void refreshActivity(tab.id);
    if (state.refreshSec <= 0) return;
    const timer = setInterval(
      () => void refreshActivity(tab.id),
      state.refreshSec * 1000,
    );
    return () => clearInterval(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [connected, state.refreshSec, state.includeIdle]);

  /** pg_cancel_backend (soft) / pg_terminate_backend (kills the connection). */
  const signal = async (row: ActivityInfo, terminate: boolean) => {
    if (!session) return;
    if (
      terminate &&
      !(await confirmDialog({
        title: `Terminate backend ${row.pid}?`,
        message: "Its connection will be closed and the transaction rolled back.",
        confirmLabel: "Terminate",
        danger: true,
      }))
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
    void refreshActivity(tab.id);
  };

  const rows = state.rows ?? [];
  const blocked = rows.filter((r) => r.blockedBy).length;

  const copyAndToast = (text: string) =>
    void copyText(text).then((ok) => ok && showToast("Cell copied", "info"));

  const openDialog = (ri: number, ci: number) => {
    const r = rows[ri];
    if (r) setDialog({ row: ri, col: ci, text: COLS[ci].value(r), isJson: false });
  };

  const handleKeyDown = (e: React.KeyboardEvent) => {
    if (!focus) return;
    const ri = rows.findIndex((r) => r.pid === focus.pid);
    if (ri < 0) return;
    if ((e.metaKey || e.ctrlKey) && e.key === "c") {
      e.preventDefault();
      copyAndToast(COLS[focus.col].value(rows[ri]));
    } else if ((e.metaKey || e.ctrlKey) && e.key === "Enter") {
      e.preventDefault();
      openDialog(ri, focus.col);
    } else if (e.key === "Escape") {
      setFocus(null);
    } else if (
      e.key === "ArrowUp" ||
      e.key === "ArrowDown" ||
      e.key === "ArrowLeft" ||
      e.key === "ArrowRight"
    ) {
      e.preventDefault();
      const nr =
        e.key === "ArrowDown"
          ? Math.min(ri + 1, rows.length - 1)
          : e.key === "ArrowUp"
            ? Math.max(ri - 1, 0)
            : ri;
      const nc =
        e.key === "ArrowRight"
          ? Math.min(focus.col + 1, COLS.length - 1)
          : e.key === "ArrowLeft"
            ? Math.max(focus.col - 1, 0)
            : focus.col;
      setFocus({ pid: rows[nr].pid, col: nc });
    }
  };

  /** Обработчики выбора/просмотра для ячейки данных (ci — индекс в COLS). */
  const cellProps = (r: ActivityInfo, ri: number, ci: number) => ({
    onMouseDown: (e: React.MouseEvent) => {
      if (e.button !== 0) return;
      // WKWebView тянет нативное текстовое выделение по drag — гасим его,
      // фокус переводим на контейнер, чтобы работали ⌘C/⌘⏎/стрелки
      e.preventDefault();
      gridRef.current?.focus();
      setFocus({ pid: r.pid, col: ci });
    },
    onDoubleClick: () => openDialog(ri, ci),
  });

  const focusCls = (r: ActivityInfo, ci: number) =>
    focus?.pid === r.pid &&
    focus.col === ci &&
    "bg-sky-500/20 shadow-[inset_0_0_0_1px_var(--color-sky-500)]";

  return (
    <div className="flex h-full min-h-0 flex-col">
      <div className="flex min-h-10 shrink-0 items-center gap-2 border-b border-zinc-800 px-2 py-1.5 text-[12px]">
        <span className="font-medium text-zinc-300">pg_stat_activity</span>
        <RefreshButton
          loading={state.loading}
          onClick={() => void refreshActivity(tab.id)}
        />
        <label className="flex cursor-pointer items-center gap-1.5 pl-1 text-[11px] text-zinc-400">
          <input
            type="checkbox"
            checked={state.includeIdle}
            onChange={(e) =>
              setActivityOptions(tab.id, { includeIdle: e.target.checked })
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
        <div
          ref={gridRef}
          tabIndex={0}
          onKeyDown={handleKeyDown}
          className="min-h-0 flex-1 overflow-auto outline-none"
        >
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
              {rows.map((r, ri) => (
                <tr
                  key={r.pid}
                  className={cn(
                    "group",
                    r.blockedBy ? "bg-red-950/30" : "hover:bg-zinc-800/40",
                  )}
                >
                  <td
                    {...cellProps(r, ri, 0)}
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-300",
                      focusCls(r, 0),
                    )}
                  >
                    {r.pid}
                  </td>
                  <td
                    {...cellProps(r, ri, 1)}
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 whitespace-nowrap",
                      stateColor(r.state),
                      focusCls(r, 1),
                    )}
                  >
                    {r.state || "—"}
                  </td>
                  <td
                    {...cellProps(r, ri, 2)}
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
                      focusCls(r, 2),
                    )}
                  >
                    {fmtDuration(r.querySec)}
                  </td>
                  <td
                    {...cellProps(r, ri, 3)}
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-400",
                      focusCls(r, 3),
                    )}
                  >
                    {r.user}
                  </td>
                  <td
                    {...cellProps(r, ri, 4)}
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-400",
                      focusCls(r, 4),
                    )}
                  >
                    {r.db}
                  </td>
                  <td
                    {...cellProps(r, ri, 5)}
                    title={r.app}
                    className={cn(
                      "max-w-40 truncate border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-500",
                      focusCls(r, 5),
                    )}
                  >
                    {r.app}
                  </td>
                  <td
                    {...cellProps(r, ri, 6)}
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-500",
                      focusCls(r, 6),
                    )}
                  >
                    {r.client}
                  </td>
                  <td
                    {...cellProps(r, ri, 7)}
                    className={cn(
                      "border-b border-r border-zinc-800/70 px-2 py-0.5 whitespace-nowrap",
                      focusCls(r, 7),
                    )}
                  >
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
                    {...cellProps(r, ri, 8)}
                    title={r.query}
                    className={cn(
                      "max-w-140 truncate border-b border-r border-zinc-800/70 px-2 py-0.5 text-zinc-300",
                      focusCls(r, 8),
                    )}
                  >
                    {r.query}
                  </td>
                  <td className="border-b border-zinc-800/70 px-1 py-0.5 whitespace-nowrap">
                    <span className="invisible flex items-center group-hover:visible">
                      <IconButton
                        title="Cancel query (pg_cancel_backend)"
                        onClick={() => void signal(r, false)}
                      >
                        <CircleStop size={13} className="text-amber-400" />
                      </IconButton>
                      <IconButton
                        title="Terminate backend (pg_terminate_backend)"
                        onClick={() => void signal(r, true)}
                      >
                        <OctagonX size={13} className="text-red-400" />
                      </IconButton>
                    </span>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
          {rows.length === 0 && (
            <div className="px-3 py-3 text-[12px] text-zinc-600">
              no {state.includeIdle ? "" : "non-idle "}backends
            </div>
          )}
        </div>
      )}
      {dialog && (
        <CellDialog
          dialog={dialog}
          columnName={COLS[dialog.col].label}
          canEdit={false}
          onText={() => {}}
          onStage={() => {}}
          onClose={() => {
            setDialog(null);
            gridRef.current?.focus();
          }}
          onCopy={copyAndToast}
        />
      )}
    </div>
  );
}
