//! Bundled metric-compatible substitute fonts and named CSS fallback chains for
//! **unembedded** Microsoft Office fonts.
//!
//! Office decks routinely name fonts the author never embedded — Calibri,
//! Cambria, Segoe UI, Consolas, … . On a machine without those fonts installed
//! (every stock macOS box lacks Calibri and Cambria) both the webview SVG
//! preview and the resvg/fontdb export path would otherwise fall straight
//! through to Helvetica. Two mechanisms fix that, and both feed off the *same*
//! source of truth here so preview and export resolve identically:
//!
//! 1. [`fallback_families`] — the richer, named CSS fallback chain the renderer
//!    appends after the authored typeface (`render::text::font_family`). For
//!    Calibri/Cambria the chain leads with the bundled clone; for fonts macOS
//!    already covers (Segoe UI, Consolas, Constantia, …) it is named-fallback
//!    only — no bundled bytes.
//! 2. [`BUNDLED_FONTS`] / [`register_bundled_fonts`] — the actual clone bytes
//!    (Carlito for Calibri, Caladea for Cambria), each in all four weight/style
//!    variants, under the SIL Open Font License (see `assets/fonts/`). The
//!    exporter registers them into its [`fontdb::Database`] so a "…, Carlito, …"
//!    list resolves during rasterization on any OS; the full-tier preview embeds
//!    the matching variant as an `@font-face` data-URI (grid thumbnails stay
//!    lean — they never embed substitutes; see `render::text`).
//!
//! We deliberately do **not** bundle Arial/Times/Georgia clones: macOS ships
//! Arial, Times New Roman and Georgia, so those (and the humanist/UI/mono Office
//! faces) are covered by named fallback chains alone.

/// One bundled substitute face: the family name fontdb and CSS `@font-face`
/// index it under, its weight/style, and the raw (validated TrueType) bytes.
#[derive(Debug, Clone, Copy)]
pub struct BundledFace {
    /// The bundled clone's own family name (`"Carlito"` / `"Caladea"`) — the
    /// name it carries in its `name` table and the one the fallback chain lists.
    pub family: &'static str,
    pub bold: bool,
    pub italic: bool,
    /// Raw TTF bytes, compiled into the binary via `include_bytes!`.
    pub bytes: &'static [u8],
}

/// Every bundled substitute face (2 families × 4 variants). Sourced verbatim
/// from `google/fonts` under the OFL — see `assets/fonts/README.md`.
pub static BUNDLED_FONTS: &[BundledFace] = &[
    // Carlito — metric-compatible with Calibri.
    BundledFace {
        family: "Carlito",
        bold: false,
        italic: false,
        bytes: include_bytes!("../assets/fonts/carlito/Carlito-Regular.ttf"),
    },
    BundledFace {
        family: "Carlito",
        bold: true,
        italic: false,
        bytes: include_bytes!("../assets/fonts/carlito/Carlito-Bold.ttf"),
    },
    BundledFace {
        family: "Carlito",
        bold: false,
        italic: true,
        bytes: include_bytes!("../assets/fonts/carlito/Carlito-Italic.ttf"),
    },
    BundledFace {
        family: "Carlito",
        bold: true,
        italic: true,
        bytes: include_bytes!("../assets/fonts/carlito/Carlito-BoldItalic.ttf"),
    },
    // Caladea — metric-compatible with Cambria.
    BundledFace {
        family: "Caladea",
        bold: false,
        italic: false,
        bytes: include_bytes!("../assets/fonts/caladea/Caladea-Regular.ttf"),
    },
    BundledFace {
        family: "Caladea",
        bold: true,
        italic: false,
        bytes: include_bytes!("../assets/fonts/caladea/Caladea-Bold.ttf"),
    },
    BundledFace {
        family: "Caladea",
        bold: false,
        italic: true,
        bytes: include_bytes!("../assets/fonts/caladea/Caladea-Italic.ttf"),
    },
    BundledFace {
        family: "Caladea",
        bold: true,
        italic: true,
        bytes: include_bytes!("../assets/fonts/caladea/Caladea-BoldItalic.ttf"),
    },
];

