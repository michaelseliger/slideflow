// Pure, dependency-free data model for the composition tray(s).
//
// Extracted from `stores/useTray.ts` so the multi-tray persistence migration,
// per-tray undo/redo isolation, and delete-active-tray behaviour are unit
// testable without a browser, zustand, or a JS test runner. See
// `trayModel.test.ts`, runnable with `node --experimental-strip-types`.
//
// Every exported function is a pure reducer: it takes a `TrayModel` and returns
// a new one (or the same reference unchanged), never touching localStorage or
// zustand. The store in `useTray.ts` is a thin wrapper that persists the result.

import type { DeckRecord, SlidePick, SlideRecord } from "./types";

export interface TrayItem {
  /** Stable id for reorder keys & dedupe (deck_id:slide_index). */
  uid: string;
  slide: SlideRecord;
  deck: DeckRecord;
  /** Set when the source deck appears to have moved/changed since it was added. */
  moved?: boolean;
}

/** A single named tray with its own bounded undo/redo history. */
export interface Tray {
  id: string;
  name: string;
  items: TrayItem[];
  /** Per-tray undo snapshots (never persisted). */
  past: TrayItem[][];
  /** Per-tray redo snapshots (never persisted). */
  future: TrayItem[][];
}

/** The whole multi-tray state: a set of trays, their display order, which one
 *  is active, and the (global) collapsed flag. */
export interface TrayModel {
  trays: Record<string, Tray>;
  order: string[];
  activeId: string;
  collapsed: boolean;
}

export const HISTORY_LIMIT = 100;
export const STORAGE_KEY_V1 = "slideflow.tray.v1";
export const STORAGE_KEY_V2 = "slideflow.tray.v2";
/** Payload version stored inside the v2 blob, so future migrations have a hook
 *  independent of the localStorage key name. */
export const TRAY_SCHEMA_VERSION = 2;

/** Persisted per-tray shape — history stacks are intentionally dropped. */
export interface PersistedTray {
  id: string;
  name: string;
  items: TrayItem[];
}

/** Persisted whole-model shape written under `STORAGE_KEY_V2`. */
export interface PersistedModel {
  version: number;
  trays: PersistedTray[];
  order: string[];
  activeId: string;
  collapsed: boolean;
}

/** A tray item's durable identity: the source deck's path plus the slide index.
 *  Keyed on `path` (never the SQLite rowid, which is recycled across reindexes)
 *  so tray membership survives Clear & Rebuild / root removal — mirroring the
 *  repo-wide "favorites are keyed by path" convention. */
export function uidFor(
  deck: Pick<DeckRecord, "path">,
  slide: Pick<SlideRecord, "slide_index">,
): string {
  return `${deck.path}:${slide.slide_index}`;
}

/** Recompute every item's uid from its durable `deck.path`, so trays persisted
 *  before the path-based uid change (uids were `${deck_id}:${slide_index}`)
 *  keep working for dedupe/reconcile after load. Malformed items are dropped. */
function reuidItems(items: unknown): TrayItem[] {
  if (!Array.isArray(items)) return [];
  return items
    .filter((it): it is TrayItem => !!it && !!it.deck && !!it.slide)
    .map((it) => ({ ...it, uid: uidFor(it.deck, it.slide) }));
}

function newTray(id: string, name: string, items: TrayItem[] = []): Tray {
  return { id, name, items, past: [], future: [] };
}

/** A brand-new model holding a single empty "Tray 1". */
export function emptyModel(id: string): TrayModel {
  return { trays: { [id]: newTray(id, "Tray 1") }, order: [id], activeId: id, collapsed: false };
}

/** The active tray's items, or `[]` if the active id is somehow dangling. */
export function activeItems(model: TrayModel): TrayItem[] {
  return model.trays[model.activeId]?.items ?? [];
}

export function picksOf(items: TrayItem[]): SlidePick[] {
  return items.map((it) => ({ pptx_path: it.deck.path, slide_index: it.slide.slide_index }));
}

/** Auto name for a fresh tray: "Tray N" with the lowest N not already taken,
 *  starting at `order.length + 1` (so the second tray defaults to "Tray 2"). */
export function autoTrayName(model: TrayModel): string {
  const taken = new Set(model.order.map((id) => model.trays[id]?.name));
  let n = model.order.length + 1;
  while (taken.has(`Tray ${n}`)) n += 1;
  return `Tray ${n}`;
}

// --- persistence ------------------------------------------------------------

function parseV1(raw: string | null): TrayItem[] {
  if (!raw) return [];
  try {
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as TrayItem[]) : [];
  } catch {
    return [];
  }
}

function parseV2(raw: string | null): PersistedModel | null {
  if (!raw) return null;
  try {
    const p = JSON.parse(raw) as PersistedModel;
    if (!p || !Array.isArray(p.trays) || !Array.isArray(p.order)) return null;
    return p;
  } catch {
    return null;
  }
}

/** Rebuild a live model from a persisted blob, tolerating partial corruption. */
function fromPersisted(p: PersistedModel): TrayModel | null {
  const trays: Record<string, Tray> = {};
  for (const t of p.trays) {
    if (!t || typeof t.id !== "string") continue;
    trays[t.id] = newTray(t.id, t.name || "Tray", reuidItems(t.items));
  }
  const order = p.order.filter((id) => trays[id]);
  // Defensively include any trays missing from `order`.
  for (const id of Object.keys(trays)) if (!order.includes(id)) order.push(id);
  if (order.length === 0) return null;
  const activeId = trays[p.activeId] ? p.activeId : order[0];
  return { trays, order, activeId, collapsed: !!p.collapsed };
}

