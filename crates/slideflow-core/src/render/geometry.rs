//! Shape geometry: `a:xfrm` parsing, coordinate transforms, and preset-geometry
//! drawing.

use roxmltree::Node;

use super::fill::{Fill, Stroke};
use super::{a, ch, f_attr, fnum, Ctx, EMU_PER_PT};

#[derive(Clone)]
pub(crate) struct Xfrm {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) cx: f64,
    pub(crate) cy: f64,
    pub(crate) rot: i64,
    pub(crate) flip_h: bool,
    pub(crate) flip_v: bool,
}

pub(crate) fn parse_xfrm(node: Node) -> Xfrm {
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
pub(crate) struct Transform {
    pub(crate) sx: f64,
    pub(crate) sy: f64,
    pub(crate) tx: f64,
    pub(crate) ty: f64,
}

impl Transform {
    pub(crate) fn identity() -> Self {
        Transform { sx: 1.0, sy: 1.0, tx: 0.0, ty: 0.0 }
    }

    /// Map a shape's `xfrm` into an absolute rectangle in points.
    pub(crate) fn apply(&self, x: &Xfrm) -> Rect {
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

pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

impl Rect {
    /// Build an SVG transform string for rotation/flip about the rect's center.
    pub(crate) fn svg_transform(&self, x: &Xfrm) -> String {
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

impl Ctx<'_> {
    pub(crate) fn draw_geometry(&mut self, geom: Option<&str>, rect: &Rect, fill: &Fill, stroke: Option<&Stroke>) {
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
}
