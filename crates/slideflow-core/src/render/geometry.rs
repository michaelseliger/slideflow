//! Shape geometry: `a:xfrm` parsing, coordinate transforms, and preset-geometry
//! drawing.

use roxmltree::Node;

use super::color::Rgba;
use super::fill::{Fill, Stroke};
use super::{a, ch, f_attr, fnum, truthy, Ctx, EMU_PER_PT};

/// Stroke-only straight/bent connector presets: legitimately zero-extent
/// along one axis (a purely horizontal/vertical line has `cx`/`cy` = 0).
pub(crate) fn is_line_preset(prst: &str) -> bool {
    matches!(
        prst,
        "line"
            | "straightConnector1"
            | "bentConnector2"
            | "bentConnector3"
            | "curvedConnector2"
            | "curvedConnector3"
    )
}

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
        flip_h: truthy(node, "flipH"),
        flip_v: truthy(node, "flipV"),
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

#[derive(Clone, Copy)]
pub(crate) struct Rect {
    pub(crate) x: f64,
    pub(crate) y: f64,
    pub(crate) w: f64,
    pub(crate) h: f64,
}

/// Build an SVG transform string for a rotation (`rot` in 60000ths of a degree)
/// and optional H/V flip about the center `(cx, cy)`. Returns `""` for the
/// identity case (no rotation, no flip). Shared by [`Rect::svg_transform`] and
/// group rendering so the shape path and the group path emit identical markup.
pub(crate) fn rot_flip_transform(rot: i64, flip_h: bool, flip_v: bool, cx: f64, cy: f64) -> String {
    let mut parts = String::new();
    if rot != 0 {
        let deg = rot as f64 / 60000.0;
        parts.push_str(&format!("rotate({} {} {})", fnum(deg), fnum(cx), fnum(cy)));
    }
    if flip_h || flip_v {
        let sx = if flip_h { -1.0 } else { 1.0 };
        let sy = if flip_v { -1.0 } else { 1.0 };
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

impl Rect {
    /// Build an SVG transform string for rotation/flip about the rect's center.
    pub(crate) fn svg_transform(&self, x: &Xfrm) -> String {
        let cx = self.x + self.w / 2.0;
        let cy = self.y + self.h / 2.0;
        rot_flip_transform(x.rot, x.flip_h, x.flip_v, cx, cy)
    }
}

impl Ctx<'_> {
    pub(crate) fn draw_geometry(
        &mut self,
        geom_node: Option<Node>,
        rect: &Rect,
        fill: &Fill,
        stroke: Option<&Stroke>,
    ) {
        let prst = geom_node.and_then(|g| a(g, "prst"));
        let fill_attrs = self.fill_attrs(fill);
        let mut stroke_attrs = String::new();
        if let Some(s) = stroke {
            stroke_attrs = format!(
                r#" stroke="{}" stroke-width="{}"{}"#,
                s.color.hex(),
                fnum(s.width_pt.max(0.25)),
                s.deco_attrs()
            );
        }

        // Custom geometry: convert the `a:pathLst` into SVG `<path>` element(s).
        // On any non-literal coordinate (formula guide) or empty path list the
        // builder returns None, and we fall through to the rectangle fallback.
        if let Some(g) = geom_node {
            if g.tag_name().name() == "custGeom" {
                if let Some(paths) = cust_geom_paths(g, rect) {
                    for p in &paths {
                        let f = if p.no_fill { r#" fill="none""#.to_string() } else { fill_attrs.clone() };
                        let s = if p.no_stroke { String::new() } else { stroke_attrs.clone() };
                        self.body.push_str(&format!(r#"<path d="{d}"{f}{s}/>"#, d = p.d));
                    }
                    return;
                }
            }
        }

        match prst.unwrap_or("rect") {
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
            "roundRect" | "round1Rect" | "round2SameRect" | "round2DiagRect" => {
                // Corner radius from the `adj` guide (fraction of the shorter
                // side, in 1/100000), clamped to half the shorter side; default
                // ≈1/6 as PowerPoint uses.
                let adj = geom_node.and_then(gd_adj).unwrap_or(16667.0);
                let short = rect.w.min(rect.h);
                let r = (short * (adj / 100_000.0)).clamp(0.0, short / 2.0);
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
            // Common closed presets → a parametric polygon. `adj` (where the
            // preset uses one) comes from `a:avLst`, else a sensible default.
            "triangle" | "isoscelesTriangle" => {
                let apex = rect.x + rect.w * adj_frac(geom_node, 0.5);
                let pts = [
                    (apex, rect.y),
                    (rect.x + rect.w, rect.y + rect.h),
                    (rect.x, rect.y + rect.h),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "rtTriangle" => {
                let pts = [
                    (rect.x, rect.y),
                    (rect.x, rect.y + rect.h),
                    (rect.x + rect.w, rect.y + rect.h),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "diamond" => {
                let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                let pts = [
                    (cx, rect.y),
                    (rect.x + rect.w, cy),
                    (cx, rect.y + rect.h),
                    (rect.x, cy),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "parallelogram" => {
                let dx = rect.w * adj_frac(geom_node, 0.25);
                let pts = [
                    (rect.x + dx, rect.y),
                    (rect.x + rect.w, rect.y),
                    (rect.x + rect.w - dx, rect.y + rect.h),
                    (rect.x, rect.y + rect.h),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "trapezoid" => {
                let dx = rect.w * adj_frac(geom_node, 0.25);
                let pts = [
                    (rect.x + dx, rect.y),
                    (rect.x + rect.w - dx, rect.y),
                    (rect.x + rect.w, rect.y + rect.h),
                    (rect.x, rect.y + rect.h),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "pentagon" => {
                // Regular pentagon, apex up, inscribed in the rect.
                let (cx, cy) = (rect.x + rect.w / 2.0, rect.y + rect.h / 2.0);
                let (rx, ry) = (rect.w / 2.0, rect.h / 2.0);
                let pts: Vec<(f64, f64)> = (0..5)
                    .map(|i| {
                        let ang = (-90.0 + i as f64 * 72.0_f64).to_radians();
                        (cx + rx * ang.cos(), cy + ry * ang.sin())
                    })
                    .collect();
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "hexagon" => {
                let dx = rect.w * adj_frac(geom_node, 0.25);
                let cy = rect.y + rect.h / 2.0;
                let pts = [
                    (rect.x + dx, rect.y),
                    (rect.x + rect.w - dx, rect.y),
                    (rect.x + rect.w, cy),
                    (rect.x + rect.w - dx, rect.y + rect.h),
                    (rect.x + dx, rect.y + rect.h),
                    (rect.x, cy),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "chevron" => {
                let dx = rect.w * adj_frac(geom_node, 0.5);
                let cy = rect.y + rect.h / 2.0;
                let pts = [
                    (rect.x, rect.y),
                    (rect.x + rect.w - dx, rect.y),
                    (rect.x + rect.w, cy),
                    (rect.x + rect.w - dx, rect.y + rect.h),
                    (rect.x, rect.y + rect.h),
                    (rect.x + dx, cy),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "rightArrow" => {
                // Shaft thickness 0.5 (y 0.25..0.75); arrowhead starts at mid-width.
                let (hx, ay1, ay2) = (0.5, 0.25, 0.75);
                let cy = rect.y + rect.h / 2.0;
                let pts = [
                    (rect.x, rect.y + rect.h * ay1),
                    (rect.x + rect.w * hx, rect.y + rect.h * ay1),
                    (rect.x + rect.w * hx, rect.y),
                    (rect.x + rect.w, cy),
                    (rect.x + rect.w * hx, rect.y + rect.h),
                    (rect.x + rect.w * hx, rect.y + rect.h * ay2),
                    (rect.x, rect.y + rect.h * ay2),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "leftArrow" => {
                let (hx, ay1, ay2) = (0.5, 0.25, 0.75);
                let cy = rect.y + rect.h / 2.0;
                let pts = [
                    (rect.x + rect.w, rect.y + rect.h * ay1),
                    (rect.x + rect.w * (1.0 - hx), rect.y + rect.h * ay1),
                    (rect.x + rect.w * (1.0 - hx), rect.y),
                    (rect.x, cy),
                    (rect.x + rect.w * (1.0 - hx), rect.y + rect.h),
                    (rect.x + rect.w * (1.0 - hx), rect.y + rect.h * ay2),
                    (rect.x + rect.w, rect.y + rect.h * ay2),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            "plus" => {
                let ai = adj_frac(geom_node, 0.25);
                let (x, y, w, h) = (rect.x, rect.y, rect.w, rect.h);
                let pts = [
                    (x + w * ai, y),
                    (x + w * (1.0 - ai), y),
                    (x + w * (1.0 - ai), y + h * ai),
                    (x + w, y + h * ai),
                    (x + w, y + h * (1.0 - ai)),
                    (x + w * (1.0 - ai), y + h * (1.0 - ai)),
                    (x + w * (1.0 - ai), y + h),
                    (x + w * ai, y + h),
                    (x + w * ai, y + h * (1.0 - ai)),
                    (x, y + h * (1.0 - ai)),
                    (x, y + h * ai),
                    (x + w * ai, y + h * ai),
                ];
                self.body.push_str(&polygon(&pts, &fill_attrs, &stroke_attrs));
            }
            // Straight lines / connectors: the main diagonal of the bounding box.
            // flipH/flipV (applied by the wrapping <g transform>) mirror it to
            // the correct diagonal, so we always draw top-left → bottom-right.
            p if is_line_preset(p) => {
                let (color, width, deco) = stroke
                    .map(|s| (s.color.hex(), s.width_pt.max(0.75), s.deco_attrs()))
                    .unwrap_or_else(|| ("#595959".to_string(), 1.0, String::new()));
                self.body.push_str(&format!(
                    r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{c}" stroke-width="{w}"{deco}/>"#,
                    x1 = fnum(rect.x),
                    y1 = fnum(rect.y),
                    x2 = fnum(rect.x + rect.w),
                    y2 = fnum(rect.y + rect.h),
                    c = color,
                    w = fnum(width)
                ));
                // Oval head/tail ends become filled dots (medium oval ≈ 3×width).
                if let Some(s) = stroke.filter(|s| s.head_oval || s.tail_oval) {
                    let r = 1.5 * width;
                    let mut dot = |x: f64, y: f64| {
                        self.body.push_str(&format!(
                            r#"<circle cx="{cx}" cy="{cy}" r="{r}" fill="{c}"/>"#,
                            cx = fnum(x),
                            cy = fnum(y),
                            r = fnum(r),
                            c = s.color.hex()
                        ));
                    };
                    if s.head_oval {
                        dot(rect.x, rect.y);
                    }
                    if s.tail_oval {
                        dot(rect.x + rect.w, rect.y + rect.h);
                    }
                }
            }
            // rect and any unimplemented preset fall back to a plain rectangle.
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

    /// Intern a `<clipPath>` shaped like `geom_node` scaled to `rect` and return
    /// its id — pictures clip to it so non-rectangular shapes (custGeom diagonal
    /// cuts, roundRects, …) crop their image instead of painting a full rect.
    /// Plain rects return `None`: the caller's rectangular clip suffices.
    pub(crate) fn geometry_clip(&mut self, geom_node: Option<Node>, rect: &Rect) -> Option<String> {
        let g = geom_node?;
        if g.tag_name().name() != "custGeom"
            && matches!(a(g, "prst"), None | Some("rect"))
        {
            return None;
        }
        // Reuse the geometry renderer: draw fill-only into a scratch buffer
        // (clipPath children contribute their geometry; paint attrs are moot).
        let saved = std::mem::take(&mut self.body);
        self.draw_geometry(Some(g), rect, &Fill::Solid(Rgba { r: 0.0, g: 0.0, b: 0.0, a: 1.0 }), None);
        let shapes = std::mem::replace(&mut self.body, saved);
        if shapes.is_empty() {
            return None;
        }
        let id = format!("geomclip{}", self.clip_id);
        self.clip_id += 1;
        self.defs.push_str(&format!(r#"<clipPath id="{id}">{shapes}</clipPath>"#));
        Some(id)
    }
}

/// Emit an SVG `<polygon>` from absolute (point-space) vertices.
fn polygon(points: &[(f64, f64)], fill_attrs: &str, stroke_attrs: &str) -> String {
    let pts: String = points
        .iter()
        .map(|(x, y)| format!("{},{}", fnum(*x), fnum(*y)))
        .collect::<Vec<_>>()
        .join(" ");
    format!(r#"<polygon points="{pts}"{fill_attrs}{stroke_attrs}/>"#)
}

/// The preset's single `adj` guide as a 0..1 fraction, clamped, or `default`.
fn adj_frac(geom_node: Option<Node>, default: f64) -> f64 {
    geom_node
        .and_then(gd_adj)
        .map(|v| (v / 100_000.0).clamp(0.0, 1.0))
        .unwrap_or(default)
}

/// One converted `a:path` subpath: its SVG `d` data plus per-path paint flags.
struct CustPath {
    d: String,
    no_fill: bool,
    no_stroke: bool,
}

/// A parsed path-space point mapped into rect (points) space.
fn cust_pt(pt: Node, rect: &Rect, sx: f64, sy: f64) -> Option<(f64, f64)> {
    let (x, y) = raw_pt(pt)?;
    Some((rect.x + x * sx, rect.y + y * sy))
}

/// Convert an `a:custGeom`'s `a:pathLst` into SVG path data, one [`CustPath`] per
/// `a:path`, scaled and translated into `rect`.
///
/// TIER 1: only literal numeric `a:pt`/`a:arcTo` coordinates are handled. A
/// formula-guide coordinate (anything that doesn't parse as a number) aborts the
/// whole conversion (returns `None`) so the caller falls back to a plain
/// rectangle rather than dropping or mis-drawing the shape.
fn cust_geom_paths(geom: Node, rect: &Rect) -> Option<Vec<CustPath>> {
    let path_lst = ch(geom, "pathLst")?;
    let mut out = Vec::new();
    for path in path_lst
        .children()
        .filter(|n| n.is_element() && n.tag_name().name() == "path")
    {
        // Path coordinate space: explicit `w`/`h`, else the shape's EMU extents.
        let space_w = a(path, "w")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(rect.w * EMU_PER_PT);
        let space_h = a(path, "h")
            .and_then(|v| v.trim().parse::<f64>().ok())
            .filter(|v| *v > 0.0)
            .unwrap_or(rect.h * EMU_PER_PT);
        if space_w <= 0.0 || space_h <= 0.0 {
            return None;
        }
        let sx = rect.w / space_w;
        let sy = rect.h / space_h;

        let mut d = String::new();
        // Current point in *path space* (for arcTo center math).
        let mut cur = (0.0_f64, 0.0_f64);
        for cmd in path.children().filter(|n| n.is_element()) {
            let pts: Vec<Node> = cmd
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "pt")
                .collect();
            match cmd.tag_name().name() {
                "moveTo" => {
                    let pt = *pts.first()?;
                    cur = raw_pt(pt)?;
                    let (ox, oy) = cust_pt(pt, rect, sx, sy)?;
                    d.push_str(&format!("M{} {}", fnum(ox), fnum(oy)));
                }
                "lnTo" => {
                    let pt = *pts.first()?;
                    cur = raw_pt(pt)?;
                    let (ox, oy) = cust_pt(pt, rect, sx, sy)?;
                    d.push_str(&format!("L{} {}", fnum(ox), fnum(oy)));
                }
                "cubicBezTo" => {
                    if pts.len() < 3 {
                        return None;
                    }
                    let mut seg = String::from("C");
                    for (i, pt) in pts.iter().take(3).enumerate() {
                        let (ox, oy) = cust_pt(*pt, rect, sx, sy)?;
                        seg.push_str(&format!("{}{} {}", if i == 0 { "" } else { " " }, fnum(ox), fnum(oy)));
                    }
                    cur = raw_pt(pts[2])?;
                    d.push_str(&seg);
                }
                "quadBezTo" => {
                    if pts.len() < 2 {
                        return None;
                    }
                    let mut seg = String::from("Q");
                    for (i, pt) in pts.iter().take(2).enumerate() {
                        let (ox, oy) = cust_pt(*pt, rect, sx, sy)?;
                        seg.push_str(&format!("{}{} {}", if i == 0 { "" } else { " " }, fnum(ox), fnum(oy)));
                    }
                    cur = raw_pt(pts[1])?;
                    d.push_str(&seg);
                }
                "arcTo" => {
                    let (seg, end) = arc_to_svg(cmd, rect, sx, sy, cur)?;
                    cur = end;
                    d.push_str(&seg);
                }
                "close" => d.push('Z'),
                _ => {} // unknown path command: skip
            }
        }
        if d.is_empty() {
            continue;
        }
        out.push(CustPath {
            d,
            no_fill: a(path, "fill") == Some("none"),
            no_stroke: matches!(a(path, "stroke"), Some("0") | Some("false")),
        });
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Raw (unmapped) path-space coordinates of an `a:pt`.
fn raw_pt(pt: Node) -> Option<(f64, f64)> {
    let x = a(pt, "x")?.trim().parse::<f64>().ok()?;
    let y = a(pt, "y")?.trim().parse::<f64>().ok()?;
    Some((x, y))
}

/// Convert an `a:arcTo` (radii `wR`/`hR`, start `stAng`, sweep `swAng`, all in
/// path units / 60000ths of a degree) starting at path-space `cur` into an SVG
/// `A` command. Returns the `d` fragment and the new current point (path space).
fn arc_to_svg(
    cmd: Node,
    rect: &Rect,
    sx: f64,
    sy: f64,
    cur: (f64, f64),
) -> Option<(String, (f64, f64))> {
    let w_r = a(cmd, "wR")?.trim().parse::<f64>().ok()?;
    let h_r = a(cmd, "hR")?.trim().parse::<f64>().ok()?;
    let st = a(cmd, "stAng")?.trim().parse::<f64>().ok()? / 60000.0;
    let sw = a(cmd, "swAng")?.trim().parse::<f64>().ok()? / 60000.0;
    let (st_r, sw_r) = (st.to_radians(), sw.to_radians());
    // Center chosen so the ellipse passes through `cur` at angle st.
    let cx = cur.0 - w_r * st_r.cos();
    let cy = cur.1 - h_r * st_r.sin();
    let end_ang = st_r + sw_r;
    let end = (cx + w_r * end_ang.cos(), cy + h_r * end_ang.sin());
    let (ox, oy) = (rect.x + end.0 * sx, rect.y + end.1 * sy);
    let large = if sw.abs() > 180.0 { 1 } else { 0 };
    // DrawingML positive sweep is clockwise; with SVG's y-down axes that is
    // sweep-flag 1.
    let sweep = if sw >= 0.0 { 1 } else { 0 };
    let seg = format!(
        "A{} {} 0 {} {} {} {}",
        fnum((w_r * sx).abs()),
        fnum((h_r * sy).abs()),
        large,
        sweep,
        fnum(ox),
        fnum(oy)
    );
    Some((seg, end))
}

/// The `adj` adjust value (1/100000 units) from a preset geometry's `a:avLst`,
/// if present as a literal `fmla="val N"`.
fn gd_adj(geom: Node) -> Option<f64> {
    let av = ch(geom, "avLst")?;
    av.children()
        .filter(|c| c.is_element() && c.tag_name().name() == "gd")
        .find(|g| a(*g, "name") == Some("adj"))
        .and_then(|g| a(g, "fmla"))
        .and_then(|f| f.strip_prefix("val "))
        .and_then(|v| v.trim().parse::<f64>().ok())
}
