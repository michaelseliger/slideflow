//! Local semantic search — desktop host side.
//!
//! Owns everything the pure engine must not: the **model download** (streamed
//! from huggingface.co with Range resume + pinned sha256 verification — core
//! never touches the network), the **enable preference**, the **embedder
//! bootstrap** (loading ~470 MB of weights off the UI thread and attaching the
//! embedder to both library connections), and the **embedding backfill** (a
//! background thread mirroring the `start_scan` pattern: AtomicBool guards +
//! progress events).
//!
//! Events: `model:download` carries [`ModelDownloadEvent`]; `embed:event`
//! carries [`EmbedEvent`]. Both are mirrored field-for-field (snake_case) in
//! `lib/types.ts`, same convention as `ScanEvent`/`UpdateEvent`.

use std::fs;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::{AppHandle, Emitter, Manager, State};

use slideflow_core::embed::e5::{E5Embedder, MODEL_ID};
use slideflow_core::embed::Embedder;
use slideflow_core::model::EmbeddingStatus;

use crate::commands::AppState;

/// Where the three model files come from. Downloaded once, verified against the
/// pinned hashes below, then loaded from disk forever after.
const MODEL_BASE_URL: &str =
    "https://huggingface.co/intfloat/multilingual-e5-small/resolve/main";

/// The model's embedding dimensionality (multilingual-e5-small).
const MODEL_DIMS: i64 = 384;

/// One downloadable model file with its pinned integrity data.
struct ModelFile {
    name: &'static str,
    sha256: &'static str,
    size: u64,
}

/// Pinned contents of `intfloat/multilingual-e5-small` (MIT license), fetched
/// 2026-07-05. A hash mismatch after download → the file is deleted and the
/// download errors; we never load unverified weights. Total ≈ 488 MB.
const MODEL_FILES: [ModelFile; 3] = [
    ModelFile {
        name: "model.safetensors",
        sha256: "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
        size: 470_641_600,
    },
    ModelFile {
        name: "tokenizer.json",
        sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
        size: 17_082_730,
    },
    ModelFile {
        name: "config.json",
        sha256: "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
        size: 655,
    },
];

/// Tauri-managed state for the semantic subsystem.
pub struct SemanticState {
    /// `<app_data>/models/multilingual-e5-small`.
    pub model_dir: PathBuf,
    /// Re-entrancy guard + liveness flag for the download thread.
    downloading: AtomicBool,
    download_cancel: AtomicBool,
    /// Re-entrancy guard for the backfill thread.
    backfilling: AtomicBool,
    backfill_cancel: AtomicBool,
    /// True while E5 weights are being loaded into memory.
    loading: AtomicBool,
    /// True once an embedder is attached to both library connections.
    ready: AtomicBool,
    /// Last download/load failure, surfaced via `get_embedding_status`.
    last_error: Mutex<Option<String>>,
}

impl SemanticState {
    pub fn new(model_dir: PathBuf) -> Self {
        SemanticState {
            model_dir,
            downloading: AtomicBool::new(false),
            download_cancel: AtomicBool::new(false),
            backfilling: AtomicBool::new(false),
            backfill_cancel: AtomicBool::new(false),
            loading: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            last_error: Mutex::new(None),
        }
    }

    fn set_error(&self, message: Option<String>) {
        if let Ok(mut guard) = self.last_error.lock() {
            *guard = message;
        }
    }
}

/// Download lifecycle events on `model:download`. `canceled` is distinct from
/// `error` so the UI can reset quietly after a user-initiated cancel.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelDownloadEvent {
    Progress {
        file: String,
        downloaded: u64,
        total: u64,
        overall_downloaded: u64,
        overall_total: u64,
    },
    Done,
    Canceled,
    Error { message: String },
}

/// Backfill lifecycle events on `embed:event`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EmbedEvent {
    Started { total: usize },
    Progress { done: usize, total: usize },
    Finished,
    Error { message: String },
}

fn emit_download(app: &AppHandle, event: &ModelDownloadEvent) {
    let _ = app.emit("model:download", event);
}

fn emit_embed(app: &AppHandle, event: &EmbedEvent) {
    let _ = app.emit("embed:event", event);
}

// ---------------------------------------------------------------------------
// Enable preference (app_config_dir/semantic-search, "1"/"0")
// ---------------------------------------------------------------------------

