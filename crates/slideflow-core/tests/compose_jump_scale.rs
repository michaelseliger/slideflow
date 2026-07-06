//! Regression: a slide-jump hyperlink target must be rescaled like the rest of
//! its deck's closure. In a mixed-size composition the jump-target slide is
//! pulled in via `copy_generic`; before the fix that path inserted the slide
//! bytes verbatim, so the target kept its unscaled geometry while its own
//! layout/master were rescaled — an internally inconsistent package.

use std::path::{Path, PathBuf};

use slideflow_core::fixtures::{DeckSpec, SlideSpec};
use slideflow_core::model::SlidePick;
use slideflow_core::opc::{rel_type, Package, Relationship};
use slideflow_core::pptx::{compose, ComposeOptions};

// 16:9 widescreen (the default fixture canvas) and its 3/4-size twin (a
// same-aspect resize ⇒ ×4/3 scale-up when composed onto the big canvas).
const W16_9_SMALL: (i64, i64) = (9144000, 5143500);

fn tmp(name: &str) -> PathBuf {
    let mut p = std::env::temp_dir();
    p.push(format!("slideflow_jump_{}_{}", std::process::id(), name));
    p
}

fn pick(path: &Path, idx: usize) -> SlidePick {
    SlidePick { pptx_path: path.to_string_lossy().into_owned(), slide_index: idx }
}

/// A text shape with an explicit top-level `a:off`/`a:ext` so scaled geometry is
/// observable in the output XML.
fn text_shape(id: u32, x: i64, y: i64, text: &str) -> String {
    format!(
        r#"<p:sp><p:nvSpPr><p:cNvPr id="{id}" name="{text}"/><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr><a:xfrm><a:off x="{x}" y="{y}"/><a:ext cx="1200000" cy="600000"/></a:xfrm></p:spPr><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US" sz="1800"/><a:t>{text}</a:t></a:r></a:p></p:txBody></p:sp>"#
    )
}

/// A clickable shape whose `hlinkClick` jumps to another slide via `rId9`.
fn jump_shape() -> String {
    r#"<p:sp><p:nvSpPr><p:cNvPr id="60" name="JumpButton"><a:hlinkClick r:id="rId9"/></p:cNvPr><p:cNvSpPr/><p:nvPr/></p:nvSpPr><p:spPr/><p:txBody><a:bodyPr/><a:lstStyle/><a:p><a:r><a:rPr lang="en-US"/><a:t>Go</a:t></a:r></a:p></p:txBody></p:sp>"#.to_string()
}

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

#[test]
fn jump_target_slide_is_scaled_with_its_deck() {
    let dst = tmp("dst.pptx"); // 16:9 reference canvas
    let src = tmp("src.pptx"); // 16:9 small ⇒ scaled ×4/3

    DeckSpec::new("Dst").slide(SlideSpec::new("D")).write_to(&dst).unwrap();

    // Src slide 1 carries a jump to slide 2; slide 2 has observable geometry.
    let src_bytes = DeckSpec::new("Src")
        .slide_size(W16_9_SMALL.0, W16_9_SMALL.1)
        .slide(SlideSpec::new("JumpSource").raw_shape(jump_shape()))
        .slide(SlideSpec::new("JumpDest").raw_shape(text_shape(61, 900_000, 600_000, "JumpTarget")))
        .build();

    // Inject the slide→slide relationship the jump references (the fixtures
    // builder has no slide-jump support).
    let mut src_pkg = Package::from_bytes(&src_bytes).unwrap();
    let mut rels = src_pkg.rels_for("ppt/slides/slide1.xml").unwrap();
    rels.push(Relationship {
        id: "rId9".into(),
        rel_type: rel_type::SLIDE.into(),
        target: "../slides/slide2.xml".into(),
        external: false,
    });
    src_pkg.set_rels("ppt/slides/slide1.xml", &rels);
    std::fs::write(&src, src_pkg.to_bytes().unwrap()).unwrap();

    // Pick only the jump *source*; the target is pulled in through the jump rel.
    let out = tmp("out.pptx");
    let picks = vec![pick(&dst, 1), pick(&src, 1)];
    let report = compose(&picks, &out, &ComposeOptions::default()).unwrap();

    assert!(
        report.warnings.iter().any(|w| w.contains("slide link")),
        "expected the out-of-show slide-link warning: {:?}",
        report.warnings
    );
    assert!(
        report.notes.iter().any(|n| n.contains("133%")),
        "src should scale ×4/3: {:?}",
        report.notes
    );

    // The jump-target slide's geometry must be scaled ×4/3, matching its own
    // rescaled layout/master: 900000→1200000, 600000→800000.
    let pkg = Package::from_bytes(&std::fs::read(&out).unwrap()).unwrap();
    let target = part_containing(&pkg, "ppt/slides/", "JumpTarget");
    assert!(
        target.contains(r#"<a:off x="1200000" y="800000"/>"#),
        "jump-target slide must be scaled with its deck, got: {target}"
    );

    for p in [&dst, &src, &out] {
        let _ = std::fs::remove_file(p);
    }
}
