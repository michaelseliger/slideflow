//! Integration tests for PNG / PDF export of picked slides (WS-D).
//!
//! Every test injects a font database holding ONLY the bundled DejaVu Sans face
//! (never the system fonts), so rendering is deterministic and CI needs no
//! installed fonts.

use std::path::PathBuf;

use slideflow_core::export::{
    export_pdf, export_pngs, fontdb, render_slide_png, PdfOptions, PngOptions,
};
use slideflow_core::fixtures::{DeckSpec, SlideSpec};
use slideflow_core::model::SlidePick;

/// A font database with just the vendored fixture face. Generic families all
/// map to it so the renderer's `…, Helvetica, Arial, sans-serif` fallbacks
/// resolve to real glyphs.
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

/// Path to a committed corpus deck under `examples/pptx/`.
fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/pptx").join(name)
}

/// A small deck written to `dir/<name>.pptx`; returns its path. When
/// `with_image` is set, slide 1 carries a picture. (The bundled `TINY_PNG`
/// fixture has a deliberately-minimal — and CRC-incorrect — IDAT chunk, which
/// resvg tolerates but krilla's strict PNG decoder rejects, so PDF tests keep
/// `with_image` off and exercise images through the real-image corpus instead.)
fn write_deck(dir: &std::path::Path, name: &str, slides: usize, with_image: bool) -> PathBuf {
    let mut spec = DeckSpec::new(format!("{name} title")).author("Tester");
    for i in 1..=slides {
        let mut s = SlideSpec::new(format!("Slide {i} of {name}"))
            .bullets(&["First point", "Second point", "Third point"]);
        if with_image && i == 1 {
            s = s.image();
        }
        spec = spec.slide(s);
    }
    let path = dir.join(format!("{name}.pptx"));
    spec.write_to(&path).unwrap();
    path
}

/// Decode a PNG and return (width, height, distinct_rgba_count capped at 3).
fn png_stats(bytes: &[u8]) -> (u32, u32, usize) {
    let img = image::load_from_memory(bytes).expect("decode PNG").to_rgba8();
    let (w, h) = (img.width(), img.height());
    let mut seen = std::collections::HashSet::new();
    for px in img.pixels() {
        seen.insert(px.0);
        if seen.len() >= 3 {
            break;
        }
    }
    (w, h, seen.len())
}

#[test]
fn png_export_writes_named_deduped_files_from_mixed_tray() {
    let tmp = tempfile::tempdir().unwrap();
    let deck_a = write_deck(tmp.path(), "alpha", 2, false);
    let deck_b = write_deck(tmp.path(), "beta", 2, false);
    let out = tmp.path().join("out");

    let picks = vec![
        SlidePick { pptx_path: deck_a.to_string_lossy().into(), slide_index: 1 },
        SlidePick { pptx_path: deck_b.to_string_lossy().into(), slide_index: 2 },
        SlidePick { pptx_path: deck_a.to_string_lossy().into(), slide_index: 2 },
    ];

    let fonts = fixture_fonts();
    let mut progress: Vec<(usize, usize)> = Vec::new();
    let report =
        export_pngs(&picks, &out, &PngOptions { target_width_px: 1280 }, &fonts, &mut |d, t| {
            progress.push((d, t))
        })
        .unwrap();

    assert_eq!(report.files_written.len(), 3, "one PNG per pick");
    assert!(report.warnings.is_empty(), "no warnings: {:?}", report.warnings);

    let names: Vec<String> = report
        .files_written
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert_eq!(
        names,
        vec![
            "001 — alpha — slide 1.png".to_string(),
            "002 — beta — slide 2.png".to_string(),
            "003 — alpha — slide 2.png".to_string(),
        ]
    );
    for p in &report.files_written {
        assert!(p.exists(), "file written to disk: {}", p.display());
    }
    // Progress ran to completion.
    assert_eq!(progress.last().copied(), Some((3, 3)));
}

#[test]
fn png_dimensions_follow_target_width_and_viewbox_aspect() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = write_deck(tmp.path(), "sized", 1, false);
    let fonts = fixture_fonts();

    // Default fixture decks are 16:9 (960×540 pt) → 1920×1080 at width 1920.
    let bytes = render_slide_png(&deck, 1, 1920, &fonts).unwrap();
    let (w, h, distinct) = png_stats(&bytes);
    assert_eq!(w, 1920, "exact target width");
    assert_eq!(h, 1080, "height follows 16:9 viewBox aspect");
    assert!(distinct >= 2, "rendered slide is not a single flat color");
}

#[test]
fn png_renders_image_gradient_and_table_non_blank() {
    let fonts = fixture_fonts();
    // A programmatic deck with a data-URI image, plus corpus decks that exercise
    // gradient fills (shapes.pptx) and a table (tables.pptx).
    let tmp = tempfile::tempdir().unwrap();
    let image_deck = write_deck(tmp.path(), "withimage", 1, true);

    let cases: [(PathBuf, usize); 3] = [
        (image_deck, 1),
        (corpus("shapes.pptx"), 1),
        (corpus("tables.pptx"), 1),
    ];
    for (deck, idx) in cases {
        assert!(deck.exists(), "fixture/corpus deck present: {}", deck.display());
        let bytes = render_slide_png(&deck, idx, 1280, &fonts)
            .unwrap_or_else(|e| panic!("render {} slide {idx}: {e}", deck.display()));
        let (w, _h, distinct) = png_stats(&bytes);
        assert_eq!(w, 1280);
        assert!(distinct >= 2, "{} slide {idx} rendered blank", deck.display());
    }
}

