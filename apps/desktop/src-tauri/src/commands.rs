//! Tauri command surface — the bridge between the React frontend and
//! `slideflow-core`. Every command is thin: it locks the [`Library`], calls a
//! core method, maps the error to a `String`, and returns serde-serializable
//! model types straight across the IPC boundary.
//!
//! Long-running work (scanning, composing, rendering) runs on a blocking
//! thread so the async runtime and the webview never stall.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use tokio::sync::Semaphore;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use slideflow_core::dragout::cache_key;
use slideflow_core::export::{
    deck_stem, export_pdf, export_pngs, render_slide_png_from, sanitize_component, PdfOptions,
    PngOptions,
};
use slideflow_core::index::Library;
use slideflow_core::model::{
    ComposeReport, DeckRecord, DuplicateGroup, ExportReport, FitMode, RootRecord, SavedSearch,
    SearchFilters, SearchHit, SimilarSlide, SlidePick, SlideRecord, StatsOverview, TagRecord,
};
use slideflow_core::pptx::composer::{compose, ComposeOptions};
use slideflow_core::pptx::PresentationFile;
use slideflow_core::render::{render_slide, RenderOptions};
use slideflow_core::thumbs::{sweep_thumbs, thumb_file_name, ThumbTier};

/// Shared, Tauri-managed application state.
///
/// Two connections to the same WAL-mode database: `library` serves search,
/// browse, and small writes; `scan_library` is dedicated to long-running
/// scans. Scans commit per deck, so searches on `library` stay live (and see
/// freshly indexed decks) while a scan is in flight instead of queueing
/// behind one mutex for its whole duration.
pub struct AppState {
    /// Interactive connection: search/browse/thumbs/roots.
    pub library: Mutex<Library>,
    /// Scan-only connection, held for the duration of a scan.
    pub scan_library: Mutex<Library>,
    /// `app_cache_dir/thumbs` — where rendered slide SVGs are cached.
    pub thumbs_dir: PathBuf,
    /// `app_cache_dir/dragout` — scratch home for the single-slide `.pptx` +
    /// PNG icon written when a slide is dragged out or saved (see
    /// [`prepare_slide_drag`]). Wiped on every launch (lib.rs setup).
    pub dragout_dir: PathBuf,
    /// Guards against two concurrent scans stepping on each other. An `Arc` so
    /// the background scan thread can hold a `'static` handle in an RAII guard
    /// that releases the flag even on a panic-unwind (see [`ScanFlagGuard`]).
    pub scanning: Arc<AtomicBool>,
    /// Bounds how many slide renders run at once. Rendering fully inflates a
    /// deck's zip and is CPU-bound, so a scroll burst of ~20 tile requests must
    /// not spawn 20 parallel 100 MB decompressions. Two keeps the pipeline fed
    /// without thrashing.
    pub render_permits: Semaphore,
    /// Tiny MRU cache of opened presentations, so scrolling one big deck's grid
    /// inflates its zip once instead of once per cache-missed slide. Keyed by
    /// `(deck_path, content_hash)`; a hash mismatch (file changed) reopens.
    pub deck_cache: Mutex<Vec<(String, String, Arc<PresentationFile>)>>,
    /// Single-flight guard for [`prepare_slide_drag`]: the set of drag-out cache
    /// keys currently being written. A second prepare for the same key waits on
    /// `dragout_done` until the first finishes, then re-checks the cache — so two
    /// concurrent prepares never write the same `.pptx` an in-flight OS drag is
    /// reading.
    pub dragout_inflight: Mutex<HashSet<String>>,
    pub dragout_done: Condvar,
}

/// Concurrent renders permitted at once (see `render_permits`).
const RENDER_CONCURRENCY: usize = 2;
/// Opened presentations kept resident. A 100 MB deck fully inflates to several
/// hundred MB, so this is deliberately tiny.
const DECK_CACHE_SLOTS: usize = 2;

impl AppState {
    pub fn new(
        library: Library,
        scan_library: Library,
        thumbs_dir: PathBuf,
        dragout_dir: PathBuf,
    ) -> Self {
        AppState {
            library: Mutex::new(library),
            scan_library: Mutex::new(scan_library),
            thumbs_dir,
            dragout_dir,
            scanning: Arc::new(AtomicBool::new(false)),
            render_permits: Semaphore::new(RENDER_CONCURRENCY),
            deck_cache: Mutex::new(Vec::new()),
            dragout_inflight: Mutex::new(HashSet::new()),
            dragout_done: Condvar::new(),
        }
    }

    /// An already-opened deck matching `(path, hash)`, promoted to most-recently
    /// used. A hash mismatch (the file changed on disk) misses so the caller
    /// reopens it.
    fn deck_lookup(&self, path: &str, hash: &str) -> Option<Arc<PresentationFile>> {
        let mut cache = self.deck_cache.lock().ok()?;
        let pos = cache.iter().position(|(p, h, _)| p == path && h == hash)?;
        let entry = cache.remove(pos);
        let arc = Arc::clone(&entry.2);
        cache.insert(0, entry);
        Some(arc)
    }