fn pref_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("semantic-search"))
}

/// Missing file or anything but "1" = disabled — semantic search is opt-in
/// (unlike auto-update, which shipped always-on).
fn pref_enabled_from_str(contents: Option<&str>) -> bool {
    matches!(contents, Some(s) if s.trim() == "1")
}

pub fn semantic_enabled(app: &AppHandle) -> bool {
    pref_enabled_from_str(
        pref_path(app)
            .and_then(|p| fs::read_to_string(p).ok())
            .as_deref(),
    )
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// All three model files present (existence + exact pinned size; the sha256 was
/// verified at download time and full re-hashing 470 MB on every status poll
/// would be wasteful).
pub fn model_present(model_dir: &Path) -> bool {
    MODEL_FILES.iter().all(|f| {
        model_dir
            .join(f.name)
            .metadata()
            .map(|m| m.len() == f.size)
            .unwrap_or(false)
    })
}

#[tauri::command]
pub async fn get_embedding_status(
    app: AppHandle,
    state: State<'_, AppState>,
    semantic: State<'_, SemanticState>,
) -> Result<EmbeddingStatus, String> {
    let (embedded, total) = {
        let lib = state.library.lock().map_err(|_| "library lock poisoned")?;
        lib.embedding_counts().map_err(|e| e.to_string())?
    };
    let error = semantic.last_error.lock().ok().and_then(|g| g.clone());

    let state_str = if semantic.downloading.load(Ordering::SeqCst) {
        "downloading"
    } else if !semantic_enabled(&app) {
        "disabled"
    } else if !model_present(&semantic.model_dir) {
        "not_downloaded"
    } else if error.is_some() {
        "error"
    } else {
        // Attached, or still loading the weights (a brief startup window during
        // which semantic queries silently fall back to lexical).
        "ready"
    };

    Ok(EmbeddingStatus {
        state: state_str.to_string(),
        model_id: MODEL_ID.to_string(),
        dims: MODEL_DIMS,
        embedded_slides: embedded,
        total_slides: total,
        error,
    })
}

// ---------------------------------------------------------------------------
// Enable / disable
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn set_semantic_search_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let path = pref_path(&app).ok_or_else(|| "resolve app config dir".to_string())?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    fs::write(&path, if enabled { "1" } else { "0" }).map_err(|e| e.to_string())?;

    let semantic = app.state::<SemanticState>();
    if enabled {
        semantic.set_error(None);
        // Model already on disk → bring the embedder up (weights load on a
        // background thread) and backfill anything missing.
        if model_present(&semantic.model_dir) && !semantic.ready.load(Ordering::SeqCst) {
            spawn_embedder_bootstrap(app.clone());
        }
    } else {
        // Detach: semantic/hybrid queries degrade to lexical immediately. The
        // model files stay on disk (delete_embedding_model removes them).
        semantic.backfill_cancel.store(true, Ordering::SeqCst);
        detach_embedder(&app);
    }
    Ok(())
}

/// Remove the model files from disk and disable the feature. Rejected while a
/// download is running (cancel it first).
#[tauri::command]
pub async fn delete_embedding_model(app: AppHandle) -> Result<(), String> {
    let semantic = app.state::<SemanticState>();
    if semantic.downloading.load(Ordering::SeqCst) {
        return Err("Cancel the running download first.".into());
    }
    semantic.backfill_cancel.store(true, Ordering::SeqCst);
    detach_embedder(&app);

    if let Some(path) = pref_path(&app) {
        let _ = fs::write(path, "0");
    }
    if semantic.model_dir.exists() {
        fs::remove_dir_all(&semantic.model_dir).map_err(|e| e.to_string())?;
    }
    semantic.set_error(None);
    Ok(())
}

