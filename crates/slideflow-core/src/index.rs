//! The library: SQLite-backed index of every slide in the user's folders,
//! with FTS5 full-text search and filesystem watching.
//!
//! CONTRACT for the module owner:
//!
//! Schema (owned by this module, created/migrated in `Library::open`):
//! - `roots(id, path UNIQUE, last_scan_unix)`
//! - `decks(id, root_id, path UNIQUE, file_name, title, author, slide_count,
//!   modified_unix, size_bytes, slide_width_emu, slide_height_emu,
//!   content_hash)` — `content_hash` = sha256 of (mtime,size) or file bytes,
//!   used for incremental rescans (unchanged files are skipped).
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

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::model::{
    DeckRecord, ExportRecord, RootRecord, ScanEvent, ScanRecord, SearchFilters, SearchHit,
    SearchHistoryEntry, SlideRecord, StatsOverview,
};
use crate::pptx::PresentationFile;

const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS roots(
    id             INTEGER PRIMARY KEY,
    path           TEXT UNIQUE NOT NULL,
    last_scan_unix INTEGER
);
CREATE TABLE IF NOT EXISTS decks(
    id               INTEGER PRIMARY KEY,
    root_id          INTEGER NOT NULL REFERENCES roots(id) ON DELETE CASCADE,
    path             TEXT UNIQUE NOT NULL,
    file_name        TEXT NOT NULL,
    title            TEXT NOT NULL,
    author           TEXT,
    slide_count      INTEGER NOT NULL,
    modified_unix    INTEGER NOT NULL,
    size_bytes       INTEGER NOT NULL,
    slide_width_emu  INTEGER NOT NULL,
    slide_height_emu INTEGER NOT NULL,
    content_hash     TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS slides(
    id          INTEGER PRIMARY KEY,
    deck_id     INTEGER NOT NULL REFERENCES decks(id) ON DELETE CASCADE,
    slide_index INTEGER NOT NULL,
    title       TEXT,
    body_text   TEXT NOT NULL,
    notes       TEXT,
    thumb_path  TEXT
);
CREATE INDEX IF NOT EXISTS idx_decks_root ON decks(root_id);
CREATE INDEX IF NOT EXISTS idx_slides_deck ON slides(deck_id);
-- Favorites are keyed by (deck path, slide index) rather than row ids so they
-- survive rescans (which delete + reinsert slide rows) and app restarts.
CREATE TABLE IF NOT EXISTS slide_favorites(
    deck_path   TEXT NOT NULL,
    slide_index INTEGER NOT NULL,
    added_unix  INTEGER NOT NULL,
    PRIMARY KEY(deck_path, slide_index)
);
CREATE TABLE IF NOT EXISTS deck_favorites(
    deck_path  TEXT PRIMARY KEY,
    added_unix INTEGER NOT NULL
);
-- Activity history feeding the stats view.
CREATE TABLE IF NOT EXISTS search_history(
    id            INTEGER PRIMARY KEY,
    query         TEXT NOT NULL,
    result_count  INTEGER NOT NULL,
    searched_unix INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS export_history(
    id            INTEGER PRIMARY KEY,
    output_path   TEXT NOT NULL,
    title         TEXT NOT NULL,
    slide_count   INTEGER NOT NULL,
    source_decks  INTEGER NOT NULL,
    exported_unix INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS scan_history(
    id           INTEGER PRIMARY KEY,
    started_unix INTEGER NOT NULL,
    duration_ms  INTEGER NOT NULL,
    indexed      INTEGER NOT NULL,
    removed      INTEGER NOT NULL,
    unchanged    INTEGER NOT NULL
);
-- A standalone (content-owning) FTS5 table: contentless tables cannot serve
-- snippet(), and an external-content table would require the content table's
-- columns to match the FTS column names (which the fixed `slides` schema does
-- not). Owning its own copy keeps snippet()/bm25() working and deletes trivial.
CREATE VIRTUAL TABLE IF NOT EXISTS slides_fts USING fts5(
    title, body, notes, deck_title,
    tokenize="unicode61 remove_diacritics 2"
);
"#;

/// Columns selected for a `DeckRecord`, in field order (11 columns; requires
/// table alias `d`).
const DECK_COLS: &str = "d.id, d.path, d.file_name, d.title, d.author, d.slide_count, \
    d.modified_unix, d.size_bytes, d.slide_width_emu, d.slide_height_emu, \
    EXISTS(SELECT 1 FROM deck_favorites df WHERE df.deck_path = d.path)";
/// Columns selected for a `SlideRecord`, in field order (8 columns; requires
/// table aliases `s` AND `d` — the favorite flag is keyed by deck path).
const SLIDE_COLS: &str = "s.id, s.deck_id, s.slide_index, s.title, s.body_text, s.notes, s.thumb_path, \
    EXISTS(SELECT 1 FROM slide_favorites sf WHERE sf.deck_path = d.path AND sf.slide_index = s.slide_index)";

/// bm25 weights: title > deck_title > body > notes.
const BM25: &str = "bm25(slides_fts, 4.0, 1.0, 0.6, 2.0)";

pub struct Library {
    conn: Connection,
}

impl Library {
    /// Open (creating/migrating as needed) the library database.
    pub fn open(db_path: &Path) -> Result<Self> {
        let conn = Connection::open(db_path)?;
        Self::init(conn)
    }

    /// In-memory library (tests).
    pub fn open_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA foreign_keys=ON; PRAGMA busy_timeout=5000;",
        )?;
        conn.execute_batch(SCHEMA)?;
        Ok(Library { conn })
    }

    pub fn add_root(&mut self, path: &Path) -> Result<RootRecord> {
        let path_str = path.to_string_lossy().to_string();
        self.conn
            .execute("INSERT OR IGNORE INTO roots(path) VALUES(?1)", params![path_str])?;
        let id: i64 =
            self.conn
                .query_row("SELECT id FROM roots WHERE path=?1", params![path_str], |r| r.get(0))?;
        self.root_record(id)
    }

    /// Remove a root and all decks/slides under it.
    pub fn remove_root(&mut self, root_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "SELECT s.id FROM slides s JOIN decks d ON d.id = s.deck_id WHERE d.root_id = ?1",
            )?;
            let ids: Vec<i64> = stmt
                .query_map(params![root_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for id in ids {
                tx.execute("DELETE FROM slides_fts WHERE rowid=?1", params![id])?;
            }
        }
        tx.execute(
            "DELETE FROM slides WHERE deck_id IN (SELECT id FROM decks WHERE root_id=?1)",
            params![root_id],
        )?;
        tx.execute("DELETE FROM decks WHERE root_id=?1", params![root_id])?;
        tx.execute("DELETE FROM roots WHERE id=?1", params![root_id])?;
        tx.commit()?;
        Ok(())
    }

    pub fn roots(&self) -> Result<Vec<RootRecord>> {
        let mut stmt = self.conn.prepare(&format!("{ROOT_SELECT} ORDER BY r.path"))?;
        let rows = stmt
            .query_map([], row_to_root)?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    fn root_record(&self, id: i64) -> Result<RootRecord> {
        Ok(self
            .conn
            .query_row(&format!("{ROOT_SELECT} WHERE r.id=?1"), params![id], row_to_root)?)
    }

    /// Incrementally (re)scan all roots. `progress` is called from the
    /// scanning thread; it must be cheap.
    pub fn scan(&mut self, progress: &mut dyn FnMut(ScanEvent)) -> Result<()> {
        let scan_started = Instant::now();
        let started_unix = now_unix();
        // Snapshot roots up front.
        let roots: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare("SELECT id, path FROM roots")?;
            let v = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            v
        };

        // Enumerate candidate .pptx files across all roots.
        let mut candidates: Vec<(i64, PathBuf)> = Vec::new();
        for (root_id, root_path) in &roots {
            for entry in WalkDir::new(root_path)
                .into_iter()
                .filter_entry(|e| !is_pruned_dir(e.path()))
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() && is_pptx_file(entry.path()) {
                    candidates.push((*root_id, entry.path().to_path_buf()));
                }
            }
        }

        let total = candidates.len();
        progress(ScanEvent::Started { total_files: total });

        let mut seen: HashSet<String> = HashSet::new();
        let mut indexed = 0usize;
        let mut unchanged = 0usize;
        let mut removed = 0usize;

        for (i, (root_id, path)) in candidates.into_iter().enumerate() {
            let done = i + 1;
            let path_str = path.to_string_lossy().to_string();
            seen.insert(path_str.clone());

            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    progress(ScanEvent::Skipped { path: path_str, reason: e.to_string() });
                    continue;
                }
            };
            let size = meta.len() as i64;
            let mtime = system_time_unix(meta.modified().ok());
            let hash = content_hash(mtime, size);

            let existing: Option<String> = self
                .conn
                .query_row(
                    "SELECT content_hash FROM decks WHERE path=?1",
                    params![path_str],
                    |r| r.get(0),
                )
                .optional()?;
            if existing.as_deref() == Some(hash.as_str()) {
                unchanged += 1;
                progress(ScanEvent::Deck { path: path_str, done, total });
                continue;
            }

            match extract_deck(&path) {
                Ok(deck) => {
                    self.index_deck(root_id, &path_str, &deck, mtime, size, &hash)?;
                    indexed += 1;
                    progress(ScanEvent::Deck { path: path_str, done, total });
                }
                Err(e) => {
                    progress(ScanEvent::Skipped { path: path_str, reason: e.to_string() });
                }
            }
        }

        // Remove decks whose files vanished.
        let stored: Vec<(i64, String)> = {
            let mut stmt = self.conn.prepare("SELECT id, path FROM decks")?;
            let v = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<_>>()?;
            v
        };
        for (deck_id, deck_path) in stored {
            if !seen.contains(&deck_path) {
                self.delete_deck(deck_id)?;
                removed += 1;
            }
        }

        let now = now_unix();
        self.conn.execute("UPDATE roots SET last_scan_unix=?1", params![now])?;

        // Record the run for the stats view (best-effort).
        let _ = self.conn.execute(
            "INSERT INTO scan_history(started_unix, duration_ms, indexed, removed, unchanged) \
             VALUES(?1,?2,?3,?4,?5)",
            params![
                started_unix,
                scan_started.elapsed().as_millis() as i64,
                indexed as i64,
                removed as i64,
                unchanged as i64,
            ],
        );
        let _ = self.conn.execute(
            "DELETE FROM scan_history WHERE id NOT IN \
             (SELECT id FROM scan_history ORDER BY id DESC LIMIT 50)",
            [],
        );

        progress(ScanEvent::Finished { indexed, removed, unchanged });
        Ok(())
    }

    fn index_deck(
        &mut self,
        root_id: i64,
        path: &str,
        deck: &ExtractedDeck,
        mtime: i64,
        size: i64,
        hash: &str,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        let existing: Option<i64> = tx
            .query_row("SELECT id FROM decks WHERE path=?1", params![path], |r| r.get(0))
            .optional()?;

        let deck_id = match existing {
            Some(id) => {
                // Purge old slides + their FTS rows before rewriting.
                let old_ids: Vec<i64> = {
                    let mut stmt = tx.prepare("SELECT id FROM slides WHERE deck_id=?1")?;
                    let v = stmt
                        .query_map(params![id], |r| r.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    v
                };
                for sid in old_ids {
                    tx.execute("DELETE FROM slides_fts WHERE rowid=?1", params![sid])?;
                }
                tx.execute("DELETE FROM slides WHERE deck_id=?1", params![id])?;
                tx.execute(
                    "UPDATE decks SET root_id=?2, file_name=?3, title=?4, author=?5, \
                     slide_count=?6, modified_unix=?7, size_bytes=?8, slide_width_emu=?9, \
                     slide_height_emu=?10, content_hash=?11 WHERE id=?1",
                    params![
                        id,
                        root_id,
                        deck.file_name,
                        deck.title,
                        deck.author,
                        deck.slides.len() as i64,
                        mtime,
                        size,
                        deck.width_emu,
                        deck.height_emu,
                        hash,
                    ],
                )?;
                id
            }
            None => {
                tx.execute(
                    "INSERT INTO decks(root_id, path, file_name, title, author, slide_count, \
                     modified_unix, size_bytes, slide_width_emu, slide_height_emu, content_hash) \
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                    params![
                        root_id,
                        path,
                        deck.file_name,
                        deck.title,
                        deck.author,
                        deck.slides.len() as i64,
                        mtime,
                        size,
                        deck.width_emu,
                        deck.height_emu,
                        hash,
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };

        for slide in &deck.slides {
            tx.execute(
                "INSERT INTO slides(deck_id, slide_index, title, body_text, notes, thumb_path) \
                 VALUES(?1,?2,?3,?4,?5,NULL)",
                params![deck_id, slide.index, slide.title, slide.body_text, slide.notes],
            )?;
            let sid = tx.last_insert_rowid();
            // deck_title column carries the docProps title AND the file name so
            // both are searchable (generators often write junk titles).
            let deck_terms = format!("{} {}", deck.title, deck.file_name);
            tx.execute(
                "INSERT INTO slides_fts(rowid, title, body, notes, deck_title) \
                 VALUES(?1,?2,?3,?4,?5)",
                params![sid, slide.title, slide.body_text, slide.notes, deck_terms],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    fn delete_deck(&mut self, deck_id: i64) -> Result<()> {
        let tx = self.conn.transaction()?;
        {
            let mut stmt = tx.prepare("SELECT id FROM slides WHERE deck_id=?1")?;
            let ids: Vec<i64> = stmt
                .query_map(params![deck_id], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            for sid in ids {
                tx.execute("DELETE FROM slides_fts WHERE rowid=?1", params![sid])?;
            }
        }
        tx.execute("DELETE FROM slides WHERE deck_id=?1", params![deck_id])?;
        tx.execute("DELETE FROM decks WHERE id=?1", params![deck_id])?;
        tx.commit()?;
        Ok(())
    }

    /// Full-text search. Empty/whitespace query returns recent slides
    /// honoring the filters (browse mode).
    pub fn search(&self, query: &str, filters: &SearchFilters) -> Result<Vec<SearchHit>> {
        let limit = filters.limit.unwrap_or(200) as i64;
        let tokens = sanitize_query(query);
        if tokens.is_empty() {
            return self.browse(filters, limit);
        }

        let match_str = tokens
            .iter()
            .map(|t| format!("\"{t}\"*"))
            .collect::<Vec<_>>()
            .join(" ");

        let mut clauses = Vec::new();
        let mut fparams = Vec::new();
        push_filters(filters, &mut clauses, &mut fparams);

        let mut where_sql = String::from(" WHERE slides_fts MATCH ?");
        for c in &clauses {
            where_sql.push_str(" AND ");
            where_sql.push_str(c);
        }

        let sql = format!(
            "SELECT {SLIDE_COLS}, {DECK_COLS}, \
             snippet(slides_fts, 1, '<mark>', '</mark>', '…', 12), {BM25} \
             FROM slides_fts \
             JOIN slides s ON s.id = slides_fts.rowid \
             JOIN decks d ON d.id = s.deck_id{where_sql} \
             ORDER BY {BM25} ASC LIMIT ?"
        );

        let mut params: Vec<Value> = Vec::with_capacity(2 + fparams.len());
        params.push(Value::Text(match_str));
        params.extend(fparams);
        params.push(Value::Integer(limit));

        let mut stmt = self.conn.prepare(&sql)?;
        let hits = stmt
            .query_map(params_from_iter(params), |row| {
                let slide = row_to_slide(row, 0)?;
                let deck = row_to_deck(row, SLIDE_COL_COUNT)?;
                let snippet: String = row.get(SLIDE_COL_COUNT + DECK_COL_COUNT)?;
                let rank: f64 = row.get(SLIDE_COL_COUNT + DECK_COL_COUNT + 1)?;
                Ok(SearchHit { slide, deck, snippet, score: -rank })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(hits)
    }

    fn browse(&self, filters: &SearchFilters, limit: i64) -> Result<Vec<SearchHit>> {
        let mut clauses = Vec::new();
        let mut fparams = Vec::new();
        push_filters(filters, &mut clauses, &mut fparams);

        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };

        let sql = format!(
            "SELECT {SLIDE_COLS}, {DECK_COLS}, s.body_text \
             FROM slides s JOIN decks d ON d.id = s.deck_id{where_sql} \
             ORDER BY d.modified_unix DESC, s.slide_index ASC LIMIT ?"
        );

        let mut params: Vec<Value> = fparams;
        params.push(Value::Integer(limit));

        let mut stmt = self.conn.prepare(&sql)?;
        let hits = stmt
            .query_map(params_from_iter(params), |row| {
                let slide = row_to_slide(row, 0)?;
                let deck = row_to_deck(row, SLIDE_COL_COUNT)?;
                let body: String = row.get(SLIDE_COL_COUNT + DECK_COL_COUNT)?;
                let snippet = html_escape(&body.chars().take(120).collect::<String>());
                Ok(SearchHit { slide, deck, snippet, score: 0.0 })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(hits)
    }

    pub fn decks(&self) -> Result<Vec<DeckRecord>> {
        let mut stmt = self
            .conn
            .prepare(&format!("SELECT {DECK_COLS} FROM decks d ORDER BY d.modified_unix DESC"))?;
        let rows = stmt
            .query_map([], |r| row_to_deck(r, 0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn deck(&self, deck_id: i64) -> Result<DeckRecord> {
        Ok(self.conn.query_row(
            &format!("SELECT {DECK_COLS} FROM decks d WHERE d.id=?1"),
            params![deck_id],
            |r| row_to_deck(r, 0),
        )?)
    }

    pub fn slides_for_deck(&self, deck_id: i64) -> Result<Vec<SlideRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT {SLIDE_COLS} FROM slides s JOIN decks d ON d.id = s.deck_id \
             WHERE s.deck_id=?1 ORDER BY s.slide_index ASC"
        ))?;
        let rows = stmt
            .query_map(params![deck_id], |r| row_to_slide(r, 0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    pub fn slide(&self, slide_id: i64) -> Result<SlideRecord> {
        Ok(self.conn.query_row(
            &format!(
                "SELECT {SLIDE_COLS} FROM slides s JOIN decks d ON d.id = s.deck_id \
                 WHERE s.id=?1"
            ),
            params![slide_id],
            |r| row_to_slide(r, 0),
        )?)
    }

    /// Persist the cached thumbnail path for a slide.
    pub fn set_thumb_path(&mut self, slide_id: i64, thumb_path: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE slides SET thumb_path=?2 WHERE id=?1",
            params![slide_id, thumb_path],
        )?;
        Ok(())
    }

    /// Library-wide stats for the UI header: (deck_count, slide_count).
    pub fn stats(&self) -> Result<(i64, i64)> {
        let decks: i64 = self.conn.query_row("SELECT COUNT(*) FROM decks", [], |r| r.get(0))?;
        let slides: i64 = self.conn.query_row("SELECT COUNT(*) FROM slides", [], |r| r.get(0))?;
        Ok((decks, slides))
    }

    // --- favorites ---------------------------------------------------------

    /// Toggle the favorite flag of a slide; returns the new state.
    pub fn toggle_slide_favorite(&mut self, slide_id: i64) -> Result<bool> {
        let (deck_path, slide_index): (String, i64) = self.conn.query_row(
            "SELECT d.path, s.slide_index FROM slides s JOIN decks d ON d.id = s.deck_id \
             WHERE s.id=?1",
            params![slide_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let removed = self.conn.execute(
            "DELETE FROM slide_favorites WHERE deck_path=?1 AND slide_index=?2",
            params![deck_path, slide_index],
        )?;
        if removed > 0 {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO slide_favorites(deck_path, slide_index, added_unix) VALUES(?1,?2,?3)",
            params![deck_path, slide_index, now_unix()],
        )?;
        Ok(true)
    }

    /// Toggle the favorite flag of a deck; returns the new state.
    pub fn toggle_deck_favorite(&mut self, deck_id: i64) -> Result<bool> {
        let deck_path: String = self.conn.query_row(
            "SELECT path FROM decks WHERE id=?1",
            params![deck_id],
            |r| r.get(0),
        )?;
        let removed = self
            .conn
            .execute("DELETE FROM deck_favorites WHERE deck_path=?1", params![deck_path])?;
        if removed > 0 {
            return Ok(false);
        }
        self.conn.execute(
            "INSERT INTO deck_favorites(deck_path, added_unix) VALUES(?1,?2)",
            params![deck_path, now_unix()],
        )?;
        Ok(true)
    }

    // --- activity history / stats view --------------------------------------

    /// Remember a search for the stats view. Refinements of the previous entry
    /// (one being a prefix of the other, within two minutes) replace it instead
    /// of piling up per-keystroke variants.
    pub fn record_search(&mut self, query: &str, result_count: i64) -> Result<()> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(());
        }
        let now = now_unix();
        let last: Option<(i64, String, i64)> = self
            .conn
            .query_row(
                "SELECT id, query, searched_unix FROM search_history \
                 ORDER BY id DESC LIMIT 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;
        if let Some((id, prev, at)) = last {
            let refinement = now - at <= 120
                && (query.starts_with(prev.as_str()) || prev.starts_with(query));
            if refinement {
                self.conn.execute(
                    "UPDATE search_history SET query=?2, result_count=?3, searched_unix=?4 \
                     WHERE id=?1",
                    params![id, query, result_count, now],
                )?;
                return Ok(());
            }
        }
        self.conn.execute(
            "INSERT INTO search_history(query, result_count, searched_unix) VALUES(?1,?2,?3)",
            params![query, result_count, now],
        )?;
        // Keep the table bounded.
        self.conn.execute(
            "DELETE FROM search_history WHERE id NOT IN \
             (SELECT id FROM search_history ORDER BY id DESC LIMIT 200)",
            [],
        )?;
        Ok(())
    }

    /// Remember a completed export/composition for the stats view.
    pub fn record_export(
        &mut self,
        output_path: &str,
        title: &str,
        slide_count: i64,
        source_decks: i64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO export_history(output_path, title, slide_count, source_decks, exported_unix) \
             VALUES(?1,?2,?3,?4,?5)",
            params![output_path, title, slide_count, source_decks, now_unix()],
        )?;
        self.conn.execute(
            "DELETE FROM export_history WHERE id NOT IN \
             (SELECT id FROM export_history ORDER BY id DESC LIMIT 200)",
            [],
        )?;
        Ok(())
    }

    /// Everything the stats view needs, in one call.
    pub fn stats_overview(&self) -> Result<StatsOverview> {
        let (deck_count, slide_count) = self.stats()?;
        let total_bytes: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM decks",
            [],
            |r| r.get(0),
        )?;
        let favorite_slides: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM slide_favorites", [], |r| r.get(0))?;
        let favorite_decks: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM deck_favorites", [], |r| r.get(0))?;

        let last_scan = self
            .conn
            .query_row(
                "SELECT started_unix, duration_ms, indexed, removed, unchanged \
                 FROM scan_history ORDER BY id DESC LIMIT 1",
                [],
                |r| {
                    Ok(ScanRecord {
                        started_unix: r.get(0)?,
                        duration_ms: r.get(1)?,
                        indexed: r.get(2)?,
                        removed: r.get(3)?,
                        unchanged: r.get(4)?,
                    })
                },
            )
            .optional()?;

        let recent_searches = {
            let mut stmt = self.conn.prepare(
                "SELECT query, result_count, searched_unix FROM search_history \
                 ORDER BY id DESC LIMIT 10",
            )?;
            let v: Vec<SearchHistoryEntry> = stmt
                .query_map([], |r| {
                    Ok(SearchHistoryEntry {
                        query: r.get(0)?,
                        result_count: r.get(1)?,
                        searched_unix: r.get(2)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            v
        };

        let recent_exports = {
            let mut stmt = self.conn.prepare(
                "SELECT output_path, title, slide_count, source_decks, exported_unix \
                 FROM export_history ORDER BY id DESC LIMIT 10",
            )?;
            let v: Vec<ExportRecord> = stmt
                .query_map([], |r| {
                    Ok(ExportRecord {
                        output_path: r.get(0)?,
                        title: r.get(1)?,
                        slide_count: r.get(2)?,
                        source_decks: r.get(3)?,
                        exported_unix: r.get(4)?,
                    })
                })?
                .collect::<rusqlite::Result<_>>()?;
            v
        };

        let largest_decks = {
            let mut stmt = self.conn.prepare(&format!(
                "SELECT {DECK_COLS} FROM decks d ORDER BY d.size_bytes DESC LIMIT 5"
            ))?;
            let v: Vec<DeckRecord> = stmt
                .query_map([], |r| row_to_deck(r, 0))?
                .collect::<rusqlite::Result<_>>()?;
            v
        };

        Ok(StatsOverview {
            deck_count,
            slide_count,
            total_bytes,
            favorite_slides,
            favorite_decks,
            last_scan,
            recent_searches,
            recent_exports,
            largest_decks,
        })
    }
}

const ROOT_SELECT: &str = "SELECT r.id, r.path, \
    (SELECT COUNT(*) FROM decks d WHERE d.root_id = r.id), \
    (SELECT COUNT(*) FROM slides s JOIN decks d ON d.id = s.deck_id WHERE d.root_id = r.id), \
    r.last_scan_unix FROM roots r";

fn row_to_root(r: &Row) -> rusqlite::Result<RootRecord> {
    Ok(RootRecord {
        id: r.get(0)?,
        path: r.get(1)?,
        deck_count: r.get(2)?,
        slide_count: r.get(3)?,
        last_scan_unix: r.get(4)?,
    })
}

/// Number of columns in [`SLIDE_COLS`] / [`DECK_COLS`] — keep in sync.
const SLIDE_COL_COUNT: usize = 8;
const DECK_COL_COUNT: usize = 11;

fn row_to_deck(r: &Row, base: usize) -> rusqlite::Result<DeckRecord> {
    Ok(DeckRecord {
        id: r.get(base)?,
        path: r.get(base + 1)?,
        file_name: r.get(base + 2)?,
        title: r.get(base + 3)?,
        author: r.get(base + 4)?,
        slide_count: r.get(base + 5)?,
        modified_unix: r.get(base + 6)?,
        size_bytes: r.get(base + 7)?,
        slide_width_emu: r.get(base + 8)?,
        slide_height_emu: r.get(base + 9)?,
        favorite: r.get(base + 10)?,
    })
}

fn row_to_slide(r: &Row, base: usize) -> rusqlite::Result<SlideRecord> {
    Ok(SlideRecord {
        id: r.get(base)?,
        deck_id: r.get(base + 1)?,
        slide_index: r.get(base + 2)?,
        title: r.get(base + 3)?,
        body_text: r.get(base + 4)?,
        notes: r.get(base + 5)?,
        thumb_path: r.get(base + 6)?,
        favorite: r.get(base + 7)?,
    })
}

/// Append `SearchFilters` clauses (over table alias `d`) and their bound params.
fn push_filters(filters: &SearchFilters, clauses: &mut Vec<String>, params: &mut Vec<Value>) {
    if let Some(q) = &filters.deck_query {
        if !q.is_empty() {
            clauses.push("(LOWER(d.file_name) LIKE ? OR LOWER(d.title) LIKE ?)".into());
            let like = format!("%{}%", q.to_lowercase());
            params.push(Value::Text(like.clone()));
            params.push(Value::Text(like));
        }
    }
    if let Some(p) = &filters.path_prefix {
        if !p.is_empty() {
            clauses.push("d.path LIKE ?".into());
            params.push(Value::Text(format!("{p}%")));
        }
    }
    if let Some(from) = filters.modified_from {
        clauses.push("d.modified_unix >= ?".into());
        params.push(Value::Integer(from));
    }
    if let Some(to) = filters.modified_to {
        clauses.push("d.modified_unix <= ?".into());
        params.push(Value::Integer(to));
    }
    if filters.favorites_only == Some(true) {
        clauses.push(
            "EXISTS(SELECT 1 FROM slide_favorites sf \
             WHERE sf.deck_path = d.path AND sf.slide_index = s.slide_index)"
                .into(),
        );
    }
}

/// Extract alphanumeric/unicode word tokens, discarding FTS operators, quotes,
/// and punctuation entirely. The result feeds prefix `"tok"*` MATCH queries.
fn sanitize_query(query: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    for ch in query.chars() {
        if ch.is_alphanumeric() {
            cur.push(ch);
        } else if !cur.is_empty() {
            tokens.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Bump to force a one-time reindex of every deck (e.g. when the extraction
/// logic or FTS content changes) — a new version makes every stored hash stale.
const INDEX_VERSION: u32 = 2;

fn content_hash(mtime: i64, size: i64) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("v{INDEX_VERSION}:{mtime}:{size}").as_bytes());
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn system_time_unix(t: Option<SystemTime>) -> i64 {
    t.and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn now_unix() -> i64 {
    system_time_unix(Some(SystemTime::now()))
}

/// A directory that scanning must not descend into.
fn is_pruned_dir(path: &Path) -> bool {
    matches!(
        path.file_name().and_then(|n| n.to_str()),
        Some("node_modules") | Some(".git")
    )
}

/// A candidate `.pptx` file (case-insensitive), excluding `~$` lockfiles and
/// dot-hidden files.
fn is_pptx_file(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.starts_with("~$") || name.starts_with('.') {
        return false;
    }
    name.to_ascii_lowercase().ends_with(".pptx")
}

struct ExtractedSlide {
    index: i64,
    title: Option<String>,
    body_text: String,
    notes: Option<String>,
}

struct ExtractedDeck {
    file_name: String,
    title: String,
    author: Option<String>,
    width_emu: i64,
    height_emu: i64,
    slides: Vec<ExtractedSlide>,
}

/// Parse a deck fully (outside any DB transaction). Any parse error here makes
/// the caller record a `Skipped` event.
fn extract_deck(path: &Path) -> Result<ExtractedDeck> {
    let pf = PresentationFile::open(path)?;
    let file_name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| file_name.clone());
    let title = pf
        .core
        .title
        .clone()
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(stem);
    let author = pf.core.creator.clone();

    let mut slides = Vec::with_capacity(pf.slide_count());
    for i in 1..=pf.slide_count() {
        let content = pf.slide_content(i)?;
        slides.push(ExtractedSlide {
            index: i as i64,
            title: content.title.clone(),
            body_text: content.texts.join("\n"),
            notes: content.notes.clone(),
        });
    }

    Ok(ExtractedDeck {
        file_name,
        title,
        author,
        width_emu: pf.slide_width_emu,
        height_emu: pf.slide_height_emu,
        slides,
    })
}

/// Filesystem watcher over the library roots. Keep the returned value alive
/// for as long as watching should continue.
pub struct LibraryWatcher {
    #[allow(dead_code)]
    watcher: RecommendedWatcher,
}

/// Watch `roots` for `.pptx` changes, invoking `on_change` (debounced ~1s)
/// with the set of affected paths.
pub fn watch_roots(
    roots: &[PathBuf],
    on_change: Box<dyn Fn(Vec<PathBuf>) + Send + 'static>,
) -> Result<LibraryWatcher> {
    let (tx, rx) = mpsc::channel::<PathBuf>();

    let mut watcher = RecommendedWatcher::new(
        move |res: notify::Result<Event>| {
            if let Ok(event) = res {
                for p in event.paths {
                    if watch_relevant(&p) {
                        let _ = tx.send(p);
                    }
                }
            }
        },
        Config::default(),
    )
    .map_err(|e| Error::Watch(e.to_string()))?;

    for root in roots {
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| Error::Watch(e.to_string()))?;
    }

    thread::spawn(move || {
        // Block until the first change of a new batch (loop ends when the
        // watcher's sender is dropped and `recv` returns `Err`).
        while let Ok(first) = rx.recv() {
            let mut batch: HashSet<PathBuf> = HashSet::new();
            batch.insert(first);

            // Coalesce further changes within the debounce window.
            let deadline = Instant::now() + Duration::from_millis(1000);
            loop {
                let now = Instant::now();
                if now >= deadline {
                    break;
                }
                match rx.recv_timeout(deadline - now) {
                    Ok(p) => {
                        batch.insert(p);
                    }
                    Err(RecvTimeoutError::Timeout) => break,
                    Err(RecvTimeoutError::Disconnected) => {
                        on_change(batch.into_iter().collect());
                        return;
                    }
                }
            }
            on_change(batch.into_iter().collect());
        }
    });

    Ok(LibraryWatcher { watcher })
}

/// A watch event path we care about: a `.pptx` that is not a `~$` lockfile.
fn watch_relevant(p: &Path) -> bool {
    let Some(name) = p.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    if name.starts_with("~$") {
        return false;
    }
    name.to_ascii_lowercase().ends_with(".pptx")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{DeckSpec, SlideSpec};
    use std::fs::OpenOptions;

    fn scan_silent(lib: &mut Library) -> ScanEvent {
        let mut finished = ScanEvent::Finished { indexed: 0, removed: 0, unchanged: 0 };
        lib.scan(&mut |e| {
            if let ScanEvent::Finished { .. } = e {
                finished = e;
            }
        })
        .unwrap();
        finished
    }

    fn two_deck_library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Quarterly Review")
            .author("Jane")
            .slide(
                SlideSpec::new("Q3 Results")
                    .bullets(&["Revenue up 12%", "Churn down"])
                    .notes("Speak to the revenue trend")
                    .image(),
            )
            .slide(SlideSpec::new("Outlook").bullets(&["Zürich office opens"]))
            .write_to(&dir.path().join("finance.pptx"))
            .unwrap();

        DeckSpec::new("Product Roadmap")
            .slide(SlideSpec::new("Themes").bullets(&["Search", "Compose"]))
            .write_to(&dir.path().join("roadmap.pptx"))
            .unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        (dir, lib)
    }

    #[test]
    fn scan_indexes_decks_and_stats() {
        let (_dir, lib) = two_deck_library();
        let (decks, slides) = lib.stats().unwrap();
        assert_eq!(decks, 2);
        assert_eq!(slides, 3);

        let roots = lib.roots().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].deck_count, 2);
        assert_eq!(roots[0].slide_count, 3);
        assert!(roots[0].last_scan_unix.is_some());
    }

    #[test]
    fn search_finds_slide_with_mark_and_deck() {
        let (_dir, lib) = two_deck_library();
        let hits = lib.search("revenue", &SearchFilters::default()).unwrap();
        assert!(!hits.is_empty());
        let top = &hits[0];
        assert!(top.snippet.contains("<mark>"));
        assert_eq!(top.deck.file_name, "finance.pptx");
        assert_eq!(top.slide.title.as_deref(), Some("Q3 Results"));

        // Prefix search.
        let hits = lib.search("rev", &SearchFilters::default()).unwrap();
        assert!(hits.iter().any(|h| h.slide.title.as_deref() == Some("Q3 Results")));
    }

    #[test]
    fn diacritics_insensitive() {
        let (_dir, lib) = two_deck_library();
        let hits = lib.search("zurich", &SearchFilters::default()).unwrap();
        assert!(hits.iter().any(|h| h.slide.title.as_deref() == Some("Outlook")));
    }

    #[test]
    fn title_outranks_body() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Deck")
            .slide(SlideSpec::new("Widget Overview").bullets(&["Intro"]))
            .slide(SlideSpec::new("Pricing").bullets(&["The widget costs money"]))
            .write_to(&dir.path().join("d.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let hits = lib.search("widget", &SearchFilters::default()).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].slide.title.as_deref(), Some("Widget Overview"));
        assert!(hits[0].score >= hits[1].score);
    }

    #[test]
    fn filter_path_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let sub_a = dir.path().join("a");
        let sub_b = dir.path().join("b");
        std::fs::create_dir_all(&sub_a).unwrap();
        std::fs::create_dir_all(&sub_b).unwrap();
        DeckSpec::new("Alpha")
            .slide(SlideSpec::new("Topic").bullets(&["shared term"]))
            .write_to(&sub_a.join("a.pptx"))
            .unwrap();
        DeckSpec::new("Beta")
            .slide(SlideSpec::new("Topic").bullets(&["shared term"]))
            .write_to(&sub_b.join("b.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let filters = SearchFilters {
            path_prefix: Some(sub_a.to_string_lossy().to_string()),
            ..Default::default()
        };
        let hits = lib.search("shared", &filters).unwrap();
        assert!(!hits.is_empty());
        // Native separators: `/a/` on Unix, `\a\` on Windows.
        let sep = std::path::MAIN_SEPARATOR;
        let a_dir = format!("{sep}a{sep}");
        assert!(hits.iter().all(|h| h.deck.path.contains(&a_dir)));
    }

    #[test]
    fn filter_deck_query() {
        let (_dir, lib) = two_deck_library();
        let filters = SearchFilters {
            deck_query: Some("roadmap".into()),
            ..Default::default()
        };
        // Browse honoring deck_query.
        let hits = lib.search("", &filters).unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.deck.file_name.contains("roadmap")));
    }

    #[test]
    fn filter_modified_from_excludes_older() {
        let dir = tempfile::tempdir().unwrap();
        let old_path = dir.path().join("old.pptx");
        let new_path = dir.path().join("new.pptx");
        DeckSpec::new("Old")
            .slide(SlideSpec::new("A").bullets(&["term"]))
            .write_to(&old_path)
            .unwrap();
        DeckSpec::new("New")
            .slide(SlideSpec::new("B").bullets(&["term"]))
            .write_to(&new_path)
            .unwrap();

        // Force distinct mtimes via std's File::set_modified.
        let old_time = UNIX_EPOCH + Duration::from_secs(1_000_000_000);
        let new_time = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        OpenOptions::new().write(true).open(&old_path).unwrap().set_modified(old_time).unwrap();
        OpenOptions::new().write(true).open(&new_path).unwrap().set_modified(new_time).unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let filters = SearchFilters {
            modified_from: Some(1_500_000_000),
            ..Default::default()
        };
        let hits = lib.search("term", &filters).unwrap();
        assert!(!hits.is_empty());
        assert!(hits.iter().all(|h| h.deck.title == "New"));
    }

    #[test]
    fn browse_mode_honors_limit() {
        let (_dir, lib) = two_deck_library();
        let all = lib.search("", &SearchFilters::default()).unwrap();
        assert_eq!(all.len(), 3);
        assert!(all.iter().all(|h| !h.snippet.contains("<mark>")));

        let filters = SearchFilters { limit: Some(1), ..Default::default() };
        let limited = lib.search("", &filters).unwrap();
        assert_eq!(limited.len(), 1);
    }

    #[test]
    fn incremental_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.pptx");
        let b = dir.path().join("b.pptx");
        DeckSpec::new("A").slide(SlideSpec::new("One").bullets(&["alpha"])).write_to(&a).unwrap();
        DeckSpec::new("B").slide(SlideSpec::new("Two").bullets(&["beta"])).write_to(&b).unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();

        let first = scan_silent(&mut lib);
        assert!(matches!(first, ScanEvent::Finished { indexed: 2, removed: 0, unchanged: 0 }));

        // No changes: nothing reindexed.
        let second = scan_silent(&mut lib);
        assert!(matches!(second, ScanEvent::Finished { indexed: 0, removed: 0, unchanged: 2 }));

        // Rewrite one file with different content (and a newer mtime).
        DeckSpec::new("A2")
            .slide(SlideSpec::new("One").bullets(&["gamma"]))
            .write_to(&a)
            .unwrap();
        let future = UNIX_EPOCH + Duration::from_secs(now_unix() as u64 + 100);
        OpenOptions::new().write(true).open(&a).unwrap().set_modified(future).unwrap();

        let third = scan_silent(&mut lib);
        assert!(matches!(third, ScanEvent::Finished { indexed: 1, removed: 0, unchanged: 1 }));
        assert!(lib.search("gamma", &SearchFilters::default()).unwrap().len() == 1);
        assert!(lib.search("alpha", &SearchFilters::default()).unwrap().is_empty());

        // Delete a file: its deck + slides + FTS rows vanish.
        std::fs::remove_file(&b).unwrap();
        let fourth = scan_silent(&mut lib);
        assert!(matches!(fourth, ScanEvent::Finished { indexed: 0, removed: 1, unchanged: 1 }));
        assert_eq!(lib.stats().unwrap().0, 1);
        assert!(lib.search("beta", &SearchFilters::default()).unwrap().is_empty());
        assert!(lib.decks().unwrap().iter().all(|d| d.title != "B"));
    }

    #[test]
    fn remove_root_purges_everything() {
        let (dir, mut lib) = two_deck_library();
        let _ = dir;
        let root_id = lib.roots().unwrap()[0].id;
        lib.remove_root(root_id).unwrap();
        assert_eq!(lib.stats().unwrap(), (0, 0));
        assert!(lib.roots().unwrap().is_empty());
        assert!(lib.search("revenue", &SearchFilters::default()).unwrap().is_empty());
    }

    #[test]
    fn fts_injection_safe() {
        let (_dir, lib) = two_deck_library();
        for q in [
            "revenue OR churn",
            "\"unterminated",
            "NEAR(a b)",
            "(revenue AND",
            "* * *",
            "revenue) NOT churn",
        ] {
            // Must not error, regardless of FTS operators in the input.
            let _ = lib.search(q, &SearchFilters::default()).unwrap();
        }
    }

    #[test]
    fn set_thumb_path_persists() {
        let (_dir, lib) = two_deck_library();
        let mut lib = lib;
        let slide_id = lib.decks().unwrap();
        let deck_id = slide_id[0].id;
        let sid = lib.slides_for_deck(deck_id).unwrap()[0].id;
        lib.set_thumb_path(sid, "/cache/thumb.svg").unwrap();
        assert_eq!(lib.slide(sid).unwrap().thumb_path.as_deref(), Some("/cache/thumb.svg"));
    }

    #[test]
    fn favorites_toggle_filter_and_survive_rescan() {
        let (dir, mut lib) = two_deck_library();
        let hits = lib.search("revenue", &SearchFilters::default()).unwrap();
        let slide = &hits[0].slide;
        assert!(!slide.favorite);

        assert!(lib.toggle_slide_favorite(slide.id).unwrap());
        assert!(lib.slide(slide.id).unwrap().favorite);

        // favorites_only filter returns exactly the starred slide.
        let filters = SearchFilters { favorites_only: Some(true), ..Default::default() };
        let favs = lib.search("", &filters).unwrap();
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].slide.id, slide.id);

        // Rescan after touching the file: slide rows are recreated, favorite
        // sticks because it is keyed by (deck path, slide index).
        let deck_path = dir.path().join("finance.pptx");
        let future = UNIX_EPOCH + Duration::from_secs(now_unix() as u64 + 500);
        OpenOptions::new().write(true).open(&deck_path).unwrap().set_modified(future).unwrap();
        scan_silent(&mut lib);
        let favs = lib.search("", &filters).unwrap();
        assert_eq!(favs.len(), 1, "favorite lost across rescan");

        // Deck favorite toggles and surfaces on DeckRecord.
        let deck_id = favs[0].deck.id;
        assert!(lib.toggle_deck_favorite(deck_id).unwrap());
        assert!(lib.deck(deck_id).unwrap().favorite);
        assert!(!lib.toggle_deck_favorite(deck_id).unwrap());

        // Untoggle slide.
        let sid = favs[0].slide.id;
        assert!(!lib.toggle_slide_favorite(sid).unwrap());
        assert!(lib.search("", &filters).unwrap().is_empty());
    }

    #[test]
    fn history_and_stats_overview() {
        let (_dir, lib) = two_deck_library();
        let mut lib = lib;

        // Search history coalesces keystroke refinements.
        lib.record_search("rev", 1).unwrap();
        lib.record_search("revenue", 1).unwrap();
        lib.record_search("churn", 2).unwrap();
        lib.record_search("  ", 0).unwrap(); // ignored

        lib.record_export("/tmp/out.pptx", "My Deck", 4, 2).unwrap();

        let o = lib.stats_overview().unwrap();
        assert_eq!(o.deck_count, 2);
        assert_eq!(o.slide_count, 3);
        assert!(o.total_bytes > 0);
        assert!(o.last_scan.is_some(), "scan_history row from two_deck_library scan");
        assert_eq!(
            o.recent_searches.iter().map(|s| s.query.as_str()).collect::<Vec<_>>(),
            vec!["churn", "revenue"],
        );
        assert_eq!(o.recent_exports.len(), 1);
        assert_eq!(o.recent_exports[0].title, "My Deck");
        assert_eq!(o.recent_exports[0].slide_count, 4);
        assert_eq!(o.largest_decks.len(), 2);
        assert!(o.largest_decks[0].size_bytes >= o.largest_decks[1].size_bytes);
    }

    #[test]
    fn watcher_fires_on_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel::<Vec<PathBuf>>();
        let watcher = watch_roots(
            &[dir.path().to_path_buf()],
            Box::new(move |paths| {
                let _ = tx.send(paths);
            }),
        )
        .unwrap();

        // Let the OS watch register.
        thread::sleep(Duration::from_millis(400));
        DeckSpec::new("Watched")
            .slide(SlideSpec::new("Hi"))
            .write_to(&dir.path().join("watched.pptx"))
            .unwrap();

        let received = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("on_change should fire for the new .pptx");
        assert!(received.iter().any(|p| p.ends_with("watched.pptx")));
        drop(watcher);
    }
}
