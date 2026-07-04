//! Text style model: the paragraph/run property inheritance chain.
//!
//! PowerPoint resolves the effective style of a run by layering, weakest to
//! strongest: presentation `defaultTextStyle` → master `txStyles`
//! (title/body/other) → the master placeholder's `lstStyle` → the layout
//! placeholder's `lstStyle` → the shape's `txBody/lstStyle` → the paragraph
//! `pPr` (+ its `defRPr`) → the run `rPr`. Each layer sets only the properties
//! it specifies; everything else is inherited. We model that as structs of
//! `Option` fields merged down the chain ([`RunProps::overlay`] /
//! [`ParaProps::overlay`]).

use roxmltree::Node;

use super::color::{Rgba, Theme};
use super::{a, ch, EMU_PER_PT};

/// Run-level text properties. Every field is optional so a partial layer (e.g. a
/// run `rPr` that sets only `b="1"`) overlays without clobbering inherited size.
#[derive(Clone, Default)]
pub(crate) struct RunProps {
    pub(crate) size_pt: Option<f64>,
    pub(crate) bold: Option<bool>,
    pub(crate) italic: Option<bool>,
    pub(crate) underline: Option<bool>,
    pub(crate) color: Option<Rgba>,
    /// Resolved typeface (`+mj-lt`/`+mn-lt` already mapped to the theme fonts).
    pub(crate) typeface: Option<String>,
    /// `a:highlight` marker color, drawn as a box behind the run's text.
    pub(crate) highlight: Option<Rgba>,
}

impl RunProps {
    /// Overlay `o` onto `self`: every property `o` specifies wins.
    pub(crate) fn overlay(&mut self, o: &RunProps) {
        if o.size_pt.is_some() {
            self.size_pt = o.size_pt;
        }
        if o.bold.is_some() {
            self.bold = o.bold;
        }
        if o.italic.is_some() {
            self.italic = o.italic;
        }
        if o.underline.is_some() {
            self.underline = o.underline;
        }
        if o.color.is_some() {
            self.color = o.color;
        }
        if o.typeface.is_some() {
            self.typeface = o.typeface.clone();
        }
        if o.highlight.is_some() {
            self.highlight = o.highlight;
        }
    }
}

/// A paragraph bullet, resolved from the style chain (`pPr` strongest).
#[derive(Clone)]
pub(crate) enum Bullet {
    /// `a:buNone` — no bullet.
    None,
    /// `a:buChar` — a literal character (`buFont` typeface, `buSzPct` scale).
    Char { chr: String, font: Option<String>, size_pct: Option<f64> },
    /// `a:buAutoNum` — an auto-incrementing counter.
    AutoNum { typ: String, start: u32, size_pct: Option<f64> },
}

/// A line-spacing or inter-paragraph gap value.
#[derive(Clone, Copy)]
pub(crate) enum Spacing {
    /// `spcPct val` as a fraction (val/100000).
    Pct(f64),
    /// `spcPts val` in points (val/100).
    Pts(f64),
}

/// Paragraph-level properties plus the paragraph's run defaults (`defRPr`).
#[derive(Clone, Default)]
pub(crate) struct ParaProps {
    pub(crate) algn: Option<String>,
    /// Left margin in points (`marL`).
    pub(crate) mar_l: Option<f64>,
    /// First-line indent in points (`indent`, may be negative for hanging).
    pub(crate) indent: Option<f64>,
    pub(crate) bullet: Option<Bullet>,
    pub(crate) ln_spc: Option<Spacing>,
    pub(crate) spc_bef: Option<Spacing>,
    pub(crate) spc_aft: Option<Spacing>,
    pub(crate) def_rpr: RunProps,
}

impl ParaProps {
    /// Overlay `o` onto `self`: paragraph fields and the nested run defaults.
    pub(crate) fn overlay(&mut self, o: &ParaProps) {
        if o.algn.is_some() {
            self.algn = o.algn.clone();
        }
        if o.mar_l.is_some() {
            self.mar_l = o.mar_l;
        }
        if o.indent.is_some() {
            self.indent = o.indent;
        }
        if o.bullet.is_some() {
            self.bullet = o.bullet.clone();
        }
        if o.ln_spc.is_some() {
            self.ln_spc = o.ln_spc;
        }
        if o.spc_bef.is_some() {
            self.spc_bef = o.spc_bef;
        }
        if o.spc_aft.is_some() {
            self.spc_aft = o.spc_aft;
        }
        self.def_rpr.overlay(&o.def_rpr);
    }
}

/// A `lstStyle`-shaped element parsed into per-level (0..=8) paragraph defaults.
/// Covers `p:defaultTextStyle`, the master `p:titleStyle`/`p:bodyStyle`/
/// `p:otherStyle`, and any `a:lstStyle`.
#[derive(Clone, Default)]
pub(crate) struct LstStyle {
    pub(crate) levels: [ParaProps; 9],
}

impl LstStyle {
    /// Parse a `lstStyle`-shaped node. An `a:defPPr` (if present) seeds every
    /// level as its base; each `a:lvlNpPr` refines its level on top.
    pub(crate) fn parse(node: Node, theme: &Theme) -> LstStyle {
        let mut out = LstStyle::default();
        let base = ch(node, "defPPr").map(|p| parse_ppr(p, theme));
        for (i, level) in out.levels.iter_mut().enumerate() {
            let mut lvl = base.clone().unwrap_or_default();
            if let Some(p) = ch(node, &format!("lvl{}pPr", i + 1)) {
                lvl.overlay(&parse_ppr(p, theme));
            }
            *level = lvl;
        }
        out
    }
}

