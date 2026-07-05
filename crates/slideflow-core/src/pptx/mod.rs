//! PresentationML: parsing decks and composing new ones.

pub mod composer;
pub mod embedded_fonts;
pub mod parser;
pub mod scale;

pub use composer::{compose, ComposeOptions};
pub use embedded_fonts::{embedded_font_set, embedded_fonts, EmbeddedFont, EmbeddedFontSet, SkippedFont};
pub use parser::{CoreProps, PresentationFile, SlideContent};
pub use scale::{scale_part_xml, SlideScale};
