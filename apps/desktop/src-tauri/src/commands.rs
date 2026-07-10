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

// ---------------------------------------------------------------------------
// CLI companion install (Settings → Advanced → "Install command line tool")
// ---------------------------------------------------------------------------

/// Result of [`install_cli`]: where the `slideflow` command was linked, the
/// scope used, and a ready-to-toast summary. Mirrors the TS `InstallCliResult`.
#[derive(Debug, Serialize)]
pub struct InstallCliResult {
    /// Absolute path of the created `slideflow` symlink.
    pub path: String,
    /// Which scope was installed: "system" or "user".
    pub scope: String,
    /// True when a shell rc file was modified, so the user must open a new
    /// terminal (or `source` it) before `slideflow` resolves.
    pub restart_shell: bool,
    /// One-line, ready-to-toast summary of what happened.
    pub note: String,
}

/// Install the bundled `slideflow` CLI onto the user's PATH by symlinking it.
///
/// `scope`:
///   * `"system"` → `/usr/local/bin/slideflow` (on the default macOS PATH via
///     `/etc/paths`). Links directly when that dir is writable, otherwise (macOS)
///     escalates once through the native admin prompt.
///   * `"user"` → `~/.local/bin/slideflow`, also adding that dir to the shell
///     PATH (zsh/bash rc), since macOS doesn't include it by default.
///
/// The link points at the CLI embedded in the app bundle (an `externalBin`
/// sidecar staged next to the main executable), so it tracks the installed app
/// and updates along with it.
#[tauri::command]
pub async fn install_cli(scope: String) -> Result<InstallCliResult, String> {
    // Filesystem work (and, on macOS, possibly an osascript admin prompt) —
    // keep it off the async IPC worker.
    tauri::async_runtime::spawn_blocking(move || install_cli_impl(&scope))
        .await
        .map_err(e)?
}

#[cfg(unix)]
fn install_cli_impl(scope: &str) -> Result<InstallCliResult, String> {
    let target = bundled_cli_path()?;
    match scope {
        "system" => install_system(&target),
        "user" => install_user(&target, &home_dir()?),
        other => Err(format!("unknown install scope {other:?}")),
    }
}

#[cfg(not(unix))]
fn install_cli_impl(_scope: &str) -> Result<InstallCliResult, String> {
    Err("Installing the command-line tool isn't supported on this platform yet.".into())
}

/// Absolute path of the CLI binary embedded in the app bundle. Tauri stages an
/// `externalBin` next to the main executable with the target-triple suffix
/// stripped, so it sits beside us as `slideflow-cli`.
#[cfg(unix)]
fn bundled_cli_path() -> Result<PathBuf, String> {
    // `beforeDevCommand` now stages the sidecar next to the debug exe, so the
    // old "file is missing" heuristic no longer catches `tauri dev`. Refuse
    // explicitly: a symlink into target/debug dangles after `cargo clean`.
    // `tauri::is_dev()` is true exactly for non-bundled builds.
    if tauri::is_dev() {
        return Err("The command-line tool is only available in the installed Slideflow app, not in `tauri dev`.".into());
    }
    let exe = std::env::current_exe().map_err(e)?;
    // App Translocation: a quarantined app launched from Downloads is mounted
    // read-only at a random /private/var/folders/…/AppTranslocation/… path. A
    // symlink into that would dangle once the app is moved, so refuse.
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        return Err(
            "Move Slideflow to your Applications folder and reopen it, then try again.".into(),
        );
    }
    // Linux AppImage: `current_exe()` resolves inside the transient
    // `/tmp/.mount_*` squashfs mount, so an installed symlink dangles once the
    // app quits. The AppImage runtime advertises itself via `$APPIMAGE`
    // (mirrors updates.rs). No macOS effect — the var is never set there.
    if std::env::var_os("APPIMAGE").is_some() {
        return Err(
            "The command-line tool can't be installed from an AppImage. Install the .deb or .rpm build of Slideflow, then try again.".into(),
        );
    }
    let dir = exe
        .parent()
        .ok_or_else(|| "could not resolve the app directory".to_string())?;
    let cli = dir.join("slideflow-cli");
    if cli.exists() {
        Ok(cli)
    } else {
        Err("The command-line tool couldn't be found next to the Slideflow app.".into())
    }
}

