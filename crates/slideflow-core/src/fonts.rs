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
/// `Cambria`/`Cambria Math` → `Caladea`; anything else → `None`. The clone names
/// self-map (`Carlito` → `Carlito`, `Caladea` → `Caladea`) because LibreOffice
/// decks author the clone name outright — the full-tier preview then embeds the
/// bundled bytes for it, matching the exporter (which carries them in fontdb).
pub fn bundled_substitute(typeface: &str) -> Option<&'static str> {
    let f = typeface.trim().to_ascii_lowercase();
    if f.contains("calibri") || f.contains("carlito") {
        Some("Carlito")
    } else if f.contains("cambria") || f.contains("caladea") {
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
    } else if f.contains("carlito") {
        // A deck that authors the Calibri clone by name (LibreOffice writes
        // "Carlito"). The authored name already leads the emitted font-family
        // list, so the chain must NOT repeat it — go straight to the staples.
        Some(&["Helvetica Neue", "Arial", "sans-serif"])
    } else if f.contains("cambria") {
        Some(&["Caladea", "Georgia", "Times New Roman", "serif"])
    } else if f.contains("caladea") {
        // Same, for the directly-authored Cambria clone.
        Some(&["Georgia", "Times New Roman", "serif"])
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

// ---------------------------------------------------------------------------
// App-local fonts (harvested / user-added / downloaded)
// ---------------------------------------------------------------------------

/// One app-local font face — a real font the user owns or we legally fetched,
/// living under `<app_data>/fonts/` (harvested from a deck, user-added, or
/// downloaded). Unlike the bundled *substitutes*, its `family` is the **real**
/// authored name a deck references (`"VilleroyBoch"`, `"Karla"`, even a licensed
/// `"Calibri"`), so it wins the `font-family` chain the renderer emits — the
/// authored name is always first.
#[derive(Clone)]
pub struct AppFontFace {
    /// Family name from the font's `name` table — what a deck's
    /// `<a:latin typeface>` must match (case-insensitively) to use this face.
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Raw (validated) TTF/OTF bytes, shared so cloning a set is a refcount bump
    /// rather than copying megabytes.
    pub bytes: std::sync::Arc<Vec<u8>>,
}

// Hand-written so a `Debug`-printed RenderOptions logs the face's identity, not
// its (potentially multi-MB) byte buffer.
impl std::fmt::Debug for AppFontFace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppFontFace")
            .field("family", &self.family)
            .field("bold", &self.bold)
            .field("italic", &self.italic)
            .field("bytes", &self.bytes.len())
            .finish()
    }
}

/// The set of app-local fonts available to one render/export, built by the
/// desktop host from `<app_data>/fonts/` and injected into the engine via
/// [`crate::render::RenderOptions::app_fonts`].
///
/// The engine NEVER reads the fonts directory itself — it only consults the set
/// the host hands it — so core stays filesystem-side-effect-free and network-
/// free, and tests inject in-memory sets. The preview path embeds a used face as
/// an `@font-face` data-URI (full/peek tier only, mirroring the bundled-
/// substitute size policy); the export path registers the same bytes into its
/// `fontdb::Database` via [`AppFontSet::register`].
#[derive(Debug, Clone, Default)]
pub struct AppFontSet {
    faces: Vec<AppFontFace>,
}

impl AppFontSet {
    pub fn new(faces: Vec<AppFontFace>) -> Self {
        AppFontSet { faces }
    }

    pub fn is_empty(&self) -> bool {
        self.faces.is_empty()
    }

    pub fn faces(&self) -> &[AppFontFace] {
        &self.faces
    }

    /// Distinct family names present (case preserved; first occurrence kept).
    pub fn families(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for f in &self.faces {
            if !out.iter().any(|e| e.eq_ignore_ascii_case(&f.family)) {
                out.push(f.family.clone());
            }
        }
        out
    }

    /// Whether any face carries `family` (case-insensitive).
    pub fn has_family(&self, family: &str) -> bool {
        self.faces.iter().any(|f| f.family.eq_ignore_ascii_case(family))
    }

    /// The best face for (`family`, `bold`, `italic`): an exact variant match if
    /// present, else the family's regular (upright, non-bold) face, else any face
    /// of that family. `None` when the set has no face for `family` at all — the
    /// caller then falls through to the bundled substitute / generic tail.
    pub fn best_face(&self, family: &str, bold: bool, italic: bool) -> Option<&AppFontFace> {
        let same = |f: &&AppFontFace| f.family.eq_ignore_ascii_case(family);
        self.faces
            .iter()
            .find(|f| same(f) && f.bold == bold && f.italic == italic)
            .or_else(|| self.faces.iter().find(|f| same(f) && !f.bold && !f.italic))
            .or_else(|| self.faces.iter().find(same))
    }

