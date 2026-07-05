//! Programmatic PPTX builders for tests.
//!
//! [`DeckSpec::build`] produces a minimal but structurally complete PPTX:
//! presentation → slides → layout → master → theme chain, docProps, optional
//! notes slides (with notes master) and an embedded picture. Themes are
//! parameterizable (accent color, fonts) so composer tests can verify that
//! source styling survives a merge.

use std::path::Path;

use crate::opc::{rel_type, Package, Relationship};

/// A valid 1×1 red PNG.
pub const TINY_PNG: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
    0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x02, 0x00, 0x00, 0x00, 0x90,
    0x77, 0x53, 0xDE, 0x00, 0x00, 0x00, 0x0C, 0x49, 0x44, 0x41, 0x54, 0x08, 0xD7, 0x63, 0xF8,
    0xCF, 0xC0, 0x00, 0x00, 0x00, 0x03, 0x00, 0x01, 0x9E, 0xDE, 0x0C, 0xAC, 0x00, 0x00, 0x00,
    0x00, 0x49, 0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
];

const CT_PRESENTATION: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const CT_SLIDE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slide+xml";
const CT_LAYOUT: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideLayout+xml";
const CT_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.slideMaster+xml";
const CT_THEME: &str = "application/vnd.openxmlformats-officedocument.theme+xml";
const CT_NOTES_SLIDE: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesSlide+xml";
const CT_NOTES_MASTER: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.notesMaster+xml";
const CT_CORE: &str = "application/vnd.openxmlformats-package.core-properties+xml";

const NS_DECL: &str = concat!(
    r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#
);

#[derive(Debug, Clone)]
pub struct SlideSpec {
    pub title: String,
    pub bullets: Vec<String>,
    pub notes: Option<String>,
    pub with_image: bool,
    /// Raw `<p:sp>`/`<p:grpSp>`/`<p:graphicFrame>` … XML appended verbatim inside
    /// the slide's `<p:spTree>`. An escape hatch for tests that need exotic
    /// shapes (tables, effects, groups) the structured builder doesn't emit.
    pub raw_shapes: Vec<String>,
}

impl SlideSpec {
    pub fn new(title: impl Into<String>) -> Self {
        SlideSpec {
            title: title.into(),
            bullets: Vec::new(),
            notes: None,
            with_image: false,
            raw_shapes: Vec::new(),
        }
    }
    pub fn bullets(mut self, bullets: &[&str]) -> Self {
        self.bullets = bullets.iter().map(|s| s.to_string()).collect();
        self
    }
    pub fn notes(mut self, notes: impl Into<String>) -> Self {
        self.notes = Some(notes.into());
        self
    }
    pub fn image(mut self) -> Self {
        self.with_image = true;
        self
    }
    /// Append raw shape XML into the slide's `<p:spTree>`.
    pub fn raw_shape(mut self, xml: impl Into<String>) -> Self {
        self.raw_shapes.push(xml.into());
        self
    }
}

/// One embedded font family for [`DeckSpec::embed_font`]: the declared
/// `typeface` plus its `(bold, italic, bytes)` variants.
#[derive(Debug, Clone)]
pub struct EmbeddedFontSpec {
    pub family: String,
    pub variants: Vec<(bool, bool, Vec<u8>)>,
}

#[derive(Debug, Clone)]
pub struct DeckSpec {
    pub title: String,
    pub author: Option<String>,
    /// Theme accent1 color, hex without `#` (default `4472C4`, Office blue).
    pub accent: String,
    /// Major/minor latin font in the theme.
    pub font: String,
    /// Slide canvas size in EMU (default 12192000×6858000, 16:9 widescreen).
    pub slide_cx: i64,
    pub slide_cy: i64,
    pub slides: Vec<SlideSpec>,
    /// Fonts embedded via `<p:embeddedFontLst>` + `/ppt/fonts/*.fntdata`.
    pub embedded_fonts: Vec<EmbeddedFontSpec>,
}

