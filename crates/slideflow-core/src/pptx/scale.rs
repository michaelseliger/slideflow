//! Uniform slide scaling for mixed-dimension composition.
//!
//! When slides picked from decks of different canvas sizes are composed into one
//! output (whose size is taken from the first picked deck), the foreign slides
//! must be rescaled so they fit the shared canvas instead of keeping their raw
//! absolute geometry. [`SlideScale`] captures a single uniform rational scale
//! factor plus a top-level translation (the letterbox/overflow offset), and
//! [`scale_part_xml`] rewrites a slide/layout/master part's geometry in place.
//!
//! Everything is deterministic integer math (i128 intermediates, round-half-up),
//! so the same inputs always produce byte-identical output.
//!
//! ## What gets rewritten
//!
//! The rewrite is **element-scoped**: an attribute is only touched when it sits
//! on an element known to carry that dimension. We never match a bare attribute
//! name, because the same spelling means different things on different elements
//! (`sz` is a font size on `a:rPr` but the enum `quarter`/`half` on `p:ph`).
//!
//! EMU (English Metric Units) attributes, scaled by the factor:
//! - `a:off`/`a:chOff` x,y · `a:ext`/`a:chExt` cx,cy (every `p:xfrm`/`p:spPr`)
//! - line widths: `a:ln` w and cell borders `a:lnL/lnR/lnT/lnB/lnTlToBr/lnBlToTr` w
//! - table metrics: `a:gridCol` w · `a:tr` h · `a:tcPr` marL/marR/marT/marB
//! - text-frame insets: `a:bodyPr` lIns/tIns/rIns/bIns
//! - paragraph metrics: `a:pPr`/`a:defPPr`/`a:lvl1pPr`…`a:lvl9pPr`
//!   marL/marR/indent/defTabSz · `a:tab` pos
//! - effects: `a:outerShdw`/`a:innerShdw`/`a:prstShdw` dist,blurRad · `a:glow`
//!   rad · `a:softEdge` rad · `a:reflection` blurRad,dist · `a:tile` tx,ty
//!
//! Hundredths-of-a-point attributes, scaled by the same factor:
//! - `a:rPr`/`a:defRPr`/`a:endParaRPr` sz (clamped to ≥ 100 = 1pt), kern, spc
//! - `a:spcPts` val · `a:buSzPts` val
//!
//! The top-level translation (`dx`,`dy`) is added to `a:off` x/y **only** for
//! shapes that are direct children of `p:spTree` (tracked via group depth);
//! nested group members receive scale but no translation.
//!
//! ## What is deliberately left untouched
//!
//! - `rot`/`flipH`/`flipV` — rotation/flip are size-independent.
//! - custom geometry `a:path` w/h and `a:pt` x/y — shape-local coordinates that
//!   are already mapped through the (now scaled) `a:ext`; rewriting them would
//!   double-scale.
//! - all percent-denominated attributes: `a:normAutofit` fontScale/lnSpcReduction,
//!   `a:spcPct`, `a:buSzPct`, srcRect/fillRect insets, `a:tile`/reflection sx,sy,
//!   miter `lim` — percentages are already scale-invariant.
//! - `p:ph sz` (placeholder size enum) — the reason `sz` matching is element-scoped.
//! - line-end `a:headEnd`/`a:tailEnd` w/len — enum sizes, not measurements.
//! - chart/diagram/embedded-part internals — separate parts, copied verbatim.
//! - any value that fails to parse as an `i64` is left exactly as found.
//!
//! Documented v1 exclusions (measurements we could scale but currently do not):
//! `a:sp3d`/bevel EMU depths, `p:oleObj` imgW/imgH, and `p:timing` durations.

use std::io::Cursor;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};

use crate::error::{Error, Result};
use crate::model::FitMode;
use crate::opc::local_name;

/// A uniform rational scale (`num`/`den`) plus a top-level translation in EMU.
///
/// `scale(v) = round_half_up(v * num / den)`. The translation offsets the whole
/// slide so scaled content is centered on the output canvas (letterboxed for
/// [`FitMode::EnsureFit`], symmetrically overflowing for [`FitMode::Maximize`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlideScale {
    pub num: i64,
    pub den: i64,
    pub dx: i64,
    pub dy: i64,
}