/// Detach the embedder from both library connections WITHOUT blocking the
/// caller. The interactive connection is detached inline (never held long, so
/// semantic/hybrid queries degrade to lexical immediately). The scan connection
/// may be held for a whole run by a backfill — the caller has already set
/// `backfill_cancel`, so rather than block the Settings toggle on that run, we
/// grab the scan connection if it's free, else hand off to a short-lived thread
/// that waits for the (now-canceling) backfill to release it and detaches then.
fn detach_embedder(app: &AppHandle) {
    let state = app.state::<AppState>();
    let semantic = app.state::<SemanticState>();
    semantic.ready.store(false, Ordering::SeqCst);
    if let Ok(mut lib) = state.library.lock() {
        lib.set_embedder(None);
    }
    // Grab the scan connection if it's free; the temporary lock result is scoped
    // to this `let` so it never outlives `state`.
    let detached_inline = if let Ok(mut lib) = state.scan_library.try_lock() {
        lib.set_embedder(None);
        true
    } else {
        // Busy (a backfill/scan holds it) or poisoned — don't wait here.
        false
    };
    if !detached_inline {
        let app = app.clone();
        std::thread::spawn(move || {
            let state = app.state::<AppState>();
            if let Ok(mut lib) = state.scan_library.lock() {
                lib.set_embedder(None);
            };
        });
    }
}

// ---------------------------------------------------------------------------
// Embedder bootstrap
// ---------------------------------------------------------------------------

/// Load E5 (heavy: ~470 MB of weights) on a background thread and attach it to
/// BOTH library connections, then kick a backfill for anything not yet embedded.
/// Called at startup (when enabled + present), after enabling, and after a
/// completed download.
pub fn spawn_embedder_bootstrap(app: AppHandle) {
    // Scope the state borrow so `app` can move into the thread closure.
    {
        let semantic = app.state::<SemanticState>();
        if semantic.loading.swap(true, Ordering::SeqCst) {
            return; // already loading
        }
    }
    std::thread::spawn(move || {
        let semantic = app.state::<SemanticState>();
        let result = E5Embedder::load(&semantic.model_dir);
        match result {
            Ok(embedder) => {
                let arc: Arc<dyn Embedder> = Arc::new(embedder);
                let state = app.state::<AppState>();
                if let Ok(mut lib) = state.library.lock() {
                    lib.set_embedder(Some(Arc::clone(&arc)));
                }
                if let Ok(mut lib) = state.scan_library.lock() {
                    lib.set_embedder(Some(arc));
                }
                semantic.ready.store(true, Ordering::SeqCst);
                semantic.set_error(None);
                semantic.loading.store(false, Ordering::SeqCst);
                spawn_backfill_if_enabled(&app);
            }
            Err(err) => {
                semantic.set_error(Some(err.to_string()));
                semantic.loading.store(false, Ordering::SeqCst);
            }
        }
    });
}

/// Startup hook: if the user enabled semantic search and the model is on disk,
/// bring the embedder up in the background.
pub fn bootstrap_on_startup(app: &AppHandle) {
    let semantic = app.state::<SemanticState>();
    if semantic_enabled(app) && model_present(&semantic.model_dir) {
        spawn_embedder_bootstrap(app.clone());
    }
}

// ---------------------------------------------------------------------------
// Model download (streamed, Range-resumable, sha256-verified)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn download_embedding_model(app: AppHandle) -> Result<bool, String> {
    let semantic = app.state::<SemanticState>();
    if semantic
        .downloading
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Ok(false); // already running — not an error
    }
    semantic.download_cancel.store(false, Ordering::SeqCst);
    semantic.set_error(None);

    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        let semantic = app_for_thread.state::<SemanticState>();
        let result = run_download(&app_for_thread, &semantic);
        semantic.downloading.store(false, Ordering::SeqCst);
        match result {
            Ok(true) => {
                emit_download(&app_for_thread, &ModelDownloadEvent::Done);
                // Model just landed: if the feature is enabled, bring it up and
                // index the library.
                if semantic_enabled(&app_for_thread) {
                    spawn_embedder_bootstrap(app_for_thread.clone());
                }
            }
            Ok(false) => {
                emit_download(&app_for_thread, &ModelDownloadEvent::Canceled);
            }
            Err(message) => {
                semantic.set_error(Some(message.clone()));
                emit_download(&app_for_thread, &ModelDownloadEvent::Error { message });
            }
        }
    });
    Ok(true)
}

