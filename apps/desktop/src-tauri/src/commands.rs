//! Tauri command surface — the bridge between the React frontend and
//! `slideflow-core`. Every command is thin: it locks the [`Library`], calls a
//! core method, maps the error to a `String`, and returns serde-serializable
//! model types straight across the IPC boundary.
//!
//! Long-running work (scanning, composing, rendering) runs on a blocking
//! thread so the async runtime and the webview never stall.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::Semaphore;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use slideflow_core::dragout::cache_key;
use slideflow_core::export::{
    export_pdf, export_pngs, render_slide_png, system_fonts, PdfOptions, PngOptions,
};
use slideflow_core::index::Library;
use slideflow_core::model::{
    ComposeReport, DeckRecord, ExportReport, FitMode, RootRecord, SavedSearch, SearchFilters,
    SearchHit, SlidePick, SlideRecord, StatsOverview,
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

    // Render off the async runtime too — it's CPU-bound.
    let options = match tier {
        ThumbTier::Thumb => RenderOptions::thumb(),
        ThumbTier::Full => RenderOptions::preview(),
    };
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

    write_cache_atomic(&state.thumbs_dir, &cache_path, outcome.svg.as_bytes());

    Ok(SlidePreview {
        path: cache_path.to_string_lossy().into_owned(),
        dropped: outcome.dropped,
    })
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

/// Deck file stem (name without extension) for user-facing names/titles.
/// Local mirror of `export.rs`'s private helper.
fn deck_stem(pptx_path: &str) -> String {
    Path::new(pptx_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "deck".to_string())
}

/// Replace path separators / colons and drop control characters so a stem is
/// safe as a single path component. Local mirror of `export.rs`'s private
/// `sanitize` (deliberately not re-exported from that module).
fn sanitize_component(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    cleaned.trim().to_string()
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
#[tauri::command]
pub async fn prepare_slide_drag(
    app: AppHandle,
    pick: SlidePick,
) -> Result<SlideDragPaths, String> {
    let SlidePick { pptx_path, slide_index } = pick;
    let key = cache_key(&pptx_path, slide_index, deck_mtime_secs(&pptx_path));
    let dir = app.state::<AppState>().dragout_dir.join(key);

    let pptx_out = dir.join(dragout_pptx_name(&pptx_path, slide_index));
    let png_out = dir.join("icon.png");

    let paths = SlideDragPaths {
        pptx: pptx_out.to_string_lossy().into_owned(),
        icon: png_out.to_string_lossy().into_owned(),
    };

    // Cache hit: both scratch files already present and non-empty — reuse them.
    if is_nonempty(&pptx_out) && is_nonempty(&png_out) {
        return Ok(paths);
    }

    // Compose + rasterize off the async runtime (zip inflate + render are
    // CPU-bound), mirroring the export commands.
    let title = deck_stem(&pptx_path);
    let fonts = system_fonts();
    let src = pptx_path.clone();
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        fs::create_dir_all(&dir).map_err(e)?;
        let opts = ComposeOptions { title, include_notes: false, fit_mode: None };
        let pick = SlidePick { pptx_path: src.clone(), slide_index };
        compose(&[pick], &pptx_out, &opts).map_err(e)?;
        let png = render_slide_png(Path::new(&src), slide_index, DRAGOUT_ICON_PX, &fonts).map_err(e)?;
        fs::write(&png_out, &png).map_err(e)?;
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

    let fonts = system_fonts();
    let app_emit = app.clone();
    let out = output_path.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
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

    let fonts = system_fonts();
    let app_emit = app.clone();
    let dir = out_dir.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
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
