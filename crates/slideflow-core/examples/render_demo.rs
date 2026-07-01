use slideflow_core::pptx::PresentationFile;
use slideflow_core::render::{render_slide_svg, RenderOptions};
fn main() {
    let path = std::env::args().nth(1).expect("pptx");
    let idx: usize = std::env::args().nth(2).expect("slide").parse().unwrap();
    let out = std::env::args().nth(3).expect("out.svg");
    let pf = PresentationFile::open(std::path::Path::new(&path)).unwrap();
    let svg = render_slide_svg(&pf, idx, &RenderOptions::default()).unwrap();
    std::fs::write(&out, &svg).unwrap();
    eprintln!("{} bytes", svg.len());
}
