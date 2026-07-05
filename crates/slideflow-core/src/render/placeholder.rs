//! Placeholder discovery and geometry-inheritance matching (layout → master).

use roxmltree::{Document, Node};

use super::color::Theme;
use super::geometry::{parse_xfrm, Xfrm};
use super::style::LstStyle;
use super::{a, ch};

#[derive(Clone)]
pub(crate) struct Placeholder {
    pub(crate) typ: Option<String>,
    pub(crate) idx: Option<String>,
    pub(crate) xfrm: Option<Xfrm>,
    /// The placeholder's raw `a:custGeom`/`a:prstGeom` XML — inherited by
    /// slide pictures alongside the xfrm (templates cut photo placeholders
    /// into diagonals via custGeom). Only layout/master placeholders carry it.
    pub(crate) geom_xml: Option<String>,
    /// The placeholder's own `txBody/lstStyle`, parsed for the style chain.
    /// Empty for slide-shape placeholders (only layout/master ones carry style).
    pub(crate) text_styles: LstStyle,
}

/// Read the `p:ph` of an `sp`/`pic`, if it is a placeholder.
pub(crate) fn shape_placeholder(node: Node) -> Option<Placeholder> {
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
        geom_xml: None,
        text_styles: LstStyle::default(),
    })
}

pub(crate) fn collect_placeholders(doc: &Document, theme: &Theme) -> Vec<Placeholder> {
    let mut out = Vec::new();
    let Some(tree) = ch(doc.root_element(), "cSld").and_then(|c| ch(c, "spTree")) else {
        return out;
    };
    collect_placeholders_in(tree, theme, &mut out);
    out
}

fn collect_placeholders_in(tree: Node, theme: &Theme, out: &mut Vec<Placeholder>) {
    for shape in tree.children().filter(|n| n.is_element()) {
        match shape.tag_name().name() {
            "sp" | "pic" => {
                if let Some(mut ph) = shape_placeholder(shape) {
                    if let Some(sp_pr) = ch(shape, "spPr") {
                        ph.xfrm = ch(sp_pr, "xfrm").map(parse_xfrm);
                        // Keep the geometry as raw XML: the source Document
                        // doesn't outlive collection, so slice its input text.
                        ph.geom_xml = ch(sp_pr, "custGeom")
                            .or_else(|| ch(sp_pr, "prstGeom"))
                            .map(|g| g.document().input_text()[g.range()].to_string());
                    }
                    if let Some(lst) = ch(shape, "txBody").and_then(|t| ch(t, "lstStyle")) {
                        ph.text_styles = LstStyle::parse(lst, theme);
                    }
                    out.push(ph);
                }
            }
            "grpSp" => collect_placeholders_in(shape, theme, out),
            _ => {}
        }
    }
}

/// Match by (type+idx), then idx, then type — per the contract.
pub(crate) fn match_placeholder<'a>(src: &'a [Placeholder], want: &Placeholder) -> Option<&'a Placeholder> {
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
