// Lazy, cached slide-preview loader. Each (slide, tier) pair is resolved once
// to a ready-to-use `<img src>` string plus the set of dropped constructs (see
// `getSlidePreview`) — an `asset:` URL in the native app, a data URI in
// browser-mock mode — and memoized so scrolling back never re-hits the backend.
// The cached values are short URL strings (not multi-MB SVG data), so the map
// stays tiny even for a large library. Concurrent loads are capped naturally by
// React only mounting the visible (virtualized) tiles.

import { useEffect, useState, useSyncExternalStore } from "react";
import { getSlidePreview, type PreviewTier } from "./api";

type Preview = { src: string; dropped: string[] };

let generation = 0;
const cache = new Map<string, Preview>();
const inflight = new Map<string, Promise<Preview>>();
const genListeners = new Set<() => void>();

function keyOf(slideId: number, tier: PreviewTier): string {
  return `${slideId}:${tier}`;
}

/** Drop every memoized preview. Call whenever the library changes (a folder is
 *  removed, a rescan finishes) or the fonts change (the host wiped the on-disk
 *  thumb cache) — slide ids are recycled after deletes, so a cache keyed by id
 *  would otherwise hand a new slide the previous slide's preview. Bumping the
 *  generation makes in-flight loads that settle *after* the clear discard their
 *  result instead of repopulating the cleared cache, and notifies mounted
 *  previews (via `useCacheGeneration`) so they refetch instead of showing the
 *  now-cleared old render / a dead `asset:` URL until remount. */
export function clearSlideSvgCache() {
  generation += 1;
  cache.clear();
  inflight.clear();
  for (const notify of genListeners) notify();
}

function subscribeGeneration(cb: () => void): () => void {
  genListeners.add(cb);
  return () => genListeners.delete(cb);
}

/** The current cache generation. A component reading it via useSyncExternalStore
 *  re-renders on every `clearSlideSvgCache`, so including it in a preview effect's
 *  deps makes mounted tiles refetch after a cache clear. */
function useCacheGeneration(): number {
  return useSyncExternalStore(subscribeGeneration, () => generation);
}

async function load(slideId: number, tier: PreviewTier): Promise<Preview> {
  const key = keyOf(slideId, tier);
  const hit = cache.get(key);
  if (hit) return hit;
  let p = inflight.get(key);
  if (!p) {
    const gen = generation;
    p = getSlidePreview(slideId, tier)
      .then((preview) => {
        if (gen === generation) {
          cache.set(key, preview);
          inflight.delete(key);
        }
        return preview;
      })
      .catch((err) => {
        if (gen === generation) inflight.delete(key);
        throw err;
      });
    inflight.set(key, p);
  }
  return p;
}

/** Returns a slide's preview `{ src, dropped }` for the given tier. `src` is an
 *  `<img src>` (null while it loads or on error); `dropped` lists the unsupported
 *  construct kinds the renderer skipped. `enabled=false` defers the load
 *  (off-viewport tiles). */
export function useSlidePreview(
  slideId: number | null | undefined,
  tier: PreviewTier = "thumb",
  enabled = true,
): { src: string | null; dropped: string[] } {
  const generation = useCacheGeneration();
  const [state, setState] = useState<{ src: string | null; dropped: string[] }>(() =>
    slideId != null ? cache.get(keyOf(slideId, tier)) ?? { src: null, dropped: [] } : { src: null, dropped: [] },
  );

  useEffect(() => {
    if (!enabled || slideId == null) return;
    const cached = cache.get(keyOf(slideId, tier));
    if (cached) {
      setState(cached);
      return;
    }
    let alive = true;
    setState({ src: null, dropped: [] });
    load(slideId, tier)
      .then((preview) => {
        if (alive) setState(preview);
      })
      .catch(() => {
        if (alive) setState({ src: null, dropped: [] });
      });
    return () => {
      alive = false;
    };
    // `generation` bumps on clearSlideSvgCache: re-run so a mounted tile refetches
    // after a font change / rescan wiped the cache instead of showing stale bytes.
  }, [slideId, tier, enabled, generation]);

  return state;
}