impl SlideScale {
    /// Scale a single value: `round_half_up(v * num / den)` with i128
    /// intermediates. `den` is always positive, so `div_euclid` gives the floor
    /// division that makes `(v·num + den/2)` a true round-half-up (ties toward
    /// +∞) for negative values too (e.g. tracking `spc`), not just positives.
    #[inline]
    pub fn scale(&self, v: i64) -> i64 {
        let num = self.num as i128;
        let den = self.den as i128;
        (((v as i128) * num + den / 2).div_euclid(den)) as i64
    }

    /// The scale factor as a truncated whole percent (e.g. 3/4 → 75, 4/3 → 133).
    pub fn percent(&self) -> i64 {
        ((100i128 * self.num as i128) / self.den as i128) as i64
    }

    /// Compute the scale that maps a `src` canvas onto a `dst` canvas.
    ///
    /// Returns `None` when the canvases are identical (or degenerate). The
    /// binding axis is kept as an *exact* rational so the along-axis dimension
    /// maps precisely (its translation is 0). `mode` only matters for genuine
    /// aspect mismatches; for a same-aspect resize both modes agree.
    pub fn compute(src: (i64, i64), dst: (i64, i64), mode: FitMode) -> Option<SlideScale> {
        let (sw, sh) = src;
        let (tw, th) = dst;
        if sw <= 0 || sh <= 0 || tw <= 0 || th <= 0 {
            return None;
        }
        if sw == tw && sh == th {
            return None;
        }
        // Compare tw/sw against th/sh via cross-multiplication (exact).
        let tw_sh = (tw as i128) * (sh as i128);
        let th_sw = (th as i128) * (sw as i128);
        let (num, den) = match mode {
            // EnsureFit → the smaller ratio binds (fit inside, letterbox).
            FitMode::EnsureFit => {
                if tw_sh <= th_sw {
                    (tw, sw)
                } else {
                    (th, sh)
                }
            }
            // Maximize → the larger ratio binds (fill, overflow the other axis).
            FitMode::Maximize => {
                if tw_sh >= th_sw {
                    (tw, sw)
                } else {
                    (th, sh)
                }
            }
        };
        let partial = SlideScale { num, den, dx: 0, dy: 0 };
        let dx = (tw - partial.scale(sw)) / 2;
        let dy = (th - partial.scale(sh)) / 2;
        Some(SlideScale { num, den, dx, dy })
    }
}

/// Whether `src` and `dst` share the same aspect ratio within a tight epsilon:
/// `|sw·th − tw·sh| ≤ 0.001·tw·sh`. Same-aspect mismatches are auto-scaled and
/// never need a fit-mode choice.
pub fn is_same_aspect(src: (i64, i64), dst: (i64, i64)) -> bool {
    let (sw, sh) = src;
    let (tw, th) = dst;
    if tw <= 0 || sh <= 0 {
        return false;
    }
    let lhs = ((sw as i128) * (th as i128) - (tw as i128) * (sh as i128)).abs();
    let rhs = (tw as i128) * (sh as i128);
    lhs * 1000 <= rhs
}

/// Which attributes on an element scale, and in what unit.
enum Rule {
    None,
    /// EMU-denominated attribute names.
    Emu(&'static [&'static str]),
    /// Hundredths-of-a-point attribute names (`sz` is additionally clamped ≥ 100).
    Pt(&'static [&'static str]),
}

