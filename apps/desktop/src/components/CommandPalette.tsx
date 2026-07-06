import { useEffect, useMemo, useRef, useState } from "react";
import { AnimatePresence, motion } from "framer-motion";
import {
  FolderPlus,
  RefreshCw,
  Download,
  SunMoon,
  Presentation,
  Search as SearchIcon,
  PanelRight,
  PanelLeft,
  Info,
  Settings,
  Trash2,
  ArrowDownUp,
} from "lucide-react";
import { useApp } from "../stores/useApp";
import { deckDisplayName, prefersReducedMotion } from "../lib/utils";

interface Action {
  id: string;
  label: string;
  hint?: string;
  icon: React.ReactNode;
  run: () => void;
}

/** ⌘K command palette: quick actions + jump-to-deck. */
export default function CommandPalette() {
  const open = useApp((s) => s.commandOpen);
  const decks = useApp((s) => s.decks);
  const reduce = prefersReducedMotion();
  const [q, setQ] = useState("");
  const [active, setActive] = useState(0);
  const inputRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    if (open) {
      setQ("");
      setActive(0);
      requestAnimationFrame(() => inputRef.current?.focus());
    }
  }, [open]);

  const actions = useMemo<Action[]>(() => {
    const app = useApp.getState();
    const base: Action[] = [
      {
        id: "add-folder",
        label: "Add folder…",
        icon: <FolderPlus size={15} />,
        run: () => void app.addFolder(),
      },
      {
        id: "rescan",
        label: "Re-index all folders",
        hint: "⌘R",
        icon: <RefreshCw size={15} />,
        run: () => void app.startScan(),
      },
      {
        id: "clear-rebuild",
        label: "Clear index & rebuild…",
        icon: <Trash2 size={15} />,
        run: () => app.confirmClearAndRebuild(),
      },
      {
        id: "export",
        label: "Export tray…",
        hint: "⌘E",
        icon: <Download size={15} />,
        run: () => app.setExportOpen(true),
      },
      {
        id: "theme",
        label: "Toggle theme (System / Light / Dark)",
        icon: <SunMoon size={15} />,
        run: () => app.cycleTheme(),
      },
      {
        id: "sort-name",
        label: "Sort by: Name",
        icon: <ArrowDownUp size={15} />,
        run: () => app.setSortMode("name"),
      },
      {
        id: "sort-added",
        label: "Sort by: Recently added",
        icon: <ArrowDownUp size={15} />,
        run: () => app.setSortMode("added"),
      },
      {
        id: "sort-modified",
        label: "Sort by: Recently modified",
        icon: <ArrowDownUp size={15} />,
        run: () => app.setSortMode("modified"),
      },
      {
        id: "sort-exported",
        label: "Sort by: Most exported",
        hint: "counting starts now",
        icon: <ArrowDownUp size={15} />,
        run: () => app.setSortMode("exported"),
      },
      {
        id: "all",
        label: "Go to All Slides",
        icon: <SearchIcon size={15} />,
        run: () => void app.setNav({ type: "all" }),
      },
      {
        id: "sidebar",
        label: "Toggle sidebar",
        hint: "⌘⌃S",
        icon: <PanelLeft size={15} />,
        run: () => app.toggleSidebar(),
      },
      {
        id: "inspector",
        label: "Toggle inspector",
        hint: "⌘I",
        icon: <PanelRight size={15} />,
        run: () => app.toggleInspector(),
      },
      {
        id: "settings",
        label: "Settings…",
        hint: "⌘,",
        icon: <Settings size={15} />,
        run: () => app.setSettingsOpen(true),
      },
      {
        id: "about",
        label: "About Slideflow",
        icon: <Info size={15} />,
        run: () => app.setAboutOpen(true),
      },
    ];
    const deckActions: Action[] = decks.map((d) => ({
      id: `deck-${d.id}`,
      label: `Jump to: ${deckDisplayName(d)}`,
      hint: `${d.slide_count} slides`,
      icon: <Presentation size={15} />,
      run: () => void app.setNav({ type: "deck", id: d.id }),
    }));
    return [...base, ...deckActions];
  }, [decks]);

  const filtered = useMemo(() => {
    const needle = q.trim().toLowerCase();
    if (!needle) return actions;
    return actions.filter((a) => a.label.toLowerCase().includes(needle));
  }, [q, actions]);

  useEffect(() => {
    setActive(0);
  }, [q]);

  const close = () => useApp.getState().setCommandOpen(false);

  const onKeyDown = (e: React.KeyboardEvent) => {
    // Only swallow keys the palette actually consumes — cmd shortcuts (⌘K to
    // toggle closed, etc.) must still reach App.tsx's window listener. Without
    // stopPropagation, Escape here would bubble on to App and wipe the search
    // query behind the palette.
    if (e.key === "ArrowDown") {
      e.preventDefault();
      e.stopPropagation();
      setActive((a) => Math.min(filtered.length - 1, a + 1));
    } else if (e.key === "ArrowUp") {
      e.preventDefault();
      e.stopPropagation();
      setActive((a) => Math.max(0, a - 1));
    } else if (e.key === "Enter") {
      e.preventDefault();
      e.stopPropagation();
      const action = filtered[active];
      if (action) {
        close();
        action.run();
      }
    } else if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    }
  };

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          className="fixed inset-0 z-[95] flex items-start justify-center bg-black/30 p-4 pt-[14vh] backdrop-blur-sm"
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          transition={{ duration: reduce ? 0 : 0.12 }}
          onClick={close}
        >
          <motion.div
            className="w-full max-w-lg overflow-hidden rounded-[12px] bg-elevated shadow-peek"
            initial={reduce ? false : { scale: 0.97, opacity: 0, y: -8 }}
            animate={{ scale: 1, opacity: 1, y: 0 }}
            exit={reduce ? { opacity: 0 } : { scale: 0.98, opacity: 0 }}
            transition={{ type: "spring", stiffness: 340, damping: 30 }}
            onClick={(e) => e.stopPropagation()}
          >
            <div className="flex items-center gap-2 px-3 hairline-b">
              <SearchIcon size={16} className="text-subtle" />
              <input
                ref={inputRef}
                value={q}
                onChange={(e) => setQ(e.target.value)}
                onKeyDown={onKeyDown}
                placeholder="Type a command or deck…"
                className="selectable h-11 w-full bg-transparent text-body text-ink outline-none placeholder:text-subtle/70"
              />
            </div>
            <div className="max-h-80 overflow-y-auto p-1.5">
              {filtered.length === 0 ? (
                <div className="px-3 py-6 text-center text-caption text-subtle">
                  No matching commands
                </div>
              ) : (
                filtered.map((a, i) => (
                  <button
                    key={a.id}
                    onMouseEnter={() => setActive(i)}
                    onClick={() => {
                      close();
                      a.run();
                    }}
                    className={`flex w-full items-center gap-2.5 rounded-[7px] px-2.5 py-2 text-left text-body ${
                      i === active ? "bg-accent text-white" : "text-ink"
                    }`}
                  >
                    <span
                      className={i === active ? "text-white" : "text-subtle"}
                    >
                      {a.icon}
                    </span>
                    <span className="flex-1 truncate">{a.label}</span>
                    {a.hint && (
                      <span
                        className={`tabnum text-caption ${
                          i === active ? "text-white/80" : "text-subtle/70"
                        }`}
                      >
                        {a.hint}
                      </span>
                    )}
                  </button>
                ))
              )}
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  );
}
