// Drag-and-drop helpers shared by the grid (drag source) and the tray (drop
// target). Payloads travel as JSON on a private MIME type.

import type { DeckRecord, SearchHit, SlideRecord } from "./types";

export const DRAG_MIME = "application/x-slideflow-slides";

export interface DragEntry {
  slide: SlideRecord;
  deck: DeckRecord;
}

/** Collect the currently-selected hits (in visible order) as drag entries. */
export function buildDragEntries(
  results: SearchHit[],
  selected: Set<number>,
): DragEntry[] {
  return results
    .filter((r) => selected.has(r.slide.id))
    .map((r) => ({ slide: r.slide, deck: r.deck }));
}

/** Parse a drop payload back into entries; returns [] on anything unexpected. */
export function parseDropEntries(dt: DataTransfer): DragEntry[] {
  try {
    const raw = dt.getData(DRAG_MIME);
    if (!raw) return [];
    const parsed = JSON.parse(raw);
    return Array.isArray(parsed) ? (parsed as DragEntry[]) : [];
  } catch {
    return [];
  }
}

/**
 * Build a real stacked-slides drag ghost with a count badge, positioned
 * off-screen so the browser can snapshot it for `setDragImage`.
 */
export function makeDragGhost(count: number): HTMLElement {
  const wrap = document.createElement("div");
  wrap.style.cssText =
    "position:fixed;top:-1000px;left:-1000px;width:112px;height:64px;pointer-events:none;";

  for (let i = Math.min(count, 3) - 1; i >= 0; i -= 1) {
    const card = document.createElement("div");
    card.style.cssText = `position:absolute;left:${i * 6}px;top:${
      i * 6
    }px;width:96px;height:54px;border-radius:6px;background:#fff;box-shadow:0 4px 12px rgba(0,0,0,.28);border:1px solid rgba(0,0,0,.08);`;
    wrap.appendChild(card);
  }

  if (count > 1) {
    const badge = document.createElement("div");
    badge.textContent = String(count);
    badge.style.cssText =
      "position:absolute;top:-8px;right:-8px;min-width:22px;height:22px;padding:0 6px;border-radius:11px;background:#0A84FF;color:#fff;font:600 12px -apple-system,system-ui,sans-serif;display:flex;align-items:center;justify-content:center;box-shadow:0 2px 6px rgba(0,0,0,.3);";
    wrap.appendChild(badge);
  }

  document.body.appendChild(wrap);
  return wrap;
}
