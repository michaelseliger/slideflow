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

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use roxmltree::{Document, Node};

use crate::error::{Error, Result};
use crate::opc::resolve_target;
use crate::pptx::PresentationFile;

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
}

impl Default for RenderOptions {
    fn default() -> Self {
        RenderOptions { embed_images: true }
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
    };

    // Background: slide's own, else layout, else master, else white.
    let slide_bg = collect_background(&slide_doc, &ctx.theme);
    let bg = slide_bg.or(layout_bg).or(master_bg);
    let bg_hex = match bg {
        Some(Fill::Solid(c)) => c.hex(),
        _ => "#FFFFFF".to_string(),
    };
    ctx.body.push_str(&format!(
        r#"<rect x="0" y="0" width="{w}" height="{h}" fill="{bg}"/>"#,
        w = fnum(w_pt),
        h = fnum(h_pt),
        bg = bg_hex
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
        for child in node.children().filter(|n| n.is_element()) {
            self.render_shape(child, inner);
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

        let fill = sp_pr.map(|s| self.resolve_fill(s)).unwrap_or(Fill::Unspecified);
        let stroke = sp_pr.and_then(|s| self.resolve_stroke(s));
        let geom = sp_pr
            .and_then(|s| ch(s, "prstGeom"))
            .and_then(|g| a(g, "prst"))
            .map(|p| p.to_string());

        let transform = rect.svg_transform(&x);
        let open_g = !transform.is_empty();
        if open_g {
            self.body.push_str(&format!(r#"<g transform="{transform}">"#));
        }

        // Draw geometry only when there's something visible to draw.
        let has_fill = matches!(fill, Fill::Solid(_));
        let has_stroke = stroke.is_some();
        if geom.is_some() || has_fill || has_stroke {
            self.draw_geometry(geom.as_deref(), &rect, &fill, stroke.as_ref());
        }

        // Text body.
        if let Some(tx_body) = ch(node, "txBody") {
            self.render_text(node, tx_body, &rect, ph.as_ref());
        }

        if open_g {
            self.body.push_str("</g>");
        }
    }

    fn draw_geometry(&mut self, geom: Option<&str>, rect: &Rect, fill: &Fill, stroke: Option<&Stroke>) {
        let fill_attrs = fill.svg_attrs();
        let mut stroke_attrs = String::new();
        if let Some(s) = stroke {
            stroke_attrs = format!(
                r#" stroke="{}" stroke-width="{}""#,
                s.color.hex(),
                fnum(s.width_pt.max(0.25))
            );
        }
        match geom.unwrap_or("rect") {
            "ellipse" => {
                self.body.push_str(&format!(
                    r#"<ellipse cx="{cx}" cy="{cy}" rx="{rx}" ry="{ry}"{fill}{stroke}/>"#,
                    cx = fnum(rect.x + rect.w / 2.0),
                    cy = fnum(rect.y + rect.h / 2.0),
                    rx = fnum(rect.w / 2.0),
                    ry = fnum(rect.h / 2.0),
                    fill = fill_attrs,
                    stroke = stroke_attrs
                ));
            }
            "roundRect" => {
                let r = (rect.w.min(rect.h) * 0.1).max(0.0);
                self.body.push_str(&format!(
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" rx="{r}" ry="{r}"{fill}{stroke}/>"#,
                    x = fnum(rect.x),
                    y = fnum(rect.y),
                    w = fnum(rect.w),
                    h = fnum(rect.h),
                    r = fnum(r),
                    fill = fill_attrs,
                    stroke = stroke_attrs
                ));
            }
            // rect and any unknown preset fall back to a plain rectangle.
            _ => {
                self.body.push_str(&format!(
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}"{fill}{stroke}/>"#,
                    x = fnum(rect.x),
                    y = fnum(rect.y),
                    w = fnum(rect.w),
                    h = fnum(rect.h),
                    fill = fill_attrs,
                    stroke = stroke_attrs
                ));
            }
        }
    }

    fn render_pic(&mut self, node: Node, tf: Transform) {
        let sp_pr = ch(node, "spPr");
        let xfrm = sp_pr.and_then(|s| ch(s, "xfrm")).map(parse_xfrm);
        let Some(x) = xfrm else { return };
        let rect = tf.apply(&x);
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let transform = rect.svg_transform(&x);
        let open_g = !transform.is_empty();
        if open_g {
            self.body.push_str(&format!(r#"<g transform="{transform}">"#));
        }

        let data_uri = if self.options.embed_images {
            self.pic_data_uri(node)
        } else {
            None
        };
        match data_uri {
            Some(uri) => {
                self.body.push_str(&format!(
                    r#"<image x="{x}" y="{y}" width="{w}" height="{h}" preserveAspectRatio="none" href="{uri}"/>"#,
                    x = fnum(rect.x),
                    y = fnum(rect.y),
                    w = fnum(rect.w),
                    h = fnum(rect.h),
                    uri = uri
                ));
            }
            None => self.draw_image_placeholder(&rect),
        }

        if open_g {
            self.body.push_str("</g>");
        }
    }

    fn pic_data_uri(&self, node: Node) -> Option<String> {
        let blip = ch(node, "blipFill").and_then(|b| ch(b, "blip"))?;
        // r:embed attribute (namespaced) — match by local name.
        let embed = blip
            .attributes()
            .find(|at| at.name() == "embed")
            .map(|at| at.value())?;
        let rel = self
            .slide_rels
            .iter()
            .find(|r| r.id == embed && !r.external)?;
        let target = resolve_target(&self.slide_part, &rel.target);
        let bytes = self.pf.package.part(&target)?;
        let ct = self
            .content_types
            .as_ref()
            .and_then(|c| c.content_type_of(&target))
            .map(|s| s.to_string())
            .or_else(|| mime_from_ext(&target));
        let ct = ct?;
        if !(ct == "image/png" || ct == "image/jpeg" || ct == "image/gif") {
            return None; // unsupported raster: skip gracefully.
        }
        Some(format!("data:{};base64,{}", ct, B64.encode(bytes)))
    }

    fn draw_image_placeholder(&mut self, rect: &Rect) {
        self.body.push_str(&format!(
            r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#D1D5DB"/>"##,
            x = fnum(rect.x),
            y = fnum(rect.y),
            w = fnum(rect.w),
            h = fnum(rect.h)
        ));
        // A minimal "photo" glyph: a sun disc and a mountain triangle.
        let cx = rect.x + rect.w * 0.3;
        let cy = rect.y + rect.h * 0.3;
        let r = (rect.w.min(rect.h) * 0.08).max(1.0);
        self.body.push_str(&format!(
            r##"<circle cx="{cx}" cy="{cy}" r="{r}" fill="#9CA3AF"/>"##,
            cx = fnum(cx),
            cy = fnum(cy),
            r = fnum(r)
        ));
        let bx = rect.x + rect.w * 0.15;
        let by = rect.y + rect.h * 0.8;
        let mx = rect.x + rect.w * 0.5;
        let my = rect.y + rect.h * 0.45;
        let ex = rect.x + rect.w * 0.85;
        self.body.push_str(&format!(
            r##"<polygon points="{bx},{by} {mx},{my} {ex},{by}" fill="#9CA3AF"/>"##,
            bx = fnum(bx),
            by = fnum(by),
            mx = fnum(mx),
            my = fnum(my),
            ex = fnum(ex)
        ));
    }

    fn resolve_fill(&self, sp_pr: Node) -> Fill {
        for child in sp_pr.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "solidFill" => {
                    if let Some(c) = child
                        .children()
                        .find(|n| n.is_element())
                        .and_then(|cn| self.theme.parse_color(cn))
                    {
                        return Fill::Solid(c);
                    }
                    return Fill::Unspecified;
                }
                "noFill" => return Fill::NoFill,
                "gradFill" => {
                    // Use the first gradient stop's color as a flat approximation.
                    if let Some(c) = ch(child, "gsLst")
                        .and_then(|l| ch(l, "gs"))
                        .and_then(|gs| gs.children().find(|n| n.is_element()))
                        .and_then(|cn| self.theme.parse_color(cn))
                    {
                        return Fill::Solid(c);
                    }
                    return Fill::Unspecified;
                }
                "pattFill" | "blipFill" => return Fill::Unspecified,
                _ => {}
            }
        }
        // Fall back to a style reference (p:style/a:fillRef) if present.
        Fill::Unspecified
    }

    fn resolve_stroke(&self, sp_pr: Node) -> Option<Stroke> {
        let ln = ch(sp_pr, "ln")?;
        // Explicit noFill outline → no stroke.
        if ln.children().any(|n| n.is_element() && n.tag_name().name() == "noFill") {
            return None;
        }
        let color = ch(ln, "solidFill")
            .and_then(|f| f.children().find(|n| n.is_element()))
            .and_then(|cn| self.theme.parse_color(cn))?;
        let width_pt = a(ln, "w")
            .and_then(|w| w.parse::<f64>().ok())
            .map(|w| w / EMU_PER_PT)
            .unwrap_or(1.0);
        Some(Stroke { color, width_pt })
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

    fn render_text(&mut self, sp: Node, tx_body: Node, rect: &Rect, ph: Option<&Placeholder>) {
        let is_title = ph
            .and_then(|p| p.typ.as_deref())
            .map(|t| t == "title" || t == "ctrTitle")
            .unwrap_or(false);
        let is_body_ph = ph.is_some() && !is_title;
        let default_sz = if is_title { 44.0 } else { 18.0 };
        let font = if is_title {
            &self.theme.major_font
        } else {
            &self.theme.minor_font
        };
        let font_family = font_family(font);

        let body_pr = ch(tx_body, "bodyPr");
        let anchor = body_pr
            .and_then(|b| a(b, "anchor"))
            .unwrap_or(if ph.and_then(|p| p.typ.as_deref()) == Some("ctrTitle") {
                "ctr"
            } else {
                "t"
            })
            .to_string();

        // Collect wrapped lines with per-line style.
        let mut lines: Vec<Line> = Vec::new();
        for para in tx_body.children().filter(|n| n.is_element() && n.tag_name().name() == "p") {
            self.collect_paragraph(para, rect, is_title, is_body_ph, default_sz, &mut lines);
        }
        if lines.is_empty() {
            return;
        }

        let total_h: f64 = lines.iter().map(|l| l.line_height).sum();
        let block_top = match anchor.as_str() {
            "ctr" => rect.y + (rect.h - total_h) / 2.0,
            "b" => rect.y + rect.h - total_h - T_INS,
            _ => rect.y + T_INS,
        };

        // Clip text to the shape.
        self.clip_id += 1;
        let clip = format!("clip{}", self.clip_id);
        self.defs.push_str(&format!(
            r#"<clipPath id="{clip}"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath>"#,
            clip = clip,
            x = fnum(rect.x),
            y = fnum(rect.y),
            w = fnum(rect.w),
            h = fnum(rect.h)
        ));
        self.body.push_str(&format!(r#"<g clip-path="url(#{clip})">"#));

        let mut cursor = block_top;
        for line in &lines {
            let baseline = cursor + line.size * 0.8;
            let (anchor_attr, tx) = match line.algn.as_str() {
                "ctr" => ("middle", rect.x + rect.w / 2.0),
                "r" => ("end", rect.x + rect.w - R_INS),
                _ => ("start", rect.x + L_INS + line.indent),
            };
            let mut style = String::new();
            if line.bold {
                style.push_str(r#" font-weight="bold""#);
            }
            if line.italic {
                style.push_str(r#" font-style="italic""#);
            }
            self.body.push_str(&format!(
                r#"<text x="{x}" y="{y}" font-family="{ff}" font-size="{sz}" fill="{fill}" text-anchor="{anchor}"{style}>{text}</text>"#,
                x = fnum(tx),
                y = fnum(baseline),
                ff = font_family,
                sz = fnum(line.size),
                fill = line.color.hex(),
                anchor = anchor_attr,
                style = style,
                text = esc(&line.text)
            ));
            cursor += line.line_height;
        }
        self.body.push_str("</g>");
        let _ = sp;
    }

    fn collect_paragraph(
        &self,
        para: Node,
        rect: &Rect,
        is_title: bool,
        is_body_ph: bool,
        default_sz: f64,
        out: &mut Vec<Line>,
    ) {
        let p_pr = ch(para, "pPr");
        let lvl = p_pr
            .and_then(|p| a(p, "lvl"))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let algn = p_pr
            .and_then(|p| a(p, "algn"))
            .unwrap_or("l")
            .to_string();
        let bu_none = p_pr
            .map(|p| p.children().any(|n| n.is_element() && n.tag_name().name() == "buNone"))
            .unwrap_or(false);

        // Gather run texts and take style from the first run.
        let mut text = String::new();
        let mut size = default_sz;
        let mut bold = false;
        let mut italic = false;
        let mut color = self.theme.text_default();
        let mut first_run = true;
        for child in para.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "r" => {
                    let r_pr = ch(child, "rPr");
                    if first_run {
                        if let Some(rp) = r_pr {
                            if let Some(sz) = a(rp, "sz").and_then(|v| v.parse::<f64>().ok()) {
                                size = sz / 100.0;
                            }
                            bold = a(rp, "b") == Some("1");
                            italic = a(rp, "i") == Some("1");
                            if let Some(c) = ch(rp, "solidFill")
                                .and_then(|f| f.children().find(|n| n.is_element()))
                                .and_then(|cn| self.theme.parse_color(cn))
                            {
                                color = c;
                            }
                        }
                        first_run = false;
                    }
                    if let Some(t) = ch(child, "t") {
                        text.push_str(t.text().unwrap_or(""));
                    }
                }
                "br" => text.push('\n'),
                "fld" => {
                    if let Some(t) = ch(child, "t") {
                        text.push_str(t.text().unwrap_or(""));
                    }
                }
                _ => {}
            }
        }

        let indent = lvl * LVL_INDENT;
        let bullet = is_body_ph && !is_title && !bu_none;
        let avail = (rect.w - L_INS - R_INS - indent).max(size); // never below one glyph
        let line_height = size * 1.2;

        if text.trim().is_empty() {
            // Preserve empty paragraphs as vertical space.
            out.push(Line {
                text: String::new(),
                size,
                bold,
                italic,
                color,
                algn: algn.clone(),
                indent,
                line_height,
            });
            return;
        }

        // Honor explicit line breaks, then wrap each segment.
        for (seg_idx, segment) in text.split('\n').enumerate() {
            let wrapped = wrap(segment, size, avail);
            for (wi, wl) in wrapped.iter().enumerate() {
                let prefix = if bullet && seg_idx == 0 && wi == 0 { "• " } else { "" };
                out.push(Line {
                    text: format!("{prefix}{wl}"),
                    size,
                    bold,
                    italic,
                    color,
                    algn: algn.clone(),
                    indent,
                    line_height,
                });
            }
        }
    }
}

struct Line {
    text: String,
    size: f64,
    bold: bool,
    italic: bool,
    color: Rgba,
    algn: String,
    indent: f64,
    line_height: f64,
}

// ---------------------------------------------------------------------------
// Geometry / transforms
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Xfrm {
    x: f64,
    y: f64,
    cx: f64,
    cy: f64,
    rot: i64,
    flip_h: bool,
    flip_v: bool,
}

fn parse_xfrm(node: Node) -> Xfrm {
    let off = ch(node, "off");
    let ext = ch(node, "ext");
    Xfrm {
        x: f_attr(off, "x"),
        y: f_attr(off, "y"),
        cx: f_attr(ext, "cx"),
        cy: f_attr(ext, "cy"),
        rot: a(node, "rot").and_then(|v| v.parse::<i64>().ok()).unwrap_or(0),
        flip_h: a(node, "flipH") == Some("1"),
        flip_v: a(node, "flipV") == Some("1"),
    }
}

/// Affine mapping from a coordinate space (EMU) into the root slide space (EMU):
/// `out = t + in * s`.
#[derive(Clone, Copy)]
struct Transform {
    sx: f64,
    sy: f64,
    tx: f64,
    ty: f64,
}

impl Transform {
    fn identity() -> Self {
        Transform { sx: 1.0, sy: 1.0, tx: 0.0, ty: 0.0 }
    }

    /// Map a shape's `xfrm` into an absolute rectangle in points.
    fn apply(&self, x: &Xfrm) -> Rect {
        let ax = self.tx + x.x * self.sx;
        let ay = self.ty + x.y * self.sy;
        let aw = x.cx * self.sx;
        let ah = x.cy * self.sy;
        Rect {
            x: ax / EMU_PER_PT,
            y: ay / EMU_PER_PT,
            w: aw / EMU_PER_PT,
            h: ah / EMU_PER_PT,
        }
    }
}

struct Rect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

impl Rect {
    /// Build an SVG transform string for rotation/flip about the rect's center.
    fn svg_transform(&self, x: &Xfrm) -> String {
        let cx = self.x + self.w / 2.0;
        let cy = self.y + self.h / 2.0;
        let mut parts = String::new();
        if x.rot != 0 {
            let deg = x.rot as f64 / 60000.0;
            parts.push_str(&format!("rotate({} {} {})", fnum(deg), fnum(cx), fnum(cy)));
        }
        if x.flip_h || x.flip_v {
            let sx = if x.flip_h { -1.0 } else { 1.0 };
            let sy = if x.flip_v { -1.0 } else { 1.0 };
            if !parts.is_empty() {
                parts.push(' ');
            }
            parts.push_str(&format!(
                "translate({} {}) scale({} {}) translate({} {})",
                fnum(cx),
                fnum(cy),
                fnum(sx),
                fnum(sy),
                fnum(-cx),
                fnum(-cy)
            ));
        }
        parts
    }
}

// ---------------------------------------------------------------------------
// Placeholders
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Placeholder {
    typ: Option<String>,
    idx: Option<String>,
    xfrm: Option<Xfrm>,
}

/// Read the `p:ph` of an `sp`/`pic`, if it is a placeholder.
fn shape_placeholder(node: Node) -> Option<Placeholder> {
    // nvSpPr/nvPr/ph  (or nvPicPr/nvPr/ph)
    let nv = node
        .children()
        .find(|n| n.is_element() && n.tag_name().name().starts_with("nv"))?;
    let nv_pr = ch(nv, "nvPr")?;
    let ph = ch(nv_pr, "ph")?;
    Some(Placeholder {
        typ: a(ph, "type").map(|s| s.to_string()),
        idx: a(ph, "idx").map(|s| s.to_string()),
        xfrm: None,
    })
}

fn collect_placeholders(doc: &Document, _theme: &Theme) -> Vec<Placeholder> {
    let mut out = Vec::new();
    let Some(tree) = ch(doc.root_element(), "cSld").and_then(|c| ch(c, "spTree")) else {
        return out;
    };
    collect_placeholders_in(tree, &mut out);
    out
}

fn collect_placeholders_in(tree: Node, out: &mut Vec<Placeholder>) {
    for shape in tree.children().filter(|n| n.is_element()) {
        match shape.tag_name().name() {
            "sp" | "pic" => {
                if let Some(mut ph) = shape_placeholder(shape) {
                    ph.xfrm = ch(shape, "spPr").and_then(|s| ch(s, "xfrm")).map(parse_xfrm);
                    out.push(ph);
                }
            }
            "grpSp" => collect_placeholders_in(shape, out),
            _ => {}
        }
    }
}

/// Match by (type+idx), then idx, then type — per the contract.
fn match_placeholder<'a>(src: &'a [Placeholder], want: &Placeholder) -> Option<&'a Placeholder> {
    if let Some(m) = src
        .iter()
        .find(|p| p.typ == want.typ && p.idx == want.idx)
    {
        return Some(m);
    }
    if want.idx.is_some() {
        if let Some(m) = src.iter().find(|p| p.idx == want.idx) {
            return Some(m);
        }
    }
    if want.typ.is_some() {
        if let Some(m) = src.iter().find(|p| p.typ == want.typ) {
            return Some(m);
        }
    }
    // A body placeholder with no explicit idx often matches the first body ph.
    None
}

// ---------------------------------------------------------------------------
// Background
// ---------------------------------------------------------------------------

fn collect_background(doc: &Document, theme: &Theme) -> Option<Fill> {
    let bg = ch(doc.root_element(), "cSld").and_then(|c| ch(c, "bg"))?;
    // p:bgPr (explicit fill) or p:bgRef (style index + color).
    if let Some(bg_pr) = ch(bg, "bgPr") {
        for child in bg_pr.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "solidFill" => {
                    return child
                        .children()
                        .find(|n| n.is_element())
                        .and_then(|cn| theme.parse_color(cn))
                        .map(Fill::Solid);
                }
                "gradFill" => {
                    return ch(child, "gsLst")
                        .and_then(|l| ch(l, "gs"))
                        .and_then(|gs| gs.children().find(|n| n.is_element()))
                        .and_then(|cn| theme.parse_color(cn))
                        .map(Fill::Solid);
                }
                "noFill" => return Some(Fill::NoFill),
                _ => {}
            }
        }
    }
    if let Some(bg_ref) = ch(bg, "bgRef") {
        return bg_ref
            .children()
            .find(|n| n.is_element())
            .and_then(|cn| theme.parse_color(cn))
            .map(Fill::Solid);
    }
    None
}

// ---------------------------------------------------------------------------
// Fill / stroke / color
// ---------------------------------------------------------------------------

enum Fill {
    Solid(Rgba),
    NoFill,
    Unspecified,
}

impl Fill {
    fn svg_attrs(&self) -> String {
        match self {
            Fill::Solid(c) => {
                if c.a < 0.999 {
                    format!(r#" fill="{}" fill-opacity="{}""#, c.hex(), fnum(c.a))
                } else {
                    format!(r#" fill="{}""#, c.hex())
                }
            }
            _ => r#" fill="none""#.to_string(),
        }
    }
}

struct Stroke {
    color: Rgba,
    width_pt: f64,
}

#[derive(Clone, Copy)]
struct Rgba {
    r: f64,
    g: f64,
    b: f64,
    a: f64,
}

impl Rgba {
    fn new(r: u8, g: u8, b: u8) -> Self {
        Rgba { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }
    }

    fn hex(&self) -> String {
        format!(
            "#{:02X}{:02X}{:02X}",
            self.r.round().clamp(0.0, 255.0) as u8,
            self.g.round().clamp(0.0, 255.0) as u8,
            self.b.round().clamp(0.0, 255.0) as u8
        )
    }
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Rgba::new(r, g, b))
}

// ---------------------------------------------------------------------------
// Theme
// ---------------------------------------------------------------------------

struct Theme {
    /// scheme slot (dk1, lt1, dk2, lt2, accent1..6, hlink, folHlink) → color
    scheme: HashMap<String, Rgba>,
    /// clrMap: bg1/tx1/bg2/tx2/accentN/... → scheme slot
    clr_map: HashMap<String, String>,
    major_font: String,
    minor_font: String,
}

impl Default for Theme {
    fn default() -> Self {
        let mut scheme = HashMap::new();
        scheme.insert("dk1".into(), Rgba::new(0, 0, 0));
        scheme.insert("lt1".into(), Rgba::new(255, 255, 255));
        scheme.insert("dk2".into(), Rgba::new(0x44, 0x54, 0x6A));
        scheme.insert("lt2".into(), Rgba::new(0xE7, 0xE6, 0xE6));
        scheme.insert("accent1".into(), Rgba::new(0x44, 0x72, 0xC4));
        Theme {
            scheme,
            clr_map: HashMap::new(),
            major_font: "Calibri".into(),
            minor_font: "Calibri".into(),
        }
    }
}

impl Theme {
    fn load_theme(&mut self, doc: &Document) {
        let root = doc.root_element();
        let Some(elems) = ch(root, "themeElements") else { return };
        if let Some(scheme) = ch(elems, "clrScheme") {
            for slot in scheme.children().filter(|n| n.is_element()) {
                let name = slot.tag_name().name().to_string();
                if let Some(color) = slot
                    .children()
                    .find(|n| n.is_element())
                    .and_then(|cn| self.parse_scheme_base(cn))
                {
                    self.scheme.insert(name, color);
                }
            }
        }
        if let Some(fonts) = ch(elems, "fontScheme") {
            if let Some(f) = ch(fonts, "majorFont")
                .and_then(|m| ch(m, "latin"))
                .and_then(|l| a(l, "typeface"))
                .filter(|s| !s.is_empty())
            {
                self.major_font = f.to_string();
            }
            if let Some(f) = ch(fonts, "minorFont")
                .and_then(|m| ch(m, "latin"))
                .and_then(|l| a(l, "typeface"))
                .filter(|s| !s.is_empty())
            {
                self.minor_font = f.to_string();
            }
        }
    }

    fn load_clr_map(&mut self, doc: &Document) {
        // clrMap lives directly under the master root.
        let root = doc.root_element();
        if let Some(map) = root
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "clrMap")
        {
            for at in map.attributes() {
                self.clr_map.insert(at.name().to_string(), at.value().to_string());
            }
        }
    }

    /// Resolve a scheme slot color without transforms.
    fn parse_scheme_base(&self, node: Node) -> Option<Rgba> {
        match node.tag_name().name() {
            "srgbClr" => a(node, "val").and_then(parse_hex),
            "sysClr" => Some(sys_color(node)),
            _ => None,
        }
    }

    fn resolve_scheme(&self, val: &str) -> Rgba {
        let slot: String = match val {
            "bg1" | "tx1" | "bg2" | "tx2" => self
                .clr_map
                .get(val)
                .cloned()
                .unwrap_or_else(|| default_map_slot(val)),
            other => other.to_string(),
        };
        self.scheme
            .get(&slot)
            .copied()
            .unwrap_or_else(|| Rgba::new(0, 0, 0))
    }

    fn text_default(&self) -> Rgba {
        self.resolve_scheme("tx1")
    }

    /// Parse a DrawingML color element (`srgbClr`/`sysClr`/`schemeClr`/`prstClr`)
    /// applying its transform children.
    fn parse_color(&self, node: Node) -> Option<Rgba> {
        let mut base = match node.tag_name().name() {
            "srgbClr" => a(node, "val").and_then(parse_hex)?,
            "sysClr" => sys_color(node),
            "schemeClr" => self.resolve_scheme(a(node, "val")?),
            "prstClr" => preset_color(a(node, "val")?),
            "scrgbClr" => scrgb_color(node)?,
            _ => return None,
        };
        for t in node.children().filter(|n| n.is_element()) {
            let f = a(t, "val").and_then(|v| v.parse::<f64>().ok()).map(|v| v / 100000.0);
            match t.tag_name().name() {
                "lumMod" => {
                    if let Some(f) = f {
                        base.r *= f;
                        base.g *= f;
                        base.b *= f;
                    }
                }
                "lumOff" => {
                    if let Some(f) = f {
                        base.r += 255.0 * f;
                        base.g += 255.0 * f;
                        base.b += 255.0 * f;
                    }
                }
                "shade" => {
                    if let Some(f) = f {
                        base.r *= f;
                        base.g *= f;
                        base.b *= f;
                    }
                }
                "tint" => {
                    if let Some(f) = f {
                        base.r = base.r * f + 255.0 * (1.0 - f);
                        base.g = base.g * f + 255.0 * (1.0 - f);
                        base.b = base.b * f + 255.0 * (1.0 - f);
                    }
                }
                "alpha" => {
                    if let Some(f) = f {
                        base.a = f;
                    }
                }
                _ => {} // satMod, hueMod, gamma, inv, gray … ignored.
            }
        }
        base.r = base.r.clamp(0.0, 255.0);
        base.g = base.g.clamp(0.0, 255.0);
        base.b = base.b.clamp(0.0, 255.0);
        base.a = base.a.clamp(0.0, 1.0);
        Some(base)
    }
}

fn default_map_slot(val: &str) -> String {
    match val {
        "bg1" => "lt1",
        "tx1" => "dk1",
        "bg2" => "lt2",
        "tx2" => "dk2",
        other => other,
    }
    .to_string()
}

fn sys_color(node: Node) -> Rgba {
    if let Some(last) = a(node, "lastClr").and_then(parse_hex) {
        return last;
    }
    match a(node, "val").unwrap_or("windowText") {
        "window" => Rgba::new(255, 255, 255),
        _ => Rgba::new(0, 0, 0),
    }
}

fn scrgb_color(node: Node) -> Option<Rgba> {
    let pct = |name: &str| {
        a(node, name)
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| (v / 100000.0 * 255.0).clamp(0.0, 255.0))
    };
    Some(Rgba {
        r: pct("r")?,
        g: pct("g")?,
        b: pct("b")?,
        a: 1.0,
    })
}

fn preset_color(name: &str) -> Rgba {
    match name {
        "black" => Rgba::new(0, 0, 0),
        "white" => Rgba::new(255, 255, 255),
        "red" => Rgba::new(255, 0, 0),
        "green" => Rgba::new(0, 128, 0),
        "blue" => Rgba::new(0, 0, 255),
        "yellow" => Rgba::new(255, 255, 0),
        "gray" | "grey" => Rgba::new(128, 128, 128),
        "cyan" => Rgba::new(0, 255, 255),
        "magenta" => Rgba::new(255, 0, 255),
        _ => Rgba::new(0, 0, 0),
    }
}

// ---------------------------------------------------------------------------
// Text helpers
// ---------------------------------------------------------------------------

/// Greedy word wrap using an average-glyph-width heuristic (~0.52em).
fn wrap(text: &str, font_size: f64, avail_width: f64) -> Vec<String> {
    let char_w = 0.52 * font_size;
    let max_chars = ((avail_width / char_w).floor() as usize).max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if wlen > max_chars {
            // Flush the current line, then hard-break the long word.
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            let mut chunk = String::new();
            for c in word.chars() {
                chunk.push(c);
                if chunk.chars().count() >= max_chars {
                    lines.push(std::mem::take(&mut chunk));
                }
            }
            if !chunk.is_empty() {
                current = chunk;
                current_len = current.chars().count();
            }
            continue;
        }
        let extra = if current.is_empty() { wlen } else { wlen + 1 };
        if current_len + extra > max_chars && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if current.is_empty() {
            current.push_str(word);
            current_len = wlen;
        } else {
            current.push(' ');
            current.push_str(word);
            current_len += wlen + 1;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn font_family(font: &str) -> String {
    format!("{}, Helvetica, Arial, sans-serif", esc(font))
}

fn mime_from_ext(part: &str) -> Option<String> {
    let ext = part.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png".into()),
        "jpg" | "jpeg" => Some("image/jpeg".into()),
        "gif" => Some("image/gif".into()),
        _ => None,
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
    use super::*;
    use crate::fixtures::{DeckSpec, SlideSpec};
    use crate::opc::Package;

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
        let svg = render_slide_svg(&pf, 1, &RenderOptions { embed_images: true }).unwrap();
        assert!(svg.contains("data:image/png;base64,"), "expected data URI");
        assert_no_external_refs(&svg);
    }

    #[test]
    fn image_placeholder_when_not_embedding() {
        let deck = DeckSpec::new("Deck").slide(SlideSpec::new("Pic").image());
        let pf = PresentationFile::from_bytes(&deck.build()).unwrap();
        let svg = render_slide_svg(&pf, 1, &RenderOptions { embed_images: false }).unwrap();
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
}
