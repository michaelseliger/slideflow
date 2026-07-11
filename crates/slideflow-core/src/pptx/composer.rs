//! Style-preserving deck composition.
//!
//! Builds a brand-new PPTX from slides picked out of arbitrary source decks.
//! The non-negotiable requirement: **each slide keeps the exact look it had in
//! its source deck.** That means every copied slide must bring its complete
//! relationship closure along:
//!
//! - the slide part itself, with rewritten `.rels`
//! - every non-external relationship target, recursively (images, charts +
//!   embedded workbooks, diagrams, audio/video, …)
//! - its slide layout → that layout's slide master → that master's theme
//! - `[Content_Types].xml` overrides/defaults for every copied part
//! - `ppt/presentation.xml` `sldIdLst`/`sldMasterIdLst` and its `.rels`
//!
//! Identical parts (e.g. two slides from the same deck sharing a master, or
//! two decks built from the same corporate template) must be deduplicated by
//! content hash so the output stays small and masters aren't multiplied.
//!
//! IMPLEMENTATION NOTES (contract for the module owner):
//! - Copied parts get fresh sequential names in the output
//!   (`ppt/slides/slideN.xml`, `ppt/slideLayouts/slideLayoutN.xml`,
//!   `ppt/media/imageN.ext`, …); all referring `.rels` are rewritten.
//! - A master copied into the output must have its `sldLayoutIdLst` rewritten
//!   to reference only the layouts actually copied with it (fresh
//!   `sldLayoutId` ids ≥ 2147483649, unique across the output).
//! - `sldId` ids in `sldIdLst` start at 256, unique; `r:id`s regenerated.
//! - Notes slides are dropped by default (`ComposeOptions::include_notes`),
//!   because notes masters multiply quickly; when included, bring the notes
//!   master closure too.
//! - The output must contain `docProps/core.xml` (title from
//!   `ComposeOptions::title`) and a root `_rels/.rels`.
//! - Presentation-level parts (`presProps`, `viewProps`, `tableStyles`) are
//!   NOT required; if the first source deck has them, copying them is fine.
//! - `p:sldSz` comes from the first source deck.
//! - External relationships (hyperlinks) are preserved verbatim.

use std::collections::HashMap;
use std::io::Cursor;
use std::path::Path;

use quick_xml::events::{BytesEnd, BytesStart, Event};
use quick_xml::{Reader, Writer, XmlVersion};

use crate::error::{Error, Result};
use crate::fixtures::xml_escape;
use crate::model::{ComposeReport, FitMode, SlidePick};
use crate::opc::{
    local_name, rel_type, resolve_target, ContentTypes, Package, Relationship,
};
use crate::pptx::scale::{is_same_aspect, scale_part_xml, SlideScale};
use crate::pptx::PresentationFile;

const CT_PRESENTATION: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml";
const CT_CORE: &str = "application/vnd.openxmlformats-package.core-properties+xml";
const CT_APP: &str = "application/vnd.openxmlformats-officedocument.extended-properties+xml";
const CT_TABLE_STYLES: &str =
    "application/vnd.openxmlformats-officedocument.presentationml.tableStyles+xml";
const CT_RELS: &str = "application/vnd.openxmlformats-package.relationships+xml";

const NS_DECL: &str = concat!(
    r#"xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" "#,
    r#"xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships" "#,
    r#"xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main""#
);

#[derive(Debug, Clone)]
pub struct ComposeOptions {
    /// Title written to the output's docProps/core.xml.
    pub title: String,
    /// Carry speaker notes into the output (default: false).
    pub include_notes: bool,
    /// How to fit slides whose *aspect ratio* differs from the output canvas.
    /// `None` (the back-compat default) leaves aspect-mismatched slides
    /// unscaled and only warns; same-aspect size mismatches are always scaled
    /// regardless of this setting.
    pub fit_mode: Option<FitMode>,
}

impl Default for ComposeOptions {
    fn default() -> Self {
        ComposeOptions { title: "Slideflow Deck".into(), include_notes: false, fit_mode: None }
    }
}

/// Compose a new PPTX at `output_path` from `picks`, in order.
///
/// Source decks are opened at most once each, regardless of how many slides
/// are picked from them or in what interleaving.
pub fn compose(
    picks: &[SlidePick],
    output_path: &Path,
    options: &ComposeOptions,
) -> Result<ComposeReport> {
    if picks.is_empty() {
        return Err(Error::Compose("no slides picked".into()));
    }

    let mut composer = Composer::new(options.include_notes);

    // Open each distinct source deck once, preserving pick order.
    let mut deck_index: HashMap<String, usize> = HashMap::new();
    for pick in picks {
        if !deck_index.contains_key(&pick.pptx_path) {
            let pf = PresentationFile::open(Path::new(&pick.pptx_path))?;
            let ct = pf.package.content_types()?;
            let idx = composer.decks.len();
            deck_index.insert(pick.pptx_path.clone(), idx);
            composer.decks.push(DeckState {
                path: pick.pptx_path.clone(),
                pf,
                ct,
                copied: HashMap::new(),
                scale: None,
            });
        }
    }

    // Slide size / notes size / default text style come from the first source
    // deck. Decks that disagree on size are rescaled onto the shared canvas
    // (same-aspect: always; aspect mismatch: only when a fit mode is chosen).
    let first_idx = deck_index[&picks[0].pptx_path];
    let slide_w = composer.decks[first_idx].pf.slide_width_emu;
    let slide_h = composer.decks[first_idx].pf.slide_height_emu;
    let notes_sz = parse_notes_sz(&composer.decks[first_idx].pf);
    let default_text_style =
        presentation_xml(&composer.decks[first_idx].pf)
            .and_then(|xml| extract_element_raw(&xml, "p:defaultTextStyle"))
            .unwrap_or_default();
    composer.resolve_deck_scaling(first_idx, slide_w, slide_h, options.fit_mode);

    // Copy each picked slide's full closure, in pick order.
    for pick in picks {
        let deck = deck_index[&pick.pptx_path];
        let slide_part = composer.decks[deck].pf.slide_part(pick.slide_index)?.to_string();
        composer.copy_slide(deck, &slide_part)?;
    }

    composer.finalize_masters()?;
    let extra_rels = composer.carry_presentation_level_parts(first_idx)?;
    composer.build_presentation(slide_w, slide_h, &notes_sz, &default_text_style, extra_rels)?;
    composer.build_core(&options.title);
    composer.build_app();
    composer.build_root_rels();
    composer.finish_content_types();

    composer.out_pkg.save(output_path)?;

    Ok(ComposeReport {
        output_path: output_path.to_string_lossy().into_owned(),
        slides_written: composer.slides_out.len(),
        source_decks: composer.decks.len(),
        warnings: composer.warnings,
        notes: composer.notes,
    })
}

struct DeckState {
    /// Source file path (for warnings).
    path: String,
    pf: PresentationFile,
    ct: ContentTypes,
    /// Original part name → output part name (within-deck dedup).
    copied: HashMap<String, String>,
    /// Uniform scale applied to this deck's slide/layout/master parts so its
    /// slides fit the output canvas. `None` for the reference deck and any deck
    /// whose size already matches (or an aspect mismatch left unscaled).
    scale: Option<SlideScale>,
}

/// A master pending finalization: its `sldLayoutIdLst` and `.rels` are rewritten
/// only after every pick is processed, since layouts accrue incrementally.
#[derive(Clone)]
struct MasterOut {
    body: Vec<u8>,
    /// Non-layout rels (theme, background images, …) with original ids preserved.
    base_rels: Vec<Relationship>,
    /// Output names of layouts assigned to this master, in insertion order.
    layouts: Vec<String>,
}

