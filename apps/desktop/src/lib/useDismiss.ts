import { useEffect, useRef, type RefObject } from "react";

interface DismissOptions {
  /** When false, no listeners are installed. Menus gate this on their `open`
   *  state; lifetime-mounted popovers omit it (defaults to true). */
  enabled?: boolean;
  /** Also dismiss on a capture-phase scroll anywhere (ContextMenu). */
  onScroll?: boolean;
}

/**
 * Shared outside-click + Escape dismissal for menus and popovers.
 *
 * Listeners are registered in the **capture** phase so they run before
 * App.tsx's bubble-phase global keydown handler. Crucially, the Escape branch
 * calls `stopPropagation()` — otherwise the same Escape that closes a popup
 * leaks to App, whose Escape branch would clear the active search query (or
 * close the inspector) behind the just-dismissed popup.
 *
 * `onClose` is read through a ref so callers can pass fresh inline closures
 * without re-registering the listeners on every render.
 */
export function useDismiss(
  ref: RefObject<HTMLElement | null>,
  onClose: () => void,
  opts: DismissOptions = {},
): void {
  const { enabled = true, onScroll = false } = opts;
  const onCloseRef = useRef(onClose);
  onCloseRef.current = onClose;

  useEffect(() => {
    if (!enabled) return;
    const close = () => onCloseRef.current();
    const onDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) close();
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") {
        // Swallow the Escape we consumed so it can't leak to App's global
        // keydown handler (which would wipe the search query).
        e.stopPropagation();
        close();
      }
    };
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey, true);
    if (onScroll) window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey, true);
      if (onScroll) window.removeEventListener("scroll", close, true);
    };
  }, [enabled, onScroll, ref]);
}
