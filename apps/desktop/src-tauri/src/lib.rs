//! Slideflow desktop — Tauri application entry point.
//!
//! Wires the `slideflow-core` [`Library`] into a Tauri app: opens the SQLite
//! database under the platform app-data dir, prepares the thumbnail cache under
//! the app-cache dir, registers the command surface, and installs the dialog /
//! opener / shell / updater plugins the frontend needs.

mod commands;
mod fonts;
mod semantic;
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
        // Outgoing native drag sessions (drag a slide out as a real file, WS-G).
        // This only starts *outgoing* OS drags; it does NOT re-enable Tauri's
        // incoming webview drop handling, so `dragDropEnabled: false` (which the
        // internal grid → tray HTML5 DnD depends on) stays intact.
        .plugin(tauri_plugin_drag::init())
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

            // Drag-out scratch dir (WS-G): the single-slide .pptx + PNG icons
            // written when a slide is dragged out. These are ephemeral caches
            // keyed on the source deck's mtime, so wipe the whole dir on every
            // launch — a fresh boot starts empty (mirrors the thumb sweep, but
            // total rather than selective). Cheap (a handful of small files),
            // so done synchronously before the dir is handed to AppState.
            let dragout_dir = cache_dir.join("dragout");
            let _ = fs::remove_dir_all(&dragout_dir);
            fs::create_dir_all(&dragout_dir).expect("create dragout cache dir");

            // App-local fonts (harvested / user-added / downloaded) live under
            // app-DATA (survive cache sweeps). Create the source subdirs up front
            // so the picker/harvest/download paths can just write into them.
            let fonts_dir = data_dir.join("fonts");
            for sub in ["harvested", "user", "downloaded"] {
                let _ = fs::create_dir_all(fonts_dir.join(sub));
            }

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

            app.manage(AppState::new(library, scan_library, thumbs_dir, dragout_dir));
            app.manage(updates::PendingUpdate::new());
            // App-local font database (render `AppFontSet` + export `fontdb`).
            // Cheap to build (parses only the small app fonts dir; the system-
            // font scan is deferred to first export).
            app.manage(fonts::FontsState::new(fonts_dir));

            // Semantic search: the E5 model lives under app-data (survives
            // cache sweeps). If the user enabled the feature and the model is
            // on disk, load + attach the embedder in the background.
            let model_dir = data_dir.join("models").join("multilingual-e5-small");
            app.manage(semantic::SemanticState::new(model_dir));
            semantic::bootstrap_on_startup(&app.handle().clone());

            // Background auto-update: first check shortly after launch (so
            // boot I/O settles first), then daily while the app runs. Found
            // updates download silently; the frontend hears about it all via
            // `update:event`. A plain thread (matching the scan pattern)
            // avoids relying on tokio's time driver.
            if updates::updates_supported() {
                let handle = app.handle().clone();
                std::thread::spawn(move || loop {
                    std::thread::sleep(std::time::Duration::from_secs(5));
                    // Gate each cycle on the user's preference so disabling
                    // auto-updates skips the silent check while the daily loop
                    // keeps ticking (honored on the next cycle).
                    if updates::auto_update_enabled(&handle) {
                        // Silent (is_manual=false): a download that finishes
                        // after the user opts out mid-flight is discarded.
                        tauri::async_runtime::block_on(updates::run_update_flow(
                            handle.clone(),
                            false,
                        ));
                    }
                    std::thread::sleep(std::time::Duration::from_secs(60 * 60 * 24 - 5));
                });
            }

            // macOS: install the native app menu. Windows/Linux have no
            // in-window menu bar and stay that way.
            #[cfg(target_os = "macos")]
            install_app_menu(app.handle())?;

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_roots,
            commands::add_root,
            commands::remove_root,
            commands::set_root_excludes,
            commands::start_scan,
            commands::is_scanning,
            commands::clear_index,
            commands::search,
            commands::get_decks,
            commands::get_deck_slides,
            commands::list_saved_searches,
            commands::save_search,
            commands::rename_saved_search,
            commands::delete_saved_search,
            commands::get_similar_slides,
            commands::list_duplicate_groups,
            commands::get_slide_preview,
            commands::compose_deck,
            commands::prepare_slide_drag,
            commands::export_tray_pdf,
            commands::export_tray_pngs,
            commands::get_stats,
            commands::get_stats_overview,
            commands::get_export_counts,
            commands::record_search,
            commands::toggle_favorite_slide,
            commands::toggle_favorite_deck,
            commands::list_tags,
            commands::get_slide_tags,
            commands::set_slide_tags,
            commands::rename_tag,
            commands::delete_tag,
            commands::reveal_in_finder,
            commands::open_file,
            commands::open_url,
            fonts::list_library_fonts,
            fonts::fonts_dir,
            fonts::add_user_fonts,
            fonts::remove_app_font,
            fonts::download_font,
            fonts::cancel_font_download,
            updates::updates_supported,
            updates::check_for_updates,
            updates::restart_to_update,
            updates::set_auto_update_enabled,
            updates::get_auto_update_enabled,
            semantic::get_embedding_status,
            semantic::set_semantic_search_enabled,
            semantic::download_embedding_model,
            semantic::cancel_model_download,
            semantic::delete_embedding_model,
            semantic::start_embed_backfill,
            semantic::cancel_embed_backfill,
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

/// macOS: install the application menu.
///
/// Starts from Tauri's default menu — so Edit (⌘C/⌘V/⌘X/⌘Z), Window, etc. and
/// their standard accelerators stay intact — then swaps the first (app)
/// submenu for a custom one. The custom submenu replaces the native About panel
/// with an "About Slideflow" item and adds "Settings…" (⌘,); both fire a
/// `menu:open` event so the frontend opens the richer in-app sheets (the native
/// About panel lacks the update controls the in-app one carries).
#[cfg(target_os = "macos")]
fn install_app_menu(app: &tauri::AppHandle) -> tauri::Result<()> {
    use tauri::menu::{Menu, MenuItem, SubmenuBuilder};
    use tauri::Emitter;

    let about = MenuItem::with_id(app, "about", "About Slideflow", true, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?;

    // App submenu in macOS order: About / Settings on top, then the standard
    // predefined items. The label is cosmetic — macOS always shows the app name
    // for the first submenu.
    let app_submenu = SubmenuBuilder::new(app, "Slideflow")
        .item(&about)
        .separator()
        .item(&settings)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;

    let menu = Menu::default(app)?;
    // Drop the default app submenu (native About, no Settings) and slot ours in
    // its place; the remaining default submenus are left untouched.
    menu.remove_at(0)?;
    menu.prepend(&app_submenu)?;
    app.set_menu(menu)?;

    app.on_menu_event(move |app, event| {
        let payload = match event.id().0.as_str() {
            "about" => "about",
            "settings" => "settings",
            _ => return,
        };
        let _ = app.emit("menu:open", payload);
    });

    Ok(())
}