    /// Insert (or refresh) an opened deck at the front, dropping any stale entry
    /// for the same path and capping the cache to [`DECK_CACHE_SLOTS`].
    fn deck_insert(&self, path: String, hash: String, pf: Arc<PresentationFile>) {
        if let Ok(mut cache) = self.deck_cache.lock() {
            cache.retain(|(p, _, _)| p != &path);
            cache.insert(0, (path, hash, pf));
            cache.truncate(DECK_CACHE_SLOTS);
        }
    }
}

/// Convenience: map any `Display` error into the `String` Tauri sends to JS.
fn e<E: std::fmt::Display>(err: E) -> String {
    err.to_string()
}

// ---------------------------------------------------------------------------
// Roots / folders
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_roots(state: State<'_, AppState>) -> Result<Vec<RootRecord>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.roots().map_err(e)
}

#[tauri::command]
pub async fn add_root(state: State<'_, AppState>, path: String) -> Result<RootRecord, String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.add_root(Path::new(&path)).map_err(e)
}

#[tauri::command]
pub async fn remove_root(state: State<'_, AppState>, root_id: i64) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.remove_root(root_id).map_err(e)
}

/// Replace a root's exclude globs (validate-then-store). Writes on the
/// interactive `library` connection; the frontend follows this with a rescan
/// (separate `scan_library` connection, same WAL DB) to apply the new filter.
#[tauri::command]
pub async fn set_root_excludes(
    state: State<'_, AppState>,
    root_id: i64,
    patterns: Vec<String>,
) -> Result<RootRecord, String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.set_root_excludes(root_id, &patterns).map_err(e)
}

// ---------------------------------------------------------------------------
// Scanning (background thread, event-driven)
// ---------------------------------------------------------------------------

/// RAII release of the `scanning` flag. Constructed at the top of the scan
/// thread so the flag clears on *every* exit path — normal return, an early
/// `return` on a poisoned lock, or a panic-unwind out of `Library::scan`.
/// Without this a panic would leave `scanning` wedged `true` forever: every
/// later `start_scan` would silently no-op and `clear_index` would fail until
/// the app restarts.
struct ScanFlagGuard(Arc<AtomicBool>);

impl Drop for ScanFlagGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Kick off an incremental rescan of all roots on a background thread.
///
/// Progress is streamed to the frontend as `scan:event` events carrying a
/// [`slideflow_core::model::ScanEvent`] payload. Re-entrancy is prevented by an
/// [`AtomicBool`]; a second call while a scan runs returns `Ok(false)`.
#[tauri::command]
pub async fn start_scan(app: AppHandle) -> Result<bool, String> {
    let state = app.state::<AppState>();
    // Claim the scanning flag; bail out (not an error) if one is already running.
    if state
        .scanning
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(false);
    }

    let app_for_thread = app.clone();
    let scanning_flag = Arc::clone(&state.scanning);
    std::thread::spawn(move || {
        // Release the flag on any exit path, panics included. Dropped
        // explicitly once the thumb sweep is done (below) so the sweep stays
        // inside the scan's mutual-exclusion window — a following scan can't
        // index a new deck whose fresh thumb this sweep would then delete.
        let flag_guard = ScanFlagGuard(scanning_flag);
        let state = app_for_thread.state::<AppState>();
        let (result, valid) = {
            let mut lib = match state.scan_library.lock() {
                Ok(lib) => lib,
                Err(_) => {
                    let _ = app_for_thread.emit("scan:error", "library lock poisoned");
                    return;
                }
            };
            let app_emit = app_for_thread.clone();
            let result = lib.scan(&mut |event| {
                let _ = app_emit.emit("scan:event", &event);
            });
            // Snapshot the valid-set for the thumb sweep while we still hold the
            // lock; skip it if the scan itself failed (the set would be partial).
            let valid = result.as_ref().ok().and_then(|_| lib.all_deck_hashes().ok());
            (result, valid)
        };
        // Reclaim orphaned cache files: decks that vanished or changed hash this
        // scan, plus any legacy `<id>.svg` files from before content-addressing.
        // Still under the scanning flag so it can't race a following scan's
        // freshly-written thumbnails.
        if let Some(valid) = valid {
            sweep_thumbs(&state.thumbs_dir, &valid);
        }
        // Sweep complete — release the flag before the (non-exclusive) post-scan
        // cache refresh + backfill/font-harvest spawns so a rescan or clear_index
        // is only ever blocked through the sweep.
        drop(flag_guard);
        match result {
            Err(err) => {
                // A failed scan still deletes+reinserts deck/slide rows on the
                // scan connection (rowids recycled), so the interactive
                // connection's caches are just as stale as on success. Library::scan
                // already invalidated the scan connection's own cache; refresh the
                // interactive one too so semantic search never serves stale ids.
                if let Ok(lib) = state.library.lock() {
                    lib.invalidate_vector_cache();
                }
                let _ = app_for_thread.emit("scan:error", err.to_string());
            }
            Ok(()) => {
                // The scan wrote through the scan connection; the interactive
                // connection's in-memory vector/near-dup caches are now stale.
                if let Ok(lib) = state.library.lock() {
                    lib.invalidate_vector_cache();
                }
                // While semantic search is enabled, embed whatever this scan's
                // inline path didn't cover (e.g. pre-hashing decks skipped as
                // unchanged). No-op when disabled or the model isn't loaded.
                crate::semantic::spawn_backfill_if_enabled(&app_for_thread);
                // Harvest any newly-indexed decks' embeddable fonts so every
                // deck naming that family benefits. No-op when nothing embeds a
                // font. Rebuilds the font set + invalidates previews if a new
                // face lands.
                crate::fonts::spawn_harvest_after_scan(&app_for_thread);
            }
        }
    });

    Ok(true)
}

