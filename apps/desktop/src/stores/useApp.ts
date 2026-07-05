// The primary application store: library data, search/browse state, selection,
// panel layout, theme, and live scan progress. The composition tray lives in
// its own store (`useTray`) since it has independent persistence + undo.

import { create } from "zustand";
import * as api from "../lib/api";
import type {
  DeckRecord,
  RootRecord,
  ScanEvent,
  SearchFilters,
  SearchHit,
  Stats,
} from "../lib/types";
import { useTray } from "./useTray";
import { toast } from "./useToast";
import { clearSlideSvgCache } from "../lib/useSlideSvg";

export type ThemeMode = "light" | "dark" | "system";
export type Grouping = "flat" | "deck";

export interface NavTarget {
  type: "all" | "root" | "deck" | "favorites" | "stats";
  id?: number;
}

export interface ScanState {
  running: boolean;
  done: number;
  total: number;
  indexed: number;
  lastPath: string | null;
}

/** A reusable confirm-dialog request. Rendered by `ConfirmDialog`; the store
 *  holds at most one at a time. */
export interface ConfirmConfig {
  title: string;
  message: string;
  confirmLabel: string;
  cancelLabel?: string;
  destructive?: boolean;
  onConfirm: () => void | Promise<void>;
}

const THEME_KEY = "slideflow.theme";
const COLS_KEY = "slideflow.gridCols";

function loadTheme(): ThemeMode {
  const v = localStorage.getItem(THEME_KEY);
  return v === "light" || v === "dark" || v === "system" ? v : "system";
}

function systemPrefersDark(): boolean {
  return window.matchMedia?.("(prefers-color-scheme: dark)").matches ?? false;
}

/** Apply the resolved theme to <html> and return whether dark is active. */
export function applyTheme(mode: ThemeMode): boolean {
  const dark = mode === "dark" || (mode === "system" && systemPrefersDark());
  const root = document.documentElement;
  root.classList.toggle("dark", dark);
  root.style.colorScheme = dark ? "dark" : "light";
  return dark;
}

interface AppState {
  // --- library data ---
  roots: RootRecord[];
  decks: DeckRecord[];
  stats: Stats;
  ready: boolean;

  // --- navigation / source ---
  nav: NavTarget;

  // --- search / browse ---
  query: string;
  filters: SearchFilters;
  results: SearchHit[];
  searching: boolean;
  grouping: Grouping;

  // --- selection ---
  selectedIds: Set<number>;
  anchorIndex: number | null;

  // --- peek ---
  peekIndex: number | null;

  // --- layout / panels ---
  sidebarCollapsed: boolean;
  inspectorVisible: boolean;
  commandOpen: boolean;
  filterPopoverOpen: boolean;
  exportOpen: boolean;
  aboutOpen: boolean;
  settingsOpen: boolean;
  gridCols: number;

  // --- theme ---
  theme: ThemeMode;
  dark: boolean;

  // --- scan ---
  scan: ScanState;

  // --- confirm dialog ---
  confirm: ConfirmConfig | null;

  // --- actions ---
  init: () => Promise<void>;
  reloadLibrary: () => Promise<void>;

  setQuery: (q: string) => void;
  setFilters: (patch: Partial<SearchFilters>) => void;
  clearFilters: () => void;
  setGrouping: (g: Grouping) => void;
  setNav: (nav: NavTarget) => Promise<void>;
  refresh: () => Promise<void>;

  // selection
  selectOnly: (index: number) => void;
  toggleSelect: (index: number) => void;
  rangeSelect: (index: number) => void;
  selectAll: () => void;
  clearSelection: () => void;
  moveSelection: (delta: number, cols: number) => void;
  addSelectionToTray: () => void;

  // peek
  openPeek: (index: number) => void;
  closePeek: () => void;
  peekBy: (delta: number) => void;

  // layout
  toggleSidebar: () => void;
  toggleInspector: () => void;
  setInspector: (v: boolean) => void;
  setCommandOpen: (v: boolean) => void;
  setFilterPopoverOpen: (v: boolean) => void;
  setExportOpen: (v: boolean) => void;
  setAboutOpen: (v: boolean) => void;
  setSettingsOpen: (v: boolean) => void;
  incCols: () => void;
  decCols: () => void;

  // theme
  setTheme: (t: ThemeMode) => void;
  cycleTheme: () => void;

  // favorites
  toggleFavoriteSlide: (slideId: number) => Promise<void>;
  toggleFavoriteDeck: (deckId: number) => Promise<void>;

