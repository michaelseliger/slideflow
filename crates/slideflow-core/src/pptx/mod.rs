//! PresentationML: parsing decks and composing new ones.

pub mod composer;
pub mod parser;

pub use composer::{compose, ComposeOptions};
pub use parser::{CoreProps, PresentationFile, SlideContent};
