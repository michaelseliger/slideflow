//! Export picked slides to **PNG images** or a **PDF**, reusing the SVG
//! renderer ([`crate::render`]) as the single source of visual truth.
//!
//! Two pure-Rust pipelines, no LibreOffice / headless PowerPoint:
//!
//! - **PNG** — parse each slide's render SVG with `usvg`, rasterize it into a
//!   `tiny-skia` pixmap with `resvg`, PNG-encode. One file per picked slide.
//! - **PDF** — one `krilla` page per picked slide, sized in points from the
//!   slide's viewBox, drawn via `krilla-svg` with selectable, subset-embedded
//!   text (shadows/filters rasterize at [`PDF_FILTER_SCALE`]).
//!
//! Both share the same `usvg` parse (the versions are pinned in lockstep) and
//! the same injected [`fontdb::Database`], so a caller resolves fonts once and
//! gets identical glyph substitution in either format. Per-slide failures are
//! collected as [`ExportReport::warnings`] rather than aborting the whole run,
//! so one unreadable deck never sinks an otherwise-good export.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use krilla::geom::Size as KrillaSize;
use krilla::metadata::Metadata;
use krilla::page::PageSettings;
use krilla::Document;
use krilla_svg::{SurfaceExt, SvgSettings};

use crate::error::{Error, Result};
use crate::model::{ExportReport, SlidePick};
use crate::pptx::PresentationFile;
use crate::render::{render_slide_svg, RenderOptions};

/// Re-exported so callers (and tests) can build/inspect the font database the
/// export API takes without depending on the exact `fontdb` version directly.
pub use fontdb;

/// PNG export options.
#[derive(Debug, Clone)]
pub struct PngOptions {
    /// Output width in pixels; height follows the slide's aspect ratio. The UI
    /// offers 1280 / 1920 / 3840 presets, but any positive width is valid.
    pub target_width_px: u32,
}

impl Default for PngOptions {
    fn default() -> Self {
        PngOptions { target_width_px: 1920 }
    }
}

/// PDF export options.
#[derive(Debug, Clone, Default)]
pub struct PdfOptions {
    /// Document title written to the PDF metadata (Info dict + XMP). `None`
    /// leaves the PDF untitled.
    pub title: Option<String>,
}

/// How much to scale rasterized SVG filters (drop shadows, blurs) when drawing
/// into the PDF. 2.0 keeps shadow edges crisp without ballooning the file.
const PDF_FILTER_SCALE: f32 = 2.0;

/// Longer-edge cap (px) for raster images embedded into the PDF's per-slide
/// SVG — crisp on screen and in print without embedding multi-MB originals.
const PDF_MAX_IMAGE_PX: u32 = 2400;

/// System-font database, loaded once and cached for the process lifetime.
/// Loading every installed face is slow, so the app resolves it a single time
/// and passes `&db` to each export call.
///
/// Tests never touch this — they inject a database holding only the bundled
/// fixture font (see `tests/export.rs`) so rendering is deterministic and CI
/// needs no installed fonts.
static SYSTEM_FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();

/// The cached system-font database: every installed face plus generic-family
/// mappings so the renderer's `… , Helvetica, Arial, sans-serif` fallbacks
/// resolve to something real.
pub fn system_fonts() -> Arc<fontdb::Database> {
    SYSTEM_FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            // Generic-family fallbacks. The render SVG always ends its
            // font-family lists with a CSS generic, so these are the last
            // resort when neither the authored font nor Helvetica/Arial exist.
            db.set_sans_serif_family("Helvetica");
            db.set_serif_family("Times New Roman");
            db.set_monospace_family("Courier New");
            db.set_cursive_family("Apple Chancery");
            db.set_fantasy_family("Papyrus");
            Arc::new(db)
        })
        .clone()
}

// ---------------------------------------------------------------------------
// SVG → raster / PDF plumbing
// ---------------------------------------------------------------------------

