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
            // Bundle the metric-compatible substitutes (Carlito↔Calibri,
            // Caladea↔Cambria) so an unembedded-Calibri deck rasterizes with
            // Carlito instead of falling through to Helvetica. A real installed
            // Calibri, if present, still wins — it comes first in the chain.
            crate::fonts::register_bundled_fonts(&mut db);
            set_generic_families(&mut db);
            Arc::new(db)
        })
        .clone()
}

/// Whether any loaded face carries this family name (case-insensitive).
fn family_exists(db: &fontdb::Database, name: &str) -> bool {
    db.faces()
        .any(|f| f.families.iter().any(|(fam, _)| fam.eq_ignore_ascii_case(name)))
}

/// First candidate family that actually exists in `db`, if any.
fn first_present<'a>(db: &fontdb::Database, candidates: &[&'a str]) -> Option<&'a str> {
    candidates.iter().copied().find(|c| family_exists(db, c))
}

/// Map the CSS generic families to fonts that **actually exist** in `db`.
///
/// The render SVG always ends its `font-family` lists with a CSS generic, so
/// these mappings are the last resort when neither the authored font nor the
/// Helvetica/Arial fallbacks are installed. Pointing a generic at a nonexistent
/// family (e.g. "Helvetica" on a stock Linux box) makes that text silently
/// vanish from PDF/PNG exports — so each generic probes a cross-platform
/// candidate list (macOS → Windows → Linux staples) and takes the first hit.
/// serif/monospace/cursive/fantasy fall back to the chosen sans-serif rather
/// than dangling; a database with no candidates at all is left untouched.
pub fn set_generic_families(db: &mut fontdb::Database) {
    let sans = first_present(
        db,
        &["Helvetica Neue", "Helvetica", "Arial", "Liberation Sans", "DejaVu Sans", "Noto Sans"],
    );
    let serif = first_present(
        db,
        &["Times New Roman", "Times", "Liberation Serif", "DejaVu Serif", "Noto Serif"],
    )
    .or(sans);
    let mono = first_present(
        db,
        &["Menlo", "Courier New", "Liberation Mono", "DejaVu Sans Mono", "Noto Sans Mono"],
    )
    .or(sans);
    let cursive =
        first_present(db, &["Apple Chancery", "Comic Sans MS", "Segoe Script"]).or(sans);
    let fantasy = first_present(db, &["Papyrus", "Impact"]).or(sans);

    if let Some(f) = sans {
        db.set_sans_serif_family(f);
    }
    if let Some(f) = serif {
        db.set_serif_family(f);
    }
    if let Some(f) = mono {
        db.set_monospace_family(f);
    }
    if let Some(f) = cursive {
        db.set_cursive_family(f);
    }
    if let Some(f) = fantasy {
        db.set_fantasy_family(f);
    }
}

// ---------------------------------------------------------------------------
// SVG → raster / PDF plumbing
// ---------------------------------------------------------------------------

