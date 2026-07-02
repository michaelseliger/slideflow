//! Compose a new deck from picked slides.
//!
//! Usage: compose_demo out.pptx deck1.pptx:3 deck2.pptx:1 deck1.pptx:7 ...

use slideflow_core::model::SlidePick;
use slideflow_core::pptx::{compose, ComposeOptions};

fn main() {
    let mut args = std::env::args().skip(1);
    let out = args.next().expect("usage: compose_demo out.pptx deck.pptx:N ...");
    let picks: Vec<SlidePick> = args
        .map(|spec| {
            let (path, idx) = spec.rsplit_once(':').expect("expected deck.pptx:N");
            SlidePick { pptx_path: path.to_string(), slide_index: idx.parse().expect("slide index") }
        })
        .collect();
    let report = compose(&picks, std::path::Path::new(&out), &ComposeOptions::default())
        .expect("compose failed");
    println!(
        "wrote {} slides from {} decks to {} (warnings: {:?})",
        report.slides_written, report.source_decks, report.output_path, report.warnings
    );
}
