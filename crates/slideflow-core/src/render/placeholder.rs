//! Placeholder discovery and geometry-inheritance matching (layout → master).

use roxmltree::{Document, Node};

use super::color::Theme;
use super::geometry::{parse_xfrm, Xfrm};
use super::{a, ch};

#[derive(Clone)]
pub(crate) struct Placeholder {
    pub(crate) typ: Option<String>,
    pub(crate) idx: Option<String>,
    pub(crate) xfrm: Option<Xfrm>,
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
    })
}

pub(crate) fn collect_placeholders(doc: &Document, _theme: &Theme) -> Vec<Placeholder> {
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
