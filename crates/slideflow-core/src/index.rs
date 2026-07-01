//! The library: SQLite-backed index of every slide in the user's folders,
//! with FTS5 full-text search and filesystem watching.
//!
//! CONTRACT for the module owner:
//!
//! Schema (owned by this module, created/migrated in `Library::open`):
//! - `roots(id, path UNIQUE, last_scan_unix)`
//! - `decks(id, root_id, path UNIQUE, file_name, title, author, slide_count,
//!    modified_unix, size_bytes, slide_width_emu, slide_height_emu,
//!    content_hash)` — `content_hash` = sha256 of (mtime,size) or file bytes,
//!    used for incremental rescans (unchanged files are skipped).
//! - `slides(id, deck_id, slide_index, title, body_text, notes, thumb_path)`
//! - FTS5: `slides_fts(title, body, notes, deck_title, content='')` —
//!   contentless or external-content table kept in sync inside the same
//!   transaction as `slides` writes; `tokenize="unicode61 remove_diacritics 2"`.
//!
//! Behavior:
//! - `scan` walks all roots (`walkdir`), indexes `.pptx` files (skip temp
//!   `~$…` lockfiles and hidden dirs), removes DB rows for vanished files,
//!   skips unchanged files by (mtime,size), reports progress via the callback.
//!   Parse failures are recorded as `ScanEvent::Skipped`, never abort the scan.
//! - `search`: FTS5 `MATCH` with each user token turned into a prefix query
//!   (`tok*`), joined to decks, filtered per `SearchFilters`, ranked by bm25
//!   (weight title > body > notes > deck_title), snippet() over the body with
//!   `<mark>` wrapping. Empty query = browse mode: newest decks' slides.
//! - `watch` starts a `notify` watcher over all roots with ~1s debounce that
//!   calls `on_change` with the affected paths; the caller (Tauri layer)
//!   decides when to rescan. Return the watcher handle so it stays alive.
//! - All methods are synchronous; the desktop layer wraps them in blocking
//!   tasks. `Library` is `Send` (no `!Send` fields) so it can live in a Mutex.

use std::path::{Path, PathBuf};

use crate::error::Result;
use crate::model::{DeckRecord, RootRecord, ScanEvent, SearchFilters, SearchHit, SlideRecord};

pub struct Library {
    #[allow(dead_code)]
    conn: rusqlite::Connection,
}

impl Library {
    /// Open (creating/migrating as needed) the library database.
    pub fn open(db_path: &Path) -> Result<Self> {
        let _ = db_path;
        todo!("implemented by the index module owner")
    }

    /// In-memory library (tests).
    pub fn open_in_memory() -> Result<Self> {
        todo!("implemented by the index module owner")
    }

    pub fn add_root(&mut self, path: &Path) -> Result<RootRecord> {
        let _ = path;
        todo!()
    }

    /// Remove a root and all decks/slides under it.
    pub fn remove_root(&mut self, root_id: i64) -> Result<()> {
        let _ = root_id;
        todo!()
    }

    pub fn roots(&self) -> Result<Vec<RootRecord>> {
        todo!()
    }

    /// Incrementally (re)scan all roots. `progress` is called from the
    /// scanning thread; it must be cheap.
    pub fn scan(&mut self, progress: &mut dyn FnMut(ScanEvent)) -> Result<()> {
        let _ = progress;
        todo!()
    }

    /// Full-text search. Empty/whitespace query returns recent slides
    /// honoring the filters (browse mode).
    pub fn search(&self, query: &str, filters: &SearchFilters) -> Result<Vec<SearchHit>> {
        let _ = (query, filters);
        todo!()
    }

    pub fn decks(&self) -> Result<Vec<DeckRecord>> {
        todo!()
    }

    pub fn deck(&self, deck_id: i64) -> Result<DeckRecord> {
        let _ = deck_id;
        todo!()
    }

    pub fn slides_for_deck(&self, deck_id: i64) -> Result<Vec<SlideRecord>> {
        let _ = deck_id;
        todo!()
    }

    pub fn slide(&self, slide_id: i64) -> Result<SlideRecord> {
        let _ = slide_id;
        todo!()
    }

    /// Persist the cached thumbnail path for a slide.
    pub fn set_thumb_path(&mut self, slide_id: i64, thumb_path: &str) -> Result<()> {
        let _ = (slide_id, thumb_path);
        todo!()
    }

    /// Library-wide stats for the UI header: (deck_count, slide_count).
    pub fn stats(&self) -> Result<(i64, i64)> {
        todo!()
    }
}

/// Filesystem watcher over the library roots. Keep the returned value alive
/// for as long as watching should continue.
pub struct LibraryWatcher {
    #[allow(dead_code)]
    watcher: notify::RecommendedWatcher,
}

/// Watch `roots` for `.pptx` changes, invoking `on_change` (debounced ~1s)
/// with the set of affected paths.
pub fn watch_roots(
    roots: &[PathBuf],
    on_change: Box<dyn Fn(Vec<PathBuf>) + Send + 'static>,
) -> Result<LibraryWatcher> {
    let _ = (roots, on_change);
    todo!("implemented by the index module owner")
}
