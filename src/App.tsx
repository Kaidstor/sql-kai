import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { message } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { ActivityTab } from "./components/ActivityTab";
import { AgentPanel } from "./components/AgentPanel";
import { ConfirmDialog } from "./components/ConfirmDialog";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { Launcher } from "./components/Launcher";
import { LogViewer } from "./components/LogViewer";
import { Palette } from "./components/Palette";
import { QueryTab } from "./components/QueryTab";
import { SettingsDialog } from "./components/SettingsDialog";
import { ShortcutSections, ShortcutsOverlay } from "./components/ShortcutsHelp";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { StructureTab } from "./components/StructureTab";
import { TableTab } from "./components/TableTab";
import { TabsBar } from "./components/TabsBar";
import { UpdateToast } from "./components/UpdateToast";
import { WhatsNewDialog } from "./components/WhatsNewDialog";
import { VaultGate } from "./components/VaultGate";
import { api } from "./lib/api";
import { buildGuiContext } from "./lib/guiContext";
import { isMac } from "./lib/platform";
import { connectedProfiles } from "./lib/profile";
import { useApp } from "./lib/store";
import { initUpdater, useUpdater } from "./lib/updater";

// ⌘K chord window: a ⌘W within this many ms closes ALL tabs.
const CHORD_MS = 5000;

type Store = ReturnType<typeof useApp.getState>;

/** One app-wide key binding. Order in the table is match priority. */
interface Hotkey {
  /** Human-readable combo — documentation and grep anchor, not used at runtime. */
  combo: string;
  match: (e: KeyboardEvent) => boolean;
  /** Returns false to pass the event through (no preventDefault) — for
   *  bindings that only claim the key under a condition (e.g. Ctrl+C only
   *  cancels while a query is running, so copy stays intact elsewhere). */
  run: (s: Store, e: KeyboardEvent) => boolean | void;
}

const mod = (e: KeyboardEvent) => e.metaKey || e.ctrlKey;
const ctrlOnly = (e: KeyboardEvent) => e.ctrlKey && !e.metaKey && !e.altKey;

/** "All tabs" = the visible ones, i.e. the active connection's. */
const closeAllVisibleTabs = (s: Store) =>
  s.closeTabs(
    s.tabs.filter((t) => t.profileId === s.activeProfileId).map((t) => t.id),
  );

