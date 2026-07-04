//! Slide → SVG preview renderer. No LibreOffice, no PowerPoint.
//!
//! Goal: **recognizable, attractive previews** for browsing and search — not
//! pixel-perfect fidelity. A user must be able to tell slides apart at
//! thumbnail size and read the text at inspector size.
//!
//! CONTRACT for the module owner (`render_slide_svg`):
//! - Output a self-contained `<svg>` string, `viewBox="0 0 W H"` where W/H are
//!   the slide size in points (EMU / 12700), plus `width`/`height` attributes.
//! - Resolve theme colors through the master's `p:clrMap` and the theme's
//!   `a:clrScheme` (`schemeClr` values incl. bg1/tx1/accentN mapping, and
//!   `lumMod`/`lumOff`/`shade`/`tint`/`alpha` transforms at least
//!   approximately). `srgbClr` and `sysClr` (use `lastClr`) must work.
//! - Draw, in z-order: slide background (slide's own `p:bg`, else layout's,
//!   else master's, else white) → layout/master placeholder *decor is NOT
//!   required* → the slide's own shapes:
//!   - `p:sp` with `a:prstGeom` rect/roundRect/ellipse (others: fall back to
//!     rect) — fill (`solidFill`/`gradFill` first stop/`noFill`) and outline.
//!   - `p:pic` — embed the image bytes as a base64 data URI (`image/png`,
//!     `image/jpeg`, `image/gif`; skip others gracefully). Respect `a:xfrm`
//!     including `rot` (rotation in 60000ths of a degree) and flipH/flipV.
//!   - `p:sp` text bodies — paragraphs with runs; approximate font size
//!     (`sz` in hundredths of a point, default 1800), bold/italic, run color,
//!     alignment (`algn`), bullets as "• " prefix for body placeholders.
//!     Use theme major font for titles, minor for everything else, with
//!     `font-family="X, Helvetica, Arial, sans-serif"` fallbacks. Wrap text
//!     to the shape width by estimating ~0.5em average glyph width; clip with
//!     an SVG clipPath sized to the shape.
//! - Placeholder inheritance: when a slide shape is a placeholder (`p:ph`)
//!   with no own `a:xfrm`, inherit position/size from the layout's matching
//!   placeholder (match by `type`+`idx`, then by `idx`, then by `type`),
//!   falling back to the master's. Same inheritance for missing text style is
//!   NOT required beyond default sizes (title 4400, body 1800).
//! - Group shapes (`p:grpSp`): apply the group transform (`chOff`/`chExt`
//!   scaling) recursively.
//! - Never panic on unknown content: skip what you can't draw. Return
//!   `Error::Render` only for structurally broken slides.
//! - Escape all text. The SVG is injected into the app's webview via
//!   `<img src=data:>` — it must not contain scripts or external references.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use crate::error::{Error, Result};
use crate::pptx::PresentationFile;

mod color;
mod fill;
mod geometry;
mod image;
mod placeholder;
mod text;

use color::Theme;
use fill::{collect_background, Fill};
use geometry::{parse_xfrm, Transform, Xfrm};
use placeholder::{collect_placeholders, match_placeholder, shape_placeholder, Placeholder};

/// Version of the SVG output format. Any change that alters the bytes produced
/// for a given slide (renderer fidelity, image encoding, …) between *released*
/// builds should bump this: it is baked into the thumbnail cache key
/// ([`crate::thumbs::thumb_file_name`]) so stale caches invalidate automatically
/// on upgrade, with no eviction bookkeeping.
pub const RENDER_VERSION: u32 = 1;

const EMU_PER_PT: f64 = 12700.0;
// Default text insets (bodyPr lIns/tIns/rIns/bIns) in points.
const L_INS: f64 = 7.2;
const R_INS: f64 = 7.2;
const T_INS: f64 = 3.6;
// Per indent-level extra left padding, in points.
const LVL_INDENT: f64 = 24.0;

#[derive(Debug, Clone)]
pub struct RenderOptions {
    /// Embed raster images as data URIs (true) or draw gray placeholders
    /// with a photo glyph (false — faster, for tiny grid thumbnails).
    pub embed_images: bool,
    /// Cap the longer edge of embedded raster images to this many pixels,
    /// downscaling anything larger before base64-encoding it. `None` embeds
    /// images at full resolution. Vector (SVG) images are never affected.
    /// This is the main lever keeping grid-thumbnail SVGs small — a full-res
    /// photo embedded at ~200px display size is otherwise multiple MB.
    pub max_image_px: Option<u32>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions { embed_images: true, max_image_px: None }
    }
}

