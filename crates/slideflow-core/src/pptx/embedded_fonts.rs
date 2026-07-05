//! Extraction of fonts embedded in a deck (`<p:embeddedFontLst>` in
//! `presentation.xml`).
//!
//! PowerPoint can embed the actual TrueType/OpenType files used by a deck so it
//! renders identically on machines that lack those fonts. Each
//! `<p:embeddedFont>` names a `typeface` and points, per weight/style variant
//! (`regular`/`bold`/`italic`/`boldItalic`), at a `/ppt/fonts/*.fntdata` part via
//! an `r:id` relationship. In a normal PPTX those parts are the **raw** TTF/OTF
//! bytes; we validate the magic bytes and skip anything else (e.g. the obfuscated
//! ODTTF form used by Word/some exporters) rather than guessing — deobfuscation
//! is out of scope for v1.
//!
//! Consumers:
//! - the renderer embeds the used variants as `@font-face` in preview SVGs
//!   ([`crate::render`]), so WebKit shows the real typeface;
//! - a future rasterizer path (resvg/fontdb, WS-D) will register the same bytes
//!   in a `fontdb::Database`, because usvg does *not* honor SVG `@font-face`.
//!   See the `register_embedded_fonts` sketch at the bottom of this file.

use crate::opc::resolve_target;
use crate::pptx::PresentationFile;

/// Skip any embedded font file larger than this. Base64-encoding a multi-MB
/// font into every thumbnail SVG would bloat the thumb cache; a legitimately
/// subsetted embedded font is far smaller.
pub const MAX_FONT_BYTES: usize = 5 * 1024 * 1024;

/// One usable embedded font variant: its declared family plus the concrete
/// TTF/OTF bytes for a single weight/style.
#[derive(Debug, Clone)]
pub struct EmbeddedFont {
    /// The `typeface` declared in `<p:embeddedFontLst>` (matches the run
    /// `font-family` the renderer emits, case-insensitively).
    pub family: String,
    pub bold: bool,
    pub italic: bool,
    /// Raw font bytes (validated TTF/OTF).
    pub bytes: Vec<u8>,
}

/// An embedded font variant that was present but could not be used, with a
/// human-readable note for dropped-construct telemetry.
#[derive(Debug, Clone)]
pub struct SkippedFont {
    pub family: String,
    /// e.g. `"embedded font Arial Narrow skipped (unsupported format)"`.
    pub note: String,
}

/// The result of scanning a deck's `<p:embeddedFontLst>`: the usable variants
/// plus notes for any that were skipped (unsupported format or oversized).
#[derive(Debug, Clone, Default)]
pub struct EmbeddedFontSet {
    pub fonts: Vec<EmbeddedFont>,
    pub skipped: Vec<SkippedFont>,
}

/// Extract every usable embedded font variant from a deck. Skipped variants are
/// silently dropped; use [`embedded_font_set`] when you also need the skip
/// notes (the renderer surfaces them as dropped-construct telemetry).
pub fn embedded_fonts(pf: &PresentationFile) -> Vec<EmbeddedFont> {
    embedded_font_set(pf).fonts
}

/// Extract embedded fonts *and* the notes for any variant that was skipped.
pub fn embedded_font_set(pf: &PresentationFile) -> EmbeddedFontSet {
    let mut set = EmbeddedFontSet::default();

    let Ok(main) = pf.package.main_document_part() else {
        return set;
    };
    let Some(bytes) = pf.package.part(&main) else {
        return set;
    };
    let Ok(xml) = std::str::from_utf8(bytes) else {
        return set;
    };
    // The overwhelming majority of decks embed no fonts — bail before parsing.
    if !xml.contains("embeddedFontLst") {
        return set;
    }
    let Ok(doc) = roxmltree::Document::parse(xml) else {
        return set;
    };
    let rels = pf.package.rels_for(&main).unwrap_or_default();

    let Some(lst) = doc
        .descendants()
        .find(|n| n.is_element() && local(n) == "embeddedFontLst")
    else {
        return set;
    };

    for ef in lst.children().filter(|n| n.is_element() && local(n) == "embeddedFont") {
        // <p:font typeface="X"/> names the family; skip an entry with no name.
        let Some(family) = ef
            .children()
            .find(|n| n.is_element() && local(n) == "font")
            .and_then(|f| attr_local(&f, "typeface"))
            .map(str::to_string)
        else {
            continue;
        };

        for var in ef.children().filter(|n| n.is_element()) {
            let (bold, italic) = match local(&var) {
                "regular" => (false, false),
                "bold" => (true, false),
                "italic" => (false, true),
                "boldItalic" => (true, true),
                _ => continue, // the <p:font> element itself, and any extension
            };
            // The variant points at its font part via r:id.
            let Some(rid) = attr_local(&var, "id") else {
                continue;
            };
            let Some(rel) = rels.iter().find(|r| r.id == rid && !r.external) else {
                continue;
            };
            let part = resolve_target(&main, &rel.target);
            let Some(fbytes) = pf.package.part(&part) else {
                continue;
            };

            // Validate before cloning: the magic bytes tell us it's raw TTF/OTF
            // (not obfuscated ODTTF or garbage), and the size guard keeps thumb
            // SVGs from ballooning.
            if !is_supported_font(fbytes) {
                set.skipped.push(SkippedFont {
                    family: family.clone(),
                    note: format!("embedded font {family} skipped (unsupported format)"),
                });
                continue;
            }
            if fbytes.len() > MAX_FONT_BYTES {
                set.skipped.push(SkippedFont {
                    family: family.clone(),
                    note: format!("embedded font {family} skipped (too large)"),
                });
                continue;
            }

            set.fonts.push(EmbeddedFont {
                family: family.clone(),
                bold,
                italic,
                bytes: fbytes.to_vec(),
            });
        }
    }

    set
}

