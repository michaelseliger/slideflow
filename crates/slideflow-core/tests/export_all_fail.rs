//! Regression: an export where EVERY pick fails must error, not report success.
//!
//! `export_pdf` already errors when no page could be rendered; `export_pngs`
//! historically returned `Ok` with an empty `files_written` in the same
//! situation, so a totally-failed PNG export surfaced as success in the UI
//! (done state, "0 PNG images", a bogus history row). Both batch exporters must
//! now agree: all-picks-fail is an error.

use slideflow_core::export::{export_pdf, export_pngs, fontdb, PdfOptions, PngOptions};
use slideflow_core::model::SlidePick;

/// A font database with just the vendored fixture face (mirrors tests/export.rs).
fn fixture_fonts() -> fontdb::Database {
    static FONT: &[u8] =
        include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/fonts/DejaVuSans.ttf"));
    let mut db = fontdb::Database::new();
    db.load_font_data(FONT.to_vec());
    db.set_sans_serif_family("DejaVu Sans");
    db.set_serif_family("DejaVu Sans");
    db.set_monospace_family("DejaVu Sans");
    db
}

/// Two picks, both pointing at decks that do not exist on disk.
fn all_missing_picks(dir: &std::path::Path) -> Vec<SlidePick> {
    vec![
        SlidePick {
            pptx_path: dir.join("missing-a.pptx").to_string_lossy().into(),
            slide_index: 1,
        },
        SlidePick {
            pptx_path: dir.join("missing-b.pptx").to_string_lossy().into(),
            slide_index: 1,
        },
    ]
}

#[test]
fn png_export_errors_when_all_picks_fail() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out");
    let fonts = fixture_fonts();
    let picks = all_missing_picks(tmp.path());

    let result = export_pngs(&picks, &out, &PngOptions::default(), &fonts, &mut |_, _| {});
    assert!(result.is_err(), "all picks failed → must be Err, got {result:?}");
    assert!(
        format!("{}", result.unwrap_err()).contains("PNG"),
        "error mentions the PNG export path"
    );
}

#[test]
fn pdf_export_errors_when_all_picks_fail() {
    // Symmetry check: the PDF path already guards; keep the two exporters aligned.
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("out.pdf");
    let fonts = fixture_fonts();
    let picks = all_missing_picks(tmp.path());

    let result = export_pdf(&picks, &out, &PdfOptions::default(), &fonts, &mut |_, _| {});
    assert!(result.is_err(), "all picks failed → must be Err, got {result:?}");
}
