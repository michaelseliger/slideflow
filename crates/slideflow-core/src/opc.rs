//! Open Packaging Conventions (ECMA-376 Part 2) layer.
//!
//! A PPTX file is a zip of "parts". This module gives the rest of the crate a
//! uniform view: read a package into memory, look up parts, follow and rewrite
//! relationships, edit `[Content_Types].xml`, and save back to a valid zip.
//!
//! Part names here are stored **without** a leading slash (`ppt/slides/slide1.xml`),
//! matching zip entry names. `[Content_Types].xml` `PartName` attributes use a
//! leading slash; conversion happens inside [`ContentTypes`].

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};
use std::path::Path;

use quick_xml::events::{BytesDecl, BytesStart, Event};
use quick_xml::{Reader, Writer};

use crate::error::{Error, Result};

pub const REL_NS: &str = "http://schemas.openxmlformats.org/package/2006/relationships";
pub const CT_NS: &str = "http://schemas.openxmlformats.org/package/2006/content-types";

/// Relationship types we care about (`Type` attribute values).
pub mod rel_type {
    pub const OFFICE_DOCUMENT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument";
    pub const SLIDE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide";
    pub const SLIDE_LAYOUT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideLayout";
    pub const SLIDE_MASTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/slideMaster";
    pub const THEME: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/theme";
    pub const IMAGE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/image";
    pub const NOTES_SLIDE: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesSlide";
    pub const NOTES_MASTER: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/notesMaster";
    pub const CORE_PROPS: &str =
        "http://schemas.openxmlformats.org/package/2006/relationships/metadata/core-properties";
    pub const EXTENDED_PROPS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/extended-properties";
    pub const PRES_PROPS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/presProps";
    pub const VIEW_PROPS: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/viewProps";
    pub const TABLE_STYLES: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/tableStyles";
    pub const FONT: &str =
        "http://schemas.openxmlformats.org/officeDocument/2006/relationships/font";
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Relationship {
    pub id: String,
    pub rel_type: String,
    pub target: String,
    /// `TargetMode="External"` (hyperlinks etc.) — target is a URI, not a part.
    pub external: bool,
}

/// An in-memory OPC package.
#[derive(Debug, Clone, Default)]
pub struct Package {
    parts: BTreeMap<String, Vec<u8>>,
}

impl Package {
    pub fn open(path: &Path) -> Result<Self> {
        let bytes = std::fs::read(path).map_err(|e| Error::io(path, e))?;
        Self::from_bytes(&bytes)
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes))?;
        let mut parts = BTreeMap::new();
        for i in 0..zip.len() {
            let mut file = zip.by_index(i)?;
            if file.is_dir() {
                continue;
            }
            let name = file.name().trim_start_matches('/').to_string();
            let mut buf = Vec::with_capacity(file.size() as usize);
            file.read_to_end(&mut buf)
                .map_err(|e| Error::InvalidPackage(format!("reading {name}: {e}")))?;
            parts.insert(name, buf);
        }
        if !parts.contains_key("[Content_Types].xml") {
            return Err(Error::InvalidPackage("missing [Content_Types].xml".into()));
        }
        Ok(Package { parts })
    }

    pub fn part(&self, name: &str) -> Option<&[u8]> {
        self.parts.get(name.trim_start_matches('/')).map(|v| v.as_slice())
    }

    pub fn require_part(&self, name: &str) -> Result<&[u8]> {
        self.part(name).ok_or_else(|| Error::MissingPart(name.to_string()))
    }

    pub fn has_part(&self, name: &str) -> bool {
        self.parts.contains_key(name.trim_start_matches('/'))
    }

    pub fn insert_part(&mut self, name: impl Into<String>, bytes: Vec<u8>) {
        let name: String = name.into();
        self.parts.insert(name.trim_start_matches('/').to_string(), bytes);
    }

    pub fn remove_part(&mut self, name: &str) -> Option<Vec<u8>> {
        self.parts.remove(name.trim_start_matches('/'))
    }

    pub fn part_names(&self) -> impl Iterator<Item = &str> {
        self.parts.keys().map(|s| s.as_str())
    }

    /// Relationships of a part (empty vec if it has no `.rels`).
    pub fn rels_for(&self, part_name: &str) -> Result<Vec<Relationship>> {
        let rels_name = rels_part_name(part_name);
        match self.part(&rels_name) {
            Some(bytes) => parse_rels(bytes, &rels_name),
            None => Ok(Vec::new()),
        }
    }

    /// Overwrite (or create) the `.rels` part for `part_name`.
    pub fn set_rels(&mut self, part_name: &str, rels: &[Relationship]) {
        self.insert_part(rels_part_name(part_name), write_rels(rels));
    }

    /// The part the package's root `officeDocument` relationship points at
    /// (for PPTX: `ppt/presentation.xml`).
    pub fn main_document_part(&self) -> Result<String> {
        let rels = self.rels_for("")?;
        rels.iter()
            .find(|r| r.rel_type == rel_type::OFFICE_DOCUMENT && !r.external)
            .map(|r| resolve_target("", &r.target))
            .ok_or_else(|| Error::InvalidPackage("no officeDocument relationship".into()))
    }

    pub fn content_types(&self) -> Result<ContentTypes> {
        ContentTypes::parse(self.require_part("[Content_Types].xml")?)
    }

    pub fn set_content_types(&mut self, ct: &ContentTypes) {
        self.insert_part("[Content_Types].xml", ct.to_xml());
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let mut cursor = Cursor::new(Vec::new());
        {
            let mut zip = zip::ZipWriter::new(&mut cursor);
            let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            for (name, bytes) in &self.parts {
                zip.start_file(name.clone(), opts)?;
                zip.write_all(bytes)
                    .map_err(|e| Error::InvalidPackage(format!("writing {name}: {e}")))?;
            }
            zip.finish()?;
        }
        Ok(cursor.into_inner())
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        let bytes = self.to_bytes()?;
        std::fs::write(path, bytes).map_err(|e| Error::io(path, e))
    }
}

