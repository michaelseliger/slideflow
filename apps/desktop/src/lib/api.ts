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
  SlidePreview,
  SlideRecord,
  Stats,
  StatsOverview,
  UpdateEvent,
} from "./types";
import { mock, MOCK_SCAN_ISSUE } from "./mock";
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

/** Replace a root's exclude globs (validated backend-side); resolves to the
 *  updated record. The caller follows this with a rescan to apply them. */
export function setRootExcludes(
  rootId: number,
  patterns: string[],
): Promise<RootRecord> {
  return isTauri()
    ? tauriInvoke("set_root_excludes", { rootId, patterns })
    : mock.setRootExcludes(rootId, patterns);
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

/** Clear the whole index + preview cache (keeps roots + favorites). The caller
 *  follows this with `startScan` to rebuild. */
export function clearIndex(): Promise<void> {
  return isTauri() ? tauriInvoke("clear_index") : mock.clearIndex();
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
  // Re-seed the mock library first so a rebuild after clearIndex() repopulates.
  await mock.rebuildFromDisk();
  const decks = await mock.getDecks();
  const total = decks.length;
  emitMockScan({ kind: "started", total_files: total });
  let done = 0;
  for (const d of decks) {
    await new Promise((r) => setTimeout(r, 120));
    done += 1;
    emitMockScan({ kind: "deck", path: d.path, done, total });
  }
  // Emit one skip so browser mode exercises the diagnostics surface.
  emitMockScan({ kind: "skipped", path: MOCK_SCAN_ISSUE.path, reason: MOCK_SCAN_ISSUE.reason });
  const stats = await mock.getStats();
  emitMockScan({
    kind: "finished",
    indexed: total,
    removed: 0,
    unchanged: 0,
    skipped: 1,
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
 * An `<img src>` for a slide's preview plus the set of unsupported construct
 * kinds the renderer skipped (feeds the "Approximate" badge).
 *
 * In the native app the SVG is rendered + cached to a file and served over the
 * `asset:` protocol (via `convertFileSrc`), so the potentially multi-MB SVG
 * never crosses IPC and the webview caches it by URL. In browser-mock mode the
 * mock SVG string is wrapped as a data URI. Both `src` values are plain
 * `<img src>` values, so callers stay mode-agnostic.
 */
export async function getSlidePreview(
  slideId: number,
  tier: PreviewTier,
): Promise<{ src: string; dropped: string[] }> {
  if (isTauri()) {
    const res = await tauriInvoke<SlidePreview>("get_slide_preview", { slideId, tier });
    const { convertFileSrc } = await import("@tauri-apps/api/core");
    return { src: convertFileSrc(res.path), dropped: res.dropped };
  }
  return { src: svgToDataUri(await mock.getSlideSvg(slideId)), dropped: mock.getSlideDropped(slideId) };
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

/** Total slides exported per deck path, keyed by path — powers "Most exported". */
export function getExportCounts(): Promise<Record<string, number>> {
  return isTauri() ? tauriInvoke("get_export_counts") : mock.getExportCounts();
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
// Auto-update
//
// The whole lifecycle (check → download → install) lives in Rust
// (`src-tauri/src/updates.rs`) so a downloaded update can still be installed
// on quit. The frontend only mirrors state via `update:event` and triggers
// the flow through the commands below.
// ---------------------------------------------------------------------------

/** Whether this install can update itself in place (false in dev builds and
 *  for Linux deb/rpm installs, which update via the package manager). */
export function updatesSupported(): Promise<boolean> {
  if (isTauri()) return tauriInvoke("updates_supported");
  // Browser mock: only pretend to support updates when the fake flow is on.
  return Promise.resolve(mockUpdateEnabled());
}

/** Subscribe to `update:event` lifecycle events. Returns an unlisten fn. */
export async function onUpdateEvent(
  handler: (ev: UpdateEvent) => void,
): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<UpdateEvent>("update:event", (e) => handler(e.payload));
    return un;
  }
  mockUpdateListeners.add(handler);
  return () => mockUpdateListeners.delete(handler);
}

/** Kick off a check (and, if an update exists, a background download).
 *  Fire-and-forget — all results arrive as `update:event`s. */
export function checkForUpdates(): Promise<void> {
  if (isTauri()) return tauriInvoke("check_for_updates");
  return mockCheckForUpdates();
}

/** Install the downloaded update and relaunch as the new version. */
export function restartToUpdate(): Promise<void> {
  if (isTauri()) return tauriInvoke("restart_to_update");
  return mockRestartToUpdate();
}

/** Enable/disable automatic (boot + daily) update checks. Manual checks are unaffected. */
export function setAutoUpdateEnabled(enabled: boolean): Promise<void> {
  return isTauri()
    ? tauriInvoke("set_auto_update_enabled", { enabled })
    : mock.setAutoUpdateEnabled(enabled);
}

// Browser-mode fake update flow, opt-in so `pnpm dev` isn't nagged:
//   localStorage.setItem("slideflow:mockUpdate", "1")
const mockUpdateListeners = new Set<(ev: UpdateEvent) => void>();
function emitMockUpdate(ev: UpdateEvent) {
  for (const l of mockUpdateListeners) l(ev);
}
function mockUpdateEnabled(): boolean {
  return localStorage.getItem("slideflow:mockUpdate") === "1";
}
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
async function mockCheckForUpdates(): Promise<void> {
  emitMockUpdate({ kind: "checking" });
  await sleep(800);
  if (!mockUpdateEnabled()) {
    emitMockUpdate({ kind: "up_to_date" });
    return;
  }
  const version = "0.99.0";
  const total = 24 * 1024 * 1024;
  emitMockUpdate({ kind: "available", version });
  for (let i = 1; i <= 12; i++) {
    await sleep(250);
    emitMockUpdate({ kind: "downloading", downloaded: (total / 12) * i, total });
  }
  emitMockUpdate({ kind: "ready", version });
}
async function mockRestartToUpdate(): Promise<void> {
  await sleep(400);
  window.location.reload();
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

/** Native "save as .pptx" dialog; browser mode returns a canned path.
 *  `defaultDir` (a remembered folder) pre-points the dialog when provided. */
export async function pickSavePath(
  defaultName: string,
  defaultDir?: string,
): Promise<string | null> {
  if (!isTauri()) {
    const dir = defaultDir && defaultDir.length > 0 ? defaultDir : "/Users/you/Desktop";
    return `${dir}/${defaultName}`;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const sep = defaultDir && defaultDir.includes("\\") ? "\\" : "/";
  const defaultPath =
    defaultDir && defaultDir.length > 0 ? `${defaultDir}${sep}${defaultName}` : defaultName;
  const result = await save({
    defaultPath,
    filters: [{ name: "PowerPoint", extensions: ["pptx"] }],
  });
  return result ?? null;
}
