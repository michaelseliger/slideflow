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

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use globset::{Glob, GlobSet, GlobSetBuilder};
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row};
use sha2::{Digest, Sha256};
use walkdir::WalkDir;

use crate::embed::store::{blob_to_vec, vec_to_blob};
use crate::embed::{rrf_fuse, Embedder, VectorStore};
use crate::error::{Error, Result};
use crate::model::{
    DeckRecord, DuplicateGroup, DuplicateSlide, ExportRecord, RenderDropStat, RootRecord,
    SavedSearch, ScanEvent, ScanIssue, ScanRecord, SearchFilters, SearchHit, SearchHistoryEntry,
    SimilarSlide, SlidePick, SlideRecord, StatsOverview, TagRecord,
};
use crate::pptx::PresentationFile;

mod query;
use query::{max_opt, min_opt, parse_query, ParsedQuery};

/// Near-duplicate cosine threshold and per-slide neighbor fan-out (roadmap #9).
const NEAR_DUP_THRESHOLD: f32 = 0.92;
const NEAR_DUP_TOP_N: usize = 10;

/// Minimum raw cosine for a semantic hit to be considered relevant. Applied to
/// the vector retrieval arm (semantic + hybrid) and to find-similar; below-floor
/// hits are dropped so a query no longer returns the whole embedded corpus.
///
/// E5 (`multilingual-e5-small`) produces a COMPRESSED cosine range: even a
/// query and an unrelated passage typically sit around 0.70–0.80, so an
/// intuitive floor like 0.5 would be a no-op. The value below was tuned
/// empirically against the live library (166 embedded slides of German/English
/// B2B content) with the real model — ranked cosines for on-topic vs. nonsense
/// queries:
///
/// | query                    | top-1 | genuine tail |
/// |--------------------------|-------|--------------|
/// | Salesforce Success …     | 0.911 | ≥ ~0.86      |
/// | commercetools Accelerator| 0.909 | ≥ ~0.85      |
/// | Magnolia Integration     | 0.899 | ≥ ~0.84      |
/// | SAP Commerce Cloud       | 0.884 | ≥ ~0.85      |
/// | Adobe Commerce / Magento | 0.875 | ≥ ~0.84      |
/// | PIM Product Info Mgmt    | 0.875 | ≥ ~0.83      |
/// | Delivery Organisation …  | 0.860 | ≥ ~0.82      |
/// | B2B E-Commerce           | 0.858 | ≥ ~0.83      |
/// | --- nonsense (ceiling) --------- top-1 --------- |
/// | quantum chess strategy   | 0.800 |              |
/// | photosynthesis of ferns  | 0.797 |              |
/// | Bananenbrot Rezept       | 0.777 |              |
///
/// Pool percentiles across all queries: genuine p50 0.803 / p90 0.837 / max
/// 0.911; nonsense p50 0.752 / p90 0.777 / **max 0.800**. 0.82 sits just above
/// the nonsense ceiling (≈ +0.02 margin) while retaining every genuine query's
/// relevant head, and drops the weakly-related 0.75–0.82 tail.
const SEMANTIC_SCORE_FLOOR: f32 = 0.82;

/// Raw near-duplicate clusters (O(n²)) for a vector-store snapshot, using the
/// library's tuned threshold + fan-out. Pure compute on the snapshot — no DB, no
/// `Library` — so the desktop host runs it in `spawn_blocking` off the
/// interactive mutex. The result is the *raw* clustering (before redundant-with-
/// exact filtering); feed it to [`Library::finish_duplicate_groups`].
pub fn near_dup_clusters_for(store: &VectorStore) -> Vec<Vec<i64>> {
    store.near_dup_clusters(NEAR_DUP_THRESHOLD, NEAR_DUP_TOP_N)
}
/// Pool size taken from each retrieval arm before reciprocal-rank fusion.
const HYBRID_POOL: usize = 200;
/// Embedding batch size fed to the model at once. 64 (up from 32) cuts matmul
/// dispatch overhead for a little more peak RAM; the desktop backfill chunks
/// `pending` by this too, so one lock-free `embed_passages` covers one batch.
pub const EMBED_BATCH: usize = 64;

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

/// Columns selected for a `DeckRecord`, in field order (12 columns; requires
/// table alias `d`).
const DECK_COLS: &str = "d.id, d.path, d.file_name, d.title, d.author, d.slide_count, \
    d.modified_unix, d.size_bytes, d.slide_width_emu, d.slide_height_emu, d.first_seen_unix, \
    EXISTS(SELECT 1 FROM deck_favorites df WHERE df.deck_path = d.path)";
/// Columns selected for a `SlideRecord`, in field order (9 columns; requires
/// table aliases `s` AND `d` — the favorite flag is keyed by deck path).
const SLIDE_COLS: &str = "s.id, s.deck_id, s.slide_index, s.title, s.body_text, s.notes, s.thumb_path, \
    EXISTS(SELECT 1 FROM slide_favorites sf WHERE sf.deck_path = d.path AND sf.slide_index = s.slide_index), \
    s.content_hash";

/// bm25 weights: title > deck_title > body > notes.
const BM25: &str = "bm25(slides_fts, 4.0, 1.0, 0.6, 2.0)";

/// Correlated subquery counting the currently-indexed slides that carry tag
/// `t.id`. The join to `decks`+`slides` drops orphaned assignments (deck removed)
/// and rows whose slide is no longer indexed, so the count is always "live".
/// Requires the outer query to alias the tags table as `t`.
const TAG_SLIDE_COUNT: &str = "COALESCE((SELECT COUNT(*) FROM slide_tags stc \
     JOIN decks dc ON dc.path = stc.deck_path \
     JOIN slides sc ON sc.deck_id = dc.id AND sc.slide_index = stc.slide_index \
     WHERE stc.tag_id = t.id), 0)";

