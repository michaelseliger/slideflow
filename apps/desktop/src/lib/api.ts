// Typed wrappers around the Tauri command surface, with a browser-mode mock
// fallback so `pnpm dev` works in a plain browser (no native shell).
//
// Every function here is the ONLY place the frontend talks to the backend —
// components never call `invoke` directly.

import type {
  ComposeReport,
  DeckRecord,
  DuplicateGroup,
  EmbedEvent,
  EmbeddingStatus,
  ExportEvent,
  ExportReport,
  FitMode,
  FontDownloadEvent,
  FontFamily,
  ModelDownloadEvent,
  RootRecord,
  SavedSearch,
  ScanEvent,
  SearchFilters,
  SearchHit,
  SimilarSlide,
  SlideDragPaths,
  SlidePick,
  SlidePreview,
  SlideRecord,
  Stats,
  StatsOverview,
  TagRecord,
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
// Saved searches
// ---------------------------------------------------------------------------

export function listSavedSearches(): Promise<SavedSearch[]> {
  return isTauri() ? tauriInvoke("list_saved_searches") : mock.listSavedSearches();
}

/** Persist the current query + filters under `name`; resolves to the stored row. */
export function saveSearch(
  name: string,
  query: string,
  filters: SearchFilters,
): Promise<SavedSearch> {
  return isTauri()
    ? tauriInvoke("save_search", { name, query, filters })
    : mock.saveSearch(name, query, filters);
}

export function renameSavedSearch(id: number, name: string): Promise<void> {
  return isTauri()
    ? tauriInvoke("rename_saved_search", { id, name })
    : mock.renameSavedSearch(id, name);
}

export function deleteSavedSearch(id: number): Promise<void> {
  return isTauri()
    ? tauriInvoke("delete_saved_search", { id })
    : mock.deleteSavedSearch(id);
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
// Tags
// ---------------------------------------------------------------------------

/** All tags, alphabetical, each with a live indexed-slide count. */
export function listTags(): Promise<TagRecord[]> {
  return isTauri() ? tauriInvoke("list_tags") : mock.listTags();
}

/** Tags currently assigned to one slide. */
export function getSlideTags(slideId: number): Promise<TagRecord[]> {
  return isTauri() ? tauriInvoke("get_slide_tags", { slideId }) : mock.getSlideTags(slideId);
}

/** Replace the full set of tags on a slide (creates/prunes tags as needed). */
export function setSlideTags(slideId: number, names: string[]): Promise<void> {
  return isTauri()
    ? tauriInvoke("set_slide_tags", { slideId, names })
    : mock.setSlideTags(slideId, names);
}

/** Rename a tag; rejects on a case-insensitive collision. */
export function renameTag(tagId: number, name: string): Promise<void> {
  return isTauri() ? tauriInvoke("rename_tag", { tagId, name }) : mock.renameTag(tagId, name);
}

/** Delete a tag and all its slide assignments. */
export function deleteTag(tagId: number): Promise<void> {
  return isTauri() ? tauriInvoke("delete_tag", { tagId }) : mock.deleteTag(tagId);
}

// ---------------------------------------------------------------------------
// Compose / export
// ---------------------------------------------------------------------------

export function composeDeck(
  picks: SlidePick[],
  outputPath: string,
  title: string,
  includeNotes: boolean,
  fitMode?: FitMode,
): Promise<ComposeReport> {
  return isTauri()
    ? tauriInvoke("compose_deck", {
        args: {
          picks,
          output_path: outputPath,
          title,
          include_notes: includeNotes,
          fit_mode: fitMode,
        },
      })
    : mock.composeDeck(picks, outputPath, title, includeNotes, fitMode);
}

/** Export the picked slides as a single PDF at `outputPath`. */
export function exportTrayPdf(
  picks: SlidePick[],
  outputPath: string,
  title: string,
): Promise<ExportReport> {
  return isTauri()
    ? tauriInvoke("export_tray_pdf", {
        args: { picks, output_path: outputPath, title },
      })
    : mockExport(picks, outputPath, "pdf");
}

/** Export the picked slides as one PNG each into `outDir`, at `width` px. */
export function exportTrayPngs(
  picks: SlidePick[],
  outDir: string,
  width: number,
): Promise<ExportReport> {
  return isTauri()
    ? tauriInvoke("export_tray_pngs", {
        args: { picks, out_dir: outDir, width },
      })
    : mockExport(picks, outDir, width);
}

/**
 * Subscribe to `export:event` progress while a PDF/PNG export runs. Returns an
 * unlisten function. In browser mode `mockExport` drives an in-memory bus.
 */
export async function onExportEvent(
  handler: (ev: ExportEvent) => void,
): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<ExportEvent>("export:event", (e) => handler(e.payload));
    return un;
  }
  mockExportListeners.add(handler);
  return () => mockExportListeners.delete(handler);
}

