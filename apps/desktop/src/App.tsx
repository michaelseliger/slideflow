import { useEffect } from "react";
import { useApp, applyTheme } from "./stores/useApp";
import { useTray } from "./stores/useTray";
import * as api from "./lib/api";
import { cmdKey } from "./lib/utils";

import Sidebar from "./components/Sidebar";
import Header from "./components/Header";
import Inspector from "./components/Inspector";
import Tray from "./components/Tray";
import SlideGrid from "./components/SlideGrid";
import EmptyState from "./components/EmptyState";
import ZeroResults from "./components/ZeroResults";
import PeekModal from "./components/PeekModal";
import StatsView from "./components/StatsView";
import CommandPalette from "./components/CommandPalette";
import ExportSheet from "./components/ExportSheet";
import AboutSheet from "./components/AboutSheet";
import Toaster from "./components/Toaster";

function focusSearch() {
  const el = document.getElementById("global-search") as HTMLInputElement | null;
  el?.focus();
  el?.select();
}

function isEditable(el: EventTarget | null): boolean {
  const node = el as HTMLElement | null;
  if (!node) return false;
  const tag = node.tagName;
  return (
    tag === "INPUT" ||
    tag === "TEXTAREA" ||
    tag === "SELECT" ||
    node.isContentEditable
  );
}

export default function App() {
  const ready = useApp((s) => s.ready);
  const stats = useApp((s) => s.stats);
  const roots = useApp((s) => s.roots);
  const results = useApp((s) => s.results);
  const searching = useApp((s) => s.searching);
  const inspectorVisible = useApp((s) => s.inspectorVisible);
  const gridCols = useApp((s) => s.gridCols);
  const scanRunning = useApp((s) => s.scan.running);
  const navType = useApp((s) => s.nav.type);

  // Boot: load library + first search, then subscribe to scan progress.
  useEffect(() => {
    void useApp.getState().init();
    let unlisten: (() => void) | undefined;
    api
      .onScanEvent((ev) => useApp.getState().handleScanEvent(ev))
      .then((un) => (unlisten = un));
    return () => unlisten?.();
  }, []);

  // Follow the system theme live when in "system" mode.
  useEffect(() => {
    const mq = window.matchMedia("(prefers-color-scheme: dark)");
    const onChange = () => {
      if (useApp.getState().theme === "system") {
        useApp.setState({ dark: applyTheme("system") });
      }
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);

  // Global keyboard map (see the UX brief's keyboard section).
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const app = useApp.getState();
      const tray = useTray.getState();
      const editing = isEditable(document.activeElement);
      const key = e.key;

      // --- Peek modal owns arrows / space / esc while open. ---
      if (app.peekIndex != null) {
        if (key === "Escape" || key === " ") {
          e.preventDefault();
          app.closePeek();
        } else if (key === "ArrowRight" || key === "ArrowDown") {
          e.preventDefault();
          app.peekBy(1);
        } else if (key === "ArrowLeft" || key === "ArrowUp") {
          e.preventDefault();
          app.peekBy(-1);
        }
        return;
      }

      // --- Command-key shortcuts (work even while editing). ---
      if (cmdKey(e)) {
        // cmd-ctrl-S — toggle sidebar
        if (e.ctrlKey && (key === "s" || key === "S")) {
          e.preventDefault();
          app.toggleSidebar();
          return;
        }
        switch (key.toLowerCase()) {
          case "f":
            e.preventDefault();
            focusSearch();
            return;
          case "k":
            e.preventDefault();
            app.setCommandOpen(!app.commandOpen);
            return;
          case "i":
            e.preventDefault();
            app.toggleInspector();
            return;
          case "t":
            e.preventDefault();
            tray.toggleCollapsed();
            return;
          case "e":
            e.preventDefault();
            if (tray.items.length > 0) app.setExportOpen(true);
            return;
          case "r":
            e.preventDefault();
            void app.startScan();
            return;
          case "z":
            e.preventDefault();
            if (e.shiftKey) tray.redo();
            else tray.undo();
            return;
          case "=":
          case "+":
            e.preventDefault();
            app.decCols(); // larger thumbnails
            return;
          case "-":
            e.preventDefault();
            app.incCols(); // smaller thumbnails
            return;
          case "a":
            if (!editing) {
              e.preventDefault();
              app.selectAll();
            }
            return;
          case "backspace":
            e.preventDefault();
            removeSelectedFromTray();
            return;
          default:
            return;
        }
      }

      // --- Escape priority: clear search → close inspector. ---
      if (key === "Escape") {
        if (app.query) {
          app.setQuery("");
          return;
        }
        if (app.inspectorVisible) {
          app.setInspector(false);
          return;
        }
        return;
      }

      // The rest are grid interactions — skip while typing in a field.
      if (editing) return;

      switch (key) {
        case "ArrowRight":
          e.preventDefault();
          app.moveSelection(1, gridCols);
          break;
        case "ArrowLeft":
          e.preventDefault();
          app.moveSelection(-1, gridCols);
          break;
        case "ArrowDown":
          e.preventDefault();
          app.moveSelection(2, gridCols);
          break;
        case "ArrowUp":
          e.preventDefault();
          app.moveSelection(-2, gridCols);
          break;
        case " ": {
          const idx = app.anchorIndex;
          if (idx != null && app.results[idx]) {
            e.preventDefault();
            app.openPeek(idx);
          }
          break;
        }
        case "Enter":
          e.preventDefault();
          app.addSelectionToTray();
          break;
        case "1":
          app.setGrouping("flat");
          break;
        case "2":
          app.setGrouping("deck");
          break;
      }
    };

    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [gridCols]);

  const hasLibrary = roots.length > 0 || stats.slide_count > 0;
  const showEmptyOnboarding = ready && !hasLibrary && !scanRunning;

  return (
    <div className="flex h-full flex-col overflow-hidden bg-canvas text-ink">
      <div className="flex min-h-0 flex-1">
        <Sidebar />

        <div className="flex min-w-0 flex-1 flex-col">
          <Header />

          <div className="flex min-h-0 flex-1">
            <main className="min-w-0 flex-1 bg-canvas">
              {navType === "stats" ? (
                <StatsView />
              ) : showEmptyOnboarding ? (
                <EmptyState />
              ) : results.length === 0 && !searching ? (
                <ZeroResults />
              ) : (
                <SlideGrid />
              )}
            </main>

            {inspectorVisible && <Inspector />}
          </div>
        </div>
      </div>

      <Tray />

      <PeekModal />
      <CommandPalette />
      <ExportSheet />
      <AboutSheet />
      <Toaster />
    </div>
  );
}

/** ⌘-Backspace: remove currently-selected slides from the tray, else the last. */
function removeSelectedFromTray() {
  const app = useApp.getState();
  const tray = useTray.getState();
  if (tray.items.length === 0) return;
  const selectedUids = new Set(
    app.results
      .filter((r) => app.selectedIds.has(r.slide.id))
      .map((r) => `${r.slide.deck_id}:${r.slide.slide_index}`),
  );
  const toRemove = tray.items.filter((i) => selectedUids.has(i.uid));
  if (toRemove.length > 0) {
    toRemove.forEach((i) => tray.remove(i.uid));
  } else {
    tray.removeAt(tray.items.length - 1);
  }
}