#[cfg(unix)]
const SYSTEM_BIN_DIR: &str = "/usr/local/bin";

/// Distinguishes "can't write without elevated privileges" (→ escalate) from a
/// genuine error we should surface verbatim.
#[cfg(unix)]
enum LinkErr {
    NeedsPrivilege,
    Other(String),
}

/// File name of the bundled CLI a Slideflow-owned symlink points at. Used to
/// tell our own link apart from an unrelated `slideflow` at the same path.
#[cfg(unix)]
const CLI_LINK_TARGET_NAME: &str = "slideflow-cli";

/// Whether we may replace whatever currently sits at `link`. We only ever
/// touch a path that is missing, or a symlink Slideflow itself created (its
/// target file name is `slideflow-cli`, which also covers a stale link from a
/// previous install). Anything else — a real binary, a personal script, a
/// prior `cargo install` — must be left alone.
#[cfg(unix)]
fn link_is_replaceable(link: &Path) -> Result<bool, String> {
    use std::io::ErrorKind;
    let meta = match fs::symlink_metadata(link) {
        Ok(m) => m,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(true),
        Err(err) => return Err(format!("inspecting {}: {err}", link.display())),
    };
    if !meta.file_type().is_symlink() {
        return Ok(false);
    }
    match fs::read_link(link) {
        Ok(dest) => Ok(dest
            .file_name()
            .is_some_and(|n| n == CLI_LINK_TARGET_NAME)),
        Err(err) => Err(format!("reading link {}: {err}", link.display())),
    }
}

/// Message shown when an unrelated `slideflow` occupies the install path. We
/// won't delete it for the user — they have to remove it deliberately.
#[cfg(unix)]
fn unrelated_binary_error(link: &Path) -> String {
    format!(
        "A different \u{201c}slideflow\u{201d} already exists at {}. Remove it manually, then try again.",
        link.display()
    )
}

/// Point `link` at `target`, replacing an existing Slideflow-owned symlink (or
/// creating a fresh one). Refuses to clobber an unrelated `slideflow`. Reports
/// permission problems as [`LinkErr::NeedsPrivilege`] so the caller can decide
/// whether to escalate.
#[cfg(unix)]
fn try_replace_symlink(target: &Path, link: &Path) -> Result<(), LinkErr> {
    use std::io::ErrorKind;
    // Never delete an unrelated binary. This needs no elevated privileges, so
    // it runs before any escalation path.
    match link_is_replaceable(link) {
        Ok(true) => {}
        Ok(false) => return Err(LinkErr::Other(unrelated_binary_error(link))),
        Err(msg) => return Err(LinkErr::Other(msg)),
    }
    if let Some(parent) = link.parent() {
        if !parent.exists() {
            match fs::create_dir_all(parent) {
                Ok(()) => {}
                Err(err) if err.kind() == ErrorKind::PermissionDenied => {
                    return Err(LinkErr::NeedsPrivilege)
                }
                Err(err) => {
                    return Err(LinkErr::Other(format!(
                        "creating {}: {err}",
                        parent.display()
                    )))
                }
            }
        }
    }
    match fs::remove_file(link) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) if err.kind() == ErrorKind::PermissionDenied => {
            return Err(LinkErr::NeedsPrivilege)
        }
        Err(err) => {
            return Err(LinkErr::Other(format!(
                "removing existing {}: {err}",
                link.display()
            )))
        }
    }
    match std::os::unix::fs::symlink(target, link) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == ErrorKind::PermissionDenied => Err(LinkErr::NeedsPrivilege),
        Err(err) => Err(LinkErr::Other(format!("linking {}: {err}", link.display()))),
    }
}

