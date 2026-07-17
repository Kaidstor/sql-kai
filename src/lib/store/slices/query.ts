// Query-tab execution: run/explain/cancel, isolated connections (own pid &
// transaction), manual commit mode and the COMMIT/ROLLBACK verbs. Owns all
// isolated-session plumbing — other slices only clear the shared maps.
import { api, errText, isSessionLost } from "../../api";
import { exportedMessage } from "../../export";
import { notifyQueryDone } from "../../notify";
import { countStatements } from "../../sql";
import type {
  ExplainResult,
  ExportFormat,
  HistoryEntry,
  SessionInfo,
} from "../../types";
import type { Get, Set, StoreContext } from "../context";
import { without } from "../helpers";
import type { QueryTabState, Tab } from "../types";

export interface QuerySlice {
  /** Session ids with a file export in flight. A Run on the same connection
   *  queues behind it (one pipelined connection per session) — surfaced with
   *  a toast instead of looking hung. */
  exporting: Record<string, true>;
  /** Full-result file export (Export menu): re-runs `sql` on `sessionId` and
   *  streams statement `statementIndex` to `path`. Session-lost errors route
   *  like runQuery's (an isolated tab drops its session, the shared one flips
   *  the profile to "connection lost"), and the tx badge refreshes after. */
  exportSqlToFile: (req: {
    profileId: string;
    sessionId: string;
    sql: string;
    statementIndex: number;
    /** Manual-commit tab: wrap the re-run in BEGIN exactly like Run would. */
    autoBegin: boolean;
    format: ExportFormat;
    path: string;
    /** Query tab owning `sessionId` when it is the tab's isolated session. */
    isolatedTabId?: string;
  }) => Promise<void>;
  /** Runs the tab's SQL, or `sqlOverride` (an editor selection) when given.
   *  The override is executed and recorded to history, but not persisted as
   *  the tab's SQL. */
  runQuery: (tabId: string, sqlOverride?: string) => Promise<void>;
  /** EXPLAIN (FORMAT JSON) the tab's statement (or the editor selection);
   *  analyze also executes it. */
  runExplain: (
    tabId: string,
    analyze: boolean,
    sqlOverride?: string,
  ) => Promise<void>;
  /** Back from the plan view to the last results. */
  clearExplain: (tabId: string) => void;
  cancelQuery: (tabId: string) => Promise<void>;
  /** Gives the query tab its own dedicated connection (own pid & transaction),
   *  opening it now. No-op if already isolated. */
  isolateTab: (tabId: string) => Promise<void>;
  /** Drops the tab's isolated connection (rolling back any open tx) and moves
   *  it back onto the profile's shared session; forces commit mode to auto. */
  unisolateTab: (tabId: string) => Promise<void>;
  /** Auto/Manual commit for an (isolated) query tab; "manual" isolates first. */
  setCommitMode: (tabId: string, mode: "auto" | "manual") => Promise<void>;
  /** COMMIT / ROLLBACK the open transaction on the tab's connection. */
  commitTx: (tabId: string) => Promise<void>;
  rollbackTx: (tabId: string) => Promise<void>;
}