/// The bundled family that metric-clones `typeface`, if we ship one (case- and
/// whitespace-insensitive). `Calibri`/`Calibri Light` → `Carlito`;
/// `Cambria`/`Cambria Math` → `Caladea`; anything else → `None`.
pub fn bundled_substitute(typeface: &str) -> Option<&'static str> {
    let f = typeface.trim().to_ascii_lowercase();
    if f.contains("calibri") {
        Some("Carlito")
    } else if f.contains("cambria") {
        Some("Caladea")
    } else {
        None
    }
}

/// The specific bundled face for (`family`, `bold`, `italic`), if shipped.
/// `family` is a bundled family name (`"Carlito"`/`"Caladea"`), matched
/// case-insensitively. Used by the full-tier preview to embed the exact variant
/// a slide referenced.
pub fn bundled_face(family: &str, bold: bool, italic: bool) -> Option<&'static BundledFace> {
    BUNDLED_FONTS
        .iter()
        .find(|f| f.family.eq_ignore_ascii_case(family) && f.bold == bold && f.italic == italic)
}

/// The named CSS fallback chain to append *after* the authored `typeface`
/// (which the renderer always emits first, unchanged). Returns `None` for
/// typefaces we have no special mapping for — the caller then appends its
/// generic `Helvetica, Arial, sans-serif` tail.
///
/// Every returned chain leads with the closest widely-available family and ends
/// in a CSS generic keyword, so neither the webview nor the exporter (whose
/// generics are mapped to a real installed face — see
/// [`crate::export::set_generic_families`]) ever dangles into invisible text.
/// The chains are ordered macOS-first, then the cross-platform staples.
pub fn fallback_families(typeface: &str) -> Option<&'static [&'static str]> {
    let f = typeface.trim().to_ascii_lowercase();
    // Serif families — must be checked before the sans branches so e.g.
    // "Cambria" doesn't get miscaught. Calibri/Cambria lead with the bundled
    // clone (Carlito/Caladea); the exporter has those bytes in fontdb and the
    // full-tier preview embeds them, so the clone wins on any machine.
    if f.contains("calibri") {
        Some(&["Carlito", "Helvetica Neue", "Arial", "sans-serif"])
    } else if f.contains("cambria") {
        Some(&["Caladea", "Georgia", "Times New Roman", "serif"])
    } else if f.contains("constantia") {
        // Constantia — a warm transitional serif; Georgia is the closest staple.
        Some(&["Georgia", "Times New Roman", "serif"])
    } else if f.contains("consolas") {
        // Consolas — a humanist monospace; Menlo (macOS) then Courier New.
        Some(&["Menlo", "Courier New", "monospace"])
    } else if f.contains("segoe") {
        // Segoe UI (+ Semibold/Semilight/…) — Windows' UI grotesque; the macOS
        // system grotesque Helvetica Neue is the natural stand-in.
        Some(&["Helvetica Neue", "Arial", "sans-serif"])
    } else if f.contains("candara") || f.contains("corbel") {
        // Candara/Corbel — humanist sans; Optima is the macOS humanist staple.
        Some(&["Optima", "Helvetica Neue", "Arial", "sans-serif"])
    } else if f.contains("aptos") {
        // Aptos — Office 2024's new default grotesque (not on macOS). ("Aptos
        // Narrow" is handled by the renderer's condensed path before this.)
        Some(&["Helvetica Neue", "Arial", "sans-serif"])
    } else {
        None
    }
}

