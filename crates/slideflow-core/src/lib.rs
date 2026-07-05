//! slideflow-core — the engine behind the Slideflow desktop app.
//!
//! Everything PPTX happens here, natively, with no external tools:
//!
//! - [`opc`]      — Open Packaging Conventions layer: zip parts, `[Content_Types].xml`, relationship (`.rels`) parsing and writing.
//! - [`pptx`]     — presentation parsing (slide order, text, notes, metadata) and the style-preserving composer that builds new decks from picked slides.
//! - [`render`]   — slide → SVG preview renderer (theme-aware, no LibreOffice).
//! - [`export`]   — picked slides → PNG images / a PDF, via the SVG renderer.
//! - [`index`]    — SQLite + FTS5 library: scanning, incremental indexing, full-text search with filters, and filesystem watching.
//! - [`embed`]    — local semantic search: embedder trait, in-memory vector store, hybrid fusion, duplicate clustering (real model behind the `embeddings` feature).
//! - [`fonts`]    — bundled metric-compatible substitutes (Carlito↔Calibri, Caladea↔Cambria) and named CSS fallback chains for unembedded Office fonts.
//! - [`hash`]     — content/text hashing for duplicate detection and embedding keys.
//! - [`thumbs`]   — content-addressed cache keys for the on-disk slide-preview cache.
//! - [`dragout`]  — content-addressed cache keys for the desktop "drag a slide out" scratch files.
//! - [`model`]    — serde-serializable domain types shared with the desktop frontend.
//! - [`fixtures`] — programmatic minimal-but-valid PPTX builders for tests.

pub mod dragout;
pub mod embed;
pub mod error;
pub mod export;
pub mod fixtures;
pub mod fonts;
pub mod hash;
pub mod index;
pub mod model;
pub mod opc;
pub mod pptx;
pub mod render;
pub mod thumbs;

pub use error::{Error, Result};
