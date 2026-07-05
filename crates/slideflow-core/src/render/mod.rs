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
mod effects;
mod fill;
mod geometry;
mod image;
mod placeholder;
mod style;
mod table;
mod text;

use color::Theme;
use fill::{bg_blip_embed, collect_background, Fill};
use geometry::{parse_xfrm, Transform, Xfrm};
use placeholder::{collect_placeholders, match_placeholder, shape_placeholder, Placeholder};
use style::LstStyle;

/// Version of the SVG output format. Any change that alters the bytes produced
/// for a given slide (renderer fidelity, image encoding, …) between *released*
/// builds should bump this: it is baked into the thumbnail cache key
/// ([`crate::thumbs::thumb_file_name`]) so stale caches invalidate automatically
/// on upgrade, with no eviction bookkeeping.
pub const RENDER_VERSION: u32 = 4;

const EMU_PER_PT: f64 = 12700.0;
// Per indent-level extra left padding, in points (fallback when a paragraph's
// style chain provides no explicit `marL`).
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

    // Parse the layout/master/theme parts once; every downstream step (theme,
    // placeholder inheritance, background, and the static-shape passes) reuses
    // these documents.
    let theme_doc = theme_xml.as_deref().and_then(|x| Document::parse(x).ok());
    let master_doc = master_xml.as_deref().and_then(|x| Document::parse(x).ok());
    let layout_doc = layout_xml.as_deref().and_then(|x| Document::parse(x).ok());

    let mut theme = Theme::default();
    if let Some(doc) = &theme_doc {
        theme.load_theme(doc);
    }
    if let Some(doc) = &master_doc {
        theme.load_clr_map(doc);
    }
    // A slide-level color-map override recolors inherited decoration too, so
    // apply it before any background/placeholder colors are resolved.
    theme.apply_clr_map_override(&slide_doc);

    // Placeholder geometry inheritance sources (layout first, then master).
    let layout_phs = layout_doc
        .as_ref()
        .map(|d| collect_placeholders(d, &theme))
        .unwrap_or_default();
    let master_phs = master_doc
        .as_ref()
        .map(|d| collect_placeholders(d, &theme))
        .unwrap_or_default();

    // Master `p:txStyles` (title/body/other buckets) and the presentation-wide
    // `p:defaultTextStyle` — the weakest layers of the run-style chain. Parsed
    // once here into owned structures the text pass merges per paragraph.
    let (title_style, body_style, other_style) = master_doc
        .as_ref()
        .and_then(|d| {
            d.root_element()
                .children()
                .find(|n| n.is_element() && n.tag_name().name() == "txStyles")
        })
        .map(|tx| {
            let bucket = |name: &str| {
                ch(tx, name).map(|n| LstStyle::parse(n, &theme)).unwrap_or_default()
            };
            (bucket("titleStyle"), bucket("bodyStyle"), bucket("otherStyle"))
        })
        .unwrap_or_default();
    let pres_style = pf
        .package
        .part("ppt/presentation.xml")
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|x| Document::parse(x).ok())
        .and_then(|doc| {
            ch(doc.root_element(), "defaultTextStyle").map(|n| LstStyle::parse(n, &theme))
        })
        .unwrap_or_default();

    let w_pt = pf.slide_width_emu as f64 / EMU_PER_PT;
    let h_pt = pf.slide_height_emu as f64 / EMU_PER_PT;

    let slide_rels = pf.package.rels_for(&slide_part).unwrap_or_default();
    let content_types = pf.package.content_types().ok();

    let mut ctx = Ctx {
        pf,
        options,
        theme,
        slide_no: slide_index,
        group_fills: Vec::new(),
        cur_part: slide_part.clone(),
        cur_rels: slide_rels,
        content_types,
        layout_phs,
        master_phs,
        title_style,
        body_style,
        other_style,
        pres_style,
        defs: String::new(),
        body: String::new(),
        clip_id: 0,
        grad_id: 0,
        grad_cache: HashMap::new(),
        shadow_cache: HashMap::new(),
    };

    // Background: the first of slide → layout → master that declares a `<p:bg>`
    // wins outright. A `bgPr/blipFill` picture background paints a full-slide
    // `<image>` over the base rect (resolved against the part that defined it);
    // any other background resolves to a solid/gradient/bgRef fill, else white.
    let bg_chain: [(Option<&Document>, Option<&str>); 3] = [
        (Some(&slide_doc), Some(slide_part.as_str())),
        (layout_doc.as_ref(), layout_part.as_deref()),
        (master_doc.as_ref(), master_part.as_deref()),
    ];
    let mut bg_fill: Option<Fill> = None;
    let mut bg_blip: Option<(String, String)> = None;
    for (doc, part) in bg_chain {
        let (Some(doc), Some(part)) = (doc, part) else { continue };
        let Some(bg_el) = ch(doc.root_element(), "cSld").and_then(|c| ch(c, "bg")) else {
            continue;
        };
        match bg_blip_embed(bg_el) {
            Some(embed) => bg_blip = Some((embed, part.to_string())),
            None => bg_fill = collect_background(doc, &ctx.theme),
        }
        break;
    }
    let bg_attr = match &bg_fill {
        Some(f @ Fill::Gradient { .. }) => ctx.fill_attrs(f),
        Some(Fill::Solid(c)) => format!(r#" fill="{}""#, c.hex()),
        _ => r##" fill="#FFFFFF""##.to_string(),
    };
    ctx.body.push_str(&format!(
        r#"<rect x="0" y="0" width="{w}" height="{h}"{bg}/>"#,
        w = fnum(w_pt),
        h = fnum(h_pt),
        bg = bg_attr
    ));
    if let Some((embed, part)) = &bg_blip {
        ctx.emit_bg_image(embed, part, w_pt, h_pt);
    }

    // Static decoration from the master and layout, painted *under* the slide's
    // own shapes (z-order: master → layout → slide).
    // showMasterSp: the master pass runs only when neither the layout nor the
    // slide suppresses it (`showMasterSp="0"`). Absent attribute = shown.
    let root = slide_doc.root_element();
    let slide_shows_master = a(root, "showMasterSp") != Some("0");
    let layout_shows_master = layout_doc
        .as_ref()
        .map(|d| a(d.root_element(), "showMasterSp") != Some("0"))
        .unwrap_or(true);
    if slide_shows_master && layout_shows_master {
        if let (Some(doc), Some(part)) = (&master_doc, master_part.as_deref()) {
            let rels = pf.package.rels_for(part).unwrap_or_default();
            ctx.render_static_pass(doc, part, rels);
        }
    }
    // The layout pass always runs.
    if let (Some(doc), Some(part)) = (&layout_doc, layout_part.as_deref()) {
        let rels = pf.package.rels_for(part).unwrap_or_default();
        ctx.render_static_pass(doc, part, rels);
    }

    // Slide shapes, in document order (on top of master/layout decoration).
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
    /// 1-based slide number being rendered (feeds `a:fld type="slidenum"`).
    slide_no: usize,
    /// Fills of the groups currently being descended into (innermost last);
    /// a shape's `a:grpFill` resolves to the top of this stack.
    group_fills: Vec<Fill>,
    /// The part currently being walked (slide, layout, or master) and its
    /// relationships. Blip/image rels resolve against THIS part, so the
    /// master/layout passes point these at their own part for the pass's
    /// duration and restore them to the slide afterward.
    cur_part: String,
    cur_rels: Vec<crate::opc::Relationship>,
    content_types: Option<crate::opc::ContentTypes>,
    layout_phs: Vec<Placeholder>,
    master_phs: Vec<Placeholder>,
    /// Master `p:txStyles` buckets and presentation `p:defaultTextStyle`: the
    /// weakest layers of the per-run style-inheritance chain (see `style.rs`).
    title_style: LstStyle,
    body_style: LstStyle,
    other_style: LstStyle,
    pres_style: LstStyle,
    defs: String,
    body: String,
    clip_id: usize,
    /// Monotonic id for interned `<defs>` gradients (`grad0`, `grad1`, …).
    grad_id: usize,
    /// Serialized-gradient → def-id, so repeated gradients emit one `<defs>`.
    grad_cache: HashMap<String, String>,
    /// Serialized-shadow → filter-id (`sh0`, `sh1`, …), so identical outer
    /// shadows emit one `<filter>` def.
    shadow_cache: HashMap<String, String>,
}

