//! Content and text hashing for duplicate detection and embedding keys.
//!
//! Two hashes are computed for every slide during indexing, both cheap and
//! unconditional (no model, no network):
//!
//! - **content hash** ([`slide_content_hash`]) — a layout/master/theme-*independent*
//!   fingerprint of a slide's authored content, so the *same* slide reused across
//!   rebranded decks clusters together. It canonicalizes the slide part's XML
//!   (normalizing relationship-id renumbering, part renames, volatile GUIDs, and
//!   attribute quoting) and folds in the bytes of every referenced media/OLE/chart
//!   part. Powers exact-duplicate detection (roadmap #9).
//! - **text hash** ([`text_hash`]) — sha256 of the exact string an embedder is fed
//!   for a slide ([`slide_embed_text`]). Embeddings are keyed by it, so identical
//!   text across decks embeds once and survives the delete+reinsert of a rescan.
//!
//! The `"sfch2:"` version prefix on the content hash lets the canonicalization
//! evolve later without a schema change (a new prefix restales every stored hash).

use std::collections::BTreeSet;
use std::io::Cursor;

use quick_xml::events::{BytesStart, Event};
use quick_xml::{Reader, Writer};
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};
use crate::opc::{local_name, rel_type, resolve_target, Package, Relationship};

/// Canonicalization version prefix. Bump to force every content hash to restale
/// when the canonicalization rules below change.
const CONTENT_HASH_VERSION: &[u8] = b"sfch2:";

/// Lowercase hex sha256 of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex(hasher.finalize())
}

fn hex(digest: impl AsRef<[u8]>) -> String {
    let mut s = String::with_capacity(digest.as_ref().len() * 2);
    for b in digest.as_ref() {
        s.push(char::from_digit((b >> 4) as u32, 16).unwrap());
        s.push(char::from_digit((b & 0xf) as u32, 16).unwrap());
    }
    s
}

/// sha256 of the exact embedder input string. Embeddings are keyed by this so
/// identical text (across decks, or across a rescan's delete+reinsert) maps to a
/// single stored vector.
pub fn text_hash(embed_input: &str) -> String {
    sha256_hex(embed_input.as_bytes())
}

/// The model-agnostic text embedded for a slide: title, body and notes joined by
/// newlines, present (non-empty) fields only. Embedders add their own task
/// prefix (E5's `passage: ` / `query: `) on top of this. Returns `None` when the
/// slide carries no indexable text — such slides get no text hash and no vector.
pub fn slide_embed_text(title: Option<&str>, body: &str, notes: Option<&str>) -> Option<String> {
    let mut parts: Vec<&str> = Vec::new();
    if let Some(t) = title {
        let t = t.trim();
        if !t.is_empty() {
            parts.push(t);
        }
    }
    let b = body.trim();
    if !b.is_empty() {
        parts.push(b);
    }
    if let Some(n) = notes {
        let n = n.trim();
        if !n.is_empty() {
            parts.push(n);
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("\n"))
    }
}

/// Content hash of a slide part: `sha256("sfch2:" + canonical_xml + media_digest)`.
///
/// `canonical_xml` is the slide part re-serialized through quick-xml events, with:
/// - every relationship-namespace (`r:`-prefixed) attribute rewritten — this
///   covers `r:id`/`r:embed`/`r:link` on media/OLE/chart refs and
///   `r:dm`/`r:lo`/`r:qs`/`r:cs` on SmartArt `<dgm:relIds>` — dropped for layout
///   refs (relationship ids are package-arbitrary), replaced by the sha256 of the
///   referenced part's bytes for internal media/OLE/chart targets, and replaced
///   by the target URI for external targets;
/// - volatile GUID carriers stripped (`p14:creationId`, `a16:creationId`, `modId`
///   in either element or attribute form).
///
/// `media_digest` is the sorted set of referenced media-part byte hashes. Layout,
/// master and theme are deliberately excluded so the same authored slide clusters
/// even when dropped into differently-branded templates.
pub fn slide_content_hash(pkg: &Package, slide_part: &str) -> Result<String> {
    let xml = pkg.require_part(slide_part)?;
    let rels = pkg.rels_for(slide_part)?;
    let mut media_hashes: BTreeSet<String> = BTreeSet::new();
    let canonical = canonicalize_slide_xml(xml, slide_part, pkg, &rels, &mut media_hashes)?;

    let mut hasher = Sha256::new();
    hasher.update(CONTENT_HASH_VERSION);
    hasher.update(&canonical);
    hasher.update(b"\x1emedia:");
    for h in &media_hashes {
        hasher.update(h.as_bytes());
        hasher.update(b",");
    }
    Ok(hex(hasher.finalize()))
}