/// Load every bundled substitute face into `db`, so a font-family list that
/// names one (e.g. `"Calibri, Carlito, …"`) resolves during rasterization even
/// on a machine without the real Office font installed. The exporter's shared
/// system-font database calls this once ([`crate::export::system_fonts`]);
/// per-deck databases inherit the faces through their clone.
pub fn register_bundled_fonts(db: &mut fontdb::Database) {
    for face in BUNDLED_FONTS {
        db.load_font_data(face.bytes.to_vec());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibri_and_cambria_lead_with_the_bundled_clone() {
        assert_eq!(bundled_substitute("Calibri"), Some("Carlito"));
        assert_eq!(bundled_substitute("calibri light"), Some("Carlito"));
        assert_eq!(bundled_substitute("CAMBRIA"), Some("Caladea"));
        assert_eq!(bundled_substitute("Cambria Math"), Some("Caladea"));

        // The chain leads with the clone, then widely-available fallbacks, and
        // ends in a CSS generic.
        assert_eq!(
            fallback_families("Calibri"),
            Some(&["Carlito", "Helvetica Neue", "Arial", "sans-serif"][..])
        );
        assert_eq!(
            fallback_families("Cambria"),
            Some(&["Caladea", "Georgia", "Times New Roman", "serif"][..])
        );
        // Every chain ends in a CSS generic so exports never dangle.
        for probe in ["Calibri", "Cambria", "Segoe UI", "Consolas", "Constantia", "Candara", "Aptos"]
        {
            let chain = fallback_families(probe).unwrap_or_else(|| panic!("{probe} has a chain"));
            let last = chain.last().copied().unwrap();
            assert!(
                matches!(last, "sans-serif" | "serif" | "monospace"),
                "{probe} chain must end in a CSS generic, got {last:?}"
            );
        }
    }

    #[test]
    fn unmapped_fonts_pass_through() {
        // A font we ship no clone for and give no special chain — the renderer
        // keeps its default `…, Helvetica, Arial, sans-serif` tail.
        assert_eq!(bundled_substitute("Papyrus"), None);
        assert_eq!(fallback_families("Papyrus"), None);
        assert_eq!(bundled_substitute(""), None);
        assert_eq!(fallback_families("   "), None);
    }

    #[test]
    fn bundled_faces_cover_all_four_variants_and_are_valid_sfnt() {
        for family in ["Carlito", "Caladea"] {
            for (bold, italic) in [(false, false), (true, false), (false, true), (true, true)] {
                let face = bundled_face(family, bold, italic)
                    .unwrap_or_else(|| panic!("{family} {bold}/{italic} bundled"));
                // Valid TrueType sfnt magic (0x00010000).
                assert_eq!(
                    face.bytes.get(..4),
                    Some(&[0x00, 0x01, 0x00, 0x00][..]),
                    "{family} {bold}/{italic} must be valid TrueType"
                );
            }
        }
        assert_eq!(BUNDLED_FONTS.len(), 8, "2 families × 4 variants");
    }

    /// The deterministic export contract: with ONLY the bundled fonts loaded,
    /// the exact font-family chain the renderer emits for a Calibri run must
    /// resolve, via fontdb, to the Carlito clone — this is what makes an
    /// unembedded-Calibri deck rasterize with Carlito instead of vanishing.
    #[test]
    fn calibri_chain_resolves_to_carlito_in_a_bundled_only_database() {
        let mut db = fontdb::Database::new();
        register_bundled_fonts(&mut db);

        // "Calibri, Carlito, Helvetica Neue, Arial, sans-serif" — Calibri /
        // Helvetica Neue / Arial are absent from a bundled-only db, so fontdb
        // must fall to Carlito.
        let query = fontdb::Query {
            families: &[
                fontdb::Family::Name("Calibri"),
                fontdb::Family::Name("Carlito"),
                fontdb::Family::Name("Helvetica Neue"),
                fontdb::Family::Name("Arial"),
            ],
            ..fontdb::Query::default()
        };
        let id = db.query(&query).expect("Calibri chain resolves in a bundled-only db");
        let face = db.face(id).expect("queried face exists");
        assert!(
            face.families.iter().any(|(fam, _)| fam == "Carlito"),
            "Calibri chain resolved to {:?}, expected Carlito",
            face.families
        );

        // And Cambria → Caladea, same way.
        let query = fontdb::Query {
            families: &[fontdb::Family::Name("Cambria"), fontdb::Family::Name("Caladea")],
            ..fontdb::Query::default()
        };
        let id = db.query(&query).expect("Cambria chain resolves");
        let face = db.face(id).expect("queried face exists");
        assert!(
            face.families.iter().any(|(fam, _)| fam == "Caladea"),
            "Cambria chain resolved to {:?}, expected Caladea",
            face.families
        );
    }
}
