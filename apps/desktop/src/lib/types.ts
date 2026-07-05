// TypeScript mirror of `crates/slideflow-core/src/model.rs`.
//
// These types cross the Tauri IPC boundary as JSON. Field names are snake_case
// to match serde's default serialization of the Rust structs EXACTLY — do not
// camelCase them.

/** An indexed presentation file. Mirrors `DeckRecord`. */
export interface DeckRecord {
  id: number;
  /** Absolute path on disk. */
  path: string;
  file_name: string;
  /** Title from docProps/core.xml, falling back to the file stem. */
  title: string;
  author: string | null;
  slide_count: number;
  /** File mtime, unix seconds. */
  modified_unix: number;
  size_bytes: number;
  /** EMU dimensions of the slide canvas. */
  slide_width_emu: number;
  slide_height_emu: number;
  /** First time this deck was indexed (unix seconds; backfilled from
   *  modified_unix on migration). */
  first_seen_unix: number;
  /** User-starred deck (persisted by path, survives reindexing). */
  favorite: boolean;
}

/** One indexed slide. Mirrors `SlideRecord`. */
export interface SlideRecord {
  id: number;
  deck_id: number;
  /** 1-based position in the deck. */
  slide_index: number;
  title: string | null;
  /** All visible text on the slide, newline-joined. */
  body_text: string;
  notes: string | null;
  /** Cached preview SVG path (inside the app cache dir), if rendered. */
  thumb_path: string | null;
  /** User-starred slide (persisted by deck path + index, survives reindexing). */
  favorite: boolean;
}

/** A search result: slide + owning deck + ranking info. Mirrors `SearchHit`. */
export interface SearchHit {
  slide: SlideRecord;
  deck: DeckRecord;
  /** Snippet with `<mark>`/`</mark>` around matched terms. */
  snippet: string;
  /** Higher is better. */
  score: number;
}

/** Search filters; all optional, combined with AND. Mirrors `SearchFilters`. */
export interface SearchFilters {
  /** Substring match on deck file name or title. */
  deck_query?: string | null;
  /** Restrict to decks under this path prefix. */
  path_prefix?: string | null;
  /** Deck modified within [from, to] (unix seconds). */
  modified_from?: number | null;
  modified_to?: number | null;
  /** Only slides the user starred. */
  favorites_only?: boolean | null;
  /** Only slides assigned this tag (by tag id). */
  tag_id?: number | null;
  limit?: number | null;
  /** Browse-mode sort key; ignored by full-text search. Drives the browse
   *  ORDER BY so the limit window selects the correct top-N for the key. */
  sort?: string | null;
}

/** A user-saved search: a named query plus the filters active when saved.
 *  Mirrors `SavedSearch`. */
export interface SavedSearch {
  id: number;
  name: string;
  /** The advanced-syntax query string (may be empty for a filters-only search). */
  query: string;
  filters: SearchFilters;
  /** When it was saved (unix seconds). */
  created_unix: number;
}

/** A user-defined slide tag with a live indexed-slide count. Mirrors `TagRecord`. */
export interface TagRecord {
  id: number;
  name: string;
  slide_count: number;
}

/** A watched root folder. Mirrors `RootRecord`. */
export interface RootRecord {
  id: number;
  path: string;
  deck_count: number;
  slide_count: number;
  last_scan_unix: number | null;
  /** Per-root ignore globs, applied to the scan walk in step4. */
  exclude_globs: string[];
}

/** Progress reported during a scan. Mirrors the `ScanEvent` enum
 *  (serde `#[serde(tag = "kind", rename_all = "snake_case")]`). */
export type ScanEvent =
  | { kind: "started"; total_files: number }
  | { kind: "deck"; path: string; done: number; total: number }
  | { kind: "skipped"; path: string; reason: string }
  | { kind: "finished"; indexed: number; removed: number; unchanged: number; skipped: number };