#[test]
fn png_missing_deck_warns_and_others_still_export() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = write_deck(tmp.path(), "good", 2, false);
    let out = tmp.path().join("out");
    let fonts = fixture_fonts();

    let picks = vec![
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 },
        SlidePick { pptx_path: tmp.path().join("nope.pptx").to_string_lossy().into(), slide_index: 1 },
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 2 },
    ];
    let report =
        export_pngs(&picks, &out, &PngOptions::default(), &fonts, &mut |_, _| {}).unwrap();

    assert_eq!(report.files_written.len(), 2, "the two good picks still export");
    assert_eq!(report.warnings.len(), 1, "one warning for the missing deck");
    assert!(report.warnings[0].contains("nope.pptx"));
}

#[test]
fn pdf_has_one_page_per_pick_with_mediabox_and_embedded_font() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = write_deck(tmp.path(), "talk", 3, false);
    let out = tmp.path().join("deck.pdf");
    let fonts = fixture_fonts();

    let picks = vec![
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 },
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 2 },
    ];
    let report = export_pdf(
        &picks,
        &out,
        &PdfOptions { title: Some("My Talk".into()) },
        &fonts,
        &mut |_, _| {},
    )
    .unwrap();

    assert_eq!(report.files_written, vec![out.clone()]);
    assert!(report.warnings.is_empty(), "warnings: {:?}", report.warnings);

    let doc = lopdf::Document::load(&out).expect("parse exported PDF");
    let pages = doc.get_pages();
    assert_eq!(pages.len(), 2, "one PDF page per pick");

    // First page MediaBox ≈ 960×540 pt (16:9).
    let (&_num, &page_id) = pages.iter().next().unwrap();
    let mbox = doc.get_object(page_id).unwrap().as_dict().unwrap().get(b"MediaBox");
    let mbox = mbox.expect("page has a MediaBox").as_array().unwrap();
    let dims: Vec<f32> = mbox.iter().map(|o| o.as_float().unwrap_or(0.0)).collect();
    assert!((dims[2] - 960.0).abs() < 1.0, "MediaBox width ≈ 960pt, got {}", dims[2]);
    assert!((dims[3] - 540.0).abs() < 1.0, "MediaBox height ≈ 540pt, got {}", dims[3]);

    // At least one embedded font (text was present on every slide).
    let font_dicts = doc
        .objects
        .values()
        .filter_map(|o| o.as_dict().ok())
        .filter(|d| matches!(d.get(b"Type").and_then(|t| t.as_name()), Ok(b"Font")))
        .count();
    assert!(font_dicts >= 1, "PDF embeds at least one /Font, found {font_dicts}");
}

#[test]
fn pdf_missing_deck_warns_and_others_still_export() {
    let tmp = tempfile::tempdir().unwrap();
    let deck = write_deck(tmp.path(), "good", 2, false);
    let out = tmp.path().join("out.pdf");
    let fonts = fixture_fonts();

    let picks = vec![
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 },
        SlidePick { pptx_path: "/no/such/deck.pptx".into(), slide_index: 1 },
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 2 },
    ];
    let report =
        export_pdf(&picks, &out, &PdfOptions::default(), &fonts, &mut |_, _| {}).unwrap();

    assert_eq!(report.files_written, vec![out.clone()]);
    assert_eq!(report.warnings.len(), 1);
    assert!(report.warnings[0].contains("deck.pptx"));

    let doc = lopdf::Document::load(&out).unwrap();
    assert_eq!(doc.get_pages().len(), 2, "the two good picks still produce pages");
}

#[test]
fn pdf_undecodable_image_degrades_to_warning_not_whole_pdf_abort() {
    // The bundled TINY_PNG fixture has a CRC-incorrect IDAT chunk that both the
    // strict `image` decoder and krilla reject. Because krilla decodes images
    // lazily at assembly time, an un-neutralized bad image would abort the whole
    // PDF; instead the export must blank it, warn, and still write every page.
    let tmp = tempfile::tempdir().unwrap();
    let deck = write_deck(tmp.path(), "withbadimg", 2, true); // slide 1 carries TINY_PNG
    let out = tmp.path().join("d.pdf");
    let fonts = fixture_fonts();

    let picks = vec![
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 1 },
        SlidePick { pptx_path: deck.to_string_lossy().into(), slide_index: 2 },
    ];
    let report =
        export_pdf(&picks, &out, &PdfOptions::default(), &fonts, &mut |_, _| {}).unwrap();

    assert_eq!(report.files_written, vec![out.clone()]);
    assert_eq!(report.warnings.len(), 1, "one warning for the blanked image");
    assert!(
        report.warnings[0].contains("could not be embedded"),
        "warning: {:?}",
        report.warnings
    );
    let doc = lopdf::Document::load(&out).unwrap();
    assert_eq!(doc.get_pages().len(), 2, "both slides still produce pages");
}

#[test]
fn empty_picks_are_rejected() {
    let tmp = tempfile::tempdir().unwrap();
    let fonts = fixture_fonts();
    assert!(export_pngs(&[], &tmp.path().join("o"), &PngOptions::default(), &fonts, &mut |_, _| {})
        .is_err());
    assert!(export_pdf(&[], &tmp.path().join("o.pdf"), &PdfOptions::default(), &fonts, &mut |_, _| {})
        .is_err());
}
