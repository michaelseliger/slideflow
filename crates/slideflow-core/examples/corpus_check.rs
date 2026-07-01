use slideflow_core::pptx::PresentationFile;
fn main() {
    let dir = std::env::args().nth(1).expect("dir");
    for entry in std::fs::read_dir(&dir).unwrap() {
        let path = entry.unwrap().path();
        if path.extension().and_then(|e| e.to_str()) != Some("pptx") { continue; }
        let start = std::time::Instant::now();
        match PresentationFile::open(&path) {
            Ok(pf) => {
                let mut chars = 0usize;
                let mut titled = 0usize;
                let mut notes = 0usize;
                for i in 1..=pf.slide_count() {
                    let c = pf.slide_content(i).expect("content");
                    chars += c.texts.iter().map(|t| t.len()).sum::<usize>();
                    titled += c.title.is_some() as usize;
                    notes += c.notes.is_some() as usize;
                }
                println!("OK  {:28} slides={:4} titled={:4} notes={:3} chars={:6} size={}x{} in {:?}",
                    path.file_name().unwrap().to_string_lossy(), pf.slide_count(), titled, notes,
                    chars, pf.slide_width_emu, pf.slide_height_emu, start.elapsed());
            }
            Err(e) => println!("ERR {:28} {e}", path.file_name().unwrap().to_string_lossy()),
        }
    }
}
