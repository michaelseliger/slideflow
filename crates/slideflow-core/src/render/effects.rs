//! Drawing effects. Only `spPr/a:effectLst/a:outerShdw` is approximated, as an
//! SVG `feDropShadow` filter; inner shadow, glow, and reflection are ignored.

use roxmltree::Node;

use super::{a, ch, fnum, Ctx, EMU_PER_PT};

impl Ctx<'_> {
    /// Parse an `a:outerShdw` (if present) into a deduplicated `<filter>` carrying
    /// an `feDropShadow`, returning the filter id to reference on the geometry.
    ///
    /// `dist`/`blurRad` are EMU; `dir` is in 60000ths of a degree, measured
    /// clockwise from due east. Because SVG's y-axis points down, `dir` maps
    /// straight to screen space: `dx = dist·cos(dir)`, `dy = dist·sin(dir)`, so
    /// `dir = 90°` (a common "drop") pushes the shadow downward.
    pub(crate) fn resolve_shadow_filter(&mut self, sp_pr: Node) -> Option<String> {
        let shdw = ch(sp_pr, "effectLst").and_then(|e| ch(e, "outerShdw"))?;
        let emu = |name: &str| {
            a(shdw, name)
                .and_then(|v| v.parse::<f64>().ok())
                .unwrap_or(0.0)
        };
        let dist_pt = emu("dist") / EMU_PER_PT;
        let blur_pt = emu("blurRad") / EMU_PER_PT;
        let dir_deg = a(shdw, "dir")
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0)
            / 60000.0;
        let theta = dir_deg.to_radians();
        let dx = dist_pt * theta.cos();
        let dy = dist_pt * theta.sin();
        // SVG's Gaussian blur std deviation is roughly half the DrawingML radius.
        let std_dev = blur_pt / 2.0;
        // Shadow color (frequently semi-transparent via a:alpha); default 40% black.
        let (hex, opacity) = shdw
            .children()
            .find(|n| n.is_element())
            .and_then(|c| self.theme.parse_color(c))
            .map(|c| (c.hex(), c.a))
            .unwrap_or_else(|| ("#000000".to_string(), 0.4));

        let key = format!("{:.3},{:.3},{:.3},{}@{:.3}", dx, dy, std_dev, hex, opacity);
        if let Some(id) = self.shadow_cache.get(&key) {
            return Some(id.clone());
        }
        let id = format!("sh{}", self.shadow_cache.len());
        // Widen the default filter region so a large offset/blur isn't clipped.
        self.defs.push_str(&format!(
            r#"<filter id="{id}" x="-50%" y="-50%" width="200%" height="200%"><feDropShadow dx="{dx}" dy="{dy}" stdDeviation="{sd}" flood-color="{c}" flood-opacity="{o}"/></filter>"#,
            id = id,
            dx = fnum(dx),
            dy = fnum(dy),
            sd = fnum(std_dev),
            c = hex,
            o = fnum(opacity),
        ));
        self.shadow_cache.insert(key, id.clone());
        Some(id)
    }
}