export function createQuerySlice(set: Set, get: Get, ctx: StoreContext): QuerySlice {
  const { tabOf, patchTab } = ctx;

  /** The connection a query tab runs on: its own isolated session when
   *  isolated & open, otherwise the profile's shared session. */
  const effectiveSession = (tab: Tab): SessionInfo | null => {
    if (tab.state.kind === "query" && tab.state.sessionId) {
      return get().isolatedSessions[tab.state.sessionId] ?? null;
    }
    return get().sessions[tab.profileId] ?? null;
  };

  /** Refreshes a connection's heuristic transaction state (badge) after a run.
   *  Updates whichever map holds it (isolated by sessionId, else shared by
   *  profileId). Advisory — a failure just leaves the last value. */
  const refreshTxStatus = async (session: SessionInfo) => {
    try {
      const tx = await api.sessionTxStatus(session.sessionId);
      set((s) => {
        if (s.isolatedSessions[session.sessionId]) {
          return {
            isolatedSessions: {
              ...s.isolatedSessions,
              [session.sessionId]: { ...s.isolatedSessions[session.sessionId], tx },
            },
          };
        }
        const cur = s.sessions[session.profileId];
        if (!cur || cur.sessionId !== session.sessionId) return {};
        return { sessions: { ...s.sessions, [session.profileId]: { ...cur, tx } } };
      });
    } catch {
      // ignore — the badge just keeps its previous value
    }
  };

  /** Drops a tab's isolated backend session (best-effort disconnect) and
   *  detaches it from the tab, so the next run lazily reopens a fresh one. */
  const dropIsolatedSession = (tabId: string) => {
    const tab = tabOf(tabId, "query");
    const sid = tab?.state.sessionId;
    if (!sid) return;
    api.disconnectSession(sid).catch(() => {});
    set((s) => ({ isolatedSessions: without(s.isolatedSessions, sid) }));
    patchTab<QueryTabState>(tabId, { sessionId: undefined });
  };

  /** Ensures an isolated query tab has a live dedicated connection, opening one
   *  if missing/stale. Returns the session, or null if it couldn't be opened
   *  (e.g. the profile isn't connected). */
  const openIsolatedSession = async (
    tabId: string,
  ): Promise<SessionInfo | null> => {
    const tab = tabOf(tabId, "query");
    if (!tab) return null;
    const existing = tab.state.sessionId
      ? get().isolatedSessions[tab.state.sessionId]
      : undefined;
    if (existing) return existing;
    if (!get().sessions[tab.profileId]) {
      get().showToast("Not connected");
      return null;
    }
    try {
      const info = await api.openIsolatedSession(tab.profileId);
      set((s) => ({
        isolatedSessions: { ...s.isolatedSessions, [info.sessionId]: info },
      }));
      patchTab<QueryTabState>(tabId, { sessionId: info.sessionId });
      return info;
    } catch (e) {
      get().showToast(errText(e));
      return null;
    }
  };

  /** In-flight открытия изолированных сессий по табам: параллельные вызовы
   *  (Isolate + сразу ⌘Enter) ждут один промис, а не открывают вторую
   *  сессию — вторая утекала бы навсегда (открытый pid на сервере). */
  const isolatedOpens = new Map<string, Promise<SessionInfo | null>>();
  const ensureIsolatedSession = (tabId: string): Promise<SessionInfo | null> => {
    const inFlight = isolatedOpens.get(tabId);
    if (inFlight) return inFlight;
    const p = openIsolatedSession(tabId).finally(() => {
      isolatedOpens.delete(tabId);
    });
    isolatedOpens.set(tabId, p);
    return p;
  };

  /** Display name for a profile id (history + notification labels). */
  const profileName = (profileId: string): string =>
    get().profiles.find((p) => p.id === profileId)?.name ?? "?";

  /** Native completion ping for a run that finished while backgrounded. */
  const notifyDone = (
    profileId: string,
    startedAt: number,
    ok: boolean,
    sql: string,
  ) =>
    void notifyQueryDone({
      profileName: profileName(profileId),
      durationMs: Date.now() - startedAt,
      ok,
      sql,
    });

  /** Flips a query tab to `running` and resolves its connection — its own
   *  isolated session when isolated, else the profile's shared one. Returns
   *  null (and clears `running`) when unavailable. The `running` flag is set
   *  before the possible isolated-open await so a second Run can't race in. */
  const beginRun = async (
    tab: Tab,
    isolated: boolean,
  ): Promise<SessionInfo | null> => {
    patchTab<QueryTabState>(tab.id, {
      running: true,
      error: undefined,
      connectionLost: undefined,
    });
    const session = isolated
      ? await ensureIsolatedSession(tab.id)
      : ctx.sessionFor(tab.profileId);
    if (!session) {
      patchTab<QueryTabState>(tab.id, { running: false });
      return null;
    }
    return session;
  };

  /** Error routing shared by runQuery/runExplain: a dying isolated connection
   *  is a per-tab event (drop it, it reopens next run); the shared connection
   *  flips the whole profile to "connection lost". */
  const handleRunError = (tab: Tab, isolated: boolean, e: unknown) => {
    if (isolated) {
      if (isSessionLost(e)) dropIsolatedSession(tab.id);
    } else {
      ctx.noteSessionLost(tab.profileId, e);
    }
  };

  /** Runs a bare COMMIT/ROLLBACK on the tab's connection (Commit/Rollback
   *  buttons), then refreshes the tx badge. */
  const runTxVerb = async (tabId: string, verb: "COMMIT" | "ROLLBACK") => {
    const tab = tabOf(tabId, "query");
    if (!tab) return;
    const session = effectiveSession(tab);
    if (!session) return;
    try {
      await api.executeSql(session.sessionId, verb, 1, false);
      get().showToast(verb === "COMMIT" ? "Committed" : "Rolled back", "success");
    } catch (e) {
      // Смерть соединения на COMMIT/ROLLBACK — та же маршрутизация, что у
      // runQuery: изолированная сессия дропается, общая флипает профиль в
      // "connection lost" (иначе Reconnect не предлагается).
      handleRunError(tab, Boolean(tab.state.sessionId), e);
      get().showToast(errText(e));
    }
    void refreshTxStatus(session);
  };

  return {
    exporting: {},

    exportSqlToFile: async ({
      profileId,
      sessionId,
      sql,
      statementIndex,
      autoBegin,
      format,
      path,
      isolatedTabId,
    }) => {
      /** Live SessionInfo for the tx-badge refresh — whichever map owns it
       *  (null once a lost isolated session has been dropped). */
      const sessionOf = () =>
        get().isolatedSessions[sessionId] ??
        (get().sessions[profileId]?.sessionId === sessionId
          ? get().sessions[profileId]
          : null);
      set((s) => ({ exporting: { ...s.exporting, [sessionId]: true } }));
      try {
        const out = await api.exportSql(
          sessionId,
          sql,
          statementIndex,
          format,
          path,
          autoBegin,
        );
        get().showToast(
          out.truncated
            ? `Exported first ${out.rows.toLocaleString()} rows — XLSX sheet limit`
            : exportedMessage(out.rows, path),
          out.truncated ? "info" : "success",
        );
      } catch (e) {
        // Та же маршрутизация, что у runQuery: умершая изолированная сессия —
        // событие таба (дроп, переоткроется на следующем Run), общая — флип
        // профиля в "connection lost", иначе Reconnect не предлагается.
        if (isolatedTabId) {
          if (isSessionLost(e)) dropIsolatedSession(isolatedTabId);
        } else {
          ctx.noteSessionLost(profileId, e);
        }
        get().showToast(errText(e));
      } finally {
        set((s) => ({ exporting: without(s.exporting, sessionId) }));
        // Экспорт (или его провал) мог сдвинуть tx-эвристику — обновить бейдж,
        // как это делают runQuery/runTxVerb.
        const session = sessionOf();
        if (session) void refreshTxStatus(session);
      }
    },

    runQuery: async (tabId, sqlOverride) => {
      const tab = tabOf(tabId, "query");
      if (!tab || tab.state.running) return;
      const sql = (sqlOverride ?? tab.state.sql).trim();
      if (!sql) return;
      // No prod-confirm on run (deliberate): the dialog was reflex-Enter'd
      // anyway — the safety story is Ctrl+C cancel + the PROD chrome tints.
      const isolated = Boolean(tab.state.isolated);
      const session = await beginRun(tab, isolated);
      if (!session) return;
      if (get().exporting[session.sessionId]) {
        // одно pipelined-соединение на сессию: запрос встанет в очередь за
        // экспортом — сказать об этом, а не выглядеть зависшим
        get().showToast(
          "Export in progress on this connection — the query starts after it",
          "info",
        );
      }
      // manual commit only ever applies on an isolated connection — never let it
      // open a transaction on the shared one
      const autoBegin = isolated && tab.state.commitMode === "manual";
      const pushHistory = (ok: boolean) => {
        const entry: HistoryEntry = {
          id: crypto.randomUUID(),
          profileId: tab.profileId,
          profileName: profileName(tab.profileId),
          sql,
          at: Date.now(),
          ok,
        };
        // the disk store dedups (a rerun bumps to the top) and caps the list
        api
          .recordHistory(entry)
          .then((history) => set({ history }))
          .catch(() => {
            // keep at least the in-memory trail for this session
            set((s) => ({ history: [entry, ...s.history] }));
          });
      };
      const started = Date.now();
      try {
        const result = await api.executeSql(
          session.sessionId,
          sql,
          tab.state.maxRows,
          autoBegin,
        );
        pushHistory(true);
        notifyDone(tab.profileId, started, true, sql);
        patchTab<QueryTabState>(tabId, {
          result,
          resultSql: sql,
          explain: undefined,
          running: false,
        });
      } catch (e) {
        pushHistory(false);
        notifyDone(tab.profileId, started, false, sql);
        patchTab<QueryTabState>(tabId, {
          error: errText(e),
          connectionLost: isSessionLost(e),
          result: undefined,
          resultSql: undefined,
          running: false,
        });
        handleRunError(tab, isolated, e);
      }
      // BEGIN/COMMIT/ROLLBACK (or manual-commit's auto-BEGIN) may have changed
      // the tx state — refresh the badge (runs on both the ok and error paths).
      void refreshTxStatus(session);
    },

    runExplain: async (tabId, analyze, sqlOverride) => {
      const tab = tabOf(tabId, "query");
      if (!tab || tab.state.running) return;
      const sql = (sqlOverride ?? tab.state.sql).trim().replace(/;\s*$/, "");
      if (!sql) return;
      if (countStatements(sql) > 1) {
        get().showToast("Explain needs a single statement", "info");
        return;
      }
      const explainSql = `EXPLAIN (${analyze ? "ANALYZE, BUFFERS, " : ""}FORMAT JSON) ${sql}`;
      const isolated = Boolean(tab.state.isolated);
      const session = await beginRun(tab, isolated);
      if (!session) return;
      // ANALYZE executes the query, so honor manual-commit; plain EXPLAIN doesn't.
      const autoBegin = analyze && isolated && tab.state.commitMode === "manual";
      const started = Date.now();
      try {
        const exec = await api.executeSql(session.sessionId, explainSql, 10, autoBegin);
        const raw = exec.results[0]?.rows[0]?.[0];
        const parsed: unknown = raw ? JSON.parse(raw) : null;
        const root = Array.isArray(parsed)
          ? (parsed[0] as ExplainResult | undefined)
          : undefined;
        if (!root?.Plan) throw new Error("Unexpected EXPLAIN output");
        notifyDone(tab.profileId, started, true, explainSql);
        patchTab<QueryTabState>(tabId, {
          explain: { ...root, analyzed: analyze },
          running: false,
        });
      } catch (e) {
        patchTab<QueryTabState>(tabId, {
          error: errText(e),
          connectionLost: isSessionLost(e),
          explain: undefined,
          running: false,
        });
        handleRunError(tab, isolated, e);
      }
      if (analyze) void refreshTxStatus(session);
    },

    clearExplain: (tabId) =>
      patchTab<QueryTabState>(tabId, { explain: undefined }),

    cancelQuery: async (tabId) => {
      const tab = get().tabs.find((t) => t.id === tabId);
      if (!tab) return;
      const session = effectiveSession(tab);
      if (!session) return;
      if (get().exporting[session.sessionId]) {
        // CancelRequest бьёт по выполняемому сейчас стейтменту — это экспорт,
        // а не запрос, который стоит за ним в очереди; предупредить.
        get().showToast(
          "Cancelling the export running on this connection",
          "info",
        );
      }
      try {
        await api.cancelQuery(session.sessionId);
      } catch (e) {
        get().showToast(errText(e));
      }
    },

    isolateTab: async (tabId) => {
      const tab = tabOf(tabId, "query");
      if (!tab || tab.state.isolated) return;
      patchTab<QueryTabState>(tabId, { isolated: true });
      const session = await ensureIsolatedSession(tabId);
      if (!session) {
        patchTab<QueryTabState>(tabId, { isolated: false }); // open failed — revert
        return;
      }
      get().showToast(
        session.pid ? `Isolated on pid ${session.pid}` : "Isolated connection",
        "success",
      );
    },

    unisolateTab: async (tabId) => {
      const tab = tabOf(tabId, "query");
      if (!tab || !tab.state.isolated) return;
      dropIsolatedSession(tabId); // disconnect rolls back any open transaction
      patchTab<QueryTabState>(tabId, { isolated: false, commitMode: "auto" });
    },

    setCommitMode: async (tabId, mode) => {
      const tab = tabOf(tabId, "query");
      if (!tab) return;
      if (mode === "manual") {
        // manual holds a transaction open — it must run on its own connection
        if (!tab.state.isolated) {
          await get().isolateTab(tabId);
          if (!tabOf(tabId, "query")?.state.isolated) return; // isolate failed
        }
        patchTab<QueryTabState>(tabId, { commitMode: "manual" });
      } else {
        const cur = tabOf(tabId, "query");
        const session = cur ? effectiveSession(cur) : null;
        if (session?.tx && session.tx !== "idle") {
          get().showToast("Commit or roll back the open transaction first");
          return;
        }
        patchTab<QueryTabState>(tabId, { commitMode: "auto" });
      }
    },

    commitTx: (tabId) => runTxVerb(tabId, "COMMIT"),
    rollbackTx: (tabId) => runTxVerb(tabId, "ROLLBACK"),
  };
}