#[tauri::command]
pub async fn is_scanning(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(state.scanning.load(Ordering::SeqCst))
}

/// Clear the entire index (decks, slides, search/scan/export history) and the
/// on-disk preview cache, keeping roots + favorites. Guarded by the same
/// `scanning` flag as [`start_scan`] so it can't race a scan; the frontend
/// follows this with a fresh `start_scan` to rebuild.
#[tauri::command]
pub async fn clear_index(app: AppHandle) -> Result<(), String> {
    let state = app.state::<AppState>();
    // Claim the scanning flag so no scan mutates the index underneath us.
    if state
        .scanning
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("Can't clear the index while a scan is running.".into());
    }

    // Run the whole op in an immediately-invoked closure so the flag is ALWAYS
    // released afterwards, whatever fails.
    let result: Result<(), String> = (|| {
        // Do the fallible filesystem prep BEFORE the (immediately-committed) DB
        // wipe: `create_dir_all` can still fail (read-only FS, ENOSPC, a parent
        // turned unwritable), and if it does we must not have already emptied
        // the index out from under a frontend that then shows an error and skips
        // its rebuild — that would leave a stale UI over an empty DB. Ordering it
        // first means a failure here leaves the index intact and retryable.
        let _ = std::fs::remove_dir_all(&state.thumbs_dir);
        std::fs::create_dir_all(&state.thumbs_dir).map_err(e)?;
        {
            let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
            lib.clear().map_err(e)?;
        }
        if let Ok(mut cache) = state.deck_cache.lock() {
            cache.clear();
        }
        Ok(())
    })();
    state.scanning.store(false, Ordering::SeqCst);
    result
}

// ---------------------------------------------------------------------------
// Search / browse
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn search(
    state: State<'_, AppState>,
    query: String,
    filters: SearchFilters,
) -> Result<Vec<SearchHit>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.search(&query, &filters).map_err(e)
}

#[tauri::command]
pub async fn get_decks(state: State<'_, AppState>) -> Result<Vec<DeckRecord>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.decks().map_err(e)
}

#[tauri::command]
pub async fn get_deck_slides(
    state: State<'_, AppState>,
    deck_id: i64,
) -> Result<Vec<SlideRecord>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.slides_for_deck(deck_id).map_err(e)
}

/// Slides semantically closest to a given slide (find-similar, roadmap #6).
/// Empty — never an error — when the model is absent or the slide isn't
/// embedded yet.
#[tauri::command]
pub async fn get_similar_slides(
    state: State<'_, AppState>,
    slide_id: i64,
    limit: usize,
) -> Result<Vec<SimilarSlide>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.get_similar_slides(slide_id, limit.min(50)).map_err(e)
}

/// Duplicate slide clusters: exact (identical content hash) always; near
/// (embedding-similar) additionally when the model is loaded (roadmap #9).
///
/// The expensive phase — the first vector-store load and the O(n²) near-dup
/// clustering — must NOT run while the interactive library mutex is held, or it
/// freezes every other command. So: snapshot the store + exact groups under a
/// short lock, release, cluster in `spawn_blocking`, then re-lock briefly only to
/// hydrate rows. Memoization is preserved (a warm cache skips the blocking phase).
#[tauri::command]
pub async fn list_duplicate_groups(app: AppHandle) -> Result<Vec<DuplicateGroup>, String> {
    let state = app.state::<AppState>();

    // Phase 1 — short lock: load the vectors (a DB read) and take a cheap `Arc`
    // snapshot alongside the exact groups and any memoized near clusters.
    let (store, exact, cached_near) = {
        let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
        let store = lib.ensure_vectors_loaded().map_err(e)?;
        let exact = lib.exact_dup_groups().map_err(e)?;
        let cached_near = lib.cached_near_clusters();
        (store, exact, cached_near)
    };

    // Phase 2 — no lock held: the O(n²) clustering, off the mutex AND off the
    // async runtime. Skipped entirely when already memoized or no model is loaded.
    let near_raw = match cached_near {
        Some(clusters) => clusters,
        None => match store.clone() {
            Some(store) => tauri::async_runtime::spawn_blocking(move || {
                slideflow_core::index::near_dup_clusters_for(&store)
            })
            .await
            .map_err(e)?,
            None => Vec::new(),
        },
    };

    // Phase 3 — short re-lock: memoize the fresh clusters + hydrate rows.
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.finish_duplicate_groups(exact, near_raw, store.as_ref()).map_err(e)
}

// ---------------------------------------------------------------------------
// Saved searches
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn list_saved_searches(state: State<'_, AppState>) -> Result<Vec<SavedSearch>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.list_saved_searches().map_err(e)
}