/// Render one slide of an already-opened deck to an SVG string, with images
/// embedded and capped at `max_image_px` on the longer edge.
fn slide_svg(pf: &PresentationFile, slide_index: usize, max_image_px: u32) -> Result<String> {
    // No `@font-face` substitutes baked into the export SVG: the exporter
    // carries the bundled Carlito/Caladea bytes fontdb-side instead (see
    // `system_fonts` / `deck_fonts`), which usvg *does* honor — unlike SVG
    // `@font-face`, which it ignores.
    let opts = RenderOptions {
        embed_images: true,
        max_image_px: Some(max_image_px),
        embed_substitute_fonts: false,
    };
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
/// [`export_pngs`], which opens each source deck at most once. A caller that
/// already holds an opened [`PresentationFile`] should use
/// [`render_slide_png_from`] to avoid reopening it.
pub fn render_slide_png(
    pptx: &Path,
    slide_index: usize,
    width_px: u32,
    fonts: &fontdb::Database,
) -> Result<Vec<u8>> {
    let pf = PresentationFile::open(pptx)?;
    render_slide_png_from(&pf, slide_index, width_px, fonts)
}

/// Render one slide (1-based) of an ALREADY-OPENED deck to PNG bytes, enriching
/// `fonts` with the deck's embedded fonts (usvg ignores SVG `@font-face`, so the
/// rasterizer needs them fontdb-side). Thin public wrapper over the internal
/// `pf_slide_png` + `deck_fonts`, so a caller holding a [`PresentationFile`] (the
/// drag-out icon path) renders without a second open/parse.
pub fn render_slide_png_from(
    pf: &PresentationFile,
    slide_index: usize,
    width_px: u32,
    fonts: &fontdb::Database,
) -> Result<Vec<u8>> {
    let base = Arc::new(fonts.clone());
    let db = deck_fonts(&base, pf);
    pf_slide_png(pf, slide_index, width_px, &db)
}

// ---------------------------------------------------------------------------
// Batch exports
// ---------------------------------------------------------------------------

/// The caller's base database, enriched with a deck's embedded fonts when it
/// has any (usvg ignores SVG `@font-face`, so the rasterizer needs the same
/// bytes fontdb-side to draw the deck's real typefaces). Decks without
/// embedded fonts share the base `Arc` untouched — the common case costs one
/// pointer clone.
fn deck_fonts(base: &Arc<fontdb::Database>, pf: &PresentationFile) -> Arc<fontdb::Database> {
    let fonts = &pf.embedded_font_set().fonts;
    if fonts.is_empty() {
        return base.clone();
    }
    let mut db = (**base).clone();
    for f in fonts {
        db.load_font_data(f.bytes.clone());
    }
    Arc::new(db)
}

/// An opened source deck paired with the font database to rasterize it with
/// (the base database, enriched with the deck's embedded fonts when present).
type CachedDeck = (Arc<PresentationFile>, Arc<fontdb::Database>);

/// A per-run cache that opens each distinct source deck at most once, pairing
/// it with its (possibly embedded-font-enriched) font database. A failed open
/// is remembered as `None` so repeated picks from a missing deck don't each
/// retry (and warn) again.
struct DeckCache {
    base_fonts: Arc<fontdb::Database>,
    decks: HashMap<String, Option<CachedDeck>>,
}

impl DeckCache {
    fn new(base_fonts: Arc<fontdb::Database>) -> Self {
        DeckCache { base_fonts, decks: HashMap::new() }
    }

    /// Get (opening on first use) the deck at `path` plus its font database.
    /// Returns `None` if it has ever failed to open; `newly_failed` is set true
    /// only on the open that first discovered the failure, so the caller warns
    /// exactly once per deck.
    fn get(&mut self, path: &str) -> (Option<CachedDeck>, bool) {
        if let Some(entry) = self.decks.get(path) {
            return (entry.clone(), false);
        }
        match PresentationFile::open(Path::new(path)) {
            Ok(pf) => {
                let pf = Arc::new(pf);
                let db = deck_fonts(&self.base_fonts, &pf);
                let entry = (pf, db);
                self.decks.insert(path.to_string(), Some(entry.clone()));
                (Some(entry), false)
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
    let mut cache = DeckCache::new(db);
    let mut report = ExportReport { files_written: Vec::new(), warnings: Vec::new() };
    let mut used_names: Vec<String> = Vec::new();

    for (i, pick) in picks.iter().enumerate() {
        let (entry, newly_failed) = cache.get(&pick.pptx_path);
        match entry {
            None => {
                if newly_failed {
                    report.warnings.push(format!("Could not open {}", pick.pptx_path));
                }
            }
            Some((pf, deck_db)) => match pf_slide_png(&pf, pick.slide_index, width_px, &deck_db) {
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
    let mut cache = DeckCache::new(db);
    let mut report = ExportReport { files_written: Vec::new(), warnings: Vec::new() };

    let mut document = Document::new();
    if let Some(title) = opts.title.as_deref().filter(|t| !t.is_empty()) {
        document.set_metadata(Metadata::new().title(title.to_string()));
    }

    let mut pages_written = 0usize;
    for (i, pick) in picks.iter().enumerate() {
        let (entry, newly_failed) = cache.get(&pick.pptx_path);
        match entry {
            None => {
                if newly_failed {
                    report.warnings.push(format!("Could not open {}", pick.pptx_path));
                }
            }
            Some((pf, deck_db)) => match render_pdf_page(&mut document, &pf, pick.slide_index, &deck_db)
            {
                Ok(dropped) => {
                    pages_written += 1;
                    if dropped > 0 {
                        report.warnings.push(format!(
                            "{} slide {}: {dropped} image(s) could not be embedded and were left blank",
                            deck_stem(&pick.pptx_path),
                            pick.slide_index
                        ));
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

/// Render one slide as a fresh page appended to `document`. Returns how many
/// embedded rasters were neutralized (see [`sanitize_pdf_images`]) so the caller
/// can surface a warning.
fn render_pdf_page(
    document: &mut Document,
    pf: &PresentationFile,
    slide_index: usize,
    db: &Arc<fontdb::Database>,
) -> Result<usize> {
    let svg = slide_svg(pf, slide_index, PDF_MAX_IMAGE_PX)?;
    // krilla decodes embedded rasters lazily at document.finish(), so a single
    // corrupt image would abort the WHOLE PDF rather than one page. Neutralize
    // any raster the strict decoder can't read *before* handing it to krilla.
    let (svg, dropped) = sanitize_pdf_images(&svg);
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
    Ok(dropped)
}

/// A known-good 1×1 fully-transparent PNG as a data URI, built once. Used to
/// replace embedded rasters krilla can't decode.
fn transparent_pixel_uri() -> &'static str {
    static URI: OnceLock<String> = OnceLock::new();
    URI.get_or_init(|| {
        use base64::Engine as _;
        let img = image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 0, 0, 0]));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .expect("encode 1×1 transparent PNG");
        format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(&buf))
    })
}

/// Whether a `data:image/<type>;base64,…` URI holds a raster the strict PNG/JPEG
/// decoder (the same strictness krilla uses) cannot read. Vector `svg+xml` URIs
/// never hit that decoder, so they're always considered fine here.
fn is_undecodable_raster(uri: &str) -> bool {
    use base64::Engine as _;
    let Some(rest) = uri.strip_prefix("data:image/") else { return false };
    let Some((subtype, b64)) = rest.split_once(";base64,") else { return false };
    if subtype.eq_ignore_ascii_case("svg+xml") {
        return false;
    }
    match base64::engine::general_purpose::STANDARD.decode(b64) {
        Ok(bytes) => image::load_from_memory(&bytes).is_err(),
        Err(_) => true,
    }
}

/// Replace every embedded raster the strict decoder rejects with a transparent
/// pixel, returning the rewritten SVG and the count neutralized. Vector images
/// and decodable rasters pass through untouched.
fn sanitize_pdf_images(svg: &str) -> (String, usize) {
    const OPEN: &str = "href=\"";
    if !svg.contains("href=\"data:image/") {
        return (svg.to_string(), 0);
    }
    let mut out = String::with_capacity(svg.len());
    let mut rest = svg;
    let mut dropped = 0usize;
    while let Some(pos) = rest.find("href=\"data:image/") {
        let val_start = pos + OPEN.len();
        out.push_str(&rest[..val_start]);
        let after = &rest[val_start..];
        let Some(qend) = after.find('"') else {
            out.push_str(after);
            return (out, dropped);
        };
        let uri = &after[..qend];
        if is_undecodable_raster(uri) {
            dropped += 1;
            out.push_str(transparent_pixel_uri());
        } else {
            out.push_str(uri);
        }
        rest = &after[qend..];
    }
    out.push_str(rest);
    (out, dropped)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A stock Linux box has neither Helvetica nor Arial, so the generic
    /// mappings must probe what is actually loaded instead of hardcoding
    /// macOS names — otherwise generic-fallback text silently vanishes from
    /// exports. With a database holding ONLY the bundled DejaVu Sans, every
    /// generic (sans-serif directly; the rest via the sans fallback) must
    /// resolve to that face.
    #[test]
    fn generic_families_probe_what_is_actually_loaded() {
        static FONT: &[u8] = include_bytes!("../fixtures/fonts/DejaVuSans.ttf");
        let mut db = fontdb::Database::new();
        db.load_font_data(FONT.to_vec());
        set_generic_families(&mut db);

        let id = db
            .query(&fontdb::Query {
                families: &[fontdb::Family::SansSerif],
                ..fontdb::Query::default()
            })
            .expect("sans-serif generic resolves in a DejaVu-only database");
        let face = db.face(id).expect("queried face exists");
        assert!(
            face.families.iter().any(|(f, _)| f == "DejaVu Sans"),
            "sans-serif resolved to {:?}, expected DejaVu Sans",
            face.families
        );

        for fam in [
            fontdb::Family::Serif,
            fontdb::Family::Monospace,
            fontdb::Family::Cursive,
            fontdb::Family::Fantasy,
        ] {
            let got = db.query(&fontdb::Query { families: &[fam], ..fontdb::Query::default() });
            assert_eq!(got, Some(id), "{fam:?} falls back to the sans-serif choice");
        }
    }

    /// An empty database must not panic and must leave the defaults untouched
    /// (there is simply nothing to map).
    #[test]
    fn generic_families_probe_tolerates_empty_database() {
        let mut db = fontdb::Database::new();
        set_generic_families(&mut db);
        assert!(db.is_empty());
    }

    #[test]
    fn deck_fonts_enriches_only_embedding_decks() {
        use crate::fixtures::{sample_ttf, DeckSpec, SlideSpec};
        use crate::pptx::PresentationFile;

        let base = Arc::new(fontdb::Database::new());

        let plain = DeckSpec::new("Plain").slide(SlideSpec::new("Hi")).build();
        let plain_pf = PresentationFile::from_bytes(&plain).unwrap();
        assert!(
            Arc::ptr_eq(&deck_fonts(&base, &plain_pf), &base),
            "a deck without embedded fonts must share the base database"
        );

        let embedding = DeckSpec::new("Embedded")
            .font("Grafton")
            .embed_font("Grafton", vec![(false, false, sample_ttf())])
            .slide(SlideSpec::new("Hi"))
            .build();
        let pf = PresentationFile::from_bytes(&embedding).unwrap();
        let enriched = deck_fonts(&base, &pf);
        assert!(
            !Arc::ptr_eq(&enriched, &base),
            "an embedding deck must get its own enriched database"
        );
        assert!(enriched.len() >= base.len(), "faces never shrink");
    }
}
