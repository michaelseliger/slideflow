//! `slideflow` — command-line companion for the Slideflow engine.
//!
//! A thin wrapper over `slideflow-core`: it parses arguments, calls the engine,
//! and formats output. There is deliberately **no business logic here** — every
//! subcommand maps onto an existing engine call (`Library::{open,add_root,scan,
//! search,stats,stats_overview}`, `pptx::compose`, `render::render_slide`).
//!
//! Because it only depends on the pure-Rust core (no GTK/WebKit), it builds and
//! runs on the same Linux/macOS/Windows runners as the engine tests.
//!
//! Conventions:
//!   * results go to **stdout**, diagnostics/errors to **stderr** (clean piping);
//!   * exit 0 = success, 1 = operational error (bad path, missing slide),
//!     2 = usage error (clap's default for bad arguments).

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use clap::{Parser, Subcommand, ValueEnum};

use slideflow_core::index::Library;
use slideflow_core::model::{FitMode, ScanEvent, SearchFilters, SlidePick};
use slideflow_core::pptx::composer::{compose, ComposeOptions};
use slideflow_core::pptx::PresentationFile;
use slideflow_core::render::{render_slide, RenderOptions};

/// Index PPTX folders, search slides, and compose or render decks — from the
/// terminal.
#[derive(Debug, Parser)]
#[command(name = "slideflow", version, about, long_about = None)]
struct Cli {
    /// Path to the library SQLite database. Created on `index`; must already
    /// exist for `search`/`stats`. Ignored by `compose`/`render`.
    #[arg(long, global = true, default_value = "slideflow.db", value_name = "PATH")]
    db: PathBuf,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Index every `.pptx` under a folder into the library database.
    ///
    /// Opens (or creates) the database at `--db`, registers the folder as a
    /// watched root, and runs an incremental scan. Per-deck progress is printed
    /// to stderr; a one-line summary and the resulting library totals go to
    /// stdout.
    Index {
        /// Folder to scan recursively for `.pptx` files.
        folder: PathBuf,
    },

    /// Full-text search the indexed slides.
    ///
    /// The query supports the full advanced syntax:
    ///
    ///   - title:word / deck:word / notes:word / body:word  — field-scoped terms
    ///   - "exact phrase"                                    — quoted phrase match
    ///   - a OR b                                            — either term (space = AND)
    ///   - NOT term  or  -term                               — exclude
    ///   - before:2024-01-31 / after:2023-06-01             — deck-modified date bounds
    ///
    /// Terms are prefix-matched and diacritic-insensitive; results are bm25-ranked,
    /// weighting title over deck over body over notes.
    #[command(verbatim_doc_comment)]
    Search {
        /// The search query (see the command help for the advanced syntax).
        query: String,
        /// Maximum number of hits to return.
        #[arg(long, default_value_t = 20, value_name = "N")]
        limit: usize,
        /// Emit the `SearchHit` list as JSON instead of a human table.
        #[arg(long)]
        json: bool,
    },

    /// Compose a new deck from picked slides, preserving each slide's formatting.
    ///
    /// Each pick is `DECK.pptx:N`, where N is the 1-based slide index in that
    /// deck (same convention as the engine's `compose_demo` example). Picks are
    /// written in the order given; a slide keeps its original layout, master,
    /// theme, and media.
    Compose {
        /// Output `.pptx` path (overwritten if it exists).
        out: PathBuf,
        /// One or more picks in `DECK.pptx:N` form (N is 1-based).
        #[arg(required = true, value_name = "DECK.pptx:N")]
        picks: Vec<String>,
        /// Title written to the output's docProps (default: "Slideflow Deck").
        #[arg(long, value_name = "TITLE")]
        title: Option<String>,
        /// Carry speaker notes into the output.
        #[arg(long)]
        include_notes: bool,
        /// How to fit slides whose aspect ratio differs from the output canvas
        /// (the first pick's size). Omitted: leave mismatches unscaled and warn.
        #[arg(long, value_enum, value_name = "MODE")]
        fit_mode: Option<FitModeArg>,
    },

    /// Render a single slide to a self-contained SVG file.
    Render {
        /// Source `.pptx`.
        deck: PathBuf,
        /// 1-based slide index to render.
        index: usize,
        /// Output `.svg` path.
        out: PathBuf,
    },

    /// Show library statistics (counts, size, recent activity).
    Stats {
        /// Emit the full `StatsOverview` as JSON instead of a human table.
        #[arg(long)]
        json: bool,
    },
}

/// CLI mirror of [`slideflow_core::model::FitMode`], spelled in kebab-case for
/// the `--fit-mode` flag (`ensure-fit` / `maximize`).
#[derive(Debug, Clone, Copy, ValueEnum)]
enum FitModeArg {
    EnsureFit,
    Maximize,
}

