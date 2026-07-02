//! Read-side of PresentationML: slide order, per-slide text/title/notes,
//! document metadata, slide dimensions.

use std::path::Path;

use quick_xml::events::Event;
use quick_xml::Reader;

use crate::error::{Error, Result};
use crate::opc::{local_name, rel_type, resolve_target, Package};

/// Default slide size (16:9) in EMU, used when `p:sldSz` is absent.
pub const DEFAULT_SLIDE_W_EMU: i64 = 12_192_000;
pub const DEFAULT_SLIDE_H_EMU: i64 = 6_858_000;

#[derive(Debug, Clone, Default)]
pub struct CoreProps {
    pub title: Option<String>,
    pub creator: Option<String>,
    pub modified_iso: Option<String>,
}

/// An opened PPTX with its slide order resolved.
#[derive(Debug)]
pub struct PresentationFile {
    pub package: Package,
    /// Slide part names in presentation order (e.g. `ppt/slides/slide1.xml`).
    pub slide_parts: Vec<String>,
    /// Slide canvas size in EMU.
    pub slide_width_emu: i64,
    pub slide_height_emu: i64,
    pub core: CoreProps,
}

/// Extracted text of one slide.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SlideContent {
    /// Text of the title/ctrTitle placeholder, if any.
    pub title: Option<String>,
    /// One entry per shape that contains text (title included), in document order.
    pub texts: Vec<String>,
    /// Speaker notes text, if a notes slide exists.
    pub notes: Option<String>,
}

impl PresentationFile {
    pub fn open(path: &Path) -> Result<Self> {
        Self::from_package(Package::open(path)?)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        Self::from_package(Package::from_bytes(bytes)?)
    }

    pub fn from_package(package: Package) -> Result<Self> {
        let main = package.main_document_part()?;
        if !main.ends_with("presentation.xml") {
            return Err(Error::InvalidPackage(format!(
                "main document is {main}, not a presentation"
            )));
        }
        let pres_xml = package.require_part(&main)?;
        let (slide_rids, slide_size) = parse_presentation_xml(pres_xml, &main)?;

        let rels = package.rels_for(&main)?;
        let mut slide_parts = Vec::with_capacity(slide_rids.len());
        for rid in &slide_rids {
            let rel = rels
                .iter()
                .find(|r| &r.id == rid && r.rel_type == rel_type::SLIDE)
                .ok_or_else(|| {
                    Error::InvalidPackage(format!("sldIdLst references unknown r:id {rid}"))
                })?;
            slide_parts.push(resolve_target(&main, &rel.target));
        }

        let core = parse_core_props(&package);
        let (w, h) = slide_size.unwrap_or((DEFAULT_SLIDE_W_EMU, DEFAULT_SLIDE_H_EMU));
        Ok(PresentationFile {
            package,
            slide_parts,
            slide_width_emu: w,
            slide_height_emu: h,
            core,
        })
    }

    pub fn slide_count(&self) -> usize {
        self.slide_parts.len()
    }

    /// Part name of a slide by 1-based index.
    pub fn slide_part(&self, index_1based: usize) -> Result<&str> {
        if index_1based == 0 || index_1based > self.slide_parts.len() {
            return Err(Error::SlideOutOfRange {
                index: index_1based,
                count: self.slide_parts.len(),
            });
        }
        Ok(&self.slide_parts[index_1based - 1])
    }

