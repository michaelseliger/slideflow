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

fn detach_embedder(app: &AppHandle) {
    let state = app.state::<AppState>();
    let semantic = app.state::<SemanticState>();
    semantic.ready.store(false, Ordering::SeqCst);
    if let Ok(mut lib) = state.library.lock() {
        lib.set_embedder(None);
    }
    if let Ok(mut lib) = state.scan_library.lock() {
        lib.set_embedder(None);
    };
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

    // Verify against the pinned hash; a mismatch (corrupt/truncated/tampered)
    // deletes the partial so the next attempt starts clean.
    let actual = sha256_of_file(&part_path).map_err(|e| format!("{}: {e}", file.name))?;
    if actual != file.sha256 {
        let _ = fs::remove_file(&part_path);
        return Err(format!(
            "{}: checksum mismatch (expected {}, got {actual})",
            file.name, file.sha256
        ));
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
    for chunk in pending.chunks(32) {
        if semantic.backfill_cancel.load(Ordering::SeqCst) {
            break;
        }
        lib.embed_and_store(chunk).map_err(|e| e.to_string())?;
        done += chunk.len();
        emit_embed(app, &EmbedEvent::Progress { done, total });
    }

    let _ = lib.cleanup_orphan_embeddings();
    lib.invalidate_vector_cache();
    drop(lib);
    // The interactive connection served stale vectors until now — refresh it.
    if let Ok(interactive) = state.library.lock() {
        interactive.invalidate_vector_cache();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::pref_enabled_from_str;

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