#[tauri::command]
pub async fn save_search(
    state: State<'_, AppState>,
    name: String,
    query: String,
    filters: SearchFilters,
) -> Result<SavedSearch, String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.save_search(&name, &query, &filters).map_err(e)
}

#[tauri::command]
pub async fn rename_saved_search(
    state: State<'_, AppState>,
    id: i64,
    name: String,
) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.rename_saved_search(id, &name).map_err(e)
}

#[tauri::command]
pub async fn delete_saved_search(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.delete_saved_search(id).map_err(e)
}

// ---------------------------------------------------------------------------
// Rendering (SVG thumbnails, cached on disk)
// ---------------------------------------------------------------------------

/// Render (or serve from cache) a slide preview and return the absolute path of
/// the cached SVG file. The frontend turns that into an `asset:` URL via
/// `convertFileSrc` and loads it in a plain `<img>` — so the multi-MB SVG never
/// crosses the IPC boundary and the webview gets HTTP-style caching for free.
///
/// `tier` picks the quality: `"thumb"` (small grid tile, images downscaled hard)
/// or `"full"` (crisper preview for the peek modal / inspector). Each tier is a
/// distinct cache file.
///
/// The cache filename is *content-addressed* (see [`thumb_file_name`]): derived
/// from the deck's path + content hash + slide index + tier, never the slide
/// rowid. Rowids are recycled after `DELETE`, so a filename keyed on the id would
/// serve a removed slide's SVG to whatever new slide inherited its id. The DB
/// lookup runs *before* the cache check so an unknown (deleted) id errors instead
/// of matching a stale file.
/// A rendered slide preview: the cache-file path plus the set of unsupported
/// construct kinds the renderer skipped (feeds the "Approximate" badge).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlidePreview {
    pub path: String,
    pub dropped: Vec<String>,
}

#[tauri::command]
pub async fn get_slide_preview(
    state: State<'_, AppState>,
    fonts: State<'_, crate::fonts::FontsState>,
    slide_id: i64,
    tier: String,
) -> Result<SlidePreview, String> {
    let tier = ThumbTier::parse(&tier);
    let (deck_path, content_hash, slide_index) = {
        let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
        lib.slide_render_info(slide_id).map_err(e)?
    };

    let file_name = thumb_file_name(&deck_path, &content_hash, slide_index as usize, tier);
    let cache_path = state.thumbs_dir.join(&file_name);

    // Fast path: a present, non-empty cache file is served straight from disk —
    // no permit, no open, no render. Kept ahead of the semaphore so cache hits
    // never queue behind in-flight renders. The drop set is a cheap indexed
    // point-read from render_issues (empty when the row is missing/stale).
    if cache_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        let dropped = {
            let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
            lib.render_issues_for(&deck_path, slide_index, &content_hash).unwrap_or_default()
        };
        return Ok(SlidePreview { path: cache_path.to_string_lossy().into_owned(), dropped });
    }

    // Bound how many renders inflate a deck + rasterize at once.
    let _permit = state.render_permits.acquire().await.map_err(e)?;

    // Reuse an already-open deck when possible; otherwise inflate it off the
    // async runtime (a 100 MB zip) and cache it.
    let pf = match state.deck_lookup(&deck_path, &content_hash) {
        Some(pf) => pf,
        None => {
            let dp = deck_path.clone();
            let opened = tauri::async_runtime::spawn_blocking(move || {
                PresentationFile::open(Path::new(&dp)).map_err(e)
            })
            .await
            .map_err(e)??;
            let arc = Arc::new(opened);
            state.deck_insert(deck_path.clone(), content_hash.clone(), Arc::clone(&arc));
            arc
        }
    };

    // Render off the async runtime too — it's CPU-bound. Inject the app-local
    // font set so both tiers embed harvested/user/downloaded faces as per-slide
    // subsetted @font-face under their real names (thumb tier skips a face when
    // subsetting fails; the full/peek tier embeds it whole — FontEmbedding).
    let mut options = match tier {
        ThumbTier::Thumb => RenderOptions::thumb(),
        ThumbTier::Full => RenderOptions::preview(),
    };
    // Snapshot the app faces AND the font generation together. If a font change
    // lands while the (blocking) render runs, the SVG is already stale: we still
    // return it for immediate display, but must NOT write it into the
    // just-recreated thumbs dir, where it would persist as a valid
    // content-addressed hit served with the old fonts.
    let (app_fonts, font_generation) = fonts.app_set_with_generation();
    options.app_fonts = Some(app_fonts);
    let idx = slide_index as usize;
    let outcome = tauri::async_runtime::spawn_blocking(move || {
        render_slide(&pf, idx, &options).map_err(e)
    })
    .await
    .map_err(e)??;

    // Persist the drop telemetry (best-effort — the badge is an optional signal).
    {
        let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
        let _ = lib.record_render_issues(&deck_path, slide_index, &content_hash, &outcome.dropped);
    }

    // Skip the cache write if the fonts changed under us (invalidate_and_notify
    // wiped and recreated thumbs_dir meanwhile) — otherwise this stale SVG becomes
    // a persistent hit. The next request re-renders with the new fonts.
    if fonts.generation() == font_generation {
        write_cache_atomic(&state.thumbs_dir, &cache_path, outcome.svg.as_bytes());
    }

    Ok(SlidePreview {
        path: cache_path.to_string_lossy().into_owned(),
        dropped: outcome.dropped,
    })
}

