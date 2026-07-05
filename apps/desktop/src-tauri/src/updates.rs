//! Auto-update lifecycle — checked, downloaded and installed from Rust.
//!
//! Rust owns the whole flow (check → download → park bytes → install) instead
//! of the JS plugin API so a fully downloaded update can still be applied when
//! the user quits without ever clicking "Restart" (Sparkle-style
//! install-on-quit, see [`install_pending_on_exit`]). The frontend only
//! mirrors state: it subscribes to `update:event` and calls the commands
//! below; it never touches the updater plugin directly.
//!
//! Update packages are signed (minisign) and verified against the `pubkey` in
//! `tauri.conf.json`; the endpoint is the `latest.json` asset of the latest
//! *published* GitHub release.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

/// A fetched update plus its fully downloaded bytes, parked until install —
/// either via [`restart_to_update`] or by the exit handler on quit.
pub struct PendingUpdate {
    update: Mutex<Option<(Update, Vec<u8>)>>,
    /// Guards against overlapping check/download flows (boot timer + manual
    /// check racing each other).
    in_flight: AtomicBool,
}

impl PendingUpdate {
    pub fn new() -> Self {
        PendingUpdate {
            update: Mutex::new(None),
            in_flight: AtomicBool::new(false),
        }
    }
}

/// Update lifecycle events streamed to the frontend on `update:event`.
/// Mirrored field-for-field by `UpdateEvent` in `lib/types.ts` (snake_case),
/// same convention as [`slideflow_core::model::ScanEvent`].
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum UpdateEvent {
    Checking,
    UpToDate,
    Available { version: String },
    Downloading { downloaded: u64, total: Option<u64> },
    Ready { version: String },
    Error { message: String },
}

fn emit(app: &AppHandle, event: &UpdateEvent) {
    let _ = app.emit("update:event", event);
}

/// Whether this install can update itself in place.
///
/// Dev builds are excluded (the updater would try to swap the debug bundle
/// for a published release). On Linux only AppImage installs can
/// self-replace — deb/rpm users update through their package manager — and
/// the AppImage runtime advertises itself via `$APPIMAGE`.
#[tauri::command]
pub fn updates_supported() -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    if cfg!(target_os = "linux") {
        std::env::var_os("APPIMAGE").is_some()
    } else {
        true
    }
}

/// Manual "Check for Updates…" from the UI. Fire-and-forget: results arrive
/// as `update:event`s, so the command returns immediately.
#[tauri::command]
pub fn check_for_updates(app: AppHandle) {
    tauri::async_runtime::spawn(run_update_flow(app));
}

fn auto_update_pref_path(app: &AppHandle) -> Option<PathBuf> {
    app.path().app_config_dir().ok().map(|d| d.join("auto-update"))
}

/// Interpret the persisted preference. "0" (trimmed) = disabled; a missing file
/// or any other contents = enabled — v0.3.0 shipped auto-update always-on, so
/// existing installs (no file) stay enabled.
fn pref_enabled_from_str(contents: Option<&str>) -> bool {
    match contents {
        Some(s) => s.trim() != "0",
        None => true,
    }
}

/// Whether automatic (boot + daily) update checks should run. Read by the
/// scheduler in lib.rs each cycle.
pub fn auto_update_enabled(app: &AppHandle) -> bool {
    pref_enabled_from_str(
        auto_update_pref_path(app)
            .and_then(|p| std::fs::read_to_string(p).ok())
            .as_deref(),
    )
}