impl Ctx<'_> {
    fn render_shape(&mut self, node: Node, tf: Transform) {
        match node.tag_name().name() {
            "sp" => self.render_sp(node, tf),
            "pic" => self.render_pic(node, tf),
            "grpSp" => self.render_group(node, tf),
            // cxnSp (connectors) etc. — draw as plain shapes if they carry geom.
            "cxnSp" => self.render_sp(node, tf),
            // graphicFrame: tables render; charts/SmartArt/OLE skip gracefully.
            "graphicFrame" => self.render_graphic_frame(node, tf),
            _ => {} // unknowns: skip gracefully.
        }
    }

    fn render_group(&mut self, node: Node, tf: Transform) {
        let Some(grp_pr) = ch(node, "grpSpPr") else { return };
        // Track the group's own fill so children with `a:grpFill` inherit it.
        // A group without a concrete fill passes the enclosing group's through.
        let gf = match self.resolve_fill(grp_pr) {
            Fill::Unspecified => {
                self.group_fills.last().cloned().unwrap_or(Fill::Unspecified)
            }
            f => f,
        };
        self.group_fills.push(gf);
        self.render_group_inner(node, grp_pr, tf);
        self.group_fills.pop();
    }

    fn render_group_inner(&mut self, node: Node, grp_pr: Node, tf: Transform) {
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
        let mut rect = tf.apply(&x);
        let geom_node = sp_pr.and_then(|s| ch(s, "prstGeom").or_else(|| ch(s, "custGeom")));
        // Purely horizontal/vertical connectors legitimately have zero extent
        // along one axis — they're stroked, not filled. Anything else with a
        // degenerate rect is skipped.
        let line_like = geom_node
            .and_then(|g| a(g, "prst"))
            .is_some_and(geometry::is_line_preset);
        if (rect.w <= 0.0 || rect.h <= 0.0) && !line_like {
            return;
        }

        // spAutoFit: PowerPoint recomputes the shape extent from the laid-out
        // text whenever a deck is opened; the STORED extent reflects the
        // author's fonts. With substituted (usually wider) fonts our layout can
        // exceed it, leaving the fill short of the text — so grow, never shrink.
        if let Some(tb) = ch(node, "txBody") {
            if ch(tb, "bodyPr").is_some_and(|b| ch(b, "spAutoFit").is_some()) {
                rect = self.autofit_grow(Some(node), tb, &rect, ph.as_ref());
            }
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
        // An explicit <a:ln><a:noFill/> means "no outline, period" — it must
        // suppress the lnRef style fallback, not fall through to it.
        if stroke.is_none() && !sp_pr.is_some_and(Ctx::explicit_no_line) {
            stroke = self.resolve_style_stroke(node);
        }
        // A picture fill (`a:blipFill`) is painted after the geometry, clipped
        // to the shape geometry (rect, or a dedicated clip for fancier shapes).
        let blip_fill = sp_pr.and_then(|s| ch(s, "blipFill"));

        let transform = rect.svg_transform(&x);
        let open_g = !transform.is_empty();
        if open_g {
            self.body.push_str(&format!(r#"<g transform="{transform}">"#));
        }

        // An outer drop-shadow (spPr/a:effectLst/a:outerShdw) becomes a
        // deduplicated SVG filter wrapping the shape's geometry.
        let shadow = sp_pr.and_then(|s| self.resolve_shadow_filter(s));

        // Draw geometry only when there's something visible to draw.
        let has_fill = matches!(fill, Fill::Solid(_) | Fill::Gradient { .. });
        let has_stroke = stroke.is_some();
        if geom_node.is_some() || has_fill || has_stroke || blip_fill.is_some() {
            if let Some(f) = &shadow {
                self.body.push_str(&format!(r#"<g filter="url(#{f})">"#));
            }
            self.draw_geometry(geom_node, &rect, &fill, stroke.as_ref());
            if shadow.is_some() {
                self.body.push_str("</g>");
            }
        }
        if let Some(bf) = blip_fill {
            let geom_clip = self.geometry_clip(geom_node, &rect);
            self.emit_blip_fill(bf, &rect, geom_clip);
        }

        // Text body.
        if let Some(tx_body) = ch(node, "txBody") {
            self.render_text(Some(node), tx_body, &rect, ph.as_ref());
        }

        if open_g {
            self.body.push_str("</g>");
        }
    }

    /// Render the *static* (non-placeholder) shapes of a layout or master
    /// `spTree` — logos, sidebars, decorative rectangles/lines that give the
    /// deck its visual identity. Placeholder shapes (`p:ph`) are prototypes
    /// PowerPoint never paints directly, so they are skipped: the slide's own
    /// placeholders render in the slide pass with inherited geometry.
    ///
    /// Blip/image rels in these shapes must resolve against the layout/master
    /// part, so `cur_part`/`cur_rels` are swapped to `part`/`rels` for the pass
    /// and restored afterward.
    fn render_static_pass(&mut self, doc: &Document, part: &str, rels: Vec<crate::opc::Relationship>) {
        let prev_part = std::mem::replace(&mut self.cur_part, part.to_string());
        let prev_rels = std::mem::replace(&mut self.cur_rels, rels);
        if let Some(tree) = ch(doc.root_element(), "cSld").and_then(|c| ch(c, "spTree")) {
            let base = Transform::identity();
            for child in tree.children().filter(|n| n.is_element()) {
                // Skip placeholder prototypes; only static decoration is painted.
                if shape_placeholder(child).is_some() {
                    continue;
                }
                self.render_shape(child, base);
            }
        }
        self.cur_part = prev_part;
        self.cur_rels = prev_rels;
    }

    pub(crate) fn inherited_xfrm(&self, ph: &Placeholder) -> Option<Xfrm> {
        for src in [&self.layout_phs, &self.master_phs] {
            if let Some(m) = match_placeholder(src, ph) {
                if let Some(x) = &m.xfrm {
                    return Some(x.clone());
                }
            }
        }
        None
    }

    /// The raw `a:custGeom`/`a:prstGeom` XML of the matching layout/master
    /// placeholder — placeholder pictures inherit their (often diagonal-cut)
    /// shape from the layout exactly like they inherit their xfrm.
    pub(crate) fn inherited_geom_xml(&self, ph: &Placeholder) -> Option<String> {
        for src in [&self.layout_phs, &self.master_phs] {
            if let Some(m) = match_placeholder(src, ph) {
                if let Some(g) = &m.geom_xml {
                    return Some(g.clone());
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
    fn zero_extent_connector_draws_dotted_line_with_end_dot() {
        // A purely vertical leader line: cx=0 (previously dropped as degenerate),
        // 2pt white sysDot dash with round caps and an oval tail dot.
        let shapes = r#"<p:cxnSp><p:nvCxnSpPr><p:cNvPr id="9" name="Leader"/><p:cNvCxnSpPr/><p:nvPr/></p:nvCxnSpPr><p:spPr><a:xfrm><a:off x="914400" y="0"/><a:ext cx="0" cy="1828800"/></a:xfrm><a:prstGeom prst="line"><a:avLst/></a:prstGeom><a:ln w="25400" cap="rnd"><a:solidFill><a:srgbClr val="FF0000"/></a:solidFill><a:prstDash val="sysDot"/><a:headEnd type="none"/><a:tailEnd type="oval"/></a:ln></p:spPr></p:cxnSp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            svg.contains(r#"<line x1="72" y1="0" x2="72" y2="144""#),
            "vertical zero-width connector renders: {svg}"
        );
        // sysDot (1,1)×2pt with round caps → dashes shrink, gaps grow by one width.
        assert!(svg.contains(r#"stroke-dasharray="0.1 4""#), "dash pattern: {svg}");
        assert!(svg.contains(r#"stroke-linecap="round""#), "round cap: {svg}");
        assert!(
            svg.contains(r##"<circle cx="72" cy="144" r="3" fill="#FF0000"/>"##),
            "oval tail end dot: {svg}"
        );
        assert!(!svg.contains(r#"cy="0" r="3""#), "no head dot (type=none): {svg}");
    }

    #[test]
    fn spautofit_grows_fill_rect_to_fit_text() {
        // Stored extent fits ONE 18pt line (authored with narrower fonts); our
        // layout wraps the text to several lines. The white fill must grow with
        // it instead of leaving text spilling out of a one-line bar.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="H"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="400000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="FFFFFF"/></a:solidFill></p:spPr><p:txBody><a:bodyPr wrap="square" anchor="ctr"><a:spAutoFit/></a:bodyPr><a:lstStyle/><a:p><a:r><a:rPr lang="de-DE" sz="1800"/><a:t>Wir sind Spezialisten für digitale Strategieprojekte und mehr</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        // Heights of every white-filled rect (slide background = 540 is one).
        let heights: Vec<f64> = svg
            .match_indices(r##" fill="#FFFFFF""##)
            .filter_map(|(i, _)| {
                let pre = &svg[..i];
                let h = pre.rfind("height=\"")? + 8;
                pre[h..].trim_end_matches('"').parse().ok()
            })
            .collect();
        // Stored: 400000 EMU ≈ 31.5pt for one 18pt line. Grown: ≥3 lines.
        assert!(
            !heights.iter().any(|h| (*h - 31.5).abs() < 1.0),
            "stored one-line height must be replaced (heights={heights:?}): {svg}"
        );
        assert!(
            heights.iter().any(|h| *h > 60.0 && *h < 400.0),
            "spAutoFit grew the fill rect (heights={heights:?}): {svg}"
        );
    }

    #[test]
    fn narrow_typeface_gets_narrow_fallbacks_and_tighter_wrap() {
        let render = |face: &str| {
            let paras = format!(
                r#"<a:p><a:r><a:rPr sz="2000"><a:latin typeface="{face}"/></a:rPr><a:t>MMMMM MMMMM</a:t></a:r></a:p>"#
            );
            let shapes = textbox(
                r#"<a:off x="0" y="0"/><a:ext cx="2151380" cy="4000000"/>"#,
                "<a:bodyPr/>",
                &paras,
            );
            let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
            render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap()
        };
        let narrow = render("Aptos Narrow");
        let regular = render("Aptos");
        assert!(
            narrow.contains("Aptos Narrow, Arial Narrow,"),
            "narrow faces fall back through narrow families: {narrow}"
        );
        assert!(
            !regular.contains("Arial Narrow"),
            "regular faces keep the plain fallback stack: {regular}"
        );
        // Same text, same box: the regular-width estimate wraps into two lines,
        // the ~18% tighter narrow estimate keeps it on one.
        assert_eq!(narrow.matches("<text ").count(), 1, "narrow fits one line: {narrow}");
        assert_eq!(regular.matches("<text ").count(), 2, "regular wraps: {regular}");
    }

    #[test]
    fn placeholder_pic_inherits_layout_custgeom_clip() {
        // Layout: picture placeholder idx=7 with a triangular custGeom (the
        // "diagonal photo cut" pattern). Slide: placeholder pic with neither
        // xfrm nor geometry of its own — both inherit, and the image is
        // clipped to the triangle.
        let layout = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="titleAndBody"><p:cSld name="Pic"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="image"/><p:cNvSpPr/><p:nvPr><p:ph type="pic" sz="quarter" idx="7"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm><a:custGeom><a:avLst/><a:gdLst/><a:pathLst><a:path w="100" h="100"><a:moveTo><a:pt x="0" y="100"/></a:moveTo><a:lnTo><a:pt x="50" y="0"/></a:lnTo><a:lnTo><a:pt x="100" y="100"/></a:lnTo><a:close/></a:path></a:pathLst></a:custGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        );
        let pic = r#"<p:pic><p:nvPicPr><p:cNvPr id="5" name="Bild 4"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr><p:ph type="pic" sz="quarter" idx="7"/></p:nvPr></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr/></p:pic>"#;
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x").image()).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", wrap_slide(pic).into_bytes());
        pkg.insert_part("ppt/slideLayouts/slideLayout1.xml", layout.into_bytes());
        let pf = PresentationFile::from_package(pkg).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            svg.contains(r#"<clipPath id="geomclip0"><path d="M0 100L50 0L100 100Z""#),
            "triangle clip from the layout placeholder: {svg}"
        );
        assert!(
            svg.contains(r##"<g clip-path="url(#geomclip0)">"##),
            "image wrapped in the geometry clip: {svg}"
        );
        assert!(svg.contains("data:image/png;base64,"), "image embedded: {svg}");
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
        // Layout title off x=838200 → 66pt; text starts there + 7.2pt left inset.
        // (Was asserted via the text clipPath's x="66" until text clipping was
        // removed to match PowerPoint's overflow-visible default.)
        assert!(svg.contains(r#"x="73.2""#), "inherited x offset: {svg}");
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

    #[test]
    fn explicit_noline_suppresses_lnref() {
        // <a:ln><a:noFill/> means "no outline, period" — the lnRef style
        // fallback must NOT apply (this drew spurious black borders on layout
        // decoration shapes).
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:ln><a:noFill/></a:ln></p:spPr><p:style><a:lnRef idx="2"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="1"><a:schemeClr val="accent1"/></a:fillRef></p:style></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").accent("FF00AA").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#FF00AA""##), "fillRef still applies: {svg}");
        assert!(!svg.contains("stroke="), "explicit noFill line suppresses lnRef: {svg}");
    }

    #[test]
    fn slidenum_field_renders_actual_number() {
        // The cached <a:t> of a slidenum field often still holds the layout's
        // "‹Nr.›" prompt — the real slide number must render instead.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="N"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="500000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:fld id="{X}" type="slidenum"><a:rPr lang="de-DE"/><a:t>&#8249;Nr.&#8250;</a:t></a:fld></a:p></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("<tspan>1</tspan>"), "field resolves to slide number: {svg}");
        assert!(!svg.contains("Nr."), "cached prompt must not render: {svg}");
    }

    #[test]
    fn run_highlight_draws_marker_box() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="H"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="4000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:noFill/></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="de-DE"><a:highlight><a:srgbClr val="FFFF00"/></a:highlight></a:rPr><a:t>Marked</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        let marker = svg.find(r##"fill="#FFFF00""##).expect("highlight box present");
        let text = svg.find("Marked").expect("text present");
        assert!(marker < text, "marker box drawn behind (before) the text: {svg}");
    }

    #[test]
    fn grpfill_inherits_group_fill() {
        // A shape with a:grpFill takes the containing group's fill (white boxes
        // behind text on photo slides work this way).
        let shapes = r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="20" name="G"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="6096000" cy="3000000"/><a:chOff x="0" y="0"/><a:chExt cx="6096000" cy="3000000"/></a:xfrm><a:solidFill><a:srgbClr val="ABCDEF"/></a:solidFill></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="21" name="Inner"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="3000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:grpFill/></p:spPr></p:sp></p:grpSp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#ABCDEF""##), "grpFill inherits group fill: {svg}");
    }

    #[test]
    fn text_is_not_clipped_to_its_shape() {
        // PowerPoint's default is overflow-visible; a text body must not emit a
        // clipPath (our width estimates run slightly wide and used to cut words).
        let deck = DeckSpec::new("Deck")
            .slide(SlideSpec::new("Hello Title").bullets(&["First point", "Second point"]));
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(!svg.contains("<clipPath"), "no text clipPath expected: {svg}");
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

    // --- Feature R2: layout/master static shapes ---------------------------

    /// Build a deck, optionally overriding the (single) layout and master parts.
    fn pf_custom(deck: DeckSpec, layout: Option<&str>, master: Option<&str>) -> PresentationFile {
        let bytes = deck.build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        if let Some(l) = layout {
            pkg.insert_part("ppt/slideLayouts/slideLayout1.xml", l.to_string().into_bytes());
        }
        if let Some(m) = master {
            pkg.insert_part("ppt/slideMasters/slideMaster1.xml", m.to_string().into_bytes());
        }
        PresentationFile::from_package(pkg).unwrap()
    }

    /// A layout carrying one static decoration rect (`#AB12CD`, no `p:ph`) and a
    /// title placeholder prototype whose fill is `#EE99FF`.
    fn layout_with_deco() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="titleAndBody"><p:cSld name="Custom"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Deco"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="AB12CD"/></a:solidFill></p:spPr></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="EE99FF"/></a:solidFill></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        )
    }

    /// A master carrying one static decoration rect (`#123ABC`, no `p:ph`).
    fn master_with_deco() -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Logo"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="123ABC"/></a:solidFill></p:spPr></p:sp></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:sldMaster>"#
        )
    }

    #[test]
    fn layout_static_shape_renders_once_placeholder_skipped() {
        let pf = pf_custom(
            DeckSpec::new("Deck").slide(SlideSpec::new("Hello")),
            Some(&layout_with_deco()),
            None,
        );
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        // Static decoration rect painted exactly once.
        assert_eq!(
            svg.matches(r##"fill="#AB12CD""##).count(),
            1,
            "static layout rect drawn exactly once: {svg}"
        );
        // The layout's placeholder prototype (its fill) must NOT leak into output.
        assert!(
            !svg.contains(r##"fill="#EE99FF""##),
            "layout placeholder prototype must be skipped: {svg}"
        );
        // The slide's own title still renders.
        assert!(svg.contains("Hello"), "slide title still rendered: {svg}");
    }

    #[test]
    fn master_static_shape_renders() {
        let pf = pf_custom(
            DeckSpec::new("Deck").slide(SlideSpec::new("Hi")),
            None,
            Some(&master_with_deco()),
        );
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(
            svg.matches(r##"fill="#123ABC""##).count(),
            1,
            "static master rect drawn exactly once: {svg}"
        );
    }

    #[test]
    fn show_master_sp_zero_suppresses_master_pass() {
        // Layout with showMasterSp="0" hides the master's static decoration.
        let layout = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="titleAndBody" showMasterSp="0"><p:cSld name="NoMaster"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        );
        let pf = pf_custom(
            DeckSpec::new("Deck").slide(SlideSpec::new("Hi")),
            Some(&layout),
            Some(&master_with_deco()),
        );
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            !svg.contains(r##"fill="#123ABC""##),
            "showMasterSp=0 must suppress master static shapes: {svg}"
        );
    }

    // --- Feature R2: clrMapOvr override -------------------------------------

    #[test]
    fn clr_map_override_recolors_scheme() {
        // A rect filled with schemeClr bg1. Under the master map bg1→lt1 (white);
        // an overrideClrMapping remaps bg1→dk1 (black). The master background
        // (also schemeClr bg1) recolors along with it — which is the point.
        let shape = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="B"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></p:spPr></p:sp>"#;
        let render = |ovr: &str| {
            let slide = format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/>{shape}</p:spTree></p:cSld><p:clrMapOvr>{ovr}</p:clrMapOvr></p:sld>"#
            );
            let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x")).build();
            let mut pkg = Package::from_bytes(&bytes).unwrap();
            pkg.insert_part("ppt/slides/slide1.xml", slide.into_bytes());
            let pf = PresentationFile::from_package(pkg).unwrap();
            render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap()
        };
        let over = render(
            r#"<a:overrideClrMapping bg1="dk1" tx1="lt1" bg2="dk2" tx2="lt2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>"#,
        );
        let none = render(r#"<a:masterClrMapping/>"#);
        assert!(
            over.contains(r##"fill="#000000""##) && !over.contains(r##"fill="#FFFFFF""##),
            "override remaps bg1→dk1 (black): {over}"
        );
        assert!(
            none.contains(r##"fill="#FFFFFF""##) && !none.contains(r##"fill="#000000""##),
            "master map keeps bg1→lt1 (white): {none}"
        );
    }

    // --- Feature R2: custom geometry ---------------------------------------

    /// Wrap a custGeom `pathLst` body into a shape whose extents are 100×100pt
    /// (1270000 EMU) and path space is 100×100 → 1:1 scale for easy assertions.
    fn custgeom_shape(path_lst: &str, fill: &str) -> String {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="C"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="1270000" cy="1270000"/></a:xfrm><a:custGeom><a:avLst/><a:gdLst/><a:rect l="0" t="0" r="0" b="0"/><a:pathLst>{path_lst}</a:pathLst></a:custGeom>{fill}</p:spPr></p:sp>"#
        )
    }

    #[test]
    fn custgeom_triangle_emits_scaled_path() {
        let path = r#"<a:path w="100" h="100"><a:moveTo><a:pt x="0" y="100"/></a:moveTo><a:lnTo><a:pt x="50" y="0"/></a:lnTo><a:lnTo><a:pt x="100" y="100"/></a:lnTo><a:close/></a:path>"#;
        let shapes = custgeom_shape(path, r#"<a:solidFill><a:srgbClr val="AA00BB"/></a:solidFill>"#);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            svg.contains(r#"<path d="M0 100L50 0L100 100Z""#),
            "triangle path scaled into rect: {svg}"
        );
        assert!(svg.contains(r##"fill="#AA00BB""##), "path carries the shape fill: {svg}");
    }

    #[test]
    fn custgeom_cubic_bezier_emits_c_command() {
        let path = r#"<a:path w="100" h="100"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:cubicBezTo><a:pt x="0" y="50"/><a:pt x="50" y="100"/><a:pt x="100" y="100"/></a:cubicBezTo></a:path>"#;
        let shapes = custgeom_shape(path, r#"<a:solidFill><a:srgbClr val="112233"/></a:solidFill>"#);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r#"<path d="M0 0C0 50 50 100 100 100""#), "cubic bezier path: {svg}");
    }

    #[test]
    fn custgeom_formula_guide_falls_back_to_rect() {
        // A guide-name coordinate (`x="wd2"`) is not a literal number → Tier 1
        // aborts the path conversion and the shape draws as a plain rectangle.
        let path = r#"<a:path w="100" h="100"><a:moveTo><a:pt x="wd2" y="0"/></a:moveTo><a:lnTo><a:pt x="100" y="100"/></a:lnTo><a:close/></a:path>"#;
        let shapes = custgeom_shape(path, r#"<a:solidFill><a:srgbClr val="0055AA"/></a:solidFill>"#);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(!svg.contains("<path d="), "non-literal coord must not emit a path: {svg}");
        assert!(
            svg.contains(r##"<rect x="0" y="0" width="100" height="100" fill="#0055AA"/>"##),
            "shape falls back to a plain rect: {svg}"
        );
    }

    // --- Feature R2: extra preset geometries -------------------------------

    #[test]
    fn extra_presets_emit_polygons_with_expected_vertex_counts() {
        let cases = [
            ("triangle", 3),
            ("rtTriangle", 3),
            ("diamond", 4),
            ("parallelogram", 4),
            ("trapezoid", 4),
            ("pentagon", 5),
            ("hexagon", 6),
            ("chevron", 6),
            ("rightArrow", 7),
            ("leftArrow", 7),
            ("plus", 12),
        ];
        for (prst, n) in cases {
            let shapes = format!(
                r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm><a:prstGeom prst="{prst}"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="123456"/></a:solidFill></p:spPr></p:sp>"#
            );
            let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
            let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
            let marker = r#"<polygon points=""#;
            let start = svg.find(marker).unwrap_or_else(|| panic!("{prst}: no polygon: {svg}"));
            let rest = &svg[start + marker.len()..];
            let pts = &rest[..rest.find('"').unwrap()];
            assert_eq!(
                pts.split(' ').filter(|s| s.contains(',')).count(),
                n,
                "{prst}: expected {n} vertices, got points={pts:?}"
            );
            assert!(svg.contains(r##"fill="#123456""##), "{prst}: carries fill: {svg}");
        }
    }

    #[test]
    fn unknown_preset_still_falls_back_to_rect() {
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="cloud"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="654321"/></a:solidFill></p:spPr></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(!svg.contains("<polygon"), "unknown preset must not emit polygon: {svg}");
        // Falls back to a plain <rect> carrying the shape fill (not a <path>).
        assert!(!svg.contains("<path"), "unknown preset must not emit a path: {svg}");
        assert!(
            svg.contains(r##"fill="#654321""##),
            "unknown preset still fills as a rect: {svg}"
        );
    }

    // --- Feature R2: blipFill shape fills & picture backgrounds -------------

    /// A rect shape whose fill is the slide image `rId2` (present when the deck
    /// slide is built with `.image()`).
    const BLIP_FILL_SHAPE: &str = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="Filled"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="2000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></a:blipFill></p:spPr></p:sp>"#;

    fn pf_with_image_slide(shapes: &str) -> PresentationFile {
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x").image()).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", wrap_slide(shapes).into_bytes());
        PresentationFile::from_package(pkg).unwrap()
    }

    #[test]
    fn shape_blip_fill_emits_clipped_image() {
        let pf = pf_with_image_slide(BLIP_FILL_SHAPE);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("data:image/png;base64,"), "blip fill embedded: {svg}");
        assert!(
            svg.contains(r#"preserveAspectRatio="xMidYMid slice""#),
            "picture fill uses slice: {svg}"
        );
        assert!(svg.contains(r##"clip-path="url(#blipclip0)""##), "clipped to shape: {svg}");
        assert_no_external_refs(&svg);
    }

    #[test]
    fn shape_blip_fill_falls_back_to_placeholder_without_embedding() {
        let pf = pf_with_image_slide(BLIP_FILL_SHAPE);
        let svg =
            render_slide_svg(&pf, 1, &RenderOptions { embed_images: false, ..Default::default() })
                .unwrap();
        assert!(!svg.contains("data:"), "no data URI when not embedding: {svg}");
        assert!(svg.contains(r##"fill="#D1D5DB""##), "gray placeholder box drawn: {svg}");
    }

    #[test]
    fn slide_picture_background_emits_full_slide_image() {
        let slide = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS}><p:cSld><p:bg><p:bgPr><a:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></a:blipFill></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
        );
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x").image()).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", slide.into_bytes());
        let pf = PresentationFile::from_package(pkg).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            svg.contains(
                r#"<image x="0" y="0" width="960" height="540" preserveAspectRatio="xMidYMid slice""#
            ),
            "full-slide bg image: {svg}"
        );
        assert!(svg.contains("data:image/png;base64,"), "bg image embedded: {svg}");
    }

    #[test]
    fn layout_picture_background_resolves_against_layout_part() {
        use crate::fixtures::TINY_PNG;
        use crate::opc::{rel_type, Relationship};
        // A layout that declares a full-bleed picture background via its own rel.
        let layout = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="titleAndBody"><p:cSld name="Bg"><p:bg><p:bgPr><a:blipFill><a:blip r:embed="rIdBg"/><a:stretch><a:fillRect/></a:stretch></a:blipFill></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        );
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x")).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slideLayouts/slideLayout1.xml", layout.into_bytes());
        pkg.insert_part("ppt/media/imageL.png", TINY_PNG.to_vec());
        pkg.set_rels(
            "ppt/slideLayouts/slideLayout1.xml",
            &[
                Relationship {
                    id: "rId1".into(),
                    rel_type: rel_type::SLIDE_MASTER.into(),
                    target: "../slideMasters/slideMaster1.xml".into(),
                    external: false,
                },
                Relationship {
                    id: "rIdBg".into(),
                    rel_type: rel_type::IMAGE.into(),
                    target: "../media/imageL.png".into(),
                    external: false,
                },
            ],
        );
        let pf = PresentationFile::from_package(pkg).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            svg.contains(
                r#"<image x="0" y="0" width="960" height="540" preserveAspectRatio="xMidYMid slice""#
            ),
            "layout picture background painted full-slide: {svg}"
        );
        assert!(svg.contains("data:image/png;base64,"), "resolved against layout rels: {svg}");
    }

    // --- Feature R3: text rendering overhaul --------------------------------

    /// A plain (non-placeholder) text box shape with the given `a:xfrm` inner
    /// XML, full `a:bodyPr` element, and paragraph XML.
    fn textbox(xfrm: &str, body_pr: &str, paras: &str) -> String {
        format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="TB"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm>{xfrm}</a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody>{body_pr}<a:lstStyle/>{paras}</p:txBody></p:sp>"#
        )
    }

    /// Order-preserving list of every `y="…"` on a `<text>` element.
    fn text_baselines(svg: &str) -> Vec<f64> {
        svg.match_indices("<text ")
            .map(|(i, _)| {
                let rest = &svg[i..];
                let y0 = rest.find(r#"y=""#).unwrap() + 3;
                let y1 = rest[y0..].find('"').unwrap();
                rest[y0..y0 + y1].parse::<f64>().unwrap()
            })
            .collect()
    }

    fn master_txstyles(txstyles: &str) -> String {
        format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster {NS}><p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/>{txstyles}</p:sldMaster>"#
        )
    }

    /// Deck whose slide1 is `shapes` and whose master is overridden.
    fn pf_slide_master(shapes: &str, master: &str) -> PresentationFile {
        let bytes = DeckSpec::new("Deck").slide(SlideSpec::new("x")).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", wrap_slide(shapes).into_bytes());
        pkg.insert_part("ppt/slideMasters/slideMaster1.xml", master.to_string().into_bytes());
        PresentationFile::from_package(pkg).unwrap()
    }

    /// Deck whose slide1 is `shapes` and whose layout is overridden (accent
    /// #FF00AA so scheme colors are checkable).
    fn pf_slide_layout(shapes: &str, layout: &str) -> PresentationFile {
        let bytes = DeckSpec::new("Deck").accent("FF00AA").slide(SlideSpec::new("x")).build();
        let mut pkg = Package::from_bytes(&bytes).unwrap();
        pkg.insert_part("ppt/slides/slide1.xml", wrap_slide(shapes).into_bytes());
        pkg.insert_part("ppt/slideLayouts/slideLayout1.xml", layout.to_string().into_bytes());
        PresentationFile::from_package(pkg).unwrap()
    }

    // Step 1 — per-run spans.

    #[test]
    fn per_run_spans_emit_distinct_tspans() {
        // plain + bold-teal + plain → three tspans; only the middle is bold and
        // carries the teal fill.
        let paras = r#"<a:p><a:r><a:rPr lang="de-DE" sz="2000"/><a:t>Wir sind </a:t></a:r><a:r><a:rPr lang="de-DE" sz="2000" b="1"><a:solidFill><a:srgbClr val="2FA190"/></a:solidFill></a:rPr><a:t>Spezialisten</a:t></a:r><a:r><a:rPr lang="de-DE" sz="2000"/><a:t> hier</a:t></a:r></a:p>"#;
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="11000000" cy="1000000"/>"#, "<a:bodyPr/>", paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(svg.matches("<tspan").count(), 3, "one tspan per run: {svg}");
        // The bold run carries the teal fill and font-weight; the plain runs do not.
        assert!(
            svg.contains(r##"<tspan fill="#2FA190" font-weight="bold"> Spezialisten</tspan>"##),
            "middle run bold + teal: {svg}"
        );
        assert!(svg.contains("<tspan>Wir sind</tspan>"), "plain run unstyled: {svg}");
    }

    #[test]
    fn wrap_respects_larger_span_size() {
        // Same two words; when the second run is huge it no longer fits beside
        // the first and wraps onto a second line.
        let two = |sz2: &str| {
            let paras = format!(
                r#"<a:p><a:r><a:rPr sz="1200"/><a:t>one</a:t></a:r><a:r><a:rPr sz="{sz2}"/><a:t> TWO</a:t></a:r></a:p>"#
            );
            let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="1300000" cy="2000000"/>"#, "<a:bodyPr/>", &paras);
            let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
            let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
            svg.matches("<text ").count()
        };
        assert_eq!(two("1200"), 1, "small second span fits on one line");
        assert_eq!(two("6000"), 2, "large second span forces a wrap");
    }

    // Step 2 — style inheritance chain.

    #[test]
    fn master_bodystyle_size_applies_without_rpr() {
        let master = master_txstyles(
            r#"<p:txStyles><p:titleStyle/><p:bodyStyle><a:lvl1pPr><a:defRPr sz="2000"/></a:lvl1pPr></p:bodyStyle><p:otherStyle/></p:txStyles>"#,
        );
        let body = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="B"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="8000000" cy="3000000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Body</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = pf_slide_master(body, &master);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r#"font-size="20""#), "master bodyStyle sz=2000 → 20pt: {svg}");
    }

    #[test]
    fn shape_rpr_overrides_layout_lststyle_size() {
        let layout = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="titleAndBody"><p:cSld name="L"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr><a:defRPr sz="4000"/></a:lvl1pPr></a:lstStyle><a:p><a:endParaRPr/></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        );
        // buNone so no default bullet (which would size from the 40pt paragraph
        // default) muddies the check — this isolates the run-vs-layout size.
        let body = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="B"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:buNone/></a:pPr><a:r><a:rPr lang="en-US" sz="1200"/><a:t>Small</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = pf_slide_layout(body, &layout);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r#"font-size="12""#), "run sz=1200 wins: {svg}");
        assert!(!svg.contains(r#"font-size="40""#), "layout lstStyle sz must be overridden: {svg}");
    }

    #[test]
    fn layout_lststyle_color_applies_to_unstyled_run() {
        let layout = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS} type="titleAndBody"><p:cSld name="L"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="3" name="Body"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle><a:lvl1pPr><a:defRPr><a:solidFill><a:srgbClr val="00AA55"/></a:solidFill></a:defRPr></a:lvl1pPr></a:lstStyle><a:p><a:endParaRPr/></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
        );
        let body = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="B"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Colored</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = pf_slide_layout(body, &layout);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#00AA55""##), "layout lstStyle color on unstyled run: {svg}");
    }

    // Step 3 — fonts + fontRef.

    #[test]
    fn run_typeface_emitted_first_in_font_family() {
        let paras = r#"<a:p><a:r><a:rPr lang="en-US"><a:latin typeface="Custom Font"/></a:rPr><a:t>Hi</a:t></a:r></a:p>"#;
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="4000000" cy="1000000"/>"#, "<a:bodyPr/>", paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(
            svg.contains(r#"font-family="Custom Font, Helvetica, Arial, sans-serif""#),
            "run typeface first, fallbacks after: {svg}"
        );
    }

    #[test]
    fn fontref_color_shows_on_unstyled_run() {
        // fontRef color (accent1 = #FF00AA) becomes the shape's default text color.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="S"/><p:cNvSpPr txBox="1"/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="4000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:style><a:lnRef idx="0"><a:schemeClr val="accent1"/></a:lnRef><a:fillRef idx="0"><a:schemeClr val="accent1"/></a:fillRef><a:effectRef idx="0"><a:schemeClr val="accent1"/></a:effectRef><a:fontRef idx="minor"><a:schemeClr val="accent1"/></a:fontRef></p:style><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Hi</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").accent("FF00AA").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#FF00AA""##), "fontRef color on text: {svg}");
    }

    // Step 4 — real bullets.

    #[test]
    fn buchar_renders_literal_character() {
        let paras = r#"<a:p><a:pPr><a:buFont typeface="Arial"/><a:buChar char="–"/></a:pPr><a:r><a:rPr lang="en-US"/><a:t>Item</a:t></a:r></a:p>"#;
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="6000000" cy="1000000"/>"#, "<a:bodyPr/>", paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("<tspan>– </tspan>"), "buChar en-dash bullet: {svg}");
    }

    #[test]
    fn buautonum_numbers_consecutive_paragraphs() {
        let p = |t: &str| format!(r#"<a:p><a:pPr><a:buAutoNum type="arabicPeriod"/></a:pPr><a:r><a:rPr/><a:t>{t}</a:t></a:r></a:p>"#);
        let paras = format!("{}{}{}", p("a"), p("b"), p("c"));
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="6000000" cy="4000000"/>"#, "<a:bodyPr/>", &paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        let (a, b, c) = (svg.find("1. ").unwrap(), svg.find("2. ").unwrap(), svg.find("3. ").unwrap());
        assert!(a < b && b < c, "auto-numbered 1./2./3. in order: {svg}");
    }

    #[test]
    fn buautonum_resets_per_level() {
        // lvl0(→1.), lvl1(→1., a fresh counter), lvl0(→2., continues).
        let p = |lvl: &str, t: &str| {
            format!(r#"<a:p><a:pPr lvl="{lvl}"><a:buAutoNum type="arabicPeriod"/></a:pPr><a:r><a:rPr/><a:t>{t}</a:t></a:r></a:p>"#)
        };
        let paras = format!("{}{}{}", p("0", "a"), p("1", "b"), p("0", "c"));
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="8000000" cy="4000000"/>"#, "<a:bodyPr/>", &paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.matches("1. ").count() >= 2, "level 1 restarts at 1.: {svg}");
        assert!(svg.contains("2. "), "level 0 continues past the nested level: {svg}");
    }

    #[test]
    fn bunone_suppresses_body_bullet() {
        // A body placeholder normally defaults to "•"; buNone removes it.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="B"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="6000000" cy="2000000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:pPr><a:buNone/></a:pPr><a:r><a:rPr lang="en-US"/><a:t>NoBullet</a:t></a:r></a:p></p:txBody></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("NoBullet"), "text present: {svg}");
        assert!(!svg.contains('\u{2022}'), "buNone suppresses the bullet: {svg}");
    }

    #[test]
    fn title_placeholder_has_no_bullet() {
        let deck = DeckSpec::new("Deck").slide(SlideSpec::new("Just a title"));
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains("Just a title"));
        assert!(!svg.contains('\u{2022}'), "title gets no bullet: {svg}");
    }

    // Step 5 — spacing, insets, autofit.

    #[test]
    fn normautofit_fontscale_scales_run_sizes() {
        let paras = r#"<a:p><a:r><a:rPr sz="4000"/><a:t>Big</a:t></a:r></a:p>"#;
        let body_pr = r#"<a:bodyPr><a:normAutofit fontScale="62500"/></a:bodyPr>"#;
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="9000000" cy="2000000"/>"#, body_pr, paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r#"font-size="25""#), "40pt × 0.625 = 25pt: {svg}");
    }

    #[test]
    fn lnspc_pct_scales_line_advance() {
        // One paragraph, two lines (a:br). The baseline delta is the line advance.
        let render = |ln_spc: &str| {
            let paras = format!(
                r#"<a:p><a:pPr>{ln_spc}</a:pPr><a:r><a:rPr sz="1800"/><a:t>A</a:t></a:r><a:br/><a:r><a:rPr sz="1800"/><a:t>B</a:t></a:r></a:p>"#
            );
            let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="9000000" cy="4000000"/>"#, "<a:bodyPr/>", &paras);
            let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
            let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
            let ys = text_baselines(&svg);
            ys[1] - ys[0]
        };
        let base = render("");
        let wide = render(r#"<a:lnSpc><a:spcPct val="150000"/></a:lnSpc>"#);
        assert!((wide / base - 1.5).abs() < 0.02, "150% line advance: base={base} wide={wide}");
    }

    #[test]
    fn custom_lins_shifts_text_left_edge() {
        // lIns=360000 EMU = 28.35pt; left-aligned text starts there.
        let paras = r#"<a:p><a:r><a:rPr sz="1800"/><a:t>Hi</a:t></a:r></a:p>"#;
        let body_pr = r#"<a:bodyPr lIns="360000"/>"#;
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="9000000" cy="2000000"/>"#, body_pr, paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r#"<text x="28.35""#), "custom lIns honored: {svg}");
    }

    #[test]
    fn wrap_none_yields_single_line() {
        // A narrow box whose long text would wrap several times, but wrap="none".
        let paras = r#"<a:p><a:r><a:rPr sz="1800"/><a:t>alpha bravo charlie delta echo</a:t></a:r></a:p>"#;
        let body_pr = r#"<a:bodyPr wrap="none"/>"#;
        let shapes = textbox(r#"<a:off x="0" y="0"/><a:ext cx="600000" cy="2000000"/>"#, body_pr, paras);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(svg.matches("<text ").count(), 1, "wrap=none → single line: {svg}");
    }

    // --- R4 Feature 3: pattFill approximation --------------------------------

    #[test]
    fn pattfill_bgclr_fills_solid() {
        // A hatch pattern with a red background approximates to a red solid fill.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="P"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:pattFill prst="ltUpDiag"><a:fgClr><a:srgbClr val="FFFFFF"/></a:fgClr><a:bgClr><a:srgbClr val="CC0000"/></a:bgClr></a:pattFill></p:spPr></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert!(svg.contains(r##"fill="#CC0000""##), "pattFill bgClr → solid: {svg}");
    }

    // --- R4 Feature 2: outer drop shadows ------------------------------------

    #[test]
    fn outer_shadow_emits_drop_shadow_filter() {
        // dir=5400000 (90°) pushes the shadow straight down; dist=127000 EMU=10pt.
        let shapes = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="1000000" y="1000000"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="336699"/></a:solidFill><a:effectLst><a:outerShdw blurRad="50800" dist="127000" dir="5400000"><a:srgbClr val="000000"><a:alpha val="40000"/></a:srgbClr></a:outerShdw></a:effectLst></p:spPr></p:sp>"#;
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(svg.matches("<filter").count(), 1, "one filter def: {svg}");
        assert!(svg.contains("feDropShadow"), "expected feDropShadow: {svg}");
        assert!(svg.contains(r#"dx="0""#) && svg.contains(r#"dy="10""#), "dir 90° → down: {svg}");
        assert!(svg.contains(r#"flood-opacity="0.4""#), "alpha → flood-opacity: {svg}");
        assert!(svg.contains(r#"filter="url(#sh0)""#), "geometry references filter: {svg}");
    }

    #[test]
    fn identical_shadows_are_deduplicated() {
        let one = r#"<p:sp><p:nvSpPr><p:cNvPr id="9" name="S"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="2000000" cy="1000000"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom><a:solidFill><a:srgbClr val="336699"/></a:solidFill><a:effectLst><a:outerShdw blurRad="50800" dist="127000" dir="2700000"><a:srgbClr val="000000"/></a:outerShdw></a:effectLst></p:spPr></p:sp>"#;
        let shapes = format!("{one}{one}");
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(svg.matches("<filter").count(), 1, "shadow def deduped: {svg}");
        assert_eq!(svg.matches(r#"filter="url(#sh0)""#).count(), 2, "both shapes reuse it: {svg}");
    }

    // --- R4 Feature 1: basic tables ------------------------------------------

    fn table_frame(xfrm_ext: &str, tbl: &str) -> String {
        format!(
            r#"<p:graphicFrame><p:nvGraphicFramePr><p:cNvPr id="5" name="Table"/><p:cNvGraphicFramePr/><p:nvPr/></p:nvGraphicFramePr><p:xfrm><a:off x="0" y="0"/>{xfrm_ext}</p:xfrm><a:graphic><a:graphicData uri="http://schemas.openxmlformats.org/drawingml/2006/table">{tbl}</a:graphicData></a:graphic></p:graphicFrame>"#
        )
    }

    #[test]
    fn table_renders_cells_with_fills_and_text() {
        // 2×2 grid: frame 200pt×100pt, columns/rows split evenly (100pt / 50pt).
        let cell = |txt: &str, col: &str| {
            format!(
                r#"<a:tc><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>{txt}</a:t></a:r></a:p></a:txBody><a:tcPr><a:solidFill><a:srgbClr val="{col}"/></a:solidFill></a:tcPr></a:tc>"#
            )
        };
        let tbl = format!(
            r#"<a:tbl><a:tblGrid><a:gridCol w="1270000"/><a:gridCol w="1270000"/></a:tblGrid><a:tr h="635000">{}{}</a:tr><a:tr h="635000">{}{}</a:tr></a:tbl>"#,
            cell("C11", "AA0001"),
            cell("C12", "AA0002"),
            cell("C21", "AA0003"),
            cell("C22", "AA0004"),
        );
        let shapes = table_frame(r#"<a:ext cx="2540000" cy="1270000"/>"#, &tbl);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        for c in ["AA0001", "AA0002", "AA0003", "AA0004"] {
            assert!(svg.contains(&format!(r##"fill="#{c}""##)), "cell fill {c}: {svg}");
        }
        for t in ["C11", "C12", "C21", "C22"] {
            assert!(svg.contains(t), "cell text {t}: {svg}");
        }
        // Cumulative offsets: right column at x=100, bottom row at y=50.
        assert!(svg.contains(r#"x="100""#), "right column offset: {svg}");
        assert!(svg.contains(r#"y="50""#), "bottom row offset: {svg}");
        // No tableStyleId → subtle default gridline drawn.
        assert!(svg.contains(r##"stroke="#D0D0D0""##), "default gridline: {svg}");
        assert_no_external_refs(&svg);
    }

    #[test]
    fn table_gridspan_spans_two_columns() {
        // A gridSpan=2 master cell (followed by its hMerge continuation) fills the
        // whole 200pt frame width; the continuation cell draws nothing.
        let tbl = r#"<a:tbl><a:tblGrid><a:gridCol w="1270000"/><a:gridCol w="1270000"/></a:tblGrid><a:tr h="635000"><a:tc gridSpan="2"><a:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:t>Wide</a:t></a:r></a:p></a:txBody><a:tcPr><a:solidFill><a:srgbClr val="BB0001"/></a:solidFill></a:tcPr></a:tc><a:tc hMerge="1"><a:txBody><a:bodyPr/><a:lstStyle/><a:p/></a:txBody><a:tcPr/></a:tc></a:tr></a:tbl>"#;
        let shapes = table_frame(r#"<a:ext cx="2540000" cy="635000"/>"#, tbl);
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(svg.matches(r##"fill="#BB0001""##).count(), 1, "one spanning fill: {svg}");
        assert!(svg.contains(r##"<rect x="0" y="0" width="200" height="50" fill="#BB0001""##), "spans both columns: {svg}");
        assert!(svg.contains("Wide"), "cell text: {svg}");
    }

    #[test]
    fn chart_graphic_frame_renders_nothing() {
        // A graphicFrame carrying a chart (non-table uri) draws no table content;
        // the only rect is the slide background.
        let shapes = table_frame(r#"<a:ext cx="2540000" cy="1270000"/>"#, "");
        // Swap the table uri for a chart uri.
        let shapes = shapes.replace(
            "http://schemas.openxmlformats.org/drawingml/2006/table",
            "http://schemas.openxmlformats.org/drawingml/2006/chart",
        );
        let pf = deck_with_slide1(DeckSpec::new("Deck").slide(SlideSpec::new("x")), &shapes);
        let svg = render_slide_svg(&pf, 1, &RenderOptions::default()).unwrap();
        assert_eq!(svg.matches("<rect").count(), 1, "only the background rect: {svg}");
        assert!(!svg.contains("<line"), "no table gridlines: {svg}");
        assert!(!svg.contains("<text"), "no cell text: {svg}");
    }
}