/// Render one slide of an already-opened deck to an SVG string, with images
/// embedded and capped at `max_image_px` on the longer edge.
fn slide_svg(pf: &PresentationFile, slide_index: usize, max_image_px: u32) -> Result<String> {
    let opts = RenderOptions { embed_images: true, max_image_px: Some(max_image_px) };
    render_slide_svg(pf, slide_index, &opts)
}

/// Build `usvg` parse options bound to the caller's font database.
fn usvg_options(db: &Arc<fontdb::Database>) -> usvg::Options<'static> {
    usvg::Options { fontdb: db.clone(), ..Default::default() }
}

/// Parse a render-SVG string into a `usvg` tree using `db` for font resolution.
fn parse_svg(svg: &str, db: &Arc<fontdb::Database>) -> Result<usvg::Tree> {
    usvg::Tree::from_str(svg, &usvg_options(db))
        .map_err(|e| Error::Render(format!("export: SVG parse failed: {e}")))
}

/// Rasterize a parsed slide tree to PNG bytes at `target_width_px` (height
/// derived from the tree's aspect ratio).
fn tree_to_png(tree: &usvg::Tree, target_width_px: u32) -> Result<Vec<u8>> {
    let size = tree.size();
    let w_pt = size.width();
    let h_pt = size.height();
    if !(w_pt > 0.0 && h_pt > 0.0) {
        return Err(Error::Render("export: slide has zero-sized viewBox".into()));
    }
    let width_px = target_width_px.max(1);
    let scale = width_px as f32 / w_pt;
    let height_px = ((h_pt * scale).round() as u32).max(1);

    let mut pixmap = tiny_skia::Pixmap::new(width_px, height_px)
        .ok_or_else(|| Error::Render(format!("export: cannot allocate {width_px}×{height_px} pixmap")))?;
    resvg::render(tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap
        .encode_png()
        .map_err(|e| Error::Render(format!("export: PNG encode failed: {e}")))
}

/// Render one slide of an opened deck straight to PNG bytes. Shared by
/// [`export_pngs`] and [`render_slide_png`] (and, later, the drag-out feature).
fn pf_slide_png(
    pf: &PresentationFile,
    slide_index: usize,
    width_px: u32,
    db: &Arc<fontdb::Database>,
) -> Result<Vec<u8>> {
    // Embed images at ~2× the output width so downscaling to the target stays
    // crisp; anything larger is wasted bytes through the rasterizer.
    let svg = slide_svg(pf, slide_index, width_px.saturating_mul(2).max(1))?;
    let tree = parse_svg(&svg, db)?;
    tree_to_png(&tree, width_px)
}

/// Render a single slide (1-based) of a deck on disk to PNG bytes.
///
/// Opens the deck fresh each call — for multi-slide exports prefer
/// [`export_pngs`], which opens each source deck at most once.
pub fn render_slide_png(
    pptx: &Path,
    slide_index: usize,
    width_px: u32,
    fonts: &fontdb::Database,
) -> Result<Vec<u8>> {
    let pf = PresentationFile::open(pptx)?;
    let db = Arc::new(fonts.clone());
    pf_slide_png(&pf, slide_index, width_px, &db)
}

// ---------------------------------------------------------------------------
// Batch exports
// ---------------------------------------------------------------------------

/// A per-run cache that opens each distinct source deck at most once. A failed
/// open is remembered as `None` so repeated picks from a missing deck don't
/// each retry (and warn) again.
struct DeckCache {
    decks: HashMap<String, Option<Arc<PresentationFile>>>,
}

impl DeckCache {
    fn new() -> Self {
        DeckCache { decks: HashMap::new() }
    }

    /// Get (opening on first use) the deck at `path`. Returns `None` if it has
    /// ever failed to open; `newly_failed` is set true only on the open that
    /// first discovered the failure, so the caller warns exactly once per deck.
    fn get(&mut self, path: &str) -> (Option<Arc<PresentationFile>>, bool) {
        if let Some(entry) = self.decks.get(path) {
            return (entry.clone(), false);
        }
        match PresentationFile::open(Path::new(path)) {
            Ok(pf) => {
                let pf = Arc::new(pf);
                self.decks.insert(path.to_string(), Some(pf.clone()));
                (Some(pf), false)
            }
            Err(_) => {
                self.decks.insert(path.to_string(), None);
                (None, true)
            }
        }
    }
}

/// Export each picked slide to its own PNG in `out_dir`.
///
/// File names are `NNN — <deck stem> — slide <original index>.png` (deck file
/// name, never docProps title — a project convention), sanitized and
/// de-duplicated. `progress(done, total)` fires after every pick is processed
/// (success or skip), so a UI bar always reaches 100 %.
pub fn export_pngs(
    picks: &[SlidePick],
    out_dir: &Path,
    opts: &PngOptions,
    fonts: &fontdb::Database,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<ExportReport> {
    if picks.is_empty() {
        return Err(Error::Compose("no slides picked".into()));
    }
    std::fs::create_dir_all(out_dir).map_err(|e| Error::io(out_dir, e))?;

    let db = Arc::new(fonts.clone());
    let width_px = opts.target_width_px.max(1);
    let total = picks.len();
    let mut cache = DeckCache::new();
    let mut report = ExportReport { files_written: Vec::new(), warnings: Vec::new() };
    let mut used_names: Vec<String> = Vec::new();

    for (i, pick) in picks.iter().enumerate() {
        let (pf, newly_failed) = cache.get(&pick.pptx_path);
        match pf {
            None => {
                if newly_failed {
                    report.warnings.push(format!("Could not open {}", pick.pptx_path));
                }
            }
            Some(pf) => match pf_slide_png(&pf, pick.slide_index, width_px, &db) {
                Ok(bytes) => {
                    let name = unique_png_name(&mut used_names, i + 1, &pick.pptx_path, pick.slide_index);
                    let path = out_dir.join(&name);
                    match std::fs::write(&path, &bytes) {
                        Ok(()) => report.files_written.push(path),
                        Err(e) => report
                            .warnings
                            .push(format!("Could not write {}: {e}", path.display())),
                    }
                }
                Err(e) => report.warnings.push(format!(
                    "{} slide {}: {e}",
                    deck_stem(&pick.pptx_path),
                    pick.slide_index
                )),
            },
        }
        progress(i + 1, total);
    }

    Ok(report)
}

/// Export all picked slides into a single PDF at `out_path`, one page per slide
/// (sized in points from each slide's viewBox), with selectable embedded text.
/// A slide whose deck can't be opened, or that fails to render, is skipped with
/// a warning; the remaining slides still make it into the PDF.
pub fn export_pdf(
    picks: &[SlidePick],
    out_path: &Path,
    opts: &PdfOptions,
    fonts: &fontdb::Database,
    progress: &mut dyn FnMut(usize, usize),
) -> Result<ExportReport> {
    if picks.is_empty() {
        return Err(Error::Compose("no slides picked".into()));
    }

    let db = Arc::new(fonts.clone());
    let total = picks.len();
    let mut cache = DeckCache::new();
    let mut report = ExportReport { files_written: Vec::new(), warnings: Vec::new() };

    let mut document = Document::new();
    if let Some(title) = opts.title.as_deref().filter(|t| !t.is_empty()) {
        document.set_metadata(Metadata::new().title(title.to_string()));
    }

    let mut pages_written = 0usize;
    for (i, pick) in picks.iter().enumerate() {
        let (pf, newly_failed) = cache.get(&pick.pptx_path);
        match pf {
            None => {
                if newly_failed {
                    report.warnings.push(format!("Could not open {}", pick.pptx_path));
                }
            }
            Some(pf) => match render_pdf_page(&mut document, &pf, pick.slide_index, &db) {
                Ok(()) => pages_written += 1,
                Err(e) => report.warnings.push(format!(
                    "{} slide {}: {e}",
                    deck_stem(&pick.pptx_path),
                    pick.slide_index
                )),
            },
        }
        progress(i + 1, total);
    }

    if pages_written == 0 {
        return Err(Error::Compose(
            "no slides could be exported to PDF (all picks failed to render)".into(),
        ));
    }

    let bytes = document
        .finish()
        .map_err(|e| Error::Render(format!("export: PDF assembly failed: {e}")))?;
    std::fs::write(out_path, &bytes).map_err(|e| Error::io(out_path, e))?;
    report.files_written.push(out_path.to_path_buf());
    Ok(report)
}

/// Render one slide as a fresh page appended to `document`.
fn render_pdf_page(
    document: &mut Document,
    pf: &PresentationFile,
    slide_index: usize,
    db: &Arc<fontdb::Database>,
) -> Result<()> {
    let svg = slide_svg(pf, slide_index, PDF_MAX_IMAGE_PX)?;
    let tree = parse_svg(&svg, db)?;
    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    let page_settings = PageSettings::from_wh(w, h)
        .ok_or_else(|| Error::Render(format!("export: invalid page size {w}×{h}pt")))?;
    let krilla_size = KrillaSize::from_wh(w, h)
        .ok_or_else(|| Error::Render(format!("export: invalid draw size {w}×{h}pt")))?;

    let mut page = document.start_page_with(page_settings);
    let mut surface = page.surface();
    let settings = SvgSettings { embed_text: true, filter_scale: PDF_FILTER_SCALE };
    surface.draw_svg(&tree, krilla_size, settings);
    surface.finish();
    page.finish();
    Ok(())
}

// ---------------------------------------------------------------------------
// PNG file naming
// ---------------------------------------------------------------------------

/// Max length of a PNG file name's stem (before `.png`), in chars, so long deck
/// names don't produce filesystem-hostile paths.
const MAX_NAME_STEM: usize = 120;

/// The deck's file stem (name without extension) for use in messages/names.
fn deck_stem(pptx_path: &str) -> String {
    Path::new(pptx_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "deck".to_string())
}

/// Replace path separators / colons and drop control characters, so a stem is
/// safe as a single path component on macOS/Windows/Linux.
fn sanitize(s: &str) -> String {
    let cleaned: String = s
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' => '-',
            c if c.is_control() => ' ',
            c => c,
        })
        .collect();
    cleaned.trim().to_string()
}