/// Element-scoped scaling rule. NEVER keyed on a bare attribute name.
fn attr_rule(local: &[u8]) -> Rule {
    match local {
        b"off" | b"chOff" => Rule::Emu(&["x", "y"]),
        b"ext" | b"chExt" => Rule::Emu(&["cx", "cy"]),
        b"ln" | b"lnL" | b"lnR" | b"lnT" | b"lnB" | b"lnTlToBr" | b"lnBlToTr" => {
            Rule::Emu(&["w"])
        }
        b"gridCol" => Rule::Emu(&["w"]),
        b"tr" => Rule::Emu(&["h"]),
        b"tcPr" => Rule::Emu(&["marL", "marR", "marT", "marB"]),
        b"bodyPr" => Rule::Emu(&["lIns", "tIns", "rIns", "bIns"]),
        b"pPr" | b"defPPr" => Rule::Emu(&["marL", "marR", "indent", "defTabSz"]),
        b"tab" => Rule::Emu(&["pos"]),
        b"outerShdw" | b"innerShdw" | b"prstShdw" => Rule::Emu(&["dist", "blurRad"]),
        b"glow" | b"softEdge" => Rule::Emu(&["rad"]),
        b"reflection" => Rule::Emu(&["blurRad", "dist"]),
        b"tile" => Rule::Emu(&["tx", "ty"]),
        b"rPr" | b"defRPr" | b"endParaRPr" => Rule::Pt(&["sz", "kern", "spc"]),
        b"spcPts" | b"buSzPts" => Rule::Pt(&["val"]),
        _ if is_lvl_ppr(local) => Rule::Emu(&["marL", "marR", "indent", "defTabSz"]),
        _ => Rule::None,
    }
}

/// `a:lvl1pPr` … `a:lvl9pPr`.
fn is_lvl_ppr(local: &[u8]) -> bool {
    local.len() == 7
        && local.starts_with(b"lvl")
        && local.ends_with(b"pPr")
        && matches!(local[3], b'1'..=b'9')
}

/// Shape containers that are (potential) direct children of `p:spTree`. Entering
/// one increments the group depth; only depth-1 shapes receive the translation.
fn is_shape_container(local: &[u8]) -> bool {
    matches!(local, b"sp" | b"grpSp" | b"pic" | b"cxnSp" | b"graphicFrame")
}

/// Rewrite a slide/layout/master part's geometry for `sc`.
///
/// Elements outside the scaling table pass through byte-for-byte; only touched
/// elements are re-serialized (their attributes are all simple numerics/enums,
/// so escaping round-trips cleanly).
pub fn scale_part_xml(bytes: &[u8], sc: &SlideScale) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(bytes);
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();
    let mut in_sptree = false;
    let mut shape_depth: i32 = 0;

    loop {
        let ev = reader.read_event_into(&mut buf).map_err(|e| Error::xml("scale", e))?;
        match ev {
            Event::Start(ref e) => {
                let local = local_name(e.name().as_ref()).to_vec();
                let translate = in_sptree && shape_depth == 1;
                write_scaled(&mut writer, e, sc, &local, translate, false)?;
                if local == b"spTree" {
                    in_sptree = true;
                }
                if is_shape_container(&local) {
                    shape_depth += 1;
                }
            }
            Event::Empty(ref e) => {
                let local = local_name(e.name().as_ref()).to_vec();
                let translate = in_sptree && shape_depth == 1;
                write_scaled(&mut writer, e, sc, &local, translate, true)?;
                // Self-closing: no depth change.
            }
            Event::End(ref e) => {
                let local = local_name(e.name().as_ref()).to_vec();
                if is_shape_container(&local) && shape_depth > 0 {
                    shape_depth -= 1;
                }
                if local == b"spTree" {
                    in_sptree = false;
                }
                writer.write_event(Event::End(e.clone())).map_err(|x| Error::xml("scale", x))?;
            }
            Event::Eof => break,
            other => {
                writer.write_event(other).map_err(|x| Error::xml("scale", x))?;
            }
        }
        buf.clear();
    }
    Ok(writer.into_inner().into_inner())
}

/// Write a Start/Empty event, rewriting scalable attributes if the element has a
/// rule and any attribute actually changes; otherwise pass the original through.
fn write_scaled(
    writer: &mut Writer<Cursor<Vec<u8>>>,
    e: &BytesStart,
    sc: &SlideScale,
    local: &[u8],
    translate: bool,
    empty: bool,
) -> Result<()> {
    let ev = match rewrite_attrs(e, sc, local, translate) {
        Some(new) if empty => Event::Empty(new),
        Some(new) => Event::Start(new),
        None if empty => Event::Empty(e.clone()),
        None => Event::Start(e.clone()),
    };
    writer.write_event(ev).map_err(|x| Error::xml("scale", x))
}

