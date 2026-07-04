//! Fill and stroke resolution for shapes, plus slide/layout/master background.

use roxmltree::{Document, Node};

use super::color::{Rgba, Theme};
use super::{a, ch, fnum, Ctx, EMU_PER_PT};

/// Namespace-carrying wrapper so a raw `fmtScheme` template substring (sliced
/// out of the theme, without its inherited `xmlns:a`) re-parses on its own.
const A_NS: &str = "http://schemas.openxmlformats.org/drawingml/2006/main";

pub(crate) enum GradKind {
    /// `angle_deg` is measured clockwise from due east (DrawingML `a:lin@ang`).
    Linear { angle_deg: f64 },
    Radial,
}

pub(crate) enum Fill {
    Solid(Rgba),
    Gradient { stops: Vec<(f64, Rgba)>, kind: GradKind },
    None,
    Unspecified,
}

impl Fill {
    /// Static fill attribute for the non-gradient cases. Gradients need
    /// [`Ctx::fill_attrs`] (they intern a `<defs>` entry); called here they
    /// degrade to their first stop as a flat color.
    pub(crate) fn svg_attrs(&self) -> String {
        match self {
            Fill::Solid(c) => solid_attr(c),
            Fill::Gradient { stops, .. } => match stops.first() {
                Some((_, c)) => solid_attr(c),
                None => r#" fill="none""#.to_string(),
            },
            _ => r#" fill="none""#.to_string(),
        }
    }
}

fn solid_attr(c: &Rgba) -> String {
    if c.a < 0.999 {
        format!(r#" fill="{}" fill-opacity="{}""#, c.hex(), fnum(c.a))
    } else {
        format!(r#" fill="{}""#, c.hex())
    }
}

pub(crate) struct Stroke {
    pub(crate) color: Rgba,
    pub(crate) width_pt: f64,
}

/// Parse an `a:gradFill` element into a real gradient (or `Unspecified` if it
/// has no usable stops). `ph` supplies the `phClr` placeholder color when the
/// gradient comes from an `fmtScheme` style template.
fn parse_gradient(grad: Node, theme: &Theme, ph: Option<Rgba>) -> Fill {
    let mut stops: Vec<(f64, Rgba)> = Vec::new();
    if let Some(gs_lst) = ch(grad, "gsLst") {
        for gs in gs_lst
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "gs")
        {
            let pos = a(gs, "pos")
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| (v / 100000.0).clamp(0.0, 1.0))
                .unwrap_or(0.0);
            if let Some(c) = gs
                .children()
                .find(|n| n.is_element())
                .and_then(|cn| theme.parse_color_ph(cn, ph))
            {
                stops.push((pos, c));
            }
        }
    }
    if stops.is_empty() {
        return Fill::Unspecified;
    }
    stops.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal));
    let kind = if let Some(lin) = ch(grad, "lin") {
        let ang = a(lin, "ang")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 60000.0)
            .unwrap_or(0.0);
        GradKind::Linear { angle_deg: ang }
    } else if ch(grad, "path").is_some() {
        GradKind::Radial
    } else {
        GradKind::Linear { angle_deg: 0.0 }
    };
    Fill::Gradient { stops, kind }
}

/// Parse a fill *element* (`solidFill`/`gradFill`/`noFill`) with an optional
/// `phClr` substitution — used for `fmtScheme` style templates.
fn parse_fill_element(node: Node, theme: &Theme, ph: Option<Rgba>) -> Fill {
    match node.tag_name().name() {
        "solidFill" => node
            .children()
            .find(|n| n.is_element())
            .and_then(|cn| theme.parse_color_ph(cn, ph))
            .map(Fill::Solid)
            .unwrap_or(Fill::Unspecified),
        "noFill" => Fill::None,
        "gradFill" => parse_gradient(node, theme, ph),
        _ => Fill::Unspecified,
    }
}

