//! Slideflow desktop — Tauri application entry point.
//!
//! Wires the `slideflow-core` [`Library`] into a Tauri app: opens the SQLite
//! database under the platform app-data dir, prepares the thumbnail cache under
//! the app-cache dir, registers the command surface, and installs the dialog /
//! opener / shell plugins the frontend needs.

mod commands;

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

            app.manage(AppState::new(library, scan_library, thumbs_dir));
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
            commands::get_slide_svg,
            commands::compose_deck,
            commands::get_stats,
            commands::get_stats_overview,
            commands::record_search,
            commands::toggle_favorite_slide,
            commands::toggle_favorite_deck,
            commands::reveal_in_finder,
            commands::open_file,
            commands::open_url,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Slideflow");
}
