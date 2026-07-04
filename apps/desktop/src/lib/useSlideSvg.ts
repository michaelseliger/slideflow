// Lazy, cached slide-preview loader. Each (slide, tier) pair is resolved once
// to a ready-to-use `<img src>` string — an `asset:` URL in the native app, a
// data URI in browser-mock mode — and memoized so scrolling back never re-hits
// the backend. The cached values are short URL strings (not multi-MB SVG data),
// so the map stays tiny even for a large library. Concurrent loads are capped
// naturally by React only mounting the visible (virtualized) tiles.

import { useEffect, useState } from "react";
import { getSlidePreviewSrc, type PreviewTier } from "./api";

let generation = 0;
const cache = new Map<string, string>();
const inflight = new Map<string, Promise<string>>();

function keyOf(slideId: number, tier: PreviewTier): string {
  return `${slideId}:${tier}`;
}

/** Drop every memoized preview. Call whenever the library changes (a folder is
 *  removed, a rescan finishes) — slide ids are recycled after deletes, so a
 *  cache keyed by id would otherwise hand a new slide the previous slide's
 *  preview. Bumping the generation makes in-flight loads that settle *after* the
 *  clear discard their result instead of repopulating the cleared cache. */
export function clearSlideSvgCache() {
  generation += 1;
  cache.clear();
  inflight.clear();
}

async function load(slideId: number, tier: PreviewTier): Promise<string> {
  const key = keyOf(slideId, tier);
  const hit = cache.get(key);
  if (hit) return hit;
  let p = inflight.get(key);
  if (!p) {
    const gen = generation;
    p = getSlidePreviewSrc(slideId, tier)
      .then((src) => {
        if (gen === generation) {
          cache.set(key, src);
          inflight.delete(key);
        }
        return src;
      })
      .catch((err) => {
        if (gen === generation) inflight.delete(key);
        throw err;
      });
    inflight.set(key, p);
  }
  return p;
}

/** Returns an `<img src>` for the slide's preview at the given tier, or null
 *  while it loads. `enabled=false` defers the load (off-viewport tiles). */
export function useSlidePreview(
  slideId: number | null | undefined,
  tier: PreviewTier = "thumb",
  enabled = true,
): string | null {
  const [src, setSrc] = useState<string | null>(
    slideId != null ? cache.get(keyOf(slideId, tier)) ?? null : null,
  );

  useEffect(() => {
    if (!enabled || slideId == null) return;
    const cached = cache.get(keyOf(slideId, tier));
    if (cached) {
      setSrc(cached);
      return;
    }
    let alive = true;
    setSrc(null);
    load(slideId, tier)
      .then((s) => {
        if (alive) setSrc(s);
      })
      .catch(() => {
        if (alive) setSrc(null);
      });
    return () => {
      alive = false;
    };
  }, [slideId, tier, enabled]);

  return src;
}
