import type { TrayItem } from "../stores/useTray";

/** Slide dimensions may be absent on trays persisted before the dims fields
 *  existed (the DeckRecord is embedded verbatim in localStorage). The TS type
 *  says `number`, so this runtime guard is deliberate. */
function deckDims(item: TrayItem): { w: number; h: number } | null {
  const w = item.deck.slide_width_emu;
  const h = item.deck.slide_height_emu;
  if (typeof w !== "number" || typeof h !== "number" || w <= 0 || h <= 0) return null;
  return { w, h };
}

/**
 * uids of tray items whose deck slide size differs from the FIRST pick's.
 * The composer adopts the first source deck's canvas (composer.rs takes deck
 * index 0), so the first item is the reference and is never flagged. Derived
 * from the current order → a reorder recomputes the reference for free.
 * Exact w/h comparison (not aspect ratio) to match composer.rs's warn.
 * Unknown dims are never flagged; if the first item's dims are unknown there is
 * no reference and nothing is flagged.
 */
export function mismatchedDimUids(items: TrayItem[]): Set<string> {
  const out = new Set<string>();
  if (items.length < 2) return out;
  const ref = deckDims(items[0]);
  if (!ref) return out;
  for (let i = 1; i < items.length; i += 1) {
    const d = deckDims(items[i]);
    if (d && (d.w !== ref.w || d.h !== ref.h)) out.add(items[i].uid);
  }
  return out;
}
