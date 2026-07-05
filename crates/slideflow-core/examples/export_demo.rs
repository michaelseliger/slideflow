//! Manual harness for the PNG / PDF slide export engine.
//!
//! ```text
//! export_demo pdf <out.pdf>  <deck.pptx>[:slide] [more decks…]
//! export_demo png <out_dir>  <deck.pptx>[:slide] [more decks…]
//! ```
//!
//! A deck without `:slide` expands to every slide of that deck, in order, so
//! `export_demo pdf /tmp/all.pdf examples/pptx/tables.pptx` exports the whole
//! deck. Uses the real system fonts (via `export::system_fonts`).

use std::path::{Path, PathBuf};

use slideflow_core::export::{export_pdf, export_pngs, system_fonts, PdfOptions, PngOptions};
use slideflow_core::model::SlidePick;
use slideflow_core::pptx::PresentationFile;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 {
        eprintln!("usage: export_demo <pdf|png> <out> <deck.pptx>[:slide] [more…]");
        std::process::exit(2);
    }
    let mode = args[1].as_str();
    let out = PathBuf::from(&args[2]);

    let mut picks: Vec<SlidePick> = Vec::new();
    for spec in &args[3..] {
        let (path, slide) = match spec.rsplit_once(':') {
            Some((p, n)) if n.parse::<usize>().is_ok() => (p.to_string(), Some(n.parse().unwrap())),
            _ => (spec.clone(), None),
        };
        match slide {
            Some(n) => picks.push(SlidePick { pptx_path: path, slide_index: n }),
            None => {
                let pf = PresentationFile::open(Path::new(&path))
                    .unwrap_or_else(|e| panic!("open {path}: {e}"));
                for i in 1..=pf.slide_count() {
                    picks.push(SlidePick { pptx_path: path.clone(), slide_index: i });
                }
            }
        }
    }

    let fonts = system_fonts();
    let mut last = (0usize, 0usize);
    let mut progress = |done: usize, total: usize| last = (done, total);

    let report = match mode {
        "pdf" => export_pdf(&picks, &out, &PdfOptions { title: Some("Slideflow Export".into()) }, &fonts, &mut progress),
        "png" => export_pngs(&picks, &out, &PngOptions { target_width_px: 1600 }, &fonts, &mut progress),
        other => {
            eprintln!("unknown mode {other:?}; expected pdf or png");
            std::process::exit(2);
        }
    }
    .unwrap_or_else(|e| panic!("export failed: {e}"));

    println!("processed {}/{} picks", last.0, last.1);
    println!("wrote {} file(s):", report.files_written.len());
    for f in &report.files_written {
        let bytes = std::fs::metadata(f).map(|m| m.len()).unwrap_or(0);
        println!("  {}  ({} KiB)", f.display(), bytes / 1024);
    }
    if report.warnings.is_empty() {
        println!("no warnings");
    } else {
        println!("{} warning(s):", report.warnings.len());
        for w in &report.warnings {
            println!("  - {w}");
        }
    }
}
