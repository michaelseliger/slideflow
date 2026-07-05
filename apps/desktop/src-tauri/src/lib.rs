//! Slideflow desktop — Tauri application entry point.
//!
//! Wires the `slideflow-core` [`Library`] into a Tauri app: opens the SQLite
//! database under the platform app-data dir, prepares the thumbnail cache under
//! the app-cache dir, registers the command surface, and installs the dialog /
//! opener / shell / updater plugins the frontend needs.

mod commands;
mod updates;

use std::fs;

use tauri::Manager;

use slideflow_core::index::Library;

use commands::AppState;

/// Build, configure and run the Tauri application.
///
/// # Panics
/// Panics only if the platform data directories can't be resolved or created,
/// or the library database can't be opened — all fatal, first-launch problems.
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .setup(|app| {
            // Resolve platform dirs.
            //   macOS: ~/Library/Application Support/com.slideflow.app/library.db
            //          ~/Library/Caches/com.slideflow.app/thumbs/
            let data_dir = app
                .path()
                .app_data_dir()
                .expect("resolve app data dir");
            let cache_dir = app
                .path()
                .app_cache_dir()
                .expect("resolve app cache dir");

            fs::create_dir_all(&data_dir).expect("create app data dir");
            let thumbs_dir = cache_dir.join("thumbs");
            fs::create_dir_all(&thumbs_dir).expect("create thumbs cache dir");

            let db_path = data_dir.join("library.db");
            let library = Library::open(&db_path).expect("open library database");
            // Second connection to the same WAL database so searches stay
            // responsive while a scan holds the other one (see AppState docs).
            let scan_library = Library::open(&db_path).expect("open scan connection");

            // Reclaim stale/orphaned preview-cache files on startup — decks no
            // longer indexed, stale render versions, and (on first upgrade) the
            // legacy `<id>.svg` files from before content-addressing. Off the
            // main thread so launch isn't blocked by cache I/O.
            if let Ok(valid) = library.all_deck_hashes() {
                let sweep_dir = thumbs_dir.clone();
                std::thread::spawn(move || {
                    slideflow_core::thumbs::sweep_thumbs(&sweep_dir, &valid);
                });
            }

            app.manage(AppState::new(library, scan_library, thumbs_dir));
            app.manage(updates::PendingUpdate::new());

            // Background auto-update: first check shortly after launch (so
            // boot I/O settles first), then daily while the app runs. Found
            // updates download silently; the frontend hears about it all via
            // `update:event`. A plain thread (matching the scan pattern)
            // avoids relying on tokio's time driver.
            if updates::updates_supported() {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    tauri::async_runtime::block_on(updates::run_update_flow(handle.clone()));
                    std::thread::sleep(std::time::Duration::from_secs(60 * 60 * 24 - 5));
                });
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_roots,
            commands::add_root,
            commands::remove_root,
            commands::start_scan,
            commands::is_scanning,
            commands::search,
            commands::get_decks,
            commands::get_deck_slides,
            commands::get_slide_preview,
            commands::compose_deck,
            commands::get_stats,
            commands::get_stats_overview,
            commands::record_search,
            commands::toggle_favorite_slide,
            commands::toggle_favorite_deck,
            commands::reveal_in_finder,
            commands::open_file,
            commands::open_url,
            updates::updates_supported,
            updates::check_for_updates,
            updates::restart_to_update,
        ])
        .build(tauri::generate_context!())
        .expect("error while building Slideflow")
        .run(|app, event| {
            // Sparkle-style install-on-quit: a downloaded update the user
            // never restarted for is applied while the app exits, so the next
            // launch is already the new version.
            if let tauri::RunEvent::ExitRequested { .. } = event {
                updates::install_pending_on_exit(app);
            }
        });
}