struct Composer {
    include_notes: bool,
    decks: Vec<DeckState>,
    out_pkg: Package,
    out_ct: ContentTypes,
    /// (dir, digit-stripped stem, ext) → last index handed out.
    name_counter: HashMap<(String, String, String), u32>,
    /// content hash → output name, for leaf parts (media, themes) deduped globally.
    leaf_by_hash: HashMap<String, String>,
    masters: HashMap<String, MasterOut>,
    master_order: Vec<String>,
    notes_masters: Vec<String>,
    slides_out: Vec<String>,
    next_big_id: u64,
    warnings: Vec<String>,
    notes: Vec<String>,
    notes_dropped_warned: bool,
}

impl Composer {
    fn new(include_notes: bool) -> Self {
        let mut out_ct = ContentTypes::default();
        out_ct.ensure_default("rels", CT_RELS);
        out_ct.ensure_default("xml", "application/xml");
        Composer {
            include_notes,
            decks: Vec::new(),
            out_pkg: Package::default(),
            out_ct,
            name_counter: HashMap::new(),
            leaf_by_hash: HashMap::new(),
            masters: HashMap::new(),
            master_order: Vec::new(),
            notes_masters: Vec::new(),
            slides_out: Vec::new(),
            // Layout ids are finalized before master ids, so the first id issued
            // is a sldLayoutId — whose ECMA-376 minimum (2147483649) is one
            // higher than sldMasterId's (2147483648). Start at the layout minimum
            // so both are satisfied and the module contract (>= 2147483649 for
            // fresh layout ids) holds.
            next_big_id: 2_147_483_649,
            warnings: Vec::new(),
            notes: Vec::new(),
            notes_dropped_warned: false,
        }
    }

    fn part_bytes(&self, deck: usize, name: &str) -> Option<Vec<u8>> {
        self.decks[deck].pf.package.part(name).map(|b| b.to_vec())
    }

    /// Rescale a slide/layout/master part's geometry onto the output canvas when
    /// this deck was assigned a scale; otherwise return the bytes untouched (so
    /// same-size decks stay byte-for-byte identical to their source).
    fn scale_bytes(&self, deck: usize, bytes: Vec<u8>) -> Result<Vec<u8>> {
        match self.decks[deck].scale {
            Some(sc) => scale_part_xml(&bytes, &sc),
            None => Ok(bytes),
        }
    }

    fn deck_rels(&self, deck: usize, name: &str) -> Result<Vec<Relationship>> {
        self.decks[deck].pf.package.rels_for(name)
    }

    fn alloc(&mut self, orig: &str) -> String {
        let (dir, file) = orig.rsplit_once('/').unwrap_or(("", orig));
        let (stem_full, ext) = file.rsplit_once('.').unwrap_or((file, ""));
        let stripped = stem_full.trim_end_matches(|c: char| c.is_ascii_digit());
        let stem = if stripped.is_empty() { stem_full } else { stripped };
        let key = (dir.to_string(), stem.to_string(), ext.to_string());
        let n = self.name_counter.entry(key).or_insert(0);
        *n += 1;
        let n = *n;
        if ext.is_empty() {
            format!("{dir}/{stem}{n}")
        } else {
            format!("{dir}/{stem}{n}.{ext}")
        }
    }

    /// Replicate the source content type (override or extension default) for a
    /// copied part under its new name.
    fn carry_ct(&mut self, deck: usize, orig: &str, new_name: &str) {
        let key = format!("/{}", orig.trim_start_matches('/'));
        if let Some(ct) = self.decks[deck].ct.overrides.get(&key).cloned() {
            self.out_ct.set_override(new_name, ct);
        } else if let Some((_, ext)) = new_name.rsplit_once('.') {
            let ext = ext.to_ascii_lowercase();
            match self.decks[deck].ct.defaults.get(&ext).cloned() {
                Some(ct) => self.out_ct.ensure_default(&ext, ct),
                None => self.out_ct.ensure_default(&ext, "application/octet-stream"),
            }
        }
    }

    fn warn_missing(&mut self, source: &str, target: &str) {
        self.warnings.push(format!(
            "skipped unresolvable relationship target {target} referenced by {source}"
        ));
    }

    /// Decide, per source deck, how to reconcile its slide size with the output
    /// canvas (the first deck's size), and record scales/notes/warnings:
    ///
    /// - equal size → nothing (scale stays `None`).
    /// - same aspect ratio, different size → always scale + informational note.
    /// - aspect mismatch → if `fit_mode` is set, scale (EnsureFit/Maximize) +
    ///   note; otherwise leave unscaled and emit the legacy size-mismatch warning
    ///   (back-compat for callers that don't opt into scaling).
    ///
    /// Also carries forward the embedded-font warning (fonts are never carried).
    fn resolve_deck_scaling(
        &mut self,
        first: usize,
        slide_w: i64,
        slide_h: i64,
        fit_mode: Option<FitMode>,
    ) {
        let dst = (slide_w, slide_h);
        let mut warnings = Vec::new();
        let mut notes = Vec::new();
        let mut assigns: Vec<(usize, SlideScale)> = Vec::new();

        for (i, deck) in self.decks.iter().enumerate() {
            let name = Path::new(&deck.path)
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| deck.path.clone());
            let src = (deck.pf.slide_width_emu, deck.pf.slide_height_emu);

            if i != first && src != dst {
                if is_same_aspect(src, dst) {
                    // Same aspect: unambiguous, so scale regardless of fit_mode.
                    if let Some(sc) = SlideScale::compute(src, dst, FitMode::EnsureFit) {
                        notes.push(format!(
                            "{name} scaled to {}% to match output size",
                            sc.percent()
                        ));
                        assigns.push((i, sc));
                    }
                } else {
                    match fit_mode {
                        Some(mode) => {
                            if let Some(sc) = SlideScale::compute(src, dst, mode) {
                                notes.push(format!(
                                    "{name} scaled to {}% to match output size",
                                    sc.percent()
                                ));
                                assigns.push((i, sc));
                            }
                        }
                        None => {
                            warnings.push(format!(
                                "{name} uses a different slide size ({}x{} vs {slide_w}x{slide_h} EMU); \
                                 its slides keep their absolute layout on the output canvas",
                                src.0, src.1
                            ));
                        }
                    }
                }
            }

            if presentation_xml(&deck.pf).is_some_and(|xml| xml.contains("<p:embeddedFontLst")) {
                warnings.push(format!("{name} embeds fonts; embedded fonts are not carried over"));
            }
        }