impl RenderOptions {
    /// Small grid thumbnail: images downscaled hard.
    pub fn thumb() -> Self {
        RenderOptions { embed_images: true, max_image_px: Some(512) }
    }

    /// Larger preview for the peek modal / inspector: crisp but still bounded
    /// (export fidelity is unaffected — the composer copies original parts,
    /// never these renders).
    pub fn preview() -> Self {
        RenderOptions { embed_images: true, max_image_px: Some(1600) }
    }
}

/// Render one slide (1-based index) of an opened presentation to an SVG string.
pub fn render_slide_svg(
    pf: &PresentationFile,
    slide_index: usize,
    options: &RenderOptions,
) -> Result<String> {
    let slide_part = pf.slide_part(slide_index)?.to_string();
    let slide_bytes = pf.package.require_part(&slide_part)?;
    let slide_xml = std::str::from_utf8(slide_bytes)
        .map_err(|e| Error::Render(format!("slide {slide_index} is not valid UTF-8: {e}")))?;
    let slide_doc = Document::parse(slide_xml)
        .map_err(|e| Error::Render(format!("slide {slide_index} XML: {e}")))?;

    // Resolve the layout → master → theme chain (best-effort; missing parts
    // just fall back to defaults rather than erroring).
    let layout_part = pf.layout_of_slide(&slide_part).ok().flatten();
    let master_part = layout_part
        .as_deref()
        .and_then(|l| pf.master_of_layout(l).ok().flatten());
    let theme_part = master_part
        .as_deref()
        .and_then(|m| pf.theme_of_master(m).ok().flatten());

    let master_xml = owned_part(pf, master_part.as_deref());
    let layout_xml = owned_part(pf, layout_part.as_deref());
    let theme_xml = owned_part(pf, theme_part.as_deref());

    let mut theme = Theme::default();
    if let Some(x) = &theme_xml {
        if let Ok(doc) = Document::parse(x) {
            theme.load_theme(&doc);
        }
    }
    if let Some(x) = &master_xml {
        if let Ok(doc) = Document::parse(x) {
            theme.load_clr_map(&doc);
        }
    }

    // Placeholder geometry inheritance sources.
    let mut layout_phs = Vec::new();
    let mut layout_bg: Option<Fill> = None;
    if let Some(x) = &layout_xml {
        if let Ok(doc) = Document::parse(x) {
            layout_phs = collect_placeholders(&doc, &theme);
            layout_bg = collect_background(&doc, &theme);
        }
    }
    let mut master_phs = Vec::new();
    let mut master_bg: Option<Fill> = None;
    if let Some(x) = &master_xml {
        if let Ok(doc) = Document::parse(x) {
            master_phs = collect_placeholders(&doc, &theme);
            master_bg = collect_background(&doc, &theme);
        }
    }

    let w_pt = pf.slide_width_emu as f64 / EMU_PER_PT;
    let h_pt = pf.slide_height_emu as f64 / EMU_PER_PT;

    let slide_rels = pf.package.rels_for(&slide_part).unwrap_or_default();
    let content_types = pf.package.content_types().ok();

    let mut ctx = Ctx {
        pf,
        options,
        theme,
        slide_part: slide_part.clone(),
        slide_rels,
        content_types,
        layout_phs,
        master_phs,
        defs: String::new(),
        body: String::new(),
        clip_id: 0,
        grad_id: 0,
        grad_cache: HashMap::new(),
    };

    // Background: slide's own, else layout, else master, else white.
    let slide_bg = collect_background(&slide_doc, &ctx.theme);
    let bg = slide_bg.or(layout_bg).or(master_bg);
    let bg_fill = match &bg {
        Some(f @ Fill::Gradient { .. }) => ctx.fill_attrs(f),
        Some(Fill::Solid(c)) => format!(r#" fill="{}""#, c.hex()),
        _ => r##" fill="#FFFFFF""##.to_string(),
    };
    ctx.body.push_str(&format!(
        r#"<rect x="0" y="0" width="{w}" height="{h}"{bg}/>"#,
        w = fnum(w_pt),
        h = fnum(h_pt),
        bg = bg_fill
    ));

    // Slide shapes, in document order.
    let root = slide_doc.root_element();
    let sp_tree = ch(root, "cSld").and_then(|c| ch(c, "spTree"));
    if let Some(tree) = sp_tree {
        let base = Transform::identity();
        for shape in tree.children().filter(|n| n.is_element()) {
            ctx.render_shape(shape, base);
        }
    }

    Ok(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}"><defs>{defs}</defs>{body}</svg>"#,
        w = fnum(w_pt),
        h = fnum(h_pt),
        defs = ctx.defs,
        body = ctx.body
    ))
}