#[cfg(unix)]
fn install_system(target: &Path) -> Result<InstallCliResult, String> {
    let link = Path::new(SYSTEM_BIN_DIR).join("slideflow");
    match try_replace_symlink(target, &link) {
        Ok(()) => Ok(system_result(&link)),
        Err(LinkErr::Other(msg)) => Err(msg),
        Err(LinkErr::NeedsPrivilege) => {
            #[cfg(target_os = "macos")]
            {
                // The elevated `ln` runs as root and would clobber anything;
                // re-run the ownership check (no root needed) so we never
                // delete an unrelated `slideflow` with admin rights.
                match link_is_replaceable(&link) {
                    Ok(true) => {}
                    Ok(false) => return Err(unrelated_binary_error(&link)),
                    Err(msg) => return Err(msg),
                }
                symlink_with_admin(target, &link)?;
                Ok(system_result(&link))
            }
            #[cfg(not(target_os = "macos"))]
            {
                Err(format!(
                    "{SYSTEM_BIN_DIR} isn't writable. Use the \u{201c}just for me\u{201d} option, or create the link with sudo."
                ))
            }
        }
    }
}

#[cfg(unix)]
fn system_result(link: &Path) -> InstallCliResult {
    InstallCliResult {
        path: link.display().to_string(),
        scope: "system".into(),
        restart_shell: false,
        note: format!(
            "Installed the \u{201c}slideflow\u{201d} command at {}. Open a new terminal to use it.",
            link.display()
        ),
    }
}

#[cfg(unix)]
fn install_user(target: &Path, home: &Path) -> Result<InstallCliResult, String> {
    let dir = home.join(".local").join("bin");
    let link = dir.join("slideflow");
    match try_replace_symlink(target, &link) {
        Ok(()) => {}
        Err(LinkErr::NeedsPrivilege) => {
            return Err(format!(
                "could not write to {} (permission denied)",
                dir.display()
            ))
        }
        Err(LinkErr::Other(msg)) => return Err(msg),
    }
    // ~/.local/bin is not on the default macOS PATH — add it to the shell
    // config appropriate for the user's shell.
    let (note, restart_shell) = match ensure_local_bin_on_path(home)? {
        PathSetup::Configured(file) => (
            format!(
                "Installed the \u{201c}slideflow\u{201d} command at {}, and added ~/.local/bin to your PATH in {}. Open a new terminal to use it.",
                link.display(),
                file
            ),
            true,
        ),
        PathSetup::AlreadyConfigured => (
            format!(
                "Installed the \u{201c}slideflow\u{201d} command at {}.",
                link.display()
            ),
            false,
        ),
        PathSetup::ManualNeeded => (
            format!(
                "Installed the \u{201c}slideflow\u{201d} command at {}. Add ~/.local/bin to your PATH to use it.",
                link.display()
            ),
            false,
        ),
    };
    Ok(InstallCliResult {
        path: link.display().to_string(),
        scope: "user".into(),
        restart_shell,
        note,
    })
}

/// Marker line prefixing the block we append, so a re-run recognises its own
/// edit even if the user later reformats the `export`/`fish_add_path` line.
#[cfg(unix)]
const SLIDEFLOW_MARKER: &str = "# Added by Slideflow";

/// Which shell-config file (if any) to edit to put `~/.local/bin` on PATH.
#[cfg(unix)]
#[derive(Debug)]
enum RcTarget {
    /// A POSIX rc file to append an `export PATH=…` block to (zsh, bash).
    Posix(PathBuf),
    /// A fish `conf.d` snippet to create with `fish_add_path`.
    Fish(PathBuf),
    /// Shell not recognised — we won't touch any file.
    Unknown,
}

/// Outcome of trying to put `~/.local/bin` on PATH.
#[cfg(unix)]
#[derive(Debug)]
enum PathSetup {
    /// A shell config was written/appended; carries its display path.
    Configured(String),
    /// A live (non-comment) reference was already present — nothing to do.
    AlreadyConfigured,
    /// The shell couldn't be determined; the user must add it themselves.
    ManualNeeded,
}