impl DeckSpec {
    pub fn new(title: impl Into<String>) -> Self {
        DeckSpec {
            title: title.into(),
            author: None,
            accent: "4472C4".into(),
            font: "Calibri".into(),
            slide_cx: 12192000,
            slide_cy: 6858000,
            slides: Vec::new(),
            embedded_fonts: Vec::new(),
        }
    }
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
        self
    }
    /// Set the slide canvas size in EMU (drives `<p:sldSz>`).
    pub fn slide_size(mut self, cx: i64, cy: i64) -> Self {
        self.slide_cx = cx;
        self.slide_cy = cy;
        self
    }
    pub fn accent(mut self, hex6: impl Into<String>) -> Self {
        self.accent = hex6.into();
        self
    }
    pub fn font(mut self, font: impl Into<String>) -> Self {
        self.font = font.into();
        self
    }
    pub fn slide(mut self, slide: SlideSpec) -> Self {
        self.slides.push(slide);
        self
    }

    /// Embed a font family under `<p:embeddedFontLst>`. `variants` is a list of
    /// `(bold, italic, bytes)`; each becomes a `regular`/`bold`/`italic`/
    /// `boldItalic` element pointing at a `/ppt/fonts/*.fntdata` part holding
    /// `bytes` verbatim (as PowerPoint stores raw TTF/OTF). See [`sample_ttf`]
    /// for valid test bytes.
    pub fn embed_font(mut self, family: impl Into<String>, variants: Vec<(bool, bool, Vec<u8>)>) -> Self {
        self.embedded_fonts.push(EmbeddedFontSpec { family: family.into(), variants });
        self
    }

    pub fn write_to(&self, path: &Path) -> crate::Result<()> {
        std::fs::write(path, self.build()).map_err(|e| crate::Error::io(path, e))
    }

    /// Build the PPTX bytes.
    pub fn build(&self) -> Vec<u8> {
        let mut pkg = Package::default();
        let n = self.slides.len();
        let any_notes = self.slides.iter().any(|s| s.notes.is_some());
        let any_image = self.slides.iter().any(|s| s.with_image);

        // --- [Content_Types].xml -------------------------------------------------
        let mut ct = crate::opc::ContentTypes::default();
        ct.ensure_default("rels", "application/vnd.openxmlformats-package.relationships+xml");
        ct.ensure_default("xml", "application/xml");
        if any_image {
            ct.ensure_default("png", "image/png");
        }
        if !self.embedded_fonts.is_empty() {
            ct.ensure_default("fntdata", "application/x-fontdata");
        }
        ct.set_override("ppt/presentation.xml", CT_PRESENTATION);
        ct.set_override("ppt/slideMasters/slideMaster1.xml", CT_MASTER);
        ct.set_override("ppt/slideLayouts/slideLayout1.xml", CT_LAYOUT);
        ct.set_override("ppt/theme/theme1.xml", CT_THEME);
        ct.set_override("docProps/core.xml", CT_CORE);
        for i in 1..=n {
            ct.set_override(format!("ppt/slides/slide{i}.xml").as_str(), CT_SLIDE);
        }
        if any_notes {
            ct.set_override("ppt/notesMasters/notesMaster1.xml", CT_NOTES_MASTER);
            for (i, s) in self.slides.iter().enumerate() {
                if s.notes.is_some() {
                    ct.set_override(
                        format!("ppt/notesSlides/notesSlide{}.xml", i + 1).as_str(),
                        CT_NOTES_SLIDE,
                    );
                }
            }
        }
        pkg.set_content_types(&ct);

        // --- package root rels ----------------------------------------------------
        pkg.set_rels(
            "",
            &[
                Relationship {
                    id: "rId1".into(),
                    rel_type: rel_type::OFFICE_DOCUMENT.into(),
                    target: "ppt/presentation.xml".into(),
                    external: false,
                },
                Relationship {
                    id: "rId2".into(),
                    rel_type: rel_type::CORE_PROPS.into(),
                    target: "docProps/core.xml".into(),
                    external: false,
                },
            ],
        );

        // --- docProps/core.xml ----------------------------------------------------
        let author = self.author.clone().unwrap_or_else(|| "Slideflow Fixture".into());
        pkg.insert_part(
            "docProps/core.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{}</dc:title><dc:creator>{}</dc:creator><dcterms:modified xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:modified></cp:coreProperties>"#,
                xml_escape(&self.title),
                xml_escape(&author)
            )
            .into_bytes(),
        );

        // --- presentation.xml + rels ------------------------------------------------
        let mut pres_rels = vec![Relationship {
            id: "rId1".into(),
            rel_type: rel_type::SLIDE_MASTER.into(),
            target: "slideMasters/slideMaster1.xml".into(),
            external: false,
        }];
        let mut sld_id_lst = String::new();
        for i in 1..=n {
            let rid = format!("rId{}", i + 1);
            pres_rels.push(Relationship {
                id: rid.clone(),
                rel_type: rel_type::SLIDE.into(),
                target: format!("slides/slide{i}.xml"),
                external: false,
            });
            sld_id_lst.push_str(&format!(r#"<p:sldId id="{}" r:id="{}"/>"#, 255 + i, rid));
        }
        let notes_master_lst = if any_notes {
            let rid = format!("rId{}", n + 2);
            pres_rels.push(Relationship {
                id: rid.clone(),
                rel_type: rel_type::NOTES_MASTER.into(),
                target: "notesMasters/notesMaster1.xml".into(),
                external: false,
            });
            format!(r#"<p:notesMasterIdLst><p:notesMasterId r:id="{rid}"/></p:notesMasterIdLst>"#)
        } else {
            String::new()
        };
        // Embedded fonts: one <p:embeddedFont> per family, a font relationship +
        // `/ppt/fonts/fontN.fntdata` part per variant.
        let mut embedded_font_lst = String::new();
        if !self.embedded_fonts.is_empty() {
            let mut k = 0usize;
            embedded_font_lst.push_str("<p:embeddedFontLst>");
            for ef in &self.embedded_fonts {
                embedded_font_lst
                    .push_str(&format!(r#"<p:embeddedFont><p:font typeface="{}"/>"#, xml_escape(&ef.family)));
                for (bold, italic, bytes) in &ef.variants {
                    k += 1;
                    let rid = format!("rIdF{k}");
                    let elem = match (*bold, *italic) {
                        (false, false) => "regular",
                        (true, false) => "bold",
                        (false, true) => "italic",
                        (true, true) => "boldItalic",
                    };
                    embedded_font_lst.push_str(&format!(r#"<p:{elem} r:id="{rid}"/>"#));
                    pres_rels.push(Relationship {
                        id: rid,
                        rel_type: rel_type::FONT.into(),
                        target: format!("fonts/font{k}.fntdata"),
                        external: false,
                    });
                    pkg.insert_part(format!("ppt/fonts/font{k}.fntdata"), bytes.clone());
                }
                embedded_font_lst.push_str("</p:embeddedFont>");
            }
            embedded_font_lst.push_str("</p:embeddedFontLst>");
        }
        pkg.set_rels("ppt/presentation.xml", &pres_rels);
        let slide_cx = self.slide_cx;
        let slide_cy = self.slide_cy;
        pkg.insert_part(
            "ppt/presentation.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {NS_DECL}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>{notes_master_lst}<p:sldIdLst>{sld_id_lst}</p:sldIdLst><p:sldSz cx="{slide_cx}" cy="{slide_cy}"/><p:notesSz cx="6858000" cy="9144000"/>{embedded_font_lst}</p:presentation>"#
            )
            .into_bytes(),
        );

        // --- theme / master / layout -------------------------------------------------
        pkg.insert_part("ppt/theme/theme1.xml", theme_xml(&self.accent, &self.font).into_bytes());
        pkg.insert_part("ppt/slideMasters/slideMaster1.xml", master_xml().into_bytes());
        pkg.set_rels(
            "ppt/slideMasters/slideMaster1.xml",
            &[
                Relationship {
                    id: "rId1".into(),
                    rel_type: rel_type::SLIDE_LAYOUT.into(),
                    target: "../slideLayouts/slideLayout1.xml".into(),
                    external: false,
                },
                Relationship {
                    id: "rId2".into(),
                    rel_type: rel_type::THEME.into(),
                    target: "../theme/theme1.xml".into(),
                    external: false,
                },
            ],
        );
        pkg.insert_part("ppt/slideLayouts/slideLayout1.xml", layout_xml().into_bytes());
        pkg.set_rels(
            "ppt/slideLayouts/slideLayout1.xml",
            &[Relationship {
                id: "rId1".into(),
                rel_type: rel_type::SLIDE_MASTER.into(),
                target: "../slideMasters/slideMaster1.xml".into(),
                external: false,
            }],
        );

        // --- media ---------------------------------------------------------------------
        if any_image {
            pkg.insert_part("ppt/media/image1.png", TINY_PNG.to_vec());
        }

        // --- notes master ----------------------------------------------------------------
        if any_notes {
            pkg.insert_part("ppt/notesMasters/notesMaster1.xml", notes_master_xml().into_bytes());
            pkg.set_rels(
                "ppt/notesMasters/notesMaster1.xml",
                &[Relationship {
                    id: "rId1".into(),
                    rel_type: rel_type::THEME.into(),
                    target: "../theme/theme1.xml".into(),
                    external: false,
                }],
            );
        }

        // --- slides ------------------------------------------------------------------------
        for (idx0, slide) in self.slides.iter().enumerate() {
            let i = idx0 + 1;
            pkg.insert_part(format!("ppt/slides/slide{i}.xml"), slide_xml(slide).into_bytes());
            let mut rels = vec![Relationship {
                id: "rId1".into(),
                rel_type: rel_type::SLIDE_LAYOUT.into(),
                target: "../slideLayouts/slideLayout1.xml".into(),
                external: false,
            }];
            if slide.with_image {
                rels.push(Relationship {
                    id: "rId2".into(),
                    rel_type: rel_type::IMAGE.into(),
                    target: "../media/image1.png".into(),
                    external: false,
                });
            }
            if slide.notes.is_some() {
                rels.push(Relationship {
                    id: "rId3".into(),
                    rel_type: rel_type::NOTES_SLIDE.into(),
                    target: format!("../notesSlides/notesSlide{i}.xml"),
                    external: false,
                });
                pkg.insert_part(
                    format!("ppt/notesSlides/notesSlide{i}.xml"),
                    notes_slide_xml(slide.notes.as_deref().unwrap_or_default()).into_bytes(),
                );
                pkg.set_rels(
                    &format!("ppt/notesSlides/notesSlide{i}.xml"),
                    &[
                        Relationship {
                            id: "rId1".into(),
                            rel_type: rel_type::NOTES_MASTER.into(),
                            target: "../notesMasters/notesMaster1.xml".into(),
                            external: false,
                        },
                        Relationship {
                            id: "rId2".into(),
                            rel_type: rel_type::SLIDE.into(),
                            target: format!("../slides/slide{i}.xml"),
                            external: false,
                        },
                    ],
                );
            }
            pkg.set_rels(&format!("ppt/slides/slide{i}.xml"), &rels);
        }

        pkg.to_bytes().expect("fixture package serializes")
    }
}

pub fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// A minimal, self-contained TrueType font for tests.
///
/// Generated here rather than vendored, so it is public-domain test data with
/// no third-party font license. It is a structurally coherent sfnt — correct
/// table directory and consistent `head`/`hhea`/`maxp`/`hmtx`/`loca`/`glyf`/
/// `cmap`/`name`/`post` tables with one empty `.notdef` glyph — that begins
/// with the TrueType magic `00 01 00 00`. Its purpose is to exercise
/// embedded-font *extraction* and `@font-face` emission; it is not intended to
/// render real glyphs. (Table checksums and `head.checkSumAdjustment` are left
/// zero; sfnt parsers do not verify them.)
pub fn sample_ttf() -> Vec<u8> {
    build_minimal_ttf("Slideflow Test")
}

/// A minimal test TrueType font carrying `family` in its `name` table — so it
/// indexes under that family in a `fontdb::Database` and matches a deck's
/// `<a:latin typeface="family">`. Used to exercise the app-local font pipeline
/// (harvested / user-added / downloaded faces resolve by their real family name).
pub fn sample_ttf_named(family: &str) -> Vec<u8> {
    build_minimal_ttf(family)
}

fn build_minimal_ttf(family: &str) -> Vec<u8> {
    fn push16(b: &mut Vec<u8>, v: u16) {
        b.extend_from_slice(&v.to_be_bytes());
    }
    fn push32(b: &mut Vec<u8>, v: u32) {
        b.extend_from_slice(&v.to_be_bytes());
    }
    fn pushi16(b: &mut Vec<u8>, v: i16) {
        b.extend_from_slice(&v.to_be_bytes());
    }

    // head (54 bytes)
    let mut head = Vec::new();
    push32(&mut head, 0x0001_0000); // version 1.0
    push32(&mut head, 0x0001_0000); // fontRevision
    push32(&mut head, 0); // checkSumAdjustment (unverified)
    push32(&mut head, 0x5F0F_3CF5); // magicNumber
    push16(&mut head, 0); // flags
    push16(&mut head, 1000); // unitsPerEm
    head.extend_from_slice(&[0u8; 8]); // created
    head.extend_from_slice(&[0u8; 8]); // modified
    pushi16(&mut head, 0); // xMin
    pushi16(&mut head, 0); // yMin
    pushi16(&mut head, 0); // xMax
    pushi16(&mut head, 0); // yMax
    push16(&mut head, 0); // macStyle
    push16(&mut head, 8); // lowestRecPPEM
    pushi16(&mut head, 2); // fontDirectionHint
    pushi16(&mut head, 0); // indexToLocFormat (short offsets)
    pushi16(&mut head, 0); // glyphDataFormat

    // hhea (36 bytes)
    let mut hhea = Vec::new();
    push32(&mut hhea, 0x0001_0000);
    pushi16(&mut hhea, 800); // ascender
    pushi16(&mut hhea, -200); // descender
    pushi16(&mut hhea, 0); // lineGap
    push16(&mut hhea, 1000); // advanceWidthMax
    pushi16(&mut hhea, 0); // minLeftSideBearing
    pushi16(&mut hhea, 0); // minRightSideBearing
    pushi16(&mut hhea, 0); // xMaxExtent
    pushi16(&mut hhea, 1); // caretSlopeRise
    pushi16(&mut hhea, 0); // caretSlopeRun
    pushi16(&mut hhea, 0); // caretOffset
    for _ in 0..4 {
        pushi16(&mut hhea, 0); // reserved
    }
    pushi16(&mut hhea, 0); // metricDataFormat
    push16(&mut hhea, 1); // numberOfHMetrics

    // maxp v1.0 (32 bytes)
    let mut maxp = Vec::new();
    push32(&mut maxp, 0x0001_0000);
    push16(&mut maxp, 1); // numGlyphs
    push16(&mut maxp, 0); // maxPoints
    push16(&mut maxp, 0); // maxContours
    push16(&mut maxp, 0); // maxCompositePoints
    push16(&mut maxp, 0); // maxCompositeContours
    push16(&mut maxp, 1); // maxZones
    push16(&mut maxp, 0); // maxTwilightPoints
    push16(&mut maxp, 0); // maxStorage
    push16(&mut maxp, 0); // maxFunctionDefs
    push16(&mut maxp, 0); // maxInstructionDefs
    push16(&mut maxp, 0); // maxStackElements
    push16(&mut maxp, 0); // maxSizeOfInstructions
    push16(&mut maxp, 0); // maxComponentElements
    push16(&mut maxp, 0); // maxComponentDepth

    // hmtx (numberOfHMetrics = 1): one longHorMetric
    let mut hmtx = Vec::new();
    push16(&mut hmtx, 500); // advanceWidth
    pushi16(&mut hmtx, 0); // leftSideBearing

    // loca (short): numGlyphs+1 entries; glyph 0 is empty (0..0)
    let mut loca = Vec::new();
    push16(&mut loca, 0);
    push16(&mut loca, 0);

    // glyf: empty (glyph 0 has no outline)
    let glyf: Vec<u8> = Vec::new();

    // cmap: one format-0 subtable mapping every code to .notdef
    let mut cmap = Vec::new();
    push16(&mut cmap, 0); // version
    push16(&mut cmap, 1); // numTables
    push16(&mut cmap, 1); // platformID (Macintosh)
    push16(&mut cmap, 0); // encodingID (Roman)
    push32(&mut cmap, 12); // offset to subtable
    push16(&mut cmap, 0); // format 0
    push16(&mut cmap, 262); // length
    push16(&mut cmap, 0); // language
    cmap.extend_from_slice(&[0u8; 256]); // glyphIdArray

    // name: family (id 1) + subfamily (id 2), Windows Unicode BMP, UTF-16BE
    let fam: Vec<u8> = family.encode_utf16().flat_map(u16::to_be_bytes).collect();
    let sub: Vec<u8> = "Regular".encode_utf16().flat_map(u16::to_be_bytes).collect();
    let mut name = Vec::new();
    push16(&mut name, 0); // format
    push16(&mut name, 2); // count
    push16(&mut name, 6 + 2 * 12); // stringOffset (after header + 2 records)
    let mut name_record = |id: u16, len: u16, off: u16| {
        push16(&mut name, 3); // platformID (Windows)
        push16(&mut name, 1); // encodingID (Unicode BMP)
        push16(&mut name, 0x0409); // languageID (en-US)
        push16(&mut name, id); // nameID
        push16(&mut name, len);
        push16(&mut name, off);
    };
    name_record(1, fam.len() as u16, 0);
    name_record(2, sub.len() as u16, fam.len() as u16);
    name.extend_from_slice(&fam);
    name.extend_from_slice(&sub);

    // post v3.0 (no glyph names) — 32 bytes
    let mut post = Vec::new();
    push32(&mut post, 0x0003_0000);
    push32(&mut post, 0); // italicAngle
    pushi16(&mut post, 0); // underlinePosition
    pushi16(&mut post, 0); // underlineThickness
    push32(&mut post, 0); // isFixedPitch
    push32(&mut post, 0); // minMemType42
    push32(&mut post, 0); // maxMemType42
    push32(&mut post, 0); // minMemType1
    push32(&mut post, 0); // maxMemType1

    // Assemble: tables in ascending-tag order (required by the directory).
    let tables: [(&[u8; 4], Vec<u8>); 9] = [
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
        (b"name", name),
        (b"post", post),
    ];
    let num_tables = tables.len();

    let mut font = Vec::new();
    push32(&mut font, 0x0001_0000); // sfnt version (TrueType)
    push16(&mut font, num_tables as u16);
    // searchRange/entrySelector/rangeShift for 9 tables (2^3 = 8 ≤ 9).
    push16(&mut font, 128); // searchRange = 2^floor(log2(n)) * 16
    push16(&mut font, 3); // entrySelector = floor(log2(n))
    push16(&mut font, num_tables as u16 * 16 - 128); // rangeShift

    let mut offset = 12 + 16 * num_tables;
    let mut blob = Vec::new();
    for (tag, data) in &tables {
        font.extend_from_slice(*tag);
        push32(&mut font, 0); // checksum (unverified)
        push32(&mut font, offset as u32);
        push32(&mut font, data.len() as u32);
        blob.extend_from_slice(data);
        let pad = (4 - (data.len() % 4)) % 4;
        blob.resize(blob.len() + pad, 0);
        offset += data.len() + pad;
    }
    font.extend_from_slice(&blob);
    font
}

fn slide_xml(slide: &SlideSpec) -> String {
    let mut shapes = String::new();
    // Title placeholder.
    shapes.push_str(&format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{}</a:t></a:r></a:p></p:txBody></p:sp>"#,
        xml_escape(&slide.title)
    ));
    // Body placeholder with bullets.
    if !slide.bullets.is_empty() {
        let paras: String = slide
            .bullets
            .iter()
            .map(|b| {
                format!(
                    r#"<a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{}</a:t></a:r></a:p>"#,
                    xml_escape(b)
                )
            })
            .collect();
        shapes.push_str(&format!(
            r#"<p:sp><p:nvSpPr><p:cNvPr id="3" name="Content 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody></p:sp>"#
        ));
    }
    if slide.with_image {
        shapes.push_str(
            r#"<p:pic><p:nvPicPr><p:cNvPr id="4" name="Picture 3"/><p:cNvPicPr><a:picLocks noChangeAspect="1"/></p:cNvPicPr><p:nvPr/></p:nvPicPr><p:blipFill><a:blip r:embed="rId2"/><a:stretch><a:fillRect/></a:stretch></p:blipFill><p:spPr><a:xfrm><a:off x="9144000" y="4572000"/><a:ext cx="1828800" cy="1371600"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr></p:pic>"#,
        );
    }
    for raw in &slide.raw_shapes {
        shapes.push_str(raw);
    }
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sld {NS_DECL}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="0" y="0"/><a:ext cx="0" cy="0"/><a:chOff x="0" y="0"/><a:chExt cx="0" cy="0"/></a:xfrm></p:grpSpPr>{shapes}</p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sld>"#
    )
}

fn notes_slide_xml(notes: &str) -> String {
    let paras: String = notes
        .lines()
        .map(|l| {
            format!(
                r#"<a:p><a:r><a:rPr lang="en-US" dirty="0"/><a:t>{}</a:t></a:r></a:p>"#,
                xml_escape(l)
            )
        })
        .collect();
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notes {NS_DECL}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Notes Placeholder 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/>{paras}</p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:notes>"#
    )
}

fn master_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldMaster {NS_DECL}><p:cSld><p:bg><p:bgPr><a:solidFill><a:schemeClr val="bg1"/></a:solidFill><a:effectLst/></p:bgPr></p:bg><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/><p:sldLayoutIdLst><p:sldLayoutId id="2147483649" r:id="rId1"/></p:sldLayoutIdLst></p:sldMaster>"#
    )
}