/// A neutral gray placeholder SVG used by the UI while a thumbnail hydrates.
pub fn svg_placeholder(width_pt: f64, height_pt: f64, label: &str) -> String {
    let w = if width_pt > 0.0 { width_pt } else { 960.0 };
    let h = if height_pt > 0.0 { height_pt } else { 540.0 };
    let fs = (h * 0.06).clamp(8.0, 48.0);
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {w} {h}" width="{w}" height="{h}"><rect x="0" y="0" width="{w}" height="{h}" fill="#E5E7EB"/><text x="{cx}" y="{cy}" font-family="Helvetica, Arial, sans-serif" font-size="{fs}" fill="#9CA3AF" text-anchor="middle">{label}</text></svg>"##,
        w = fnum(w),
        h = fnum(h),
        cx = fnum(w / 2.0),
        cy = fnum(h / 2.0 + fs * 0.35),
        fs = fnum(fs),
        label = esc(label)
    )
}

fn owned_part(pf: &PresentationFile, part: Option<&str>) -> Option<String> {
    let part = part?;
    let bytes = pf.package.part(part)?;
    std::str::from_utf8(bytes).ok().map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Rendering context
// ---------------------------------------------------------------------------

struct Ctx<'a> {
    pf: &'a PresentationFile,
    options: &'a RenderOptions,
    theme: Theme,
    slide_part: String,
    slide_rels: Vec<crate::opc::Relationship>,
    content_types: Option<crate::opc::ContentTypes>,
    layout_phs: Vec<Placeholder>,
    master_phs: Vec<Placeholder>,
    defs: String,
    body: String,
    clip_id: usize,
    /// Monotonic id for interned `<defs>` gradients (`grad0`, `grad1`, …).
    grad_id: usize,
    /// Serialized-gradient → def-id, so repeated gradients emit one `<defs>`.
    grad_cache: HashMap<String, String>,
}

