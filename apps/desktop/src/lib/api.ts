// Typed wrappers around the Tauri command surface, with a browser-mode mock
// fallback so `pnpm dev` works in a plain browser (no native shell).
//
// Every function here is the ONLY place the frontend talks to the backend —
// components never call `invoke` directly.

import type {
  ComposeReport,
  DeckRecord,
  RootRecord,
  ScanEvent,
  SearchFilters,
  SearchHit,
  SlidePick,
  SlideRecord,
  Stats,
  StatsOverview,
} from "./types";
import { mock } from "./mock";
import { svgToDataUri } from "./utils";

/** True when running inside the Tauri webview (native shell present). */
export function isTauri(): boolean {
  return (
    typeof window !== "undefined" &&
    // Tauri v2 injects this global into the webview.
    ("__TAURI_INTERNALS__" in window || "__TAURI__" in window)
  );
}

// Lazily import the Tauri APIs so a plain browser never touches them.
async function tauriInvoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(cmd, args);
}

// ---------------------------------------------------------------------------
// Roots / folders
// ---------------------------------------------------------------------------

export function listRoots(): Promise<RootRecord[]> {
  return isTauri() ? tauriInvoke("list_roots") : mock.listRoots();
}

export function addRoot(path: string): Promise<RootRecord> {
  return isTauri() ? tauriInvoke("add_root", { path }) : mock.addRoot(path);
}

export function removeRoot(rootId: number): Promise<void> {
  return isTauri()
    ? tauriInvoke("remove_root", { rootId })
    : mock.removeRoot(rootId);
}

// ---------------------------------------------------------------------------
// Scanning
// ---------------------------------------------------------------------------

export function startScan(): Promise<boolean> {
  if (isTauri()) return tauriInvoke("start_scan");
  return mockStartScan();
}

export function isScanning(): Promise<boolean> {
  return isTauri() ? tauriInvoke("is_scanning") : Promise.resolve(false);
}

/**
 * Subscribe to `scan:event` progress events. Returns an unlisten function.
 * In browser mode this wires up an in-memory event bus that `mockStartScan`
 * drives.
 */
export async function onScanEvent(
  handler: (ev: ScanEvent) => void,
): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<ScanEvent>("scan:event", (e) => handler(e.payload));
    return un;
  }
  mockScanListeners.add(handler);
  return () => mockScanListeners.delete(handler);
}

const mockScanListeners = new Set<(ev: ScanEvent) => void>();
function emitMockScan(ev: ScanEvent) {
  for (const l of mockScanListeners) l(ev);
}
async function mockStartScan(): Promise<boolean> {
  const decks = await mock.getDecks();
  const total = decks.length;
  emitMockScan({ kind: "started", total_files: total });
  let done = 0;
  for (const d of decks) {
    await new Promise((r) => setTimeout(r, 120));
    done += 1;
    emitMockScan({ kind: "deck", path: d.path, done, total });
  }
  const stats = await mock.getStats();
  emitMockScan({
    kind: "finished",
    indexed: total,
    removed: 0,
    unchanged: 0,
  });
  void stats;
  return true;
}

// ---------------------------------------------------------------------------
// Search / browse
// ---------------------------------------------------------------------------

export function search(
  query: string,
  filters: SearchFilters = {},
): Promise<SearchHit[]> {
  return isTauri()
    ? tauriInvoke("search", { query, filters })
    : mock.search(query, filters);
}

export function getDecks(): Promise<DeckRecord[]> {
  return isTauri() ? tauriInvoke("get_decks") : mock.getDecks();
}

