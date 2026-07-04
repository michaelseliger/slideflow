//! Fill and stroke resolution for shapes, plus slide/layout/master background.

use roxmltree::{Document, Node};

use super::color::{Rgba, Theme};
use super::{a, ch, fnum, Ctx, EMU_PER_PT};

pub(crate) enum Fill {
    Solid(Rgba),
    None,
    Unspecified,
}

impl Fill {
    pub(crate) fn svg_attrs(&self) -> String {
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

pub(crate) struct Stroke {
    pub(crate) color: Rgba,
    pub(crate) width_pt: f64,
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
                    return ch(child, "gsLst")
                        .and_then(|l| ch(l, "gs"))
                        .and_then(|gs| gs.children().find(|n| n.is_element()))
                        .and_then(|cn| theme.parse_color(cn))
                        .map(Fill::Solid);
                }
                "noFill" => return Some(Fill::None),
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
}
