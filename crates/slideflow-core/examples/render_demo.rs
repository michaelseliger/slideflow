use slideflow_core::pptx::PresentationFile;
use slideflow_core::render::{render_slide_svg, RenderOptions};
fn main() {
    let path = std::env::args().nth(1).expect("pptx");
    let idx: usize = std::env::args().nth(2).expect("slide").parse().unwrap();
    let out = std::env::args().nth(3).expect("out.svg");
    // Optional 4th arg picks the image tier: "thumb" (grid, 512px), "preview"
    // (peek modal, 1600px), or default (uncapped full resolution).
    let options = match std::env::args().nth(4).as_deref() {
        Some("thumb") => RenderOptions::thumb(),
        Some("preview") => RenderOptions::preview(),
        _ => RenderOptions::default(),
    };
    let pf = PresentationFile::open(std::path::Path::new(&path)).unwrap();
    let svg = render_slide_svg(&pf, idx, &options).unwrap();
    std::fs::write(&out, &svg).unwrap();
    eprintln!("{} bytes", svg.len());
}