/// Persist whether automatic update checks run. Manual "Check for Updates…" is
/// unaffected. Takes effect on the next scheduler cycle (does not cancel an
/// in-flight download).
#[tauri::command]
pub fn set_auto_update_enabled(app: AppHandle, enabled: bool) -> Result<(), String> {
    let path = auto_update_pref_path(&app).ok_or_else(|| "resolve app config dir".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    std::fs::write(&path, if enabled { "1" } else { "0" }).map_err(|e| e.to_string())
}

/// Install the downloaded update and relaunch as the new version.
///
/// The pending pair is *taken* before installing, so the exit handler that
/// runs during the restart finds nothing and can't double-install. On
/// Windows `install` hands off to the NSIS/MSI installer, which exits this
/// process and relaunches the app itself — `restart` is unreachable there by
/// design.
#[tauri::command]
pub async fn restart_to_update(app: AppHandle) -> Result<(), String> {
    let pending = {
        let state = app.state::<PendingUpdate>();
        let mut guard = state.update.lock().map_err(|_| "update lock poisoned")?;
        guard.take()
    };
    let Some((update, bytes)) = pending else {
        return Err("no pending update".into());
    };
    if let Err(err) = update.install(&bytes) {
        // Park it again so install-on-quit (or a retry) still works.
        if let Ok(mut guard) = app.state::<PendingUpdate>().update.lock() {
            *guard = Some((update, bytes));
        }
        return Err(err.to_string());
    }
    app.restart();
}

/// One full update pass: ask the release endpoint, and if a newer version
/// exists download it in the background, park it in [`PendingUpdate`], and
/// tell the frontend it's ready. Never fatal — failures surface as an
/// [`UpdateEvent::Error`] and the next pass starts fresh.
pub async fn run_update_flow(app: AppHandle) {
    let state = app.state::<PendingUpdate>();

    // An update is already downloaded and waiting: re-announce instead of
    // re-fetching (e.g. manual check while the ready toast was dismissed).
    if let Ok(guard) = state.update.lock() {
        if let Some((update, _)) = guard.as_ref() {
            let version = update.version.clone();
            drop(guard);
            emit(&app, &UpdateEvent::Ready { version });
            return;
        }
    }
    if state.in_flight.swap(true, Ordering::SeqCst) {
        return;
    }
    let result = check_and_download(&app).await;
    app.state::<PendingUpdate>()
        .in_flight
        .store(false, Ordering::SeqCst);
    if let Err(err) = result {
        emit(&app, &UpdateEvent::Error { message: err.to_string() });
    }
}

async fn check_and_download(app: &AppHandle) -> Result<(), tauri_plugin_updater::Error> {
    emit(app, &UpdateEvent::Checking);
    let Some(update) = app.updater()?.check().await? else {
        emit(app, &UpdateEvent::UpToDate);
        return Ok(());
    };

    emit(
        app,
        &UpdateEvent::Available {
            version: update.version.clone(),
        },
    );

    // Stream progress, throttled (~2% steps) so the IPC channel isn't flooded
    // by per-chunk callbacks.
    let mut downloaded: u64 = 0;
    let mut last_emitted: u64 = 0;
    let app_progress = app.clone();
    let bytes = update
        .download(
            move |chunk, total| {
                downloaded += chunk as u64;
                let step = total.map_or(512 * 1024, |t| (t / 50).max(64 * 1024));
                if downloaded - last_emitted >= step {
                    last_emitted = downloaded;
                    emit(&app_progress, &UpdateEvent::Downloading { downloaded, total });
                }
            },
            || {},
        )
        .await?;

    let version = update.version.clone();
    if let Ok(mut guard) = app.state::<PendingUpdate>().update.lock() {
        *guard = Some((update, bytes));
    }
    emit(app, &UpdateEvent::Ready { version });
    Ok(())
}

/// Install a fully downloaded update while the app is exiting, so the next
/// launch is the new version even if the user ignored the restart prompt.
///
/// Windows is excluded: its updater installer relaunches the app when done,
/// which is exactly wrong after a quit — Windows installs re-download on the
/// next launch instead. Best-effort: a failure here must never block exit.
pub fn install_pending_on_exit(app: &AppHandle) {
    if cfg!(target_os = "windows") {
        return;
    }
    let Some(state) = app.try_state::<PendingUpdate>() else {
        return;
    };
    let pending = state.update.lock().ok().and_then(|mut guard| guard.take());
    if let Some((update, bytes)) = pending {
        let _ = update.install(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::pref_enabled_from_str;

    #[test]
    fn missing_file_defaults_enabled() {
        // No preference file — existing installs stay on auto-update.
        assert!(pref_enabled_from_str(None));
    }

    #[test]
    fn zero_disables() {
        assert!(!pref_enabled_from_str(Some("0")));
    }

    #[test]
    fn one_enables() {
        assert!(pref_enabled_from_str(Some("1")));
    }

    #[test]
    fn trims_whitespace_around_zero() {
        assert!(!pref_enabled_from_str(Some(" 0\n")));
    }

    #[test]
    fn empty_defaults_enabled() {
        assert!(pref_enabled_from_str(Some("")));
    }
}
