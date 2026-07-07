import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { ConnectionDialog } from "./components/ConnectionDialog";
import { AppPalette } from "./components/Palette";
import { QueryTab } from "./components/QueryTab";
import { SettingsDialog } from "./components/SettingsDialog";
import { ShortcutSections, ShortcutsOverlay } from "./components/ShortcutsHelp";
import { Sidebar } from "./components/Sidebar";
import { StatusBar } from "./components/StatusBar";
import { StructureTab } from "./components/StructureTab";
import { TableTab } from "./components/TableTab";
import { TabsBar } from "./components/TabsBar";
import { VaultGate } from "./components/VaultGate";
import { connectedProfiles } from "./lib/profile";
import { useApp } from "./lib/store";
import { initUpdater } from "./lib/updater";

// ⌘K chord window: a ⌘W within this many ms closes ALL tabs.
const CHORD_MS = 5000;

function App() {
  const { init, tabs, activeTabId } = useApp();
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

  // App-wide hotkeys: Ctrl+1..9 switch active connections, ⌘⌥O connection
  // palette, ⌘P saved-queries palette, ⌘S save query, ⌘R refresh table/
  // structure view, ⌘W/⌘⇧T close/reopen tab. Grid/editor hotkeys stay local.
  useEffect(() => {
    // On mac ⌘W/⌘⇧T arrive as native menu events (see lib.rs) — the menu
    // accelerator consumes the keypress before the webview sees it.
    const isMac = navigator.userAgent.includes("Mac");
    const onKey = (e: KeyboardEvent) => {
      const s = useApp.getState();
      const mod = e.metaKey || e.ctrlKey;
      const key = e.key.toLowerCase();

      if (e.ctrlKey && !e.metaKey && !e.altKey && /^[1-9]$/.test(e.key)) {
        const target = connectedProfiles(s.profiles, s.sessions)[Number(e.key) - 1];
        if (!target) return;
        e.preventDefault();
        s.selectProfile(target.id);
      } else if (mod && e.altKey && e.code === "KeyO") {
        // e.code, not e.key — Option+O types "ø" on mac
        e.preventDefault();
        s.setPalette(s.palette === "connections" ? null : "connections");
      } else if (mod && !e.altKey && key === "p") {
        e.preventDefault();
        s.setPalette(s.palette === "queries" ? null : "queries");
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
          void s.loadTablePage(tab.id);
        } else if (tab?.state.kind === "structure") {
          e.preventDefault();
          void s.loadStructure(tab.id);
        }
      } else if (!isMac && e.ctrlKey && !e.altKey) {
        // JS fallback for the shortcuts the mac menu owns
        if (key === "w") {
          e.preventDefault();
          if (chordFired()) s.closeTabs(s.tabs.map((t) => t.id));
          else s.closeActiveTab();
        } else if (key === "t" && e.shiftKey) {
          e.preventDefault();
          s.reopenClosedTab();
        } else if (key === "n" && !e.shiftKey) {
          e.preventDefault();
          s.newQueryTab();
        }
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // Native menu items (mac): New Query Tab ⌘N, Close Tab ⌘W, Reopen ⌘⇧T.
  useEffect(() => {
    const unlisten = [
      listen("menu://new-query-tab", () => useApp.getState().newQueryTab()),
      listen("menu://close-tab", () => {
        const s = useApp.getState();
        // the ⌘K chord's second key arrives as this menu event on mac
        if (chordFired()) s.closeTabs(s.tabs.map((t) => t.id));
        else s.closeActiveTab();
      }),
      listen("menu://reopen-tab", () => useApp.getState().reopenClosedTab()),
      listen("menu://settings", () => useApp.getState().setSettingsOpen(true)),
    ];
    return () => {
      for (const u of unlisten) void u.then((f) => f());
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const activeTab = tabs.find((t) => t.id === activeTabId);

  return (
    <VaultGate>
    <div className="h-screen flex flex-col bg-zinc-950 text-zinc-200 text-[13px] antialiased">
      <div className="flex flex-1 min-h-0">
        <Sidebar />
        <main className="flex-1 flex flex-col min-w-0 min-h-0">
          <TabsBar />
          <div className="flex-1 min-h-0">
            {activeTab ? (
              activeTab.state.kind === "query" ? (
                <QueryTab key={activeTab.id} tab={activeTab} />
              ) : activeTab.state.kind === "table" ? (
                <TableTab key={activeTab.id} tab={activeTab} />
              ) : (
                <StructureTab key={activeTab.id} tab={activeTab} />
              )
            ) : (
              <div className="h-full flex flex-col items-center justify-center gap-2 overflow-y-auto text-zinc-600">
                <div className="text-[15px]">sql-tauri</div>
                <div className="text-[12px]">
                  Connect to a database to get started
                </div>
                <ShortcutSections className="mt-8 px-6" />
              </div>
            )}
          </div>
        </main>
      </div>
      <StatusBar />
      <ConnectionDialog />
      <SettingsDialog />
      <AppPalette />
      <ShortcutsOverlay
        open={showShortcuts}
        onClose={() => setShowShortcuts(false)}
      />
    </div>
    </VaultGate>
  );
}

export default App;
