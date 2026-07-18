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

  // App-wide hotkeys: Ctrl+1..9 switch active connections, ⌘`/⌘~ cycle them,
  // Ctrl+Tab and ⌘⇧]/⌘⇧[ cycle tabs, ⌘⌥O connection palette, ⌘P saved-queries
  // palette, ⌘S save query, ⌘R refresh table/structure view, ⌘W/⌘⇧T close/reopen
  // tab. Grid/editor hotkeys stay local.
  useEffect(() => {
    // On mac ⌘W/⌘⇧T arrive as native menu events (see lib.rs) — the menu
    // accelerator consumes the keypress before the webview sees it.
    const handleKey = (e: KeyboardEvent) => {
      const s = useApp.getState();
      const mod = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();

      if (e.ctrlKey && !e.metaKey && !e.altKey && /^[1-9]$/.test(e.key)) {
        const target = connectedProfiles(s.profiles, s.sessions)[Number(e.key) - 1];
        if (!target) return;
        e.preventDefault();
        s.selectProfile(target.id);
      } else if (e.ctrlKey && !e.metaKey && !e.altKey && !e.shiftKey && key === "c") {
        // Ctrl+C (terminal-style) — cancel the active tab's running query.
        // Only claimed while one is actually running, so copy stays intact
        // on platforms where Ctrl+C is the copy shortcut.
        const tab = s.tabs.find((t) => t.id === s.activeTabId);
        if (tab?.state.kind === "query" && tab.state.running) {
          e.preventDefault();
          void s.cancelQuery(tab.id);
        }
      } else if (e.ctrlKey && !e.metaKey && !e.altKey && e.key === "Tab") {
        // Ctrl+Tab / Ctrl+⇧Tab — cycle through this connection's tabs like a browser
        e.preventDefault();
        s.cycleTab(e.shiftKey ? -1 : 1);
      } else if (
        mod &&
        e.shiftKey &&
        !e.altKey &&
        (e.code === "BracketRight" || e.code === "BracketLeft")
      ) {
        // ⌘⇧] / ⌘⇧[ — next / previous tab (browser-style, works from anywhere)
        e.preventDefault();
        s.cycleTab(e.code === "BracketRight" ? 1 : -1);
      } else if (mod && !e.altKey && e.code === "Backquote") {
        // ⌘` next connection, ⌘~ (⌘⇧`) previous — macOS window-cycle convention
        e.preventDefault();
        s.cycleProfile(e.shiftKey ? -1 : 1);
      } else if (mod && e.altKey && e.code === "KeyO") {
        // e.code, not e.key — Option+O types "ø" on mac
        e.preventDefault();
        s.setPalette(s.palette === "connections" ? null : "connections");
      } else if (mod && !e.altKey && key === "p") {
        e.preventDefault();
        s.setPalette(s.palette === "queries" ? null : "queries");
      } else if (mod && !e.altKey && !e.shiftKey && key === "t") {
        // ⌘T — symbols palette (tables / columns / functions)
        if (s.activeProfileId && s.sessions[s.activeProfileId]) {
          e.preventDefault();
          s.setPalette(s.palette === "symbols" ? null : "symbols");
        }
      } else if (mod && !e.altKey && !e.shiftKey && key === ",") {
        // ⌘, — on mac the menu accelerator normally consumes this first
        e.preventDefault();
        s.setSettingsOpen(!s.settingsOpen);
      } else if (
        mod &&
        !e.altKey &&
        // ⌘? (= ⌘⇧/) and plain ⌘/ both open the shortcuts cheat-sheet;
        // defaultPrevented = the SQL editor already used ⌘/ as toggle-comment
        (e.key === "?" || (e.key === "/" && !e.defaultPrevented))
      ) {
        e.preventDefault();
        setShowShortcuts((v) => !v);
      } else if (mod && !e.altKey && !e.shiftKey && key === "b") {
        // ⌘B — toggle the sidebar
        e.preventDefault();
        s.toggleSidebar();
      } else if (mod && !e.altKey && !e.shiftKey && key === "j") {
        // ⌘J — toggle the AI agent panel
        e.preventDefault();
        s.toggleAgentPanel();
      } else if (mod && !e.altKey && !e.shiftKey && key === "k") {
        // chord leader: ⌘K then ⌘W closes all tabs
        e.preventDefault();
        chordAt.current = Date.now();
        s.showToast(
          isMac
            ? "⌘K — press ⌘W to close all tabs"
            : "Ctrl+K — press Ctrl+W to close all tabs",
          "info",
        );
      } else if (mod && !e.altKey && !e.shiftKey && key === "s") {
        // ⌘S: query tab saves, structure tab applies staged DDL
        // (table grids handle their own ⌘S apply)
        const tab = s.tabs.find((t) => t.id === s.activeTabId);
        if (tab?.state.kind === "query") {
          e.preventDefault();
          void s.saveQueryTab(tab.id);
        } else if (tab?.state.kind === "structure") {
          e.preventDefault();
          void s.applyStructureEdits(tab.id);
        }
      } else if (mod && !e.altKey && !e.shiftKey && key === "r") {
        // ⌘R: refresh the active table page / structure view (also keeps
        // the webview from reloading itself on Windows/Linux)
        const tab = s.tabs.find((t) => t.id === s.activeTabId);
        if (tab?.state.kind === "table") {
          e.preventDefault();
          void s.refreshTablePage(tab.id);
        } else if (tab?.state.kind === "structure") {
          e.preventDefault();
          void s.refreshStructure(tab.id);
        } else if (tab?.state.kind === "activity") {
          e.preventDefault();
          void s.refreshActivity(tab.id);
        }
      } else if (!isMac && e.ctrlKey && !e.altKey) {
        // JS fallback for the shortcuts the mac menu owns
        if (key === "w") {
          e.preventDefault();
          if (chordFired()) {
            // "all tabs" = the visible ones, i.e. the active connection's
            s.closeTabs(
              s.tabs
                .filter((t) => t.profileId === s.activeProfileId)
                .map((t) => t.id),
            );
          } else s.closeActiveTab();
        } else if (key === "t" && e.shiftKey) {
          e.preventDefault();
          s.reopenClosedTab();
        } else if (key === "n" && !e.shiftKey) {
          e.preventDefault();
          s.newQueryTab();
        }
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
        // the ⌘K chord's second key arrives as this menu event on mac;
        // "all tabs" = the visible ones, i.e. the active connection's
        if (chordFired()) {
          s.closeTabs(
            s.tabs
              .filter((t) => t.profileId === s.activeProfileId)
              .map((t) => t.id),
          );
        } else s.closeActiveTab();
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