pub struct Library {
    conn: Connection,
    /// Optional embedder; when set, the scan path embeds new slide texts and
    /// semantic/hybrid search + find-similar + near-dup detection are enabled.
    embedder: Option<Arc<dyn Embedder>>,
    /// Lazily-loaded in-memory vectors for the active model. `None` = not yet
    /// loaded (or invalidated by a scan/backfill); interior-mutable so read-only
    /// (`&self`) search paths can populate it on first use. Held behind an `Arc`
    /// so the (expensive, O(n²)) duplicate-clustering can snapshot the store under
    /// a short lock and run off the interactive mutex (see `ensure_vectors_loaded`
    /// / `finish_duplicate_groups`).
    vectors: RefCell<Option<Arc<VectorStore>>>,
    /// Memoized near-duplicate clusters (slide-id groups), invalidated whenever
    /// the library changes. Recomputing is O(n²), so the Duplicates view reuses it.
    near_clusters: RefCell<Option<Vec<Vec<i64>>>>,
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
        conn.execute_batch(SCHEMA)?; // frozen v0 baseline (IF NOT EXISTS, idempotent)
        let mut lib = Library {
            conn,
            embedder: None,
            vectors: RefCell::new(None),
            near_clusters: RefCell::new(None),
        };
        lib.migrate()?;
        Ok(lib)
    }

    /// Attach (or detach) the embedder. Detaching, or swapping models, invalidates
    /// the in-memory vector caches. Setting this enables the semantic features.
    pub fn set_embedder(&mut self, embedder: Option<Arc<dyn Embedder>>) {
        self.embedder = embedder;
        self.invalidate_vector_cache();
    }

    /// Whether a model is attached (semantic features available).
    pub fn has_embedder(&self) -> bool {
        self.embedder.is_some()
    }

    /// A cloned handle to the attached embedder, if any. Lets the desktop backfill
    /// snapshot the embedder under a short lock and then run the CPU-bound
    /// `embed_passages` with NO library lock held (it stores the result via
    /// [`store_embedding_vectors`] under a separate short lock).
    pub fn embedder_handle(&self) -> Option<Arc<dyn Embedder>> {
        self.embedder.clone()
    }

    /// Drop the lazily-loaded vector store and memoized near-dup clusters. Call
    /// after any change to slides or embeddings (scan/backfill completion).
    pub fn invalidate_vector_cache(&self) {
        *self.vectors.borrow_mut() = None;
        *self.near_clusters.borrow_mut() = None;
    }

    /// Apply additive migrations up to SCHEMA_VERSION. Each migration + its
    /// user_version bump commit in ONE tx, so a crash mid-migration rolls back
    /// cleanly (never leaves added columns at version 0, which would re-run the
    /// ALTER and hit duplicate-column on the next open).
    fn migrate(&mut self) -> Result<()> {
        let mut version: i64 = self.conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        while version < SCHEMA_VERSION {
            let next = version + 1;
            let tx = self.conn.transaction()?;
            match next {
                1 => tx.execute_batch(MIGRATIONS_V1)?,
                2 => tx.execute_batch(MIGRATIONS_V2)?,
                3 => tx.execute_batch(MIGRATIONS_V3)?,
                4 => tx.execute_batch(MIGRATIONS_V4)?,
                _ => unreachable!("no migration for schema v{next}"),
            }
            // PRAGMA cannot bind params — format the (internal, trusted) integer.
            tx.execute_batch(&format!("PRAGMA user_version = {next};"))?;
            tx.commit()?;
            version = next;
        }
        Ok(())
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
        // Slide rows (and their rowids) are gone — the in-memory vector/near-dup
        // caches would otherwise serve removed slides, or the wrong slide once a
        // rowid is recycled. Drop them.
        self.invalidate_vector_cache();
        Ok(())
    }

    /// Wipe all indexed content (decks, slides, FTS, and activity history) in a
    /// single transaction, keeping the configured roots — with their
    /// `exclude_globs` — and both favorites tables so stars survive and relink
    /// on the next rescan. Tags (`tags` + `slide_tags`) are likewise untouched, so
    /// tag assignments relink by (deck_path, slide_index) after the rescan.
    /// `last_scan_unix` is reset to NULL so the library reads as unscanned until
    /// the follow-up scan runs. `foreign_keys` is ON (set in `init`), so deleting
    /// `scan_history` / `export_history` cascades to their `scan_issues` /
    /// `export_picks` children; `render_issues` has no FK and is deleted
    /// explicitly.
    pub fn clear(&mut self) -> Result<()> {
        let tx = self.conn.transaction()?;
        // Whole-table wipes (no params). A content-owning FTS5 table supports a
        // plain DELETE, same as the per-row DELETEs used elsewhere. `tags` and
        // `slide_tags` are deliberately NOT wiped — like favorites, they relink
        // by (deck_path, slide_index) on the next scan.
        tx.execute_batch(
            "DELETE FROM slides_fts;
             DELETE FROM slides;
             DELETE FROM decks;
             DELETE FROM scan_history;
             DELETE FROM search_history;
             DELETE FROM export_history;
             DELETE FROM render_issues;
             UPDATE roots SET last_scan_unix = NULL;",
        )?;
        tx.commit()?;
        // Every slide row is gone and rowids restart on the follow-up rebuild, so
        // any lazily-loaded vector store now points at slides that no longer
        // exist. Drop it (and the memoized near-dup clusters) so semantic search
        // reloads from the empty/rebuilt index instead of serving stale ids.
        self.invalidate_vector_cache();
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

    /// Replace a root's exclude globs. Blank lines are dropped; every remaining
    /// pattern is compiled (and the whole set built) BEFORE any write, so an
    /// invalid pattern is rejected without touching the stored value. The globs
    /// are persisted as a JSON array in `roots.exclude_globs` and take effect on
    /// the next scan.
    pub fn set_root_excludes(&mut self, root_id: i64, patterns: &[String]) -> Result<RootRecord> {
        let cleaned: Vec<String> = patterns
            .iter()
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect();
        let mut builder = GlobSetBuilder::new();
        for p in &cleaned {
            let g = Glob::new(p).map_err(|e| Error::InvalidGlob(format!("{p}: {e}")))?;
            builder.add(g);
        }
        builder.build().map_err(|e| Error::InvalidGlob(e.to_string()))?;
        let json = serde_json::to_string(&cleaned).map_err(|e| Error::InvalidGlob(e.to_string()))?;
        self.conn
            .execute("UPDATE roots SET exclude_globs=?2 WHERE id=?1", params![root_id, json])?;
        self.root_record(root_id)
    }

    /// Incrementally (re)scan all roots. `progress` is called from the
    /// scanning thread; it must be cheap.
    ///
    /// The in-memory vector/near-dup caches are dropped on EVERY exit (success or
    /// error): a scan deletes+reinserts deck/slide rows (recycling rowids), so a
    /// mid-scan failure can leave a cache pointing at slides that were already
    /// rewritten. Invalidating unconditionally keeps semantic search from serving
    /// stale ids after a partial scan.
    pub fn scan(&mut self, progress: &mut dyn FnMut(ScanEvent)) -> Result<()> {
        let result = self.scan_inner(progress);
        self.invalidate_vector_cache();
        result
    }

    fn scan_inner(&mut self, progress: &mut dyn FnMut(ScanEvent)) -> Result<()> {
        let scan_started = Instant::now();
        let started_unix = now_unix();
        // Snapshot roots up front, compiling each root's exclude globs into a
        // raw file-match set and a directory-prune set.
        let roots: Vec<(i64, String, GlobSet, GlobSet)> = {
            let mut stmt = self.conn.prepare("SELECT id, path, exclude_globs FROM roots")?;
            let rows: Vec<(i64, String, String)> = stmt
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<rusqlite::Result<_>>()?;
            rows.into_iter()
                .map(|(id, path, json)| {
                    let patterns = serde_json::from_str::<Vec<String>>(&json).unwrap_or_default();
                    let (raw, prune) = build_glob_sets(&patterns);
                    (id, path, raw, prune)
                })
                .collect()
        };

        // Enumerate candidate .pptx files across all roots.
        let mut candidates: Vec<(i64, PathBuf)> = Vec::new();
        for (root_id, root_path, raw_set, prune_set) in &roots {
            let root = Path::new(root_path);
            for entry in WalkDir::new(root_path)
                .into_iter()
                .filter_entry(|e| {
                    if is_pruned_dir(e.path()) {
                        return false;
                    }
                    if e.file_type().is_dir() {
                        if let Some(rel) = rel_forward_slash(root, e.path()) {
                            if !rel.is_empty() && prune_set.is_match(&rel) {
                                return false; // skip whole excluded subtree
                            }
                        }
                    }
                    true
                })
                .filter_map(|e| e.ok())
            {
                if entry.file_type().is_file() && is_pptx_file(entry.path()) {
                    if let Some(rel) = rel_forward_slash(root, entry.path()) {
                        if raw_set.is_match(&rel) {
                            continue;
                        }
                    }
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
        let mut skipped: Vec<(String, String)> = Vec::new();

        for (i, (root_id, path)) in candidates.into_iter().enumerate() {
            let done = i + 1;
            let path_str = path.to_string_lossy().to_string();
            seen.insert(path_str.clone());

            let meta = match std::fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    let reason = e.to_string();
                    skipped.push((path_str.clone(), reason.clone()));
                    progress(ScanEvent::Skipped { path: path_str, reason });
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
                    let reason = e.to_string();
                    skipped.push((path_str.clone(), reason.clone()));
                    progress(ScanEvent::Skipped { path: path_str, reason });
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

        // Record the run for the stats view (best-effort). Insert history first so
        // we can attach one scan_issues row per buffered skip via its rowid.
        let history_written = self
            .conn
            .execute(
                "INSERT INTO scan_history(started_unix, duration_ms, indexed, removed, unchanged, skipped) \
                 VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    started_unix,
                    scan_started.elapsed().as_millis() as i64,
                    indexed as i64,
                    removed as i64,
                    unchanged as i64,
                    skipped.len() as i64,
                ],
            )
            .is_ok();
        if history_written {
            let scan_id = self.conn.last_insert_rowid();
            for (path, reason) in &skipped {
                let _ = self.conn.execute(
                    "INSERT INTO scan_issues(scan_id, path, reason) VALUES(?1,?2,?3)",
                    params![scan_id, path, reason],
                );
            }
        }
        // Keep the table bounded; scan_issues ON DELETE CASCADE drops trimmed scans' issues.
        let _ = self.conn.execute(
            "DELETE FROM scan_history WHERE id NOT IN \
             (SELECT id FROM scan_history ORDER BY id DESC LIMIT 50)",
            [],
        );

        // Drop embeddings orphaned by removed/changed slides. The vector/near-dup
        // caches are invalidated by the `scan` wrapper on every exit, so this
        // success tail only handles the orphan cleanup.
        self.cleanup_orphan_embeddings()?;

        progress(ScanEvent::Finished { indexed, removed, unchanged, skipped: skipped.len() });
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
                     modified_unix, size_bytes, slide_width_emu, slide_height_emu, content_hash, \
                     first_seen_unix) \
                     VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
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
                        now_unix(),
                    ],
                )?;
                tx.last_insert_rowid()
            }
        };

        for slide in &deck.slides {
            tx.execute(
                "INSERT INTO slides(deck_id, slide_index, title, body_text, notes, thumb_path, \
                 content_hash, text_hash) \
                 VALUES(?1,?2,?3,?4,?5,NULL,?6,?7)",
                params![
                    deck_id,
                    slide.index,
                    slide.title,
                    slide.body_text,
                    slide.notes,
                    slide.content_hash,
                    slide.text_hash,
                ],
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

        // With a model attached, embed any of this deck's slide texts not already
        // vectorized (keyed by text_hash, so unchanged text and cross-deck reuse
        // are skipped). Runs after the slide rows are committed.
        if self.embedder.is_some() {
            let pairs: Vec<(String, String)> = deck
                .slides
                .iter()
                .filter_map(|s| match (&s.text_hash, &s.embed_text) {
                    (Some(th), Some(t)) => Some((th.clone(), t.clone())),
                    _ => None,
                })
                .collect();
            self.embed_and_store_missing(&pairs, None)?;
        }
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
        // This deck's slide rowids are freed and may be recycled by a later
        // insert; invalidate so the vector/near-dup caches can't outlive them.
        self.invalidate_vector_cache();
        Ok(())
    }

    /// Full-text search. Empty/whitespace query returns recent slides
    /// honoring the filters (browse mode).
    pub fn search(&self, query: &str, filters: &SearchFilters) -> Result<Vec<SearchHit>> {
        let limit = filters.limit.unwrap_or(200) as i64;

        // Parse ONCE: the advanced query syntax becomes an FTS5 MATCH expression,
        // `before:`/`after:` date bounds lifted out of the text, and the plain
        // semantic text (positive-term content only — see ParsedQuery).
        let parsed = parse_query(query);

        // Fold the parsed date bounds into the caller's filters, combining
        // restrictively (max of the froms, min of the tos) so a `before:`/`after:`
        // in the query box can only narrow a range the FilterPopover set. EVERY
        // retrieval arm below — FTS, browse, and vector post-filtering — sees
        // these effective filters.
        let mut eff = filters.clone();
        eff.modified_from = max_opt(eff.modified_from, parsed.after);
        eff.modified_to = min_opt(eff.modified_to, parsed.before);

        // Mode dispatch AFTER parsing. Semantic/hybrid need a model and some
        // embeddable text; otherwise (lexical mode, no model, or a query that
        // reduced to no positive terms) this is exactly the lexical flow — the
        // embedder is never touched, and no mode ever errors.
        let mode = filters.search_mode.as_deref().unwrap_or("lexical");
        let want_vector = matches!(mode, "semantic" | "hybrid")
            && self.embedder.is_some()
            && !parsed.semantic_text.is_empty();
        if !want_vector {
            return self.lexical_flow(query, &parsed, &eff, limit);
        }
        match mode {
            "semantic" => self.semantic_search(&parsed.semantic_text, &eff, limit as usize),
            _ => self.hybrid_search(query, &parsed, &eff, limit as usize),
        }
    }

    /// The lexical retrieval flow (advanced-syntax FTS with fallbacks), shared by
    /// lexical mode and the hybrid FTS arm.
    fn lexical_flow(
        &self,
        raw: &str,
        parsed: &ParsedQuery,
        eff: &SearchFilters,
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
        match &parsed.match_expr {
            // Primary path: run the parsed MATCH. It is constructed to be valid
            // FTS5, but as a belt-and-suspenders guarantee that a user never sees
            // an FTS syntax error, fall back to plain tokens if it ever errors.
            Some(match_str) => match self.run_fts(match_str, eff, limit) {
                Ok(hits) => Ok(hits),
                Err(_) => self.search_plain(raw, eff, limit),
            },
            // No positive text term (empty, date-only, or purely negative): fall
            // back on the RESIDUAL — only the tokens the parser could not
            // classify — never the raw input. Tokenizing the raw input here would
            // text-search consumed tokens (e.g. a date-only query's digits) and
            // wrongly return zero hits; with the residual, date-only and
            // purely-negative queries browse with the merged filters applied.
            None => self.search_plain(&parsed.residual, eff, limit),
        }
    }

    /// Fallback search equivalent to the pre-advanced-syntax behavior: reduce the
    /// query to plain alphanumeric tokens (`"tok"*`, implicit AND). An empty token
    /// set is browse mode, honoring `filters` (including any merged date bounds).
    fn search_plain(
        &self,
        query: &str,
        filters: &SearchFilters,
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
        let tokens = sanitize_query(query);
        if tokens.is_empty() {
            return self.browse(filters, limit);
        }
        let match_str = tokens
            .iter()
            .map(|t| format!("\"{t}\"*"))
            .collect::<Vec<_>>()
            .join(" ");
        self.run_fts(&match_str, filters, limit)
    }

    /// Execute an FTS5 `MATCH` against the slide index with `filters` applied,
    /// bm25-ranked with a `<mark>`-wrapped body snippet. `match_str` must be a
    /// valid FTS5 query expression.
    fn run_fts(
        &self,
        match_str: &str,
        filters: &SearchFilters,
        limit: i64,
    ) -> Result<Vec<SearchHit>> {
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
        params.push(Value::Text(match_str.to_string()));
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

    /// Semantic-only retrieval: cosine top-`limit` over the vector store for the
    /// parsed query's semantic text, post-filtered to the EFFECTIVE filter set
    /// (structured filters + parsed date bounds), with a fallback body snippet
    /// per hit (there is no FTS match to mark up).
    fn semantic_search(
        &self,
        semantic_text: &str,
        eff: &SearchFilters,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let pool = self.vector_pool(semantic_text, eff, limit)?;
        let ids: Vec<i64> = pool.iter().map(|(id, _)| *id).collect();
        let fetched = self.fetch_slides_with_decks(&ids)?;
        let mut out = Vec::with_capacity(pool.len());
        for (id, score) in pool {
            if let Some((slide, deck, body)) = fetched.get(&id) {
                out.push(SearchHit {
                    slide: slide.clone(),
                    deck: deck.clone(),
                    snippet: fallback_snippet(body),
                    score: score as f64,
                });
            }
        }
        Ok(out)
    }

    /// Hybrid retrieval: reciprocal-rank fusion of the lexical top-N and the
    /// cosine top-N. The FTS arm is exactly the lexical flow (parsed MATCH with
    /// its fallbacks) over the effective filters — except that when the query
    /// has no positive term and no residual tokens it contributes an EMPTY list
    /// rather than browse results (fusing recency-ordered browse hits into a
    /// ranked semantic result would poison the ranking). The vector arm embeds
    /// only the parsed semantic text and is post-filtered against the same
    /// effective filters, so `before:`/`after:`, tags, and favorites constrain
    /// both arms identically. FTS hits keep their `<mark>` snippet; semantic-only
    /// hits get the fallback body snippet. Result score is the RRF score.
    fn hybrid_search(
        &self,
        raw: &str,
        parsed: &ParsedQuery,
        eff: &SearchFilters,
        limit: usize,
    ) -> Result<Vec<SearchHit>> {
        let fts = match &parsed.match_expr {
            Some(match_str) => match self.run_fts(match_str, eff, HYBRID_POOL as i64) {
                Ok(hits) => hits,
                // Same belt-and-suspenders fallback as the lexical flow; a Some
                // match_expr implies raw has alphanumeric tokens, so this never
                // reaches browse.
                Err(_) => self.search_plain(raw, eff, HYBRID_POOL as i64)?,
            },
            None if sanitize_query(&parsed.residual).is_empty() => Vec::new(),
            None => self.search_plain(&parsed.residual, eff, HYBRID_POOL as i64)?,
        };
        let vec = self.vector_pool(&parsed.semantic_text, eff, HYBRID_POOL)?;

        let fts_ids: Vec<i64> = fts.iter().map(|h| h.slide.id).collect();
        let vec_ids: Vec<i64> = vec.iter().map(|(id, _)| *id).collect();
        let fused = rrf_fuse(&[&fts_ids, &vec_ids]);

        let mut fts_map: HashMap<i64, SearchHit> =
            fts.into_iter().map(|h| (h.slide.id, h)).collect();
        let top: Vec<(i64, f64)> = fused.into_iter().take(limit).collect();
        let missing: Vec<i64> =
            top.iter().map(|(id, _)| *id).filter(|id| !fts_map.contains_key(id)).collect();
        let fetched = self.fetch_slides_with_decks(&missing)?;

        let mut out = Vec::with_capacity(top.len());
        for (id, score) in top {
            if let Some(mut hit) = fts_map.remove(&id) {
                hit.score = score;
                out.push(hit);
            } else if let Some((slide, deck, body)) = fetched.get(&id) {
                out.push(SearchHit {
                    slide: slide.clone(),
                    deck: deck.clone(),
                    snippet: fallback_snippet(body),
                    score,
                });
            }
        }
        Ok(out)
    }

    // --- saved searches ------------------------------------------------------

    /// All saved searches, in sidebar order (position, then insertion order).
    pub fn list_saved_searches(&self) -> Result<Vec<SavedSearch>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, name, query, filters_json, created_unix \
             FROM saved_searches ORDER BY position ASC, id ASC",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let filters_json: String = r.get(3)?;
                Ok(SavedSearch {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    query: r.get(2)?,
                    // Tolerate legacy/garbled JSON by falling back to no filters.
                    filters: serde_json::from_str(&filters_json).unwrap_or_default(),
                    created_unix: r.get(4)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Persist a new saved search at the end of the list; returns the stored row.
    pub fn save_search(
        &mut self,
        name: &str,
        query: &str,
        filters: &SearchFilters,
    ) -> Result<SavedSearch> {
        let filters_json = serde_json::to_string(filters).unwrap_or_else(|_| "{}".into());
        let created_unix = now_unix();
        let position: i64 = self.conn.query_row(
            "SELECT COALESCE(MAX(position) + 1, 0) FROM saved_searches",
            [],
            |r| r.get(0),
        )?;
        self.conn.execute(
            "INSERT INTO saved_searches(name, query, filters_json, position, created_unix) \
             VALUES(?1, ?2, ?3, ?4, ?5)",
            params![name, query, filters_json, position, created_unix],
        )?;
        let id = self.conn.last_insert_rowid();
        Ok(SavedSearch {
            id,
            name: name.to_string(),
            query: query.to_string(),
            filters: filters.clone(),
            created_unix,
        })
    }

    /// Rename a saved search (no-op if the id is unknown).
    pub fn rename_saved_search(&mut self, id: i64, name: &str) -> Result<()> {
        self.conn
            .execute("UPDATE saved_searches SET name = ?2 WHERE id = ?1", params![id, name])?;
        Ok(())
    }

    /// Delete a saved search (no-op if the id is unknown).
    pub fn delete_saved_search(&mut self, id: i64) -> Result<()> {
        self.conn.execute("DELETE FROM saved_searches WHERE id = ?1", params![id])?;
        Ok(())
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

        // Order by the requested browse key so the LIMIT window selects the
        // correct top-N (the frontend then reorders that window for grouping,
        // but the set it sees is already the right one). Decks stay contiguous
        // (deck key → deck id → slide_index) so a group-by-deck view never
        // interleaves and the window cuts on deck boundaries. "name" mirrors the
        // frontend's file-name display order; unknown/`None` keeps the historical
        // modified-DESC default.
        let order_sql = match filters.sort.as_deref() {
            Some("name") => "d.file_name COLLATE NOCASE ASC, d.id ASC, s.slide_index ASC",
            Some("added") => "d.first_seen_unix DESC, d.id ASC, s.slide_index ASC",
            Some("exported") => {
                "(SELECT COUNT(*) FROM export_picks ep WHERE ep.deck_path = d.path) DESC, \
                 d.modified_unix DESC, d.id ASC, s.slide_index ASC"
            }
            _ => "d.modified_unix DESC, d.id ASC, s.slide_index ASC",
        };

        let sql = format!(
            "SELECT {SLIDE_COLS}, {DECK_COLS}, s.body_text \
             FROM slides s JOIN decks d ON d.id = s.deck_id{where_sql} \
             ORDER BY {order_sql} LIMIT ?"
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

    // --- semantic search / find-similar / duplicates ------------------------

    /// Lazily (re)load the in-memory vector store for the active model. No-op when
    /// no embedder is attached or the store is already loaded.
    fn ensure_vectors(&self) -> Result<()> {
        let Some(embedder) = self.embedder.as_ref() else {
            return Ok(());
        };
        if self.vectors.borrow().is_some() {
            return Ok(());
        }
        let model_id = embedder.id().to_string();
        let dims = embedder.dims();
        let mut stmt = self.conn.prepare(
            "SELECT s.id, s.text_hash, e.vector \
             FROM slides s JOIN embeddings e \
             ON e.text_hash = s.text_hash AND e.model_id = ?1 \
             WHERE s.text_hash IS NOT NULL ORDER BY s.id",
        )?;
        let rows = stmt
            .query_map(params![model_id], |r| {
                let id: i64 = r.get(0)?;
                let th: String = r.get(1)?;
                let blob: Vec<u8> = r.get(2)?;
                Ok((id, th, blob_to_vec(&blob)))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        *self.vectors.borrow_mut() = Some(Arc::new(VectorStore::new(model_id, dims, rows)));
        Ok(())
    }

    /// Ensure the vector store is loaded and hand back a cheap `Arc` snapshot of
    /// it (`None` when no model is attached). Callers that must run heavy vector
    /// work off the interactive mutex take this snapshot under a short lock,
    /// release, and compute on the clone.
    pub fn ensure_vectors_loaded(&self) -> Result<Option<Arc<VectorStore>>> {
        self.ensure_vectors()?;
        Ok(self.vectors.borrow().clone())
    }

    /// The memoized raw near-duplicate clusters, if they've been computed since
    /// the last cache invalidation. `None` means "not computed yet" — the caller
    /// must (re)cluster (see [`near_dup_clusters_for`]).
    pub fn cached_near_clusters(&self) -> Option<Vec<Vec<i64>>> {
        self.near_clusters.borrow().clone()
    }

    /// Cosine-top-`cap` slide ids (+scores) for `query`, post-filtered to the
    /// structured-filter allowed set when any filter is active. Empty when no
    /// model/vectors are available.
    fn vector_pool(&self, query: &str, filters: &SearchFilters, cap: usize) -> Result<Vec<(i64, f32)>> {
        let Some(embedder) = self.embedder.clone() else {
            return Ok(Vec::new());
        };
        self.ensure_vectors()?;
        let qv = embedder.embed_query(query)?;
        let allowed = if filters_active(filters) {
            Some(self.allowed_slide_ids(filters)?)
        } else {
            None
        };
        let guard = self.vectors.borrow();
        let Some(store) = guard.as_ref() else {
            return Ok(Vec::new());
        };
        if store.is_empty() {
            return Ok(Vec::new());
        }
        let mut hits = store.top_k(&qv, cap, |i| match &allowed {
            Some(set) => !set.contains(&store.slide_ids()[i]),
            None => false,
        });
        // Drop below-floor hits so a query no longer surfaces the whole embedded
        // corpus. Applied here (raw cosine still available) it covers semantic
        // mode AND hybrid's vector arm — filtering BEFORE fusion, since post-RRF
        // the cosine is gone. `top_k` itself is left untouched (it is shared with
        // near-dup clustering, which floors at its own NEAR_DUP_THRESHOLD).
        hits.retain(|(_, s)| *s >= SEMANTIC_SCORE_FLOOR);
        Ok(hits.into_iter().map(|(i, s)| (store.slide_ids()[i], s)).collect())
    }

    /// Slide ids satisfying the structured filters (no text match) — the allowed
    /// set for post-filtering vector hits.
    fn allowed_slide_ids(&self, filters: &SearchFilters) -> Result<HashSet<i64>> {
        let mut clauses = Vec::new();
        let mut fparams = Vec::new();
        push_filters(filters, &mut clauses, &mut fparams);
        let where_sql = if clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", clauses.join(" AND "))
        };
        let sql = format!("SELECT s.id FROM slides s JOIN decks d ON d.id = s.deck_id{where_sql}");
        let mut stmt = self.conn.prepare(&sql)?;
        let ids = stmt
            .query_map(params_from_iter(fparams), |r| r.get::<_, i64>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(ids)
    }

    /// Fetch `(SlideRecord, DeckRecord, body_text)` for a set of slide ids, keyed
    /// by slide id. Missing ids are simply absent from the map.
    fn fetch_slides_with_decks(
        &self,
        ids: &[i64],
    ) -> Result<HashMap<i64, (SlideRecord, DeckRecord, String)>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT {SLIDE_COLS}, {DECK_COLS}, s.body_text \
             FROM slides s JOIN decks d ON d.id = s.deck_id WHERE s.id IN ({placeholders})"
        );
        let params: Vec<Value> = ids.iter().map(|id| Value::Integer(*id)).collect();
        let mut stmt = self.conn.prepare(&sql)?;
        let mut map = HashMap::with_capacity(ids.len());
        let rows = stmt.query_map(params_from_iter(params), |row| {
            let slide = row_to_slide(row, 0)?;
            let deck = row_to_deck(row, SLIDE_COL_COUNT)?;
            let body: String = row.get(SLIDE_COL_COUNT + DECK_COL_COUNT)?;
            Ok((slide.id, (slide, deck, body)))
        })?;
        for r in rows {
            let (id, v) = r?;
            map.insert(id, v);
        }
        Ok(map)
    }

    /// Slides semantically closest to `slide_id` (find-similar, roadmap #6).
    /// Excludes the anchor and any same-text twin. Empty when the model is absent
    /// or the anchor isn't embedded — never an error.
    pub fn get_similar_slides(&self, slide_id: i64, limit: usize) -> Result<Vec<SimilarSlide>> {
        if self.embedder.is_none() {
            return Ok(Vec::new());
        }
        self.ensure_vectors()?;
        let scored: Vec<(i64, f32)> = {
            let guard = self.vectors.borrow();
            let Some(store) = guard.as_ref() else {
                return Ok(Vec::new());
            };
            let Some(row) = store.row_of_slide(slide_id) else {
                return Ok(Vec::new());
            };
            let anchor_hash = store.text_hash_at(row).to_string();
            let qv = store.row(row).to_vec();
            store
                .top_k(&qv, limit, |i| {
                    store.slide_ids()[i] == slide_id || store.text_hash_at(i) == anchor_hash
                })
                .into_iter()
                // Drop below-floor neighbors (raw cosine) so find-similar shows
                // only genuinely related slides, not unrelated padding.
                .filter(|(_, s)| *s >= SEMANTIC_SCORE_FLOOR)
                .map(|(i, s)| (store.slide_ids()[i], s))
                .collect()
        };
        let ids: Vec<i64> = scored.iter().map(|(id, _)| *id).collect();
        let fetched = self.fetch_slides_with_decks(&ids)?;
        let mut out = Vec::with_capacity(scored.len());
        for (id, score) in scored {
            if let Some((slide, deck, _)) = fetched.get(&id) {
                out.push(SimilarSlide {
                    slide: slide.clone(),
                    deck: deck.clone(),
                    score: score as f64,
                });
            }
        }
        Ok(out)
    }

    /// All duplicate groups: exact (identical content hash) first, then near
    /// (embedding-similar, when a model is attached). Near groups redundant with
    /// an exact group (all members share one content hash) are omitted.
    pub fn list_duplicate_groups(&self) -> Result<Vec<DuplicateGroup>> {
        // Single-threaded convenience path (tests / CLI): load, cluster, hydrate
        // inline. The desktop host instead drives these pieces itself so the
        // O(n²) clustering runs off the interactive mutex (see commands.rs).
        let store = self.ensure_vectors_loaded()?;
        let exact = self.exact_dup_groups()?;
        let near_raw = match self.cached_near_clusters() {
            Some(clusters) => clusters,
            None => match &store {
                Some(store) => near_dup_clusters_for(store),
                None => Vec::new(),
            },
        };
        self.finish_duplicate_groups(exact, near_raw, store.as_ref())
    }

    /// Turn raw exact + near cluster id-lists into the hydrated [`DuplicateGroup`]
    /// payload: memoize the freshly-computed near clusters (only while the vector
    /// snapshot they came from is still live), drop near groups already covered by
    /// an exact group, fetch rows, and score near-group cohesion from `store`.
    ///
    /// `near_raw` is the *raw* clustering output (pre content-hash filtering), as
    /// produced by [`near_dup_clusters_for`]; `store` is the snapshot those
    /// clusters were computed from (used for cohesion and the memo-freshness check).
    pub fn finish_duplicate_groups(
        &self,
        exact: Vec<Vec<i64>>,
        near_raw: Vec<Vec<i64>>,
        store: Option<&Arc<VectorStore>>,
    ) -> Result<Vec<DuplicateGroup>> {
        // Memoize the raw clusters for the next call — but only if the store they
        // were built from is still the live one. A concurrent scan/backfill may
        // have invalidated (or reloaded) the vectors while we clustered off-lock;
        // caching stale clusters would then outlive their slides.
        if let Some(store) = store {
            let live = self.vectors.borrow();
            if live.as_ref().is_some_and(|v| Arc::ptr_eq(v, store)) {
                *self.near_clusters.borrow_mut() = Some(near_raw.clone());
            }
        }

        // Drop near clusters whose members all share one content hash — those are
        // already surfaced as an exact group.
        let mut near: Vec<Vec<i64>> = Vec::new();
        for g in near_raw {
            if !self.all_same_content_hash(&g)? {
                near.push(g);
            }
        }

        let mut all_ids: Vec<i64> = Vec::new();
        for g in exact.iter().chain(near.iter()) {
            all_ids.extend_from_slice(g);
        }
        let fetched = self.fetch_slides_with_decks(&all_ids)?;

        let build = |ids: &[i64]| -> Vec<DuplicateSlide> {
            let mut v: Vec<DuplicateSlide> = ids
                .iter()
                .filter_map(|id| fetched.get(id))
                .map(|(slide, deck, _)| DuplicateSlide { slide: slide.clone(), deck: deck.clone() })
                .collect();
            // Newest-modified first so the UI can badge the newest copy.
            v.sort_by(|a, b| {
                b.deck.modified_unix.cmp(&a.deck.modified_unix).then(a.slide.id.cmp(&b.slide.id))
            });
            v
        };

        let mut out = Vec::with_capacity(exact.len() + near.len());
        for g in &exact {
            let slides = build(g);
            if slides.len() >= 2 {
                out.push(DuplicateGroup { kind: "exact".into(), score: None, slides });
            }
        }
        for g in &near {
            let slides = build(g);
            if slides.len() < 2 {
                continue;
            }
            let score = store.and_then(|s| s.group_cohesion(g));
            out.push(DuplicateGroup { kind: "near".into(), score, slides });
        }
        Ok(out)
    }

    /// Exact-duplicate slide-id groups (identical `content_hash`, count > 1),
    /// largest first.
    pub fn exact_dup_groups(&self) -> Result<Vec<Vec<i64>>> {
        let mut stmt = self.conn.prepare(
            "SELECT content_hash, s.id FROM slides s \
             WHERE content_hash IN \
               (SELECT content_hash FROM slides WHERE content_hash IS NOT NULL \
                GROUP BY content_hash HAVING COUNT(*) > 1) \
             ORDER BY content_hash, s.id",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        let mut groups: Vec<Vec<i64>> = Vec::new();
        let mut cur_hash: Option<String> = None;
        for (hash, id) in rows {
            if cur_hash.as_deref() != Some(hash.as_str()) {
                groups.push(Vec::new());
                cur_hash = Some(hash);
            }
            groups.last_mut().unwrap().push(id);
        }
        groups.sort_by(|a, b| b.len().cmp(&a.len()).then(a[0].cmp(&b[0])));
        Ok(groups)
    }

    /// Whether every slide id in `ids` shares one non-null content hash (→ already
    /// captured by an exact group).
    fn all_same_content_hash(&self, ids: &[i64]) -> Result<bool> {
        if ids.len() < 2 {
            return Ok(false);
        }
        let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT COUNT(DISTINCT content_hash), COUNT(*), COUNT(content_hash) \
             FROM slides WHERE id IN ({placeholders})"
        );
        let params: Vec<Value> = ids.iter().map(|id| Value::Integer(*id)).collect();
        let (distinct, total, non_null): (i64, i64, i64) =
            self.conn.query_row(&sql, params_from_iter(params), |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?))
            })?;
        Ok(distinct == 1 && non_null == total)
    }

    // --- embedding pipeline -------------------------------------------------

    /// Embed and store any `(text_hash, embed_text)` not already vectorized for
    /// the active model. Dedupes within the batch, filters against existing rows,
    /// embeds in batches of [`EMBED_BATCH`]. Returns the number of new vectors.
    fn embed_and_store_missing(
        &mut self,
        pairs: &[(String, String)],
        cancel: Option<&AtomicBool>,
    ) -> Result<usize> {
        let Some(embedder) = self.embedder.clone() else {
            return Ok(0);
        };
        let model_id = embedder.id().to_string();
        let dims = embedder.dims();

        let mut seen: HashSet<&str> = HashSet::new();
        let mut todo: Vec<(String, String)> = Vec::new();
        for (th, text) in pairs {
            if !seen.insert(th.as_str()) {
                continue;
            }
            let exists: bool = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM embeddings WHERE model_id=?1 AND text_hash=?2)",
                params![model_id, th],
                |r| r.get(0),
            )?;
            if !exists {
                todo.push((th.clone(), text.clone()));
            }
        }
        if todo.is_empty() {
            return Ok(0);
        }

        let mut written = 0usize;
        for chunk in todo.chunks(EMBED_BATCH) {
            // Poll cancellation between model batches so a disable/delete toggle
            // stops a long backfill within one batch instead of one whole run.
            if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                break;
            }
            let texts: Vec<String> = chunk.iter().map(|(_, t)| t.clone()).collect();
            let vectors = embedder.embed_passages(&texts)?;
            written += self.store_embedding_vectors(&model_id, dims, chunk, vectors)?;
        }
        Ok(written)
    }

    /// L2-normalize `vectors` (one per pair in `chunk`, same order) and
    /// `INSERT OR IGNORE` them under `model_id`/`dims` in a single transaction;
    /// returns the number of rows attempted (== `chunk.len()`). This is the
    /// DB-write half of the embedding pipeline, split out so the desktop backfill
    /// can run the CPU-bound `embed_passages` with NO library lock held and then
    /// re-lock only for this store. `INSERT OR IGNORE` makes a redundant embed
    /// race (an interleaved inline scan already vectorized the same text)
    /// correctness-neutral — the second write is silently dropped.
    pub fn store_embedding_vectors(
        &mut self,
        model_id: &str,
        dims: usize,
        chunk: &[(String, String)],
        vectors: Vec<Vec<f32>>,
    ) -> Result<usize> {
        let mut written = 0usize;
        let tx = self.conn.transaction()?;
        for ((th, _), mut vec) in chunk.iter().zip(vectors) {
            crate::embed::store::l2_normalize(&mut vec); // normalize at write time
            let blob = vec_to_blob(&vec);
            tx.execute(
                "INSERT OR IGNORE INTO embeddings(model_id, text_hash, dims, vector) \
                 VALUES(?1,?2,?3,?4)",
                params![model_id, th, dims as i64, blob],
            )?;
            written += 1;
        }
        tx.commit()?;
        Ok(written)
    }

    /// Decks with at least one slide missing its content hash (indexed before
    /// hashing existed). The backfill reparses these to fill the hashes.
    pub fn decks_needing_hash_backfill(&self) -> Result<Vec<(i64, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DISTINCT d.id, d.path FROM decks d \
             JOIN slides s ON s.deck_id = d.id WHERE s.content_hash IS NULL",
        )?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Reparse `path` and fill each slide's content/text hash by slide index,
    /// without touching FTS or embeddings. Brings pre-hashing rows up to date.
    pub fn backfill_deck_hashes(&mut self, deck_id: i64, path: &str) -> Result<()> {
        let deck = extract_deck(Path::new(path))?;
        let tx = self.conn.transaction()?;
        for slide in &deck.slides {
            tx.execute(
                "UPDATE slides SET content_hash=?3, text_hash=?4 \
                 WHERE deck_id=?1 AND slide_index=?2",
                params![deck_id, slide.index, slide.content_hash, slide.text_hash],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    /// `(text_hash, embed_text)` for every distinct slide text lacking a vector for
    /// the active model. `embed_text` is rebuilt from the stored title/body/notes
    /// (identical to the inline path, so it hashes back to `text_hash`). Empty
    /// without a model.
    pub fn pending_embedding_texts(&self) -> Result<Vec<(String, String)>> {
        let Some(embedder) = self.embedder.as_ref() else {
            return Ok(Vec::new());
        };
        let model_id = embedder.id().to_string();
        let mut stmt = self.conn.prepare(
            "SELECT s.text_hash, s.title, s.body_text, s.notes FROM slides s \
             WHERE s.text_hash IS NOT NULL \
               AND NOT EXISTS(SELECT 1 FROM embeddings e \
                 WHERE e.model_id = ?1 AND e.text_hash = s.text_hash) \
             GROUP BY s.text_hash",
        )?;
        let rows = stmt.query_map(params![model_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, Option<String>>(3)?,
            ))
        })?;
        let mut out = Vec::new();
        for row in rows {
            let (th, title, body, notes) = row?;
            if let Some(text) =
                crate::hash::slide_embed_text(title.as_deref(), &body, notes.as_deref())
            {
                out.push((th, text));
            }
        }
        Ok(out)
    }

    /// Embed and store a chunk of `(text_hash, embed_text)` pairs (idempotent;
    /// skips already-stored texts). Used by the desktop backfill loop. Returns the
    /// number of new vectors written.
    pub fn embed_and_store(&mut self, pairs: &[(String, String)]) -> Result<usize> {
        self.embed_and_store_missing(pairs, None)
    }

    /// Like [`embed_and_store`] but polls `cancel` between internal model batches
    /// so the desktop backfill stops promptly (within one batch) when the user
    /// disables/deletes the model — instead of holding the scan connection for a
    /// whole run. A cancel mid-way still commits the batches that completed.
    pub fn embed_and_store_canceled(
        &mut self,
        pairs: &[(String, String)],
        cancel: &AtomicBool,
    ) -> Result<usize> {
        self.embed_and_store_missing(pairs, Some(cancel))
    }

    /// Remove embeddings whose text no longer appears in any slide.
    pub fn cleanup_orphan_embeddings(&mut self) -> Result<usize> {
        let n = self.conn.execute(
            "DELETE FROM embeddings WHERE text_hash NOT IN \
             (SELECT DISTINCT text_hash FROM slides WHERE text_hash IS NOT NULL)",
            [],
        )?;
        Ok(n)
    }

    /// `(embedded_slides, embeddable_slides)` for the active model: slides whose
    /// text has a stored vector, and slides that carry indexable text. With no
    /// model attached, `embedded_slides` is 0.
    pub fn embedding_counts(&self) -> Result<(i64, i64)> {
        let embeddable: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM slides WHERE text_hash IS NOT NULL",
            [],
            |r| r.get(0),
        )?;
        let embedded: i64 = match self.embedder.as_ref() {
            Some(e) => self.conn.query_row(
                "SELECT COUNT(*) FROM slides s WHERE s.text_hash IS NOT NULL \
                 AND EXISTS(SELECT 1 FROM embeddings e \
                   WHERE e.model_id=?1 AND e.text_hash=s.text_hash)",
                params![e.id()],
                |r| r.get(0),
            )?,
            None => 0,
        };
        Ok((embedded, embeddable))
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

    /// The minimum a slide needs to render + cache-key its preview:
    /// `(deck_path, content_hash, slide_index)`. One indexed join; keeps
    /// `content_hash` out of the IPC models. Errors if the slide id is unknown
    /// (e.g. it was deleted) — callers must treat that as "no preview" rather
    /// than serving a possibly-stale cached file keyed on a reused id.
    pub fn slide_render_info(&self, slide_id: i64) -> Result<(String, String, i64)> {
        Ok(self.conn.query_row(
            "SELECT d.path, d.content_hash, s.slide_index \
             FROM slides s JOIN decks d ON d.id = s.deck_id WHERE s.id=?1",
            params![slide_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?)
    }

    /// Persist (upsert) the set of dropped-construct kinds for a rendered slide,
    /// keyed by `(deck_path, slide_index)` and guarded by `content_hash` so a
    /// later edit invalidates the row. An empty `dropped` deletes any existing
    /// row rather than storing `[]`, keeping the table free of no-drop noise.
    pub fn record_render_issues(
        &mut self,
        deck_path: &str,
        slide_index: i64,
        content_hash: &str,
        dropped: &[String],
    ) -> Result<()> {
        if dropped.is_empty() {
            self.conn.execute(
                "DELETE FROM render_issues WHERE deck_path=?1 AND slide_index=?2",
                params![deck_path, slide_index],
            )?;
            return Ok(());
        }
        let json = serde_json::to_string(dropped).unwrap_or_else(|_| "[]".into());
        self.conn.execute(
            "INSERT INTO render_issues(deck_path, slide_index, content_hash, dropped, updated_unix) \
             VALUES(?1,?2,?3,?4,?5) \
             ON CONFLICT(deck_path, slide_index) DO UPDATE SET \
             content_hash=excluded.content_hash, dropped=excluded.dropped, \
             updated_unix=excluded.updated_unix",
            params![deck_path, slide_index, content_hash, json, now_unix()],
        )?;
        Ok(())
    }

    /// The dropped-construct kinds recorded for a slide, but only when the stored
    /// row's `content_hash` still matches (a missing row OR a stale hash both
    /// return empty — the cached render no longer describes the current file).
    pub fn render_issues_for(
        &self,
        deck_path: &str,
        slide_index: i64,
        content_hash: &str,
    ) -> Result<Vec<String>> {
        let row: Option<(String, String)> = self
            .conn
            .query_row(
                "SELECT content_hash, dropped FROM render_issues \
                 WHERE deck_path=?1 AND slide_index=?2",
                params![deck_path, slide_index],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((stored_hash, json)) if stored_hash == content_hash => {
                serde_json::from_str::<Vec<String>>(&json).unwrap_or_default()
            }
            _ => Vec::new(),
        })
    }

    /// Aggregate live render drops by construct kind for the stats view. The JOIN
    /// on `decks(path, content_hash)` drops stale rows (file edited since the
    /// render) and orphans (deck removed), so only current previews are counted.
    /// Each slide's kinds are pre-deduped on write, so `+= 1` per kind per row is
    /// a distinct-slide count. Sorted by slide count desc, then kind asc.
    fn render_drops(&self) -> Result<Vec<RenderDropStat>> {
        let mut stmt = self.conn.prepare(
            "SELECT ri.dropped FROM render_issues ri \
             JOIN decks d ON d.path = ri.deck_path AND d.content_hash = ri.content_hash",
        )?;
        let mut counts: HashMap<String, i64> = HashMap::new();
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for row in rows {
            let json = row?;
            for kind in serde_json::from_str::<Vec<String>>(&json).unwrap_or_default() {
                *counts.entry(kind).or_insert(0) += 1;
            }
        }
        let mut stats: Vec<RenderDropStat> = counts
            .into_iter()
            .map(|(kind, slides)| RenderDropStat { kind, slides })
            .collect();
        stats.sort_by(|a, b| b.slides.cmp(&a.slides).then_with(|| a.kind.cmp(&b.kind)));
        Ok(stats)
    }

    /// `(deck_path, content_hash)` for every indexed deck — the valid-set for
    /// [`crate::thumbs::sweep_thumbs`].
    pub fn all_deck_hashes(&self) -> Result<HashSet<(String, String)>> {
        let mut stmt = self.conn.prepare("SELECT path, content_hash FROM decks")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(rows)
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

    // --- tags ----------------------------------------------------------------
    //
    // Tags are assigned to slides by (deck_path, slide_index) — the favorites
    // convention — so they survive rescans (which delete + reinsert slide rows)
    // and Clear & Rebuild (which never touches `tags`/`slide_tags`). A tag's
    // `slide_count` only counts assignments whose slide is currently in the
    // index; the join below drops orphans (deck removed) and stale rows.

    /// All tags, alphabetical, each with a live count of currently-indexed
    /// slides that carry it.
    pub fn list_tags(&self) -> Result<Vec<TagRecord>> {
        let mut stmt = self.conn.prepare(&format!(
            "SELECT t.id, t.name, {TAG_SLIDE_COUNT} FROM tags t \
             ORDER BY t.name COLLATE NOCASE ASC"
        ))?;
        let rows = stmt
            .query_map([], |r| {
                Ok(TagRecord { id: r.get(0)?, name: r.get(1)?, slide_count: r.get(2)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Tags assigned to one slide (resolved by its current deck_path + index),
    /// alphabetical. `slide_count` is each tag's global live count.
    pub fn slide_tags(&self, slide_id: i64) -> Result<Vec<TagRecord>> {
        let (deck_path, slide_index): (String, i64) = self.conn.query_row(
            "SELECT d.path, s.slide_index FROM slides s JOIN decks d ON d.id = s.deck_id \
             WHERE s.id=?1",
            params![slide_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        let mut stmt = self.conn.prepare(&format!(
            "SELECT t.id, t.name, {TAG_SLIDE_COUNT} FROM tags t \
             JOIN slide_tags st ON st.tag_id = t.id \
             WHERE st.deck_path = ?1 AND st.slide_index = ?2 \
             ORDER BY t.name COLLATE NOCASE ASC"
        ))?;
        let rows = stmt
            .query_map(params![deck_path, slide_index], |r| {
                Ok(TagRecord { id: r.get(0)?, name: r.get(1)?, slide_count: r.get(2)? })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Replace the full set of tags on a slide. Names are trimmed; blanks are
    /// ignored; matching is case-insensitive (an existing tag is reused, else a
    /// new one is created). De-assigned tags are removed from this slide, and any
    /// tag left with zero assignments anywhere is pruned.
    pub fn set_slide_tags(&mut self, slide_id: i64, names: &[String]) -> Result<()> {
        let (deck_path, slide_index): (String, i64) = self.conn.query_row(
            "SELECT d.path, s.slide_index FROM slides s JOIN decks d ON d.id = s.deck_id \
             WHERE s.id=?1",
            params![slide_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;

        let tx = self.conn.transaction()?;

        // Resolve desired tag ids, creating missing tags (case-insensitive).
        let mut desired: Vec<i64> = Vec::new();
        for raw in names {
            let name = raw.trim();
            if name.is_empty() {
                continue;
            }
            let existing: Option<i64> = tx
                .query_row(
                    "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE",
                    params![name],
                    |r| r.get(0),
                )
                .optional()?;
            let id = match existing {
                Some(id) => id,
                None => {
                    tx.execute(
                        "INSERT INTO tags(name, created_unix) VALUES(?1, ?2)",
                        params![name, now_unix()],
                    )?;
                    tx.last_insert_rowid()
                }
            };
            if !desired.contains(&id) {
                desired.push(id);
            }
        }

        // Current assignments for this slide.
        let current: Vec<i64> = {
            let mut stmt =
                tx.prepare("SELECT tag_id FROM slide_tags WHERE deck_path=?1 AND slide_index=?2")?;
            let ids = stmt
                .query_map(params![deck_path, slide_index], |r| r.get(0))?
                .collect::<rusqlite::Result<_>>()?;
            ids
        };

        for id in &desired {
            if !current.contains(id) {
                tx.execute(
                    "INSERT OR IGNORE INTO slide_tags(tag_id, deck_path, slide_index) \
                     VALUES(?1,?2,?3)",
                    params![id, deck_path, slide_index],
                )?;
            }
        }
        for id in &current {
            if !desired.contains(id) {
                tx.execute(
                    "DELETE FROM slide_tags WHERE tag_id=?1 AND deck_path=?2 AND slide_index=?3",
                    params![id, deck_path, slide_index],
                )?;
            }
        }

        // Prune tags with no remaining assignments anywhere.
        tx.execute(
            "DELETE FROM tags WHERE NOT EXISTS(SELECT 1 FROM slide_tags st WHERE st.tag_id = tags.id)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Rename a tag. Errors on a case-insensitive collision with another tag —
    /// v1 never silently merges — and on an empty name or unknown id.
    pub fn rename_tag(&mut self, id: i64, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::InvalidInput("Tag name cannot be empty".into()));
        }
        let clash: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM tags WHERE name = ?1 COLLATE NOCASE AND id <> ?2",
                params![name, id],
                |r| r.get(0),
            )
            .optional()?;
        if clash.is_some() {
            return Err(Error::InvalidInput(format!(
                "A tag named “{name}” already exists"
            )));
        }
        let updated = self
            .conn
            .execute("UPDATE tags SET name=?1 WHERE id=?2", params![name, id])?;
        if updated == 0 {
            return Err(Error::InvalidInput("Tag not found".into()));
        }
        Ok(())
    }

    /// Delete a tag and all its slide assignments (`slide_tags` cascades).
    pub fn delete_tag(&mut self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM tags WHERE id=?1", params![id])?;
        Ok(())
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

    /// Remember a completed export/composition for the stats view. Also records
    /// each picked slide in `export_picks` so `export_counts` can rank decks by
    /// how many slides they've contributed to exports.
    pub fn record_export(
        &mut self,
        output_path: &str,
        title: &str,
        slide_count: i64,
        source_decks: i64,
        picks: &[SlidePick],
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO export_history(output_path, title, slide_count, source_decks, exported_unix) \
             VALUES(?1,?2,?3,?4,?5)",
            params![output_path, title, slide_count, source_decks, now_unix()],
        )?;
        let export_id = tx.last_insert_rowid();
        for pick in picks {
            tx.execute(
                "INSERT INTO export_picks(export_id, deck_path, slide_index) VALUES(?1,?2,?3)",
                params![export_id, pick.pptx_path, pick.slide_index as i64],
            )?;
        }
        // Trim; the FK cascade (foreign_keys=ON) removes trimmed rows' picks too.
        tx.execute(
            "DELETE FROM export_history WHERE id NOT IN \
             (SELECT id FROM export_history ORDER BY id DESC LIMIT 200)",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Total slides ever exported from each deck, keyed by deck path. A deck that
    /// contributed N picks to one export counts N; the "Most exported" browse
    /// sort ranks by this. Empty for existing users until they export again.
    pub fn export_counts(&self) -> Result<HashMap<String, i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT deck_path, COUNT(*) FROM export_picks GROUP BY deck_path")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<HashMap<_, _>>>()?;
        Ok(rows)
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
                "SELECT started_unix, duration_ms, indexed, removed, unchanged, skipped \
                 FROM scan_history ORDER BY id DESC LIMIT 1",
                [],
                |r| {
                    Ok(ScanRecord {
                        started_unix: r.get(0)?,
                        duration_ms: r.get(1)?,
                        indexed: r.get(2)?,
                        removed: r.get(3)?,
                        unchanged: r.get(4)?,
                        skipped: r.get(5)?,
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

        let last_scan_issues = {
            let mut stmt = self.conn.prepare(
                "SELECT path, reason FROM scan_issues \
                 WHERE scan_id = (SELECT MAX(id) FROM scan_history) ORDER BY id LIMIT 200",
            )?;
            let v: Vec<ScanIssue> = stmt
                .query_map([], |r| Ok(ScanIssue { path: r.get(0)?, reason: r.get(1)? }))?
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
            last_scan_issues,
            render_drops: self.render_drops()?,
        })
    }
}

const ROOT_SELECT: &str = "SELECT r.id, r.path, \
    (SELECT COUNT(*) FROM decks d WHERE d.root_id = r.id), \
    (SELECT COUNT(*) FROM slides s JOIN decks d ON d.id = s.deck_id WHERE d.root_id = r.id), \
    r.last_scan_unix, r.exclude_globs FROM roots r";

fn row_to_root(r: &Row) -> rusqlite::Result<RootRecord> {
    let raw_globs: String = r.get(5)?;
    Ok(RootRecord {
        id: r.get(0)?,
        path: r.get(1)?,
        deck_count: r.get(2)?,
        slide_count: r.get(3)?,
        last_scan_unix: r.get(4)?,
        exclude_globs: serde_json::from_str(&raw_globs).unwrap_or_default(),
    })
}

/// Number of columns in [`SLIDE_COLS`] / [`DECK_COLS`] — keep in sync.
const SLIDE_COL_COUNT: usize = 9;
const DECK_COL_COUNT: usize = 12;

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
        first_seen_unix: r.get(base + 10)?,
        favorite: r.get(base + 11)?,
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
        content_hash: r.get(base + 8)?,
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
    if let Some(tag_id) = filters.tag_id {
        // Clause + param pushed together so positional binding stays correct.
        clauses.push(
            "EXISTS(SELECT 1 FROM slide_tags st \
             WHERE st.tag_id = ? AND st.deck_path = d.path AND st.slide_index = s.slide_index)"
                .into(),
        );
        params.push(Value::Integer(tag_id));
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

/// Whether any structured (non-text) filter is active — the trigger for
/// post-filtering semantic hits against an allowed-slide set.
fn filters_active(f: &SearchFilters) -> bool {
    f.deck_query.as_ref().is_some_and(|q| !q.is_empty())
        || f.path_prefix.as_ref().is_some_and(|p| !p.is_empty())
        || f.modified_from.is_some()
        || f.modified_to.is_some()
        || f.favorites_only == Some(true)
        || f.tag_id.is_some()
}

/// Fallback snippet for a hit with no FTS match (semantic-only): the first ~160
/// chars of the body, HTML-escaped like the browse snippets.
fn fallback_snippet(body: &str) -> String {
    html_escape(&body.chars().take(160).collect::<String>())
}

/// Bump to force a one-time reindex of every deck (e.g. when the extraction
/// logic or FTS content changes) — a new version makes every stored hash stale.
const INDEX_VERSION: u32 = 2;

/// Highest schema version this build knows how to migrate to. `PRAGMA
/// user_version` tracks the DB's current level; [`Library::migrate`] applies
/// each `MIGRATIONS_V*` step in order until the DB reaches this. Distinct from
/// INDEX_VERSION: a schema migration is not a re-parse trigger.
const SCHEMA_VERSION: i64 = 4;

/// v1 additive migration: new deck/scan/root columns and the diagnostics tables
/// (scan_issues/render_issues/export_picks). ADD COLUMN NOT NULL all carry a
/// DEFAULT; exclude_globs/dropped are JSON stored as TEXT. Applied exactly once
/// per DB, so these must NEVER also appear in the frozen v0 SCHEMA const (that
/// would make the ALTER fail with 'duplicate column name' on a fresh DB).
const MIGRATIONS_V1: &str = r#"
ALTER TABLE decks ADD COLUMN first_seen_unix INTEGER NOT NULL DEFAULT 0;
UPDATE decks SET first_seen_unix = modified_unix;   -- backfill existing rows
ALTER TABLE scan_history ADD COLUMN skipped INTEGER NOT NULL DEFAULT 0;
ALTER TABLE roots ADD COLUMN exclude_globs TEXT NOT NULL DEFAULT '[]';   -- JSON array
CREATE TABLE IF NOT EXISTS scan_issues(
    id      INTEGER PRIMARY KEY,
    scan_id INTEGER NOT NULL REFERENCES scan_history(id) ON DELETE CASCADE,
    path    TEXT NOT NULL,
    reason  TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_scan_issues_scan ON scan_issues(scan_id);
CREATE TABLE IF NOT EXISTS render_issues(
    deck_path    TEXT NOT NULL,
    slide_index  INTEGER NOT NULL,
    content_hash TEXT NOT NULL,
    dropped      TEXT NOT NULL,   -- JSON array of dropped-construct kinds
    updated_unix INTEGER NOT NULL,
    PRIMARY KEY(deck_path, slide_index)
);
CREATE TABLE IF NOT EXISTS export_picks(
    id          INTEGER PRIMARY KEY,
    export_id   INTEGER NOT NULL REFERENCES export_history(id) ON DELETE CASCADE,
    deck_path   TEXT NOT NULL,
    slide_index INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_export_picks_deck ON export_picks(deck_path);
"#;

/// v2 additive migration: saved searches.
///
/// RULE (learned the hard way mid-wave-2): a `MIGRATIONS_V*` batch is FROZEN
/// the moment any database anywhere may have reached that version — never
/// append to one; add a new version instead. A DB stamped at version N will
/// never re-run batch N, so late additions to it silently never apply.
/// Idempotent DDL (`IF NOT EXISTS`) is still required, so a DB that picked up
/// parts of a later batch early heals instead of erroring.
const MIGRATIONS_V2: &str = r#"
-- WS-A: saved searches
CREATE TABLE IF NOT EXISTS saved_searches(
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL,
    query        TEXT NOT NULL,
    filters_json TEXT NOT NULL DEFAULT '{}',
    position     INTEGER NOT NULL DEFAULT 0,
    created_unix INTEGER NOT NULL
);
"#;

/// v3 additive migration: slide tags. Keyed by (deck_path, slide_index) — the
/// favorites convention — so they survive rescans (which delete + reinsert
/// slide rows) and Clear & Rebuild (which never touches these tables).
/// `tags.name` is UNIQUE and case-insensitive; slide_tags cascades when a tag
/// is deleted.
const MIGRATIONS_V3: &str = r#"
-- WS-E: tags
CREATE TABLE IF NOT EXISTS tags(
    id           INTEGER PRIMARY KEY,
    name         TEXT NOT NULL UNIQUE COLLATE NOCASE,
    created_unix INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS slide_tags(
    tag_id      INTEGER NOT NULL REFERENCES tags(id) ON DELETE CASCADE,
    deck_path   TEXT NOT NULL,
    slide_index INTEGER NOT NULL,
    PRIMARY KEY(tag_id, deck_path, slide_index)
);
CREATE INDEX IF NOT EXISTS idx_slide_tags_slide ON slide_tags(deck_path, slide_index);
"#;

/// v4 additive migration: per-slide content/text hashes + the embeddings store.
/// Its own version step (see the freeze rule on [`MIGRATIONS_V2`]): the
/// `ALTER TABLE ADD COLUMN`s here are NOT idempotent, which is exactly why they
/// must live in a version a stamped DB runs exactly once — a v3-stamped DB
/// reaches this step once and gains the columns; a v4-stamped DB never re-runs
/// it, so the ALTERs can never hit 'duplicate column name'.
const MIGRATIONS_V4: &str = r#"
-- WS-B: embeddings + content hashes
ALTER TABLE slides ADD COLUMN content_hash TEXT;
ALTER TABLE slides ADD COLUMN text_hash TEXT;
CREATE INDEX IF NOT EXISTS idx_slides_content_hash ON slides(content_hash);
CREATE TABLE IF NOT EXISTS embeddings(
    model_id  TEXT NOT NULL,
    text_hash TEXT NOT NULL,
    dims      INTEGER NOT NULL,
    vector    BLOB NOT NULL,
    PRIMARY KEY(model_id, text_hash)
);
"#;

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

/// Compile a root's exclude patterns into two sets: a `raw` set matched against
/// every candidate file's root-relative forward-slash path, and a `prune` set
/// used to skip whole subtrees during the walk. The prune set is built ONLY from
/// the whole-folder form `.../**` (the trailing `/**` stripped), so a bare
/// `*`/`**` or a file-only glob (e.g. `**/*.tmp.pptx`) can never prune a
/// directory — such patterns are still filtered file-by-file via `raw`.
/// Individually invalid patterns are skipped (validation happens up-front in
/// [`Library::set_root_excludes`]); a failed set build falls back to empty.
fn build_glob_sets(patterns: &[String]) -> (GlobSet, GlobSet) {
    let mut raw = GlobSetBuilder::new();
    let mut prune = GlobSetBuilder::new();
    for p in patterns {
        if let Ok(g) = Glob::new(p) {
            raw.add(g);
        }
        if let Some(prefix) = p.strip_suffix("/**") {
            if !prefix.is_empty() {
                if let Ok(g) = Glob::new(prefix) {
                    prune.add(g);
                }
            }
        }
    }
    (
        raw.build().unwrap_or_else(|_| GlobSet::default()),
        prune.build().unwrap_or_else(|_| GlobSet::default()),
    )
}

/// A path's location relative to `root`, as a forward-slash string (globset
/// matches `/`-separated paths regardless of platform). `None` if `path` is not
/// under `root`.
fn rel_forward_slash(root: &Path, path: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    let s = rel.to_string_lossy();
    if std::path::MAIN_SEPARATOR == '/' {
        Some(s.into_owned())
    } else {
        Some(s.replace(std::path::MAIN_SEPARATOR, "/"))
    }
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
    /// Layout-independent content fingerprint (always computed; cheap).
    content_hash: String,
    /// sha256 of the embedder input string, or `None` when the slide has no
    /// indexable text. Keys the (model_id, text_hash) embeddings rows.
    text_hash: Option<String>,
    /// The exact text an embedder is fed (title/body/notes joined). Not persisted
    /// — carried so the inline scan path can embed newly-seen texts without a
    /// second parse. `None` mirrors `text_hash` being `None`.
    embed_text: Option<String>,
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
        let slide_part = pf.slide_part(i)?.to_string();
        let content_hash = crate::hash::slide_content_hash(&pf.package, &slide_part)?;
        let body_text = content.texts.join("\n");
        let embed_text =
            crate::hash::slide_embed_text(content.title.as_deref(), &body_text, content.notes.as_deref());
        let text_hash = embed_text.as_deref().map(crate::hash::text_hash);
        slides.push(ExtractedSlide {
            index: i as i64,
            title: content.title.clone(),
            body_text,
            notes: content.notes.clone(),
            content_hash,
            text_hash,
            embed_text,
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
    use crate::embed::FakeEmbedder;
    use crate::fixtures::{DeckSpec, SlideSpec};
    use std::collections::HashSet;
    use std::fs::OpenOptions;
    use std::sync::Arc;

    fn embedding_rows(lib: &Library) -> i64 {
        lib.conn.query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0)).unwrap()
    }

    /// A small three-slide, two-deck library with a FakeEmbedder attached and a
    /// completed scan (so every slide is embedded).
    fn embedded_library() -> (tempfile::TempDir, Library, Arc<FakeEmbedder>) {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Finance")
            .slide(SlideSpec::new("Revenue").bullets(&["Revenue up 12%", "Churn down"]))
            .slide(SlideSpec::new("Outlook").bullets(&["Zürich office opens"]))
            .write_to(&dir.path().join("finance.pptx"))
            .unwrap();
        DeckSpec::new("Product")
            .slide(SlideSpec::new("Roadmap").bullets(&["Ship search", "Ship compose"]))
            .write_to(&dir.path().join("product.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let fake = Arc::new(FakeEmbedder::new(32));
        lib.set_embedder(Some(fake.clone()));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        (dir, lib, fake)
    }

    /// A library whose slides each carry a single body word and no title, so the
    /// embed text is EXACTLY that one word (the fixture parser duplicates a
    /// title into the body, so a title-only slide would embed "word\nword" —
    /// hence the empty title). With the FakeEmbedder (identical text → cosine
    /// 1.0, distinct text → ≈orthogonal ≈0), a semantic query equal to a word
    /// scores 1.0 for that slide and ~0 for the rest — cleanly straddling
    /// `SEMANTIC_SCORE_FLOOR` so flooring is deterministic to test.
    fn floor_test_library() -> (tempfile::TempDir, Library, Arc<FakeEmbedder>) {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Deck")
            .slide(SlideSpec::new("").bullets(&["alpha"]))
            .slide(SlideSpec::new("").bullets(&["beta"]))
            .slide(SlideSpec::new("").bullets(&["gamma"]))
            .write_to(&dir.path().join("d.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let fake = Arc::new(FakeEmbedder::new(32));
        lib.set_embedder(Some(fake.clone()));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        (dir, lib, fake)
    }

    fn scan_silent(lib: &mut Library) -> ScanEvent {
        let mut finished = ScanEvent::Finished { indexed: 0, removed: 0, unchanged: 0, skipped: 0 };
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
    fn scan_fills_slide_hashes_and_dedupes_identical_content() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("A")
            .slide(SlideSpec::new("Shared").bullets(&["x", "y"]))
            .slide(SlideSpec::new("UniqueA").bullets(&["only a"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        // b.pptx re-authors the SAME "Shared" slide under a different deck.
        DeckSpec::new("B")
            .slide(SlideSpec::new("Shared").bullets(&["x", "y"]))
            .write_to(&dir.path().join("b.pptx"))
            .unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        // Every slide got a content hash + text hash.
        let (n_ch, n_th): (i64, i64) = lib
            .conn
            .query_row(
                "SELECT COUNT(content_hash), COUNT(text_hash) FROM slides",
                [],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .unwrap();
        assert_eq!(n_ch, 3, "all three slides hashed");
        assert_eq!(n_th, 3, "all three slides have text");

        // The identically-authored "Shared" slide clusters across the two decks.
        let dup_groups: i64 = lib
            .conn
            .query_row(
                "SELECT COUNT(*) FROM \
                 (SELECT content_hash FROM slides GROUP BY content_hash HAVING COUNT(*) > 1)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(dup_groups, 1);
        let distinct: i64 = lib
            .conn
            .query_row("SELECT COUNT(DISTINCT content_hash) FROM slides", [], |r| r.get(0))
            .unwrap();
        assert_eq!(distinct, 2, "Shared collides; UniqueA does not");

        // The hash surfaces on the record model.
        let deck = lib.decks().unwrap().into_iter().find(|d| d.file_name == "a.pptx").unwrap();
        let slides = lib.slides_for_deck(deck.id).unwrap();
        assert!(slides.iter().all(|s| s.content_hash.is_some()));
    }

    #[test]
    fn unchanged_text_rescan_keeps_text_hash_stable() {
        // Re-indexing a deck (delete+reinsert of its slide rows) must preserve
        // each slide's text hash, so embeddings keyed by it are never orphaned.
        let (_dir, lib) = two_deck_library();
        let before: Vec<Option<String>> = {
            let mut stmt = lib
                .conn
                .prepare("SELECT text_hash FROM slides ORDER BY id")
                .unwrap();
            stmt.query_map([], |r| r.get(0)).unwrap().collect::<rusqlite::Result<_>>().unwrap()
        };
        assert!(before.iter().all(|h| h.is_some()));
    }

    #[test]
    fn inline_embeds_only_missing_texts() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("A")
            .slide(SlideSpec::new("Alpha").bullets(&["aaa"]))
            .slide(SlideSpec::new("Beta").bullets(&["bbb"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let fake = Arc::new(FakeEmbedder::new(16));
        lib.set_embedder(Some(fake.clone()));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        assert_eq!(fake.passage_count(), 2, "two distinct texts embedded");
        assert_eq!(embedding_rows(&lib), 2);

        // Unchanged rescan: deck skipped by (mtime,size) → no new embed calls/rows.
        scan_silent(&mut lib);
        assert_eq!(fake.passage_count(), 2, "unchanged rescan must not re-embed");
        assert_eq!(embedding_rows(&lib), 2);

        // A new deck reusing the SAME "Alpha/aaa" text embeds nothing new.
        DeckSpec::new("B")
            .slide(SlideSpec::new("Alpha").bullets(&["aaa"]))
            .write_to(&dir.path().join("b.pptx"))
            .unwrap();
        scan_silent(&mut lib);
        assert_eq!(fake.passage_count(), 2, "identical text across decks embeds once");
        assert_eq!(embedding_rows(&lib), 2);
    }

    #[test]
    fn lexical_mode_never_embeds_query() {
        let (_dir, lib, fake) = embedded_library();
        let before = fake.query_count();
        // Default (no search_mode) is lexical.
        lib.search("revenue", &SearchFilters::default()).unwrap();
        // Explicit lexical.
        let lex = SearchFilters { search_mode: Some("lexical".into()), ..Default::default() };
        lib.search("revenue", &lex).unwrap();
        assert_eq!(fake.query_count(), before, "lexical search must never embed the query");
    }

    #[test]
    fn semantic_search_embeds_query_and_floors_unrelated() {
        // The relevance floor: semantic search embeds the query, returns the
        // exact-text match (cosine 1.0), and DROPS the rest instead of ranking
        // the whole embedded corpus.
        let (_dir, lib, fake) = floor_test_library();
        let before = fake.query_count();
        let f = SearchFilters { search_mode: Some("semantic".into()), ..Default::default() };
        let hits = lib.search("alpha", &f).unwrap();
        assert!(fake.query_count() > before, "semantic search embeds the query");
        assert_eq!(hits.len(), 1, "only the above-floor (exact-text) slide survives");
        assert_eq!(hits[0].slide.body_text, "alpha");
        assert!(hits[0].score >= SEMANTIC_SCORE_FLOOR as f64, "kept hit clears the floor");
        assert!(!hits[0].snippet.contains("<mark>"), "semantic hits get a fallback snippet");
        // A query matching NO slide's text returns nothing (all below floor),
        // not the entire embedded corpus.
        assert!(lib.search("nonsense", &f).unwrap().is_empty());
    }

    #[test]
    fn hybrid_returns_lexical_hits_when_vector_arm_floored() {
        // Frontend sanity: hybrid must still surface exact FTS matches even when
        // the vector arm floors to empty. "revenue" matches the Revenue slide in
        // FTS, but equals no slide's exact embed text, so every cosine is well
        // below the floor and the vector arm contributes nothing. One-sided RRF
        // (rrf_fuse handles a single non-empty list) must still return the
        // lexical hit with its `<mark>` snippet, and no sub-floor semantic-only
        // hit may leak in.
        let (_dir, lib, _fake) = embedded_library();
        let lex = lib.search("revenue", &SearchFilters::default()).unwrap();
        assert_eq!(lex.len(), 1);

        let hy = SearchFilters { search_mode: Some("hybrid".into()), ..Default::default() };
        let hits = lib.search("revenue", &hy).unwrap();
        assert_eq!(hits.len(), lex.len(), "vector arm floored to empty → FTS hits only");
        assert!(hits.iter().all(|h| h.snippet.contains("<mark>")), "only FTS-marked hits remain");
        assert_eq!(hits[0].slide.id, lex[0].slide.id, "the lexical hit survives fusion");
    }

    #[test]
    fn hybrid_and_semantic_respect_parsed_date_bounds() {
        // Two decks, BOTH with a slide whose exact embed text is "revenue" (one
        // body word, no title), so the FakeEmbedder scores the "revenue" query
        // at cosine 1.0 — above the floor — for both. One deck is made old. A
        // parsed `after:` bound must exclude the old deck's slide from the vector
        // arm too (eff post-filtering), not just from FTS.
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Old")
            .slide(SlideSpec::new("").bullets(&["revenue"]))
            .write_to(&dir.path().join("old.pptx"))
            .unwrap();
        DeckSpec::new("New")
            .slide(SlideSpec::new("").bullets(&["revenue"]))
            .write_to(&dir.path().join("new.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.set_embedder(Some(Arc::new(FakeEmbedder::new(32))));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        // Backdate old.pptx to 2010 (fixtures share the real file mtime).
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix = 1262304000 WHERE file_name = 'old.pptx'",
                [],
            )
            .unwrap();

        for mode in ["hybrid", "semantic"] {
            let f = SearchFilters { search_mode: Some(mode.into()), ..Default::default() };
            // Control: without the bound, both decks' slides surface.
            let all = lib.search("revenue", &f).unwrap();
            assert!(
                all.iter().any(|h| h.deck.file_name == "old.pptx"),
                "{mode}: old deck present without a date bound"
            );
            // With `after:` parsed out of the query, the old deck must vanish
            // from BOTH retrieval arms.
            let bounded = lib.search("revenue after:2015-01-01", &f).unwrap();
            assert!(
                bounded.iter().all(|h| h.deck.file_name != "old.pptx"),
                "{mode}: parsed after: must exclude the old deck's hits"
            );
            assert!(
                bounded.iter().any(|h| h.deck.file_name == "new.pptx"),
                "{mode}: the in-range deck still matches"
            );
        }
    }

    #[test]
    fn semantic_falls_back_to_lexical_without_model() {
        // No embedder attached: semantic/hybrid silently behave lexically.
        let (_dir, lib) = two_deck_library();
        let f = SearchFilters { search_mode: Some("semantic".into()), ..Default::default() };
        let hits = lib.search("revenue", &f).unwrap();
        assert!(!hits.is_empty());
        assert!(hits[0].snippet.contains("<mark>"), "degraded to FTS, not empty/error");
    }

    #[test]
    fn get_similar_slides_floors_unrelated_and_excludes_twins() {
        // Find-similar drops both padding sources of noise:
        //  - the cross-deck twin scores cosine 1.0 (ABOVE the floor) but is
        //    excluded by the same-text-twin rule — proving the exclusion runs;
        //  - the non-twin "Other" scores ≈0 (BELOW the floor) and is dropped by
        //    the floor — without it, this unrelated slide would pad the list.
        // With nothing genuinely related left, the result is empty.
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("A")
            .slide(SlideSpec::new("Shared").bullets(&["same text"]))
            .slide(SlideSpec::new("Other").bullets(&["different alpha content"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        DeckSpec::new("B")
            .slide(SlideSpec::new("Shared").bullets(&["same text"])) // twin of A/Shared
            .write_to(&dir.path().join("b.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let fake = Arc::new(FakeEmbedder::new(32));
        lib.set_embedder(Some(fake.clone()));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let a = lib.decks().unwrap().into_iter().find(|d| d.file_name == "a.pptx").unwrap();
        let anchor = lib
            .slides_for_deck(a.id)
            .unwrap()
            .into_iter()
            .find(|s| s.title.as_deref() == Some("Shared"))
            .unwrap();

        let sim = lib.get_similar_slides(anchor.id, 10).unwrap();
        assert!(
            sim.is_empty(),
            "above-floor twin excluded by rule; below-floor non-twin dropped by the floor"
        );
    }

    #[test]
    fn get_similar_slides_empty_without_model() {
        let (_dir, lib) = two_deck_library(); // no embedder
        let any = lib.decks().unwrap()[0].id;
        let s = lib.slides_for_deck(any).unwrap()[0].id;
        assert!(lib.get_similar_slides(s, 10).unwrap().is_empty());
    }

    #[test]
    fn clear_invalidates_vector_cache() {
        // Regression: Clear & Rebuild recycles slide rowids, so a vector store
        // primed before the clear would resolve stale ids afterwards. clear()
        // must drop the in-memory caches so the next semantic lookup reloads from
        // the (now empty) index.
        let dir = tempfile::tempdir().unwrap();
        // One body word, no title: the exact embed text is "revenue", so a
        // semantic query for "revenue" scores cosine 1.0 (above the floor) and
        // primes / exercises the vector store.
        DeckSpec::new("A")
            .slide(SlideSpec::new("").bullets(&["revenue"]))
            .slide(SlideSpec::new("").bullets(&["outlook"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.set_embedder(Some(Arc::new(FakeEmbedder::new(32))));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let f = SearchFilters { search_mode: Some("semantic".into()), ..Default::default() };
        // Prime the lazily-loaded vector store; the exact-text query clears the floor.
        assert!(
            !lib.search("revenue", &f).unwrap().is_empty(),
            "precondition: the primed store resolves the query"
        );

        lib.clear().unwrap();

        // A stale (non-invalidated) store would still hold the slide's row and
        // resolve "revenue"; a correctly invalidated one reloads to an empty store.
        assert!(
            lib.search("revenue", &f).unwrap().is_empty(),
            "cleared library must not serve vectors for recycled/removed slide ids"
        );
    }

    #[test]
    fn exact_duplicate_groups_detected() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("A")
            .slide(SlideSpec::new("Shared").bullets(&["dup body"]))
            .slide(SlideSpec::new("UniqueA").bullets(&["only here"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        DeckSpec::new("B")
            .slide(SlideSpec::new("Shared").bullets(&["dup body"]))
            .write_to(&dir.path().join("b.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap(); // no model → exact only
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let groups = lib.list_duplicate_groups().unwrap();
        assert!(groups.iter().all(|g| g.kind == "exact"), "no near groups without a model");
        let exact: Vec<_> = groups.iter().filter(|g| g.kind == "exact").collect();
        assert_eq!(exact.len(), 1);
        assert_eq!(exact[0].slides.len(), 2);
        assert!(exact[0].score.is_none());
        let decks: HashSet<&str> =
            exact[0].slides.iter().map(|s| s.deck.file_name.as_str()).collect();
        assert!(decks.contains("a.pptx") && decks.contains("b.pptx"));
    }

    #[test]
    fn near_duplicate_groups_detected_with_model() {
        // Identical visible text, different XML (one has an extra text-less shape):
        // content hashes differ (not exact), embeddings match (near).
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("A")
            .slide(SlideSpec::new("Deck").bullets(&["identical body"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        DeckSpec::new("B")
            .slide(
                SlideSpec::new("Deck").bullets(&["identical body"]).raw_shape(
                    r#"<p:sp><p:nvSpPr><p:cNvPr id="99" name="deco"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:p/></p:txBody></p:sp>"#,
                ),
            )
            .write_to(&dir.path().join("b.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let fake = Arc::new(FakeEmbedder::new(32));
        lib.set_embedder(Some(fake.clone()));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let groups = lib.list_duplicate_groups().unwrap();
        assert!(groups.iter().all(|g| g.kind != "exact"), "differing XML → not exact");
        let near: Vec<_> = groups.iter().filter(|g| g.kind == "near").collect();
        assert_eq!(near.len(), 1, "the two same-text slides form one near group");
        assert_eq!(near[0].slides.len(), 2);
        assert!(near[0].score.unwrap() >= NEAR_DUP_THRESHOLD);
    }

    #[test]
    fn backfill_fills_hashes_and_embeds_missing() {
        // Emulate a library indexed before hashing: NULL content/text hashes.
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Old")
            .slide(SlideSpec::new("One").bullets(&["alpha"]))
            .slide(SlideSpec::new("Two").bullets(&["beta"]))
            .write_to(&dir.path().join("old.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        lib.conn.execute("UPDATE slides SET content_hash=NULL, text_hash=NULL", []).unwrap();

        // Attach a model and run the primitives the desktop backfill loop uses.
        let fake = Arc::new(FakeEmbedder::new(16));
        lib.set_embedder(Some(fake.clone()));
        let decks = lib.decks_needing_hash_backfill().unwrap();
        assert_eq!(decks.len(), 1);
        for (id, path) in decks {
            lib.backfill_deck_hashes(id, &path).unwrap();
        }
        let nulls: i64 = lib
            .conn
            .query_row("SELECT COUNT(*) FROM slides WHERE content_hash IS NULL", [], |r| r.get(0))
            .unwrap();
        assert_eq!(nulls, 0, "hashes backfilled");

        let pending = lib.pending_embedding_texts().unwrap();
        assert_eq!(pending.len(), 2);
        for chunk in pending.chunks(1) {
            lib.embed_and_store(chunk).unwrap();
        }
        assert_eq!(embedding_rows(&lib), 2);
        assert_eq!(fake.passage_count(), 2);
        assert_eq!(lib.embedding_counts().unwrap(), (2, 2));
    }

    #[test]
    fn store_embedding_vectors_is_idempotent() {
        // Split-phase backfill: embed off-lock, then store under a short re-lock.
        // Storing the same chunk twice must be a no-op (INSERT OR IGNORE), never a
        // duplicate row — covers a redundant embed racing an inline scan.
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("A")
            .slide(SlideSpec::new("Alpha").bullets(&["aaa"]))
            .slide(SlideSpec::new("Beta").bullets(&["bbb"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib); // no embedder yet → nothing embedded inline

        let fake = Arc::new(FakeEmbedder::new(16));
        lib.set_embedder(Some(fake.clone()));
        let embedder = lib.embedder_handle().expect("embedder attached");
        let model_id = embedder.id().to_string();
        let dims = embedder.dims();

        let pending = lib.pending_embedding_texts().unwrap();
        assert_eq!(pending.len(), 2);
        let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();

        let n = lib
            .store_embedding_vectors(&model_id, dims, &pending, embedder.embed_passages(&texts).unwrap())
            .unwrap();
        assert_eq!(n, 2);
        assert_eq!(embedding_rows(&lib), 2);

        // Re-store the identical snapshot → both rows already present, ignored.
        lib.store_embedding_vectors(&model_id, dims, &pending, embedder.embed_passages(&texts).unwrap())
            .unwrap();
        assert_eq!(embedding_rows(&lib), 2, "idempotent: no duplicate rows");
    }

    #[test]
    fn snapshot_store_then_delete_leaves_no_orphan() {
        // Models the restructured backfill's Phase B: snapshot pending under a
        // short lock, embed off-lock, but a scan deletes a slide before we store.
        // The stored vector for the vanished text must be reaped by the tail's
        // cleanup_orphan_embeddings — no orphan survives.
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("A")
            .slide(SlideSpec::new("Keep").bullets(&["keep me around"]))
            .write_to(&dir.path().join("a.pptx"))
            .unwrap();
        DeckSpec::new("B")
            .slide(SlideSpec::new("Gone").bullets(&["delete me soon please"]))
            .write_to(&dir.path().join("b.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);

        let fake = Arc::new(FakeEmbedder::new(16));
        lib.set_embedder(Some(fake.clone()));
        let embedder = lib.embedder_handle().unwrap();
        let model_id = embedder.id().to_string();
        let dims = embedder.dims();

        // Snapshot BOTH pending texts, then embed off-lock (as the backfill does).
        let pending = lib.pending_embedding_texts().unwrap();
        assert_eq!(pending.len(), 2);
        let texts: Vec<String> = pending.iter().map(|(_, t)| t.clone()).collect();
        let vectors = embedder.embed_passages(&texts).unwrap();

        // Interleaved scan removes b.pptx's slide between snapshot and store.
        std::fs::remove_file(dir.path().join("b.pptx")).unwrap();
        scan_silent(&mut lib);

        // Store the pre-delete snapshot: writes a vector for the now-gone text too.
        lib.store_embedding_vectors(&model_id, dims, &pending, vectors).unwrap();
        assert_eq!(embedding_rows(&lib), 2, "both stored, incl. the vanished text");

        // The tail reaps the embedding whose text no longer backs any slide.
        let removed = lib.cleanup_orphan_embeddings().unwrap();
        assert_eq!(removed, 1);
        assert_eq!(embedding_rows(&lib), 1, "only the surviving slide's vector remains");
    }

    #[test]
    fn orphan_embeddings_cleaned_after_text_change() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("D")
            .slide(SlideSpec::new("S").bullets(&["alpha one"]))
            .write_to(&dir.path().join("d.pptx"))
            .unwrap();
        let mut lib = Library::open_in_memory().unwrap();
        let fake = Arc::new(FakeEmbedder::new(16));
        lib.set_embedder(Some(fake.clone()));
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        assert_eq!(embedding_rows(&lib), 1);

        // Rewrite the slide text (different length → new deck hash forces reindex).
        DeckSpec::new("D")
            .slide(SlideSpec::new("S").bullets(&["beta two three four five"]))
            .write_to(&dir.path().join("d.pptx"))
            .unwrap();
        scan_silent(&mut lib);
        assert_eq!(embedding_rows(&lib), 1, "old text's embedding cleaned; new one added");
    }

    #[test]
    fn scan_issues_persisted_and_retrievable() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Good")
            .slide(SlideSpec::new("Title").bullets(&["ok"]))
            .write_to(&dir.path().join("good.pptx"))
            .unwrap();
        std::fs::write(dir.path().join("broken.pptx"), b"not a zip").unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        let finished = scan_silent(&mut lib);
        assert!(matches!(finished, ScanEvent::Finished { indexed: 1, skipped: 1, .. }));

        let o = lib.stats_overview().unwrap();
        assert_eq!(o.last_scan.as_ref().unwrap().skipped, 1);
        assert_eq!(o.last_scan_issues.len(), 1);
        assert!(o.last_scan_issues[0].path.ends_with("broken.pptx"));
        assert!(!o.last_scan_issues[0].reason.is_empty());
    }

    #[test]
    fn scan_history_trim_cascades_issues() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("broken.pptx"), b"not a zip").unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        // Each rescan re-fails parse on broken.pptx (never indexed), so every run
        // records exactly one scan_issues row.
        for _ in 0..55 {
            scan_silent(&mut lib);
        }

        let history: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM scan_history", [], |r| r.get(0)).unwrap();
        assert_eq!(history, 50, "scan_history bounded to 50 rows");

        let orphans: i64 = lib
            .conn
            .query_row(
                "SELECT COUNT(*) FROM scan_issues \
                 WHERE scan_id NOT IN (SELECT id FROM scan_history)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "trim cascade left no orphan issues");

        let issues: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM scan_issues", [], |r| r.get(0)).unwrap();
        assert_eq!(issues, 50, "one surviving issue per surviving scan");
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
        assert!(matches!(first, ScanEvent::Finished { indexed: 2, removed: 0, unchanged: 0, skipped: 0 }));

        // No changes: nothing reindexed.
        let second = scan_silent(&mut lib);
        assert!(matches!(second, ScanEvent::Finished { indexed: 0, removed: 0, unchanged: 2, skipped: 0 }));

        // Rewrite one file with different content (and a newer mtime).
        DeckSpec::new("A2")
            .slide(SlideSpec::new("One").bullets(&["gamma"]))
            .write_to(&a)
            .unwrap();
        let future = UNIX_EPOCH + Duration::from_secs(now_unix() as u64 + 100);
        OpenOptions::new().write(true).open(&a).unwrap().set_modified(future).unwrap();

        let third = scan_silent(&mut lib);
        assert!(matches!(third, ScanEvent::Finished { indexed: 1, removed: 0, unchanged: 1, skipped: 0 }));
        assert!(lib.search("gamma", &SearchFilters::default()).unwrap().len() == 1);
        assert!(lib.search("alpha", &SearchFilters::default()).unwrap().is_empty());

        // Delete a file: its deck + slides + FTS rows vanish.
        std::fs::remove_file(&b).unwrap();
        let fourth = scan_silent(&mut lib);
        assert!(matches!(fourth, ScanEvent::Finished { indexed: 0, removed: 1, unchanged: 1, skipped: 0 }));
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
    fn clear_wipes_content_keeps_roots_and_favorites() {
        let (_dir, mut lib) = two_deck_library();

        // Star a slide and a deck (both keyed by path/index, not row id).
        let hits = lib.search("revenue", &SearchFilters::default()).unwrap();
        let slide_id = hits[0].slide.id;
        assert!(lib.toggle_slide_favorite(slide_id).unwrap());
        let deck_id = lib.decks().unwrap()[0].id;
        assert!(lib.toggle_deck_favorite(deck_id).unwrap());

        // Populate activity history.
        lib.record_search("revenue", 1).unwrap();
        lib.record_export(
            "/tmp/out.pptx",
            "My Deck",
            4,
            2,
            &[
                SlidePick { pptx_path: "/tmp/a.pptx".into(), slide_index: 1 },
                SlidePick { pptx_path: "/tmp/b.pptx".into(), slide_index: 2 },
            ],
        )
        .unwrap();

        // Seed a scan_issues row (cascades off scan_history) and a render_issues
        // row (deleted explicitly) so clear() must remove both.
        let scan_id: i64 = lib
            .conn
            .query_row("SELECT id FROM scan_history ORDER BY id DESC LIMIT 1", [], |r| r.get(0))
            .unwrap();
        lib.conn
            .execute(
                "INSERT INTO scan_issues(scan_id, path, reason) VALUES(?1,?2,?3)",
                params![scan_id, "/decks/broken.pptx", "corrupt zip"],
            )
            .unwrap();
        lib.conn
            .execute(
                "INSERT INTO render_issues(deck_path, slide_index, content_hash, dropped, updated_unix) \
                 VALUES(?1,?2,?3,?4,?5)",
                params!["/decks/root/a.pptx", 1i64, "hash", "[\"chart\"]", 111i64],
            )
            .unwrap();

        lib.clear().unwrap();

        // All indexed content is gone.
        assert_eq!(lib.stats().unwrap(), (0, 0));
        assert!(lib.decks().unwrap().is_empty());
        assert!(lib.search("revenue", &SearchFilters::default()).unwrap().is_empty());

        // Roots kept, but reset to unscanned + zero counts.
        let roots = lib.roots().unwrap();
        assert_eq!(roots.len(), 1);
        assert_eq!(roots[0].last_scan_unix, None);
        assert_eq!(roots[0].deck_count, 0);
        assert_eq!(roots[0].slide_count, 0);

        // Favorites kept.
        let sf: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM slide_favorites", [], |r| r.get(0)).unwrap();
        assert_eq!(sf, 1);
        let df: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM deck_favorites", [], |r| r.get(0)).unwrap();
        assert_eq!(df, 1);

        // History cleared.
        let o = lib.stats_overview().unwrap();
        assert!(o.recent_searches.is_empty());
        assert!(o.recent_exports.is_empty());
        assert!(o.last_scan.is_none());

        // Diagnostics gone: scan_issues via cascade, render_issues via explicit delete.
        let si: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM scan_issues", [], |r| r.get(0)).unwrap();
        assert_eq!(si, 0);
        let ri: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM render_issues", [], |r| r.get(0)).unwrap();
        assert_eq!(ri, 0);
    }

    #[test]
    fn clear_then_rescan_reindexes_and_favorites_relink() {
        let (dir, mut lib) = two_deck_library();

        // Star the revenue slide before wiping.
        let hits = lib.search("revenue", &SearchFilters::default()).unwrap();
        let starred_title = hits[0].slide.title.clone();
        assert!(lib.toggle_slide_favorite(hits[0].slide.id).unwrap());

        lib.clear().unwrap();
        assert_eq!(lib.stats().unwrap(), (0, 0));

        // Files are still on disk in the tempdir, so a rescan reindexes them.
        let finished = scan_silent(&mut lib);
        assert!(matches!(
            finished,
            ScanEvent::Finished { indexed: 2, removed: 0, unchanged: 0, skipped: 0 }
        ));
        assert_eq!(lib.stats().unwrap(), (2, 3));

        // The path/index-keyed favorite re-attached to the freshly indexed rows.
        let filters = SearchFilters { favorites_only: Some(true), ..Default::default() };
        let favs = lib.search("", &filters).unwrap();
        assert_eq!(favs.len(), 1);
        assert_eq!(favs[0].slide.title, starred_title);
        let _ = dir;
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

    /// Two decks where the token `scopeword` sits in slide A's TITLE and slide
    /// B's BODY, with per-slide unique body tokens `onlyone`/`onlytwo` — enough
    /// to exercise column scoping, OR/NOT, dates, and favorites.
    fn field_scoped_library() -> (tempfile::TempDir, Library) {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Finance")
            .slide(
                SlideSpec::new("scopeword alpha")
                    .bullets(&["onlyone detail"])
                    .notes("private memo"),
            )
            .write_to(&dir.path().join("financedeck.pptx"))
            .unwrap();
        DeckSpec::new("Roadmap")
            .slide(SlideSpec::new("beta results").bullets(&["scopeword onlytwo"]))
            .write_to(&dir.path().join("roadmapdeck.pptx"))
            .unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        (dir, lib)
    }

    #[test]
    fn advanced_field_scoping() {
        let (_dir, lib) = field_scoped_library();
        // `scopeword` is in deck A's slide TITLE and deck B's slide BODY. A
        // title-scoped term must exclude the body-only hit (deck B).
        let title_hits = lib.search("title:scopeword", &SearchFilters::default()).unwrap();
        assert_eq!(title_hits.len(), 1);
        assert_eq!(title_hits[0].deck.file_name, "financedeck.pptx");

        // `onlytwo` lives only in deck B's bullets: found via body:, never title:.
        let body_hits = lib.search("body:onlytwo", &SearchFilters::default()).unwrap();
        assert_eq!(body_hits.len(), 1);
        assert_eq!(body_hits[0].deck.file_name, "roadmapdeck.pptx");
        assert!(lib.search("title:onlytwo", &SearchFilters::default()).unwrap().is_empty());

        // A bare term ignores columns → both slides.
        let both = lib.search("scopeword", &SearchFilters::default()).unwrap();
        assert_eq!(both.len(), 2);
    }

    #[test]
    fn advanced_deck_scoping() {
        let (_dir, lib) = field_scoped_library();
        let a = lib.search("deck:financedeck", &SearchFilters::default()).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].deck.file_name, "financedeck.pptx");

        let b = lib.search("deck:roadmapdeck", &SearchFilters::default()).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].deck.file_name, "roadmapdeck.pptx");
    }

    #[test]
    fn advanced_or_widens_not_narrows() {
        let (_dir, lib) = field_scoped_library();
        // OR unions the two unique body tokens.
        let or_hits = lib.search("onlyone OR onlytwo", &SearchFilters::default()).unwrap();
        assert_eq!(or_hits.len(), 2);

        // scopeword matches both; NOT onlytwo drops deck B.
        let not_hits = lib.search("scopeword NOT onlytwo", &SearchFilters::default()).unwrap();
        assert_eq!(not_hits.len(), 1);
        assert_eq!(not_hits[0].deck.file_name, "financedeck.pptx");

        // `-term` is equivalent NOT sugar.
        let dash_hits = lib.search("scopeword -onlytwo", &SearchFilters::default()).unwrap();
        assert_eq!(dash_hits.len(), 1);
        assert_eq!(dash_hits[0].deck.file_name, "financedeck.pptx");
    }

    #[test]
    fn advanced_date_bounds_filter_by_mtime() {
        let (_dir, lib) = field_scoped_library();
        // 2020-01-01T00:00:00Z = 1_577_836_800. Deck A predates it; deck B follows.
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1500000000 WHERE file_name='financedeck.pptx'",
                [],
            )
            .unwrap();
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1900000000 WHERE file_name='roadmapdeck.pptx'",
                [],
            )
            .unwrap();

        let after = lib.search("scopeword after:2020-01-01", &SearchFilters::default()).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].deck.file_name, "roadmapdeck.pptx");

        let before = lib.search("scopeword before:2020-01-01", &SearchFilters::default()).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].deck.file_name, "financedeck.pptx");
    }

    #[test]
    fn advanced_query_date_combines_restrictively_with_filter() {
        let (_dir, lib) = field_scoped_library();
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1500000000 WHERE file_name='financedeck.pptx'",
                [],
            )
            .unwrap();
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1900000000 WHERE file_name='roadmapdeck.pptx'",
                [],
            )
            .unwrap();

        // A popover `modified_from` far in the future would exclude everything;
        // the query's `before:` can only narrow, never widen, so still empty.
        let filters = SearchFilters { modified_from: Some(2_000_000_000), ..Default::default() };
        let hits = lib.search("scopeword before:2020-01-01", &filters).unwrap();
        assert!(hits.is_empty(), "query before: must not loosen the popover's from-bound");
    }

    #[test]
    fn advanced_date_only_query_browses_with_date_filter() {
        let (_dir, lib) = field_scoped_library();
        // 2020-01-01T00:00:00Z = 1_577_836_800. Deck A predates it; deck B follows.
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1500000000 WHERE file_name='financedeck.pptx'",
                [],
            )
            .unwrap();
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1900000000 WHERE file_name='roadmapdeck.pptx'",
                [],
            )
            .unwrap();

        // A date-only query has no text term: it must BROWSE with the date
        // filter applied — not text-search the date's digits and return nothing.
        let after = lib.search("after:2020-01-01", &SearchFilters::default()).unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].deck.file_name, "roadmapdeck.pptx");

        let before = lib.search("before:2020-01-01", &SearchFilters::default()).unwrap();
        assert_eq!(before.len(), 1);
        assert_eq!(before[0].deck.file_name, "financedeck.pptx");
    }

    #[test]
    fn advanced_date_only_query_combines_with_favorites_filter() {
        let (_dir, mut lib) = field_scoped_library();
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1500000000 WHERE file_name='financedeck.pptx'",
                [],
            )
            .unwrap();
        lib.conn
            .execute(
                "UPDATE decks SET modified_unix=1900000000 WHERE file_name='roadmapdeck.pptx'",
                [],
            )
            .unwrap();
        // Star only deck A's slide (deck A predates the bound).
        let s1 = lib.search("onlyone", &SearchFilters::default()).unwrap();
        assert_eq!(s1.len(), 1);
        assert!(lib.toggle_slide_favorite(s1[0].slide.id).unwrap());

        let favs = SearchFilters { favorites_only: Some(true), ..Default::default() };
        // Date bound keeps deck A, favorites keeps the starred slide → 1 hit.
        let hits = lib.search("before:2020-01-01", &favs).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].deck.file_name, "financedeck.pptx");
        // The other side of the bound excludes the starred slide → empty.
        assert!(lib.search("after:2020-01-01", &favs).unwrap().is_empty());
    }

    #[test]
    fn advanced_purely_negative_query_browses() {
        let (_dir, lib) = field_scoped_library();
        // A standalone negation is inexpressible in FTS5; v1 browses instead of
        // text-searching the excluded word (which would wrongly return its hit).
        for q in ["-onlytwo", "NOT onlytwo"] {
            let hits = lib.search(q, &SearchFilters::default()).unwrap();
            assert_eq!(hits.len(), 2, "purely-negative {q:?} should browse all slides");
        }
    }

    #[test]
    fn advanced_query_combines_with_favorites_filter() {
        let (_dir, mut lib) = field_scoped_library();
        // Star only deck A's slide (the one carrying `onlyone`).
        let s1 = lib.search("onlyone", &SearchFilters::default()).unwrap();
        assert_eq!(s1.len(), 1);
        assert!(lib.toggle_slide_favorite(s1[0].slide.id).unwrap());

        let filters = SearchFilters { favorites_only: Some(true), ..Default::default() };
        // scopeword matches both slides; favorites_only keeps just the starred one.
        let hits = lib.search("scopeword", &filters).unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].deck.file_name, "financedeck.pptx");
    }

    #[test]
    fn advanced_parser_output_executes_against_fts() {
        let (_dir, lib) = two_deck_library();
        let mut hostile: Vec<String> = [
            "revenue OR churn",
            "\"unterminated",
            "title:\"unterminated",
            "NEAR(a b)",
            "(revenue AND",
            "* * *",
            "revenue) NOT churn",
            "-churn",
            "NOT churn",
            "a:b:c:d",
            "title: body: deck:",
            "%%% ;;; ()",
            "😀 revenue 🎉",
            "before:2020-13-45 after:not-a-date",
            "revenue -churn title:foo deck:\"multi word\" OR notes:bar",
            "OR OR OR NOT NOT",
        ]
        .into_iter()
        .map(String::from)
        .collect();
        hostile.push("z ".repeat(3000));
        hostile.push("\"".repeat(50));

        for q in &hostile {
            // 1) The full search path never errors on any input.
            lib.search(q, &SearchFilters::default())
                .unwrap_or_else(|e| panic!("search errored on {q:?}: {e}"));
            // 2) Any generated MATCH string is valid FTS5 (executes cleanly).
            if let Some(m) = super::query::parse_query(q).match_expr {
                let mut stmt = lib
                    .conn
                    .prepare("SELECT rowid FROM slides_fts WHERE slides_fts MATCH ?1")
                    .unwrap();
                let res: rusqlite::Result<Vec<i64>> =
                    stmt.query_map([&m], |r| r.get(0)).and_then(|rows| rows.collect());
                res.unwrap_or_else(|e| panic!("MATCH {m:?} (from {q:?}) failed: {e}"));
            }
        }
    }

    #[test]
    fn thumb_keys_survive_rowid_reuse() {
        use crate::thumbs::{thumb_file_name, ThumbTier};

        fn capture(lib: &Library) -> Vec<(i64, String)> {
            let mut out = Vec::new();
            for deck in lib.decks().unwrap() {
                for slide in lib.slides_for_deck(deck.id).unwrap() {
                    let (path, chash, idx) = lib.slide_render_info(slide.id).unwrap();
                    let key = thumb_file_name(&path, &chash, idx as usize, ThumbTier::Thumb);
                    out.push((slide.id, key));
                }
            }
            out
        }

        let dir_a = tempfile::tempdir().unwrap();
        DeckSpec::new("Alpha")
            .slide(SlideSpec::new("A1").bullets(&["one"]))
            .slide(SlideSpec::new("A2").bullets(&["two"]))
            .write_to(&dir_a.path().join("alpha.pptx"))
            .unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir_a.path()).unwrap();
        scan_silent(&mut lib);
        let a = capture(&lib);
        assert!(!a.is_empty());

        // Remove folder A: its slide rowids are now free for reuse.
        let root_a = lib.roots().unwrap()[0].id;
        lib.remove_root(root_a).unwrap();

        // Add a *different* folder B; its slides reclaim the freed rowids.
        let dir_b = tempfile::tempdir().unwrap();
        DeckSpec::new("Beta")
            .slide(SlideSpec::new("B1").bullets(&["three"]))
            .slide(SlideSpec::new("B2").bullets(&["four"]))
            .write_to(&dir_b.path().join("beta.pptx"))
            .unwrap();
        lib.add_root(dir_b.path()).unwrap();
        scan_silent(&mut lib);
        let b = capture(&lib);

        // Precondition of the original bug: a rowid really is reused here.
        let a_ids: HashSet<i64> = a.iter().map(|(id, _)| *id).collect();
        assert!(
            b.iter().any(|(id, _)| a_ids.contains(id)),
            "expected rowid reuse across remove+add"
        );

        // The fix: content-addressed keys never collide across decks, so folder
        // B's slides can't be served folder A's cached SVGs.
        let a_keys: HashSet<&str> = a.iter().map(|(_, k)| k.as_str()).collect();
        for (_, key) in &b {
            assert!(!a_keys.contains(key.as_str()), "thumb key {key} collides across decks");
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

        lib.record_export(
            "/tmp/out.pptx",
            "My Deck",
            4,
            2,
            &[
                SlidePick { pptx_path: "/tmp/a.pptx".into(), slide_index: 1 },
                SlidePick { pptx_path: "/tmp/b.pptx".into(), slide_index: 2 },
            ],
        )
        .unwrap();

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
    fn export_records_picks_and_counts_aggregate() {
        let mut lib = Library::open_in_memory().unwrap();
        let picks = vec![
            SlidePick { pptx_path: "/decks/a.pptx".into(), slide_index: 1 },
            SlidePick { pptx_path: "/decks/a.pptx".into(), slide_index: 3 },
            SlidePick { pptx_path: "/decks/b.pptx".into(), slide_index: 2 },
        ];
        lib.record_export("/out/deck1.pptx", "Deck 1", 3, 2, &picks).unwrap();
        lib.record_export(
            "/out/deck2.pptx",
            "Deck 2",
            1,
            1,
            &[SlidePick { pptx_path: "/decks/a.pptx".into(), slide_index: 1 }],
        )
        .unwrap();
        let counts = lib.export_counts().unwrap();
        assert_eq!(counts.get("/decks/a.pptx").copied(), Some(3));
        assert_eq!(counts.get("/decks/b.pptx").copied(), Some(1));
        assert_eq!(counts.get("/decks/missing.pptx"), None);
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

    #[test]
    fn migration_v0_to_latest_preserves_data() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lib.db");

        // Build a genuine v0 database: apply only the frozen baseline schema, so
        // PRAGMA user_version stays 0 and none of the v1 columns/tables exist.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            let v0: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v0, 0);

            conn.execute("INSERT INTO roots(path) VALUES(?1)", params!["/decks/root"])
                .unwrap();
            let root_id = conn.last_insert_rowid();
            let modified: i64 = 1_700_000_000;
            conn.execute(
                "INSERT INTO decks(root_id, path, file_name, title, author, slide_count, \
                 modified_unix, size_bytes, slide_width_emu, slide_height_emu, content_hash) \
                 VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
                params![
                    root_id,
                    "/decks/root/a.pptx",
                    "a.pptx",
                    "A",
                    Option::<String>::None,
                    1i64,
                    modified,
                    1000i64,
                    12_192_000i64,
                    6_858_000i64,
                    "hash",
                ],
            )
            .unwrap();
            let deck_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO slides(deck_id, slide_index, title, body_text, notes, thumb_path) \
                 VALUES(?1,?2,?3,?4,NULL,NULL)",
                params![deck_id, 1i64, "One", "body"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO slide_favorites(deck_path, slide_index, added_unix) VALUES(?1,?2,?3)",
                params!["/decks/root/a.pptx", 1i64, 111i64],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO deck_favorites(deck_path, added_unix) VALUES(?1,?2)",
                params!["/decks/root/a.pptx", 111i64],
            )
            .unwrap();
        }

        // Opening through Library runs the migration.
        let lib = Library::open(&db_path).unwrap();

        let version: i64 = lib.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // New columns exist (prepare succeeds even against empty tables).
        assert!(lib.conn.prepare("SELECT first_seen_unix FROM decks").is_ok());
        assert!(lib.conn.prepare("SELECT skipped FROM scan_history").is_ok());
        assert!(lib.conn.prepare("SELECT exclude_globs FROM roots").is_ok());

        // New tables exist (v1 diagnostics + v2 embeddings).
        for table in ["scan_issues", "render_issues", "export_picks", "embeddings"] {
            let sql = format!("SELECT COUNT(*) FROM {table}");
            let n: i64 = lib.conn.query_row(&sql, [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0);
        }

        // v2 slide-hash columns exist and are NULL for the pre-migration row
        // (they are only populated by a build that rescans).
        assert!(lib.conn.prepare("SELECT content_hash, text_hash FROM slides").is_ok());
        let (ch, th): (Option<String>, Option<String>) = lib
            .conn
            .query_row("SELECT content_hash, text_hash FROM slides", [], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .unwrap();
        assert_eq!(ch, None);
        assert_eq!(th, None);

        // Favorites survived the migration.
        let sf: i64 = lib.conn.query_row("SELECT COUNT(*) FROM slide_favorites", [], |r| r.get(0)).unwrap();
        assert_eq!(sf, 1);
        let df: i64 = lib.conn.query_row("SELECT COUNT(*) FROM deck_favorites", [], |r| r.get(0)).unwrap();
        assert_eq!(df, 1);

        // first_seen_unix backfilled from modified_unix; exclude_globs parsed from '[]'.
        let decks = lib.decks().unwrap();
        assert_eq!(decks.len(), 1);
        assert_eq!(decks[0].first_seen_unix, 1_700_000_000);
        let roots = lib.roots().unwrap();
        assert_eq!(roots.len(), 1);
        assert!(roots[0].exclude_globs.is_empty());
    }

    #[test]
    fn saved_searches_crud_round_trip() {
        let mut lib = Library::open_in_memory().unwrap();
        assert!(lib.list_saved_searches().unwrap().is_empty());

        let f1 = SearchFilters {
            deck_query: Some("finance".into()),
            modified_from: Some(1_700_000_000),
            ..Default::default()
        };
        let a = lib.save_search("Finance decks", "title:revenue", &f1).unwrap();
        let b = lib
            .save_search(
                "Starred",
                "",
                &SearchFilters { favorites_only: Some(true), ..Default::default() },
            )
            .unwrap();
        assert_ne!(a.id, b.id);

        let list = lib.list_saved_searches().unwrap();
        assert_eq!(list.len(), 2);
        // Insertion order preserved via `position`.
        assert_eq!(list[0].id, a.id);
        assert_eq!(list[0].name, "Finance decks");
        assert_eq!(list[0].query, "title:revenue");
        assert_eq!(list[0].filters.deck_query.as_deref(), Some("finance"));
        assert_eq!(list[0].filters.modified_from, Some(1_700_000_000));
        assert_eq!(list[1].name, "Starred");
        assert_eq!(list[1].filters.favorites_only, Some(true));

        lib.rename_saved_search(a.id, "Finance").unwrap();
        assert_eq!(lib.list_saved_searches().unwrap()[0].name, "Finance");

        lib.delete_saved_search(a.id).unwrap();
        let list = lib.list_saved_searches().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, b.id);
    }

    #[test]
    fn saved_searches_persist_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lib.db");
        let saved_id;
        {
            let mut lib = Library::open(&db_path).unwrap();
            let f = SearchFilters {
                path_prefix: Some("/Users/you/Decks".into()),
                ..Default::default()
            };
            saved_id = lib.save_search("My folder", "deck:roadmap", &f).unwrap().id;
        }
        // Reopen the same file: the saved search survives.
        let lib = Library::open(&db_path).unwrap();
        let list = lib.list_saved_searches().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, saved_id);
        assert_eq!(list[0].name, "My folder");
        assert_eq!(list[0].query, "deck:roadmap");
        assert_eq!(list[0].filters.path_prefix.as_deref(), Some("/Users/you/Decks"));
    }

    #[test]
    fn migration_v1_to_v2_creates_saved_searches_preserving_data() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lib.db");

        // Build a genuine v1 database: baseline + v1 migration, stamped at
        // user_version 1 so the v2 migration has not yet run.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(MIGRATIONS_V1).unwrap();
            conn.execute_batch("PRAGMA user_version = 1;").unwrap();

            conn.execute("INSERT INTO roots(path) VALUES(?1)", params!["/decks/root"]).unwrap();
            let root_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO decks(root_id, path, file_name, title, author, slide_count, \
                 modified_unix, size_bytes, slide_width_emu, slide_height_emu, content_hash, \
                 first_seen_unix) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    root_id,
                    "/decks/root/a.pptx",
                    "a.pptx",
                    "A",
                    Option::<String>::None,
                    1i64,
                    1_700_000_000i64,
                    1000i64,
                    12_192_000i64,
                    6_858_000i64,
                    "hash",
                    1_700_000_000i64,
                ],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO deck_favorites(deck_path, added_unix) VALUES(?1,?2)",
                params!["/decks/root/a.pptx", 111i64],
            )
            .unwrap();

            // saved_searches must not exist yet at v1.
            let exists: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master \
                     WHERE type='table' AND name='saved_searches'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(exists, 0);
        }

        // Opening through Library runs every pending migration.
        let mut lib = Library::open(&db_path).unwrap();
        let version: i64 = lib.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Existing rows preserved across the migration.
        assert_eq!(lib.decks().unwrap().len(), 1);
        let df: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM deck_favorites", [], |r| r.get(0)).unwrap();
        assert_eq!(df, 1);

        // The new table exists and is usable.
        assert!(lib.list_saved_searches().unwrap().is_empty());
        let s = lib.save_search("After migration", "revenue", &SearchFilters::default()).unwrap();
        let list = lib.list_saved_searches().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].id, s.id);
    }

    #[test]
    fn double_open_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lib.db");

        // Mirrors the desktop opening the library twice at startup: the second
        // open must see user_version==1 and skip the ALTERs (no duplicate-column).
        let a = Library::open(&db_path).unwrap();
        let mut b = Library::open(&db_path).unwrap();

        b.add_root(dir.path()).unwrap();
        // The first handle sees the second's committed write over the shared file.
        assert!(!a.roots().unwrap().is_empty());
    }

    #[test]
    fn exclude_pattern_skips_matching_file() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Keep")
            .slide(SlideSpec::new("Keep").bullets(&["keepme"]))
            .write_to(&dir.path().join("keep.pptx"))
            .unwrap();
        DeckSpec::new("Draft")
            .slide(SlideSpec::new("Draft").bullets(&["draftme"]))
            .write_to(&dir.path().join("draft.pptx"))
            .unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        let root = lib.add_root(dir.path()).unwrap();
        lib.set_root_excludes(root.id, &["**/draft.pptx".into()]).unwrap();
        scan_silent(&mut lib);

        assert_eq!(lib.stats().unwrap().0, 1);
        assert!(lib.search("draftme", &SearchFilters::default()).unwrap().is_empty());
        assert!(!lib.search("keepme", &SearchFilters::default()).unwrap().is_empty());
    }

    #[test]
    fn exclude_pattern_prunes_directory() {
        let dir = tempfile::tempdir().unwrap();
        let live = dir.path().join("live");
        let archive = dir.path().join("archive");
        std::fs::create_dir_all(&live).unwrap();
        std::fs::create_dir_all(&archive).unwrap();
        DeckSpec::new("Current")
            .slide(SlideSpec::new("Current").bullets(&["liveterm"]))
            .write_to(&live.join("current.pptx"))
            .unwrap();
        DeckSpec::new("Old")
            .slide(SlideSpec::new("Old").bullets(&["oldterm"]))
            .write_to(&archive.join("old.pptx"))
            .unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        let root = lib.add_root(dir.path()).unwrap();
        lib.set_root_excludes(root.id, &["**/archive/**".into()]).unwrap();
        scan_silent(&mut lib);

        assert_eq!(lib.stats().unwrap().0, 1);
        assert!(lib.search("oldterm", &SearchFilters::default()).unwrap().is_empty());
        assert!(!lib.search("liveterm", &SearchFilters::default()).unwrap().is_empty());
    }

    #[test]
    fn exclusion_removes_previously_indexed_deck() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Keep")
            .slide(SlideSpec::new("Keep").bullets(&["keepterm"]))
            .write_to(&dir.path().join("keep.pptx"))
            .unwrap();
        DeckSpec::new("Secret")
            .slide(SlideSpec::new("Secret").bullets(&["secretterm"]))
            .write_to(&dir.path().join("secret.pptx"))
            .unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        let root = lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        assert_eq!(lib.stats().unwrap().0, 2);
        assert!(!lib.search("secretterm", &SearchFilters::default()).unwrap().is_empty());

        // A newly-added pattern drops the already-indexed deck via the seen-sweep.
        lib.set_root_excludes(root.id, &["**/secret.pptx".into()]).unwrap();
        let finished = scan_silent(&mut lib);
        assert!(matches!(finished, ScanEvent::Finished { removed: 1, .. }));
        assert_eq!(lib.stats().unwrap().0, 1);
        assert!(lib.search("secretterm", &SearchFilters::default()).unwrap().is_empty());
    }

    #[test]
    fn set_root_excludes_rejects_invalid_pattern() {
        let (_dir, mut lib) = two_deck_library();
        let root_id = lib.roots().unwrap()[0].id;

        // Unclosed char class: rejected, and nothing is persisted.
        assert!(lib.set_root_excludes(root_id, &["a[b".into()]).is_err());
        let root = lib.roots().unwrap().into_iter().find(|r| r.id == root_id).unwrap();
        assert!(root.exclude_globs.is_empty());

        // A valid pattern still stores fine afterwards.
        let updated = lib.set_root_excludes(root_id, &["**/x.pptx".into()]).unwrap();
        assert_eq!(updated.exclude_globs, vec!["**/x.pptx".to_string()]);
    }

    #[test]
    fn render_issues_roundtrip() {
        let mut lib = Library::open_in_memory().unwrap();
        lib.record_render_issues("/decks/a.pptx", 3, "hashA", &["chart".into(), "smartart".into()])
            .unwrap();
        assert_eq!(
            lib.render_issues_for("/decks/a.pptx", 3, "hashA").unwrap(),
            vec!["chart".to_string(), "smartart".to_string()]
        );
    }

    #[test]
    fn render_issues_hash_mismatch_returns_empty() {
        let mut lib = Library::open_in_memory().unwrap();
        lib.record_render_issues("/decks/a.pptx", 3, "hashA", &["chart".into()]).unwrap();
        // A stale content hash means the cached render no longer describes the file.
        assert!(lib.render_issues_for("/decks/a.pptx", 3, "hashB").unwrap().is_empty());
    }

    #[test]
    fn record_render_issues_empty_deletes_row() {
        let mut lib = Library::open_in_memory().unwrap();
        lib.record_render_issues("/decks/a.pptx", 3, "hashA", &["chart".into()]).unwrap();
        // Re-recording an empty set removes the row rather than storing "[]".
        lib.record_render_issues("/decks/a.pptx", 3, "hashA", &[]).unwrap();
        assert!(lib.render_issues_for("/decks/a.pptx", 3, "hashA").unwrap().is_empty());
    }

    /// Insert a single-slide deck with fully-controlled columns (bypassing the
    /// parser) so browse-order and render-drop tests can pin exact values.
    fn insert_deck(
        lib: &Library,
        root_id: i64,
        path: &str,
        file_name: &str,
        modified: i64,
        first_seen: i64,
        hash: &str,
    ) {
        lib.conn
            .execute(
                "INSERT INTO decks(root_id, path, file_name, title, author, slide_count, \
                 modified_unix, size_bytes, slide_width_emu, slide_height_emu, content_hash, \
                 first_seen_unix) VALUES(?1,?2,?3,?3,'a',1,?4,10,960,540,?5,?6)",
                params![root_id, path, file_name, modified, hash, first_seen],
            )
            .unwrap();
        let deck_id = lib.conn.last_insert_rowid();
        lib.conn
            .execute(
                "INSERT INTO slides(deck_id, slide_index, title, body_text, notes, thumb_path) \
                 VALUES(?1, 1, ?2, 'body', NULL, NULL)",
                params![deck_id, file_name],
            )
            .unwrap();
    }

    #[test]
    fn browse_sort_selects_window_by_key() {
        let mut lib = Library::open_in_memory().unwrap();
        lib.conn
            .execute(
                "INSERT INTO roots(path, last_scan_unix, exclude_globs) VALUES('/r', NULL, '[]')",
                [],
            )
            .unwrap();
        let root_id = lib.conn.last_insert_rowid();
        // (path, file_name, modified, first_seen)
        insert_deck(&lib, root_id, "/r/zeta.pptx", "zeta.pptx", 300, 100, "h1");
        insert_deck(&lib, root_id, "/r/alpha.pptx", "alpha.pptx", 200, 300, "h2");
        insert_deck(&lib, root_id, "/r/mid.pptx", "mid.pptx", 100, 200, "h3");
        // Export mid.pptx twice so it leads the "exported" sort despite the
        // oldest modified time — the very case a client reorder of a
        // modified-DESC window past the limit could never surface.
        lib.record_export(
            "/out.pptx",
            "t",
            2,
            1,
            &[
                SlidePick { pptx_path: "/r/mid.pptx".into(), slide_index: 1 },
                SlidePick { pptx_path: "/r/mid.pptx".into(), slide_index: 1 },
            ],
        )
        .unwrap();

        // With LIMIT 1, the single returned slide must belong to the deck the
        // key ranks first — proving the window itself (not just a client
        // reorder) honors the sort.
        let top = |sort: &str| -> String {
            let f = SearchFilters {
                limit: Some(1),
                sort: Some(sort.to_string()),
                ..Default::default()
            };
            lib.search("", &f).unwrap()[0].deck.file_name.clone()
        };
        assert_eq!(top("name"), "alpha.pptx", "alphabetical first");
        assert_eq!(top("modified"), "zeta.pptx", "newest modified");
        assert_eq!(top("added"), "alpha.pptx", "newest first_seen");
        assert_eq!(top("exported"), "mid.pptx", "most exported");
        // No/unknown sort keeps the historical modified-DESC default.
        let f = SearchFilters { limit: Some(1), ..Default::default() };
        assert_eq!(lib.search("", &f).unwrap()[0].deck.file_name, "zeta.pptx");
    }

    #[test]
    fn render_drops_aggregates_and_excludes_stale_and_orphans() {
        let mut lib = Library::open_in_memory().unwrap();
        lib.conn
            .execute(
                "INSERT INTO roots(path, last_scan_unix, exclude_globs) VALUES('/r', NULL, '[]')",
                [],
            )
            .unwrap();
        let root_id = lib.conn.last_insert_rowid();
        insert_deck(&lib, root_id, "/r/a.pptx", "a.pptx", 1, 1, "hashA");

        // Two live slides of the deck record drops with the matching content
        // hash; chart appears on both, smartart on one.
        lib.record_render_issues("/r/a.pptx", 1, "hashA", &["chart".into()]).unwrap();
        lib.record_render_issues("/r/a.pptx", 2, "hashA", &["chart".into(), "smartart".into()])
            .unwrap();
        // Stale row (hash no longer matches the deck) — excluded by the JOIN.
        lib.record_render_issues("/r/a.pptx", 3, "STALE", &["ole".into()]).unwrap();
        // Orphan row (no such deck path) — excluded by the JOIN.
        lib.record_render_issues("/r/gone.pptx", 1, "whatever", &["chart".into()]).unwrap();

        let drops: Vec<(String, i64)> = lib
            .stats_overview()
            .unwrap()
            .render_drops
            .into_iter()
            .map(|d| (d.kind, d.slides))
            .collect();
        // Distinct-slide counts, sorted by count desc then kind asc; the stale
        // "ole" and orphan "chart" rows must not appear.
        assert_eq!(drops, vec![("chart".into(), 2), ("smartart".into(), 1)]);
    }

    #[test]
    fn export_history_trim_cascades_picks() {
        let mut lib = Library::open_in_memory().unwrap();
        // 205 exports, each contributing one pick for the same deck.
        for i in 0..205 {
            lib.record_export(
                &format!("/out/{i}.pptx"),
                "t",
                1,
                1,
                &[SlidePick { pptx_path: "/decks/a.pptx".into(), slide_index: 1 }],
            )
            .unwrap();
        }
        let history: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM export_history", [], |r| r.get(0)).unwrap();
        assert_eq!(history, 200, "export_history bounded to 200 rows");

        let orphans: i64 = lib
            .conn
            .query_row(
                "SELECT COUNT(*) FROM export_picks \
                 WHERE export_id NOT IN (SELECT id FROM export_history)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0, "trim cascade left no orphan picks");

        // export_counts sees only surviving picks — one per surviving export.
        assert_eq!(lib.export_counts().unwrap().get("/decks/a.pptx").copied(), Some(200));
    }

    #[test]
    fn clean_rescan_clears_last_scan_issues() {
        let dir = tempfile::tempdir().unwrap();
        DeckSpec::new("Good")
            .slide(SlideSpec::new("Title").bullets(&["ok"]))
            .write_to(&dir.path().join("good.pptx"))
            .unwrap();
        std::fs::write(dir.path().join("broken.pptx"), b"not a zip").unwrap();

        let mut lib = Library::open_in_memory().unwrap();
        lib.add_root(dir.path()).unwrap();
        scan_silent(&mut lib);
        let o = lib.stats_overview().unwrap();
        assert_eq!(o.last_scan.as_ref().unwrap().skipped, 1);
        assert_eq!(o.last_scan_issues.len(), 1);

        // Remove the broken file and rescan: the newest scan has no issues, so
        // the Problems surface empties even though older scans' rows persist in
        // the table until trimmed. Guards the MAX(scan_id) scoping.
        std::fs::remove_file(dir.path().join("broken.pptx")).unwrap();
        scan_silent(&mut lib);
        let o = lib.stats_overview().unwrap();
        assert_eq!(o.last_scan.as_ref().unwrap().skipped, 0);
        assert!(o.last_scan_issues.is_empty());
    }

    // --- tags ----------------------------------------------------------------

    fn finance_slides(lib: &Library) -> Vec<SlideRecord> {
        let deck = lib
            .decks()
            .unwrap()
            .into_iter()
            .find(|d| d.file_name == "finance.pptx")
            .unwrap();
        lib.slides_for_deck(deck.id).unwrap()
    }

    #[test]
    fn tags_crud_set_remove_prune() {
        let (_dir, mut lib) = two_deck_library();
        assert!(lib.list_tags().unwrap().is_empty());

        let slides = finance_slides(&lib);
        let s1 = slides[0].id;
        let s2 = slides[1].id;

        // Assign two tags to slide 1 (creates them). Blank names are ignored.
        lib.set_slide_tags(s1, &["Finance".into(), "  ".into(), "KPI".into()])
            .unwrap();
        let tags = lib.list_tags().unwrap();
        assert_eq!(tags.len(), 2, "blank skipped, two tags created");
        // Alphabetical (COLLATE NOCASE): Finance, KPI.
        assert_eq!(tags[0].name, "Finance");
        assert_eq!(tags[0].slide_count, 1);
        assert_eq!(lib.slide_tags(s1).unwrap().len(), 2);

        // Case-insensitive reuse: "finance" on slide 2 reuses the existing tag,
        // preserving the original casing.
        lib.set_slide_tags(s2, &["finance".into()]).unwrap();
        let finance = lib
            .list_tags()
            .unwrap()
            .into_iter()
            .find(|t| t.name.eq_ignore_ascii_case("finance"))
            .unwrap();
        assert_eq!(finance.slide_count, 2, "same tag now on two slides");
        assert_eq!(finance.name, "Finance", "original casing kept");

        // Replace slide 1's set with only Finance: KPI loses its last assignment
        // and is pruned.
        lib.set_slide_tags(s1, &["Finance".into()]).unwrap();
        let names: Vec<String> = lib.list_tags().unwrap().into_iter().map(|t| t.name).collect();
        assert_eq!(names, vec!["Finance".to_string()], "KPI pruned");

        // delete_tag removes it and cascades slide_tags.
        let fid = lib.list_tags().unwrap()[0].id;
        lib.delete_tag(fid).unwrap();
        assert!(lib.list_tags().unwrap().is_empty());
        assert!(lib.slide_tags(s1).unwrap().is_empty());
        let st_rows: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM slide_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(st_rows, 0, "cascade removed slide_tags rows");
    }

    #[test]
    fn rename_tag_collision_and_empty_error() {
        let (_dir, mut lib) = two_deck_library();
        let s1 = finance_slides(&lib)[0].id;
        lib.set_slide_tags(s1, &["Alpha".into(), "Beta".into()]).unwrap();
        let alpha = lib
            .list_tags()
            .unwrap()
            .into_iter()
            .find(|t| t.name == "Alpha")
            .unwrap()
            .id;

        // Case-insensitive collision with "Beta" is rejected (no silent merge).
        let err = lib.rename_tag(alpha, "beta").unwrap_err();
        assert!(matches!(err, Error::InvalidInput(_)));
        assert_eq!(lib.list_tags().unwrap().len(), 2, "no merge happened");
        assert!(lib.list_tags().unwrap().iter().any(|t| t.name == "Alpha"));

        // Empty / whitespace rejected.
        assert!(lib.rename_tag(alpha, "   ").is_err());

        // A non-colliding rename works.
        lib.rename_tag(alpha, "Alpha Prime").unwrap();
        assert!(lib.list_tags().unwrap().iter().any(|t| t.name == "Alpha Prime"));
    }

    #[test]
    fn tags_survive_rescan_that_modifies_deck() {
        let (dir, mut lib) = two_deck_library();
        let sid = finance_slides(&lib)[0].id;
        lib.set_slide_tags(sid, &["Keeper".into()]).unwrap();
        assert_eq!(lib.list_tags().unwrap()[0].slide_count, 1);

        // Touch mtime -> content_hash changes -> deck reindexed (slides deleted +
        // reinserted with new rowids).
        let deck_path = dir.path().join("finance.pptx");
        let future = UNIX_EPOCH + Duration::from_secs(now_unix() as u64 + 500);
        OpenOptions::new().write(true).open(&deck_path).unwrap().set_modified(future).unwrap();
        scan_silent(&mut lib);

        let tags = lib.list_tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].name, "Keeper");
        assert_eq!(tags[0].slide_count, 1, "tag lost across rescan");

        // The new (different-id) slide row resolves the tag by (path, index).
        let new_sid = finance_slides(&lib)[0].id;
        let st = lib.slide_tags(new_sid).unwrap();
        assert_eq!(st.len(), 1);
        assert_eq!(st[0].name, "Keeper");
    }

    #[test]
    fn tags_survive_clear_and_rescan() {
        let (_dir, mut lib) = two_deck_library();
        let sid = finance_slides(&lib)[0].id;
        lib.set_slide_tags(sid, &["Persistent".into()]).unwrap();

        lib.clear().unwrap();
        // The assignment row survives clear; the live count is 0 (no slides
        // indexed) but the tag itself is retained.
        let tags = lib.list_tags().unwrap();
        assert_eq!(tags.len(), 1, "tag rows survive clear");
        assert_eq!(tags[0].slide_count, 0, "no live slides after clear");
        let st_rows: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM slide_tags", [], |r| r.get(0)).unwrap();
        assert_eq!(st_rows, 1, "slide_tags row survives clear");

        // Rescan relinks the assignment by (deck_path, slide_index).
        scan_silent(&mut lib);
        let tags = lib.list_tags().unwrap();
        assert_eq!(tags.len(), 1);
        assert_eq!(tags[0].slide_count, 1, "tag relinked after rescan");
    }

    #[test]
    fn tag_filter_in_search_and_browse() {
        let (_dir, mut lib) = two_deck_library();
        // finance slide 1 ("Q3 Results", body "Revenue up 12%") gets tagged;
        // finance slide 2 ("Outlook", body "... office opens") does not.
        let tagged = finance_slides(&lib)[0].id;
        lib.set_slide_tags(tagged, &["Highlight".into()]).unwrap();
        let tag_id = lib.list_tags().unwrap()[0].id;
        let filters = SearchFilters { tag_id: Some(tag_id), ..Default::default() };

        // Browse (empty query) with tag_id -> exactly the tagged slide.
        let browse = lib.search("", &filters).unwrap();
        assert_eq!(browse.len(), 1);
        assert_eq!(browse[0].slide.id, tagged);

        // Full-text search + tag_id: a query matching the tagged slide keeps it.
        let s = lib.search("revenue", &filters).unwrap();
        assert_eq!(s.len(), 1);
        assert_eq!(s[0].slide.id, tagged);

        // A query whose only text match is on a NON-tagged slide is filtered out
        // (proves the clause joins into the FTS path too).
        assert!(
            lib.search("office", &filters).unwrap().is_empty(),
            "tag filter removes untagged text matches"
        );
        // ...but matches without the tag filter.
        assert_eq!(lib.search("office", &SearchFilters::default()).unwrap().len(), 1);
    }

    /// Simulates the mid-wave dev breakage that motivated the one-version-per-
    /// change rule: a database stamped at user_version 2 (saved searches
    /// applied) but WITHOUT the tags tables must gain them on reopen via the
    /// v3 step — never crash on a missing table.
    #[test]
    fn reopening_heals_db_stamped_v2_without_tags() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lib.db");
        drop(Library::open(&db_path).unwrap()); // fresh DB, fully migrated

        {
            // Reconstruct a GENUINE v2-era DB: strip the v3 objects being tested
            // AND every later version's objects. A DB really stamped at 2
            // predates v4, so it cannot carry the v4 columns — each batch and
            // its stamp commit atomically. Leaving them in place would make the
            // v4 replay (non-idempotent ALTERs, by design) fail on a state that
            // cannot occur in the wild.
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "DROP INDEX idx_slide_tags_slide;
                 DROP TABLE slide_tags;
                 DROP TABLE tags;
                 DROP INDEX idx_slides_content_hash;
                 DROP TABLE embeddings;
                 ALTER TABLE slides DROP COLUMN content_hash;
                 ALTER TABLE slides DROP COLUMN text_hash;
                 PRAGMA user_version = 2;",
            )
            .unwrap();
        }

        let lib = Library::open(&db_path).unwrap();
        assert!(lib.list_tags().unwrap().is_empty(), "tags tables recreated by the v3 step");
        let version: i64 =
            lib.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    /// Same healing shape for WS-B: a DB stamped at 3 WITHOUT the hash columns
    /// or the embeddings table gains them on reopen — precisely because the v4
    /// step runs for it. The v4 `ALTER TABLE ADD COLUMN`s are NOT idempotent,
    /// which is exactly why they live in their own version step that a
    /// v3-stamped DB executes exactly once; a v4-stamped DB never re-runs them,
    /// so reopening an up-to-date DB can never hit 'duplicate column name'.
    #[test]
    fn reopening_heals_db_stamped_v3_without_embeddings() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lib.db");
        drop(Library::open(&db_path).unwrap()); // fresh DB, fully migrated

        {
            // Strip every v4 object and stamp the DB back to 3 — the state of a
            // library last touched by a build without the embeddings step.
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(
                "DROP INDEX idx_slides_content_hash;
                 DROP TABLE embeddings;
                 ALTER TABLE slides DROP COLUMN content_hash;
                 ALTER TABLE slides DROP COLUMN text_hash;
                 PRAGMA user_version = 3;",
            )
            .unwrap();
            // Sanity: the column really is gone before the healing reopen.
            assert!(conn.prepare("SELECT content_hash FROM slides").is_err());
        }

        // Reopen: migrate() finds user_version == 3 < SCHEMA_VERSION and runs
        // EXACTLY the v4 batch — the objects below can only come from it.
        let lib = Library::open(&db_path).unwrap();
        let version: i64 =
            lib.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
        assert!(
            lib.conn.prepare("SELECT content_hash, text_hash FROM slides").is_ok(),
            "hash columns recreated by the v4 step"
        );
        let embeddings: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM embeddings", [], |r| r.get(0)).unwrap();
        assert_eq!(embeddings, 0, "embeddings table recreated by the v4 step");
        drop(lib);

        // A v4-stamped DB reopening must NOT error: the non-idempotent ALTERs
        // are guarded by user_version and never run twice.
        let again = Library::open(&db_path).unwrap();
        let version: i64 =
            again.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);
    }

    #[test]
    fn migration_v1_to_latest_creates_tags_and_keeps_data() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("lib.db");

        // Build a genuine v1 database: baseline schema + v1 migration, user_version
        // pinned to 1, with a deck/slide/favorite already present.
        {
            let conn = Connection::open(&db_path).unwrap();
            conn.execute_batch(SCHEMA).unwrap();
            conn.execute_batch(MIGRATIONS_V1).unwrap();
            conn.execute_batch("PRAGMA user_version = 1;").unwrap();

            conn.execute("INSERT INTO roots(path) VALUES(?1)", params!["/decks/root"]).unwrap();
            let root_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO decks(root_id, path, file_name, title, author, slide_count, \
                 modified_unix, size_bytes, slide_width_emu, slide_height_emu, content_hash, \
                 first_seen_unix) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params![
                    root_id,
                    "/decks/root/a.pptx",
                    "a.pptx",
                    "A",
                    Option::<String>::None,
                    1i64,
                    1_700_000_000i64,
                    1000i64,
                    12_192_000i64,
                    6_858_000i64,
                    "hash",
                    1_700_000_000i64,
                ],
            )
            .unwrap();
            let deck_id = conn.last_insert_rowid();
            conn.execute(
                "INSERT INTO slides(deck_id, slide_index, title, body_text, notes, thumb_path) \
                 VALUES(?1,?2,?3,?4,NULL,NULL)",
                params![deck_id, 1i64, "One", "body"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO slide_favorites(deck_path, slide_index, added_unix) VALUES(?1,?2,?3)",
                params!["/decks/root/a.pptx", 1i64, 111i64],
            )
            .unwrap();

            let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 1);
        }

        // Opening through Library runs every pending migration.
        let lib = Library::open(&db_path).unwrap();
        let version: i64 = lib.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION);

        // Tags tables now exist and are empty.
        for table in ["tags", "slide_tags"] {
            let n: i64 =
                lib.conn.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0);
        }

        // Pre-existing data survived the migration untouched.
        assert_eq!(lib.decks().unwrap().len(), 1);
        let sf: i64 =
            lib.conn.query_row("SELECT COUNT(*) FROM slide_favorites", [], |r| r.get(0)).unwrap();
        assert_eq!(sf, 1);
    }
}
