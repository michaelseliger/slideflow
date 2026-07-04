//! Theme color resolution: scheme/clrMap slots and DrawingML color parsing.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use super::{a, ch, EMU_PER_PT};

#[derive(Clone, Copy)]
pub(crate) struct Rgba {
    pub(crate) r: f64,
    pub(crate) g: f64,
    pub(crate) b: f64,
    pub(crate) a: f64,
}

impl Rgba {
    fn new(r: u8, g: u8, b: u8) -> Self {
        Rgba { r: r as f64, g: g as f64, b: b as f64, a: 1.0 }
    }

    pub(crate) fn hex(&self) -> String {
        format!(
            "#{:02X}{:02X}{:02X}",
            self.r.round().clamp(0.0, 255.0) as u8,
            self.g.round().clamp(0.0, 255.0) as u8,
            self.b.round().clamp(0.0, 255.0) as u8
        )
    }
}

fn parse_hex(s: &str) -> Option<Rgba> {
    let s = s.trim();
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(Rgba::new(r, g, b))
}

pub(crate) struct Theme {
    /// scheme slot (dk1, lt1, dk2, lt2, accent1..6, hlink, folHlink) → color
    pub(crate) scheme: HashMap<String, Rgba>,
    /// clrMap: bg1/tx1/bg2/tx2/accentN/... → scheme slot
    pub(crate) clr_map: HashMap<String, String>,
    pub(crate) major_font: String,
    pub(crate) minor_font: String,
    /// `a:fmtScheme/a:fillStyleLst` entries, stored as raw XML templates that
    /// still carry `<a:schemeClr val="phClr">` placeholders. Resolved lazily
    /// against a shape's `a:fillRef` color via [`Theme::parse_color_ph`].
    pub(crate) fill_styles: Vec<String>,
    /// `a:lnStyleLst` entries: (line width in points, raw `<a:ln>` XML template).
    pub(crate) line_styles: Vec<(f64, String)>,
    /// `a:bgFillStyleLst` entries (raw XML templates, like `fill_styles`).
    pub(crate) bg_fill_styles: Vec<String>,
}

impl Default for Theme {
    fn default() -> Self {
        let mut scheme = HashMap::new();
        scheme.insert("dk1".into(), Rgba::new(0, 0, 0));
        scheme.insert("lt1".into(), Rgba::new(255, 255, 255));
        scheme.insert("dk2".into(), Rgba::new(0x44, 0x54, 0x6A));
        scheme.insert("lt2".into(), Rgba::new(0xE7, 0xE6, 0xE6));
        scheme.insert("accent1".into(), Rgba::new(0x44, 0x72, 0xC4));
        Theme {
            scheme,
            clr_map: HashMap::new(),
            major_font: "Calibri".into(),
            minor_font: "Calibri".into(),
            fill_styles: Vec::new(),
            line_styles: Vec::new(),
            bg_fill_styles: Vec::new(),
        }
    }
}

impl Theme {
    pub(crate) fn load_theme(&mut self, doc: &Document) {
        let root = doc.root_element();
        let Some(elems) = ch(root, "themeElements") else { return };
        if let Some(scheme) = ch(elems, "clrScheme") {
            for slot in scheme.children().filter(|n| n.is_element()) {
                let name = slot.tag_name().name().to_string();
                if let Some(color) = slot
                    .children()
                    .find(|n| n.is_element())
                    .and_then(|cn| self.parse_scheme_base(cn))
                {
                    self.scheme.insert(name, color);
                }
            }
        }
        if let Some(fonts) = ch(elems, "fontScheme") {
            if let Some(f) = ch(fonts, "majorFont")
                .and_then(|m| ch(m, "latin"))
                .and_then(|l| a(l, "typeface"))
                .filter(|s| !s.is_empty())
            {
                self.major_font = f.to_string();
            }
            if let Some(f) = ch(fonts, "minorFont")
                .and_then(|m| ch(m, "latin"))
                .and_then(|l| a(l, "typeface"))
                .filter(|s| !s.is_empty())
            {
                self.minor_font = f.to_string();
            }
        }
        if let Some(fmt) = ch(elems, "fmtScheme") {
            let src = doc.input_text();
            if let Some(lst) = ch(fmt, "fillStyleLst") {
                for f in lst.children().filter(|n| n.is_element()) {
                    self.fill_styles.push(src[f.range()].to_string());
                }
            }
            if let Some(lst) = ch(fmt, "lnStyleLst") {
                for ln in lst
                    .children()
                    .filter(|n| n.is_element() && n.tag_name().name() == "ln")
                {
                    let w = a(ln, "w")
                        .and_then(|v| v.parse::<f64>().ok())
                        .map(|v| v / EMU_PER_PT)
                        .unwrap_or(1.0);
                    self.line_styles.push((w, src[ln.range()].to_string()));
                }
            }
            if let Some(lst) = ch(fmt, "bgFillStyleLst") {
                for f in lst.children().filter(|n| n.is_element()) {
                    self.bg_fill_styles.push(src[f.range()].to_string());
                }
            }
        }
    }