/// Choose the shell-config file from the `$SHELL` basename. Pure (no IO), so it
/// is unit-testable across shells without touching the environment.
#[cfg(unix)]
fn rc_target_for_shell(shell: &str, home: &Path, zdotdir: Option<&Path>) -> RcTarget {
    // `$SHELL` is an absolute path like `/bin/zsh`; match on its file name.
    let name = Path::new(shell)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    match name {
        "zsh" => RcTarget::Posix(zdotdir.unwrap_or(home).join(".zshrc")),
        // Login shells read `~/.bash_profile`; on Linux, non-login terminals
        // read `~/.bashrc` instead. macOS Terminal opens login shells.
        "bash" => {
            let file = if cfg!(target_os = "macos") {
                ".bash_profile"
            } else {
                ".bashrc"
            };
            RcTarget::Posix(home.join(file))
        }
        "fish" => RcTarget::Fish(home.join(".config/fish/conf.d/slideflow.fish")),
        _ => RcTarget::Unknown,
    }
}

/// The block appended to a POSIX rc file. Leading blank line separates it from
/// whatever preceded it; the marker lets a re-run detect its own edit.
#[cfg(unix)]
fn posix_export_block() -> String {
    format!("\n{SLIDEFLOW_MARKER} \u{2014} put the slideflow CLI on your PATH\nexport PATH=\"$HOME/.local/bin:$PATH\"\n")
}

/// The snippet written into fish's `conf.d`.
#[cfg(unix)]
fn fish_path_block() -> String {
    format!("\n{SLIDEFLOW_MARKER} \u{2014} put the slideflow CLI on your PATH\nfish_add_path -g $HOME/.local/bin\n")
}

/// Whether `file` already puts `~/.local/bin` on PATH. True when it carries our
/// marker, or any *live* (non-comment) line mentions `.local/bin`. A missing
/// file is not configured; other read errors propagate. Reads raw bytes and
/// decodes lossily so non-UTF-8 shell configs never derail the check.
#[cfg(unix)]
fn already_on_path(file: &Path) -> Result<bool, String> {
    use std::io::ErrorKind;
    let bytes = match fs::read(file) {
        Ok(b) => b,
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(format!("reading {}: {err}", file.display())),
    };
    let text = String::from_utf8_lossy(&bytes);
    if text.contains(SLIDEFLOW_MARKER) {
        return Ok(true);
    }
    Ok(text.lines().any(|line| {
        let t = line.trim_start();
        !t.starts_with('#') && t.contains(".local/bin")
    }))
}

/// Append `block` to `file`, creating it (and any missing parent dirs) first.
/// Crucially, this only ever *appends* — existing content, UTF-8 or not, can
/// never be lost.
#[cfg(unix)]
fn append_block(file: &Path, block: &str) -> Result<(), String> {
    use std::io::Write;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent).map_err(|err| format!("creating {}: {err}", parent.display()))?;
    }
    let mut f = fs::OpenOptions::new()
        .append(true)
        .create(true)
        .open(file)
        .map_err(|err| format!("opening {}: {err}", file.display()))?;
    f.write_all(block.as_bytes())
        .map_err(|err| format!("updating {}: {err}", file.display()))
}

/// Ensure `~/.local/bin` is on PATH, using the shell named by `shell` (a
/// `$SHELL`-style path). Idempotent; never destroys existing config.
#[cfg(unix)]
fn ensure_local_bin_on_path_for(
    shell: &str,
    home: &Path,
    zdotdir: Option<&Path>,
) -> Result<PathSetup, String> {
    let (file, block) = match rc_target_for_shell(shell, home, zdotdir) {
        RcTarget::Posix(f) => (f, posix_export_block()),
        RcTarget::Fish(f) => (f, fish_path_block()),
        RcTarget::Unknown => return Ok(PathSetup::ManualNeeded),
    };
    if already_on_path(&file)? {
        return Ok(PathSetup::AlreadyConfigured);
    }
    append_block(&file, &block)?;
    Ok(PathSetup::Configured(file.display().to_string()))
}

