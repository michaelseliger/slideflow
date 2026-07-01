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
  limit?: number | null;
}

/** A watched root folder. Mirrors `RootRecord`. */
export interface RootRecord {
  id: number;
  path: string;
  deck_count: number;
  slide_count: number;
  last_scan_unix: number | null;
}

/** Progress reported during a scan. Mirrors the `ScanEvent` enum
 *  (serde `#[serde(tag = "kind", rename_all = "snake_case")]`). */
export type ScanEvent =
  | { kind: "started"; total_files: number }
  | { kind: "deck"; path: string; done: number; total: number }
  | { kind: "skipped"; path: string; reason: string }
  | { kind: "finished"; indexed: number; removed: number; unchanged: number };

/** Reference to a slide inside a source deck. Mirrors `SlidePick`. */
export interface SlidePick {
  /** Absolute path to the source .pptx. */
  pptx_path: string;
  /** 1-based slide index in that deck. */
  slide_index: number;
}

/** Result of composing a new deck. Mirrors `ComposeReport`. */
export interface ComposeReport {
  output_path: string;
  slides_written: number;
  /** Number of distinct source decks that contributed slides. */
  source_decks: number;
  /** Non-fatal notes. */
  warnings: string[];
}

/** Library-wide stats. Mirrors the desktop `Stats` command struct. */
export interface Stats {
  deck_count: number;
  slide_count: number;
}