/// Local names of elements/attributes that carry per-edit volatile GUIDs. Dropped
/// wholesale (element subtree or attribute) so they never perturb the hash.
fn is_volatile(local: &[u8]) -> bool {
    matches!(local, b"creationId" | b"modId")
}

/// Re-serialize a slide part's XML with the canonicalization rules applied.
fn canonicalize_slide_xml(
    xml: &[u8],
    slide_part: &str,
    pkg: &Package,
    rels: &[Relationship],
    media_hashes: &mut BTreeSet<String>,
) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(xml);
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();
    // Depth of a `creationId`/`modId` element subtree currently being skipped.
    let mut skip: usize = 0;

    loop {
        match reader.read_event_into(&mut buf).map_err(|e| Error::xml(slide_part, e))? {
            Event::Start(ref e) => {
                if skip > 0 {
                    skip += 1;
                } else if is_volatile(local_name(e.name().as_ref())) {
                    skip = 1;
                } else {
                    let new = rewrite_element(e, slide_part, pkg, rels, media_hashes)?;
                    writer
                        .write_event(Event::Start(new))
                        .map_err(|e| Error::xml(slide_part, e))?;
                }
            }
            Event::Empty(ref e) => {
                if skip == 0 && !is_volatile(local_name(e.name().as_ref())) {
                    let new = rewrite_element(e, slide_part, pkg, rels, media_hashes)?;
                    writer
                        .write_event(Event::Empty(new))
                        .map_err(|e| Error::xml(slide_part, e))?;
                }
            }
            Event::End(ref e) => {
                if skip > 0 {
                    skip -= 1;
                } else {
                    writer
                        .write_event(Event::End(e.clone()))
                        .map_err(|e| Error::xml(slide_part, e))?;
                }
            }
            // The XML declaration carries no authored content and varies by writer;
            // drop it for stability.
            Event::Decl(_) => {}
            Event::Eof => break,
            other => {
                if skip == 0 {
                    writer.write_event(other).map_err(|e| Error::xml(slide_part, e))?;
                }
            }
        }
        buf.clear();
    }
    Ok(writer.into_inner().into_inner())
}

/// Rebuild a start/empty element, dropping volatile attributes and rewriting
/// relationship-id references into content-derived tokens.
fn rewrite_element(
    e: &BytesStart,
    slide_part: &str,
    pkg: &Package,
    rels: &[Relationship],
    media_hashes: &mut BTreeSet<String>,
) -> Result<BytesStart<'static>> {
    let name = e.name();
    let name_str = std::str::from_utf8(name.as_ref())
        .map_err(|_| Error::InvalidPackage(format!("non-utf8 element name in {slide_part}")))?;
    let mut new = BytesStart::new(name_str.to_owned());

    for attr in e.attributes() {
        let attr = attr.map_err(|e| Error::xml(slide_part, e))?;
        let key = attr.key.as_ref();
        if is_volatile(local_name(key)) {
            continue;
        }
        let key_str = std::str::from_utf8(key)
            .map_err(|_| Error::InvalidPackage(format!("non-utf8 attribute name in {slide_part}")))?;
        let value = attr.unescape_value().map_err(|e| Error::xml(slide_part, e))?;

        // Every attribute in the relationships namespace (prefix `r:`) is an
        // ST_RelationshipId: r:id/r:embed/r:link on media/OLE/chart refs *and*
        // r:dm/r:lo/r:qs/r:cs on SmartArt `<dgm:relIds>`. Route them all through
        // classify_rel so diagram/OLE targets are both rid-normalized and folded
        // into the media digest.
        if key.starts_with(b"r:") {
            match classify_rel(&value, rels, pkg, slide_part) {
                RelToken::Drop => continue,
                RelToken::Token(token, media) => {
                    if let Some(h) = media {
                        media_hashes.insert(h);
                    }
                    new.push_attribute((key_str, token.as_str()));
                }
            }
        } else {
            new.push_attribute((key_str, value.as_ref()));
        }
    }
    Ok(new)
}

enum RelToken {
    /// Drop the attribute entirely (layout refs — ids are package-arbitrary).
    Drop,
    /// Replace the attribute value with `token`; if `Some`, also fold the media
    /// byte hash into the media digest.
    Token(String, Option<String>),
}

