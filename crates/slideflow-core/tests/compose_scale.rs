//! Integration coverage for mixed-dimension composition (WS-C, roadmap #7):
//! decks whose slide size differs from the output canvas are rescaled, and
//! uniform trays stay byte-for-byte identical to their source.

use std::path::{Path, PathBuf};

use slideflow_core::fixtures::{DeckSpec, SlideSpec};
use slideflow_core::model::{FitMode, SlidePick};
use slideflow_core::opc::Package;
use slideflow_core::pptx::{compose, ComposeOptions};

// 16:9 widescreen (the default fixture canvas).
const W16_9: (i64, i64) = (12192000, 6858000);
// 16:9 but 3/4 the size → a same-aspect resize (4/3 scale-up on the big canvas).
const W16_9_SMALL: (i64, i64) = (9144000, 5143500);
// 4:3 standard → a genuine aspect mismatch against a 16:9 canvas.
const W4_3: (i64, i64) = (9144000, 6858000);

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("slideflow_scale_{}_{}", std::process::id(), name));
    p
}

fn pick(path: &Path, idx: usize) -> SlidePick {
    SlidePick { pptx_path: path.to_string_lossy().into_owned(), slide_index: idx }
}

/// A minimal text shape with an explicit top-level `a:off`, `a:ext` and font
/// size, so scaled geometry is observable in the output XML.
fn text_shape(id: u32, x: i64, y: i64, sz: i64, text: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{text}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="1200000" cy="600000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="{sz}"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
    )
}

/// The first part under `dir` (e.g. `ppt/slides/`) whose text contains `marker`.
fn part_containing(pkg: &Package, dir: &str, marker: &str) -> String {
    for name in pkg.part_names() {
        if name.starts_with(dir) && name.ends_with(".xml") && !name.contains("/_rels/") {
            let s = String::from_utf8_lossy(pkg.part(name).unwrap()).into_owned();
            if s.contains(marker) {
                return s;
            }
        }
    }
    panic!("no part under {dir} contains {marker}");
}

fn read_pkg(path: &Path) -> Package {
    Package::from_bytes(&std::fs::read(path).unwrap()).unwrap()
}

fn cleanup(paths: &[&Path]) {
    for p in paths {
        let _ = std::fs::remove_file(p);
    }
}