impl From<FitModeArg> for FitMode {
    fn from(m: FitModeArg) -> Self {
        match m {
            FitModeArg::EnsureFit => FitMode::EnsureFit,
            FitModeArg::Maximize => FitMode::Maximize,
        }
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            eprintln!("error: {msg}");
            ExitCode::FAILURE // exit code 1
        }
    }
}

/// Dispatch a parsed command. Every arm returns `Result<(), String>`; the string
/// is the human-readable error shown on stderr before exiting non-zero.
fn run(cli: &Cli) -> Result<(), String> {
    match &cli.command {
        Command::Index { folder } => cmd_index(&cli.db, folder),
        Command::Search { query, limit, json } => cmd_search(&cli.db, query, *limit, *json),
        Command::Compose { out, picks, title, include_notes, fit_mode } => {
            cmd_compose(out, picks, title.clone(), *include_notes, *fit_mode)
        }
        Command::Render { deck, index, out } => cmd_render(deck, *index, out),
        Command::Stats { json } => cmd_stats(&cli.db, *json),
    }
}

// ---------------------------------------------------------------------------
// index
// ---------------------------------------------------------------------------

fn cmd_index(db: &Path, folder: &Path) -> Result<(), String> {
    if !folder.exists() {
        return Err(format!("folder does not exist: {}", folder.display()));
    }
    let mut lib = Library::open(db)
        .map_err(|e| format!("opening library at {}: {e}", db.display()))?;
    lib.add_root(folder)
        .map_err(|e| format!("adding root {}: {e}", folder.display()))?;

    // Per-deck progress to stderr; capture the final tallies for the stdout
    // summary (mirrors the engine's `seed_library` example).
    let mut finished: Option<(usize, usize, usize, usize)> = None;
    lib.scan(&mut |ev| match ev {
        ScanEvent::Started { total_files } => {
            eprintln!("scanning {total_files} file(s) under {}...", folder.display());
        }
        ScanEvent::Deck { path, done, total } => {
            eprintln!("[{done}/{total}] indexed {path}");
        }
        ScanEvent::Skipped { path, reason } => {
            eprintln!("[skipped] {path}: {reason}");
        }
        ScanEvent::Finished { indexed, removed, unchanged, skipped } => {
            finished = Some((indexed, removed, unchanged, skipped));
        }
    })
    .map_err(|e| format!("scan failed: {e}"))?;

    let (indexed, removed, unchanged, skipped) = finished.unwrap_or_default();
    let (decks, slides) = lib
        .stats()
        .map_err(|e| format!("reading library stats: {e}"))?;

    println!(
        "Indexed {indexed} deck(s) ({unchanged} unchanged, {skipped} skipped, {removed} removed)."
    );
    println!(
        "Library at {} now holds {decks} deck(s) / {slides} slide(s).",
        db.display()
    );
    Ok(())
}

// ---------------------------------------------------------------------------
// search
// ---------------------------------------------------------------------------

fn cmd_search(db: &Path, query: &str, limit: usize, json: bool) -> Result<(), String> {
    // `Library::open` would silently *create* an empty database, so guard the
    // read commands explicitly: a missing db is an operational error, not "0 hits".
    require_existing_db(db)?;
    let lib = Library::open(db)
        .map_err(|e| format!("opening library at {}: {e}", db.display()))?;

    let filters = SearchFilters { limit: Some(limit), ..SearchFilters::default() };
    let hits = lib
        .search(query, &filters)
        .map_err(|e| format!("search failed: {e}"))?;

    if json {
        let out = serde_json::to_string_pretty(&hits)
            .map_err(|e| format!("serializing results: {e}"))?;
        println!("{out}");
        return Ok(());
    }

    if hits.is_empty() {
        println!("No matches for {query:?}.");
        return Ok(());
    }

    for (i, hit) in hits.iter().enumerate() {
        let title = hit.slide.title.as_deref().unwrap_or("(untitled)");
        println!(
            "{:>3}. {} · slide {} · {}  (score {:.2})",
            i + 1,
            hit.deck.file_name,
            hit.slide.slide_index,
            title,
            hit.score,
        );
        let snippet = strip_marks(&hit.snippet);
        let snippet = snippet.trim();
        if !snippet.is_empty() {
            println!("     {snippet}");
        }
    }
    Ok(())
}

/// Remove the `<mark>`/`</mark>` highlight tags the FTS snippet wraps matches in.
fn strip_marks(snippet: &str) -> String {
    snippet.replace("<mark>", "").replace("</mark>", "")
}

// ---------------------------------------------------------------------------
// compose
// ---------------------------------------------------------------------------