/// Rebuild an element's attributes with the scalable ones replaced. Returns
/// `None` (→ pass original through verbatim) when nothing changes or the element
/// can't be safely reconstructed.
fn rewrite_attrs(
    e: &BytesStart,
    sc: &SlideScale,
    local: &[u8],
    translate: bool,
) -> Option<BytesStart<'static>> {
    let rule = attr_rule(local);
    let attrs = match &rule {
        Rule::None => return None,
        Rule::Emu(a) | Rule::Pt(a) => *a,
    };
    let is_pt = matches!(rule, Rule::Pt(_));

    let name = e.name();
    let name_str = std::str::from_utf8(name.as_ref()).ok()?;
    let mut new = BytesStart::new(name_str.to_owned());
    let mut changed = false;

    for attr in e.attributes() {
        let attr = attr.ok()?; // malformed attr → bail, pass original through
        let key = attr.key.as_ref();
        let key_str = std::str::from_utf8(key).ok()?;
        let val = attr.unescape_value().ok()?;

        if attrs.iter().any(|a| a.as_bytes() == key) {
            if let Ok(v) = val.parse::<i64>() {
                let mut scaled = sc.scale(v);
                if is_pt && key == b"sz" && scaled < 100 {
                    scaled = 100;
                }
                if translate && local == b"off" {
                    if key == b"x" {
                        scaled += sc.dx;
                    } else if key == b"y" {
                        scaled += sc.dy;
                    }
                }
                new.push_attribute((key_str, scaled.to_string().as_str()));
                changed = true;
                continue;
            }
        }
        new.push_attribute((key_str, val.as_ref()));
    }

    if changed {
        Some(new)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16:9 → same shape, 4/3 up-scale (both used across cases below).
    fn s43() -> SlideScale {
        // num/den = 4/3, no translation.
        SlideScale { num: 4, den: 3, dx: 0, dy: 0 }
    }

    fn scaled(xml: &str, sc: &SlideScale) -> String {
        String::from_utf8(scale_part_xml(xml.as_bytes(), sc).unwrap()).unwrap()
    }

    // (1) off/ext/chOff/chExt scaled; rot preserved.
    #[test]
    fn geometry_scaled_rot_preserved() {
        let xml = r#"<p:sp><p:spPr><a:xfrm rot="5400000"><a:off x="300" y="600"/><a:ext cx="900" cy="1200"/><a:chOff x="30" y="60"/><a:chExt cx="90" cy="120"/></a:xfrm></p:spPr></p:sp>"#;
        let out = scaled(xml, &s43());
        assert!(out.contains(r#"rot="5400000""#), "rot must be preserved: {out}");
        assert!(out.contains(r#"<a:off x="400" y="800"/>"#), "{out}");
        assert!(out.contains(r#"<a:ext cx="1200" cy="1600"/>"#), "{out}");
        assert!(out.contains(r#"<a:chOff x="40" y="80"/>"#), "{out}");
        assert!(out.contains(r#"<a:chExt cx="120" cy="160"/>"#), "{out}");
    }

    // (2) nested group — children scaled, dx/dy only at spTree level.
    #[test]
    fn nested_group_translation_only_top_level() {
        let sc = SlideScale { num: 1, den: 1, dx: 1000, dy: 2000 };
        let xml = r#"<p:spTree><p:nvGrpSpPr/><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr><p:grpSp><p:grpSpPr><a:xfrm><a:off x="100" y="100"/><a:ext cx="500" cy="500"/><a:chOff x="0" y="0"/><a:chExt cx="500" cy="500"/></a:xfrm></p:grpSpPr><p:sp><p:spPr><a:xfrm><a:off x="10" y="20"/><a:ext cx="50" cy="60"/></a:xfrm></p:spPr></p:sp></p:grpSp></p:spTree>"#;
        let out = scaled(xml, &sc);
        // spTree's own grpSpPr xfrm (identity) is depth 0 → no translate.
        assert!(out.contains(r#"<p:grpSpPr><a:xfrm><a:off x="0" y="0"/>"#), "{out}");
        // Top-level group's own off is translated.
        assert!(out.contains(r#"<a:off x="1100" y="2100"/>"#), "group off translated: {out}");
        // Nested shape's off is scaled only (s=1) — NOT translated.
        assert!(out.contains(r#"<a:off x="10" y="20"/>"#), "nested off not translated: {out}");
    }

    // (3) rPr-family sz/kern/spc scaled + clamp; p:ph sz="quarter" untouched.
    #[test]
    fn rpr_scaled_ph_size_untouched() {
        let half = SlideScale { num: 1, den: 2, dx: 0, dy: 0 };
        let xml = r#"<p:sp><p:nvSpPr><p:nvPr><p:ph type="body" sz="quarter"/></p:nvPr></p:nvSpPr><p:txBody><a:p><a:r><a:rPr sz="150" kern="1200" spc="-40"/></a:r><a:endParaRPr sz="1800"/></a:p></p:txBody></p:sp>"#;
        let out = scaled(xml, &half);
        assert!(out.contains(r#"<p:ph type="body" sz="quarter"/>"#), "ph sz enum untouched: {out}");
        // sz 150 → 75 → clamped to 100.
        assert!(out.contains(r#"sz="100""#), "sz clamped ≥100: {out}");
        assert!(out.contains(r#"kern="600""#), "{out}");
        assert!(out.contains(r#"spc="-20""#), "{out}");
        assert!(out.contains(r#"<a:endParaRPr sz="900"/>"#), "{out}");
    }

    // (4) table gridCol/tr/tcPr margins/cell-border w.
    #[test]
    fn table_metrics_scaled() {
        let xml = r#"<a:tbl><a:tblGrid><a:gridCol w="3000"/></a:tblGrid><a:tr h="600"><a:tc><a:tcPr marL="90" marR="90" marT="45" marB="45"><a:lnL w="120"/><a:lnBlToTr w="240"/></a:tcPr></a:tc></a:tr></a:tbl>"#;
        let out = scaled(xml, &s43());
        assert!(out.contains(r#"<a:gridCol w="4000"/>"#), "{out}");
        assert!(out.contains(r#"<a:tr h="800">"#), "{out}");
        assert!(out.contains(r#"marL="120" marR="120" marT="60" marB="60""#), "{out}");
        assert!(out.contains(r#"<a:lnL w="160"/>"#), "{out}");
        assert!(out.contains(r#"<a:lnBlToTr w="320"/>"#), "{out}");
    }

    // (5) spcPts/buSzPts scaled; spcPct/buSzPct/normAutofit untouched.
    #[test]
    fn points_scaled_percents_untouched() {
        let xml = r#"<a:pPr><a:lnSpc><a:spcPct val="150000"/></a:lnSpc><a:spcBef><a:spcPts val="600"/></a:spcBef><a:buSzPct val="45000"/><a:buSzPts val="1200"/></a:pPr><a:bodyPr><a:normAutofit fontScale="62500" lnSpcReduction="20000"/></a:bodyPr>"#;
        let out = scaled(xml, &s43());
        assert!(out.contains(r#"<a:spcPct val="150000"/>"#), "spcPct untouched: {out}");
        assert!(out.contains(r#"<a:spcPts val="800"/>"#), "{out}");
        assert!(out.contains(r#"<a:buSzPct val="45000"/>"#), "buSzPct untouched: {out}");
        assert!(out.contains(r#"<a:buSzPts val="1600"/>"#), "{out}");
        assert!(
            out.contains(r#"<a:normAutofit fontScale="62500" lnSpcReduction="20000"/>"#),
            "normAutofit untouched: {out}"
        );
    }

    // (6) custGeom pathLst untouched.
    #[test]
    fn custom_geometry_untouched() {
        let xml = r#"<a:custGeom><a:pathLst><a:path w="2000" h="1000"><a:moveTo><a:pt x="0" y="0"/></a:moveTo><a:lnTo><a:pt x="2000" y="1000"/></a:lnTo></a:path></a:pathLst></a:custGeom>"#;
        let out = scaled(xml, &s43());
        assert_eq!(out, xml, "custom geometry must be byte-identical: {out}");
    }

    // (7) effect dist/blurRad/rad.
    #[test]
    fn effects_scaled() {
        let xml = r#"<a:effectLst><a:outerShdw blurRad="600" dist="300" dir="2700000"/><a:glow rad="900"/><a:softEdge rad="1200"/><a:reflection blurRad="150" dist="600" sx="90000"/></a:effectLst>"#;
        let out = scaled(xml, &s43());
        assert!(out.contains(r#"blurRad="800" dist="400" dir="2700000""#), "shdw + dir kept: {out}");
        assert!(out.contains(r#"<a:glow rad="1200"/>"#), "{out}");
        assert!(out.contains(r#"<a:softEdge rad="1600"/>"#), "{out}");
        // reflection blurRad/dist scaled, sx (percent) untouched.
        assert!(out.contains(r#"<a:reflection blurRad="200" dist="800" sx="90000"/>"#), "{out}");
    }

    // (8) bodyPr insets + pPr marL/indent/defTabSz + tab pos.
    #[test]
    fn insets_and_paragraph_metrics_scaled() {
        let xml = r#"<a:bodyPr lIns="90" tIns="45" rIns="90" bIns="45"/><a:pPr marL="342900" indent="-342900" defTabSz="914400"><a:tabLst><a:tab pos="457200"/></a:tabLst></a:pPr><a:lstStyle><a:lvl1pPr marL="228600" indent="0"/><a:lvl9pPr marL="2743200"/></a:lstStyle>"#;
        let out = scaled(xml, &s43());
        assert!(out.contains(r#"<a:bodyPr lIns="120" tIns="60" rIns="120" bIns="60"/>"#), "{out}");
        assert!(out.contains(r#"marL="457200" indent="-457200" defTabSz="1219200""#), "{out}");
        assert!(out.contains(r#"<a:tab pos="609600"/>"#), "{out}");
        assert!(out.contains(r#"<a:lvl1pPr marL="304800" indent="0"/>"#), "{out}");
        assert!(out.contains(r#"<a:lvl9pPr marL="3657600"/>"#), "{out}");
    }

    #[test]
    fn compute_same_size_is_none() {
        assert_eq!(SlideScale::compute((100, 50), (100, 50), FitMode::EnsureFit), None);
    }

    #[test]
    fn compute_same_aspect_pure_scale() {
        // 9144000×5143500 → 12192000×6858000 is exactly 4:3 both ways.
        let sc =
            SlideScale::compute((9144000, 5143500), (12192000, 6858000), FitMode::EnsureFit)
                .unwrap();
        assert_eq!(sc.dx, 0);
        assert_eq!(sc.dy, 0);
        assert_eq!(sc.scale(9144000), 12192000);
        assert_eq!(sc.scale(5143500), 6858000);
        assert_eq!(sc.percent(), 133);
        assert!(is_same_aspect((9144000, 5143500), (12192000, 6858000)));
    }

    #[test]
    fn compute_ensurefit_letterbox() {
        // 4:3 source into a 16:9 canvas → s=1, letterbox horizontally.
        let sc =
            SlideScale::compute((9144000, 6858000), (12192000, 6858000), FitMode::EnsureFit)
                .unwrap();
        assert_eq!((sc.num, sc.den), (6858000, 6858000));
        assert_eq!(sc.dx, 1524000);
        assert_eq!(sc.dy, 0);
    }

    #[test]
    fn compute_ensurefit_and_maximize_inverse() {
        // 16:9 source into a 4:3 canvas.
        let fit =
            SlideScale::compute((12192000, 6858000), (9144000, 6858000), FitMode::EnsureFit)
                .unwrap();
        assert_eq!(fit.scale(1800), 1350);
        assert_eq!(fit.dx, 0);
        assert_eq!(fit.dy, 857250);

        let max =
            SlideScale::compute((12192000, 6858000), (9144000, 6858000), FitMode::Maximize)
                .unwrap();
        assert_eq!((max.num, max.den), (6858000, 6858000)); // s=1
        assert_eq!(max.dx, -1524000);
        assert_eq!(max.dy, 0);
    }
}