export function getDeckSlides(deckId: number): Promise<SlideRecord[]> {
  return isTauri()
    ? tauriInvoke("get_deck_slides", { deckId })
    : mock.getDeckSlides(deckId);
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

/** Preview quality tier: small grid tile vs. crisper modal/inspector preview. */
export type PreviewTier = "thumb" | "full";

/**
 * A string usable directly as an `<img src>` for a slide's preview.
 *
 * In the native app the SVG is rendered + cached to a file and served over the
 * `asset:` protocol (via `convertFileSrc`), so the potentially multi-MB SVG
 * never crosses IPC and the webview caches it by URL. In browser-mock mode the
 * mock SVG string is wrapped as a data URI. Both are plain `<img src>` values,
 * so callers stay mode-agnostic.
 */
export async function getSlidePreviewSrc(
  slideId: number,
  tier: PreviewTier,
): Promise<string> {
  if (isTauri()) {
    const path = await tauriInvoke<string>("get_slide_preview", { slideId, tier });
    const { convertFileSrc } = await import("@tauri-apps/api/core");
    return convertFileSrc(path);
  }
  return svgToDataUri(await mock.getSlideSvg(slideId));
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

export function getStats(): Promise<Stats> {
  return isTauri() ? tauriInvoke("get_stats") : mock.getStats();
}

export function getStatsOverview(): Promise<StatsOverview> {
  return isTauri() ? tauriInvoke("get_stats_overview") : mock.getStatsOverview();
}

/** Remember a settled search for the stats view (fire-and-forget). */
export function recordSearch(query: string, resultCount: number): Promise<void> {
  return isTauri()
    ? tauriInvoke("record_search", { query, resultCount })
    : mock.recordSearch(query, resultCount);
}

// ---------------------------------------------------------------------------
// Favorites
// ---------------------------------------------------------------------------

/** Toggle a slide's favorite star; resolves to the new state. */
export function toggleFavoriteSlide(slideId: number): Promise<boolean> {
  return isTauri()
    ? tauriInvoke("toggle_favorite_slide", { slideId })
    : mock.toggleFavoriteSlide(slideId);
}

/** Toggle a deck's favorite star; resolves to the new state. */
export function toggleFavoriteDeck(deckId: number): Promise<boolean> {
  return isTauri()
    ? tauriInvoke("toggle_favorite_deck", { deckId })
    : mock.toggleFavoriteDeck(deckId);
}

// ---------------------------------------------------------------------------
// Compose / export
// ---------------------------------------------------------------------------

export function composeDeck(
  picks: SlidePick[],
  outputPath: string,
  title: string,
  includeNotes: boolean,
): Promise<ComposeReport> {
  return isTauri()
    ? tauriInvoke("compose_deck", {
        args: {
          picks,
          output_path: outputPath,
          title,
          include_notes: includeNotes,
        },
      })
    : mock.composeDeck(picks, outputPath, title, includeNotes);
}

// ---------------------------------------------------------------------------
// System integration
// ---------------------------------------------------------------------------

export function revealInFinder(path: string): Promise<void> {
  if (isTauri()) return tauriInvoke("reveal_in_finder", { path });
  console.info("[mock] reveal in Finder:", path);
  return Promise.resolve();
}

export function openFile(path: string): Promise<void> {
  if (isTauri()) return tauriInvoke("open_file", { path });
  console.info("[mock] open file:", path);
  return Promise.resolve();
}

/** Open an external URL in the default browser. */
export function openUrl(url: string): Promise<void> {
  if (isTauri()) return tauriInvoke("open_url", { url });
  window.open(url, "_blank", "noopener,noreferrer");
  return Promise.resolve();
}

/** App version string, read from the Tauri bundle config at runtime. */
export async function getAppVersion(): Promise<string> {
  if (!isTauri()) return "dev";
  const { getVersion } = await import("@tauri-apps/api/app");
  return getVersion();
}

// ---------------------------------------------------------------------------
// Native dialogs (folder picker, save sheet)
// ---------------------------------------------------------------------------

/** Native "choose folder" dialog; browser mode returns a canned path. */
export async function pickFolder(): Promise<string | null> {
  if (!isTauri()) {
    return `/Users/you/Decks/Sample-${Math.floor(Math.random() * 900 + 100)}`;
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({ directory: true, multiple: false });
  return typeof result === "string" ? result : null;
}

/** Native "save as .pptx" dialog; browser mode returns a canned path. */
export async function pickSavePath(defaultName: string): Promise<string | null> {
  if (!isTauri()) {
    return `/Users/you/Desktop/${defaultName}`;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const result = await save({
    defaultPath: defaultName,
    filters: [{ name: "PowerPoint", extensions: ["pptx"] }],
  });
  return result ?? null;
}