/// Build the `NNN — <stem> — slide <idx>.png` name for one pick, truncating the
/// deck stem so the whole stem stays within [`MAX_NAME_STEM`] chars.
fn png_name(seq: usize, pptx_path: &str, slide_index: usize) -> String {
    let prefix = format!("{seq:03} — ");
    let suffix = format!(" — slide {slide_index}");
    let stem = sanitize(&deck_stem(pptx_path));
    let budget = MAX_NAME_STEM.saturating_sub(prefix.chars().count() + suffix.chars().count());
    let stem: String = stem.chars().take(budget.max(1)).collect();
    format!("{prefix}{stem}{suffix}.png")
}

/// Like [`png_name`] but guarantees uniqueness within one export by appending
/// ` (2)`, ` (3)`, … before the extension on collision.
fn unique_png_name(used: &mut Vec<String>, seq: usize, pptx_path: &str, slide_index: usize) -> String {
    let base = png_name(seq, pptx_path, slide_index);
    if !used.iter().any(|n| n == &base) {
        used.push(base.clone());
        return base;
    }
    let (stem, ext) = base
        .rsplit_once(".png")
        .map(|(s, _)| (s.to_string(), ".png"))
        .unwrap_or((base.clone(), ""));
    let mut n = 2;
    loop {
        let candidate = format!("{stem} ({n}){ext}");
        if !used.iter().any(|c| c == &candidate) {
            used.push(candidate.clone());
            return candidate;
        }
        n += 1;
    }
}