#[tauri::command]
pub async fn cancel_model_download(semantic: State<'_, SemanticState>) -> Result<(), String> {
    semantic.download_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// Download every missing model file. Returns `Ok(false)` on user cancel,
/// `Ok(true)` when all files are present and verified.
fn run_download(app: &AppHandle, semantic: &SemanticState) -> Result<bool, String> {
    fs::create_dir_all(&semantic.model_dir).map_err(|e| e.to_string())?;
    // The blocking client defaults to a 30s WHOLE-REQUEST timeout, which would
    // abort a 470 MB download; disable it (the connect timeout still catches
    // unreachable hosts, and any interrupted stream resumes via Range next run).
    let client = reqwest::blocking::Client::builder()
        .timeout(None)
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let overall_total: u64 = MODEL_FILES.iter().map(|f| f.size).sum();
    let mut overall_done: u64 = MODEL_FILES
        .iter()
        .filter(|f| final_file_ok(&semantic.model_dir, f))
        .map(|f| f.size)
        .sum();

    for file in MODEL_FILES.iter() {
        if final_file_ok(&semantic.model_dir, file) {
            continue; // finished in an earlier (interrupted) run
        }
        let done = download_one(app, semantic, &client, file, &mut overall_done, overall_total)?;
        if !done {
            return Ok(false); // canceled
        }
    }
    Ok(true)
}

/// The final (renamed) file exists with the pinned size.
fn final_file_ok(dir: &Path, file: &ModelFile) -> bool {
    dir.join(file.name)
        .metadata()
        .map(|m| m.len() == file.size)
        .unwrap_or(false)
}

/// What to do with a finished `.part` after a download attempt ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PartVerdict {
    /// Full pinned size and hash matches → rename into place.
    Complete,
    /// Shorter than the pin — a valid prefix from an interrupted transfer. KEEP it
    /// (a Range resume continues from here); never delete.
    Resumable,
    /// Full size but the sha256 disagrees → genuine corruption/tampering; delete
    /// and restart from zero.
    Corrupt,
}

/// Decide a `.part`'s fate purely from its on-disk size vs. the pinned size and
/// whether its hash matched. Split out from [`download_one`] so the "short read
/// keeps, size-match-bad-hash deletes" rule is unit-testable without the network.
fn classify_part(on_disk: u64, expected_size: u64, hash_matches: bool) -> PartVerdict {
    if on_disk < expected_size {
        PartVerdict::Resumable
    } else if hash_matches {
        PartVerdict::Complete
    } else {
        PartVerdict::Corrupt
    }
}