        for (i, sc) in assigns {
            self.decks[i].scale = Some(sc);
        }
        self.warnings.extend(warnings);
        self.notes.extend(notes);
    }

    /// Copy presentation-level style parts into the output and return the
    /// relationship type + output part name pairs for `build_presentation`.
    ///
    /// `presProps` / `viewProps` come from the first source deck. `tableStyles`
    /// is *merged* across all source decks: slides reference table styles by
    /// GUID into this single presentation-level part, so a slide from deck B
    /// would lose its table styling if only deck A's part were carried.
    fn carry_presentation_level_parts(
        &mut self,
        first: usize,
    ) -> Result<Vec<(String, String)>> {
        let mut extra: Vec<(String, String)> = Vec::new();

        for (rel_ty, out_name) in [
            (rel_type::PRES_PROPS, "ppt/presProps.xml"),
            (rel_type::VIEW_PROPS, "ppt/viewProps.xml"),
        ] {
            let main = match self.decks[first].pf.package.main_document_part() {
                Ok(m) => m,
                Err(_) => continue,
            };
            let rels = self.deck_rels(first, &main)?;
            if let Some(rel) = rels.iter().find(|r| r.rel_type == rel_ty && !r.external) {
                let resolved = resolve_target(&main, &rel.target);
                if self.decks[first].pf.package.has_part(&resolved) {
                    // Canonical fixed name (alloc() names always carry a number,
                    // so this can never collide with slide-closure parts).
                    self.copy_fixed_name(first, &resolved, out_name)?;
                    extra.push((rel_ty.to_string(), out_name.to_string()));
                }
            }
        }

        if let Some(bytes) = self.merge_table_styles()? {
            let name = "ppt/tableStyles.xml".to_string();
            self.out_pkg.insert_part(name.clone(), bytes);
            self.out_ct.set_override(&name, CT_TABLE_STYLES);
            extra.push((rel_type::TABLE_STYLES.to_string(), name));
        }

        Ok(extra)
    }

    /// The plain relationship-copy skeleton: clone external rels verbatim,
    /// resolve internal ones, warn+skip missing targets, recursively copy the
    /// rest via [`Composer::copy_target`], and rewrite each into an output
    /// relationship rooted at `out_name`.
    ///
    /// This is the shared body for copiers with **no** per-rel-type special case
    /// (`copy_fixed_name`, `copy_generic`, `copy_notes_master`). The copiers that
    /// *do* special-case a rel type mid-loop (slide, layout, master, notes-slide)
    /// open-code their own loop, because their divergences — pruning layouts
    /// before resolution, routing rels into `self.masters` instead of the output
    /// package, slide-jump/back-reference handling, `mo.layouts` bookkeeping —
    /// don't fold cleanly into one callback.
    fn copy_plain_rels(
        &mut self,
        deck: usize,
        orig: &str,
        out_name: &str,
        rels: &[Relationship],
    ) -> Result<Vec<Relationship>> {
        let mut new_rels = Vec::new();
        for rel in rels {
            if rel.external {
                new_rels.push(rel.clone());
                continue;
            }
            let resolved = resolve_target(orig, &rel.target);
            if !self.decks[deck].pf.package.has_part(&resolved) {
                self.warn_missing(orig, &resolved);
                continue;
            }
            let child = self.copy_target(deck, &resolved)?;
            new_rels.push(rewritten_rel(rel, out_name, &child));
        }
        Ok(new_rels)
    }

    /// Copy a part under a fixed output name, following its relationships like
    /// [`Composer::copy_generic`] does.
    fn copy_fixed_name(&mut self, deck: usize, orig: &str, out_name: &str) -> Result<()> {
        let bytes = self
            .part_bytes(deck, orig)
            .ok_or_else(|| Error::MissingPart(orig.to_string()))?;
        let rels = self.deck_rels(deck, orig)?;
        let new_rels = self.copy_plain_rels(deck, orig, out_name, &rels)?;
        self.out_pkg.insert_part(out_name.to_string(), bytes);
        if !new_rels.is_empty() {
            self.out_pkg.set_rels(out_name, &new_rels);
        }
        self.carry_ct(deck, orig, out_name);
        Ok(())
    }

    /// Merge every source deck's `tableStyles.xml` into one part, deduplicating
    /// styles by their GUID `styleId` (first deck wins, including the `def`
    /// default-style attribute). Returns `None` when no deck has table styles.
    fn merge_table_styles(&mut self) -> Result<Option<Vec<u8>>> {
        let mut def: Option<String> = None;
        let mut seen_ids: Vec<String> = Vec::new();
        let mut styles = String::new();
        let mut found_any = false;

        for deck in 0..self.decks.len() {
            let Ok(main) = self.decks[deck].pf.package.main_document_part() else { continue };
            let rels = self.deck_rels(deck, &main)?;
            let Some(rel) = rels
                .iter()
                .find(|r| r.rel_type == rel_type::TABLE_STYLES && !r.external)
            else {
                continue;
            };
            let resolved = resolve_target(&main, &rel.target);
            let Some(bytes) = self.part_bytes(deck, &resolved) else { continue };
            let xml = String::from_utf8_lossy(&bytes).into_owned();
            found_any = true;

            if def.is_none() {
                def = attr_value(&xml, "tblStyleLst", "def");
            }
            for style in extract_all_elements_raw(&xml, "a:tblStyle") {
                let id = attr_value(&style, "tblStyle", "styleId").unwrap_or_default();
                if !id.is_empty() && seen_ids.iter().any(|s| s == &id) {
                    continue;
                }
                seen_ids.push(id);
                styles.push_str(&style);
            }
        }

        if !found_any {
            return Ok(None);
        }
        let def_attr = def
            .map(|d| format!(r#" def="{}""#, xml_escape(&d)))
            .unwrap_or_default();
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main"{def_attr}>{styles}</a:tblStyleLst>"#
        );
        Ok(Some(xml.into_bytes()))
    }

    /// Copy a picked slide and its whole closure; register it in `slides_out`.
    fn copy_slide(&mut self, deck: usize, orig: &str) -> Result<String> {
        let name = self.alloc(orig);
        let bytes = self
            .part_bytes(deck, orig)
            .ok_or_else(|| Error::MissingPart(orig.to_string()))?;
        let rels = self.deck_rels(deck, orig)?;

        let mut new_rels = Vec::new();
        for rel in &rels {
            if rel.external {
                new_rels.push(rel.clone());
                continue;
            }
            let resolved = resolve_target(orig, &rel.target);
            if !self.decks[deck].pf.package.has_part(&resolved) {
                self.warn_missing(orig, &resolved);
                continue;
            }
            if rel.rel_type == rel_type::NOTES_SLIDE {
                if !self.include_notes {
                    if !self.notes_dropped_warned {
                        self.warnings
                            .push("speaker notes dropped (include_notes = false)".into());
                        self.notes_dropped_warned = true;
                    }
                    continue;
                }
                let child = self.copy_notes_slide(deck, &resolved, &name)?;
                new_rels.push(rewritten_rel(rel, &name, &child));
            } else if rel.rel_type == rel_type::SLIDE {
                // A slide-jump hyperlink. The target slide is copied (with its
                // closure) so the r:id resolves, but it is not part of the
                // output slide list — the jump lands outside the show.
                self.warnings.push(format!(
                    "slide link on {orig} points at a slide that is not part of the composition"
                ));
                let child = self.copy_target(deck, &resolved)?;
                new_rels.push(rewritten_rel(rel, &name, &child));
            } else {
                let child = self.copy_target(deck, &resolved)?;
                new_rels.push(rewritten_rel(rel, &name, &child));
            }
        }

        let bytes = self.scale_bytes(deck, bytes)?;
        self.out_pkg.insert_part(name.clone(), bytes);
        self.out_pkg.set_rels(&name, &new_rels);
        self.carry_ct(deck, orig, &name);
        self.slides_out.push(name.clone());
        Ok(name)
    }

    /// Dispatch an arbitrary relationship target to the right copier.
    fn copy_target(&mut self, deck: usize, orig: &str) -> Result<String> {
        if orig.contains("/slideLayouts/") {
            self.copy_layout(deck, orig)
        } else if orig.contains("/slideMasters/") {
            self.copy_master(deck, orig)
        } else if orig.contains("/notesMasters/") {
            self.copy_notes_master(deck, orig)
        } else {
            self.copy_generic(deck, orig)
        }
    }

    /// Media, charts, themes, embeddings, diagrams — copied verbatim with rels
    /// followed recursively. Leaf parts (no internal rels) dedupe globally by
    /// content hash; everything else dedupes within its source deck by name.
    fn copy_generic(&mut self, deck: usize, orig: &str) -> Result<String> {
        if let Some(existing) = self.decks[deck].copied.get(orig).cloned() {
            return Ok(existing);
        }
        let bytes = self
            .part_bytes(deck, orig)
            .ok_or_else(|| Error::MissingPart(orig.to_string()))?;
        // A slide-jump target reaches copy_generic (it is not a layout/master/
        // notesMaster). Its layout/master closure IS rescaled, so its own
        // geometry must be too, or a mixed-size composition emits an internally
        // inconsistent package. Notes slides (ppt/notesSlides/) are deliberately
        // NOT scaled — they use the separate notesSz canvas. scale_bytes is a
        // no-op when this deck has no scale, so same-size decks stay identical.
        let bytes = if orig.starts_with("ppt/slides/") {
            self.scale_bytes(deck, bytes)?
        } else {
            bytes
        };
        let rels = self.deck_rels(deck, orig)?;
        let has_internal = rels.iter().any(|r| !r.external);

        // Theme parts are exempt from cross-deck hash dedup: PowerPoint pairs
        // every master with its own theme part, and sharing one part between
        // masters is a structural deviation some consumers handle badly.
        if !has_internal && !orig.contains("/theme/") {
            let hash = crate::hash::sha256_hex(&bytes);
            if let Some(existing) = self.leaf_by_hash.get(&hash).cloned() {
                self.decks[deck].copied.insert(orig.to_string(), existing.clone());
                return Ok(existing);
            }
            let name = self.alloc(orig);
            self.decks[deck].copied.insert(orig.to_string(), name.clone());
            let ext_rels: Vec<Relationship> =
                rels.iter().filter(|r| r.external).cloned().collect();
            self.out_pkg.insert_part(name.clone(), bytes);
            if !ext_rels.is_empty() {
                self.out_pkg.set_rels(&name, &ext_rels);
            }
            self.carry_ct(deck, orig, &name);
            self.leaf_by_hash.insert(hash, name.clone());
            return Ok(name);
        }

        let name = self.alloc(orig);
        self.decks[deck].copied.insert(orig.to_string(), name.clone());
        let new_rels = self.copy_plain_rels(deck, orig, &name, &rels)?;
        self.out_pkg.insert_part(name.clone(), bytes);
        if !new_rels.is_empty() {
            self.out_pkg.set_rels(&name, &new_rels);
        }
        self.carry_ct(deck, orig, &name);
        Ok(name)
    }

    fn copy_layout(&mut self, deck: usize, orig: &str) -> Result<String> {
        if let Some(existing) = self.decks[deck].copied.get(orig).cloned() {
            return Ok(existing);
        }
        let name = self.alloc(orig);
        self.decks[deck].copied.insert(orig.to_string(), name.clone());
        let bytes = self
            .part_bytes(deck, orig)
            .ok_or_else(|| Error::MissingPart(orig.to_string()))?;
        let rels = self.deck_rels(deck, orig)?;

        let mut new_rels = Vec::new();
        for rel in &rels {
            if rel.external {
                new_rels.push(rel.clone());
                continue;
            }
            let resolved = resolve_target(orig, &rel.target);
            if !self.decks[deck].pf.package.has_part(&resolved) {
                self.warn_missing(orig, &resolved);
                continue;
            }
            if rel.rel_type == rel_type::SLIDE_MASTER {
                let master = self.copy_master(deck, &resolved)?;
                if let Some(mo) = self.masters.get_mut(&master) {
                    if !mo.layouts.contains(&name) {
                        mo.layouts.push(name.clone());
                    }
                }
                new_rels.push(rewritten_rel(rel, &name, &master));
            } else {
                let child = self.copy_target(deck, &resolved)?;
                new_rels.push(rewritten_rel(rel, &name, &child));
            }
        }
        let bytes = self.scale_bytes(deck, bytes)?;
        self.out_pkg.insert_part(name.clone(), bytes);
        self.out_pkg.set_rels(&name, &new_rels);
        self.carry_ct(deck, orig, &name);
        Ok(name)
    }

    /// Copy a master's content and theme closure, deferring its `sldLayoutIdLst`
    /// and `.rels` to [`Composer::finalize_masters`]. Layout rels are
    /// intentionally NOT followed here — layouts are pulled in only as slides
    /// require them.
    fn copy_master(&mut self, deck: usize, orig: &str) -> Result<String> {
        if let Some(existing) = self.decks[deck].copied.get(orig).cloned() {
            return Ok(existing);
        }
        let name = self.alloc(orig);
        self.decks[deck].copied.insert(orig.to_string(), name.clone());
        let bytes = self
            .part_bytes(deck, orig)
            .ok_or_else(|| Error::MissingPart(orig.to_string()))?;
        let rels = self.deck_rels(deck, orig)?;

        let mut base_rels = Vec::new();
        for rel in &rels {
            if rel.external {
                base_rels.push(rel.clone());
                continue;
            }
            if rel.rel_type == rel_type::SLIDE_LAYOUT {
                continue; // pruned; re-emitted in finalize_masters for copied layouts only
            }
            let resolved = resolve_target(orig, &rel.target);
            if !self.decks[deck].pf.package.has_part(&resolved) {
                self.warn_missing(orig, &resolved);
                continue;
            }
            let child = self.copy_target(deck, &resolved)?;
            base_rels.push(rewritten_rel(rel, &name, &child));
        }

        let bytes = self.scale_bytes(deck, bytes)?;
        self.master_order.push(name.clone());
        self.masters
            .insert(name.clone(), MasterOut { body: bytes, base_rels, layouts: Vec::new() });
        self.carry_ct(deck, orig, &name);
        Ok(name)
    }

    fn copy_notes_master(&mut self, deck: usize, orig: &str) -> Result<String> {
        if let Some(existing) = self.decks[deck].copied.get(orig).cloned() {
            return Ok(existing);
        }
        let name = self.alloc(orig);
        self.decks[deck].copied.insert(orig.to_string(), name.clone());
        let bytes = self
            .part_bytes(deck, orig)
            .ok_or_else(|| Error::MissingPart(orig.to_string()))?;
        let rels = self.deck_rels(deck, orig)?;
        let new_rels = self.copy_plain_rels(deck, orig, &name, &rels)?;
        self.out_pkg.insert_part(name.clone(), bytes);
        self.out_pkg.set_rels(&name, &new_rels);
        self.carry_ct(deck, orig, &name);
        self.notes_masters.push(name.clone());
        Ok(name)
    }

    /// Notes slides are per-slide-instance and never deduped (each carries a
    /// back-reference to its owning slide, which differs per pick).
    fn copy_notes_slide(&mut self, deck: usize, orig: &str, slide_out: &str) -> Result<String> {
        let name = self.alloc(orig);
        let bytes = self
            .part_bytes(deck, orig)
            .ok_or_else(|| Error::MissingPart(orig.to_string()))?;
        let rels = self.deck_rels(deck, orig)?;

        let mut new_rels = Vec::new();
        for rel in &rels {
            if rel.external {
                new_rels.push(rel.clone());
                continue;
            }
            if rel.rel_type == rel_type::SLIDE {
                // Back-reference to the slide we were copied for.
                new_rels.push(rewritten_rel(rel, &name, slide_out));
                continue;
            }
            let resolved = resolve_target(orig, &rel.target);
            if !self.decks[deck].pf.package.has_part(&resolved) {
                self.warn_missing(orig, &resolved);
                continue;
            }
            let child = self.copy_target(deck, &resolved)?;
            new_rels.push(rewritten_rel(rel, &name, &child));
        }
        self.out_pkg.insert_part(name.clone(), bytes);
        self.out_pkg.set_rels(&name, &new_rels);
        self.carry_ct(deck, orig, &name);
        Ok(name)
    }

    fn finalize_masters(&mut self) -> Result<()> {
        let order = self.master_order.clone();
        for master_name in &order {
            let Some(mo) = self.masters.get(master_name).cloned() else { continue };
            let mut rels = mo.base_rels.clone();
            let mut entries: Vec<(u64, String)> = Vec::new();
            for (next, layout) in (max_rid_num(&rels) + 1..).zip(&mo.layouts) {
                let rid = format!("rId{next}");
                let big = self.next_big_id;
                self.next_big_id += 1;
                rels.push(Relationship {
                    id: rid.clone(),
                    rel_type: rel_type::SLIDE_LAYOUT.into(),
                    target: relative_target(master_name, layout),
                    external: false,
                });
                entries.push((big, rid));
            }
            let body = rewrite_master_layout_list(&mo.body, &entries, master_name)?;
            self.out_pkg.insert_part(master_name.clone(), body);
            self.out_pkg.set_rels(master_name, &rels);
        }
        Ok(())
    }

    fn build_presentation(
        &mut self,
        slide_w: i64,
        slide_h: i64,
        notes_sz: &str,
        default_text_style: &str,
        extra_rels: Vec<(String, String)>,
    ) -> Result<()> {
        let pres = "ppt/presentation.xml";
        let mut pres_rels: Vec<Relationship> = Vec::new();
        let mut rid = 1u32;

        let master_order = self.master_order.clone();
        let mut master_entries: Vec<(u64, String)> = Vec::new();
        for m in &master_order {
            let r = format!("rId{rid}");
            rid += 1;
            pres_rels.push(Relationship {
                id: r.clone(),
                rel_type: rel_type::SLIDE_MASTER.into(),
                target: relative_target(pres, m),
                external: false,
            });
            let big = self.next_big_id;
            self.next_big_id += 1;
            master_entries.push((big, r));
        }

        let slides_out = self.slides_out.clone();
        let mut slide_entries: Vec<(u64, String)> = Vec::new();
        for (i, s) in slides_out.iter().enumerate() {
            let r = format!("rId{rid}");
            rid += 1;
            pres_rels.push(Relationship {
                id: r.clone(),
                rel_type: rel_type::SLIDE.into(),
                target: relative_target(pres, s),
                external: false,
            });
            slide_entries.push((256 + i as u64, r));
        }

        let notes_masters = self.notes_masters.clone();
        let mut nm_rids: Vec<String> = Vec::new();
        for nm in &notes_masters {
            let r = format!("rId{rid}");
            rid += 1;
            pres_rels.push(Relationship {
                id: r.clone(),
                rel_type: rel_type::NOTES_MASTER.into(),
                target: relative_target(pres, nm),
                external: false,
            });
            nm_rids.push(r);
        }

        // presProps / viewProps / tableStyles — referenced by relationship only,
        // no element inside <p:presentation>.
        for (rel_ty, part) in extra_rels {
            let r = format!("rId{rid}");
            rid += 1;
            pres_rels.push(Relationship {
                id: r,
                rel_type: rel_ty,
                target: relative_target(pres, &part),
                external: false,
            });
        }

        let master_lst: String = master_entries
            .iter()
            .map(|(id, r)| format!(r#"<p:sldMasterId id="{id}" r:id="{r}"/>"#))
            .collect();
        let slide_lst: String = slide_entries
            .iter()
            .map(|(id, r)| format!(r#"<p:sldId id="{id}" r:id="{r}"/>"#))
            .collect();
        let notes_master_lst = if nm_rids.is_empty() {
            String::new()
        } else {
            let inner: String = nm_rids
                .iter()
                .map(|r| format!(r#"<p:notesMasterId r:id="{r}"/>"#))
                .collect();
            format!("<p:notesMasterIdLst>{inner}</p:notesMasterIdLst>")
        };

        // Child order follows CT_Presentation: sldMasterIdLst, notesMasterIdLst,
        // sldIdLst, sldSz, notesSz, defaultTextStyle.
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<p:presentation {NS_DECL}><p:sldMasterIdLst>{master_lst}</p:sldMasterIdLst>{notes_master_lst}<p:sldIdLst>{slide_lst}</p:sldIdLst><p:sldSz cx="{slide_w}" cy="{slide_h}"/>{notes_sz}{default_text_style}</p:presentation>"#
        );

        self.out_pkg.insert_part(pres, xml.into_bytes());
        self.out_pkg.set_rels(pres, &pres_rels);
        self.out_ct.set_override(pres, CT_PRESENTATION);
        Ok(())
    }

    fn build_core(&mut self, title: &str) {
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<cp:coreProperties xmlns:cp="http://schemas.openxmlformats.org/package/2006/metadata/core-properties" xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:dcterms="http://purl.org/dc/terms/" xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance"><dc:title>{title}</dc:title><dc:creator>Slideflow</dc:creator><dcterms:created xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:created><dcterms:modified xsi:type="dcterms:W3CDTF">2026-01-01T00:00:00Z</dcterms:modified></cp:coreProperties>"#,
            title = xml_escape(title)
        );
        self.out_pkg.insert_part("docProps/core.xml", xml.into_bytes());
        self.out_ct.set_override("docProps/core.xml", CT_CORE);
    }

    /// Minimal `docProps/app.xml` — PowerPoint always writes one; some
    /// consumers get confused without it.
    fn build_app(&mut self) {
        let slides = self.slides_out.len();
        let xml = format!(
            r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Properties xmlns="http://schemas.openxmlformats.org/officeDocument/2006/extended-properties" xmlns:vt="http://schemas.openxmlformats.org/officeDocument/2006/docPropsVTypes"><Application>Slideflow</Application><Slides>{slides}</Slides><ScaleCrop>false</ScaleCrop><LinksUpToDate>false</LinksUpToDate><SharedDoc>false</SharedDoc><HyperlinksChanged>false</HyperlinksChanged><AppVersion>16.0000</AppVersion></Properties>"#
        );
        self.out_pkg.insert_part("docProps/app.xml", xml.into_bytes());
        self.out_ct.set_override("docProps/app.xml", CT_APP);
    }

    fn build_root_rels(&mut self) {
        let rels = vec![
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
            Relationship {
                id: "rId3".into(),
                rel_type: rel_type::EXTENDED_PROPS.into(),
                target: "docProps/app.xml".into(),
                external: false,
            },
        ];
        self.out_pkg.set_rels("", &rels);
    }

    fn finish_content_types(&mut self) {
        self.out_ct.ensure_default("rels", CT_RELS);
        self.out_ct.ensure_default("xml", "application/xml");
        let ct = self.out_ct.clone();
        self.out_pkg.set_content_types(&ct);
    }
}

fn rewritten_rel(orig: &Relationship, from_part: &str, to_part: &str) -> Relationship {
    Relationship {
        id: orig.id.clone(),
        rel_type: orig.rel_type.clone(),
        target: relative_target(from_part, to_part),
        external: false,
    }
}

fn max_rid_num(rels: &[Relationship]) -> u32 {
    rels.iter()
        .filter_map(|r| r.id.strip_prefix("rId").and_then(|n| n.parse::<u32>().ok()))
        .max()
        .unwrap_or(0)
}

/// Compute a relationship `Target` (relative path) from a source part to a
/// destination part, both given as normalized package names.
fn relative_target(from_part: &str, to_part: &str) -> String {
    let from: Vec<&str> = from_part.split('/').collect();
    let to: Vec<&str> = to_part.split('/').collect();
    let from_dir = &from[..from.len().saturating_sub(1)];
    let mut common = 0;
    while common < from_dir.len() && common + 1 < to.len() && from_dir[common] == to[common] {
        common += 1;
    }
    let ups = from_dir.len() - common;
    let mut s = String::new();
    for _ in 0..ups {
        s.push_str("../");
    }
    s.push_str(&to[common..].join("/"));
    s
}

/// Rewrite a slide master's `<p:sldLayoutIdLst>` to reference exactly `entries`
/// (fresh `id`/`r:id` pairs). Inserts the element before `</p:sldMaster>` if the
/// source master lacked one.
fn rewrite_master_layout_list(
    xml: &[u8],
    entries: &[(u64, String)],
    part: &str,
) -> Result<Vec<u8>> {
    let mut reader = Reader::from_reader(xml);
    let mut writer = Writer::new(Cursor::new(Vec::new()));
    let mut buf = Vec::new();
    let mut in_list = false;
    let mut wrote = false;

    let write_list = |writer: &mut Writer<Cursor<Vec<u8>>>| -> Result<()> {
        writer
            .write_event(Event::Start(BytesStart::new("p:sldLayoutIdLst")))
            .map_err(|e| Error::xml(part, e))?;
        for (id, rid) in entries {
            let mut e = BytesStart::new("p:sldLayoutId");
            let id = id.to_string();
            e.push_attribute(("id", id.as_str()));
            e.push_attribute(("r:id", rid.as_str()));
            writer.write_event(Event::Empty(e)).map_err(|e| Error::xml(part, e))?;
        }
        writer
            .write_event(Event::End(BytesEnd::new("p:sldLayoutIdLst")))
            .map_err(|e| Error::xml(part, e))?;
        Ok(())
    };

    loop {
        let ev = reader.read_event_into(&mut buf).map_err(|e| Error::xml(part, e))?;
        match &ev {
            Event::Start(e) if local_name(e.name().as_ref()) == b"sldLayoutIdLst" => {
                write_list(&mut writer)?;
                wrote = true;
                in_list = true;
            }
            Event::Empty(e) if local_name(e.name().as_ref()) == b"sldLayoutIdLst" => {
                // Self-closing source list: replace it in place.
                write_list(&mut writer)?;
                wrote = true;
            }
            Event::End(e) if local_name(e.name().as_ref()) == b"sldLayoutIdLst" => {
                in_list = false;
            }
            Event::End(e) if local_name(e.name().as_ref()) == b"sldMaster" && !wrote => {
                write_list(&mut writer)?;
                wrote = true;
                writer.write_event(Event::End(e.clone())).map_err(|e| Error::xml(part, e))?;
            }
            Event::Eof => break,
            _ if in_list => {}
            _ => {
                writer.write_event(ev.clone()).map_err(|e| Error::xml(part, e))?;
            }
        }
        buf.clear();
    }
    Ok(writer.into_inner().into_inner())
}

/// The raw bytes of a deck's `ppt/presentation.xml`, as a string.
fn presentation_xml(pf: &PresentationFile) -> Option<String> {
    let main = pf.package.main_document_part().ok()?;
    let bytes = pf.package.part(&main)?;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

/// Extract one raw XML element (open tag through matching close tag) by its
/// qualified name. Handles the self-closing form. Assumes the element does not
/// nest within itself — true for every element this module extracts
/// (`p:defaultTextStyle`, `a:tblStyle`).
fn extract_element_raw(xml: &str, qname: &str) -> Option<String> {
    let open = format!("<{qname}");
    let mut from = 0;
    while let Some(rel) = xml[from..].find(&open) {
        let start = from + rel;
        // The character after the name must terminate it (attr space, `>`,
        // `/>`) — otherwise this is a longer name sharing the prefix (e.g.
        // `<a:tblStyleLst` when looking for `a:tblStyle`); keep scanning.
        let after = xml[start + open.len()..].chars().next()?;
        if !matches!(after, ' ' | '>' | '/' | '\t' | '\r' | '\n') {
            from = start + open.len();
            continue;
        }
        let tag_end = start + xml[start..].find('>')?;
        if xml[..tag_end].ends_with('/') {
            return Some(xml[start..=tag_end].to_string());
        }
        let close = format!("</{qname}>");
        let end = start + xml[start..].find(&close)? + close.len();
        return Some(xml[start..end].to_string());
    }
    None
}

/// All raw occurrences of an element (see [`extract_element_raw`]).
fn extract_all_elements_raw(xml: &str, qname: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(el) = extract_element_raw(rest, qname) {
        let pos = rest.find(&el).unwrap_or(0) + el.len();
        out.push(el);
        rest = &rest[pos..];
    }
    out
}

/// The value of `attr` on the first `<...{element_local} ...>` open tag.
fn attr_value(xml: &str, element_local: &str, attr: &str) -> Option<String> {
    let mut reader = Reader::from_reader(xml.as_bytes());
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if local_name(e.name().as_ref()) == element_local.as_bytes() =>
            {
                for a in e.attributes().flatten() {
                    if a.key.as_ref() == attr.as_bytes() {
                        return a.normalized_value(XmlVersion::Implicit1_0).ok().map(|v| v.into_owned());
                    }
                }
                return None;
            }
            Ok(Event::Eof) | Err(_) => return None,
            _ => {}
        }
        buf.clear();
    }
}

/// Extract the raw `<p:notesSz .../>` element from a deck's presentation part,
/// falling back to the standard portrait notes size.
fn parse_notes_sz(pf: &PresentationFile) -> String {
    const DEFAULT: &str = r#"<p:notesSz cx="6858000" cy="9144000"/>"#;
    let Ok(main) = pf.package.main_document_part() else { return DEFAULT.into() };
    let Some(xml) = pf.package.part(&main) else { return DEFAULT.into() };
    let mut reader = Reader::from_reader(xml);
    reader.config_mut().trim_text(true);
    let mut buf = Vec::new();
    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(ref e)) | Ok(Event::Empty(ref e))
                if local_name(e.name().as_ref()) == b"notesSz" =>
            {
                let mut cx = None;
                let mut cy = None;
                for attr in e.attributes().flatten() {
                    if let Ok(val) = attr.normalized_value(XmlVersion::Implicit1_0) {
                        match attr.key.as_ref() {
                            b"cx" => cx = Some(val.into_owned()),
                            b"cy" => cy = Some(val.into_owned()),
                            _ => {}
                        }
                    }
                }
                if let (Some(cx), Some(cy)) = (cx, cy) {
                    return format!(r#"<p:notesSz cx="{cx}" cy="{cy}"/>"#);
                }
                return DEFAULT.into();
            }
            Ok(Event::Eof) | Err(_) => break,
            _ => {}
        }
        buf.clear();
    }
    DEFAULT.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixtures::{DeckSpec, SlideSpec};
    use crate::opc::rels_part_name;
    use std::collections::HashSet;

    fn tmp(name: &str) -> std::path::PathBuf {
        let mut p = std::env::temp_dir();
        p.push(format!("slideflow_compose_{}_{}", std::process::id(), name));
        p
    }

    /// For every part in the package: its .rels targets all resolve to existing
    /// parts, and it has a content type. Also checks presentation r:ids resolve.
    fn assert_integrity(pkg: &Package) {
        let ct = pkg.content_types().unwrap();
        let names: HashSet<String> = pkg.part_names().map(|s| s.to_string()).collect();
        for name in &names {
            if name.ends_with(".rels") || name == "[Content_Types].xml" {
                continue;
            }
            assert!(ct.content_type_of(name).is_some(), "part {name} has no content type");
            for rel in pkg.rels_for(name).unwrap() {
                if rel.external {
                    continue;
                }
                let resolved = resolve_target(name, &rel.target);
                assert!(
                    names.contains(&resolved),
                    "part {name} rel {} -> {resolved} does not exist",
                    rel.id
                );
            }
        }
        assert!(names.contains("[Content_Types].xml"));
        assert!(names.contains("_rels/.rels"));

        // presentation.xml r:ids in sldIdLst/sldMasterIdLst/notesMasterIdLst exist in its rels.
        let main = pkg.main_document_part().unwrap();
        let pres = pkg.require_part(&main).unwrap();
        let rel_ids: HashSet<String> =
            pkg.rels_for(&main).unwrap().into_iter().map(|r| r.id).collect();
        let text = String::from_utf8_lossy(pres);
        for marker in ["sldId", "sldMasterId", "notesMasterId"] {
            for chunk in text.split(&format!("<p:{marker} ")).skip(1) {
                if let Some(idx) = chunk.find("r:id=\"") {
                    let rest = &chunk[idx + 6..];
                    if let Some(end) = rest.find('"') {
                        let rid = &rest[..end];
                        assert!(rel_ids.contains(rid), "presentation r:id {rid} missing in rels");
                    }
                }
            }
        }
    }

    fn count_family(pkg: &Package, dir: &str) -> usize {
        pkg.part_names()
            .filter(|n| n.starts_with(dir) && n.ends_with(".xml") && !n.contains("/_rels/"))
            .count()
    }

    #[test]
    fn round_trip_two_decks_preserves_style() {
        let deck_a = tmp("rt_a.pptx");
        let deck_b = tmp("rt_b.pptx");
        DeckSpec::new("Deck A")
            .accent("FF0000")
            .slide(SlideSpec::new("A-One").bullets(&["alpha", "beta"]))
            .slide(SlideSpec::new("A-Two").bullets(&["gamma"]).image().notes("secret note"))
            .write_to(&deck_a)
            .unwrap();
        DeckSpec::new("Deck B")
            .accent("00FF00")
            .slide(SlideSpec::new("B-One").bullets(&["delta"]))
            .write_to(&deck_b)
            .unwrap();

        let out = tmp("rt_out.pptx");
        let picks = vec![
            SlidePick { pptx_path: deck_a.to_string_lossy().into(), slide_index: 2 },
            SlidePick { pptx_path: deck_b.to_string_lossy().into(), slide_index: 1 },
            SlidePick { pptx_path: deck_a.to_string_lossy().into(), slide_index: 1 },
        ];
        let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();
        assert_eq!(report.slides_written, 3);
        assert_eq!(report.source_decks, 2);

        let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_integrity(&pkg);

        let pf = PresentationFile::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(pf.slide_count(), 3);

        // Order matches picks: A-Two, B-One, A-One.
        assert_eq!(pf.slide_content(1).unwrap().title.as_deref(), Some("A-Two"));
        assert_eq!(pf.slide_content(2).unwrap().title.as_deref(), Some("B-One"));
        assert_eq!(pf.slide_content(3).unwrap().title.as_deref(), Some("A-One"));
        assert!(pf.slide_content(1).unwrap().texts.iter().any(|t| t.contains("gamma")));
        assert!(pf.slide_content(2).unwrap().texts.iter().any(|t| t.contains("delta")));

        // Media carried for the image slide.
        assert!(pkg.part_names().any(|n| n.starts_with("ppt/media/")));

        // Full chain resolves for every output slide, and each slide's theme
        // carries its own source accent.
        for (idx, accent) in [(1usize, "FF0000"), (2, "00FF00"), (3, "FF0000")] {
            let slide = pf.slide_part(idx).unwrap().to_string();
            let layout = pf.layout_of_slide(&slide).unwrap().expect("layout");
            let master = pf.master_of_layout(&layout).unwrap().expect("master");
            let theme = pf.theme_of_master(&master).unwrap().expect("theme");
            let theme_xml = String::from_utf8_lossy(pkg.part(&theme).unwrap());
            assert!(
                theme_xml.contains(&format!(r#"<a:srgbClr val="{accent}"/></a:accent1>"#)),
                "slide {idx} theme {theme} missing accent {accent}"
            );
        }

        // Both distinct themes present.
        let themes: Vec<String> = pkg
            .part_names()
            .filter(|n| n.starts_with("ppt/theme/"))
            .map(|s| s.to_string())
            .collect();
        assert_eq!(themes.len(), 2, "expected two themes, got {themes:?}");

        // Notes dropped by default.
        assert!(!pkg.part_names().any(|n| n.starts_with("ppt/notesSlides/")));

        let _ = std::fs::remove_file(&deck_a);
        let _ = std::fs::remove_file(&deck_b);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn dedupes_master_and_theme_within_deck() {
        let deck = tmp("dedup.pptx");
        DeckSpec::new("Shared")
            .slide(SlideSpec::new("One").bullets(&["a"]))
            .slide(SlideSpec::new("Two").bullets(&["b"]))
            .write_to(&deck)
            .unwrap();
        let out = tmp("dedup_out.pptx");
        let picks = vec![
            SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 },
            SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 2 },
        ];
        compose(&picks, &out, &ComposeOptions::default()).unwrap();
        let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_integrity(&pkg);
        assert_eq!(count_family(&pkg, "ppt/slideMasters/"), 1);
        assert_eq!(count_family(&pkg, "ppt/theme/"), 1);
        assert_eq!(count_family(&pkg, "ppt/slideLayouts/"), 1);
        assert_eq!(count_family(&pkg, "ppt/slides/"), 2);
        let _ = std::fs::remove_file(&deck);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn same_slide_picked_twice_duplicates() {
        let deck = tmp("dup.pptx");
        DeckSpec::new("D").slide(SlideSpec::new("Only").bullets(&["x"])).write_to(&deck).unwrap();
        let out = tmp("dup_out.pptx");
        let p = SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 };
        let picks = vec![p.clone(), p];
        compose(&picks, &out, &ComposeOptions::default()).unwrap();
        let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_integrity(&pkg);
        assert_eq!(count_family(&pkg, "ppt/slides/"), 2);
        assert_eq!(count_family(&pkg, "ppt/slideMasters/"), 1);
        let _ = std::fs::remove_file(&deck);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn notes_included_when_requested() {
        let deck = tmp("notes.pptx");
        DeckSpec::new("N")
            .slide(SlideSpec::new("Talk").bullets(&["point"]).notes("remember the joke"))
            .write_to(&deck)
            .unwrap();
        let out = tmp("notes_out.pptx");
        let picks = vec![SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 }];
        let opts =
            ComposeOptions { title: "With Notes".into(), include_notes: true, fit_mode: None };
        compose(&picks, &out, &opts).unwrap();
        let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_integrity(&pkg);
        assert!(pkg.part_names().any(|n| n.starts_with("ppt/notesSlides/")));
        assert!(pkg.part_names().any(|n| n.starts_with("ppt/notesMasters/")));

        let pf = PresentationFile::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(pf.slide_content(1).unwrap().notes.as_deref(), Some("remember the joke"));
        let _ = std::fs::remove_file(&deck);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn missing_rel_target_is_skipped_not_fatal() {
        // Build a deck, then corrupt a slide's rels to point at a missing image.
        let deck = tmp("missing.pptx");
        DeckSpec::new("M").slide(SlideSpec::new("S").bullets(&["y"])).write_to(&deck).unwrap();
        let mut pkg = Package::open(&deck).unwrap();
        let slide = "ppt/slides/slide1.xml";
        let mut rels = pkg.rels_for(slide).unwrap();
        rels.push(Relationship {
            id: "rId9".into(),
            rel_type: rel_type::IMAGE.into(),
            target: "../media/ghost.png".into(),
            external: false,
        });
        pkg.set_rels(slide, &rels);
        pkg.save(&deck).unwrap();

        let out = tmp("missing_out.pptx");
        let picks = vec![SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 }];
        let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();
        assert!(report.warnings.iter().any(|w| w.contains("ghost.png")));
        let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_integrity(&pkg);
        let _ = std::fs::remove_file(&deck);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn external_hyperlink_preserved_verbatim() {
        let deck = tmp("hyper.pptx");
        DeckSpec::new("H").slide(SlideSpec::new("S").bullets(&["link"])).write_to(&deck).unwrap();
        let mut pkg = Package::open(&deck).unwrap();
        let slide = "ppt/slides/slide1.xml";
        let mut rels = pkg.rels_for(slide).unwrap();
        rels.push(Relationship {
            id: "rId5".into(),
            rel_type: "http://schemas.openxmlformats.org/officeDocument/2006/relationships/hyperlink".into(),
            target: "https://example.com/page".into(),
            external: true,
        });
        pkg.set_rels(slide, &rels);
        pkg.save(&deck).unwrap();

        let out = tmp("hyper_out.pptx");
        let picks = vec![SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 }];
        compose(&picks, &out, &ComposeOptions::default()).unwrap();
        let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_integrity(&pkg);
        let out_slide = "ppt/slides/slide1.xml";
        let out_rels = pkg.rels_for(out_slide).unwrap();
        let hyper = out_rels.iter().find(|r| r.external).expect("external rel preserved");
        assert_eq!(hyper.target, "https://example.com/page");
        assert!(pkg.has_part(&rels_part_name(out_slide)));
        let _ = std::fs::remove_file(&deck);
        let _ = std::fs::remove_file(&out);
    }

    /// Give a fixture deck presentation-level style parts the way PowerPoint /
    /// PptxGenJS write them: a defaultTextStyle in presentation.xml plus a
    /// tableStyles part referenced from its rels.
    fn add_presentation_style_parts(path: &std::path::Path, style_id: &str) {
        let mut pkg = Package::open(path).unwrap();
        let pres = String::from_utf8(pkg.part("ppt/presentation.xml").unwrap().to_vec()).unwrap();
        let styled = pres.replace(
            "</p:presentation>",
            r#"<p:defaultTextStyle><a:lvl1pPr><a:defRPr sz="1800"/></a:lvl1pPr></p:defaultTextStyle></p:presentation>"#,
        );
        pkg.insert_part("ppt/presentation.xml", styled.into_bytes());
        pkg.insert_part(
            "ppt/tableStyles.xml",
            format!(
                r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<a:tblStyleLst xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" def="{{{style_id}}}"><a:tblStyle styleId="{{{style_id}}}" styleName="Fixture Style"><a:wholeTbl><a:tcStyle><a:fill><a:solidFill><a:srgbClr val="EEEEEE"/></a:solidFill></a:fill></a:tcStyle></a:wholeTbl></a:tblStyle></a:tblStyleLst>"#
            )
            .into_bytes(),
        );
        let mut rels = pkg.rels_for("ppt/presentation.xml").unwrap();
        rels.push(Relationship {
            id: "rId90".into(),
            rel_type: rel_type::TABLE_STYLES.into(),
            target: "tableStyles.xml".into(),
            external: false,
        });
        pkg.set_rels("ppt/presentation.xml", &rels);
        let mut ct = pkg.content_types().unwrap();
        ct.set_override("ppt/tableStyles.xml", CT_TABLE_STYLES);
        pkg.set_content_types(&ct);
        pkg.save(path).unwrap();
    }

    #[test]
    fn carries_presentation_level_style_parts() {
        let deck_a = tmp("style_a.pptx");
        let deck_b = tmp("style_b.pptx");
        DeckSpec::new("A").slide(SlideSpec::new("S1").bullets(&["x"])).write_to(&deck_a).unwrap();
        DeckSpec::new("B").slide(SlideSpec::new("S2").bullets(&["y"])).write_to(&deck_b).unwrap();
        add_presentation_style_parts(&deck_a, "5C22544A-7EE6-4342-B048-85BDC9FD1C3A");
        add_presentation_style_parts(&deck_b, "21E4AEA4-8DFA-4A89-87EB-49C32662AFE0");

        let out = tmp("style_out.pptx");
        let picks = vec![
            SlidePick { pptx_path: deck_a.to_string_lossy().into(), slide_index: 1 },
            SlidePick { pptx_path: deck_b.to_string_lossy().into(), slide_index: 1 },
        ];
        compose(&picks, &out, &ComposeOptions::default()).unwrap();
        let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
        assert_integrity(&pkg);

        // app.xml is always generated and wired into the root rels.
        assert!(pkg.has_part("docProps/app.xml"));
        assert!(pkg
            .rels_for("")
            .unwrap()
            .iter()
            .any(|r| r.rel_type == rel_type::EXTENDED_PROPS));

        // defaultTextStyle carried from the first deck.
        let pres = String::from_utf8_lossy(pkg.part("ppt/presentation.xml").unwrap()).into_owned();
        assert!(pres.contains("<p:defaultTextStyle>"), "defaultTextStyle missing");

        // tableStyles merged across BOTH decks, referenced from presentation rels.
        let ts = String::from_utf8_lossy(pkg.part("ppt/tableStyles.xml").unwrap()).into_owned();
        assert!(ts.contains("5C22544A-7EE6-4342-B048-85BDC9FD1C3A"));
        assert!(ts.contains("21E4AEA4-8DFA-4A89-87EB-49C32662AFE0"));
        assert!(pkg
            .rels_for("ppt/presentation.xml")
            .unwrap()
            .iter()
            .any(|r| r.rel_type == rel_type::TABLE_STYLES));

        let _ = std::fs::remove_file(&deck_a);
        let _ = std::fs::remove_file(&deck_b);
        let _ = std::fs::remove_file(&out);
    }

    #[test]
    fn error_paths() {
        // Empty picks.
        let out = tmp("err_out.pptx");
        assert!(matches!(
            compose(&[], &out, &ComposeOptions::default()),
            Err(Error::Compose(_))
        ));

        // Nonexistent source path.
        let picks = vec![SlidePick { pptx_path: "/no/such/deck.pptx".into(), slide_index: 1 }];
        assert!(compose(&picks, &out, &ComposeOptions::default()).is_err());

        // Slide index out of range.
        let deck = tmp("err_range.pptx");
        DeckSpec::new("R").slide(SlideSpec::new("Only")).write_to(&deck).unwrap();
        let picks = vec![SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 5 }];
        assert!(matches!(
            compose(&picks, &out, &ComposeOptions::default()),
            Err(Error::SlideOutOfRange { .. })
        ));
        let _ = std::fs::remove_file(&deck);
    }
}