/// `.rels` part name for a given part. `""` (package root) → `_rels/.rels`.
pub fn rels_part_name(part_name: &str) -> String {
    let part_name = part_name.trim_start_matches('/');
    if part_name.is_empty() {
        return "_rels/.rels".to_string();
    }
    match part_name.rsplit_once('/') {
        Some((dir, file)) => format!("{dir}/_rels/{file}.rels"),
        None => format!("_rels/{part_name}.rels"),
    }
}

/// Resolve a relationship `Target` relative to its source part into a
/// normalized part name. Handles `../`, absolute (`/ppt/...`) targets.
pub fn resolve_target(source_part: &str, target: &str) -> String {
    if let Some(abs) = target.strip_prefix('/') {
        return abs.to_string();
    }
    let source_part = source_part.trim_start_matches('/');
    let base_dir = match source_part.rsplit_once('/') {
        Some((dir, _)) => dir,
        None => "",
    };
    let mut segments: Vec<&str> = if base_dir.is_empty() {
        Vec::new()
    } else {
        base_dir.split('/').collect()
    };
    for seg in target.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                segments.pop();
            }
            s => segments.push(s),
        }
    }
    segments.join("/")
}

pub fn parse_rels(bytes: &[u8], part_for_errors: &str) -> Result<Vec<Relationship>> {
    let mut reader = Reader::from_reader(bytes);
    reader.config_mut().trim_text(true);
    let mut rels = Vec::new();
    let mut buf = Vec::new();
    loop {
        match reader
            .read_event_into(&mut buf)
            .map_err(|e| Error::xml(part_for_errors, e))?
        {
            Event::Start(ref e) | Event::Empty(ref e)
                if local_name(e.name().as_ref()) == b"Relationship" =>
            {
                let mut rel = Relationship {
                    id: String::new(),
                    rel_type: String::new(),
                    target: String::new(),
                    external: false,
                };
                for attr in e.attributes().flatten() {
                    let val = attr
                        .unescape_value()
                        .map_err(|e| Error::xml(part_for_errors, e))?
                        .into_owned();
                    match attr.key.as_ref() {
                        b"Id" => rel.id = val,
                        b"Type" => rel.rel_type = val,
                        b"Target" => rel.target = val,
                        b"TargetMode" => rel.external = val.eq_ignore_ascii_case("External"),
                        _ => {}
                    }
                }
                rels.push(rel);
            }
            Event::Eof => break,
            _ => {}
        }
        buf.clear();
    }
    Ok(rels)
}