/// Ensure `~/.local/bin` is on PATH for the current shell (`$SHELL`, honouring
/// `$ZDOTDIR` for zsh). See [`ensure_local_bin_on_path_for`].
#[cfg(unix)]
fn ensure_local_bin_on_path(home: &Path) -> Result<PathSetup, String> {
    let shell = std::env::var("SHELL").unwrap_or_default();
    let zdotdir = std::env::var_os("ZDOTDIR")
        .filter(|z| !z.is_empty())
        .map(PathBuf::from);
    ensure_local_bin_on_path_for(&shell, home, zdotdir.as_deref())
}

#[cfg(unix)]
fn home_dir() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .ok_or_else(|| "could not determine your home directory".to_string())
}

/// Create the `/usr/local/bin/slideflow` symlink with a one-time macOS admin
/// prompt (used when the directory isn't user-writable).
#[cfg(target_os = "macos")]
fn symlink_with_admin(target: &Path, link: &Path) -> Result<(), String> {
    let dir = link.parent().unwrap_or_else(|| Path::new(SYSTEM_BIN_DIR));
    // POSIX shell command, each path single-quoted for the shell.
    let shell_cmd = format!(
        "mkdir -p {} && ln -sf {} {}",
        sh_single_quote(&dir.to_string_lossy()),
        sh_single_quote(&target.to_string_lossy()),
        sh_single_quote(&link.to_string_lossy()),
    );
    // Embed that as an AppleScript string literal (escape \ and ").
    let script = format!(
        "do shell script \"{}\" with administrator privileges",
        applescript_escape(&shell_cmd)
    );
    let out = std::process::Command::new("osascript")
        .arg("-e")
        .arg(&script)
        .output()
        .map_err(|err| format!("could not run osascript: {err}"))?;
    if out.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&out.stderr);
    // AppleScript error -128 == the user clicked Cancel on the auth dialog.
    if stderr.contains("-128") || stderr.contains("User canceled") {
        Err("Installation cancelled.".into())
    } else {
        Err(format!("admin install failed: {}", stderr.trim()))
    }
}

/// Wrap `s` in single quotes for a POSIX shell, escaping embedded single quotes.
#[cfg(target_os = "macos")]
fn sh_single_quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for ch in s.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
    out
}

