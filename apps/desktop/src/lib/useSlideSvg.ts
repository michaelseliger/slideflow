// Lazy, cached slide-SVG loader. Thumbnails are fetched once per slide id and
// memoized in a module-level cache so scrolling back never re-renders on the
// Rust side. Concurrent decodes are naturally capped by React only mounting
// the visible (virtualized) tiles.

import { useEffect, useState } from "react";
import { getSlideSvg } from "./api";
import { svgToDataUri } from "./utils";

const cache = new Map<number, string>();
const inflight = new Map<number, Promise<string>>();

async function load(slideId: number): Promise<string> {
  const hit = cache.get(slideId);
  if (hit) return hit;
  let p = inflight.get(slideId);
  if (!p) {
    p = getSlideSvg(slideId)
      .then((svg) => {
        const uri = svgToDataUri(svg);
        cache.set(slideId, uri);
        inflight.delete(slideId);
        return uri;
      })
      .catch((err) => {
        inflight.delete(slideId);
        throw err;
      });
    inflight.set(slideId, p);
  }
  return p;
}

/** Returns a data-URI for the slide's SVG, or null while loading. */
export function useSlideSvg(slideId: number | null | undefined, enabled = true) {
  const [uri, setUri] = useState<string | null>(
    slideId != null ? cache.get(slideId) ?? null : null,
  );

  useEffect(() => {
    if (!enabled || slideId == null) return;
    const cached = cache.get(slideId);
    if (cached) {
      setUri(cached);
      return;
    }
    let alive = true;
    setUri(null);
    load(slideId)
      .then((u) => {
        if (alive) setUri(u);
      })
      .catch(() => {
        if (alive) setUri(null);
      });
    return () => {
      alive = false;
    };
  }, [slideId, enabled]);

  return uri;
}

/** The raw SVG string (not a data URI) — used by the peek modal / inspector for
 *  crisp inline rendering. */
export function useRawSlideSvg(slideId: number | null | undefined) {
  const [svg, setSvg] = useState<string | null>(null);
  useEffect(() => {
    if (slideId == null) {
      setSvg(null);
      return;
    }
    let alive = true;
    setSvg(null);
    getSlideSvg(slideId)
      .then((s) => alive && setSvg(s))
      .catch(() => alive && setSvg(null));
    return () => {
      alive = false;
    };
  }, [slideId]);
  return svg;
}