pub fn write_rels(rels: &[Relationship]) -> Vec<u8> {
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    writer
        .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), Some("yes"))))
        .expect("in-memory write");
    let mut root = BytesStart::new("Relationships");
    root.push_attribute(("xmlns", REL_NS));
    writer.write_event(Event::Start(root)).expect("in-memory write");
    for rel in rels {
        let mut e = BytesStart::new("Relationship");
        e.push_attribute(("Id", rel.id.as_str()));
        e.push_attribute(("Type", rel.rel_type.as_str()));
        e.push_attribute(("Target", rel.target.as_str()));
        if rel.external {
            e.push_attribute(("TargetMode", "External"));
        }
        writer.write_event(Event::Empty(e)).expect("in-memory write");
    }
    writer
        .write_event(Event::End(quick_xml::events::BytesEnd::new("Relationships")))
        .expect("in-memory write");
    writer.into_inner().into_inner()
}

/// Parsed `[Content_Types].xml`.
#[derive(Debug, Clone, Default)]
pub struct ContentTypes {
    /// extension (lowercase, no dot) → content type
    pub defaults: BTreeMap<String, String>,
    /// part name (leading slash) → content type
    pub overrides: BTreeMap<String, String>,
}

impl ContentTypes {
    pub fn parse(bytes: &[u8]) -> Result<Self> {
        let mut reader = Reader::from_reader(bytes);
        reader.config_mut().trim_text(true);
        let mut ct = ContentTypes::default();
        let mut buf = Vec::new();
        loop {
            match reader
                .read_event_into(&mut buf)
                .map_err(|e| Error::xml("[Content_Types].xml", e))?
            {
                Event::Start(ref e) | Event::Empty(ref e) => {
                    let name = local_name(e.name().as_ref()).to_vec();
                    let mut a1 = None;
                    let mut a2 = None;
                    for attr in e.attributes().flatten() {
                        let val = attr
                            .unescape_value()
                            .map_err(|e| Error::xml("[Content_Types].xml", e))?
                            .into_owned();
                        match attr.key.as_ref() {
                            b"Extension" | b"PartName" => a1 = Some(val),
                            b"ContentType" => a2 = Some(val),
                            _ => {}
                        }
                    }
                    if let (Some(key), Some(val)) = (a1, a2) {
                        match name.as_slice() {
                            b"Default" => {
                                ct.defaults.insert(key.to_ascii_lowercase(), val);
                            }
                            b"Override" => {
                                ct.overrides.insert(key, val);
                            }
                            _ => {}
                        }
                    }
                }
                Event::Eof => break,
                _ => {}
            }
            buf.clear();
        }
        Ok(ct)
    }

    /// Content type of a part name (without leading slash), consulting
    /// overrides first, then extension defaults.
    pub fn content_type_of(&self, part_name: &str) -> Option<&str> {
        let with_slash = format!("/{}", part_name.trim_start_matches('/'));
        if let Some(ct) = self.overrides.get(&with_slash) {
            return Some(ct);
        }
        let ext = part_name.rsplit_once('.')?.1.to_ascii_lowercase();
        self.defaults.get(&ext).map(|s| s.as_str())
    }

    pub fn set_override(&mut self, part_name: &str, content_type: impl Into<String>) {
        let with_slash = format!("/{}", part_name.trim_start_matches('/'));
        self.overrides.insert(with_slash, content_type.into());
    }

    pub fn remove_override(&mut self, part_name: &str) {
        let with_slash = format!("/{}", part_name.trim_start_matches('/'));
        self.overrides.remove(&with_slash);
    }

