//! slideflow-core — the engine behind the Slideflow desktop app.
//!
//! Everything PPTX happens here, natively, with no external tools:
//!
//! - [`opc`]      — Open Packaging Conventions layer: zip parts, `[Content_Types].xml`,
//!                  relationship (`.rels`) parsing and writing.
//! - [`pptx`]     — presentation parsing (slide order, text, notes, metadata) and the
//!                  style-preserving composer that builds new decks from picked slides.
//! - [`render`]   — slide → SVG preview renderer (theme-aware, no LibreOffice).
//! - [`index`]    — SQLite + FTS5 library: scanning, incremental indexing, full-text
//!                  search with filters, and filesystem watching.
//! - [`model`]    — serde-serializable domain types shared with the desktop frontend.
//! - [`fixtures`] — programmatic minimal-but-valid PPTX builders for tests.

pub mod error;
pub mod fixtures;
pub mod index;
pub mod model;
pub mod opc;
pub mod pptx;
pub mod render;

pub use error::{Error, Result};
