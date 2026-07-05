// The composition tray(s) — the app's whole reason to exist. Trays are
// first-class, persistent objects: saved continuously to localStorage and
// restored on relaunch, with cmd-Z undo/redo (scoped PER tray) across
// add / remove / reorder.
//
// This store is a thin wrapper over the pure reducers in `lib/trayModel.ts`.
// The multi-tray state lives flattened here (`trays`/`order`/`activeId`), and a
// top-level `items` field MIRRORS the active tray so every existing consumer
// that only cares about "the tray" keeps working unchanged (selectors like
// `useTray(s => s.items)`, `add`, `remove`, `clear`, `picks`, `has`, undo/redo
// all operate on the active tray).

import { create } from "zustand";
import type { DeckRecord, SlidePick, SlideRecord } from "../lib/types";
import { toast } from "./useToast";
import * as tm from "../lib/trayModel";
import { uidFor, type Tray, type TrayItem, type TrayModel } from "../lib/trayModel";

export type { TrayItem };

/** One entry in the tray switcher. */
export interface TraySummary {
  id: string;
  name: string;
  count: number;
}

function freshId(): string {
  try {
    return crypto.randomUUID();
  } catch {
    return `t-${Date.now()}-${Math.random().toString(36).slice(2, 8)}`;
  }
}

function persist(model: TrayModel) {
  try {
    localStorage.setItem(tm.STORAGE_KEY_V2, JSON.stringify(tm.toPersisted(model)));
  } catch {
    /* storage full / disabled — non-fatal */
  }
}

function load(): TrayModel {
  try {
    const v1 = localStorage.getItem(tm.STORAGE_KEY_V1);
    const v2 = localStorage.getItem(tm.STORAGE_KEY_V2);
    return tm.migrate(v1, v2, freshId);
  } catch {
    return tm.emptyModel(freshId());
  }
}

interface TrayState {
  // Multi-tray model, flattened for zustand selectors.
  trays: Record<string, Tray>;
  order: string[];
  activeId: string;
  collapsed: boolean;
  /** Compatibility mirror: the ACTIVE tray's items, kept in sync on every op. */
  items: TrayItem[];

  setCollapsed: (v: boolean) => void;
  toggleCollapsed: () => void;

  /** Add slides to the active tray (ignoring duplicates). Returns how many were
   *  newly added. */
  add: (entries: { slide: SlideRecord; deck: DeckRecord }[], atIndex?: number) => number;
  remove: (uid: string) => void;
  removeAt: (index: number) => void;
  reorder: (nextItems: TrayItem[]) => void;
  clear: () => void;

  undo: () => void;
  redo: () => void;

  /** Mark items whose deck is missing from the freshly-indexed deck set, across
   *  ALL trays. */
  reconcile: (decks: DeckRecord[]) => void;

  picks: () => SlidePick[];
  has: (slide: SlideRecord) => boolean;

  // --- multiple named trays ------------------------------------------------
  /** Create a new (auto-named unless `name` given) tray and switch to it.
   *  Returns the new tray id. */
  createTray: (name?: string) => string;
  renameTray: (id: string, name: string) => void;
  deleteTray: (id: string) => void;
  switchTray: (id: string) => void;
  /** Trays in display order, with item counts, for the switcher UI. */
  trayList: () => TraySummary[];
}

export const useTray = create<TrayState>((set, get) => {
  const modelOf = (): TrayModel => ({
    trays: get().trays,
    order: get().order,
    activeId: get().activeId,
    collapsed: get().collapsed,
  });

  const applyModel = (model: TrayModel) => {
    persist(model);
    set({
      trays: model.trays,
      order: model.order,
      activeId: model.activeId,
      collapsed: model.collapsed,
      items: tm.activeItems(model),
    });
  };

  const initial = load();

  return {
    trays: initial.trays,
    order: initial.order,
    activeId: initial.activeId,
    collapsed: initial.collapsed,
    items: tm.activeItems(initial),

    setCollapsed: (v) => applyModel({ ...modelOf(), collapsed: v }),
    toggleCollapsed: () => applyModel({ ...modelOf(), collapsed: !get().collapsed }),

    add: (entries, atIndex) => {
      const model = modelOf();
      const cur = tm.activeItems(model);
      const existing = new Set(cur.map((i) => i.uid));
      const fresh: TrayItem[] = [];
      for (const { slide, deck } of entries) {
        const uid = uidFor(slide);
        if (existing.has(uid)) continue;
        existing.add(uid);
        fresh.push({ uid, slide, deck });
      }
      if (fresh.length === 0) return 0;
      let next: TrayItem[];
      if (atIndex == null || atIndex >= cur.length) {
        next = [...cur, ...fresh];
      } else {
        const at = Math.max(0, atIndex);
        next = [...cur.slice(0, at), ...fresh, ...cur.slice(at)];
      }
      applyModel(tm.commitItems(model, model.activeId, next));
      return fresh.length;
    },

    remove: (uid) => {
      const model = modelOf();
      const cur = tm.activeItems(model);
      const removed = cur.find((i) => i.uid === uid);
      const next = cur.filter((i) => i.uid !== uid);
      if (next.length === cur.length) return;
      applyModel(tm.commitItems(model, model.activeId, next));
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
      const model = modelOf();
      const cur = tm.activeItems(model);
      const same =
        cur.length === nextItems.length &&
        cur.every((it, i) => it.uid === nextItems[i].uid);
      if (same) return;
      applyModel(tm.commitItems(model, model.activeId, nextItems));
    },

    clear: () => {
      const model = modelOf();
      if (tm.activeItems(model).length === 0) return;
      applyModel(tm.commitItems(model, model.activeId, []));
    },

    undo: () => {
      const model = modelOf();
      const next = tm.undo(model, model.activeId);
      if (next !== model) applyModel(next);
    },
    redo: () => {
      const model = modelOf();
      const next = tm.redo(model, model.activeId);
      if (next !== model) applyModel(next);
    },

    reconcile: (decks) => {
      const model = modelOf();
      const next = tm.reconcile(model, decks);
      if (next !== model) applyModel(next);
    },

    picks: () => tm.picksOf(get().items),

    has: (slide) => {
      const uid = uidFor(slide);
      return get().items.some((i) => i.uid === uid);
    },

    createTray: (name) => {
      const id = freshId();
      applyModel(tm.createTray(modelOf(), id, name));
      return id;
    },

    renameTray: (id, name) => {
      const model = modelOf();
      const next = tm.renameTray(model, id, name);
      if (next !== model) applyModel(next);
    },

    deleteTray: (id) => {
      const model = modelOf();
      const next = tm.deleteTray(model, id, freshId);
      if (next !== model) applyModel(next);
    },

    switchTray: (id) => {
      const model = modelOf();
      const next = tm.switchTray(model, id);
      if (next !== model) applyModel(next);
    },

    trayList: () =>
      get().order.map((id) => {
        const t = get().trays[id];
        return { id, name: t.name, count: t.items.length };
      }),
  };
});

export { uidFor };