  // scan
  startScan: () => Promise<void>;
  handleScanEvent: (ev: ScanEvent) => void;

  // folders
  addFolder: () => Promise<void>;
  removeRoot: (rootId: number) => Promise<void>;

  // confirm / destructive flows
  requestConfirm: (cfg: ConfirmConfig) => void;
  dismissConfirm: () => void;
  clearAndRebuild: () => Promise<void>;
  confirmClearAndRebuild: () => void;
}

let searchToken = 0;
let debounceTimer: number | undefined;
let recordTimer: number | undefined;

export const useApp = create<AppState>((set, get) => ({
  roots: [],
  decks: [],
  stats: { deck_count: 0, slide_count: 0 },
  ready: false,

  nav: { type: "all" },

  query: "",
  filters: {},
  results: [],
  searching: false,
  grouping: "flat",

  selectedIds: new Set(),
  anchorIndex: null,

  peekIndex: null,

  sidebarCollapsed: false,
  inspectorVisible: false,
  commandOpen: false,
  filterPopoverOpen: false,
  exportOpen: false,
  aboutOpen: false,
  settingsOpen: false,
  gridCols: (() => {
    const n = Number(localStorage.getItem(COLS_KEY));
    return Number.isFinite(n) && n >= 3 && n <= 10 ? n : 5;
  })(),

  theme: loadTheme(),
  dark: applyTheme(loadTheme()),

  scan: { running: false, done: 0, total: 0, indexed: 0, lastPath: null },

  confirm: null,

  // -------------------------------------------------------------------------

  init: async () => {
    await get().reloadLibrary();
    set({ ready: true });
    await get().refresh();
    // Kick a background rescan so the index is fresh on launch (no-op in mock).
    void get().startScan();
  },

  reloadLibrary: async () => {
    const [roots, decks, stats] = await Promise.all([
      api.listRoots(),
      api.getDecks(),
      api.getStats(),
    ]);
    set({ roots, decks, stats });
    useTray.getState().reconcile(decks);
  },

  setQuery: (q) => {
    set({ query: q });
    if (debounceTimer) window.clearTimeout(debounceTimer);
    if (q.trim() === "") {
      // Clear results immediately on empty query (brief: never linger).
      void get().refresh();
      return;
    }
    debounceTimer = window.setTimeout(() => {
      void get().refresh();
    }, 150);
  },

  setFilters: (patch) => {
    set({ filters: { ...get().filters, ...patch } });
    void get().refresh();
  },

  clearFilters: () => {
    set({ filters: {} });
    void get().refresh();
  },

  setGrouping: (g) => set({ grouping: g }),

  setNav: async (nav) => {
    set({ nav, query: "" });
    await get().refresh();
  },

  refresh: async () => {
    const token = ++searchToken;
    const { query, filters, nav, decks } = get();

    // The stats view fetches its own data; keep the grid empty behind it.
    if (nav.type === "stats") {
      set({ results: [], selectedIds: new Set(), anchorIndex: null, searching: false });
      return;
    }

    // Show the shimmer only if results are slow (>150ms) so fast queries never
    // flash a loader.
    let slow = false;
    const slowTimer = window.setTimeout(() => {
      slow = true;
      if (token === searchToken) set({ searching: true });
    }, 150);

    try {
      let hits: SearchHit[];

      // Effective filters: fold the active nav source into path_prefix.
      const eff: SearchFilters = { ...filters };
      if (nav.type === "root") {
        const root = get().roots.find((r) => r.id === nav.id);
        if (root) eff.path_prefix = root.path;
      }
      if (nav.type === "favorites") {
        eff.favorites_only = true;
      }

      if (nav.type === "deck" && nav.id != null && query.trim() === "") {
        // Deliberate deck browse: show the deck's slides in order.
        const deck = decks.find((d) => d.id === nav.id);
        const slides = await api.getDeckSlides(nav.id);
        hits = deck
          ? slides.map((slide) => ({
              slide,
              deck,
              snippet: (slide.body_text || slide.title || "").slice(0, 120),
              score: 0,
            }))
          : [];
      } else {
        hits = await api.search(query, eff);
      }

      if (token !== searchToken) return; // stale — a newer query superseded us.
      set({ results: hits, selectedIds: new Set(), anchorIndex: null });

      // Remember settled searches for the stats view: only after the user
      // pauses typing for a moment, so keystroke prefixes don't pile up.
      if (recordTimer) window.clearTimeout(recordTimer);
      const settled = query.trim();
      if (settled !== "") {
        const count = hits.length;
        recordTimer = window.setTimeout(() => {
          void api.recordSearch(settled, count).catch(() => {});
        }, 1200);
      }
    } catch (err) {
      if (token === searchToken) {
        toast.error(`Search failed: ${String(err)}`);
        set({ results: [] });
      }
    } finally {
      window.clearTimeout(slowTimer);
      if (token === searchToken && slow) set({ searching: false });
      if (token === searchToken) set({ searching: false });
    }
  },

  // --- selection ---------------------------------------------------------

  selectOnly: (index) => {
    const id = get().results[index]?.slide.id;
    if (id == null) return;
    set({ selectedIds: new Set([id]), anchorIndex: index });
  },

  toggleSelect: (index) => {
    const id = get().results[index]?.slide.id;
    if (id == null) return;
    const next = new Set(get().selectedIds);
    if (next.has(id)) next.delete(id);
    else next.add(id);
    set({ selectedIds: next, anchorIndex: index });
  },

  rangeSelect: (index) => {
    const { anchorIndex, results } = get();
    const from = anchorIndex ?? index;
    const [lo, hi] = from < index ? [from, index] : [index, from];
    const next = new Set<number>();
    for (let i = lo; i <= hi; i += 1) {
      const id = results[i]?.slide.id;
      if (id != null) next.add(id);
    }
    set({ selectedIds: next });
  },

  selectAll: () =>
    set({ selectedIds: new Set(get().results.map((r) => r.slide.id)) }),

  clearSelection: () => set({ selectedIds: new Set(), anchorIndex: null }),

  moveSelection: (delta, cols) => {
    const { results, anchorIndex } = get();
    if (results.length === 0) return;
    const step = delta === -2 ? -cols : delta === 2 ? cols : delta;
    const cur = anchorIndex ?? 0;
    const next = Math.max(0, Math.min(results.length - 1, cur + step));
    get().selectOnly(next);
  },

  addSelectionToTray: () => {
    const { selectedIds, results } = get();
    const entries = results
      .filter((r) => selectedIds.has(r.slide.id))
      .map((r) => ({ slide: r.slide, deck: r.deck }));
    if (entries.length === 0) return;
    const added = useTray.getState().add(entries);
    if (added > 0) {
      toast.success(
        added === 1 ? "Added 1 slide to the tray" : `Added ${added} slides to the tray`,
      );
      if (useTray.getState().collapsed) useTray.getState().setCollapsed(false);
    }
  },

  // --- peek --------------------------------------------------------------

  openPeek: (index) => set({ peekIndex: index }),
  closePeek: () => set({ peekIndex: null }),
  peekBy: (delta) => {
    const { peekIndex, results } = get();
    if (peekIndex == null) return;
    const next = Math.max(0, Math.min(results.length - 1, peekIndex + delta));
    set({ peekIndex: next });
    get().selectOnly(next);
  },

  // --- layout ------------------------------------------------------------

  toggleSidebar: () => set((s) => ({ sidebarCollapsed: !s.sidebarCollapsed })),
  toggleInspector: () => set((s) => ({ inspectorVisible: !s.inspectorVisible })),
  setInspector: (v) => set({ inspectorVisible: v }),
  setCommandOpen: (v) => set({ commandOpen: v }),
  setFilterPopoverOpen: (v) => set({ filterPopoverOpen: v }),
  setExportOpen: (v) => set({ exportOpen: v }),
  setAboutOpen: (v) => set({ aboutOpen: v }),
  setSettingsOpen: (v) => set({ settingsOpen: v }),

  incCols: () => {
    const n = Math.min(10, get().gridCols + 1);
    localStorage.setItem(COLS_KEY, String(n));
    set({ gridCols: n });
  },
  decCols: () => {
    const n = Math.max(3, get().gridCols - 1);
    localStorage.setItem(COLS_KEY, String(n));
    set({ gridCols: n });
  },

  // --- theme -------------------------------------------------------------

  setTheme: (t) => {
    localStorage.setItem(THEME_KEY, t);
    set({ theme: t, dark: applyTheme(t) });
  },
  cycleTheme: () => {
    const order: ThemeMode[] = ["system", "light", "dark"];
    const next = order[(order.indexOf(get().theme) + 1) % order.length];
    get().setTheme(next);
  },

  // --- favorites -----------------------------------------------------------

  toggleFavoriteSlide: async (slideId) => {
    try {
      const fav = await api.toggleFavoriteSlide(slideId);
      // Patch the visible results in place; drop the slide when un-starring
      // inside the Favorites view.
      const { results, nav } = get();
      const next = results
        .map((r) =>
          r.slide.id === slideId ? { ...r, slide: { ...r.slide, favorite: fav } } : r,
        )
        .filter((r) => !(nav.type === "favorites" && r.slide.id === slideId && !fav));
      set({ results: next });
      toast.success(fav ? "Added to Favorites" : "Removed from Favorites");
    } catch (err) {
      toast.error(`Couldn't update favorite: ${String(err)}`);
    }
  },

  toggleFavoriteDeck: async (deckId) => {
    try {
      const fav = await api.toggleFavoriteDeck(deckId);
      const patchDeck = (d: DeckRecord) => (d.id === deckId ? { ...d, favorite: fav } : d);
      set({
        decks: get().decks.map(patchDeck),
        results: get().results.map((r) => ({ ...r, deck: patchDeck(r.deck) })),
      });
      toast.success(fav ? "Deck added to Favorites" : "Deck removed from Favorites");
    } catch (err) {
      toast.error(`Couldn't update favorite: ${String(err)}`);
    }
  },

  // --- scan --------------------------------------------------------------

  startScan: async () => {
    if (get().scan.running) return;
    set({
      scan: { running: true, done: 0, total: 0, indexed: 0, lastPath: null },
    });
    try {
      const started = await api.startScan();
      if (!started) {
        set({ scan: { ...get().scan, running: false } });
      }
    } catch (err) {
      set({ scan: { ...get().scan, running: false } });
      toast.error(`Scan failed to start: ${String(err)}`);
    }
  },

  handleScanEvent: (ev) => {
    const scan = { ...get().scan };
    switch (ev.kind) {
      case "started":
        scan.running = true;
        scan.total = ev.total_files;
        scan.done = 0;
        scan.indexed = 0;
        scan.lastPath = null;
        break;
      case "deck":
        scan.done = ev.done;
        scan.total = ev.total;
        scan.indexed = ev.done;
        scan.lastPath = ev.path;
        break;
      case "skipped":
        scan.lastPath = ev.path;
        break;
      case "finished":
        scan.running = false;
        scan.indexed = ev.indexed;
        scan.lastPath = null;
        // Slide ids get recycled across reindexes; drop the session preview
        // cache if anything actually changed so no slide shows a stale preview.
        // Skip on no-op rescans so scrollback stays warm.
        if (ev.indexed > 0 || ev.removed > 0) clearSlideSvgCache();
        // Refresh library + current view now that the index changed.
        void get().reloadLibrary().then(() => get().refresh());
        break;
    }
    set({ scan });
  },

  // --- folders -----------------------------------------------------------

  addFolder: async () => {
    const path = await api.pickFolder();
    if (!path) return;
    try {
      await api.addRoot(path);
      toast.success("Folder added — indexing now");
      await get().reloadLibrary();
      await get().startScan();
    } catch (err) {
      toast.error(`Couldn't add folder: ${String(err)}`);
    }
  },

  removeRoot: async (rootId) => {
    try {
      await api.removeRoot(rootId);
      // Removing decks frees their slide ids for reuse by later scans; clear
      // the session preview cache so a reused id can't serve a stale preview.
      clearSlideSvgCache();
      if (get().nav.type === "root" && get().nav.id === rootId) {
        set({ nav: { type: "all" } });
      }
      await get().reloadLibrary();
      await get().refresh();
      toast.info("Folder removed");
    } catch (err) {
      toast.error(`Couldn't remove folder: ${String(err)}`);
    }
  },

  // --- confirm / destructive flows ---------------------------------------

  requestConfirm: (cfg) => set({ confirm: cfg }),
  dismissConfirm: () => set({ confirm: null }),

  clearAndRebuild: async () => {
    try {
      await api.clearIndex();
    } catch (err) {
      toast.error(`Couldn't clear the index: ${String(err)}`);
      return;
    }
    // Slide ids get recycled on reindex; drop the session preview cache so no
    // slide can show a stale preview from the wiped library.
    clearSlideSvgCache();
    await get().reloadLibrary();
    await get().refresh();
    toast.success("Index cleared — rebuilding now");
    await get().startScan();
  },

  confirmClearAndRebuild: () =>
    get().requestConfirm({
      title: "Clear index & rebuild?",
      message:
        "This clears the search index and preview cache, then re-scans your folders from scratch. Starred slides and decks are kept. Recent scan, search, and export history will be cleared, and tray slides show as moved until the rescan finishes.",
      confirmLabel: "Clear & rebuild",
      destructive: true,
      onConfirm: () => get().clearAndRebuild(),
    }),
}));