/// True if `bytes` begins with a recognized sfnt magic number: TrueType
/// (`00 01 00 00` or `true`), OpenType/CFF (`OTTO`), or a TrueType collection
/// (`ttcf`). PPTX embeds raw fonts, so anything else (notably obfuscated ODTTF)
/// is treated as unsupported.
fn is_supported_font(bytes: &[u8]) -> bool {
    matches!(bytes.get(..4), Some([0x00, 0x01, 0x00, 0x00]))
        || bytes.starts_with(b"OTTO")
        || bytes.starts_with(b"true")
        || bytes.starts_with(b"ttcf")
}

/// The data-URI media type and CSS `format()` hint for a validated font, chosen
/// from its sfnt magic. Used by the renderer when emitting `@font-face`.
pub(crate) fn font_media_type(bytes: &[u8]) -> (&'static str, &'static str) {
    match bytes.get(..4) {
        Some(b"OTTO") => ("font/otf", "opentype"),
        Some(b"ttcf") => ("font/collection", "collection"),
        _ => ("font/ttf", "truetype"),
    }
}

fn local<'a>(n: &roxmltree::Node<'a, 'a>) -> &'a str {
    n.tag_name().name()
}

/// Value of the attribute whose *local* name matches `name`, ignoring its XML
/// namespace prefix (`r:id`, `typeface`, …).
fn attr_local<'a>(n: &roxmltree::Node<'a, 'a>, name: &str) -> Option<&'a str> {
    n.attributes().find(|a| a.name() == name).map(|a| a.value())
}

// ---------------------------------------------------------------------------
// WS-D hand-off: fontdb registration for the resvg rasterizer.
//
// usvg ignores SVG `@font-face`, so the export path must register the same
// bytes in a `fontdb::Database`. `fontdb` is not yet a workspace dependency, so
// this helper is intentionally left for the lead to wire once WS-D lands it.
// Drop this in and add `use fontdb;`:
//
// pub fn register_embedded_fonts(db: &mut fontdb::Database, pf: &PresentationFile) {
//     for f in embedded_fonts(pf) {
//         // load_font_data indexes the face by its own name table; real
//         // embedded fonts carry the matching family. Callers that match by the
//         // declared `f.family` may need to set it explicitly on the loaded face.
//         db.load_font_data(f.bytes);
//     }
// }
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{sample_ttf, DeckSpec, SlideSpec};

    #[test]
    fn regular_and_bold_variants_extracted() {
        let ttf = sample_ttf();
        let bytes = DeckSpec::new("Embedded")
            .embed_font("Arial Narrow", vec![(false, false, ttf.clone()), (true, false, ttf.clone())])
            .slide(SlideSpec::new("Hi"))
            .build();
        let pf = PresentationFile::from_bytes(&bytes).unwrap();

        let fonts = embedded_fonts(&pf);
        assert_eq!(fonts.len(), 2, "regular + bold expected: {fonts:?}");
        assert!(fonts.iter().all(|f| f.family == "Arial Narrow"));
        assert!(fonts.iter().all(|f| !f.italic));
        let regular = fonts.iter().find(|f| !f.bold).expect("regular variant");
        let bold = fonts.iter().find(|f| f.bold).expect("bold variant");
        assert_eq!(regular.bytes, ttf, "raw font bytes round-trip");
        assert_eq!(bold.bytes, ttf);
    }

    #[test]
    fn non_font_bytes_are_skipped_with_note() {
        let bytes = DeckSpec::new("Bad")
            .embed_font("Obfuscated", vec![(false, false, b"NOT-A-FONT-AT-ALL".to_vec())])
            .slide(SlideSpec::new("Hi"))
            .build();
        let pf = PresentationFile::from_bytes(&bytes).unwrap();

        let set = embedded_font_set(&pf);
        assert!(set.fonts.is_empty(), "unsupported bytes must not be extracted");
        assert_eq!(set.skipped.len(), 1);
        assert_eq!(set.skipped[0].family, "Obfuscated");
        assert!(
            set.skipped[0].note.contains("unsupported format"),
            "note was {:?}",
            set.skipped[0].note
        );
    }

    #[test]
    fn oversized_font_is_skipped_with_note() {
        let mut big = sample_ttf();
        big.resize(MAX_FONT_BYTES + 16, 0); // keeps the sfnt magic, exceeds the cap
        let bytes = DeckSpec::new("Big")
            .embed_font("Huge Sans", vec![(false, false, big)])
            .slide(SlideSpec::new("Hi"))
            .build();
        let pf = PresentationFile::from_bytes(&bytes).unwrap();

        let set = embedded_font_set(&pf);
        assert!(set.fonts.is_empty());
        assert_eq!(set.skipped.len(), 1);
        assert!(set.skipped[0].note.contains("too large"), "note was {:?}", set.skipped[0].note);
    }

    #[test]
    fn deck_without_embedded_fonts_yields_nothing() {
        let bytes = DeckSpec::new("Plain").slide(SlideSpec::new("Hi")).build();
        let pf = PresentationFile::from_bytes(&bytes).unwrap();
        let set = embedded_font_set(&pf);
        assert!(set.fonts.is_empty() && set.skipped.is_empty());
    }
}
