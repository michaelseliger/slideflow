// The composition tray — the app's whole reason to exist. It is a first-class,
// persistent object: saved continuously to localStorage and restored on
// relaunch, with cmd-Z undo/redo across add / remove / reorder.

import { create } from "zustand";
import type { DeckRecord, SlidePick, SlideRecord } from "../lib/types";
import { toast } from "./useToast";

export interface TrayItem {
  /** Stable id for reorder keys & dedupe (deck_id:slide_index). */
  uid: string;
  slide: SlideRecord;
  deck: DeckRecord;
  /** Set when the source deck appears to have moved/changed since it was added. */
  moved?: boolean;
}

const STORAGE_KEY = "slideflow.tray.v1";
const HISTORY_LIMIT = 100;

function uidFor(slide: SlideRecord): string {
  return `${slide.deck_id}:${slide.slide_index}`;
}

function persist(items: TrayItem[]) {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(items));
  } catch {
    /* storage full / disabled — non-fatal */
  }
}

function load(): TrayItem[] {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as TrayItem[]) : [];
  } catch {
    return [];
  }
}

interface TrayState {
  items: TrayItem[];
  collapsed: boolean;
  /** Undo/redo snapshots of the full item list. */
  past: TrayItem[][];
  future: TrayItem[][];

  setCollapsed: (v: boolean) => void;
  toggleCollapsed: () => void;

  /** Add slides (ignoring duplicates). Returns how many were newly added. */
  add: (entries: { slide: SlideRecord; deck: DeckRecord }[], atIndex?: number) => number;
  remove: (uid: string) => void;
  removeAt: (index: number) => void;
  reorder: (nextItems: TrayItem[]) => void;
  clear: () => void;

  undo: () => void;
  redo: () => void;

  /** Mark items whose deck is missing from the freshly-indexed deck set. */
  reconcile: (decks: DeckRecord[]) => void;

  picks: () => SlidePick[];
  has: (slide: SlideRecord) => boolean;
}

function commit(
  set: (partial: Partial<TrayState>) => void,
  get: () => TrayState,
  next: TrayItem[],
) {
  const prev = get().items;
  const past = [...get().past, prev].slice(-HISTORY_LIMIT);
  set({ items: next, past, future: [] });
  persist(next);
}

export const useTray = create<TrayState>((set, get) => ({
  items: load(),
  collapsed: false,
  past: [],
  future: [],

  setCollapsed: (v) => set({ collapsed: v }),
  toggleCollapsed: () => set((s) => ({ collapsed: !s.collapsed })),

  add: (entries, atIndex) => {
    const existing = new Set(get().items.map((i) => i.uid));
    const fresh: TrayItem[] = [];
    for (const { slide, deck } of entries) {
      const uid = uidFor(slide);
      if (existing.has(uid)) continue;
      existing.add(uid);
      fresh.push({ uid, slide, deck });
    }
    if (fresh.length === 0) return 0;
    const cur = get().items;
    let next: TrayItem[];
    if (atIndex == null || atIndex >= cur.length) {
      next = [...cur, ...fresh];
    } else {
      const at = Math.max(0, atIndex);
      next = [...cur.slice(0, at), ...fresh, ...cur.slice(at)];
    }
    commit(set, get, next);
    return fresh.length;
  },

  remove: (uid) => {
    const cur = get().items;
    const removed = cur.find((i) => i.uid === uid);
    const next = cur.filter((i) => i.uid !== uid);
    if (next.length === cur.length) return;
    commit(set, get, next);
    if (removed) {
      toast.info("Removed from tray", {
        label: "Undo",
        run: () => get().undo(),
      });
    }
  },

  removeAt: (index) => {
    const cur = get().items;
    if (index < 0 || index >= cur.length) return;
    get().remove(cur[index].uid);
  },

  reorder: (nextItems) => {
    // Only commit if the order actually changed.
    const cur = get().items;
    const same =
      cur.length === nextItems.length &&
      cur.every((it, i) => it.uid === nextItems[i].uid);
    if (same) return;
    commit(set, get, nextItems);
  },

  clear: () => {
    if (get().items.length === 0) return;
    commit(set, get, []);
  },

  undo: () => {
    const { past, items, future } = get();
    if (past.length === 0) return;
    const previous = past[past.length - 1];
    set({
      items: previous,
      past: past.slice(0, -1),
      future: [items, ...future].slice(0, HISTORY_LIMIT),
    });
    persist(previous);
  },

  redo: () => {
    const { future, items, past } = get();
    if (future.length === 0) return;
    const next = future[0];
    set({
      items: next,
      future: future.slice(1),
      past: [...past, items].slice(-HISTORY_LIMIT),
    });
    persist(next);
  },

  reconcile: (decks) => {
    const byId = new Map(decks.map((d) => [d.id, d]));
    const byPath = new Map(decks.map((d) => [d.path, d]));
    let changed = false;
    const next = get().items.map((it) => {
      // The deck is considered present if we can still find it by id OR path.
      const present = byId.has(it.deck.id) || byPath.has(it.deck.path);
      const moved = !present;
      if (moved !== !!it.moved) changed = true;
      return moved === !!it.moved ? it : { ...it, moved };
    });
    if (changed) {
      set({ items: next });
      persist(next);
    }
  },

  picks: () =>
    get().items.map((it) => ({
      pptx_path: it.deck.path,
      slide_index: it.slide.slide_index,
    })),

  has: (slide) => {
    const uid = uidFor(slide);
    return get().items.some((i) => i.uid === uid);
  },
}));

export { uidFor };