fn layout_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:sldLayout {NS_DECL} type="titleAndBody"><p:cSld name="Title and Content"><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/><p:sp><p:nvSpPr><p:cNvPr id="2" name="Title 1"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="365125"/><a:ext cx="10515600" cy="1325563"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp><p:sp><p:nvSpPr><p:cNvPr id="3" name="Content 2"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr><a:xfrm><a:off x="838200" y="1825625"/><a:ext cx="10515600" cy="4351338"/></a:xfrm><a:prstGeom prst="rect"><a:avLst/></a:prstGeom></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:endParaRPr lang="en-US"/></a:p></p:txBody></p:sp></p:spTree></p:cSld><p:clrMapOvr><a:masterClrMapping/></p:clrMapOvr></p:sldLayout>"#
    )
}

fn notes_master_xml() -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:notesMaster {NS_DECL}><p:cSld><p:spTree><p:nvGrpSpPr><p:cNvPr id="1" name=""/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr/></p:spTree></p:cSld><p:clrMap bg1="lt1" tx1="dk1" bg2="lt2" tx2="dk2" accent1="accent1" accent2="accent2" accent3="accent3" accent4="accent4" accent5="accent5" accent6="accent6" hlink="hlink" folHlink="folHlink"/></p:notesMaster>"#
    )
}

