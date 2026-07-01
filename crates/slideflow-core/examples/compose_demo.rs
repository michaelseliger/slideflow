use slideflow_core::model::SlidePick;
use slideflow_core::pptx::{compose, ComposeOptions};
fn main() {
    let dir = std::env::args().nth(1).expect("corpus dir");
    let out = std::env::args().nth(2).expect("output path");
    let pick = |f: &str, i: usize| SlidePick { pptx_path: format!("{dir}/{f}"), slide_index: i };
    let picks = vec![
        pick("large_sales_review.pptx", 1),
        pick("test.pptx", 1),                 // real PowerPoint 2007+ file, 4:3
        pick("strategy_offsite.pptx", 3),
        pick("large_sales_review.pptx", 150),
        pick("product_roadmap.pptx", 2),
        pick("strategy_offsite.pptx", 50),
    ];
    let report = compose(&picks, std::path::Path::new(&out), &ComposeOptions {
        title: "Mixed Highlight Deck".into(), include_notes: true,
    }).expect("compose");
    println!("slides={} decks={} warnings={:?}", report.slides_written, report.source_decks, report.warnings);
}