    pub fn ensure_default(&mut self, extension: &str, content_type: impl Into<String>) {
        self.defaults
            .entry(extension.to_ascii_lowercase())
            .or_insert_with(|| content_type.into());
    }

    pub fn to_xml(&self) -> Vec<u8> {
        let mut writer = Writer::new(Cursor::new(Vec::new()));
        writer
            .write_event(Event::Decl(BytesDecl::new("1.0", Some("UTF-8"), Some("yes"))))
            .expect("in-memory write");
        let mut root = BytesStart::new("Types");
        root.push_attribute(("xmlns", CT_NS));
        writer.write_event(Event::Start(root)).expect("in-memory write");
        for (ext, ct) in &self.defaults {
            let mut e = BytesStart::new("Default");
            e.push_attribute(("Extension", ext.as_str()));
            e.push_attribute(("ContentType", ct.as_str()));
            writer.write_event(Event::Empty(e)).expect("in-memory write");
        }
        for (part, ct) in &self.overrides {
            let mut e = BytesStart::new("Override");
            e.push_attribute(("PartName", part.as_str()));
            e.push_attribute(("ContentType", ct.as_str()));
            writer.write_event(Event::Empty(e)).expect("in-memory write");
        }
        writer
            .write_event(Event::End(quick_xml::events::BytesEnd::new("Types")))
            .expect("in-memory write");
        writer.into_inner().into_inner()
    }
}

/// Strip an XML namespace prefix: `p:sp` → `sp`.
pub fn local_name(qname: &[u8]) -> &[u8] {
    match qname.iter().rposition(|&b| b == b':') {
        Some(pos) => &qname[pos + 1..],
        None => qname,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rels_part_names() {
        assert_eq!(rels_part_name(""), "_rels/.rels");
        assert_eq!(rels_part_name("ppt/presentation.xml"), "ppt/_rels/presentation.xml.rels");
        assert_eq!(rels_part_name("ppt/slides/slide1.xml"), "ppt/slides/_rels/slide1.xml.rels");
    }

    #[test]
    fn target_resolution() {
        assert_eq!(resolve_target("ppt/presentation.xml", "slides/slide1.xml"), "ppt/slides/slide1.xml");
        assert_eq!(resolve_target("ppt/slides/slide1.xml", "../media/image1.png"), "ppt/media/image1.png");
        assert_eq!(resolve_target("ppt/slides/slide1.xml", "/docProps/core.xml"), "docProps/core.xml");
        assert_eq!(resolve_target("", "ppt/presentation.xml"), "ppt/presentation.xml");
    }

    #[test]
    fn rels_roundtrip() {
        let rels = vec![
            Relationship {
                id: "rId1".into(),
                rel_type: rel_type::SLIDE.into(),
                target: "slides/slide1.xml".into(),
                external: false,
            },
            Relationship {
                id: "rId2".into(),
                rel_type: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink".into(),
                target: "https://example.com".into(),
                external: true,
            },
        ];
        let xml = write_rels(&rels);
        let parsed = parse_rels(&xml, "test").unwrap();
        assert_eq!(parsed, rels);
    }

    #[test]
    fn content_types_roundtrip() {
        let mut ct = ContentTypes::default();
        ct.ensure_default("rels", "application/vnd.openxmlformats-package.relationships+xml");
        ct.ensure_default("xml", "application/xml");
        ct.set_override(
            "ppt/slides/slide1.xml",
            "application/vnd.openxmlformats-officedocument.presentationml.slide+xml",
        );
        let xml = ct.to_xml();
        let parsed = ContentTypes::parse(&xml).unwrap();
        assert_eq!(
            parsed.content_type_of("ppt/slides/slide1.xml").unwrap(),
            "application/vnd.openxmlformats-officedocument.presentationml.slide+xml"
        );
        assert_eq!(
            parsed.content_type_of("_rels/.rels").unwrap(),
            "application/vnd.openxmlformats-package.relationships+xml"
        );
        assert!(parsed.content_type_of("ppt/media/image1.png").is_none());
    }
}