    pub(crate) fn load_clr_map(&mut self, doc: &Document) {
        // clrMap lives directly under the master root.
        let root = doc.root_element();
        if let Some(map) = root
            .descendants()
            .find(|n| n.is_element() && n.tag_name().name() == "clrMap")
        {
            for at in map.attributes() {
                self.clr_map.insert(at.name().to_string(), at.value().to_string());
            }
        }
    }

    /// Resolve a scheme slot color without transforms.
    fn parse_scheme_base(&self, node: Node) -> Option<Rgba> {
        match node.tag_name().name() {
            "srgbClr" => a(node, "val").and_then(parse_hex),
            "sysClr" => Some(sys_color(node)),
            _ => None,
        }
    }

    fn resolve_scheme(&self, val: &str) -> Rgba {
        let slot: String = match val {
            "bg1" | "tx1" | "bg2" | "tx2" => self
                .clr_map
                .get(val)
                .cloned()
                .unwrap_or_else(|| default_map_slot(val)),
            other => other.to_string(),
        };
        self.scheme
            .get(&slot)
            .copied()
            .unwrap_or_else(|| Rgba::new(0, 0, 0))
    }

    pub(crate) fn text_default(&self) -> Rgba {
        self.resolve_scheme("tx1")
    }

    /// Parse a DrawingML color element (`srgbClr`/`sysClr`/`schemeClr`/`prstClr`)
    /// applying its transform children.
    pub(crate) fn parse_color(&self, node: Node) -> Option<Rgba> {
        self.parse_color_ph(node, None)
    }

    /// Like [`parse_color`], but a `<a:schemeClr val="phClr">` placeholder (used
    /// inside `fmtScheme` style templates) resolves to `ph` when supplied. Its
    /// own transform children still apply on top of the substituted color.
    pub(crate) fn parse_color_ph(&self, node: Node, ph: Option<Rgba>) -> Option<Rgba> {
        let mut base = match node.tag_name().name() {
            "srgbClr" => a(node, "val").and_then(parse_hex)?,
            "sysClr" => sys_color(node),
            "schemeClr" => {
                let val = a(node, "val")?;
                if val == "phClr" {
                    ph?
                } else {
                    self.resolve_scheme(val)
                }
            }
            "prstClr" => preset_color(a(node, "val")?),
            "scrgbClr" => scrgb_color(node)?,
            _ => return None,
        };
        for t in node.children().filter(|n| n.is_element()) {
            let f = a(t, "val").and_then(|v| v.parse::<f64>().ok()).map(|v| v / 100000.0);
            match t.tag_name().name() {
                "lumMod" => {
                    if let Some(f) = f {
                        base.r *= f;
                        base.g *= f;
                        base.b *= f;
                    }
                }
                "lumOff" => {
                    if let Some(f) = f {
                        base.r += 255.0 * f;
                        base.g += 255.0 * f;
                        base.b += 255.0 * f;
                    }
                }
                "shade" => {
                    if let Some(f) = f {
                        base.r *= f;
                        base.g *= f;
                        base.b *= f;
                    }
                }
                "tint" => {
                    if let Some(f) = f {
                        base.r = base.r * f + 255.0 * (1.0 - f);
                        base.g = base.g * f + 255.0 * (1.0 - f);
                        base.b = base.b * f + 255.0 * (1.0 - f);
                    }
                }
                "alpha" => {
                    if let Some(f) = f {
                        base.a = f;
                    }
                }
                // Saturation / hue / luminance transforms round-trip through HSL.
                "satMod" => {
                    if let Some(f) = f {
                        let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
                        set_rgb(&mut base, hsl_to_rgb(h, (s * f).clamp(0.0, 1.0), l));
                    }
                }
                "satOff" => {
                    if let Some(f) = f {
                        let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
                        set_rgb(&mut base, hsl_to_rgb(h, (s + f).clamp(0.0, 1.0), l));
                    }
                }
                "hueMod" => {
                    if let Some(f) = f {
                        let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
                        set_rgb(&mut base, hsl_to_rgb((h * f).rem_euclid(360.0), s, l));
                    }
                }
                "hueOff" => {
                    // `val` is in 60000ths of a degree (not the 1/1000 % of `f`).
                    if let Some(deg) =
                        a(t, "val").and_then(|v| v.parse::<f64>().ok()).map(|v| v / 60000.0)
                    {
                        let (h, s, l) = rgb_to_hsl(base.r, base.g, base.b);
                        set_rgb(&mut base, hsl_to_rgb((h + deg).rem_euclid(360.0), s, l));
                    }
                }
                "lum" => {
                    if let Some(f) = f {
                        let (h, s, _) = rgb_to_hsl(base.r, base.g, base.b);
                        set_rgb(&mut base, hsl_to_rgb(h, s, f.clamp(0.0, 1.0)));
                    }
                }
                "gray" => {
                    let (h, _, l) = rgb_to_hsl(base.r, base.g, base.b);
                    set_rgb(&mut base, hsl_to_rgb(h, 0.0, l));
                }
                "inv" => {
                    base.r = 255.0 - base.r;
                    base.g = 255.0 - base.g;
                    base.b = 255.0 - base.b;
                }
                _ => {} // gamma, comp, red/green/blueMod … ignored.
            }
        }
        base.r = base.r.clamp(0.0, 255.0);
        base.g = base.g.clamp(0.0, 255.0);
        base.b = base.b.clamp(0.0, 255.0);
        base.a = base.a.clamp(0.0, 1.0);
        Some(base)
    }
}