/// Resolve a `fmtScheme` fill/bgFill template (raw XML) into a concrete `Fill`,
/// substituting `phClr` with `ph`.
fn resolve_template_fill(template: &str, theme: &Theme, ph: Option<Rgba>) -> Fill {
    let wrapped = format!(r#"<sf xmlns:a="{A_NS}">{template}</sf>"#);
    let Ok(doc) = Document::parse(&wrapped) else { return Fill::Unspecified };
    match doc.root_element().children().find(|n| n.is_element()) {
        Some(node) => parse_fill_element(node, theme, ph),
        None => Fill::Unspecified,
    }
}

/// Resolve a `lnStyleLst` template (raw `<a:ln>` XML) into a `Stroke`, using the
/// pre-parsed `width_pt` and substituting `phClr` with `ph`.
fn resolve_template_stroke(
    template: &str,
    theme: &Theme,
    width_pt: f64,
    ph: Rgba,
) -> Option<Stroke> {
    let wrapped = format!(r#"<sf xmlns:a="{A_NS}">{template}</sf>"#);
    let doc = Document::parse(&wrapped).ok()?;
    let ln = doc.root_element().children().find(|n| n.is_element())?;
    if ln.children().any(|n| n.is_element() && n.tag_name().name() == "noFill") {
        return None;
    }
    let color = ch(ln, "solidFill")
        .and_then(|f| f.children().find(|n| n.is_element()))
        .and_then(|cn| theme.parse_color_ph(cn, Some(ph)))?;
    Some(Stroke { color, width_pt })
}

pub(crate) fn collect_background(doc: &Document, theme: &Theme) -> Option<Fill> {
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
                    return match parse_gradient(child, theme, None) {
                        Fill::Unspecified => None,
                        f => Some(f),
                    };
                }
                "noFill" => return Some(Fill::None),
                _ => {}
            }
        }
    }
    if let Some(bg_ref) = ch(bg, "bgRef") {
        let ph = bg_ref
            .children()
            .find(|n| n.is_element())
            .and_then(|cn| theme.parse_color(cn));
        // idx >= 1001 → bgFillStyleLst[idx-1001]; idx 1000 clamps to the first.
        if let Some(idx) = a(bg_ref, "idx").and_then(|v| v.parse::<usize>().ok()) {
            if idx >= 1000 && !theme.bg_fill_styles.is_empty() {
                let i = idx.saturating_sub(1001).min(theme.bg_fill_styles.len() - 1);
                return Some(resolve_template_fill(&theme.bg_fill_styles[i], theme, ph));
            }
        }
        // No usable style list: treat the bgRef color as a plain solid fill.
        return ph.map(Fill::Solid);
    }
    None
}

