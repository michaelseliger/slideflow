//! PresentationML: parsing decks and composing new ones.

pub mod composer;
pub mod parser;
pub mod scale;

pub use composer::{compose, ComposeOptions};
pub use parser::{CoreProps, PresentationFile, SlideContent};
pub use scale::{scale_part_xml, SlideScale};
