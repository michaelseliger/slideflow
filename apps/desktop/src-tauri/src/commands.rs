//! Tauri command surface — the bridge between the React frontend and
//! `slideflow-core`. Every command is thin: it locks the [`Library`], calls a
//! core method, maps the error to a `String`, and returns serde-serializable
//! model types straight across the IPC boundary.
//!
//! Long-running work (scanning, composing, rendering) runs on a blocking
//! thread so the async runtime and the webview never stall.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use slideflow_core::index::Library;
use slideflow_core::model::{
    ComposeReport, DeckRecord, RootRecord, SearchFilters, SearchHit, SlidePick, SlideRecord,
    StatsOverview,
};
use slideflow_core::pptx::composer::{compose, ComposeOptions};
use slideflow_core::pptx::PresentationFile;
use slideflow_core::render::{render_slide_svg, RenderOptions};
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
    /// Guards against two concurrent scans stepping on each other.
    pub scanning: AtomicBool,
    /// Bounds how many slide renders run at once. Rendering fully inflates a
    /// deck's zip and is CPU-bound, so a scroll burst of ~20 tile requests must
    /// not spawn 20 parallel 100 MB decompressions. Two keeps the pipeline fed
    /// without thrashing.
    pub render_permits: Semaphore,
    /// Tiny MRU cache of opened presentations, so scrolling one big deck's grid
    /// inflates its zip once instead of once per cache-missed slide. Keyed by
    /// `(deck_path, content_hash)`; a hash mismatch (file changed) reopens.
    pub deck_cache: Mutex<Vec<(String, String, Arc<PresentationFile>)>>,
}

/// Concurrent renders permitted at once (see `render_permits`).
const RENDER_CONCURRENCY: usize = 2;
/// Opened presentations kept resident. A 100 MB deck fully inflates to several
/// hundred MB, so this is deliberately tiny.
const DECK_CACHE_SLOTS: usize = 2;

impl AppState {
    pub fn new(library: Library, scan_library: Library, thumbs_dir: PathBuf) -> Self {
        AppState {
            library: Mutex::new(library),
            scan_library: Mutex::new(scan_library),
            thumbs_dir,
            scanning: AtomicBool::new(false),
            render_permits: Semaphore::new(RENDER_CONCURRENCY),
            deck_cache: Mutex::new(Vec::new()),
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

// ---------------------------------------------------------------------------
// Scanning (background thread, event-driven)
// ---------------------------------------------------------------------------

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
    std::thread::spawn(move || {
        let state = app_for_thread.state::<AppState>();
        let (result, valid) = {
            let mut lib = match state.scan_library.lock() {
                Ok(lib) => lib,
                Err(_) => {
                    state.scanning.store(false, Ordering::SeqCst);
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
        state.scanning.store(false, Ordering::SeqCst);
        // Reclaim orphaned cache files: decks that vanished or changed hash this
        // scan, plus any legacy `<id>.svg` files from before content-addressing.
        if let Some(valid) = valid {
            sweep_thumbs(&state.thumbs_dir, &valid);
        }
        if let Err(err) = result {
            let _ = app_for_thread.emit("scan:error", err.to_string());
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
        {
            let mut lib = state.library.lock().map_err(|_| "library lock poisoned")?;
            lib.clear().map_err(e)?;
        } // drop the interactive lock before filesystem work
          // Wipe + recreate the preview cache directory.
        let _ = std::fs::remove_dir_all(&state.thumbs_dir);
        std::fs::create_dir_all(&state.thumbs_dir).map_err(e)?;
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
#[tauri::command]
pub async fn get_slide_preview(
    state: State<'_, AppState>,
    slide_id: i64,
    tier: String,
) -> Result<String, String> {
    let tier = ThumbTier::parse(&tier);
    let (deck_path, content_hash, slide_index) = {
        let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
        lib.slide_render_info(slide_id).map_err(e)?
    };

    let file_name = thumb_file_name(&deck_path, &content_hash, slide_index as usize, tier);
    let cache_path = state.thumbs_dir.join(&file_name);

    // Fast path: a present, non-empty cache file is served straight from disk —
    // no permit, no open, no render. Kept ahead of the semaphore so cache hits
    // never queue behind in-flight renders.
    if cache_path.metadata().map(|m| m.len() > 0).unwrap_or(false) {
        return Ok(cache_path.to_string_lossy().into_owned());
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

    // Render off the async runtime too — it's CPU-bound.
    let options = match tier {
        ThumbTier::Thumb => RenderOptions::thumb(),
        ThumbTier::Full => RenderOptions::preview(),
    };
    let idx = slide_index as usize;
    let svg = tauri::async_runtime::spawn_blocking(move || {
        render_slide_svg(&pf, idx, &options).map_err(e)
    })
    .await
    .map_err(e)??;

    write_cache_atomic(&state.thumbs_dir, &cache_path, svg.as_bytes());

    Ok(cache_path.to_string_lossy().into_owned())
}

/// Write `bytes` to `final_path` atomically: a uniquely named temp file in the
/// same directory, then `rename` over the target (atomic on one filesystem). A
/// crash mid-write can therefore never leave a partial file that the fast path
/// would later serve as a valid cache hit. Best-effort — the cache is only an
/// optimization, so I/O errors are swallowed.
fn write_cache_atomic(dir: &Path, final_path: &Path, bytes: &[u8]) {
    use std::sync::atomic::AtomicU64;
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let _ = fs::create_dir_all(dir);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    let tmp = dir.join(format!(".tmp-{}-{}", std::process::id(), seq));
    if fs::write(&tmp, bytes).is_ok() && fs::rename(&tmp, final_path).is_err() {
        let _ = fs::remove_file(&tmp);
    }
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
    } = args;
    let record_title = title.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let opts = ComposeOptions {
            title,
            include_notes,
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
        );
    }
    Ok(report)
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