fn set_rgb(c: &mut Rgba, rgb: (f64, f64, f64)) {
    c.r = rgb.0;
    c.g = rgb.1;
    c.b = rgb.2;
}

/// RGB (each 0..255) → HSL with hue in degrees (0..360), s/l in 0..1.
fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let (r, g, b) = (r / 255.0, g / 255.0, b / 255.0);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let d = max - min;
    if d.abs() < 1e-9 {
        return (0.0, 0.0, l); // achromatic
    }
    let s = if l > 0.5 { d / (2.0 - max - min) } else { d / (max + min) };
    let mut h = if max == r {
        (g - b) / d + if g < b { 6.0 } else { 0.0 }
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    };
    h *= 60.0;
    (h, s, l)
}

/// Inverse of [`rgb_to_hsl`]; returns RGB in 0..255.
fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let h = h.rem_euclid(360.0);
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h / 60.0;
    let x = c * (1.0 - (hp % 2.0 - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    ((r1 + m) * 255.0, (g1 + m) * 255.0, (b1 + m) * 255.0)
}

fn default_map_slot(val: &str) -> String {
    match val {
        "bg1" => "lt1",
        "tx1" => "dk1",
        "bg2" => "lt2",
        "tx2" => "dk2",
        other => other,
    }
    .to_string()
}

fn sys_color(node: Node) -> Rgba {
    if let Some(last) = a(node, "lastClr").and_then(parse_hex) {
        return last;
    }
    match a(node, "val").unwrap_or("windowText") {
        "window" => Rgba::new(255, 255, 255),
        _ => Rgba::new(0, 0, 0),
    }
}

fn scrgb_color(node: Node) -> Option<Rgba> {
    let pct = |name: &str| {
        a(node, name)
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| (v / 100000.0 * 255.0).clamp(0.0, 255.0))
    };
    Some(Rgba {
        r: pct("r")?,
        g: pct("g")?,
        b: pct("b")?,
        a: 1.0,
    })
}

