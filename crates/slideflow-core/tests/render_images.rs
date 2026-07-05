//! Rendering regression for previously-unsupported image formats.
//!
//! `real/artistic-effects.pptx` carries an `image/tiff` raster referenced by a
//! plain `<a:blip r:embed>` in slideLayout6 (used by slide 2). Before TIFF
//! support the renderer dropped it as `unsupported-image` and painted the gray
//! photo placeholder; now it must decode and embed the picture instead.

use std::path::PathBuf;

use slideflow_core::pptx::PresentationFile;
use slideflow_core::render::{render_slide, RenderOptions};

/// Path to a committed corpus deck under `examples/pptx/`.
fn corpus(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/pptx").join(name)
}

#[test]
fn tiff_layout_image_renders_without_unsupported_drop() {
    let pf = PresentationFile::open(&corpus("real/artistic-effects.pptx")).unwrap();

    // Slide 2 uses slideLayout6, whose static (non-placeholder) picture embeds
    // the TIFF. Render at both cap settings to cover the pass-through and the
    // downscale re-encode branches.
    for opts in [RenderOptions::default(), RenderOptions::thumb(), RenderOptions::preview()] {
        let outcome = render_slide(&pf, 2, &opts).unwrap();
        assert!(
            !outcome.dropped.iter().any(|d| d == "unsupported-image"),
            "TIFF layout image should no longer drop as unsupported-image (dropped={:?}, opts={opts:?})",
            outcome.dropped,
        );
        // The TIFF is transcoded to a raster data URI (PNG at full res, or JPEG
        // once the thumbnail cap re-encodes the opaque photo).
        assert!(
            outcome.svg.contains("data:image/png") || outcome.svg.contains("data:image/jpeg"),
            "expected an embedded raster data URI in the rendered slide",
        );
        assert!(outcome.svg.starts_with("<svg"));
    }

    // The whole deck must now be free of unsupported-image drops.
    for i in 1..=pf.slide_count() {
        let outcome = render_slide(&pf, i, &RenderOptions::thumb()).unwrap();
        assert!(
            !outcome.dropped.iter().any(|d| d == "unsupported-image"),
            "slide {i} dropped an unsupported image: {:?}",
            outcome.dropped,
        );
    }
}