impl Ctx<'_> {
    fn render_shape(&mut self, node: Node, tf: Transform) {
        match node.tag_name().name() {
            "sp" => self.render_sp(node, tf),
            "pic" => self.render_pic(node, tf),
            "grpSp" => self.render_group(node, tf),
            // cxnSp (connectors) etc. — draw as plain shapes if they carry geom.
            "cxnSp" => self.render_sp(node, tf),
            _ => {} // graphicFrame (tables/charts) and unknowns: skip gracefully.
        }
    }

    fn render_group(&mut self, node: Node, tf: Transform) {
        let Some(grp_pr) = ch(node, "grpSpPr") else { return };
        let Some(xfrm) = ch(grp_pr, "xfrm") else {
            // No transform: pass through unchanged.
            for child in node.children().filter(|n| n.is_element()) {
                self.render_shape(child, tf);
            }
            return;
        };
        let x = parse_xfrm(xfrm);
        // Child-space extents; guard against zero to avoid div-by-zero.
        let ch_off_x = f_attr(ch(xfrm, "chOff"), "x");
        let ch_off_y = f_attr(ch(xfrm, "chOff"), "y");
        let ch_ext_cx = f_attr(ch(xfrm, "chExt"), "cx").max(1.0);
        let ch_ext_cy = f_attr(ch(xfrm, "chExt"), "cy").max(1.0);

        // Map the group's own frame into root space, then derive the child map.
        let abs_off_x = tf.tx + x.x * tf.sx;
        let abs_off_y = tf.ty + x.y * tf.sy;
        let abs_ext_cx = x.cx * tf.sx;
        let abs_ext_cy = x.cy * tf.sy;
        let nsx = abs_ext_cx / ch_ext_cx;
        let nsy = abs_ext_cy / ch_ext_cy;
        let inner = Transform {
            sx: nsx,
            sy: nsy,
            tx: abs_off_x - ch_off_x * nsx,
            ty: abs_off_y - ch_off_y * nsy,
        };

        // A group can carry its own rotation/flip; children are positioned in
        // absolute root space by `inner`, so wrap them in a transform about the
        // group's absolute center. (Non-uniform child scale × rotation isn't
        // exact, but it's a close approximation.)
        let group_tf = if x.rot != 0 || x.flip_h || x.flip_v {
            let cx = (abs_off_x + abs_ext_cx / 2.0) / EMU_PER_PT;
            let cy = (abs_off_y + abs_ext_cy / 2.0) / EMU_PER_PT;
            let mut t = String::new();
            if x.rot != 0 {
                t.push_str(&format!(
                    "rotate({} {} {})",
                    fnum(x.rot as f64 / 60000.0),
                    fnum(cx),
                    fnum(cy)
                ));
            }
            if x.flip_h || x.flip_v {
                let sx = if x.flip_h { -1.0 } else { 1.0 };
                let sy = if x.flip_v { -1.0 } else { 1.0 };
                if !t.is_empty() {
                    t.push(' ');
                }
                t.push_str(&format!(
                    "translate({} {}) scale({} {}) translate({} {})",
                    fnum(cx),
                    fnum(cy),
                    fnum(sx),
                    fnum(sy),
                    fnum(-cx),
                    fnum(-cy)
                ));
            }
            Some(t)
        } else {
            None
        };

        if let Some(t) = &group_tf {
            self.body.push_str(&format!(r#"<g transform="{t}">"#));
        }
        for child in node.children().filter(|n| n.is_element()) {
            self.render_shape(child, inner);
        }
        if group_tf.is_some() {
            self.body.push_str("</g>");
        }
    }

    fn render_sp(&mut self, node: Node, tf: Transform) {
        let sp_pr = ch(node, "spPr");
        let ph = shape_placeholder(node);
        let xfrm = sp_pr
            .and_then(|s| ch(s, "xfrm"))
            .map(parse_xfrm)
            .or_else(|| ph.as_ref().and_then(|p| self.inherited_xfrm(p)));
        let Some(x) = xfrm else { return };
        let rect = tf.apply(&x);
        if rect.w <= 0.0 || rect.h <= 0.0 {
            // Still may carry text with inherited geometry; skip if truly empty.
            return;
        }

        // Explicit spPr fill/line wins; otherwise fall back to the shape's
        // p:style fillRef/lnRef references into the theme's fmtScheme.
        let mut fill = sp_pr.map(|s| self.resolve_fill(s)).unwrap_or(Fill::Unspecified);
        if matches!(fill, Fill::Unspecified) {
            if let Some(f) = self.resolve_style_fill(node) {
                fill = f;
            }
        }
        let mut stroke = sp_pr.and_then(|s| self.resolve_stroke(s));
        if stroke.is_none() {
            stroke = self.resolve_style_stroke(node);
        }
        let geom_node = sp_pr.and_then(|s| ch(s, "prstGeom"));

        let transform = rect.svg_transform(&x);
        let open_g = !transform.is_empty();
        if open_g {
            self.body.push_str(&format!(r#"<g transform="{transform}">"#));
        }

        // Draw geometry only when there's something visible to draw.
        let has_fill = matches!(fill, Fill::Solid(_) | Fill::Gradient { .. });
        let has_stroke = stroke.is_some();
        if geom_node.is_some() || has_fill || has_stroke {
            self.draw_geometry(geom_node, &rect, &fill, stroke.as_ref());
        }

        // Text body.
        if let Some(tx_body) = ch(node, "txBody") {
            self.render_text(node, tx_body, &rect, ph.as_ref());
        }

        if open_g {
            self.body.push_str("</g>");
        }
    }

    fn inherited_xfrm(&self, ph: &Placeholder) -> Option<Xfrm> {
        for src in [&self.layout_phs, &self.master_phs] {
            if let Some(m) = match_placeholder(src, ph) {
                if let Some(x) = &m.xfrm {
                    return Some(x.clone());
                }
            }
        }
        None
    }
}

// ---------------------------------------------------------------------------
// Small XML/number utilities
// ---------------------------------------------------------------------------

/// First child element with the given local name.
fn ch<'a, 'i>(n: Node<'a, 'i>, name: &str) -> Option<Node<'a, 'i>> {
    n.children().find(|c| c.is_element() && c.tag_name().name() == name)
}

/// Attribute value by local name (ignoring namespace).
fn a<'a>(n: Node<'a, '_>, name: &str) -> Option<&'a str> {
    n.attributes().find(|at| at.name() == name).map(|at| at.value())
}

/// Numeric attribute (defaults to 0.0).
fn f_attr(n: Option<Node>, name: &str) -> f64 {
    n.and_then(|n| a(n, name))
        .and_then(|v| v.parse::<f64>().ok())
        .unwrap_or(0.0)
}

/// Format a number: integers print without a decimal point, else 2 dp trimmed.
fn fnum(v: f64) -> String {
    if !v.is_finite() {
        return "0".to_string();
    }
    if (v - v.round()).abs() < 1e-6 {
        return format!("{}", v.round() as i64);
    }
    let mut s = format!("{v:.2}");
    while s.ends_with('0') {
        s.pop();
    }
    if s.ends_with('.') {
        s.pop();
    }
    s
}

/// XML-escape text and attribute values.
fn esc(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::image::downscale_raster;
    use super::text::wrap;
    use super::{render_slide_svg, svg_placeholder, RenderOptions};
    use crate::error::Error;
    use crate::fixtures::{DeckSpec, SlideSpec};
    use crate::opc::Package;
    use crate::pptx::PresentationFile;

    fn png_bytes(w: u32, h: u32, alpha: bool) -> Vec<u8> {
        let mut out = Vec::new();
        let img = if alpha {
            image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                w,
                h,
                image::Rgba([10, 120, 220, 128]),
            ))
        } else {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
                w,
                h,
                image::Rgb([10, 120, 220]),
            ))
        };
        img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    #[test]
    fn downscale_shrinks_oversized_opaque_to_jpeg() {
        let big = png_bytes(1000, 600, false);
        let (ct, bytes) = downscale_raster(&big, 512).expect("should downscale");
        assert_eq!(ct, "image/jpeg");
        let dim = imagesize::blob_size(&bytes).unwrap();
        assert!(dim.width as u32 <= 512 && dim.height as u32 <= 512);
        // Aspect ratio preserved: longer edge (width) hits the cap.
        assert_eq!(dim.width, 512);
        assert!(bytes.len() < big.len());
    }

    #[test]
    fn downscale_keeps_alpha_as_png() {
        let big = png_bytes(1000, 600, true);
        let (ct, bytes) = downscale_raster(&big, 512).expect("should downscale");
        assert_eq!(ct, "image/png");
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert!(decoded.color().has_alpha());
        assert!(decoded.width() <= 512 && decoded.height() <= 512);
    }

    #[test]
    fn downscale_leaves_small_images_alone() {
        let small = png_bytes(200, 150, false);
        assert!(downscale_raster(&small, 512).is_none());
    }

    const NS: &str = concat!(
        r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
        r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
        r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#
    );

        /// The SVG namespace is the one legitimate http substring in output.
    fn assert_no_external_refs(svg: &str) {
        let sans_ns = svg.replace(r#"xmlns="http://www.w3.org/2000/svg""#, "");
        assert!(!sans_ns.contains("http"), "unexpected external reference");
        assert!(!sans_ns.contains("<script"));
    }

    fn wrap_slide(shapes: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
        )
    }

    /// Replace slide1 with custom shape XML and reopen.
    fn deck_with_slide1(deck: DeckSpec, shapes: &str) -> PresentationFile {
        let bytes = deck.build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", wrap_slide(shapes).into_bytes());
        PresentationFile::from_package(pkg).unwrap()
    }

    #[test]
    fn title_and_bullets_render() {
        let deck = DeckSpec::new("Deck")
            .slide(SlideSpec::new("Hello Title").bullets(&["First point", "Second point"]));
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();

        assert!(svg.contains(r#"viewBox="0 0 960 540""#), "viewBox: {svg}");
        assert!(svg.contains("Hello Title"));
        assert!(svg.contains("First point"));
        assert!(svg.contains("Second point"));
        assert!(svg.contains("• "), "expected bullet prefix");
        assert!(!svg.contains("<script"));
        assert_no_external_refs(&svg);
    }

    #[test]
    fn image_embedded_as_data_uri() {
        let deck = DeckSpec::new("Deck").slide(SlideSpec::new("Pic").image());
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions { embed_images: true, ..Default::default() }).unwrap();
        assert!(svg.contains("data:image/png;base64,"), "expected data URI");
        assert_no_external_refs(&svg);
    }

    #[test]
    fn image_placeholder_when_not_embedding() {
        let deck = DeckSpec::new("Deck").slide(SlideSpec::new("Pic").image());
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions { embed_images: false, ..Default::default() }).unwrap();
        assert!(!svg.contains("data:"), "should not embed data URIs");
        assert!(svg.contains("<rect"));
    }

    #[test]
    fn special_chars_escaped() {
        let deck = DeckSpec::new("Deck")
            .slide(SlideSpec::new("A & B < C \" D — Grüße Köln"));
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("A &amp; B &lt; C &quot; D"), "escaped: {svg}");
        assert!(svg.contains("Grüße Köln"), "umlauts preserved");
        assert!(!svg.contains("A & B"));
    }

    #[test]
    fn solid_fill_srgb_rect() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="C81E1E"/></a:solidFill></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#C81E1E""##), "svg: {svg}");
    }

    #[test]
    fn scheme_accent_resolves_from_theme() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="accent1"/></a:solidFill></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").accent("FF00AA").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#FF00AA""##), "svg: {svg}");
    }

    #[test]
    fn scheme_bg1_maps_through_clrmap() {
        // bg1 → (clrMap) lt1 → (scheme) window/FFFFFF.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="tx1"/></a:solidFill></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        // tx1 → dk1 → windowText/000000.
        assert!(svg.contains(r##"fill="#000000""##), "svg: {svg}");
    }

    #[test]
    fn lum_transform_changes_color() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Box"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="808080"><a:lumMod val="50000"/></a:srgbClr></a:solidFill></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        // 0x80 * 0.5 = 0x40.
        assert!(svg.contains(r##"fill="#404040""##), "svg: {svg}");
    }

    #[test]
    fn rotated_picture_emits_transform() {
        let shapes = r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="Picture 3"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm rot="2700000"><a:off x="4572000" y="2286000"/><a:ext cx="1828800" cy="1371600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#;
        // Use a deck whose slide1 already has the image rel (rId2).
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x").image()).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", wrap_slide(shapes).into_bytes());
        let pf = PresentationFile::from_package(pkg).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("rotate("), "expected rotation transform: {svg}");
        assert!(svg.contains("data:image/png;base64,"));
    }

    #[test]
    fn flipped_picture_emits_scale() {
        let shapes = r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="P"/><p:cNvPicPr/><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm flipH="1"><a:off x="0" y="0"/><a:ext cx="1828800" cy="1371600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#;
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x").image()).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", wrap_slide(shapes).into_bytes());
        let pf = PresentationFile::from_package(pkg).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("scale(-1 1)"), "expected flip scale: {svg}");
    }

    #[test]
    fn malformed_slide_returns_error() {
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x")).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", b"<p:sld><not closed".to_vec());
        let pf = PresentationFile::from_package(pkg).unwrap();
        let res = render_slide_svg(&pf, 1, &RenderOptions::default());
        assert!(matches!(res, Err(Error::Render(_))), "expected render error");
    }

    #[test]
    fn ellipse_and_roundrect_geometry() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="E"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm><a:prstGeom prst="ellipse"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="112233"/></a:solidFill></p:spPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id="10" name="R"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="3000000" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm><a:prstGeom prst="roundRect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="445566"/></a:solidFill></p:spPr></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("<ellipse"), "svg: {svg}");
        assert!(svg.contains("rx="), "roundRect rounded corners");
    }

    #[test]
    fn group_scaling_applies() {
        // Group maps child space [0,1000000] to a shifted/scaled root frame.
        let shapes = r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="20" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="6096000" y="0"/><a:ext cx="6096000" cy="6858000"/><a:chOff x="0" y="0"/><a:chExt cx="12192000" cy="6858000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="21" name="Inner"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="12192000" cy="6858000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></p:spPr></p:sp></p:grpSp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        // Inner rect fills the group's right half: x=480, width=480.
        assert!(svg.contains(r##"fill="#00FF00""##), "svg: {svg}");
        assert!(svg.contains(r#"x="480""#), "group-scaled x: {svg}");
    }

    #[test]
    fn line_preset_emits_line_element() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="L"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:ln w="12700"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill></a:ln></p:spPr></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("<line "), "expected a <line> element: {svg}");
        assert!(svg.contains(r##"stroke="#FF0000""##), "line stroke color: {svg}");
    }

    #[test]
    fn roundrect_adj_controls_radius() {
        let sp = |adj: &str| {
            format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="R"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm><a:prstGeom prst="roundRect"><a:avLst><a:gd name="adj" fmla="val {adj}"/></a:avLst></a:prstGeom><a:solidFill><a:srgbClr val="445566"/></a:solidFill></p:spPr></p:sp>"#
            )
        };
        let render = |adj: &str| {
            let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &sp(adj));
            render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap()
        };
        // adj=0 → square corners (rx=0); adj=50000 → half the side (rounded).
        assert!(render("0").contains(r#"rx="0""#), "adj=0 should be square");
        assert!(!render("50000").contains(r#"rx="0""#), "adj=50000 should round");
    }

    #[test]
    fn group_rotation_wraps_children() {
        let shapes = r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="20" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm rot="5400000"><a:off x="0" y="0"/><a:ext cx="6096000" cy="6858000"/><a:chOff x="0" y="0"/><a:chExt cx="6096000" cy="6858000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="21" name="Inner"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="3000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="00FF00"/></a:solidFill></p:spPr></p:sp></p:grpSp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        // 5400000/60000 = 90 degrees, about the group's center.
        assert!(svg.contains("rotate(90 "), "group rotation transform: {svg}");
    }

    #[test]
    fn placeholder_inherits_layout_geometry() {
        // Title placeholder with NO xfrm inherits from the layout's title ph.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Inherited</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("Inherited"), "text rendered via inherited geom: {svg}");
        // Layout title off x=838200 → 66pt; clip rect should start near there.
        assert!(svg.contains(r#"x="66""#), "inherited x offset: {svg}");
    }

    #[test]
    fn empty_deck_slide_out_of_range() {
        let deck = DeckSpec::new("Deck").slide(SlideSpec::new("x"));
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        assert!(render_slide_svg(&pf, 2, &RenderOptions::default()).is_err());
        assert!(render_slide_svg(&pf, 0, &RenderOptions::default()).is_err());
    }

    #[test]
    fn placeholder_svg_is_safe() {
        let svg = svg_placeholder(960.0, 540.0, "Loading & <stuff>");
        assert!(svg.contains(r#"viewBox="0 0 960 540""#));
        assert!(svg.contains("Loading &amp; &lt;stuff&gt;"));
        assert_no_external_refs(&svg);
        assert!(!svg.contains("<script"));
    }

    #[test]
    fn wrap_hard_breaks_long_words() {
        let lines = wrap("supercalifragilisticexpialidocious", 18.0, 40.0);
        assert!(lines.len() > 1, "long word should hard-break: {lines:?}");
    }

    // --- Feature 2: real gradients -----------------------------------------

    #[test]
    fn gradient_fill_emits_linear_gradient_with_ordered_stops() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="G"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="4000000" cy="2000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="50000"><a:srgbClr val="00FF00"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:lin ang="0"/></a:gradFill></p:spPr></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("<linearGradient"), "expected a linearGradient: {svg}");
        assert!(svg.contains(r#"fill="url(#grad0)""#), "rect references gradient: {svg}");
        let r = svg.find(r##"stop-color="#FF0000""##).unwrap();
        let g = svg.find(r##"stop-color="#00FF00""##).unwrap();
        let b = svg.find(r##"stop-color="#0000FF""##).unwrap();
        assert!(r < g && g < b, "stops must be in position order: {svg}");
        assert_no_external_refs(&svg);
    }

    #[test]
    fn gradient_angle_90_is_vertical() {
        // ang=5400000 → 90°: endpoints run top→bottom (x1==x2, y from 0 to 1).
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="G"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="4000000" cy="2000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:lin ang="5400000"/></a:gradFill></p:spPr></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r#"x1="0.5""#) && svg.contains(r#"x2="0.5""#), "vertical x: {svg}");
        assert!(svg.contains(r#"y1="0""#) && svg.contains(r#"y2="1""#), "vertical y: {svg}");
    }

    #[test]
    fn identical_gradients_are_deduplicated() {
        let one = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="G"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:gradFill><a:gsLst><a:gs pos="0"><a:srgbClr val="FF0000"/></a:gs><a:gs pos="100000"><a:srgbClr val="0000FF"/></a:gs></a:gsLst><a:lin ang="0"/></a:gradFill></p:spPr></p:sp>"#;
        let shapes = format!("{one}{one}");
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(svg.matches("<linearGradient").count(), 1, "gradient def deduped: {svg}");
        assert_eq!(svg.matches(r#"fill="url(#grad0)""#).count(), 2, "both shapes reuse it: {svg}");
    }

    // --- Feature 3: shape style references ----------------------------------

    #[test]
    fn style_fillref_resolves_from_fmtscheme() {
        // No spPr fill; fillRef idx=1 → fillStyleLst[0] (solidFill phClr),
        // phClr = accent1. lnRef idx=3 → lnStyleLst[2] width 19050 EMU = 1.5pt.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Styled"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:style><a:lnRef idx="3"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="tx1"/></a:fontRef></p:style><p:txBody><a:bodyPr/><a:lstStyle/><a:p/></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").accent("FF00AA").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#FF00AA""##), "fillRef → accent1 fill: {svg}");
        assert!(svg.contains(r##"stroke="#FF00AA""##), "lnRef → accent1 stroke: {svg}");
        assert!(svg.contains(r#"stroke-width="1.5""#), "lnRef width from template: {svg}");
    }

    #[test]
    fn explicit_sppr_fill_wins_over_style_ref() {
        // An explicit solidFill must override the fillRef.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Styled"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="00CC00"/></a:solidFill></p:spPr><p:style><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef></p:style></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").accent("FF00AA").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#00CC00""##), "explicit fill wins: {svg}");
        assert!(!svg.contains(r##"fill="#FF00AA""##), "style ref must not apply: {svg}");
    }

    // --- Feature 4: bgRef backgrounds ---------------------------------------

    fn slide_with_bg(bg_inner: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:bg>{bg_inner}</p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
        )
    }

    fn pf_with_slide_and_theme(deck: DeckSpec, slide_xml: &str, theme: Option<&str>) -> PresentationFile {
        let bytes = deck.build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", slide_xml.to_string().into_bytes());
        if let Some(t) = theme {
            pkg.insert_part("ppt/theme/theme1.xml", t.to_string().into_bytes());
        }
        PresentationFile::from_package(pkg).unwrap()
    }

    #[test]
    fn bgref_derives_background_from_bgfillstylelst() {
        // Fixture bgFillStyleLst is solid phClr; bgRef idx=1001 → accent1.
        let slide = slide_with_bg(r#"<p:bgRef idx="1001"><a:schemeClr val="accent1"/></p:bgRef>"#);
        let pf = pf_with_slide_and_theme(
            DeckSpec::new("Deck").accent("FF00AA").slide(SlideSpec::new("x")),
            &slide,
            None,
        );
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            svg.contains(r##"<rect x="0" y="0" width="960" height="540" fill="#FF00AA"/>"##),
            "background derived from bgRef, not white: {svg}"
        );
    }

    #[test]
    fn bgref_gradient_background_emits_gradient() {
        // Custom theme whose bgFillStyleLst[0] is a gradient; bgRef phClr = #123456.
        let theme = format!(
            r#"<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="T"><a:themeElements><a:clrScheme name="c"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="4472C4"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="f"><a:majorFont><a:latin typeface="Calibri"/></a:majorFont><a:minorFont><a:latin typeface="Calibri"/></a:minorFont></a:fontScheme><a:fmtScheme name="s"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="12700"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln><a:ln w="19050"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:gradFill><a:gsLst><a:gs pos="0"><a:schemeClr val="phClr"/></a:gs><a:gs pos="100000"><a:schemeClr val="phClr"><a:alpha val="0"/></a:schemeClr></a:gs></a:gsLst><a:lin ang="2700000"/></a:gradFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#
        );
        let slide = slide_with_bg(r#"<p:bgRef idx="1001"><a:srgbClr val="123456"/></p:bgRef>"#);
        let pf = pf_with_slide_and_theme(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &slide, Some(&theme));
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("<linearGradient"), "bgRef gradient template emitted: {svg}");
        assert!(svg.contains(r##"stop-color="#123456""##), "phClr substituted into stops: {svg}");
        assert!(svg.contains(r#"fill="url(#grad0)""#), "bg rect uses the gradient: {svg}");
    }
}