    /// Load every app face into `db` so a `font-family` list that names one
    /// resolves during rasterization (the export path — usvg ignores SVG
    /// `@font-face`). Faces index under their own `name` table.
    pub fn register(&self, db: &mut fontdb::Database) {
        for f in &self.faces {
            db.load_font_data((*f.bytes).clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn face(family: &str, bold: bool, italic: bool) -> AppFontFace {
        AppFontFace { family: family.to_string(), bold, italic, bytes: Arc::new(sample_bytes()) }
    }

    // Distinct dummy bytes per call so `register` can be told faces apart. Not a
    // valid font — only `best_face`/`families`/`has_family` are exercised here.
    fn sample_bytes() -> Vec<u8> {
        vec![0x00, 0x01, 0x00, 0x00]
    }

    #[test]
    fn best_face_prefers_exact_then_regular_then_any() {
        let set = AppFontSet::new(vec![
            face("VilleroyBoch", false, false),
            face("VilleroyBoch", true, false),
        ]);
        // Exact variant wins.
        assert!(set.best_face("villeroyboch", true, false).unwrap().bold);
        // No italic face → falls back to the regular (upright, non-bold) face.
        let it = set.best_face("VilleroyBoch", false, true).unwrap();
        assert!(!it.bold && !it.italic, "italic request falls back to regular");
        // Unknown family → None.
        assert!(set.best_face("Karla", false, false).is_none());
    }

    /// The export contract: an app-font set registered into a `fontdb::Database`
    /// resolves by its real family name during rasterization (usvg ignores SVG
    /// `@font-face`, so the exporter needs the bytes fontdb-side). Uses the real,
    /// fontdb-parseable bundled Carlito bytes as a stand-in app face — the
    /// minimal fixture font has no OS/2 table and fontdb rejects it.
    #[test]
    fn app_font_register_resolves_in_export_fontdb() {
        let carlito = bundled_face("Carlito", false, false).expect("bundled Carlito regular");
        let set = AppFontSet::new(vec![AppFontFace {
            family: carlito.family.to_string(),
            bold: false,
            italic: false,
            bytes: Arc::new(carlito.bytes.to_vec()),
        }]);
        let mut db = fontdb::Database::new();
        set.register(&mut db);
        let id = db
            .query(&fontdb::Query {
                families: &[fontdb::Family::Name("Carlito")],
                ..fontdb::Query::default()
            })
            .expect("registered app face resolves in the export fontdb");
        let face = db.face(id).expect("queried face exists");
        assert!(
            face.families.iter().any(|(f, _)| f == "Carlito"),
            "resolved to {:?}, expected the registered app face",
            face.families
        );
    }

    #[test]
    fn families_and_has_family_are_case_insensitive_and_deduped() {
        let set = AppFontSet::new(vec![
            face("Karla", false, false),
            face("Karla", true, false),
            face("Aptos", false, false),
        ]);
        assert_eq!(set.families(), vec!["Karla".to_string(), "Aptos".to_string()]);
        assert!(set.has_family("karla") && set.has_family("APTOS"));
        assert!(!set.has_family("Calibri"));
    }

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
        for probe in [
            "Calibri", "Carlito", "Cambria", "Caladea", "Segoe UI", "Consolas", "Constantia",
            "Candara", "Aptos",
        ] {
            let chain = fallback_families(probe).unwrap_or_else(|| panic!("{probe} has a chain"));
            let last = chain.last().copied().unwrap();
            assert!(
                matches!(last, "sans-serif" | "serif" | "monospace"),
                "{probe} chain must end in a CSS generic, got {last:?}"
            );
        }
    }

    #[test]
    fn directly_authored_carlito_and_caladea_self_map_to_bundled_bytes() {
        // LibreOffice decks author the clone name outright ("Carlito"/"Caladea").
        // The substitute map must self-map them so the full-tier preview embeds the
        // bundled bytes (matching the exporter, which carries them in fontdb)
        // instead of embedding nothing and falling to Helvetica.
        assert_eq!(bundled_substitute("Carlito"), Some("Carlito"));
        assert_eq!(bundled_substitute("carlito"), Some("Carlito"));
        assert_eq!(bundled_substitute("Caladea"), Some("Caladea"));
        assert_eq!(bundled_substitute("CALADEA"), Some("Caladea"));
        // The bundled bytes for the authored variant resolve.
        assert!(bundled_face("Carlito", true, false).is_some());
        assert!(bundled_face("Caladea", false, true).is_some());

        // The named chain must NOT repeat the authored clone name — the renderer
        // emits "Carlito, <chain>", so a self-reference would dupe it.
        let carlito = fallback_families("Carlito").expect("Carlito has a chain");
        assert_eq!(carlito, &["Helvetica Neue", "Arial", "sans-serif"]);
        assert!(
            !carlito.iter().any(|f| f.eq_ignore_ascii_case("carlito")),
            "Carlito chain must not self-reference"
        );
        let caladea = fallback_families("Caladea").expect("Caladea has a chain");
        assert_eq!(caladea, &["Georgia", "Times New Roman", "serif"]);
        assert!(
            !caladea.iter().any(|f| f.eq_ignore_ascii_case("caladea")),
            "Caladea chain must not self-reference"
        );
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