    /// Extract title/texts/notes of a slide by 1-based index.
    pub fn slide_content(&self, index_1based: usize) -> Result<SlideContent> {
        let part = self.slide_part(index_1based)?.to_string();
        let xml = self.package.require_part(&part)?;
        let mut content = extract_texts(xml, &part)?;

        // Speaker notes live in a separate part linked from the slide's rels.
        let rels = self.package.rels_for(&part)?;
        if let Some(rel) = rels
            .iter()
            .find(|r| r.rel_type == rel_type::NOTES_SLIDE && !r.external)
        {
            let notes_part = resolve_target(&part, &rel.target);
            if let Some(notes_xml) = self.package.part(&notes_part) {
                let notes = extract_texts(notes_xml, &notes_part)?;
                let joined = notes.texts.join("\n");
                // Notes placeholders often contain just the slide number; ignore
                // pure-numeric noise.
                let meaningful = joined
                    .lines()
                    .filter(|l| !l.trim().is_empty() && l.trim().parse::<u64>().is_err())
                    .collect::<Vec<_>>()
                    .join("\n");
                if !meaningful.is_empty() {
                    content.notes = Some(meaningful);
                }
            }
        }
        Ok(content)
    }

    /// The slide layout part a slide references, if any.
    pub fn layout_of_slide(&self, slide_part: &str) -> Result<Option<String>> {
        let rels = self.package.rels_for(slide_part)?;
        Ok(rels
            .iter()
            .find(|r| r.rel_type == rel_type::SLIDE_LAYOUT && !r.external)
            .map(|r| resolve_target(slide_part, &r.target)))
    }

    /// The master part a layout references, if any.
    pub fn master_of_layout(&self, layout_part: &str) -> Result<Option<String>> {
        let rels = self.package.rels_for(layout_part)?;
        Ok(rels
            .iter()
            .find(|r| r.rel_type == rel_type::SLIDE_MASTER && !r.external)
            .map(|r| resolve_target(layout_part, &r.target)))
    }

    /// The theme part a master references, if any.
    pub fn theme_of_master(&self, master_part: &str) -> Result<Option<String>> {
        let rels = self.package.rels_for(master_part)?;
        Ok(rels
            .iter()
            .find(|r| r.rel_type == rel_type::THEME && !r.external)
            .map(|r| resolve_target(master_part, &r.target)))
    }
}

/// Ordered slide `r:id`s plus the slide size in EMU, if declared.
type PresentationInfo = (Vec<String>, Option<(i64, i64)>);