fn cmd_compose(
    out: &Path,
    picks_raw: &[String],
    title: Option<String>,
    include_notes: bool,
    fit_mode: Option<FitModeArg>,
) -> Result<(), String> {
    let picks = picks_raw
        .iter()
        .map(|s| parse_pick(s))
        .collect::<Result<Vec<_>, _>>()?;

    let opts = ComposeOptions {
        title: title.unwrap_or_else(|| ComposeOptions::default().title),
        include_notes,
        fit_mode: fit_mode.map(FitMode::from),
    };

    let report = compose(&picks, out, &opts).map_err(|e| format!("compose failed: {e}"))?;

    println!(
        "Wrote {} slide(s) from {} deck(s) to {}.",
        report.slides_written, report.source_decks, report.output_path,
    );
    // Neutral informational notes on stdout; non-fatal warnings on stderr so the
    // primary result line pipes clean.
    for note in &report.notes {
        println!("note: {note}");
    }
    for warning in &report.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(())
}

/// Parse one `DECK.pptx:N` pick. Splits on the **last** colon so Windows drive
/// letters (`C:\deck.pptx:3`) parse correctly.
fn parse_pick(spec: &str) -> Result<SlidePick, String> {
    let (path, idx) = spec
        .rsplit_once(':')
        .ok_or_else(|| format!("invalid pick {spec:?}: expected DECK.pptx:N (1-based slide index)"))?;
    let slide_index: usize = idx.parse().map_err(|_| {
        format!("invalid pick {spec:?}: slide index {idx:?} is not a positive integer")
    })?;
    if slide_index == 0 {
        return Err(format!("invalid pick {spec:?}: slide index is 1-based, cannot be 0"));
    }
    if path.is_empty() {
        return Err(format!("invalid pick {spec:?}: missing deck path before ':'"));
    }
    Ok(SlidePick { pptx_path: path.to_string(), slide_index })
}

// ---------------------------------------------------------------------------
// render
// ---------------------------------------------------------------------------

fn cmd_render(deck: &Path, index: usize, out: &Path) -> Result<(), String> {
    let pf = PresentationFile::open(deck)
        .map_err(|e| format!("opening {}: {e}", deck.display()))?;
    // `render_slide` is `render_slide_svg` plus the dropped-construct set, which
    // we surface on stderr; the SVG string itself is what gets written.
    let outcome = render_slide(&pf, index, &RenderOptions::default())
        .map_err(|e| format!("rendering slide {index} of {}: {e}", deck.display()))?;

    std::fs::write(out, &outcome.svg)
        .map_err(|e| format!("writing {}: {e}", out.display()))?;

    if !outcome.dropped.is_empty() {
        eprintln!(
            "note: approximate preview — dropped construct(s): {}",
            outcome.dropped.join(", ")
        );
    }
    println!("Rendered slide {index} of {} to {}.", deck.display(), out.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// stats
// ---------------------------------------------------------------------------

fn cmd_stats(db: &Path, json: bool) -> Result<(), String> {
    require_existing_db(db)?;
    let lib = Library::open(db)
        .map_err(|e| format!("opening library at {}: {e}", db.display()))?;
    let ov = lib
        .stats_overview()
        .map_err(|e| format!("reading stats: {e}"))?;

    if json {
        let out = serde_json::to_string_pretty(&ov)
            .map_err(|e| format!("serializing stats: {e}"))?;
        println!("{out}");
        return Ok(());
    }

    println!("Library: {}", db.display());
    println!("  decks           {}", ov.deck_count);
    println!("  slides          {}", ov.slide_count);
    println!("  total size      {}", human_bytes(ov.total_bytes));
    println!("  favorite decks  {}", ov.favorite_decks);
    println!("  favorite slides {}", ov.favorite_slides);
    match &ov.last_scan {
        Some(s) => println!(
            "  last scan       indexed {}, removed {}, unchanged {} ({} ms)",
            s.indexed, s.removed, s.unchanged, s.duration_ms
        ),
        None => println!("  last scan       (none)"),
    }
    if !ov.largest_decks.is_empty() {
        println!("  largest decks:");
        for d in &ov.largest_decks {
            println!(
                "    {:>10}  {} ({} slides)",
                human_bytes(d.size_bytes),
                d.file_name,
                d.slide_count
            );
        }
    }
    if !ov.recent_searches.is_empty() {
        println!("  recent searches:");
        for s in &ov.recent_searches {
            println!("    {:?} → {} hit(s)", s.query, s.result_count);
        }
    }
    if !ov.recent_exports.is_empty() {
        println!("  recent exports:");
        for x in &ov.recent_exports {
            println!("    {} ({} slides from {} decks)", x.title, x.slide_count, x.source_decks);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

/// Guard the read-only commands: `Library::open` creates a fresh empty database
/// when the file is absent, which would mask a wrong `--db` path as "no results".
fn require_existing_db(db: &Path) -> Result<(), String> {
    if db.exists() {
        Ok(())
    } else {
        Err(format!(
            "no library database at {} — run `slideflow index <folder> --db {}` first",
            db.display(),
            db.display()
        ))
    }
}

/// Format a byte count as a short human-readable string (binary units).
fn human_bytes(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    if bytes < 0 {
        return bytes.to_string();
    }
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}