/** Auto-update lifecycle events streamed on `update:event`. Mirrors the
 *  `UpdateEvent` enum in `src-tauri/src/updates.rs`
 *  (serde `#[serde(tag = "kind", rename_all = "snake_case")]`). */
export type UpdateEvent =
  | { kind: "checking" }
  | { kind: "up_to_date" }
  | { kind: "available"; version: string }
  | { kind: "downloading"; downloaded: number; total: number | null }
  | { kind: "ready"; version: string }
  | { kind: "error"; message: string };

/** Reference to a slide inside a source deck. Mirrors `SlidePick`. */
export interface SlidePick {
  /** Absolute path to the source .pptx. */
  pptx_path: string;
  /** 1-based slide index in that deck. */
  slide_index: number;
}

/** How to fit aspect-mismatched slides on export. Mirrors `FitMode` (serde
 *  snake_case). */
export type FitMode = "ensure_fit" | "maximize";

/** Scratch file paths backing a native drag-out of one slide. Mirrors the
 *  desktop `SlideDragPaths` command struct. */
export interface SlideDragPaths {
  /** Absolute path to the composed single-slide .pptx (the drag payload). */
  pptx: string;
  /** Absolute path to the PNG drag-preview icon, next to the .pptx. */
  icon: string;
}

/** Result of composing a new deck. Mirrors `ComposeReport`. */
export interface ComposeReport {
  output_path: string;
  slides_written: number;
  /** Number of distinct source decks that contributed slides. */
  source_decks: number;
  /** Non-fatal warnings. */
  warnings: string[];
  /** Neutral, informational notes (e.g. a deck scaled to the output size). */
  notes: string[];
}

/** Result of a PNG/PDF export of picked slides. Mirrors `ExportReport`
 *  (`files_written: Vec<PathBuf>` serializes as an array of path strings). */
export interface ExportReport {
  /** Absolute paths written — one PNG per slide, or a single PDF. */
  files_written: string[];
  /** Non-fatal notes (e.g. a slide whose deck could not be opened). */
  warnings: string[];
}

/** Progress streamed on `export:event` during a PNG/PDF export. Mirrors the
 *  `ExportEvent` struct in `src-tauri/src/commands.rs`. */
export interface ExportEvent {
  done: number;
  total: number;
}

/** Library-wide stats. Mirrors the desktop `Stats` command struct. */
export interface Stats {
  deck_count: number;
  slide_count: number;
}

/** One remembered search. Mirrors `SearchHistoryEntry`. */
export interface SearchHistoryEntry {
  query: string;
  result_count: number;
  searched_unix: number;
}

/** One remembered export/composition. Mirrors `ExportRecord`. */
export interface ExportRecord {
  output_path: string;
  title: string;
  slide_count: number;
  source_decks: number;
  exported_unix: number;
}

/** One completed index run. Mirrors `ScanRecord`. */
export interface ScanRecord {
  started_unix: number;
  duration_ms: number;
  indexed: number;
  removed: number;
  unchanged: number;
  skipped: number;
}

/** One per-file problem recorded during a scan. Mirrors `ScanIssue`. */
export interface ScanIssue {
  path: string;
  reason: string;
}

/** Slides where the renderer dropped a construct kind. Mirrors `RenderDropStat`. */
export interface RenderDropStat {
  kind: string;
  slides: number;
}

/** Mirrors the desktop `SlidePreview` command struct. */
export interface SlidePreview {
  path: string;
  dropped: string[];
}

/** Full stats-view payload. Mirrors `StatsOverview`. */
export interface StatsOverview {
  deck_count: number;
  slide_count: number;
  total_bytes: number;
  favorite_slides: number;
  favorite_decks: number;
  last_scan: ScanRecord | null;
  recent_searches: SearchHistoryEntry[];
  recent_exports: ExportRecord[];
  largest_decks: DeckRecord[];
  last_scan_issues: ScanIssue[];
  render_drops: RenderDropStat[];
}