/// Returns the ordered slide r:ids and the slide size in EMU if declared.
fn parse_presentation_xml(xml: &[u8], part: &str) -> Result<PresentationInfo> {
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    let mut in_sld_id_lst = false;
    let mut rids = Vec::new();
    let mut size = None;
    loop {
        match reader.read_event_into(&mut buf).map_err(|e| Error::xml(part, e))? {
            Event::Start(ref e) if local_name(e.name().as_ref()) == b"sldIdLst" => {
                in_sld_id_lst = true;
            }
            Event::End(ref e) if local_name(e.name().as_ref()) == b"sldIdLst" => {
                in_sld_id_lst = false;
            }
            Event::Start(ref e) | Event::Empty(ref e) => {
                let qname = e.name();
                let name = local_name(qname.as_ref());
                if in_sld_id_lst && name == b"sldId" {
                    for attr in e.attributes().flatten() {
                        if local_name(attr.key.as_ref()) == b"id"
                            && attr.key.as_ref().starts_with(b"r:")
                        {
                            rids.push(
                                attr.unescape_value()
                                    .map_err(|e| Error::xml(part, e))?
                                    .into_owned(),
                            );
                        }
                    }
                } else if name == b"sldSz" {
                    let mut cx = None;
                    let mut cy = None;
                    for attr in e.attributes().flatten() {
                        let val = attr.unescape_value().map_err(|e| Error::xml(part, e))?;
                        match attr.key.as_ref() {
                            b"cx" => cx = val.parse::<i64>().ok(),
                            b"cy" => cy = val.parse::<i64>().ok(),
                            _ => {}
                        }
                    }
                    if let (Some(cx), Some(cy)) = (cx, cy) {
                        size = Some((cx, cy));
                    }
                }
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok((rids, size))
}

/// Walk a slide (or notes slide) XML and collect per-shape text.
///
/// State machine: text runs (`a:t`) accumulate into the current shape's
/// buffer; paragraph boundaries (`a:p`) and explicit breaks (`a:br`) become
/// newlines; the shape's placeholder type (`p:ph type="..."`) decides whether
/// it is the title. `p:graphicFrame` counts as a text container too — that is
/// where tables live (`a:tbl` cell text would otherwise never be indexed).
fn extract_texts(xml: &[u8], part: &str) -> Result<SlideContent> {
    let mut reader = Reader::from_reader(xml);
    // NB: no trim_text — spacing inside <a:t> is significant.
    let mut buf = Vec::new();
    let mut content = SlideContent::default();

    let mut shape_depth = 0usize;
    let mut current_text = String::new();
    let mut current_is_title = false;
    let mut in_a_t = false;
    let mut pending_newline = false;

    loop {
        match reader.read_event_into(&mut buf).map_err(|e| Error::xml(part, e))? {
            Event::Start(ref e) => match local_name(e.name().as_ref()) {
                b"sp" | b"graphicFrame" => {
                    shape_depth += 1;
                    if shape_depth == 1 {
                        current_text.clear();
                        current_is_title = false;
                        pending_newline = false;
                    }
                }
                b"t" if shape_depth > 0 => in_a_t = true,
                b"p" if shape_depth > 0 && !current_text.is_empty() => {
                    pending_newline = true;
                }
                _ => {}
            },
            Event::Empty(ref e) => match local_name(e.name().as_ref()) {
                b"ph" if shape_depth > 0 => {
                    for attr in e.attributes().flatten() {
                        if attr.key.as_ref() == b"type" {
                            let v = attr.unescape_value().map_err(|e| Error::xml(part, e))?;
                            if v.as_ref() == "title" || v.as_ref() == "ctrTitle" {
                                current_is_title = true;
                            }
                        }
                    }
                }
                b"br" if (in_a_t || shape_depth > 0) && !current_text.is_empty() => {
                    pending_newline = true;
                }
                _ => {}
            },
            Event::Text(ref t) if in_a_t => {
                let text = t.unescape().map_err(|e| Error::xml(part, e))?;
                if !text.is_empty() {
                    if pending_newline {
                        current_text.push('\n');
                        pending_newline = false;
                    }
                    current_text.push_str(&text);
                }
            }
            Event::End(ref e) => match local_name(e.name().as_ref()) {
                b"t" => in_a_t = false,
                b"sp" | b"graphicFrame" if shape_depth > 0 => {
                    shape_depth -= 1;
                    if shape_depth == 0 && !current_text.trim().is_empty() {
                        let text = current_text.trim().to_string();
                        if current_is_title && content.title.is_none() {
                            content.title = Some(text.clone());
                        }
                        content.texts.push(text);
                    }
                }
                _ => {}
            },
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(content)
}

fn parse_core_props(package: &Package) -> CoreProps {
    let mut core = CoreProps::default();
    let Some(xml) = package.part("docProps/core.xml") else {
        return core;
    };
    let mut reader = Reader::from_reader(xml);
    let mut buf = Vec::new();
    let mut current: Option<&'static str> = None;
    while let Ok(event) = reader.read_event_into(&mut buf) {
        match event {
            Event::Start(ref e) => {
                current = match local_name(e.name().as_ref()) {
                    b"title" => Some("title"),
                    b"creator" => Some("creator"),
                    b"modified" => Some("modified"),
                    _ => None,
                };
            }
            Event::Text(ref t) => {
                if let (Some(field), Ok(text)) = (current, t.unescape()) {
                    let text = text.into_owned();
                    if !text.trim().is_empty() {
                        match field {
                            "title" => core.title = Some(text),
                            "creator" => core.creator = Some(text),
                            "modified" => core.modified_iso = Some(text),
                            _ => {}
                        }
                    }
                }
            }
            Event::End(_) => current = None,
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    core
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{DeckSpec, SlideSpec};

    #[test]
    fn opens_fixture_and_reads_slides_in_order() {
        let bytes = DeckSpec::new("Quarterly Review")
            .author("Jane Doe")
            .slide(SlideSpec::new("Q3 Results").bullets(&["Revenue up 12%", "Churn down"]))
            .slide(SlideSpec::new("Roadmap").bullets(&["Ship search", "Ship compose"]).notes("Speak slowly"))
            .build();
        let pf = PresentationFile::from_bytes(&bytes).unwrap();
        assert_eq!(pf.slide_count(), 2);
        assert_eq!(pf.core.title.as_deref(), Some("Quarterly Review"));
        assert_eq!(pf.core.creator.as_deref(), Some("Jane Doe"));
        assert_eq!(pf.slide_width_emu, DEFAULT_SLIDE_W_EMU);

        let s1 = pf.slide_content(1).unwrap();
        assert_eq!(s1.title.as_deref(), Some("Q3 Results"));
        assert!(s1.texts.iter().any(|t| t.contains("Revenue up 12%")));
        assert!(s1.notes.is_none());

        let s2 = pf.slide_content(2).unwrap();
        assert_eq!(s2.title.as_deref(), Some("Roadmap"));
        assert_eq!(s2.notes.as_deref(), Some("Speak slowly"));
    }

    #[test]
    fn table_text_in_graphic_frame_is_extracted() {
        // Tables live in <p:graphicFrame>, not <p:sp> — their cell text must
        // still be collected so it is searchable.
        let xml = r#"<?xml version="1.0"?>
<p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main">
  <p:cSld><p:spTree>
    <p:sp><p:nvSpPr><p:nvPr><p:ph type="title"/></p:nvPr></p:nvSpPr>
      <p:txBody><a:p><a:r><a:t>Quarterly Table</a:t></a:r></a:p></p:txBody></p:sp>
    <p:graphicFrame><a:graphic><a:graphicData><a:tbl>
      <a:tr><a:tc><a:txBody><a:p><a:r><a:t>Quartal</a:t></a:r></a:p></a:txBody></a:tc>
             <a:tc><a:txBody><a:p><a:r><a:t>Umsatz</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
      <a:tr><a:tc><a:txBody><a:p><a:r><a:t>Q1</a:t></a:r></a:p></a:txBody></a:tc>
             <a:tc><a:txBody><a:p><a:r><a:t>120 T€</a:t></a:r></a:p></a:txBody></a:tc></a:tr>
    </a:tbl></a:graphicData></a:graphic></p:graphicFrame>
  </p:spTree></p:cSld>
</p:sld>"#;
        let content = extract_texts(xml.as_bytes(), "test").unwrap();
        assert_eq!(content.title.as_deref(), Some("Quarterly Table"));
        let table_text = content
            .texts
            .iter()
            .find(|t| t.contains("Quartal"))
            .expect("table cell text extracted");
        for term in ["Quartal", "Umsatz", "Q1", "120 T€"] {
            assert!(table_text.contains(term), "missing {term} in {table_text:?}");
        }
    }

    #[test]
    fn out_of_range_slide_errors() {
        let bytes = DeckSpec::new("One").slide(SlideSpec::new("Only")).build();
        let pf = PresentationFile::from_bytes(&bytes).unwrap();
        assert!(pf.slide_content(0).is_err());
        assert!(pf.slide_content(2).is_err());
    }

    #[test]
    fn resolves_layout_master_theme_chain() {
        let bytes = DeckSpec::new("Chain").slide(SlideSpec::new("S")).build();
        let pf = PresentationFile::from_bytes(&bytes).unwrap();
        let slide = pf.slide_part(1).unwrap().to_string();
        let layout = pf.layout_of_slide(&slide).unwrap().expect("layout");
        let master = pf.master_of_layout(&layout).unwrap().expect("master");
        let theme = pf.theme_of_master(&master).unwrap().expect("theme");
        assert!(layout.contains("slideLayouts/"));
        assert!(master.contains("slideMasters/"));
        assert!(theme.contains("theme/"));
    }
}