/// Escape a string for use inside an AppleScript double-quoted literal.
#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
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

    #[cfg(unix)]
    #[test]
    fn user_install_symlinks_and_is_idempotent() {
        let home = scratch_dir("cli-user-install");
        // A stand-in for the bundled CLI binary.
        let target = home.join("slideflow-cli");
        fs::write(&target, b"#!/bin/sh\n").unwrap();

        let res = install_user(&target, &home).unwrap();
        assert_eq!(res.scope, "user");
        let link = home.join(".local/bin/slideflow");
        assert!(link.is_symlink(), "expected a symlink at {}", link.display());
        assert_eq!(fs::read_link(&link).unwrap(), target);

        // Re-running relinks the Slideflow-owned symlink and never re-touches a
        // shell config, so `restart_shell` is false the second time regardless
        // of which shell the test runs under. (PATH-file idempotence is covered
        // deterministically by `zsh_path_setup_appends_once_and_is_idempotent`.)
        let res2 = install_user(&target, &home).unwrap();
        assert!(!res2.restart_shell, "second run must not re-touch a shell config");
        assert_eq!(fs::read_link(&link).unwrap(), target);

        let _ = fs::remove_dir_all(&home);
    }

    /// An unrelated `slideflow` at the install path must never be deleted.
    #[cfg(unix)]
    #[test]
    fn user_install_refuses_to_clobber_unrelated_binary() {
        let home = scratch_dir("cli-clobber");
        let target = home.join("slideflow-cli");
        fs::write(&target, b"#!/bin/sh\n").unwrap();

        // Someone's own `slideflow` (a plain file, not our symlink) sits there.
        let bin_dir = home.join(".local/bin");
        fs::create_dir_all(&bin_dir).unwrap();
        let existing = bin_dir.join("slideflow");
        fs::write(&existing, b"my own script\n").unwrap();

        let err = install_user(&target, &home).unwrap_err();
        assert!(err.contains("already exists"), "unexpected error: {err}");
        // The user's file is untouched.
        assert_eq!(fs::read(&existing).unwrap(), b"my own script\n");
        assert!(!existing.is_symlink());

        let _ = fs::remove_dir_all(&home);
    }

    /// A stale Slideflow-owned symlink (target file name `slideflow-cli`) is
    /// replaceable, even when it dangles.
    #[cfg(unix)]
    #[test]
    fn stale_slideflow_symlink_is_replaceable() {
        let dir = scratch_dir("cli-stale-link");
        let link = dir.join("slideflow");
        // A dangling link to a nonexistent `…/slideflow-cli` (a previous install).
        std::os::unix::fs::symlink(dir.join("gone/slideflow-cli"), &link).unwrap();
        assert!(link_is_replaceable(&link).unwrap());

        // A symlink to something else is NOT ours.
        let other = dir.join("other");
        std::os::unix::fs::symlink(dir.join("elsewhere/brew-slideflow"), &other).unwrap();
        assert!(!link_is_replaceable(&other).unwrap());

        // A plain file is not replaceable; a missing path is.
        let plain = dir.join("plain");
        fs::write(&plain, b"x").unwrap();
        assert!(!link_is_replaceable(&plain).unwrap());
        assert!(link_is_replaceable(&dir.join("missing")).unwrap());

        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn rc_target_follows_the_shell_basename() {
        let home = Path::new("/home/u");
        // zsh honours $ZDOTDIR, else falls back to $HOME.
        assert!(matches!(
            rc_target_for_shell("/bin/zsh", home, None),
            RcTarget::Posix(p) if p == home.join(".zshrc")
        ));
        let zdot = Path::new("/cfg/zsh");
        assert!(matches!(
            rc_target_for_shell("/usr/bin/zsh", home, Some(zdot)),
            RcTarget::Posix(p) if p == zdot.join(".zshrc")
        ));
        // bash: platform-appropriate login rc.
        let bash_rc = if cfg!(target_os = "macos") {
            ".bash_profile"
        } else {
            ".bashrc"
        };
        assert!(matches!(
            rc_target_for_shell("/bin/bash", home, None),
            RcTarget::Posix(p) if p == home.join(bash_rc)
        ));
        // fish uses a conf.d snippet.
        assert!(matches!(
            rc_target_for_shell("/usr/local/bin/fish", home, None),
            RcTarget::Fish(p) if p == home.join(".config/fish/conf.d/slideflow.fish")
        ));
        // Unknown or empty shells are left untouched.
        assert!(matches!(rc_target_for_shell("/bin/tcsh", home, None), RcTarget::Unknown));
        assert!(matches!(rc_target_for_shell("", home, None), RcTarget::Unknown));
    }

    #[cfg(unix)]
    #[test]
    fn zsh_path_setup_appends_once_and_is_idempotent() {
        let home = scratch_dir("cli-zsh-path");
        match ensure_local_bin_on_path_for("zsh", &home, None).unwrap() {
            PathSetup::Configured(f) => assert!(f.ends_with(".zshrc"), "wrote {f}"),
            other => panic!("expected Configured, got {other:?}"),
        }
        let rc = home.join(".zshrc");
        let first = fs::read_to_string(&rc).unwrap();
        assert!(first.contains(SLIDEFLOW_MARKER), "marker must be present");
        assert_eq!(first.matches(".local/bin").count(), 1);

        // Second run detects its own marker and does nothing.
        assert!(matches!(
            ensure_local_bin_on_path_for("zsh", &home, None).unwrap(),
            PathSetup::AlreadyConfigured
        ));
        assert_eq!(fs::read_to_string(&rc).unwrap(), first, "rc must be unchanged");

        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn fish_path_setup_writes_confd_snippet() {
        let home = scratch_dir("cli-fish-path");
        match ensure_local_bin_on_path_for("/usr/bin/fish", &home, None).unwrap() {
            PathSetup::Configured(f) => assert!(f.ends_with("conf.d/slideflow.fish"), "wrote {f}"),
            other => panic!("expected Configured, got {other:?}"),
        }
        let snippet = home.join(".config/fish/conf.d/slideflow.fish");
        let body = fs::read_to_string(&snippet).unwrap();
        assert!(body.contains("fish_add_path -g $HOME/.local/bin"));
        // Re-running is a no-op.
        assert!(matches!(
            ensure_local_bin_on_path_for("fish", &home, None).unwrap(),
            PathSetup::AlreadyConfigured
        ));
        assert_eq!(fs::read_to_string(&snippet).unwrap(), body);

        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(unix)]
    #[test]
    fn unknown_shell_touches_nothing() {
        let home = scratch_dir("cli-unknown-shell");
        assert!(matches!(
            ensure_local_bin_on_path_for("/bin/tcsh", &home, None).unwrap(),
            PathSetup::ManualNeeded
        ));
        // No files created anywhere under home.
        assert!(fs::read_dir(&home).unwrap().next().is_none(), "nothing should be written");
        let _ = fs::remove_dir_all(&home);
    }

    /// Finding 5: a commented-out line must not read as configured.
    #[cfg(unix)]
    #[test]
    fn commented_local_bin_is_not_treated_as_configured() {
        let dir = scratch_dir("already-on-path");
        let rc = dir.join(".zshrc");

        fs::write(&rc, "# export PATH=\"$HOME/.local/bin:$PATH\"\n").unwrap();
        assert!(!already_on_path(&rc).unwrap(), "a comment is not a live PATH edit");

        fs::write(&rc, "  export PATH=\"$HOME/.local/bin:$PATH\"\n").unwrap();
        assert!(already_on_path(&rc).unwrap(), "an indented real export line counts");

        // Our marker counts even if the user later edits the export line.
        fs::write(&rc, format!("{SLIDEFLOW_MARKER}\n# (line removed)\n")).unwrap();
        assert!(already_on_path(&rc).unwrap(), "our marker counts");

        assert!(!already_on_path(&dir.join("nope")).unwrap(), "a missing file is not configured");

        let _ = fs::remove_dir_all(&dir);
    }

    /// Finding 1 (the destructive bug): a non-UTF-8 rc file must be appended to,
    /// never rewritten — every original byte survives.
    #[cfg(unix)]
    #[test]
    fn non_utf8_rc_content_is_preserved_when_appending() {
        let home = scratch_dir("cli-latin1-rc");
        let rc = home.join(".zshrc");
        // A Latin-1 comment (0xFC='ü', 0xDF='ß') that breaks read_to_string.
        let original: &[u8] = b"# gr\xFC\xDFe\nalias ll='ls -l'\n";
        fs::write(&rc, original).unwrap();

        assert!(matches!(
            ensure_local_bin_on_path_for("zsh", &home, None).unwrap(),
            PathSetup::Configured(_)
        ));

        let after = fs::read(&rc).unwrap();
        assert!(after.starts_with(original), "existing shell config must never be lost");
        let appended = String::from_utf8_lossy(&after);
        assert!(appended.contains(SLIDEFLOW_MARKER));
        assert!(appended.contains("export PATH=\"$HOME/.local/bin:$PATH\""));

        let _ = fs::remove_dir_all(&home);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn shell_and_applescript_quoting() {
        // Spaces and single quotes in a path survive shell single-quoting.
        assert_eq!(sh_single_quote("/Apps/My App/x"), "'/Apps/My App/x'");
        assert_eq!(sh_single_quote("a'b"), r"'a'\''b'");
        // AppleScript literal escaping of backslash and double-quote.
        assert_eq!(applescript_escape(r#"a"b\c"#), r#"a\"b\\c"#);
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