/// Stream one file to `<name>.part` (resuming via HTTP Range when a partial
/// exists), verify its sha256 against the pin, then atomically rename it into
/// place. Returns `Ok(false)` on cancel.
fn download_one(
    app: &AppHandle,
    semantic: &SemanticState,
    client: &reqwest::blocking::Client,
    file: &ModelFile,
    overall_done: &mut u64,
    overall_total: u64,
) -> Result<bool, String> {
    let part_path = semantic.model_dir.join(format!("{}.part", file.name));
    let mut downloaded = part_path.metadata().map(|m| m.len()).unwrap_or(0);
    // A partial larger than the pinned size can never verify — start over.
    if downloaded > file.size {
        let _ = fs::remove_file(&part_path);
        downloaded = 0;
    }

    if downloaded < file.size {
        let url = format!("{MODEL_BASE_URL}/{}", file.name);
        let mut request = client.get(&url);
        if downloaded > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={downloaded}-"));
        }
        let mut response = request
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("{}: {e}", file.name))?;

        // Server ignored the Range request (rare) → restart the file from zero.
        if downloaded > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            let _ = fs::remove_file(&part_path);
            downloaded = 0;
        }

        let mut out = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&part_path)
            .map_err(|e| format!("{}: {e}", file.name))?;
        if downloaded == 0 {
            out.set_len(0).map_err(|e| e.to_string())?;
            out.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        }

        // ~64 KB reads; progress throttled to ~1% steps of the overall total so
        // the IPC channel isn't flooded.
        let mut buf = vec![0u8; 64 * 1024];
        let step = (overall_total / 100).max(256 * 1024);
        let mut last_emitted = *overall_done + downloaded;
        loop {
            if semantic.download_cancel.load(Ordering::SeqCst) {
                return Ok(false); // .part stays for a future resume
            }
            let n = response
                .read(&mut buf)
                .map_err(|e| format!("{}: {e}", file.name))?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n]).map_err(|e| format!("{}: {e}", file.name))?;
            downloaded += n as u64;
            let overall_now = *overall_done + downloaded;
            if overall_now - last_emitted >= step {
                last_emitted = overall_now;
                emit_download(
                    app,
                    &ModelDownloadEvent::Progress {
                        file: file.name.to_string(),
                        downloaded,
                        total: file.size,
                        overall_downloaded: overall_now,
                        overall_total,
                    },
                );
            }
        }
        out.flush().map_err(|e| e.to_string())?;
    }

    // Classify the finished `.part` before touching it. A stream can end cleanly
    // (server closed the connection mid-transfer) with FEWER bytes than the pin —
    // that's a valid prefix, not corruption. Deleting it would throw away up to
    // ~470 MB of resume progress; instead keep it and return a retryable error so
    // the next attempt continues via HTTP Range. Only a full-size file whose
    // sha256 disagrees is genuine corruption worth deleting.
    let on_disk = part_path.metadata().map(|m| m.len()).unwrap_or(0);
    // Hashing a short prefix is pointless (it can never match the pin), so only
    // compute the digest once the file has reached the pinned size.
    let actual = if on_disk >= file.size {
        Some(sha256_of_file(&part_path).map_err(|e| format!("{}: {e}", file.name))?)
    } else {
        None
    };
    match classify_part(on_disk, file.size, actual.as_deref() == Some(file.sha256)) {
        PartVerdict::Resumable => {
            return Err(format!(
                "{}: download interrupted at {on_disk}/{} bytes — resume by retrying",
                file.name, file.size
            ));
        }
        PartVerdict::Corrupt => {
            let _ = fs::remove_file(&part_path);
            return Err(format!(
                "{}: checksum mismatch (expected {}, got {})",
                file.name,
                file.sha256,
                actual.as_deref().unwrap_or("(short read)")
            ));
        }
        PartVerdict::Complete => {}
    }
    let final_path = semantic.model_dir.join(file.name);
    fs::rename(&part_path, &final_path).map_err(|e| format!("{}: {e}", file.name))?;

    *overall_done += file.size;
    emit_download(
        app,
        &ModelDownloadEvent::Progress {
            file: file.name.to_string(),
            downloaded: file.size,
            total: file.size,
            overall_downloaded: *overall_done,
            overall_total,
        },
    );
    Ok(true)
}

fn sha256_of_file(path: &Path) -> std::io::Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

// ---------------------------------------------------------------------------
// Embedding backfill (background thread, mirrors start_scan)
// ---------------------------------------------------------------------------

#[tauri::command]
pub async fn start_embed_backfill(app: AppHandle) -> Result<bool, String> {
    Ok(spawn_backfill(&app))
}

#[tauri::command]
pub async fn cancel_embed_backfill(semantic: State<'_, SemanticState>) -> Result<(), String> {
    semantic.backfill_cancel.store(true, Ordering::SeqCst);
    Ok(())
}

/// After a scan completes (or the embedder comes up): embed whatever the
/// inline path didn't cover. No-op unless enabled + attached.
pub fn spawn_backfill_if_enabled(app: &AppHandle) {
    let semantic = app.state::<SemanticState>();
    if semantic_enabled(app) && semantic.ready.load(Ordering::SeqCst) {
        spawn_backfill(app);
    }
}

/// Start the backfill thread. Returns false (not an error) when one is already
/// running or no embedder is attached.
fn spawn_backfill(app: &AppHandle) -> bool {
    let semantic = app.state::<SemanticState>();
    if !semantic.ready.load(Ordering::SeqCst) {
        return false;
    }
    if semantic
        .backfilling
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return false;
    }
    semantic.backfill_cancel.store(false, Ordering::SeqCst);

    let app_for_thread = app.clone();
    std::thread::spawn(move || {
        let semantic = app_for_thread.state::<SemanticState>();
        let result = run_backfill(&app_for_thread, &semantic);
        semantic.backfilling.store(false, Ordering::SeqCst);
        match result {
            Ok(()) => emit_embed(&app_for_thread, &EmbedEvent::Finished),
            Err(message) => emit_embed(&app_for_thread, &EmbedEvent::Error { message }),
        }
    });
    true
}

