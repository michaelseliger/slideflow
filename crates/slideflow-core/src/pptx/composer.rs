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

use std::path::Path;

use crate::error::Result;
use crate::model::{ComposeReport, SlidePick};

#[derive(Debug, Clone)]
pub struct ComposeOptions {
    /// Title written to the output's docProps/core.xml.
    pub title: String,
    /// Carry speaker notes into the output (default: false).
    pub include_notes: bool,
}

impl Default for ComposeOptions {
    fn default() -> Self {
        ComposeOptions { title: "Slideflow Deck".into(), include_notes: false }
    }
}

/// Compose a new PPTX at `output_path` from `picks`, in order.
///
/// Source decks are opened at most once each, regardless of how many slides
/// are picked from them or in what interleaving.
pub fn compose(picks: &[SlidePick], output_path: &Path, options: &ComposeOptions) -> Result<ComposeReport> {
    let _ = (picks, output_path, options);
    todo!("implemented by the composer module owner")
}