/// A uniquely named temp path beside `final_path` (same directory → same
/// filesystem, so the follow-up `rename` is atomic). Unique per process +
/// monotonic sequence so concurrent writers never collide on the temp name.
fn tmp_sibling(final_path: &Path) -> PathBuf {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let dir = final_path.parent().unwrap_or_else(|| Path::new("."));
    dir.join(format!(".tmp-{}-{}", std::process::id(), seq))
}

/// Atomically place `bytes` at `final_path`: write a uniquely named sibling temp,
/// then `rename` over the target. A crash or a concurrent reader therefore never
/// observes a partially written file — the target flips old→new in one step. The
/// temp is removed if the rename fails.
fn atomic_write(final_path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = tmp_sibling(final_path);
    fs::write(&tmp, bytes)?;
    if let Err(err) = fs::rename(&tmp, final_path) {
        let _ = fs::remove_file(&tmp);
        return Err(err);
    }
    Ok(())
}

/// Atomically move an already-written `tmp` onto `final_path` (for producers like
/// the composer that write the temp themselves). Removes `tmp` on failure.
fn atomic_place(tmp: &Path, final_path: &Path) -> std::io::Result<()> {
    if let Err(err) = fs::rename(tmp, final_path) {
        let _ = fs::remove_file(tmp);
        return Err(err);
    }
    Ok(())
}

/// Best-effort atomic cache write (thumbnails): a partial file could never be
/// served as a valid cache hit. Swallows I/O errors — the cache is only an
/// optimization.
fn write_cache_atomic(dir: &Path, final_path: &Path, bytes: &[u8]) {
    let _ = fs::create_dir_all(dir);
    let _ = atomic_write(final_path, bytes);
}

// ---------------------------------------------------------------------------
// Compose / export
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeArgs {
    pub picks: Vec<SlidePick>,
    pub output_path: String,
    pub title: String,
    pub include_notes: bool,
    /// How to fit aspect-mismatched slides. Absent (the frontend omits it unless
    /// the tray actually mixes aspect ratios) means "don't scale, just warn".
    #[serde(default)]
    pub fit_mode: Option<FitMode>,
}

#[tauri::command]
pub async fn compose_deck(
    state: State<'_, AppState>,
    args: ComposeArgs,
) -> Result<ComposeReport, String> {
    // Composition is pure filesystem/CPU work with no `Library` dependency, so
    // run it on a blocking thread; the mutex is only touched afterwards to
    // record the export for the stats view.
    let ComposeArgs {
        picks,
        output_path,
        title,
        include_notes,
        fit_mode,
    } = args;
    let record_title = title.clone();
    let record_picks = picks.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let opts = ComposeOptions {
            title,
            include_notes,
            fit_mode,
        };
        compose(&picks, Path::new(&output_path), &opts).map_err(e)
    })
    .await
    .map_err(e)??;

    if let Ok(mut lib) = state.library.lock() {
        let _ = lib.record_export(
            &report.output_path,
            &record_title,
            report.slides_written as i64,
            report.source_decks as i64,
            &record_picks,
        );
    }
    Ok(report)
}

// ---------------------------------------------------------------------------
// Drag a slide out of the app (WS-G, macOS-first)
// ---------------------------------------------------------------------------

/// Pixel width of the PNG drag-preview icon. Small — it's only the thumbnail
/// shown under the cursor during the OS drag session.
const DRAGOUT_ICON_PX: u32 = 160;

/// Max chars of the deck-stem portion of a drag-out file name, so a very long
/// deck name can't produce a filesystem-hostile path.
const DRAGOUT_MAX_STEM: usize = 80;

/// Absolute paths of the scratch files backing one drag-out: the single-slide
/// `.pptx` (the drag payload) and its PNG drag icon. Handed to the frontend,
/// which passes them to the native drag plugin. Mirrors `SlideDragPaths` in
/// `lib/types.ts`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideDragPaths {
    pub pptx: String,
    pub icon: String,
}


/// The pristine, user-visible file name for one slide's drag-out `.pptx` —
/// exactly what lands wherever the user drops it, so no cache tail here. The
/// content-addressing lives in the [`cache_key`]-named *subdirectory* the file
/// sits in, keeping the dropped name clean while staleness still
/// self-invalidates (an edited deck yields a new subdir).
fn dragout_pptx_name(pptx_path: &str, slide_index: usize) -> String {
    let head = sanitize_component(&deck_stem(pptx_path));
    let head: String = head.chars().take(DRAGOUT_MAX_STEM).collect();
    format!("{head} — slide {slide_index}.pptx")
}