/// The actual backfill, on the scan connection (so interactive search stays
/// live): (1) reparse decks whose slides predate hashing to fill their hashes,
/// (2) embed every distinct text still missing a vector, in chunks, honoring
/// the cancel flag, (3) clean up orphans and invalidate BOTH connections'
/// in-memory vector caches.
///
/// NB: this holds the `scan_library` mutex for the WHOLE run, so a concurrent
/// scan (`start_scan`) queues behind a long backfill (a first-time index of a
/// large library on CPU can take minutes). That is the intended two-connection
/// trade-off — searches on the interactive connection stay responsive — but it
/// is why the backfill is cancelable and chunked.
fn run_backfill(app: &AppHandle, semantic: &SemanticState) -> Result<(), String> {
    let state = app.state::<AppState>();
    let mut lib = state
        .scan_library
        .lock()
        .map_err(|_| "library lock poisoned".to_string())?;

    // Phase A: hashes for pre-hashing rows (cheap reparse, no embedding).
    let decks = lib.decks_needing_hash_backfill().map_err(|e| e.to_string())?;
    for (deck_id, path) in decks {
        if semantic.backfill_cancel.load(Ordering::SeqCst) {
            break;
        }
        // Best-effort per deck: a vanished/corrupt file must not kill the run.
        let _ = lib.backfill_deck_hashes(deck_id, &path);
    }

    // Phase B: embed all missing texts.
    let pending = lib.pending_embedding_texts().map_err(|e| e.to_string())?;
    let total = pending.len();
    emit_embed(app, &EmbedEvent::Started { total });
    let mut done = 0usize;
    // Hold onto a chunk error instead of `?`-returning it: the cleanup + cache
    // invalidation tail below MUST still run so the chunks that DID embed become
    // visible (an early return left earlier successful work invisible).
    let mut embed_error: Option<String> = None;
    for chunk in pending.chunks(32) {
        if semantic.backfill_cancel.load(Ordering::SeqCst) {
            break;
        }
        // Cancelable at batch granularity so the scan connection is released
        // promptly when the user disables the model mid-run.
        if let Err(err) = lib.embed_and_store_canceled(chunk, &semantic.backfill_cancel) {
            embed_error = Some(err.to_string());
            break;
        }
        done += chunk.len();
        emit_embed(app, &EmbedEvent::Progress { done, total });
    }

    // ALWAYS run the tail — on success, cancel, OR error — so orphan cleanup and
    // both connections' cache invalidations happen and the embedded chunks show up.
    let _ = lib.cleanup_orphan_embeddings();
    lib.invalidate_vector_cache();
    drop(lib);
    // The interactive connection served stale vectors until now — refresh it.
    if let Ok(interactive) = state.library.lock() {
        interactive.invalidate_vector_cache();
    }

    match embed_error {
        Some(err) => Err(err),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_part, pref_enabled_from_str, PartVerdict};

    #[test]
    fn short_part_is_resumable_and_kept() {
        // A clean-EOF truncation (fewer bytes than the pin) must be preserved for
        // a Range resume — never deleted, whatever a hash of the prefix says.
        assert_eq!(classify_part(100, 470, false), PartVerdict::Resumable);
        assert_eq!(classify_part(0, 470, false), PartVerdict::Resumable);
        // Hash is irrelevant while short (we don't even compute it).
        assert_eq!(classify_part(469, 470, true), PartVerdict::Resumable);
    }

    #[test]
    fn full_size_bad_hash_is_corrupt() {
        // Right size but wrong bytes → genuine corruption; caller deletes.
        assert_eq!(classify_part(470, 470, false), PartVerdict::Corrupt);
        // Over-long (can never match the pin) is corrupt too.
        assert_eq!(classify_part(600, 470, false), PartVerdict::Corrupt);
    }

    #[test]
    fn full_size_good_hash_is_complete() {
        assert_eq!(classify_part(470, 470, true), PartVerdict::Complete);
    }

    #[test]
    fn missing_pref_defaults_disabled() {
        // Semantic search is opt-in — the inverse of the auto-update default.
        assert!(!pref_enabled_from_str(None));
    }

    #[test]
    fn one_enables_zero_disables() {
        assert!(pref_enabled_from_str(Some("1")));
        assert!(pref_enabled_from_str(Some(" 1\n")));
        assert!(!pref_enabled_from_str(Some("0")));
        assert!(!pref_enabled_from_str(Some("")));
        assert!(!pref_enabled_from_str(Some("yes")));
    }
}
