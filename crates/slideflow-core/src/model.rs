//! Serde-serializable domain types shared between the core engine and the
//! desktop frontend (they cross the Tauri IPC boundary as JSON).

use serde::{Deserialize, Serialize};

/// An indexed presentation file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeckRecord {
    pub id: i64,
    /// Absolute path on disk.
    pub path: String,
    pub file_name: String,
    /// Title from docProps/core.xml, falling back to the file stem.
    pub title: String,
    pub author: Option<String>,
    pub slide_count: i64,
    /// File mtime, unix seconds.
    pub modified_unix: i64,
    pub size_bytes: i64,
    /// EMU dimensions of the slide canvas.
    pub slide_width_emu: i64,
    pub slide_height_emu: i64,
    /// First time this deck was indexed (unix seconds; backfilled from
    /// modified_unix on migration). Powers recently-added sort in step8.
    pub first_seen_unix: i64,
    /// User-starred deck. Keyed by path, so it survives reindexing.
    pub favorite: bool,
}

/// One indexed slide.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlideRecord {
    pub id: i64,
    pub deck_id: i64,
    /// 1-based position in the deck.
    pub slide_index: i64,
    pub title: Option<String>,
    /// All visible text on the slide, newline-joined.
    pub body_text: String,
    pub notes: Option<String>,
    /// Cached preview SVG path (inside the app cache dir), if rendered.
    pub thumb_path: Option<String>,
    /// User-starred slide. Keyed by (deck path, slide index), so it survives
    /// reindexing.
    pub favorite: bool,
}

/// A search result: slide + owning deck + ranking info.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHit {
    pub slide: SlideRecord,
    pub deck: DeckRecord,
    /// Snippet with `<mark>`/`</mark>` around matched terms (HTML-escaped otherwise).
    pub snippet: String,
    /// Higher is better.
    pub score: f64,
}

/// Search filters; all optional, combined with AND.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SearchFilters {
    /// Substring match on deck file name or title.
    pub deck_query: Option<String>,
    /// Restrict to decks under this path prefix.
    pub path_prefix: Option<String>,
    /// Deck modified within [from, to] (unix seconds).
    pub modified_from: Option<i64>,
    pub modified_to: Option<i64>,
    /// Only slides the user starred.
    pub favorites_only: Option<bool>,
    pub limit: Option<usize>,
    /// Browse-mode sort key ("name" | "added" | "modified" | "exported").
    /// Ignored by full-text search (always bm25-ranked); drives the browse
    /// `ORDER BY` so the `limit` window selects the correct top-N for the key.
    pub sort: Option<String>,
}

/// A watched root folder.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RootRecord {
    pub id: i64,
    pub path: String,
    pub deck_count: i64,
    pub slide_count: i64,
    pub last_scan_unix: Option<i64>,
    /// Per-root ignore globs, JSON-encoded in roots.exclude_globs; applied to
    /// the scan walk in step4.
    pub exclude_globs: Vec<String>,
}

/// Progress reported during a scan (sent to the UI as events).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScanEvent {
    Started { total_files: usize },
    Deck { path: String, done: usize, total: usize },
    Skipped { path: String, reason: String },
    Finished { indexed: usize, removed: usize, unchanged: usize, skipped: usize },
}

/// Reference to a slide inside a source deck, as used by the composer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SlidePick {
    /// Absolute path to the source .pptx.
    pub pptx_path: String,
    /// 1-based slide index in that deck.
    pub slide_index: usize,
}

/// How to fit slides from a deck whose aspect ratio differs from the output
/// canvas (the first picked deck's size). Same-aspect size mismatches are always
/// scaled and never consult this; only genuine aspect mismatches are ambiguous.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    /// Scale down to fit inside the canvas, letterboxing the extra axis.
    EnsureFit,
    /// Scale up to fill the canvas, letting the overflowing axis bleed off-canvas.
    Maximize,
}

/// Result of composing a new deck.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComposeReport {
    pub output_path: String,
    pub slides_written: usize,
    /// Number of distinct source decks that contributed slides.
    pub source_decks: usize,
    /// Non-fatal notes (e.g. skipped notes pages, deduplicated masters).
    pub warnings: Vec<String>,
    /// Neutral, informational notes (e.g. a deck scaled to match the output
    /// canvas). Not problems — kept distinct from `warnings`.
    pub notes: Vec<String>,
}

/// One remembered search (for the stats view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchHistoryEntry {
    pub query: String,
    pub result_count: i64,
    pub searched_unix: i64,
}

/// One remembered export/composition (for the stats view).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportRecord {
    pub output_path: String,
    pub title: String,
    pub slide_count: i64,
    pub source_decks: i64,
    pub exported_unix: i64,
}

/// One completed index run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanRecord {
    pub started_unix: i64,
    pub duration_ms: i64,
    pub indexed: i64,
    pub removed: i64,
    pub unchanged: i64,
    pub skipped: i64,
}

/// One per-file problem recorded during a scan (persisted to scan_issues in step3).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScanIssue {
    pub path: String,
    pub reason: String,
}

/// Aggregate: slides where the renderer dropped a given construct kind
/// (populated from render_issues in step6). `kind` is one of chart/smartart/
/// ole/unsupported-image/unknown-shape.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderDropStat {
    pub kind: String,
    pub slides: i64,
}

/// Everything the stats view shows, gathered in one call.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatsOverview {
    pub deck_count: i64,
    pub slide_count: i64,
    /// Sum of all indexed deck file sizes.
    pub total_bytes: i64,
    pub favorite_slides: i64,
    pub favorite_decks: i64,
    /// Most recent completed index run.
    pub last_scan: Option<ScanRecord>,
    pub recent_searches: Vec<SearchHistoryEntry>,
    pub recent_exports: Vec<ExportRecord>,
    /// Biggest decks by file size (descending).
    pub largest_decks: Vec<DeckRecord>,
    /// Per-file problems from the newest scan (populated in step3).
    pub last_scan_issues: Vec<ScanIssue>,
    /// Renderer drop telemetry aggregated by construct kind (populated in step6).
    pub render_drops: Vec<RenderDropStat>,
}
