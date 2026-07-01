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
}

impl SlideSpec {
    pub fn new(title: impl Into<String>) -> Self {
        SlideSpec { title: title.into(), bullets: Vec::new(), notes: None, with_image: false }
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
}

#[derive(Debug, Clone)]
pub struct DeckSpec {
    pub title: String,
    pub author: Option<String>,
    /// Theme accent1 color, hex without `#` (default `4472C4`, Office blue).
    pub accent: String,
    /// Major/minor latin font in the theme.
    pub font: String,
    pub slides: Vec<SlideSpec>,
}

impl DeckSpec {
    pub fn new(title: impl Into<String>) -> Self {
        DeckSpec {
            title: title.into(),
            author: None,
            accent: "4472C4".into(),
            font: "Calibri".into(),
            slides: Vec::new(),
        }
    }
    pub fn author(mut self, author: impl Into<String>) -> Self {
        self.author = Some(author.into());
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
        pkg.set_rels("ppt/presentation.xml", &pres_rels);
        pkg.insert_part(
            "ppt/presentation.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {NS_DECL}><p:sldMasterIdLst><p:sldMasterId id="2147483648" r:id="rId1"/></p:sldMasterIdLst>{notes_master_lst}<p:sldIdLst>{sld_id_lst}</p:sldIdLst><p:sldSz cx="12192000" cy="6858000"/><p:notesSz cx="6858000" cy="9144000"/></p:presentation>"#
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