/// The DrawingML `ST_PresetColorVal` table (`<a:prstClr val="…"/>`). Unknown
/// names fall back to black. Values are the standard X11/HTML hexes; the many
/// `dk*`/`lt*`/`med*` spellings are DrawingML aliases of `dark*`/`light*`/
/// `medium*`.
fn preset_color(name: &str) -> Rgba {
    let hex = match name {
        "aliceBlue" => 0xF0F8FF,
        "antiqueWhite" => 0xFAEBD7,
        "aqua" => 0x00FFFF,
        "aquamarine" => 0x7FFFD4,
        "azure" => 0xF0FFFF,
        "beige" => 0xF5F5DC,
        "bisque" => 0xFFE4C4,
        "black" => 0x000000,
        "blanchedAlmond" => 0xFFEBCD,
        "blue" => 0x0000FF,
        "blueViolet" => 0x8A2BE2,
        "brown" => 0xA52A2A,
        "burlyWood" => 0xDEB887,
        "cadetBlue" => 0x5F9EA0,
        "chartreuse" => 0x7FFF00,
        "chocolate" => 0xD2691E,
        "coral" => 0xFF7F50,
        "cornflowerBlue" => 0x6495ED,
        "cornsilk" => 0xFFF8DC,
        "crimson" => 0xDC143C,
        "cyan" => 0x00FFFF,
        "darkBlue" | "dkBlue" => 0x00008B,
        "darkCyan" | "dkCyan" => 0x008B8B,
        "darkGoldenrod" | "dkGoldenrod" => 0xB8860B,
        "darkGray" | "darkGrey" | "dkGray" | "dkGrey" => 0xA9A9A9,
        "darkGreen" | "dkGreen" => 0x006400,
        "darkKhaki" | "dkKhaki" => 0xBDB76B,
        "darkMagenta" | "dkMagenta" => 0x8B008B,
        "darkOliveGreen" | "dkOliveGreen" => 0x556B2F,
        "darkOrange" | "dkOrange" => 0xFF8C00,
        "darkOrchid" | "dkOrchid" => 0x9932CC,
        "darkRed" | "dkRed" => 0x8B0000,
        "darkSalmon" | "dkSalmon" => 0xE9967A,
        "darkSeaGreen" | "dkSeaGreen" => 0x8FBC8F,
        "darkSlateBlue" | "dkSlateBlue" => 0x483D8B,
        "darkSlateGray" | "darkSlateGrey" | "dkSlateGray" | "dkSlateGrey" => 0x2F4F4F,
        "darkTurquoise" | "dkTurquoise" => 0x00CED1,
        "darkViolet" | "dkViolet" => 0x9400D3,
        "deepPink" => 0xFF1493,
        "deepSkyBlue" => 0x00BFFF,
        "dimGray" | "dimGrey" => 0x696969,
        "dodgerBlue" => 0x1E90FF,
        "firebrick" => 0xB22222,
        "floralWhite" => 0xFFFAF0,
        "forestGreen" => 0x228B22,
        "fuchsia" => 0xFF00FF,
        "gainsboro" => 0xDCDCDC,
        "ghostWhite" => 0xF8F8FF,
        "gold" => 0xFFD700,
        "goldenrod" => 0xDAA520,
        "gray" | "grey" => 0x808080,
        "green" => 0x008000,
        "greenYellow" => 0xADFF2F,
        "honeydew" => 0xF0FFF0,
        "hotPink" => 0xFF69B4,
        "indianRed" => 0xCD5C5C,
        "indigo" => 0x4B0082,
        "ivory" => 0xFFFFF0,
        "khaki" => 0xF0E68C,
        "lavender" => 0xE6E6FA,
        "lavenderBlush" => 0xFFF0F5,
        "lawnGreen" => 0x7CFC00,
        "lemonChiffon" => 0xFFFACD,
        "lightBlue" | "ltBlue" => 0xADD8E6,
        "lightCoral" | "ltCoral" => 0xF08080,
        "lightCyan" | "ltCyan" => 0xE0FFFF,
        "lightGoldenrodYellow" | "ltGoldenrodYellow" => 0xFAFAD2,
        "lightGray" | "lightGrey" | "ltGray" | "ltGrey" => 0xD3D3D3,
        "lightGreen" | "ltGreen" => 0x90EE90,
        "lightPink" | "ltPink" => 0xFFB6C1,
        "lightSalmon" | "ltSalmon" => 0xFFA07A,
        "lightSeaGreen" | "ltSeaGreen" => 0x20B2AA,
        "lightSkyBlue" | "ltSkyBlue" => 0x87CEFA,
        "lightSlateGray" | "lightSlateGrey" | "ltSlateGray" | "ltSlateGrey" => 0x778899,
        "lightSteelBlue" | "ltSteelBlue" => 0xB0C4DE,
        "lightYellow" | "ltYellow" => 0xFFFFE0,
        "lime" => 0x00FF00,
        "limeGreen" => 0x32CD32,
        "linen" => 0xFAF0E6,
        "magenta" => 0xFF00FF,
        "maroon" => 0x800000,
        "mediumAquamarine" | "medAquamarine" => 0x66CDAA,
        "mediumBlue" | "medBlue" => 0x0000CD,
        "mediumOrchid" | "medOrchid" => 0xBA55D3,
        "mediumPurple" | "medPurple" => 0x9370DB,
        "mediumSeaGreen" | "medSeaGreen" => 0x3CB371,
        "mediumSlateBlue" | "medSlateBlue" => 0x7B68EE,
        "mediumSpringGreen" | "medSpringGreen" => 0x00FA9A,
        "mediumTurquoise" | "medTurquoise" => 0x48D1CC,
        "mediumVioletRed" | "medVioletRed" => 0xC71585,
        "midnightBlue" => 0x191970,
        "mintCream" => 0xF5FFFA,
        "mistyRose" => 0xFFE4E1,
        "moccasin" => 0xFFE4B5,
        "navajoWhite" => 0xFFDEAD,
        "navy" => 0x000080,
        "oldLace" => 0xFDF5E6,
        "olive" => 0x808000,
        "oliveDrab" => 0x6B8E23,
        "orange" => 0xFFA500,
        "orangeRed" => 0xFF4500,
        "orchid" => 0xDA70D6,
        "paleGoldenrod" => 0xEEE8AA,
        "paleGreen" => 0x98FB98,
        "paleTurquoise" => 0xAFEEEE,
        "paleVioletRed" => 0xDB7093,
        "papayaWhip" => 0xFFEFD5,
        "peachPuff" => 0xFFDAB9,
        "peru" => 0xCD853F,
        "pink" => 0xFFC0CB,
        "plum" => 0xDDA0DD,
        "powderBlue" => 0xB0E0E6,
        "purple" => 0x800080,
        "red" => 0xFF0000,
        "rosyBrown" => 0xBC8F8F,
        "royalBlue" => 0x4169E1,
        "saddleBrown" => 0x8B4513,
        "salmon" => 0xFA8072,
        "sandyBrown" => 0xF4A460,
        "seaGreen" => 0x2E8B57,
        "seaShell" => 0xFFF5EE,
        "sienna" => 0xA0522D,
        "silver" => 0xC0C0C0,
        "skyBlue" => 0x87CEEB,
        "slateBlue" => 0x6A5ACD,
        "slateGray" | "slateGrey" => 0x708090,
        "snow" => 0xFFFAFA,
        "springGreen" => 0x00FF7F,
        "steelBlue" => 0x4682B4,
        "tan" => 0xD2B48C,
        "teal" => 0x008080,
        "thistle" => 0xD8BFD8,
        "tomato" => 0xFF6347,
        "turquoise" => 0x40E0D0,
        "violet" => 0xEE82EE,
        "wheat" => 0xF5DEB3,
        "white" => 0xFFFFFF,
        "whiteSmoke" => 0xF5F5F5,
        "yellow" => 0xFFFF00,
        "yellowGreen" => 0x9ACD32,
        _ => 0x000000,
    };
    Rgba::new((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[cfg(test)]
mod tests {
    use super::*;
    use roxmltree::Document;

    const A: &str = r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main""#;

    fn color(xml: &str) -> Rgba {
        let doc = Document::parse(xml).unwrap();
        Theme::default().parse_color(doc.root_element()).unwrap()
    }

    #[test]
    fn satmod_zero_desaturates_to_gray() {
        // Fully desaturating a saturated color yields a neutral gray (r≈g≈b).
        let c = color(&format!(r#"<a:srgbClr {A} val="FF0000"><a:satMod val="0"/></a:srgbClr>"#));
        assert!(
            (c.r - c.g).abs() < 2.0 && (c.g - c.b).abs() < 2.0,
            "expected gray, got {}",
            c.hex()
        );
    }

    #[test]
    fn preset_colors_match_table() {
        assert_eq!(color(&format!(r#"<a:prstClr {A} val="cornflowerBlue"/>"#)).hex(), "#6495ED");
        assert_eq!(color(&format!(r#"<a:prstClr {A} val="ltGray"/>"#)).hex(), "#D3D3D3");
        assert_eq!(color(&format!(r#"<a:prstClr {A} val="medBlue"/>"#)).hex(), "#0000CD");
    }

    #[test]
    fn inv_transform_inverts_rgb() {
        let c = color(&format!(r#"<a:srgbClr {A} val="112233"><a:inv/></a:srgbClr>"#));
        assert_eq!(c.hex(), "#EEDDCC");
    }

    #[test]
    fn hueoff_rotates_hue_120_degrees() {
        // Red rotated +120° (7_200_000 in 60000ths) → green.
        let c =
            color(&format!(r#"<a:srgbClr {A} val="FF0000"><a:hueOff val="7200000"/></a:srgbClr>"#));
        assert_eq!(c.hex(), "#00FF00");
    }

    #[test]
    fn lummod_unchanged_by_hsl_additions() {
        // Regression: lumMod stays RGB-scaling; 0x80 * 0.5 = 0x40.
        let c =
            color(&format!(r#"<a:srgbClr {A} val="808080"><a:lumMod val="50000"/></a:srgbClr>"#));
        assert_eq!(c.hex(), "#404040");
    }
}