const mockExportListeners = new Set<(ev: ExportEvent) => void>();
function emitMockExport(ev: ExportEvent) {
  for (const l of mockExportListeners) l(ev);
}
/** Browser-mode fake export: streams short per-slide progress, then hands off to
 *  the mock library for a plausible report (and "Most exported" bump). The
 *  third arg is a PNG width when a number, or the PDF marker `"pdf"`. */
async function mockExport(
  picks: SlidePick[],
  target: string,
  kind: number | "pdf",
): Promise<ExportReport> {
  const total = picks.length;
  emitMockExport({ done: 0, total });
  for (let i = 1; i <= total; i++) {
    await sleep(Math.max(50, 400 / Math.max(1, total)));
    emitMockExport({ done: i, total });
  }
  return mock.exportTray(picks, target, kind);
}

// ---------------------------------------------------------------------------
// Native drag-out (macOS-first): drag a slide out of the app as a real file.
//
// Two steps: prepare the scratch files (compose the single-slide .pptx +
// render a drag-preview PNG), then start a native OS drag session carrying the
// .pptx via the drag plugin. Native-only — the browser UI hides the feature, so
// both wrappers are inert stubs in mock mode.
// ---------------------------------------------------------------------------

/** Compose the single-slide .pptx + drag-preview PNG for `pick` and return
 *  their paths. Cheap to call repeatedly — the host caches on (deck, slide,
 *  mtime), so the UI can pre-warm it on mousedown. Stub in browser mode. */
export function prepareSlideDrag(pick: SlidePick): Promise<SlideDragPaths> {
  return isTauri() ? tauriInvoke("prepare_slide_drag", { pick }) : mock.prepareSlideDrag(pick);
}

/** Start a native OS drag session carrying `paths` (absolute file paths) with
 *  `icon` (a PNG path) as the drag image. Wraps the CrabNebula drag plugin.
 *  Must run during the drag gesture; no-op in browser mode. */