/// The source deck's mtime in whole seconds since the Unix epoch, or 0 if it
/// can't be read (a missing deck then simply fails the compose below with a
/// real error).
fn deck_mtime_secs(pptx_path: &str) -> u64 {
    fs::metadata(pptx_path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Whether `p` is a present, non-empty regular file.
fn is_nonempty(p: &Path) -> bool {
    p.metadata().map(|m| m.len() > 0).unwrap_or(false)
}

/// RAII single-flight token for one drag-out cache key. Acquiring it blocks
/// while another prepare of the SAME key is in flight; dropping it releases the
/// key and wakes any waiters. Poison-tolerant so a panicked prepare can never
/// wedge a key permanently. It intentionally holds NO lock across the (heavy)
/// compose/render — only key membership in the shared set — so distinct keys run
/// fully in parallel and there is no lock to deadlock on.
struct DragGuard<'a> {
    inflight: &'a Mutex<HashSet<String>>,
    done: &'a Condvar,
    key: String,
}

impl<'a> DragGuard<'a> {
    fn acquire(inflight: &'a Mutex<HashSet<String>>, done: &'a Condvar, key: &str) -> Self {
        let mut set = inflight.lock().unwrap_or_else(|p| p.into_inner());
        while set.contains(key) {
            set = done.wait(set).unwrap_or_else(|p| p.into_inner());
        }
        set.insert(key.to_string());
        DragGuard { inflight, done, key: key.to_string() }
    }
}

impl Drop for DragGuard<'_> {
    fn drop(&mut self) {
        let mut set = self.inflight.lock().unwrap_or_else(|p| p.into_inner());
        set.remove(&self.key);
        drop(set);
        self.done.notify_all();
    }
}

/// Prepare the scratch files for dragging one slide out of the app as a native
/// file: a single-slide `.pptx` (full formatting, via the composer) and a small
/// PNG drag icon. They live in a [`cache_key`]-named subdirectory of
/// `app_cache/dragout` — the subdir carries the content-addressing, so the
/// `.pptx` itself keeps the pristine `<deck stem> — slide N.pptx` name the user
/// sees wherever they drop it. Returns both paths for the frontend to hand to
/// the native drag plugin.
///
/// Cheap to call repeatedly: the subdir is keyed on
/// `(deck path, slide, deck mtime)`, so a matching pair already on disk is
/// reused untouched (a second drag of the same slide is instant) and an edited
/// deck self-invalidates into a fresh subdir. The whole `dragout` dir is wiped
/// on app startup.
///
/// Both files are written atomically (compose to a temp then rename, likewise the
/// PNG) and the whole write is single-flighted per cache key, so a second drag of
/// the same slide that races the first can never read a half-written `.pptx`.
#[tauri::command]
pub async fn prepare_slide_drag(
    app: AppHandle,
    pick: SlidePick,
) -> Result<SlideDragPaths, String> {
    let SlidePick { pptx_path, slide_index } = pick;
    let key = cache_key(&pptx_path, slide_index, deck_mtime_secs(&pptx_path));
    let dir = app.state::<AppState>().dragout_dir.join(&key);

    let pptx_out = dir.join(dragout_pptx_name(&pptx_path, slide_index));
    let png_out = dir.join("icon.png");

    let paths = SlideDragPaths {
        pptx: pptx_out.to_string_lossy().into_owned(),
        icon: png_out.to_string_lossy().into_owned(),
    };

    // Fast path (no lock): both scratch files already present and non-empty.
    if is_nonempty(&pptx_out) && is_nonempty(&png_out) {
        return Ok(paths);
    }

    // Compose + rasterize off the async runtime (zip inflate + render are
    // CPU-bound), mirroring the export commands. `is_nonempty` only accepts a
    // present, non-empty file, but a partially-written one could still be
    // non-empty — hence both the atomic writes and the single-flight guard.
    let title = deck_stem(&pptx_path);
    let src = pptx_path.clone();
    let app_for_thread = app.clone();
    let pptx_out = pptx_out.clone();
    let png_out = png_out.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let state = app_for_thread.state::<AppState>();
        // Wait out any concurrent prepare of THIS key, then re-check the cache —
        // the call we waited on may have just produced both files.
        let _guard = DragGuard::acquire(&state.dragout_inflight, &state.dragout_done, &key);
        if is_nonempty(&pptx_out) && is_nonempty(&png_out) {
            return Ok(());
        }

        fs::create_dir_all(&dir).map_err(e)?;
        // Compose to a temp file, then atomically rename it into place.
        let opts = ComposeOptions { title, include_notes: false, fit_mode: None };
        let pick = SlidePick { pptx_path: src.clone(), slide_index };
        let pptx_tmp = tmp_sibling(&pptx_out);
        compose(&[pick], &pptx_tmp, &opts).map_err(e)?;
        atomic_place(&pptx_tmp, &pptx_out).map_err(e)?;

        // Rasterize the drag icon and write it atomically too. Resolve the app
        // font DB here (its first call scans all installed fonts; it also carries
        // harvested/user/downloaded faces) and open the source deck once —
        // render_slide_png would otherwise reopen it — via the pf-taking wrapper.
        let fonts = app_for_thread.state::<crate::fonts::FontsState>().db();
        let icon_pf = PresentationFile::open(Path::new(&src)).map_err(e)?;
        let png =
            render_slide_png_from(&icon_pf, slide_index, DRAGOUT_ICON_PX, &fonts).map_err(e)?;
        atomic_write(&png_out, &png).map_err(e)?;
        Ok(())
    })
    .await
    .map_err(e)??;

    Ok(paths)
}

