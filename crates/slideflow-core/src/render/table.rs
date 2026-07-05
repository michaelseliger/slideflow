//! DrawingML tables: an `a:tbl` inside a `p:graphicFrame`. Renders the cell
//! grid, explicit `tcPr` fills, cell borders, and cell text (via the shared text
//! renderer). Table *styles* (`tblPr` banding through `tableStyles.xml`) are out
//! of scope: only explicit `tcPr` fills/borders are honored. When a cell declares
//! no borders and the table carries no table style, a subtle default gridline is
//! drawn so bare tables still read as tables.

use roxmltree::Node;

use super::fill::{Fill, Stroke};
use super::geometry::{parse_xfrm, Rect, Transform};
use super::{a, ch, fnum, Ctx, EMU_PER_PT};

const DEFAULT_BORDER: &str = "#D0D0D0";
const DEFAULT_BORDER_W: f64 = 0.75;
const BORDER_EDGES: [&str; 4] = ["lnL", "lnR", "lnT", "lnB"];

impl Ctx<'_> {
    /// Render a `p:graphicFrame`: only tables (graphicData `uri` ending in
    /// `/table` with an `a:tbl`) draw; charts/SmartArt/OLE are skipped silently.
    /// The frame's own `p:xfrm` (not inside `spPr`) positions it like any shape.
    pub(crate) fn render_graphic_frame(&mut self, node: Node, tf: Transform) {
        let Some(xfrm) = ch(node, "xfrm") else { return };
        let x = parse_xfrm(xfrm);
        let rect = tf.apply(&x);
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let Some(gdata) = ch(node, "graphic").and_then(|g| ch(g, "graphicData")) else {
            return;
        };
        if !a(gdata, "uri")
            .map(|u| u.ends_with("/table"))
            .unwrap_or(false)
        {
            return;
        }
        let Some(tbl) = ch(gdata, "tbl") else { return };

        let transform = rect.svg_transform(&x);
        let open_g = !transform.is_empty();
        if open_g {
            self.body
                .push_str(&format!(r#"<g transform="{transform}">"#));
        }
        self.render_table(tbl, &rect);
        if open_g {
            self.body.push_str("</g>");
        }
    }

    fn render_table(&mut self, tbl: Node, rect: &Rect) {
        let cols: Vec<f64> = ch(tbl, "tblGrid")
            .map(|g| {
                g.children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "gridCol")
                    .map(|c| a(c, "w").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0))
                    .collect()
            })
            .unwrap_or_default();
        let rows: Vec<Node> = tbl
            .children()
            .filter(|n| n.is_element() && n.tag_name().name() == "tr")
            .collect();
        if cols.is_empty() || rows.is_empty() {
            return;
        }
        // A referenced table style would supply banding/borders we don't render;
        // its presence suppresses the default-gridline fallback.
        let has_tbl_style = ch(tbl, "tblPr")
            .and_then(|p| ch(p, "tableStyleId"))
            .is_some();

        let row_h: Vec<f64> = rows
            .iter()
            .map(|r| {
                a(*r, "h")
                    .and_then(|v| v.parse::<f64>().ok())
                    .unwrap_or(0.0)
            })
            .collect();

        // The grid's natural extent, scaled independently on each axis to fill the
        // frame rect (PowerPoint keeps grid and frame equal). Boundaries are in
        // points, one more than the count of columns/rows.
        let col_x = boundaries(&cols, rect.x, rect.w);
        let row_y = boundaries(&row_h, rect.y, rect.h);

        for (ri, tr) in rows.iter().enumerate() {
            // Each `a:tc` maps to one grid column (a spanning cell is followed by
            // hMerge/vMerge continuation cells), so the column index advances by 1
            // per cell and a master cell draws across its `gridSpan` columns.
            let mut ci = 0usize;
            for tc in tr
                .children()
                .filter(|n| n.is_element() && n.tag_name().name() == "tc")
            {
                if ci >= cols.len() {
                    break;
                }
                if a(tc, "hMerge") == Some("1") || a(tc, "vMerge") == Some("1") {
                    ci += 1;
                    continue;
                }
                let span = a(tc, "gridSpan")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1);
                let rspan = a(tc, "rowSpan")
                    .and_then(|v| v.parse::<usize>().ok())
                    .unwrap_or(1)
                    .max(1);
                let c1 = (ci + span).min(cols.len());
                let r1 = (ri + rspan).min(rows.len());
                let cell = Rect {
                    x: col_x[ci],
                    y: row_y[ri],
                    w: (col_x[c1] - col_x[ci]).max(0.0),
                    h: (row_y[r1] - row_y[ri]).max(0.0),
                };
                if cell.w > 0.0 && cell.h > 0.0 {
                    self.render_cell(tc, &cell, has_tbl_style);
                }
                ci += 1;
            }
        }
    }

    fn render_cell(&mut self, tc: Node, cell: &Rect, has_tbl_style: bool) {
        let tc_pr = ch(tc, "tcPr");

        // Background: explicit tcPr solidFill/gradFill only (banding is skipped).
        if let Some(pr) = tc_pr {
            let fill = self.resolve_fill(pr);
            if matches!(fill, Fill::Solid(_) | Fill::Gradient { .. }) {
                let attrs = self.fill_attrs(&fill);
                self.body.push_str(&format!(
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}"{f}/>"#,
                    x = fnum(cell.x),
                    y = fnum(cell.y),
                    w = fnum(cell.w),
                    h = fnum(cell.h),
                    f = attrs,
                ));
            }
        }

        self.render_cell_borders(tc_pr, cell, has_tbl_style);

        // Cell text via the shared renderer (no shape node, no placeholder). Its
        // default insets (7.2pt/3.6pt) match PowerPoint's default cell margins;
        // custom tcPr margins are approximated by them.
        if let Some(tx) = ch(tc, "txBody") {
            self.render_text(None, tx, cell, None);
        }
    }

    fn render_cell_borders(&mut self, tc_pr: Option<Node>, cell: &Rect, has_tbl_style: bool) {
        let get = |name: &str| tc_pr.and_then(|p| ch(p, name));
        let any_declared = BORDER_EDGES.iter().any(|n| get(n).is_some());

        if !any_declared {
            if !has_tbl_style {
                self.body.push_str(&format!(
                    r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="none" stroke="{c}" stroke-width="{sw}"/>"#,
                    x = fnum(cell.x),
                    y = fnum(cell.y),
                    w = fnum(cell.w),
                    h = fnum(cell.h),
                    c = DEFAULT_BORDER,
                    sw = fnum(DEFAULT_BORDER_W),
                ));
            }
            return;
        }

        let (l, t, r, b) = (cell.x, cell.y, cell.x + cell.w, cell.y + cell.h);
        let edges = [
            ("lnT", (l, t, r, t)),
            ("lnB", (l, b, r, b)),
            ("lnL", (l, t, l, b)),
            ("lnR", (r, t, r, b)),
        ];
        for (name, (x1, y1, x2, y2)) in edges {
            let Some(ln) = get(name) else { continue };
            let Some(s) = self.cell_border(ln) else {
                continue;
            };
            self.body.push_str(&format!(
                r#"<line x1="{x1}" y1="{y1}" x2="{x2}" y2="{y2}" stroke="{c}" stroke-width="{w}"/>"#,
                x1 = fnum(x1),
                y1 = fnum(y1),
                x2 = fnum(x2),
                y2 = fnum(y2),
                c = s.color.hex(),
                w = fnum(s.width_pt.max(0.25)),
            ));
        }
    }

    /// Parse one cell-border element (`a:lnL`/`lnR`/`lnT`/`lnB`) — shaped like an
    /// `a:ln` — into a `Stroke`. An explicit `a:noFill` means "no line".
    fn cell_border(&self, ln: Node) -> Option<Stroke> {
        if ln
            .children()
            .any(|n| n.is_element() && n.tag_name().name() == "noFill")
        {
            return None;
        }
        let color = ch(ln, "solidFill")
            .and_then(|f| f.children().find(|n| n.is_element()))
            .and_then(|cn| self.theme.parse_color(cn))?;
        let width_pt = a(ln, "w")
            .and_then(|v| v.parse::<f64>().ok())
            .map(|w| w / EMU_PER_PT)
            .unwrap_or(1.0);
        Some(Stroke::solid(color, width_pt))
    }
}

/// Cumulative boundary positions (length `sizes.len() + 1`) mapping a run of
/// natural sizes into `[start, start + extent]` in points. A zero total falls
/// back to equal division so a degenerate grid still lays out.
fn boundaries(sizes: &[f64], start: f64, extent: f64) -> Vec<f64> {
    let mut out = Vec::with_capacity(sizes.len() + 1);
    out.push(start);
    let total: f64 = sizes.iter().sum();
    if total > 0.0 {
        let mut acc = 0.0;
        for s in sizes {
            acc += s;
            out.push(start + acc / total * extent);
        }
    } else {
        let n = sizes.len().max(1) as f64;
        for i in 1..=sizes.len() {
            out.push(start + extent * i as f64 / n);
        }
    }
    out
}