export async function startNativeDrag(paths: string[], icon: string): Promise<void> {
  if (!isTauri()) return mock.startNativeDrag(paths, icon);
  const { startDrag } = await import("@crabnebula/tauri-plugin-drag");
  await startDrag({ item: paths, icon });
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
// Native menu (macOS)
// ---------------------------------------------------------------------------

/**
 * Subscribe to `menu:open` events from the native macOS app menu. Rust emits
 * the payload "about" | "settings" when the matching Slideflow-menu item is
 * clicked; both open the corresponding in-app sheet. No-op in browser mode
 * (and on Windows/Linux, which have no in-window menu bar), so it never fires
 * there. Returns an unlisten function.
 */
export async function onMenuOpen(
  handler: (target: "about" | "settings") => void,
): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  const un = await listen<"about" | "settings">("menu:open", (e) => handler(e.payload));
  return un;
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

/** Read the persisted auto-update preference — the same backend file the
 *  scheduler gates on — so the Settings toggle reflects the real state. */
export function getAutoUpdateEnabled(): Promise<boolean> {
  return isTauri()
    ? tauriInvoke("get_auto_update_enabled")
    : mock.getAutoUpdateEnabled();
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
// Semantic search (local E5 model, downloaded on consent)
//
// The Rust side (`src-tauri/src/semantic.rs`) owns the model download, the
// enable preference, and the embedding backfill; the frontend mirrors state
// via `get_embedding_status` + the `model:download` / `embed:event` streams.
// ---------------------------------------------------------------------------

export function getEmbeddingStatus(): Promise<EmbeddingStatus> {
  return isTauri() ? tauriInvoke("get_embedding_status") : mock.getEmbeddingStatus();
}

export function setSemanticSearchEnabled(enabled: boolean): Promise<void> {
  return isTauri()
    ? tauriInvoke("set_semantic_search_enabled", { enabled })
    : mock.setSemanticSearchEnabled(enabled);
}

/** Start (or resume) the ~490 MB model download. Resolves to false when a
 *  download is already running. Progress arrives on `model:download`. */
export function downloadEmbeddingModel(): Promise<boolean> {
  if (isTauri()) return tauriInvoke("download_embedding_model");
  return mockDownloadModel();
}

export function cancelModelDownload(): Promise<void> {
  if (isTauri()) return tauriInvoke("cancel_model_download");
  mockDownloadCanceled = true;
  return Promise.resolve();
}

/** Remove the model files from disk and disable semantic search. */
export function deleteEmbeddingModel(): Promise<void> {
  return isTauri() ? tauriInvoke("delete_embedding_model") : mock.deleteEmbeddingModel();
}

/** Re-run indexing: embed every slide text still missing a vector. Resolves to
 *  false when a backfill is already running (or no model is loaded). */
export function startEmbedBackfill(): Promise<boolean> {
  if (isTauri()) return tauriInvoke("start_embed_backfill");
  return mockStartBackfill();
}

export function cancelEmbedBackfill(): Promise<void> {
  if (isTauri()) return tauriInvoke("cancel_embed_backfill");
  return Promise.resolve();
}

/** Slides semantically closest to `slideId`. Empty when the model is absent. */
export function getSimilarSlides(slideId: number, limit = 12): Promise<SimilarSlide[]> {
  return isTauri()
    ? tauriInvoke("get_similar_slides", { slideId, limit })
    : mock.getSimilarSlides(slideId, limit);
}

/** Duplicate slide clusters (exact always; near when the model is loaded). */
export function listDuplicateGroups(): Promise<DuplicateGroup[]> {
  return isTauri() ? tauriInvoke("list_duplicate_groups") : mock.listDuplicateGroups();
}

/** Subscribe to `model:download` progress events. Returns an unlisten fn. */
export async function onModelDownloadEvent(
  handler: (ev: ModelDownloadEvent) => void,
): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<ModelDownloadEvent>("model:download", (e) => handler(e.payload));
    return un;
  }
  mockModelListeners.add(handler);
  return () => mockModelListeners.delete(handler);
}

/** Subscribe to `embed:event` backfill events. Returns an unlisten fn. */
export async function onEmbedEvent(
  handler: (ev: EmbedEvent) => void,
): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<EmbedEvent>("embed:event", (e) => handler(e.payload));
    return un;
  }
  mockEmbedListeners.add(handler);
  return () => mockEmbedListeners.delete(handler);
}

// Browser-mode fake model download + backfill, so `pnpm dev` demos the whole
// consent → download → indexing → ready flow without a native shell.
const mockModelListeners = new Set<(ev: ModelDownloadEvent) => void>();
const mockEmbedListeners = new Set<(ev: EmbedEvent) => void>();
let mockDownloadCanceled = false;
function emitMockModel(ev: ModelDownloadEvent) {
  for (const l of mockModelListeners) l(ev);
}
function emitMockEmbed(ev: EmbedEvent) {
  for (const l of mockEmbedListeners) l(ev);
}
async function mockDownloadModel(): Promise<boolean> {
  mockDownloadCanceled = false;
  mock.setModelDownloading(true);
  const total = 490 * 1024 * 1024;
  for (let i = 1; i <= 20; i++) {
    await sleep(180);
    if (mockDownloadCanceled) {
      mock.setModelDownloading(false);
      emitMockModel({ kind: "canceled" });
      return true;
    }
    emitMockModel({
      kind: "progress",
      file: "model.safetensors",
      downloaded: (total / 20) * i,
      total,
      overall_downloaded: (total / 20) * i,
      overall_total: total,
    });
  }
  mock.setModelDownloading(false);
  mock.setModelDownloaded(true);
  emitMockModel({ kind: "done" });
  void mockStartBackfill();
  return true;
}
async function mockStartBackfill(): Promise<boolean> {
  const total = (await mock.getStats()).slide_count;
  emitMockEmbed({ kind: "started", total });
  for (let done = 1; done <= total; done++) {
    await sleep(60);
    emitMockEmbed({ kind: "progress", done, total });
  }
  mock.setAllEmbedded();
  emitMockEmbed({ kind: "finished" });
  return true;
}