fn classify_rel(rid: &str, rels: &[Relationship], pkg: &Package, slide_part: &str) -> RelToken {
    let Some(rel) = rels.iter().find(|r| r.id == rid) else {
        // Unknown id (malformed package): keep the raw value so distinct refs are
        // never silently collapsed. Valid slides never hit this.
        return RelToken::Token(format!("rid:{rid}"), None);
    };
    if rel.rel_type == rel_type::SLIDE_LAYOUT {
        return RelToken::Drop;
    }
    if rel.external {
        return RelToken::Token(format!("ext:{}", rel.target), None);
    }
    let part = resolve_target(slide_part, &rel.target);
    match pkg.part(&part) {
        Some(bytes) => {
            let h = sha256_hex(bytes);
            RelToken::Token(format!("sha:{h}"), Some(h))
        }
        None => RelToken::Token(format!("missing:{part}"), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opc::{ContentTypes, Package, Relationship};

    /// Build a one-slide package whose slide XML + rels + media are given, so the
    /// canonicalizer can be exercised over hand-crafted references.
    fn pkg_with_slide(slide_xml: &str, rels: &[Relationship], media: &[(&str, &[u8])]) -> Package {
        let mut pkg = Package::default();
        let mut ct = ContentTypes::default();
        ct.ensure_default("xml", "application/xml");
        ct.ensure_default("png", "image/png");
        pkg.set_content_types(&ct);
        pkg.insert_part("ppt/slides/slide1.xml", slide_xml.as_bytes().to_vec());
        pkg.set_rels("ppt/slides/slide1.xml", rels);
        for (name, bytes) in media {
            pkg.insert_part(*name, bytes.to_vec());
        }
        pkg
    }

    fn layout_rel(id: &str) -> Relationship {
        Relationship {
            id: id.into(),
            rel_type: rel_type::SLIDE_LAYOUT.into(),
            target: "../slideLayouts/slideLayout1.xml".into(),
            external: false,
        }
    }
    fn image_rel(id: &str, target: &str) -> Relationship {
        Relationship {
            id: id.into(),
            rel_type: rel_type::IMAGE.into(),
            target: target.into(),
            external: false,
        }
    }

    #[test]
    fn stable_across_rid_renumbering_and_part_renames() {
        // Deck A: image is rId2 → media/image1.png; layout is rId1.
        let a = pkg_with_slide(
            r#"<p:sld xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:pic><p:blipFill><a:blip r:embed="rId2"/></p:blipFill></p:pic></p:spTree></p:cSld></p:sld>"#,
            &[layout_rel("rId1"), image_rel("rId2", "../media/image1.png")],
            &[("ppt/media/image1.png", b"PNGDATA")],
        );
        // Deck B: SAME authored slide, but the image is rId7 → media/pic99.png
        // (renumbered id + renamed part), and the layout id is rId9.
        let b = pkg_with_slide(
            r#"<p:sld xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:pic><p:blipFill><a:blip r:embed="rId7"/></p:blipFill></p:pic></p:spTree></p:cSld></p:sld>"#,
            &[layout_rel("rId9"), image_rel("rId7", "../media/pic99.png")],
            &[("ppt/media/pic99.png", b"PNGDATA")],
        );
        let ha = slide_content_hash(&a, "ppt/slides/slide1.xml").unwrap();
        let hb = slide_content_hash(&b, "ppt/slides/slide1.xml").unwrap();
        assert_eq!(ha, hb, "rId renumbering + media rename must not change the hash");
    }

    /// A SmartArt diagram relationship (`r:dm`/`r:lo`/`r:qs`/`r:cs` on
    /// `<dgm:relIds>`). rel_type is never SLIDE_LAYOUT and the target is a real
    /// part, so classify_rel folds its bytes into the media digest.
    fn diagram_rel(id: &str, target: &str) -> Relationship {
        Relationship {
            id: id.into(),
            rel_type:
                "http://schemas.openxmlformats.org/officeDocument/2006/relationships/diagramData"
                    .into(),
            target: target.into(),
            external: false,
        }
    }

    #[test]
    fn smartart_stable_across_rid_renumbering() {
        // Deck A: SmartArt refs rId4/5/6/7 → four diagram parts.
        let a = pkg_with_slide(
            r#"<p:sld xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:graphicFrame><a:graphic><a:graphicData><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId4" r:lo="rId5" r:qs="rId6" r:cs="rId7"/></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#,
            &[
                layout_rel("rId1"),
                diagram_rel("rId4", "../diagrams/data1.xml"),
                diagram_rel("rId5", "../diagrams/layout1.xml"),
                diagram_rel("rId6", "../diagrams/quickStyle1.xml"),
                diagram_rel("rId7", "../diagrams/colors1.xml"),
            ],
            &[
                ("ppt/diagrams/data1.xml", b"DATA"),
                ("ppt/diagrams/layout1.xml", b"LAYOUT"),
                ("ppt/diagrams/quickStyle1.xml", b"QSTYLE"),
                ("ppt/diagrams/colors1.xml", b"COLORS"),
            ],
        );
        // Deck B: SAME authored SmartArt, but renumbered ids + renamed parts.
        let b = pkg_with_slide(
            r#"<p:sld xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:cSld><p:spTree><p:graphicFrame><a:graphic><a:graphicData><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId14" r:lo="rId15" r:qs="rId16" r:cs="rId17"/></a:graphicData></a:graphic></p:graphicFrame></p:spTree></p:cSld></p:sld>"#,
            &[
                layout_rel("rId9"),
                diagram_rel("rId14", "../diagrams/data9.xml"),
                diagram_rel("rId15", "../diagrams/layout9.xml"),
                diagram_rel("rId16", "../diagrams/quickStyle9.xml"),
                diagram_rel("rId17", "../diagrams/colors9.xml"),
            ],
            &[
                ("ppt/diagrams/data9.xml", b"DATA"),
                ("ppt/diagrams/layout9.xml", b"LAYOUT"),
                ("ppt/diagrams/quickStyle9.xml", b"QSTYLE"),
                ("ppt/diagrams/colors9.xml", b"COLORS"),
            ],
        );
        assert_eq!(
            slide_content_hash(&a, "ppt/slides/slide1.xml").unwrap(),
            slide_content_hash(&b, "ppt/slides/slide1.xml").unwrap(),
            "SmartArt rId renumbering + diagram-part rename must not change the hash"
        );
    }

    #[test]
    fn smartart_changes_when_diagram_bytes_change() {
        // Identical slide XML and identical rIds, but the diagram *data* differs;
        // the two slides must not collapse into one exact-duplicate cluster.
        let slide = r#"<p:sld xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><dgm:relIds xmlns:dgm="http://schemas.openxmlformats.org/drawingml/2006/diagram" r:dm="rId4" r:lo="rId5" r:qs="rId6" r:cs="rId7"/></p:sld>"#;
        let rels = &[
            diagram_rel("rId4", "../diagrams/data1.xml"),
            diagram_rel("rId5", "../diagrams/layout1.xml"),
            diagram_rel("rId6", "../diagrams/quickStyle1.xml"),
            diagram_rel("rId7", "../diagrams/colors1.xml"),
        ];
        let a = pkg_with_slide(
            slide,
            rels,
            &[
                ("ppt/diagrams/data1.xml", b"AAAA"),
                ("ppt/diagrams/layout1.xml", b"LAYOUT"),
                ("ppt/diagrams/quickStyle1.xml", b"QSTYLE"),
                ("ppt/diagrams/colors1.xml", b"COLORS"),
            ],
        );
        let b = pkg_with_slide(
            slide,
            rels,
            &[
                ("ppt/diagrams/data1.xml", b"BBBB"),
                ("ppt/diagrams/layout1.xml", b"LAYOUT"),
                ("ppt/diagrams/quickStyle1.xml", b"QSTYLE"),
                ("ppt/diagrams/colors1.xml", b"COLORS"),
            ],
        );
        assert_ne!(
            slide_content_hash(&a, "ppt/slides/slide1.xml").unwrap(),
            slide_content_hash(&b, "ppt/slides/slide1.xml").unwrap(),
            "different SmartArt diagram bytes must change the hash"
        );
    }

    #[test]
    fn changes_when_media_bytes_change() {
        let a = pkg_with_slide(
            r#"<p:sld xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:pic><a:blip r:embed="rId2"/></p:pic></p:sld>"#,
            &[image_rel("rId2", "../media/image1.png")],
            &[("ppt/media/image1.png", b"AAAA")],
        );
        let b = pkg_with_slide(
            r#"<p:sld xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:pic><a:blip r:embed="rId2"/></p:pic></p:sld>"#,
            &[image_rel("rId2", "../media/image1.png")],
            &[("ppt/media/image1.png", b"BBBB")],
        );
        assert_ne!(
            slide_content_hash(&a, "ppt/slides/slide1.xml").unwrap(),
            slide_content_hash(&b, "ppt/slides/slide1.xml").unwrap(),
            "different media bytes must change the hash"
        );
    }

    #[test]
    fn changes_when_text_changes() {
        let a = pkg_with_slide(r#"<p:sld><a:t>Hello</a:t></p:sld>"#, &[], &[]);
        let b = pkg_with_slide(r#"<p:sld><a:t>World</a:t></p:sld>"#, &[], &[]);
        assert_ne!(
            slide_content_hash(&a, "ppt/slides/slide1.xml").unwrap(),
            slide_content_hash(&b, "ppt/slides/slide1.xml").unwrap(),
        );
    }

    #[test]
    fn volatile_guids_stripped() {
        // Two saves of the SAME authored slide: identical structure, but the
        // per-edit creationId element GUIDs and modId attribute differ. Stripping
        // both must make the two hash identically.
        let save_a = pkg_with_slide(
            r#"<p:sld><p:cSld><p:spTree><p:sp modId="{M-AAAA}"><p:nvSpPr><p:cNvPr id="2" name="T"><a:extLst><a:ext><a16:creationId xmlns:a16="x" id="{AAAA}"/></a:ext></a:extLst></p:cNvPr></p:nvSpPr></p:sp></p:spTree></p:cSld><p14:creationId xmlns:p14="y" val="{P-AAAA}"/></p:sld>"#,
            &[],
            &[],
        );
        let save_b = pkg_with_slide(
            r#"<p:sld><p:cSld><p:spTree><p:sp modId="{M-BBBB}"><p:nvSpPr><p:cNvPr id="2" name="T"><a:extLst><a:ext><a16:creationId xmlns:a16="x" id="{BBBB}"/></a:ext></a:extLst></p:cNvPr></p:nvSpPr></p:sp></p:spTree></p:cSld><p14:creationId xmlns:p14="y" val="{P-BBBB}"/></p:sld>"#,
            &[],
            &[],
        );
        assert_eq!(
            slide_content_hash(&save_a, "ppt/slides/slide1.xml").unwrap(),
            slide_content_hash(&save_b, "ppt/slides/slide1.xml").unwrap(),
            "creationId/modId GUID churn must not perturb the hash"
        );
        // Sanity: a real authored-text change still moves the hash.
        let changed = pkg_with_slide(
            r#"<p:sld><p:cSld><p:spTree><p:sp modId="{M-AAAA}"><p:nvSpPr><p:cNvPr id="2" name="CHANGED"><a:extLst><a:ext><a16:creationId xmlns:a16="x" id="{AAAA}"/></a:ext></a:extLst></p:cNvPr></p:nvSpPr></p:sp></p:spTree></p:cSld><p14:creationId xmlns:p14="y" val="{P-AAAA}"/></p:sld>"#,
            &[],
            &[],
        );
        assert_ne!(
            slide_content_hash(&save_a, "ppt/slides/slide1.xml").unwrap(),
            slide_content_hash(&changed, "ppt/slides/slide1.xml").unwrap(),
        );
    }

    #[test]
    fn attribute_quoting_normalized() {
        // Single vs double quotes, and entity vs literal — same canonical form.
        let a = pkg_with_slide(r#"<p:sld><p:sp name="A &amp; B"/></p:sld>"#, &[], &[]);
        let b = pkg_with_slide(r#"<p:sld><p:sp name='A &amp; B'/></p:sld>"#, &[], &[]);
        assert_eq!(
            slide_content_hash(&a, "ppt/slides/slide1.xml").unwrap(),
            slide_content_hash(&b, "ppt/slides/slide1.xml").unwrap(),
        );
    }

    #[test]
    fn embed_text_joins_present_fields_only() {
        assert_eq!(
            slide_embed_text(Some("Title"), "Body", Some("Notes")).as_deref(),
            Some("Title\nBody\nNotes")
        );
        assert_eq!(slide_embed_text(None, "Body", None).as_deref(), Some("Body"));
        assert_eq!(slide_embed_text(Some("  "), "", Some("")), None);
    }

    #[test]
    fn text_hash_matches_reconstruction() {
        // The backfill path rebuilds embed text from stored title/body/notes; it
        // must hash identically to the inline path.
        let t = slide_embed_text(Some("Q3"), "Revenue up", Some("speak slowly")).unwrap();
        assert_eq!(text_hash(&t), text_hash("Q3\nRevenue up\nspeak slowly"));
    }
}
