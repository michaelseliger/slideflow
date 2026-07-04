//! Theme color resolution: scheme/clrMap slots and DrawingML color parsing.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use super::{a, ch};

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
        let mut base = match node.tag_name().name() {
            "srgbClr" => a(node, "val").and_then(parse_hex)?,
            "sysClr" => sys_color(node),
            "schemeClr" => self.resolve_scheme(a(node, "val")?),
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
                _ => {} // satMod, hueMod, gamma, inv, gray … ignored.
            }
        }
        base.r = base.r.clamp(0.0, 255.0);
        base.g = base.g.clamp(0.0, 255.0);
        base.b = base.b.clamp(0.0, 255.0);
        base.a = base.a.clamp(0.0, 1.0);
        Some(base)
    }
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

fn preset_color(name: &str) -> Rgba {
    match name {
        "black" => Rgba::new(0, 0, 0),
        "white" => Rgba::new(255, 255, 255),
        "red" => Rgba::new(255, 0, 0),
        "green" => Rgba::new(0, 128, 0),
        "blue" => Rgba::new(0, 0, 255),
        "yellow" => Rgba::new(255, 255, 0),
        "gray" | "grey" => Rgba::new(128, 128, 128),
        "cyan" => Rgba::new(0, 255, 255),
        "magenta" => Rgba::new(255, 0, 255),
        _ => Rgba::new(0, 0, 0),
    }
}