/// Parse `b`/`i`-style boolean attributes (`1`/`true`/`0`/`false`).
fn bool_attr(node: Node, name: &str) -> Option<bool> {
    match a(node, name)? {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

/// Resolve a run's typeface, mapping the `+mj-lt`/`+mn-lt` theme references to
/// the actual major/minor font names. Empty typefaces yield `None`.
fn resolve_typeface(tf: &str, theme: &Theme) -> Option<String> {
    let tf = tf.trim();
    if tf.is_empty() {
        return None;
    }
    Some(match tf {
        "+mj-lt" | "+mj-ea" | "+mj-cs" => theme.major_font.clone(),
        "+mn-lt" | "+mn-ea" | "+mn-cs" => theme.minor_font.clone(),
        other => other.to_string(),
    })
}

/// The representative color of a run fill: `solidFill`, else the first stop of a
/// `gradFill` (document order — a reasonable single-color stand-in for a
/// gradient run).
fn run_fill_color(rpr: Node, theme: &Theme) -> Option<Rgba> {
    if let Some(sf) = ch(rpr, "solidFill") {
        return sf
            .children()
            .find(|n| n.is_element())
            .and_then(|cn| theme.parse_color(cn));
    }
    if let Some(gf) = ch(rpr, "gradFill") {
        return ch(gf, "gsLst")
            .and_then(|l| l.children().find(|n| n.is_element() && n.tag_name().name() == "gs"))
            .and_then(|gs| gs.children().find(|n| n.is_element()))
            .and_then(|cn| theme.parse_color(cn));
    }
    None
}

/// Parse an `a:rPr`/`a:defRPr`/`a:endParaRPr` into partial [`RunProps`].
pub(crate) fn parse_rpr(rpr: Node, theme: &Theme) -> RunProps {
    let size_pt = a(rpr, "sz").and_then(|v| v.parse::<f64>().ok()).map(|v| v / 100.0);
    let underline = a(rpr, "u").map(|u| u != "none");
    let typeface = ch(rpr, "latin")
        .and_then(|l| a(l, "typeface"))
        .and_then(|tf| resolve_typeface(tf, theme));
    RunProps {
        size_pt,
        bold: bool_attr(rpr, "b"),
        italic: bool_attr(rpr, "i"),
        underline,
        color: run_fill_color(rpr, theme),
        typeface,
        highlight: ch(rpr, "highlight")
            .and_then(|h| h.children().find(|n| n.is_element()))
            .and_then(|c| theme.parse_color(c)),
    }
}

/// Parse an `a:pPr`/`a:lvlNpPr`/`a:defPPr` into [`ParaProps`].
pub(crate) fn parse_ppr(ppr: Node, theme: &Theme) -> ParaProps {
    let emu_pt = |name: &str| {
        a(ppr, name).and_then(|v| v.parse::<f64>().ok()).map(|v| v / EMU_PER_PT)
    };
    ParaProps {
        algn: a(ppr, "algn").map(|s| s.to_string()),
        mar_l: emu_pt("marL"),
        indent: emu_pt("indent"),
        bullet: parse_bullet(ppr),
        ln_spc: ch(ppr, "lnSpc").and_then(parse_spacing),
        spc_bef: ch(ppr, "spcBef").and_then(parse_spacing),
        spc_aft: ch(ppr, "spcAft").and_then(parse_spacing),
        def_rpr: ch(ppr, "defRPr").map(|r| parse_rpr(r, theme)).unwrap_or_default(),
    }
}

/// A spacing container (`lnSpc`/`spcBef`/`spcAft`) → [`Spacing`].
fn parse_spacing(node: Node) -> Option<Spacing> {
    if let Some(p) = ch(node, "spcPct").and_then(|n| a(n, "val")).and_then(|v| v.parse::<f64>().ok())
    {
        return Some(Spacing::Pct(p / 100_000.0));
    }
    if let Some(p) = ch(node, "spcPts").and_then(|n| a(n, "val")).and_then(|v| v.parse::<f64>().ok())
    {
        return Some(Spacing::Pts(p / 100.0));
    }
    None
}

/// Resolve a paragraph's bullet directive from its `pPr` children.
fn parse_bullet(ppr: Node) -> Option<Bullet> {
    let size_pct = ch(ppr, "buSzPct")
        .and_then(|n| a(n, "val"))
        .and_then(|v| v.parse::<f64>().ok())
        .map(|v| v / 100_000.0);
    for child in ppr.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "buNone" => return Some(Bullet::None),
            "buChar" => {
                let chr = a(child, "char").unwrap_or("\u{2022}").to_string();
                let font = ch(ppr, "buFont").and_then(|f| a(f, "typeface")).map(|s| s.to_string());
                return Some(Bullet::Char { chr, font, size_pct });
            }
            "buAutoNum" => {
                let typ = a(child, "type").unwrap_or("arabicPeriod").to_string();
                let start = a(child, "startAt").and_then(|v| v.parse::<u32>().ok()).unwrap_or(1);
                return Some(Bullet::AutoNum { typ, start, size_pct });
            }
            _ => {}
        }
    }
    None
}