function App() {
  const init = useApp((s) => s.init);
  const tabs = useApp((s) => s.tabs);
  const activeTabId = useApp((s) => s.activeTabId);
  const activeProfileId = useApp((s) => s.activeProfileId);
  const profiles = useApp((s) => s.profiles);
  const sessions = useApp((s) => s.sessions);
  const lost = useApp((s) => s.lost);
  const launcherOpen = useApp((s) => s.launcherOpen);
  const sidebarOpen = useApp((s) => s.sidebarOpen);
  const agentOpen = useApp((s) => s.agentOpen);
  const [showShortcuts, setShowShortcuts] = useState(false);
  // When ⌘K was pressed; shared by the keydown handler and (on mac, where
  // ⌘W arrives as a native menu event) the menu listener.
  const chordAt = useRef(0);

  const chordFired = () => {
    if (Date.now() - chordAt.current >= CHORD_MS) return false;
    chordAt.current = 0;
    return true;
  };

  useEffect(() => {
    void init();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // Окно создаётся скрытым (visible: false) — показываем его после того, как
  // UI отрисовал первый кадр, чтобы не было «растягивания» окна из дефолтной
  // геометрии на старте и при перезапуске после автообновления. Двойной rAF:
  // первый колбэк — перед ближайшей отрисовкой, второй — уже после неё.
  useEffect(() => {
    const outer = requestAnimationFrame(() => {
      requestAnimationFrame(() => {
        void api.revealWindow().catch(() => {});
      });
    });
    return () => cancelAnimationFrame(outer);
  }, []);

  useEffect(() => initUpdater(), []);

  // The native menu-bar switcher mirrors the live connections shown in the
  // status bar. Rebuilding it also updates the checkmark when selection moves.
  useEffect(() => {
    const connections = connectedProfiles(profiles, sessions).map((profile) => ({
      profileId: profile.id,
      name: profile.name,
    }));
    void api.syncTrayConnections(connections, activeProfileId).catch(() => {
      // Non-macOS builds and an app that is already shutting down have no tray
      // to update; neither case should affect the workspace.
    });
  }, [activeProfileId, profiles, sessions]);

  // App-wide hotkeys — a declarative table, first match wins. Grid/editor
  // hotkeys stay local to their components.
  useEffect(() => {
    // On mac ⌘W/⌘⇧T/⌘N arrive as native menu events (see lib.rs) — the menu
    // accelerator consumes the keypress before the webview sees it; the
    // trailing Ctrl+… bindings are the JS fallback for other platforms.
    const hotkeys: Hotkey[] = [
      {
        combo: "Ctrl+1…9",
        match: (e) => ctrlOnly(e) && /^[1-9]$/.test(e.key),
        run: (s, e) => {
          const target =
            connectedProfiles(s.profiles, s.sessions)[Number(e.key) - 1];
          if (!target) return false;
          s.selectProfile(target.id);
        },
      },
      {
        // Terminal-style cancel of the active tab's running query. Only
        // claimed while one is actually running, so copy stays intact on
        // platforms where Ctrl+C is the copy shortcut.
        combo: "Ctrl+C",
        match: (e) => ctrlOnly(e) && !e.shiftKey && e.key.toLowerCase() === "c",
        run: (s) => {
          const tab = s.tabs.find((t) => t.id === s.activeTabId);
          if (tab?.state.kind !== "query" || !tab.state.running) return false;
          void s.cancelQuery(tab.id);
        },
      },
      {
        // Cycle through this connection's tabs like a browser (⇧ = back).
        combo: "Ctrl+Tab / Ctrl+⇧Tab",
        match: (e) => ctrlOnly(e) && e.key === "Tab",
        run: (s, e) => s.cycleTab(e.shiftKey ? -1 : 1),
      },
      {
        // Next / previous tab (browser-style, works from anywhere).
        combo: "⌘⇧] / ⌘⇧[",
        match: (e) =>
          mod(e) &&
          e.shiftKey &&
          !e.altKey &&
          (e.code === "BracketRight" || e.code === "BracketLeft"),
        run: (s, e) => s.cycleTab(e.code === "BracketRight" ? 1 : -1),
      },
      {
        // Next / previous connection — macOS window-cycle convention.
        combo: "⌘` / ⌘~",
        match: (e) => mod(e) && !e.altKey && e.code === "Backquote",
        run: (s, e) => s.cycleProfile(e.shiftKey ? -1 : 1),
      },
      {
        // e.code, not e.key — Option+O types "ø" on mac.
        combo: "⌘⌥O",
        match: (e) => mod(e) && e.altKey && e.code === "KeyO",
        run: (s) =>
          s.setPalette(s.palette === "connections" ? null : "connections"),
      },
      {
        combo: "⌘P",
        match: (e) => mod(e) && !e.altKey && e.key.toLowerCase() === "p",
        run: (s) => s.setPalette(s.palette === "queries" ? null : "queries"),
      },
      {
        // Symbols palette (tables / columns / functions) — needs a live session.
        combo: "⌘T",
        match: (e) =>
          mod(e) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "t",
        run: (s) => {
          if (!s.activeProfileId || !s.sessions[s.activeProfileId]) return false;
          s.setPalette(s.palette === "symbols" ? null : "symbols");
        },
      },
      {
        // On mac the menu accelerator normally consumes this first.
        combo: "⌘,",
        match: (e) => mod(e) && !e.altKey && !e.shiftKey && e.key === ",",
        run: (s) => s.setSettingsOpen(!s.settingsOpen),
      },
      {
        // ⌘? (= ⌘⇧/) and plain ⌘/ both open the shortcuts cheat-sheet;
        // defaultPrevented = the SQL editor already used ⌘/ as toggle-comment.
        combo: "⌘? / ⌘/",
        match: (e) =>
          mod(e) &&
          !e.altKey &&
          (e.key === "?" || (e.key === "/" && !e.defaultPrevented)),
        run: () => setShowShortcuts((v) => !v),
      },
      {
        combo: "⌘B — toggle sidebar",
        match: (e) =>
          mod(e) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "b",
        run: (s) => s.toggleSidebar(),
      },
      {
        combo: "⌘J — toggle agent panel",
        match: (e) =>
          mod(e) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "j",
        run: (s) => s.toggleAgentPanel(),
      },
      {
        // Chord leader: ⌘K then ⌘W closes all tabs.
        combo: "⌘K",
        match: (e) =>
          mod(e) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "k",
        run: (s) => {
          chordAt.current = Date.now();
          s.showToast(
            isMac
              ? "⌘K — press ⌘W to close all tabs"
              : "Ctrl+K — press Ctrl+W to close all tabs",
            "info",
          );
        },
      },
      {
        // Query tab saves, structure tab applies staged DDL (table grids
        // handle their own ⌘S apply).
        combo: "⌘S",
        match: (e) =>
          mod(e) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "s",
        run: (s) => {
          const tab = s.tabs.find((t) => t.id === s.activeTabId);
          if (tab?.state.kind === "query") void s.saveQueryTab(tab.id);
          else if (tab?.state.kind === "structure")
            void s.applyStructureEdits(tab.id);
          else return false;
        },
      },
      {
        // Refresh the active table page / structure / activity view (also
        // keeps the webview from reloading itself on Windows/Linux).
        combo: "⌘R",
        match: (e) =>
          mod(e) && !e.altKey && !e.shiftKey && e.key.toLowerCase() === "r",
        run: (s) => {
          const tab = s.tabs.find((t) => t.id === s.activeTabId);
          if (tab?.state.kind === "table") void s.refreshTablePage(tab.id);
          else if (tab?.state.kind === "structure")
            void s.refreshStructure(tab.id);
          else if (tab?.state.kind === "activity")
            void s.refreshActivity(tab.id);
          else return false;
        },
      },
      {
        combo: "Ctrl+W (non-mac)",
        match: (e) =>
          !isMac && e.ctrlKey && !e.altKey && e.key.toLowerCase() === "w",
        run: (s) => {
          if (chordFired()) closeAllVisibleTabs(s);
          else s.closeActiveTab();
        },
      },
      {
        combo: "Ctrl+⇧T (non-mac)",
        match: (e) =>
          !isMac &&
          e.ctrlKey &&
          !e.altKey &&
          e.shiftKey &&
          e.key.toLowerCase() === "t",
        run: (s) => s.reopenClosedTab(),
      },
      {
        combo: "Ctrl+N (non-mac)",
        match: (e) =>
          !isMac &&
          e.ctrlKey &&
          !e.altKey &&
          !e.shiftKey &&
          e.key.toLowerCase() === "n",
        run: (s) => s.newQueryTab(),
      },
    ];

    const handleKey = (e: KeyboardEvent) => {
      const s = useApp.getState();
      for (const hotkey of hotkeys) {
        if (!hotkey.match(e)) continue;
        if (hotkey.run(s, e) !== false) e.preventDefault();
        return;
      }
    };
    window.addEventListener("keydown", handleKey);
    return () => window.removeEventListener("keydown", handleKey);
  }, []);

  // Native menu items (mac): New Query Tab ⌘N, Close Tab ⌘W, Reopen ⌘⇧T.
  useEffect(() => {
    const unlisten = [
      listen("menu://new-query-tab", () => useApp.getState().newQueryTab()),
      listen("menu://close-tab", () => {
        const s = useApp.getState();
        // the ⌘K chord's second key arrives as this menu event on mac
        if (chordFired()) closeAllVisibleTabs(s);
        else s.closeActiveTab();
      }),
      listen("menu://reopen-tab", () => useApp.getState().reopenClosedTab()),
      listen("menu://settings", () => useApp.getState().setSettingsOpen(true)),
      listen("menu://log-viewer", () =>
        useApp.getState().setLogViewerOpen(true),
      ),
      listen("menu://check-updates", () =>
        void useUpdater.getState().checkForUpdates(true),
      ),
      listen("menu://install-cli", async () => {
        try {
          const path = await invoke<string>("install_cli");
          await message(
            `sql-kai установлен: ${path}\n\nОткройте новый терминал и проверьте: sql-kai -V`,
            { title: "Install CLI", kind: "info" },
          );
        } catch (e) {
          const msg = String(e);
          if (msg.includes("cancelled")) return; // отменил диалог пароля
          await message(`Не удалось установить CLI:\n${msg}`, {
            title: "Install CLI",
            kind: "error",
          });
        }
      }),
      listen<string>("tray://select-connection", async (e) => {
        const profileId = e.payload;
        const s = useApp.getState();
        if (s.sessions[profileId]) {
          s.selectProfile(profileId);
        } else if (s.lost[profileId]) {
          await s.reconnect(profileId);
        } else {
          // Handles a click from a stale native menu snapshot gracefully.
          await s.connect(profileId);
        }
      }),
      // сессия умерла на проводе (ssh-туннель/сеть/сервер) — бэкенд сообщает
      // сразу, не дожидаясь, пока следующий запрос наткнётся на труп
      listen<{ sessionId: string; profileId: string; reason: string }>(
        "session://lost",
        (e) =>
          useApp
            .getState()
            .markSessionLost(e.payload.sessionId, e.payload.profileId),
      ),
      // брокер: sql-kai открыл/закрыл cli-сессию — обновить бейджи
      listen("broker://changed", () =>
        void useApp.getState().refreshCliSessions(),
      ),
      // sql-kai discover/rm изменил состав профилей или обновилась отметка
      // last connected (cli-коннект) — перечитать список
      listen("profiles://changed", () =>
        void useApp.getState().reloadProfiles(),
      ),
      // MCP-tools агента (open_table/open_query): открыть вкладку в GUI
      listen<
        | { kind: "table"; profileId: string; schema: string; table: string }
        | { kind: "query"; profileId: string; sql: string }
      >("agent://open", (e) => {
        const s = useApp.getState();
        const p = e.payload;
        // вкладка осмысленна только на живом подключении этого профиля
        if (!s.sessions[p.profileId]) return;
        if (p.kind === "table") {
          s.openTableTab(p.profileId, p.schema, p.table);
        } else {
          s.openQueryTab(p.profileId, p.sql);
        }
      }),
      // MCP-tool агента `selection`: собрать активную вкладку/выделение из
      // стора и ответить брокеру (agent_gui_reply ждёт oneshot по id)
      listen<{ id: string; kind: string; profileId: string }>(
        "agent://gui-request",
        (e) => {
          const payload = buildGuiContext(useApp.getState(), e.payload.profileId);
          void invoke("agent_gui_reply", { id: e.payload.id, payload });
        },
      ),
    ];
    return () => {
      for (const u of unlisten) void u.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const activeTab = tabs.find((t) => t.id === activeTabId);
  // The workspace (sidebar + tabs) needs an active connection behind it —
  // live or lost-but-cached. Anything else shows the launcher; it also
  // opens on demand over a live workspace ("All connections").
  const hasWorkspace = Boolean(
    activeProfileId && (sessions[activeProfileId] || lost[activeProfileId]),
  );
  const showLauncher = launcherOpen || !hasWorkspace;

  return (
    <VaultGate>
    <div className="h-screen flex flex-col bg-zinc-950 text-zinc-200 text-[13px] antialiased">
      <div className="flex flex-1 min-h-0">
        {showLauncher ? (
          <Launcher />
        ) : (
          <>
            {sidebarOpen && <Sidebar />}
            <main className="flex-1 flex flex-col min-w-0 min-h-0">
              <TabsBar />
              <div className="flex-1 min-h-0">
                {activeTab ? (
                  activeTab.state.kind === "query" ? (
                    <QueryTab key={activeTab.id} tab={activeTab} />
                  ) : activeTab.state.kind === "table" ? (
                    <TableTab key={activeTab.id} tab={activeTab} />
                  ) : activeTab.state.kind === "activity" ? (
                    <ActivityTab key={activeTab.id} tab={activeTab} />
                  ) : (
                    <StructureTab key={activeTab.id} tab={activeTab} />
                  )
                ) : (
                  <div className="h-full flex flex-col items-center justify-center gap-2 overflow-y-auto text-zinc-600">
                    <div className="text-[15px]">sql-kai</div>
                    <div className="text-[12px]">
                      No open tabs — ⌘N starts a new query
                    </div>
                    <ShortcutSections className="mt-8 px-6" />
                  </div>
                )}
              </div>
            </main>
            {/* агент требует живого подключения — на lost-сессии панель прячется */}
            {agentOpen && activeProfileId && sessions[activeProfileId] && (
              <AgentPanel />
            )}
          </>
        )}
      </div>
      <StatusBar />
      <UpdateToast />
      <WhatsNewDialog />
      <ConnectionDialog />
      <ConfirmDialog />
      <SettingsDialog />
      <LogViewer />
      <Palette />
      <ShortcutsOverlay
        open={showShortcuts}
        onClose={() => setShowShortcuts(false)}
      />
    </div>
    </VaultGate>
  );
}

export default App;