export function toPersisted(model: TrayModel): PersistedModel {
  return {
    version: TRAY_SCHEMA_VERSION,
    trays: model.order
      .map((id) => model.trays[id])
      .filter((t): t is Tray => !!t)
      .map((t) => ({ id: t.id, name: t.name, items: t.items })),
    order: [...model.order],
    activeId: model.activeId,
    collapsed: model.collapsed,
  };
}

/** Resolve the initial model from raw localStorage values. Prefers a valid v2
 *  blob; otherwise migrates a v1 single-tray payload losslessly into "Tray 1"
 *  (the old v1 key is left in place — harmless). `freshId` mints a tray id. */
export function migrate(
  v1Raw: string | null,
  v2Raw: string | null,
  freshId: () => string,
): TrayModel {
  const v2 = parseV2(v2Raw);
  if (v2) {
    const m = fromPersisted(v2);
    if (m) return m;
  }
  const v1Items = parseV1(v1Raw);
  const id = freshId();
  const model = emptyModel(id);
  model.trays[id] = newTray(id, "Tray 1", reuidItems(v1Items));
  return model;
}

// --- reducers ---------------------------------------------------------------

function withTray(model: TrayModel, id: string, tray: Tray): TrayModel {
  return { ...model, trays: { ...model.trays, [id]: tray } };
}

/** Commit new items to a tray, pushing prior items onto its bounded undo stack
 *  and clearing its redo stack. Returns `model` unchanged if the tray is gone. */
export function commitItems(model: TrayModel, trayId: string, next: TrayItem[]): TrayModel {
  const t = model.trays[trayId];
  if (!t) return model;
  const past = [...t.past, t.items].slice(-HISTORY_LIMIT);
  return withTray(model, trayId, { ...t, items: next, past, future: [] });
}

export function undo(model: TrayModel, trayId: string): TrayModel {
  const t = model.trays[trayId];
  if (!t || t.past.length === 0) return model;
  const previous = t.past[t.past.length - 1];
  return withTray(model, trayId, {
    ...t,
    items: previous,
    past: t.past.slice(0, -1),
    future: [t.items, ...t.future].slice(0, HISTORY_LIMIT),
  });
}

export function redo(model: TrayModel, trayId: string): TrayModel {
  const t = model.trays[trayId];
  if (!t || t.future.length === 0) return model;
  const nextItems = t.future[0];
  return withTray(model, trayId, {
    ...t,
    items: nextItems,
    future: t.future.slice(1),
    past: [...t.past, t.items].slice(-HISTORY_LIMIT),
  });
}

export function switchTray(model: TrayModel, id: string): TrayModel {
  if (!model.trays[id] || id === model.activeId) return model;
  return { ...model, activeId: id };
}

/** Add a new tray (with `id`) and make it active. `name` falls back to the next
 *  auto name. No-op if the id already exists. */
export function createTray(model: TrayModel, id: string, name?: string): TrayModel {
  if (model.trays[id]) return model;
  const trayName = name?.trim() || autoTrayName(model);
  return {
    ...model,
    trays: { ...model.trays, [id]: newTray(id, trayName) },
    order: [...model.order, id],
    activeId: id,
  };
}

export function renameTray(model: TrayModel, id: string, name: string): TrayModel {
  const t = model.trays[id];
  const trimmed = name.trim();
  if (!t || trimmed === "" || trimmed === t.name) return model;
  return withTray(model, id, { ...t, name: trimmed });
}

/** Delete a tray. Deleting the last one resets to a single empty "Tray 1"
 *  (minted via `freshId`); deleting the active one switches to a neighbour. */
export function deleteTray(model: TrayModel, id: string, freshId: () => string): TrayModel {
  if (!model.trays[id]) return model;
  const idx = model.order.indexOf(id);
  const order = model.order.filter((x) => x !== id);
  const trays = { ...model.trays };
  delete trays[id];
  if (order.length === 0) {
    return { ...emptyModel(freshId()), collapsed: model.collapsed };
  }
  let activeId = model.activeId;
  if (id === model.activeId) {
    const nextIdx = Math.min(idx, order.length - 1);
    activeId = order[Math.max(0, nextIdx)];
  }
  return { ...model, trays, order, activeId };
}

/** Re-check every tray's items against the freshly-indexed deck set, flagging
 *  (or clearing) `moved`. Returns `model` unchanged if nothing flipped. */
export function reconcile(
  model: TrayModel,
  decks: Pick<DeckRecord, "id" | "path">[],
): TrayModel {
  // Presence is judged by the durable identity (path) only — never the SQLite
  // rowid, which is recycled after DELETE (Clear & Rebuild / root removal), so a
  // stale item's `deck.id` can collide with an unrelated freshly-indexed deck
  // and wrongly mask the "moved" flag. This mirrors the repo-wide "favorites are
  // keyed by path" convention.
  const byPath = new Set(decks.map((d) => d.path));
  let changed = false;
  const trays: Record<string, Tray> = { ...model.trays };
  for (const id of model.order) {
    const t = model.trays[id];
    if (!t) continue;
    let trayChanged = false;
    const items = t.items.map((it) => {
      const present = byPath.has(it.deck.path);
      const moved = !present;
      if (moved !== !!it.moved) {
        trayChanged = true;
        return { ...it, moved };
      }
      return it;
    });
    if (trayChanged) {
      trays[id] = { ...t, items };
      changed = true;
    }
  }
  return changed ? { ...model, trays } : model;
}