// ---------------------------------------------------------------------------
// Fonts (harvested / user-added / downloaded, under <app_data>/fonts)
//
// The Rust side (`src-tauri/src/fonts.rs`) owns the fonts dir, the curated
// download resolver, and harvesting; the frontend mirrors the family list and
// listens on `font:download` (per-download progress) and `fonts:changed` (any
// set change → drop the preview cache + re-render).
// ---------------------------------------------------------------------------

/** Every font family the indexed library names, with availability + source. */
export function listLibraryFonts(): Promise<FontFamily[]> {
  return isTauri() ? tauriInvoke("list_library_fonts") : mock.listLibraryFonts();
}

/** The `<app_data>/fonts` directory path (for "Reveal in Finder"). */
export function fontsDir(): Promise<string> {
  return isTauri() ? tauriInvoke("fonts_dir") : mock.fontsDir();
}

/** Copy validated .ttf/.otf files into the user fonts folder; returns the
 *  refreshed list. */
export function addUserFonts(paths: string[]): Promise<FontFamily[]> {
  return isTauri() ? tauriInvoke("add_user_fonts", { paths }) : mock.addUserFonts(paths);
}

/** Remove an app-added (harvested/user/downloaded) family; returns the list. */
export function removeAppFont(family: string): Promise<FontFamily[]> {
  return isTauri() ? tauriInvoke("remove_app_font", { family }) : mock.removeAppFont(family);
}

/** Start a curated font download (consent is obtained by the caller first).
 *  Resolves false when one is already running. Progress on `font:download`. */
export function downloadFont(family: string): Promise<boolean> {
  return isTauri() ? tauriInvoke("download_font", { family }) : mock.downloadFont(family);
}

export function cancelFontDownload(): Promise<void> {
  if (isTauri()) return tauriInvoke("cancel_font_download");
  return mock.cancelFontDownload();
}

/** Subscribe to `font:download` lifecycle events. Returns an unlisten fn. */
export async function onFontDownloadEvent(
  handler: (ev: FontDownloadEvent) => void,
): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen<FontDownloadEvent>("font:download", (e) => handler(e.payload));
    return un;
  }
  return mock.onFontDownloadEvent(handler);
}

/** Subscribe to `fonts:changed` (fired after any font-set change so the caller
 *  can drop its preview cache and re-render). Returns an unlisten fn. */
export async function onFontsChanged(handler: () => void): Promise<() => void> {
  if (isTauri()) {
    const { listen } = await import("@tauri-apps/api/event");
    const un = await listen("fonts:changed", () => handler());
    return un;
  }
  return mock.onFontsChanged(handler);
}

/** Native picker for one or more .ttf/.otf files; browser mode returns canned
 *  paths so the Settings flow is demoable. */
export async function pickFontFiles(): Promise<string[]> {
  if (!isTauri()) {
    return [`/Users/you/Fonts/VilleroyBoch-${Math.floor(Math.random() * 900 + 100)}.ttf`];
  }
  const { open } = await import("@tauri-apps/plugin-dialog");
  const result = await open({
    directory: false,
    multiple: true,
    filters: [{ name: "Fonts", extensions: ["ttf", "otf"] }],
  });
  if (Array.isArray(result)) return result;
  return typeof result === "string" ? [result] : [];
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

/** Native "save as…" dialog; browser mode returns a canned path. `defaultDir`
 *  (a remembered folder) pre-points the dialog when provided; `filter` sets the
 *  file-type filter (defaults to PowerPoint/.pptx). */
export async function pickSavePath(
  defaultName: string,
  defaultDir?: string,
  filter: { name: string; extensions: string[] } = { name: "PowerPoint", extensions: ["pptx"] },
): Promise<string | null> {
  if (!isTauri()) {
    const dir = defaultDir && defaultDir.length > 0 ? defaultDir : "/Users/you/Desktop";
    return `${dir}/${defaultName}`;
  }
  const { save } = await import("@tauri-apps/plugin-dialog");
  const sep = defaultDir && defaultDir.includes("\\") ? "\\" : "/";
  const defaultPath =
    defaultDir && defaultDir.length > 0 ? `${defaultDir}${sep}${defaultName}` : defaultName;
  const result = await save({ defaultPath, filters: [filter] });
  return result ?? null;
}