/// Progress event streamed on `export:event` while a PDF/PNG export runs — the
/// engine fires it once per processed slide so the sheet's bar is determinate.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct ExportEvent {
    pub done: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPdfArgs {
    pub picks: Vec<SlidePick>,
    pub output_path: String,
    pub title: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportPngsArgs {
    pub picks: Vec<SlidePick>,
    pub out_dir: String,
    pub width: u32,
}

/// Distinct source-deck count among `picks` — the `source_decks` figure for the
/// stats view (an export can pull slides from several decks).
fn distinct_decks(picks: &[SlidePick]) -> usize {
    picks
        .iter()
        .map(|p| p.pptx_path.as_str())
        .collect::<std::collections::HashSet<_>>()
        .len()
}

/// Record a PNG/PDF export in the same history table `compose_deck` uses, so the
/// "Most exported" stat includes them. Best-effort — a stats write never fails
/// an otherwise-good export.
fn record_export_best_effort(
    state: &AppState,
    output_path: &str,
    title: &str,
    slide_count: usize,
    picks: &[SlidePick],
) {
    if let Ok(mut lib) = state.library.lock() {
        let _ = lib.record_export(
            output_path,
            title,
            slide_count as i64,
            distinct_decks(picks) as i64,
            picks,
        );
    }
}

#[tauri::command]
pub async fn export_tray_pdf(
    app: AppHandle,
    args: ExportPdfArgs,
) -> Result<ExportReport, String> {
    let ExportPdfArgs { picks, output_path, title } = args;
    // Title for the stats row: the chosen title, else the output file stem.
    let record_title = title.clone().unwrap_or_else(|| file_stem_or(&output_path, "Slideflow PDF"));
    let record_picks = picks.clone();
    let slide_count = picks.len();

    let app_emit = app.clone();
    let out = output_path.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        // Resolve the font DB on the blocking thread: the first call scans every
        // installed face (100–300 ms) and must not stall the IPC worker. This is
        // the app font database — system + bundled substitutes + harvested/user/
        // downloaded fonts — so a licensed app font exports as itself.
        let fonts = app_emit.state::<crate::fonts::FontsState>().db();
        let opts = PdfOptions { title };
        export_pdf(&picks, Path::new(&out), &opts, &fonts, &mut |done, total| {
            let _ = app_emit.emit("export:event", ExportEvent { done, total });
        })
        .map_err(e)
    })
    .await
    .map_err(e)??;

    let state = app.state::<AppState>();
    record_export_best_effort(&state, &output_path, &record_title, slide_count, &record_picks);
    Ok(report)
}

#[tauri::command]
pub async fn export_tray_pngs(
    app: AppHandle,
    args: ExportPngsArgs,
) -> Result<ExportReport, String> {
    let ExportPngsArgs { picks, out_dir, width } = args;
    let record_title = file_stem_or(&out_dir, "Slideflow PNGs");
    let record_picks = picks.clone();

    let app_emit = app.clone();
    let dir = out_dir.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        // Resolve the app font DB on the blocking thread (see export_tray_pdf).
        let fonts = app_emit.state::<crate::fonts::FontsState>().db();
        let opts = PngOptions { target_width_px: width };
        export_pngs(&picks, Path::new(&dir), &opts, &fonts, &mut |done, total| {
            let _ = app_emit.emit("export:event", ExportEvent { done, total });
        })
        .map_err(e)
    })
    .await
    .map_err(e)??;

    // One row per exported PNG; count the files actually written.
    let slide_count = report.files_written.len();
    let state = app.state::<AppState>();
    record_export_best_effort(&state, &out_dir, &record_title, slide_count, &record_picks);
    Ok(report)
}

/// The final path component's file stem, or `fallback` when there isn't one.
fn file_stem_or(path: &str, fallback: &str) -> String {
    Path::new(path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| fallback.to_string())
}

// ---------------------------------------------------------------------------
// Stats
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub deck_count: i64,
    pub slide_count: i64,
}

#[tauri::command]
pub async fn get_stats(state: State<'_, AppState>) -> Result<Stats, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    let (deck_count, slide_count) = lib.stats().map_err(e)?;
    Ok(Stats {
        deck_count,
        slide_count,
    })
}

/// Full stats-view payload: counts, sizes, last index run, recent activity.
#[tauri::command]
pub async fn get_stats_overview(state: State<'_, AppState>) -> Result<StatsOverview, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.stats_overview().map_err(e)
}

/// Total slides exported per deck path, for the "Most exported" browse sort.
#[tauri::command]
pub async fn get_export_counts(state: State<'_, AppState>) -> Result<HashMap<String, i64>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.export_counts().map_err(e)
}

/// Remember a settled search (called by the frontend after its debounce).
#[tauri::command]
pub async fn record_search(
    state: State<'_, AppState>,
    query: String,
    result_count: i64,
) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.record_search(&query, result_count).map_err(e)
}

// ---------------------------------------------------------------------------
// Favorites
// ---------------------------------------------------------------------------