fn theme_xml(accent1: &str, font: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:theme xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" name="Slideflow Fixture Theme"><a:themeElements><a:clrScheme name="Fixture"><a:dk1><a:sysClr val="windowText" lastClr="000000"/></a:dk1><a:lt1><a:sysClr val="window" lastClr="FFFFFF"/></a:lt1><a:dk2><a:srgbClr val="44546A"/></a:dk2><a:lt2><a:srgbClr val="E7E6E6"/></a:lt2><a:accent1><a:srgbClr val="{accent1}"/></a:accent1><a:accent2><a:srgbClr val="ED7D31"/></a:accent2><a:accent3><a:srgbClr val="A5A5A5"/></a:accent3><a:accent4><a:srgbClr val="FFC000"/></a:accent4><a:accent5><a:srgbClr val="5B9BD5"/></a:accent5><a:accent6><a:srgbClr val="70AD47"/></a:accent6><a:hlink><a:srgbClr val="0563C1"/></a:hlink><a:folHlink><a:srgbClr val="954F72"/></a:folHlink></a:clrScheme><a:fontScheme name="Fixture"><a:majorFont><a:latin typeface="{font}"/><a:ea typeface=""/><a:cs typeface=""/></a:majorFont><a:minorFont><a:latin typeface="{font}"/><a:ea typeface=""/><a:cs typeface=""/></a:minorFont></a:fontScheme><a:fmtScheme name="Fixture"><a:fillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"><a:alpha val="80000"/></a:schemeClr></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:fillStyleLst><a:lnStyleLst><a:ln w="6350" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="12700" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln><a:ln w="19050" cap="flat" cmpd="sng" algn="ctr"><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:prstDash val="solid"/></a:ln></a:lnStyleLst><a:effectStyleLst><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle><a:effectStyle><a:effectLst/></a:effectStyle></a:effectStyleLst><a:bgFillStyleLst><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill><a:solidFill><a:schemeClr val="phClr"/></a:solidFill></a:bgFillStyleLst></a:fmtScheme></a:themeElements></a:theme>"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_builds_valid_package() {
        let bytes = DeckSpec::new("Fixture Deck")
            .slide(SlideSpec::new("Alpha").bullets(&["one", "two"]).notes("note text").image())
            .slide(SlideSpec::new("Beta"))
            .build();
        let pkg = Package::from_bytes(&bytes).unwrap();
        assert_eq!(pkg.main_document_part().unwrap(), "ppt/presentation.xml");
        assert!(pkg.has_part("ppt/slides/slide1.xml"));
        assert!(pkg.has_part("ppt/slides/slide2.xml"));
        assert!(pkg.has_part("ppt/media/image1.png"));
        assert!(pkg.has_part("ppt/notesSlides/notesSlide1.xml"));
        assert!(pkg.has_part("ppt/notesMasters/notesMaster1.xml"));
        let ct = pkg.content_types().unwrap();
        assert!(ct.content_type_of("ppt/slides/slide2.xml").unwrap().contains("slide+xml"));
        assert_eq!(ct.content_type_of("ppt/media/image1.png").unwrap(), "image/png");
    }

    #[test]
    fn escaping_special_chars() {
        let bytes = DeckSpec::new("A&B <Deck>")
            .slide(SlideSpec::new("Title & \"quotes\" <tags>"))
            .build();
        let pf = crate::pptx::PresentationFile::from_bytes(&bytes).unwrap();
        let content = pf.slide_content(1).unwrap();
        assert_eq!(content.title.as_deref(), Some("Title & \"quotes\" <tags>"));
    }
}
