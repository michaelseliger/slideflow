// Small pure helpers shared across the UI.

import type React from "react";

/** Join class names, dropping falsy values. */
export function cx(...parts: Array<string | false | null | undefined>): string {
  return parts.filter(Boolean).join(" ");
}

/** Encode an SVG string as a data URI safe for `<img src>` (handles unicode). */
export function svgToDataUri(svg: string): string {
  // Prefer base64 so `#`, `%`, quotes etc. never need escaping.
  const bytes = new TextEncoder().encode(svg);
  let binary = "";
  for (let i = 0; i < bytes.length; i += 1) {
    binary += String.fromCharCode(bytes[i]);
  }
  return `data:image/svg+xml;base64,${btoa(binary)}`;
}

/** Basename of a POSIX-ish path. */
export function basename(path: string): string {
  const parts = path.split(/[\\/]/).filter(Boolean);
  return parts[parts.length - 1] ?? path;
}

/** Directory name of a POSIX-ish path. */
export function dirname(path: string): string {
  const idx = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
  return idx > 0 ? path.slice(0, idx) : path;
}

/**
 * Display name for a deck: the real file name (without extension). Deliberately
 * NOT the docProps/core.xml title — generators write junk there ("PptxGenJS
 * Presentation"), while the file name is what users recognize.
 */
export function deckDisplayName(deck: { file_name: string; path: string }): string {
  const name = deck.file_name || basename(deck.path);
  return name.replace(/\.pptx$/i, "");
}

/** Human file size. */
export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB"];
  let n = bytes / 1024;
  let u = 0;
  while (n >= 1024 && u < units.length - 1) {
    n /= 1024;
    u += 1;
  }
  return `${n.toFixed(n < 10 ? 1 : 0)} ${units[u]}`;
}

/** Relative "modified" label from a unix-seconds timestamp. */
export function formatModified(unix: number): string {
  const then = unix * 1000;
  const diff = Date.now() - then;
  const day = 86_400_000;
  if (diff < day) return "Today";
  if (diff < 2 * day) return "Yesterday";
  if (diff < 7 * day) return `${Math.floor(diff / day)} days ago`;
  return new Date(then).toLocaleDateString(undefined, {
    year: "numeric",
    month: "short",
    day: "numeric",
  });
}

/** Strip `<mark>` tags to plain text (for aria-labels / titles). */
export function stripMarks(html: string): string {
  return html.replace(/<\/?mark>/g, "");
}

/** Whether the user prefers reduced motion right now. A pref in Settings →
 *  Appearance can force it on top of the OS `prefers-reduced-motion` setting. */
export function prefersReducedMotion(): boolean {
  if (typeof window === "undefined") return false;
  if (window.localStorage?.getItem("slideflow.reduceMotion.v1") === "1") return true;
  return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches === true;
}

/** Detect a Mac-like platform for shortcut hint rendering. */
export function isMac(): boolean {
  if (typeof navigator === "undefined") return true;
  return /Mac|iPhone|iPad/.test(navigator.platform || navigator.userAgent);
}

/** Detect Windows — used to gate features not yet supported there (e.g. the
 *  CLI installer, whose backend is a stub on non-unix platforms). */
export function isWindows(): boolean {
  if (typeof navigator === "undefined") return false;
  return /Win/.test(navigator.platform || navigator.userAgent);
}

/** The platform command-key modifier for an event (meta on mac, ctrl else). */
export function cmdKey(e: KeyboardEvent | React.KeyboardEvent): boolean {
  return isMac() ? e.metaKey : e.ctrlKey;
}