impl Ctx<'_> {
    pub(crate) fn resolve_fill(&self, sp_pr: Node) -> Fill {
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
                "noFill" => return Fill::None,
                "gradFill" => return parse_gradient(child, &self.theme, None),
                "pattFill" | "blipFill" => return Fill::Unspecified,
                _ => {}
            }
        }
        // No direct fill child: leave it to the caller to try a style reference.
        Fill::Unspecified
    }

    pub(crate) fn resolve_stroke(&self, sp_pr: Node) -> Option<Stroke> {
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

    /// Resolve a shape's `p:style/a:fillRef` into a `Fill` (used when `spPr`
    /// carries no explicit fill). idx 0 → no fill; 1..=999 → `fillStyleLst`;
    /// >=1000 → `bgFillStyleLst`. The `fillRef`'s color child is the `phClr`.
    pub(crate) fn resolve_style_fill(&self, sp: Node) -> Option<Fill> {
        let fill_ref = ch(ch(sp, "style")?, "fillRef")?;
        let idx = a(fill_ref, "idx").and_then(|v| v.parse::<usize>().ok())?;
        if idx == 0 {
            return Some(Fill::None);
        }
        let ph = fill_ref
            .children()
            .find(|n| n.is_element())
            .and_then(|cn| self.theme.parse_color(cn));
        let template = if idx >= 1000 {
            self.theme.bg_fill_styles.get(idx - 1000)?
        } else {
            self.theme.fill_styles.get(idx - 1)?
        };
        Some(resolve_template_fill(template, &self.theme, ph))
    }

    /// Resolve a shape's `p:style/a:lnRef` into a `Stroke`. idx 0 → none;
    /// 1..=N → `lnStyleLst[idx-1]` (width from the template, color from the
    /// `lnRef`'s color child as `phClr`).
    pub(crate) fn resolve_style_stroke(&self, sp: Node) -> Option<Stroke> {
        let ln_ref = ch(ch(sp, "style")?, "lnRef")?;
        let idx = a(ln_ref, "idx").and_then(|v| v.parse::<usize>().ok())?;
        if idx == 0 {
            return None;
        }
        let ph = ln_ref
            .children()
            .find(|n| n.is_element())
            .and_then(|cn| self.theme.parse_color(cn))?;
        let (width_pt, template) = self.theme.line_styles.get(idx - 1)?;
        resolve_template_stroke(template, &self.theme, *width_pt, ph)
    }

    /// Fill attribute string, interning a gradient `<defs>` entry when needed.
    /// Identical gradients are deduplicated by a serialized key (layouts repeat
    /// them heavily). Non-gradient fills defer to [`Fill::svg_attrs`].
    pub(crate) fn fill_attrs(&mut self, fill: &Fill) -> String {
        let Fill::Gradient { stops, kind } = fill else {
            return fill.svg_attrs();
        };
        if stops.is_empty() {
            return r#" fill="none""#.to_string();
        }
        let key = grad_key(stops, kind);
        let id = if let Some(existing) = self.grad_cache.get(&key) {
            existing.clone()
        } else {
            let id = format!("grad{}", self.grad_id);
            self.grad_id += 1;
            let def = build_gradient_def(&id, stops, kind);
            self.defs.push_str(&def);
            self.grad_cache.insert(key, id.clone());
            id
        };
        format!(r#" fill="url(#{id})""#)
    }
}

/// Serialized identity of a gradient for `<defs>` deduplication.
fn grad_key(stops: &[(f64, Rgba)], kind: &GradKind) -> String {
    let mut k = match kind {
        GradKind::Linear { angle_deg } => format!("L{:.2};", angle_deg),
        GradKind::Radial => "R;".to_string(),
    };
    for (pos, c) in stops {
        k.push_str(&format!("{:.4}={}@{:.3};", pos, c.hex(), c.a));
    }
    k
}

/// Emit a `<linearGradient>`/`<radialGradient>` on `objectBoundingBox`. The
/// angle maps to endpoints via x1=0.5-0.5·cos θ, y1=0.5-0.5·sin θ (and +… for
/// the far end): θ=0 runs left→right (due east), and because SVG's y-axis points
/// down, positive θ sweeps clockwise — matching DrawingML's `a:lin@ang`.
fn build_gradient_def(id: &str, stops: &[(f64, Rgba)], kind: &GradKind) -> String {
    let stops_svg: String = stops
        .iter()
        .map(|(pos, c)| {
            if c.a < 0.999 {
                format!(
                    r#"<stop offset="{}" stop-color="{}" stop-opacity="{}"/>"#,
                    fnum(*pos),
                    c.hex(),
                    fnum(c.a)
                )
            } else {
                format!(r#"<stop offset="{}" stop-color="{}"/>"#, fnum(*pos), c.hex())
            }
        })
        .collect();
    match kind {
        GradKind::Linear { angle_deg } => {
            let theta = angle_deg.to_radians();
            let (cos, sin) = (theta.cos(), theta.sin());
            format!(
                r#"<linearGradient id="{id}" gradientUnits="objectBoundingBox" x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}">{stops_svg}</linearGradient>"#,
                x1 = fnum(0.5 - 0.5 * cos),
                y1 = fnum(0.5 - 0.5 * sin),
                x2 = fnum(0.5 + 0.5 * cos),
                y2 = fnum(0.5 + 0.5 * sin),
            )
        }
        GradKind::Radial => format!(
            r#"<radialGradient id="{id}" gradientUnits="objectBoundingBox" cx="0.5" cy="0.5" r="0.5">{stops_svg}</radialGradient>"#
        ),
    }
}
