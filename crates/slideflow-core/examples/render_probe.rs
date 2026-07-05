//! Report the dropped-construct telemetry for specific slides — the diagnostic
//! behind the "approximate preview" badge. For each `deck.pptx:N` argument it
//! renders slide `N` with the preview options and prints
//! [`RenderOutcome::dropped`](slideflow_core::render::RenderOutcome), so a slide
//! that still falls back to a placeholder can be traced to the exact construct
//! (`unsupported-image`, `chart`, an unknown shape, a skipped font, …).
//!
//! ```text
//! cargo run --release -p slideflow-core --example render_probe -- deck.pptx:3 deck.pptx:8
//! ```
use slideflow_core::pptx::PresentationFile;
use slideflow_core::render::{render_slide, RenderOptions};

fn main() {
    for arg in std::env::args().skip(1) {
        let (path, idx) = arg.rsplit_once(':').expect("usage: deck.pptx:N");
        let idx: usize = idx.parse().expect("slide index");
        let name = path.rsplit('/').next().unwrap_or(path);
        match PresentationFile::open(std::path::Path::new(path)) {
            Ok(pf) => match render_slide(&pf, idx, &RenderOptions::preview()) {
                Ok(out) => println!("{name}:{idx} dropped={:?}", out.dropped),
                Err(e) => println!("{name}:{idx} ERROR {e}"),
            },
            Err(e) => println!("{name} OPEN ERROR {e}"),
        }
    }
}