/// Toggle a slide's favorite star; returns the new state.
#[tauri::command]
pub async fn toggle_favorite_slide(
    state: State<'_, AppState>,
    slide_id: i64,
) -> Result<bool, String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.toggle_slide_favorite(slide_id).map_err(e)
}

/// Toggle a deck's favorite star; returns the new state.
#[tauri::command]
pub async fn toggle_favorite_deck(
    state: State<'_, AppState>,
    deck_id: i64,
) -> Result<bool, String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.toggle_deck_favorite(deck_id).map_err(e)
}

// ---------------------------------------------------------------------------
// Tags
// ---------------------------------------------------------------------------

/// All tags, alphabetical, each with a live indexed-slide count.
#[tauri::command]
pub async fn list_tags(state: State<'_, AppState>) -> Result<Vec<TagRecord>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.list_tags().map_err(e)
}

/// Tags currently assigned to one slide.
#[tauri::command]
pub async fn get_slide_tags(
    state: State<'_, AppState>,
    slide_id: i64,
) -> Result<Vec<TagRecord>, String> {
    let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.slide_tags(slide_id).map_err(e)
}

/// Replace the full set of tags on a slide (creates/prunes tags as needed).
#[tauri::command]
pub async fn set_slide_tags(
    state: State<'_, AppState>,
    slide_id: i64,
    names: Vec<String>,
) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.set_slide_tags(slide_id, &names).map_err(e)
}

/// Rename a tag; errors on a case-insensitive collision.
#[tauri::command]
pub async fn rename_tag(
    state: State<'_, AppState>,
    tag_id: i64,
    name: String,
) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.rename_tag(tag_id, &name).map_err(e)
}

/// Delete a tag and all its slide assignments.
#[tauri::command]
pub async fn delete_tag(state: State<'_, AppState>, tag_id: i64) -> Result<(), String> {
    let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
    lib.delete_tag(tag_id).map_err(e)
}

// ---------------------------------------------------------------------------
// System integration
// ---------------------------------------------------------------------------

/// Reveal a file (or folder) in the OS file browser (Finder on macOS).
#[tauri::command]
pub async fn reveal_in_finder(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .reveal_item_in_dir(&path)
        .map_err(e)
}

/// Open a file with its default application.
#[tauri::command]
pub async fn open_file(app: AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_path(path, None::<&str>)
        .map_err(e)
}

/// Open a URL in the default browser.
#[tauri::command]
pub async fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    use tauri_plugin_opener::OpenerExt;
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(e)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh, unique scratch dir under the system temp (no tempfile dev-dep).
    fn scratch_dir(tag: &str) -> PathBuf {
        use std::sync::atomic::AtomicU64;
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let seq = SEQ.fetch_add(1, Ordering::Relaxed);
        let mut d = std::env::temp_dir();
        d.push(format!("slideflow-cmd-test-{}-{tag}-{seq}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn atomic_write_replaces_and_leaves_no_temp() {
        let dir = scratch_dir("atomic-write");
        let target = dir.join("out.bin");
        atomic_write(&target, b"first").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"first");
        // Overwriting is atomic and swaps the whole contents in one step.
        atomic_write(&target, b"second-longer").unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"second-longer");
        // No `.tmp-*` siblings survive a successful write.
        let leftovers: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().starts_with(".tmp-"))
            .collect();
        assert!(leftovers.is_empty(), "temp files must be renamed away, got {leftovers:?}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn atomic_place_consumes_the_source_temp() {
        let dir = scratch_dir("atomic-place");
        let tmp = tmp_sibling(&dir.join("final.bin"));
        let target = dir.join("final.bin");
        fs::write(&tmp, b"payload").unwrap();
        atomic_place(&tmp, &target).unwrap();
        assert_eq!(fs::read(&target).unwrap(), b"payload");
        assert!(!tmp.exists(), "the composed temp is renamed into place, not left behind");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn tmp_sibling_is_unique_and_colocated() {
        let final_path = Path::new("/some/dir/icon.png");
        let a = tmp_sibling(final_path);
        let b = tmp_sibling(final_path);
        assert_ne!(a, b, "the monotonic sequence makes each temp name unique");
        assert_eq!(a.parent(), final_path.parent(), "temp sits beside its target (same FS)");
    }

    #[test]
    fn scan_flag_guard_releases_on_normal_drop() {
        let flag = Arc::new(AtomicBool::new(true));
        {
            let _guard = ScanFlagGuard(Arc::clone(&flag));
            assert!(flag.load(Ordering::SeqCst), "flag stays set while the guard lives");
        }
        assert!(!flag.load(Ordering::SeqCst), "dropping the guard clears the flag");
    }

    #[test]
    fn scan_flag_guard_releases_on_panic_unwind() {
        // The whole point of the guard: a panic inside the scan thread must
        // still release `scanning`, or every later scan silently no-ops.
        let flag = Arc::new(AtomicBool::new(true));
        let flag_for_thread = Arc::clone(&flag);
        let result = std::panic::catch_unwind(move || {
            let _guard = ScanFlagGuard(flag_for_thread);
            panic!("simulated Library::scan panic");
        });
        assert!(result.is_err(), "the closure panicked");
        assert!(!flag.load(Ordering::SeqCst), "an unwind past the guard still clears the flag");
    }
}