// (9) same-aspect auto-scale: 9144000×5143500 → 12192000×6858000 (×4/3). Scaled
// even with fit_mode None; a note is present and no size warning is emitted.
#[test]
fn same_aspect_auto_scales_without_fit_mode() {
    let dst = tmp("s9_dst.pptx");
    let src = tmp("s9_src.pptx");
    DeckSpec::new("Dst").slide(SlideSpec::new("D")).write_to(&dst).unwrap();
    DeckSpec::new("Src")
        .slide_size(W16_9_SMALL.0, W16_9_SMALL.1)
        .slide(SlideSpec::new("S").raw_shape(text_shape(9, 900_000, 600_000, 1800, "NineShape")))
        .write_to(&src)
        .unwrap();

    let out = tmp("s9_out.pptx");
    let picks = vec![pick(&dst, 1), pick(&src, 1)];
    let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();

    assert!(
        report.notes.iter().any(|n| n.contains("scaled to 133%")),
        "expected a 133% scale note, got {:?}",
        report.notes
    );
    assert!(
        !report.warnings.iter().any(|w| w.contains("different slide size")),
        "same-aspect resize must not warn: {:?}",
        report.warnings
    );

    let pkg = read_pkg(&out);
    let slide = part_containing(&pkg, "ppt/slides/", "NineShape");
    // 900000·4/3 = 1200000, 600000·4/3 = 800000, sz 1800·4/3 = 2400.
    assert!(slide.contains(r#"<a:off x="1200000" y="800000"/>"#), "geometry: {slide}");
    assert!(slide.contains(r#"sz="2400""#), "font size: {slide}");

    cleanup(&[&dst, &src, &out]);
}

// (10) 4:3 into a 16:9-first canvas, EnsureFit ⇒ s=1, pure horizontal letterbox
// (dx=1524000) applied to top-level offsets only — nested shapes get no dx.
#[test]
fn aspect_mismatch_ensurefit_letterboxes_top_level_only() {
    let dst = tmp("s10_dst.pptx"); // 16:9
    let src = tmp("s10_src.pptx"); // 4:3
    DeckSpec::new("Dst").slide(SlideSpec::new("D")).write_to(&dst).unwrap();
    let group = r#"<p:grpSp><p:nvGrpSpPr><p:cNvPr id="20" name="TenGroup"/><p:cNvGrpSpPr/><p:nvPr/></p:nvGrpSpPr><p:grpSpPr><a:xfrm><a:off x="900000" y="600000"/><a:ext cx="1000000" cy="1000000"/><a:chOff x="0" y="0"/><a:chExt cx="1000000" cy="1000000"/></a:xfrm></p:grpSpPr><p:sp><p:nvSpPr><p:cNvPr id="21" name="TenChild"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="100000" y="100000"/><a:ext cx="200000" cy="200000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1800"/><a:t>TenChild</a:t></a:r></a:p></p:txBody></p:sp></p:grpSp>"#;
    DeckSpec::new("Src")
        .slide_size(W4_3.0, W4_3.1)
        .slide(SlideSpec::new("S").raw_shape(group))
        .write_to(&src)
        .unwrap();

    let out = tmp("s10_out.pptx");
    let picks = vec![pick(&dst, 1), pick(&src, 1)];
    let opts = ComposeOptions { fit_mode: Some(FitMode::EnsureFit), ..Default::default() };
    let report = compose(&picks, &out, &opts).unwrap();

    assert!(
        report.notes.iter().any(|n| n.contains("scaled to 100%")),
        "expected a 100% (letterbox) note, got {:?}",
        report.notes
    );

    let pkg = read_pkg(&out);
    let slide = part_containing(&pkg, "ppt/slides/", "TenChild");
    // s=1: top-level group offset shifts by dx only; sz unchanged.
    assert!(slide.contains(r#"<a:off x="2424000" y="600000"/>"#), "group off: {slide}");
    // Nested child offset is untouched (no dx, s=1).
    assert!(slide.contains(r#"<a:off x="100000" y="100000"/>"#), "nested off: {slide}");
    assert!(slide.contains(r#"sz="1800""#), "sz unchanged at s=1: {slide}");
    // Group child-origin (chOff) is scaled only, never translated.
    assert!(slide.contains(r#"<a:chOff x="0" y="0"/>"#), "chOff: {slide}");

    cleanup(&[&dst, &src, &out]);
}

// (11) 16:9 into a 4:3-first canvas. EnsureFit ⇒ s=3/4 with vertical letterbox
// (dy=857250) and sz 1800→1350; Maximize ⇒ s=1 with horizontal overflow
// (dx=−1524000, an off-canvas offset).
#[test]
fn aspect_mismatch_ensurefit_and_maximize_are_inverse() {
    let dst = tmp("s11_dst.pptx"); // 4:3 canvas
    let src = tmp("s11_src.pptx"); // 16:9 source
    DeckSpec::new("Dst").slide_size(W4_3.0, W4_3.1).slide(SlideSpec::new("D")).write_to(&dst).unwrap();
    DeckSpec::new("Src")
        .slide_size(W16_9.0, W16_9.1)
        .slide(SlideSpec::new("S").raw_shape(text_shape(11, 900_000, 600_000, 1800, "ElevenShape")))
        .write_to(&src)
        .unwrap();
    let picks = vec![pick(&dst, 1), pick(&src, 1)];

    // EnsureFit: scale down to 3/4, letterbox vertically.
    let fit_out = tmp("s11_fit.pptx");
    let fit_opts = ComposeOptions { fit_mode: Some(FitMode::EnsureFit), ..Default::default() };
    compose(&picks, &fit_out, &fit_opts).unwrap();
    let fit_slide = part_containing(&read_pkg(&fit_out), "ppt/slides/", "ElevenShape");
    // 900000·3/4 = 675000 (+dx 0); 600000·3/4 = 450000, +dy 857250 = 1307250.
    assert!(fit_slide.contains(r#"<a:off x="675000" y="1307250"/>"#), "EnsureFit off: {fit_slide}");
    assert!(fit_slide.contains(r#"sz="1350""#), "EnsureFit sz: {fit_slide}");

    // Maximize: s=1, overflow horizontally (negative dx is valid off-canvas).
    let max_out = tmp("s11_max.pptx");
    let max_opts = ComposeOptions { fit_mode: Some(FitMode::Maximize), ..Default::default() };
    compose(&picks, &max_out, &max_opts).unwrap();
    let max_slide = part_containing(&read_pkg(&max_out), "ppt/slides/", "ElevenShape");
    // 900000 + dx(−1524000) = −624000; y unchanged; sz unchanged.
    assert!(max_slide.contains(r#"<a:off x="-624000" y="600000"/>"#), "Maximize off: {max_slide}");
    assert!(max_slide.contains(r#"sz="1800""#), "Maximize sz: {max_slide}");

    cleanup(&[&dst, &src, &fit_out, &max_out]);
}

// (12) byte-identity regression: a uniform multi-deck tray must not run any part
// through the scaler. Slide and layout bytes stay identical to the source; the
// master matches outside the composer's own sldLayoutIdLst rewrite.
#[test]
fn uniform_tray_is_byte_identical() {
    let a = tmp("s12_a.pptx");
    let b = tmp("s12_b.pptx");
    // Same canvas, different accents/content → distinct-but-uniform decks.
    DeckSpec::new("A").accent("FF0000").slide(SlideSpec::new("Aye").bullets(&["a"])).write_to(&a).unwrap();
    DeckSpec::new("B").accent("00FF00").slide(SlideSpec::new("Bee").bullets(&["b"])).write_to(&b).unwrap();

    let out = tmp("s12_out.pptx");
    let picks = vec![pick(&a, 1), pick(&b, 1)];
    let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();
    assert!(report.notes.is_empty(), "uniform tray must produce no scale notes: {:?}", report.notes);

    let src_a = read_pkg(&a);
    let src_b = read_pkg(&b);
    let out_pkg = read_pkg(&out);

    // Output slides carry each source's slide bytes verbatim.
    let out_slide_aye = part_containing(&out_pkg, "ppt/slides/", "Aye");
    let out_slide_bee = part_containing(&out_pkg, "ppt/slides/", "Bee");
    let src_slide_aye = String::from_utf8_lossy(src_a.part("ppt/slides/slide1.xml").unwrap()).into_owned();
    let src_slide_bee = String::from_utf8_lossy(src_b.part("ppt/slides/slide1.xml").unwrap()).into_owned();
    assert_eq!(out_slide_aye, src_slide_aye, "slide A must be byte-identical");
    assert_eq!(out_slide_bee, src_slide_bee, "slide B must be byte-identical");

    // Layout bytes are verbatim too (the fixture layout is accent-independent).
    let src_layout = src_a.part("ppt/slideLayouts/slideLayout1.xml").unwrap().to_vec();
    let mut layout_matches = 0;
    for name in out_pkg.part_names() {
        if name.starts_with("ppt/slideLayouts/") && name.ends_with(".xml") && !name.contains("/_rels/")
        {
            assert_eq!(out_pkg.part(name).unwrap(), src_layout.as_slice(), "layout {name} not verbatim");
            layout_matches += 1;
        }
    }
    assert!(layout_matches >= 1, "expected at least one copied layout");

    // Masters are only mutated by the composer's sldLayoutIdLst rewrite — the
    // rest (which the scaler *would* touch) is byte-identical to the source.
    let src_master = String::from_utf8_lossy(src_a.part("ppt/slideMasters/slideMaster1.xml").unwrap()).into_owned();
    for name in out_pkg.part_names().map(|s| s.to_string()).collect::<Vec<_>>() {
        if name.starts_with("ppt/slideMasters/") && name.ends_with(".xml") && !name.contains("/_rels/")
        {
            let out_master = String::from_utf8_lossy(out_pkg.part(&name).unwrap()).into_owned();
            assert_eq!(
                strip_layout_list(&out_master),
                strip_layout_list(&src_master),
                "master {name} changed outside its layout list — scaler must not have run"
            );
        }
    }

    cleanup(&[&a, &b, &out]);
}

/// Remove the `<p:sldLayoutIdLst>…</p:sldLayoutIdLst>` span (the composer rewrites
/// its ids); everything else must survive a copy byte-for-byte.
fn strip_layout_list(xml: &str) -> String {
    match (xml.find("<p:sldLayoutIdLst"), xml.find("</p:sldLayoutIdLst>")) {
        (Some(a), Some(b)) => {
            let end = b + "</p:sldLayoutIdLst>".len();
            format!("{}{}", &xml[..a], &xml[end..])
        }
        _ => xml.to_string(),
    }
}

// (13) back-compat: fit_mode None + an aspect mismatch leaves foreign slides
// unscaled (verbatim) and emits the legacy size-mismatch warning.
#[test]
fn aspect_mismatch_without_fit_mode_warns_and_copies_verbatim() {
    let dst = tmp("s13_dst.pptx"); // 4:3
    let src = tmp("s13_src.pptx"); // 16:9
    DeckSpec::new("Dst").slide_size(W4_3.0, W4_3.1).slide(SlideSpec::new("D")).write_to(&dst).unwrap();
    DeckSpec::new("Src")
        .slide_size(W16_9.0, W16_9.1)
        .slide(SlideSpec::new("S").raw_shape(text_shape(13, 900_000, 600_000, 1800, "ThirteenShape")))
        .write_to(&src)
        .unwrap();

    let out = tmp("s13_out.pptx");
    let picks = vec![pick(&dst, 1), pick(&src, 1)];
    let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();

    assert!(
        report.warnings.iter().any(|w| w.contains("different slide size")),
        "expected the legacy size-mismatch warning: {:?}",
        report.warnings
    );
    assert!(report.notes.is_empty(), "no scaling ⇒ no notes: {:?}", report.notes);

    // The foreign slide is copied verbatim: original geometry, no dx.
    let out_pkg = read_pkg(&out);
    let out_slide = part_containing(&out_pkg, "ppt/slides/", "ThirteenShape");
    let src_slide = String::from_utf8_lossy(read_pkg(&src).part("ppt/slides/slide1.xml").unwrap()).into_owned();
    assert_eq!(out_slide, src_slide, "unscaled slide must be byte-identical");
    assert!(out_slide.contains(r#"<a:off x="900000" y="600000"/>"#), "off untouched: {out_slide}");

    cleanup(&[&dst, &src, &out]);
}

// (14) layout inheritance: a slide placeholder with no xfrm draws its geometry
// from the (scaled) layout, so the scaled position lives on the layout — not the
// slide.
#[test]
fn layout_inherited_placeholder_scales_via_layout() {
    let dst = tmp("s14_dst.pptx"); // 16:9
    let src = tmp("s14_src.pptx"); // 16:9 small → ×4/3
    DeckSpec::new("Dst").slide(SlideSpec::new("D")).write_to(&dst).unwrap();
    // A body placeholder with no <a:xfrm> — its geometry is inherited from the
    // layout's body placeholder (off 838200×1825625 → scaled 1117600×2434167).
    let inherited = r#"<p:sp><p:nvSpPr><p:cNvPr id="40" name="FourteenBody"/><p:cNvSpPr><a:spLocks noGrp="1"/></p:cNvSpPr><p:nvPr><p:ph type="body" idx="1"/></p:nvPr></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>FourteenBody</a:t></a:r></a:p></p:txBody></p:sp>"#;
    DeckSpec::new("Src")
        .slide_size(W16_9_SMALL.0, W16_9_SMALL.1)
        .slide(SlideSpec::new("S").raw_shape(inherited))
        .write_to(&src)
        .unwrap();

    let out = tmp("s14_out.pptx");
    let picks = vec![pick(&dst, 1), pick(&src, 1)];
    let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();
    assert!(report.notes.iter().any(|n| n.contains("133%")), "notes: {:?}", report.notes);

    let pkg = read_pkg(&out);
    // The scaled layout carries the inherited body geometry.
    let layout = part_containing(&pkg, "ppt/slideLayouts/", r#"<a:off x="1117600" y="2434167"/>"#);
    assert!(layout.contains(r#"<a:off x="1117600" y="2434167"/>"#));
    // The slide's inherited placeholder has no such offset of its own.
    let slide = part_containing(&pkg, "ppt/slides/", "FourteenBody");
    assert!(!slide.contains(r#"y="2434167""#), "inherited placeholder must not gain an xfrm: {slide}");

    cleanup(&[&dst, &src, &out]);
}

// (15) three-deck mix: 16:9 first (reference), a same-aspect-smaller deck, and a
// 4:3 deck. With a fit mode set, both non-reference decks scale — two notes,
// each with its own factor.
#[test]
fn three_deck_mix_scales_each_deck_independently() {
    let a = tmp("s15_a.pptx"); // 16:9 reference
    let b = tmp("s15_b.pptx"); // 16:9 small → ×4/3
    let c = tmp("s15_c.pptx"); // 4:3 → EnsureFit s=1 + dx letterbox
    DeckSpec::new("A").slide(SlideSpec::new("D")).write_to(&a).unwrap();
    DeckSpec::new("B")
        .slide_size(W16_9_SMALL.0, W16_9_SMALL.1)
        .slide(SlideSpec::new("S").raw_shape(text_shape(15, 900_000, 600_000, 1800, "BeeShape")))
        .write_to(&b)
        .unwrap();
    DeckSpec::new("C")
        .slide_size(W4_3.0, W4_3.1)
        .slide(SlideSpec::new("S").raw_shape(text_shape(16, 900_000, 600_000, 1800, "CeeShape")))
        .write_to(&c)
        .unwrap();

    let out = tmp("s15_out.pptx");
    let picks = vec![pick(&a, 1), pick(&b, 1), pick(&c, 1)];
    let opts = ComposeOptions { fit_mode: Some(FitMode::EnsureFit), ..Default::default() };
    let report = compose(&picks, &out, &opts).unwrap();

    assert_eq!(report.notes.len(), 2, "expected two scale notes, got {:?}", report.notes);
    assert!(report.notes.iter().any(|n| n.contains("scaled to 133%")), "B factor: {:?}", report.notes);
    assert!(report.notes.iter().any(|n| n.contains("scaled to 100%")), "C factor: {:?}", report.notes);

    let pkg = read_pkg(&out);
    // B scaled ×4/3: 900000→1200000, 600000→800000.
    let bee = part_containing(&pkg, "ppt/slides/", "BeeShape");
    assert!(bee.contains(r#"<a:off x="1200000" y="800000"/>"#), "B geometry: {bee}");
    // C EnsureFit s=1 + dx 1524000: 900000→2424000, 600000 unchanged.
    let cee = part_containing(&pkg, "ppt/slides/", "CeeShape");
    assert!(cee.contains(r#"<a:off x="2424000" y="600000"/>"#), "C geometry: {cee}");

    cleanup(&[&a, &b, &c, &out]);
}
