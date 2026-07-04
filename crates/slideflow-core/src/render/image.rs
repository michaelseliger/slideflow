//! Picture (`p:pic`) rendering: image embedding as data URIs, raster
//! downscaling, and gray photo-glyph placeholders.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use roxmltree::Node;

use crate::opc::resolve_target;

use super::geometry::{parse_xfrm, Rect, Transform};
use super::{ch, fnum, Ctx};

impl Ctx<'_> {
    pub(crate) fn render_pic(&mut self, node: Node, tf: Transform) {
        let sp_pr = ch(node, "spPr");
        let xfrm = sp_pr.and_then(|s| ch(s, "xfrm")).map(parse_xfrm);
        let Some(x) = xfrm else { return };
        let rect = tf.apply(&x);
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let transform = rect.svg_transform(&x);
        let open_g = !transform.is_empty();
        if open_g {
            self.body.push_str(&format!(r#"<g transform="{transform}">"#));
        }

        let data_uri = if self.options.embed_images {
            self.pic_data_uri(node)
        } else {
            None
        };
        match data_uri {
            Some(uri) => {
                self.body.push_str(&format!(
                    r#"<image x="{x}" y="{y}" width="{w}" height="{h}" preserveAspectRatio="none" href="{uri}"/>"#,
                    x = fnum(rect.x),
                    y = fnum(rect.y),
                    w = fnum(rect.w),
                    h = fnum(rect.h),
                    uri = uri
                ));
            }
            None => self.draw_image_placeholder(&rect),
        }

        if open_g {
            self.body.push_str("</g>");
        }
    }

    fn pic_data_uri(&self, node: Node) -> Option<String> {
        let blip = ch(node, "blipFill").and_then(|b| ch(b, "blip"))?;

        // Prefer a vector (SVG) source when present. PowerPoint stores it in an
        // a:extLst extension (<asvg:svgBlip r:embed>) and keeps the raster
        // r:embed only as a fallback — so try the SVG first, and fall through to
        // the raster if it's missing or looks unsafe.
        if let Some(svg_embed) = svg_blip_embed(blip) {
            if let Some((_, bytes)) = self.embed_target(&svg_embed) {
                if svg_is_safe(bytes) {
                    return Some(format!("data:image/svg+xml;base64,{}", B64.encode(bytes)));
                }
            }
        }

        // Raster (or content-typed SVG) via the blip's own r:embed.
        let embed = blip.attributes().find(|at| at.name() == "embed").map(|at| at.value())?;
        let (target, bytes) = self.embed_target(embed)?;
        let ct = self
            .content_types
            .as_ref()
            .and_then(|c| c.content_type_of(&target))
            .map(|s| s.to_string())
            .or_else(|| mime_from_ext(&target))?;
        // A directly-content-typed SVG passes through untouched (compact,
        // resolution-independent) after the same safety check.
        if ct == "image/svg+xml" {
            return svg_is_safe(bytes)
                .then(|| format!("data:image/svg+xml;base64,{}", B64.encode(bytes)));
        }
        if !(ct == "image/png" || ct == "image/jpeg" || ct == "image/gif") {
            return None; // unsupported raster (EMF/WMF/…): skip gracefully.
        }
        // Downscale oversized rasters for lightweight thumbnails; on any decode
        // failure fall through and embed the original bytes (never drop the image).
        if let Some(cap) = self.options.max_image_px {
            if let Some((new_ct, new_bytes)) = downscale_raster(bytes, cap) {
                return Some(format!("data:{};base64,{}", new_ct, B64.encode(&new_bytes)));
            }
        }
        Some(format!("data:{};base64,{}", ct, B64.encode(bytes)))
    }

    /// Resolve a (non-external) relationship id on the slide to the referenced
    /// part's `(target, bytes)`.
    fn embed_target(&self, embed_id: &str) -> Option<(String, &[u8])> {
        let rel = self.slide_rels.iter().find(|r| r.id == embed_id && !r.external)?;
        let target = resolve_target(&self.slide_part, &rel.target);
        let bytes = self.pf.package.part(&target)?;
        Some((target, bytes))
    }

    fn draw_image_placeholder(&mut self, rect: &Rect) {
        self.body.push_str(&format!(
            r##"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="#D1D5DB"/>"##,
            x = fnum(rect.x),
            y = fnum(rect.y),
            w = fnum(rect.w),
            h = fnum(rect.h)
        ));
        // A minimal "photo" glyph: a sun disc and a mountain triangle.
        let cx = rect.x + rect.w * 0.3;
        let cy = rect.y + rect.h * 0.3;
        let r = (rect.w.min(rect.h) * 0.08).max(1.0);
        self.body.push_str(&format!(
            r##"<circle cx="{cx}" cy="{cy}" r="{r}" fill="#9CA3AF"/>"##,
            cx = fnum(cx),
            cy = fnum(cy),
            r = fnum(r)
        ));
        let bx = rect.x + rect.w * 0.15;
        let by = rect.y + rect.h * 0.8;
        let mx = rect.x + rect.w * 0.5;
        let my = rect.y + rect.h * 0.45;
        let ex = rect.x + rect.w * 0.85;
        self.body.push_str(&format!(
            r##"<polygon points="{bx},{by} {mx},{my} {ex},{by}" fill="#9CA3AF"/>"##,
            bx = fnum(bx),
            by = fnum(by),
            mx = fnum(mx),
            my = fnum(my),
            ex = fnum(ex)
        ));
    }
}

/// The `r:embed` of an `<asvg:svgBlip>` inside a blip's `a:extLst`, if present.
/// Matched by local name so it's namespace-prefix agnostic.
fn svg_blip_embed(blip: Node) -> Option<String> {
    blip.descendants()
        .find(|n| n.is_element() && n.tag_name().name() == "svgBlip")?
        .attributes()
        .find(|a| a.name() == "embed")
        .map(|a| a.value().to_string())
}

/// Whether an embedded SVG is safe to inline as an `<image>` data URI: it must
/// look like SVG and must not run scripts or fetch external resources. Note the
/// SVG namespace declaration (`xmlns="http://www.w3.org/2000/svg"`) legitimately
/// contains an http URL, so we flag concrete external *references* — `href`/
/// `src`/`url(...)` pointing at http(s) — and `<script>`/`<foreignObject>`,
/// rather than any occurrence of "http". Our output is itself loaded as a
/// sandboxed image, but keep the embed honest anyway.
fn svg_is_safe(bytes: &[u8]) -> bool {
    let Ok(s) = std::str::from_utf8(bytes) else {
        return false;
    };
    let lower = s.to_ascii_lowercase();
    if !lower.contains("<svg") {
        return false;
    }
    const BAD: [&str; 7] = [
        "<script",
        "<foreignobject",
        "href=\"http",
        "href='http",
        "src=\"http",
        "src='http",
        "url(http",
    ];
    !BAD.iter().any(|p| lower.contains(p))
}

fn mime_from_ext(part: &str) -> Option<String> {
    let ext = part.rsplit_once('.')?.1.to_ascii_lowercase();
    match ext.as_str() {
        "png" => Some("image/png".into()),
        "jpg" | "jpeg" => Some("image/jpeg".into()),
        "gif" => Some("image/gif".into()),
        _ => None,
    }
}

/// Downscale a raster whose longer edge exceeds `cap` pixels, returning the
/// re-encoded `(content_type, bytes)`. Images with an alpha channel re-encode as
/// PNG (to keep transparency); opaque images as JPEG q80 — JPEG-ing opaque
/// photos is where most of the size reduction comes from. Returns `None` when
/// the image is already within `cap` or can't be decoded; the caller then embeds
/// the original bytes rather than dropping the image entirely.
pub(crate) fn downscale_raster(bytes: &[u8], cap: u32) -> Option<(String, Vec<u8>)> {
    // Cheap header sniff to skip decoding images already under the cap.
    if let Ok(dim) = imagesize::blob_size(bytes) {
        if dim.width as u32 <= cap && dim.height as u32 <= cap {
            return None;
        }
    }
    let img = image::load_from_memory(bytes).ok()?;
    if img.width() <= cap && img.height() <= cap {
        return None;
    }
    // Fits the image within cap×cap, preserving aspect ratio.
    let resized = img.resize(cap, cap, image::imageops::FilterType::Triangle);
    let mut out = Vec::new();
    if resized.color().has_alpha() {
        resized
            .write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png)
            .ok()?;
        Some(("image/png".to_string(), out))
    } else {
        let rgb = image::DynamicImage::ImageRgb8(resized.to_rgb8());
        let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, 80);
        enc.encode_image(&rgb).ok()?;
        Some(("image/jpeg".to_string(), out))
    }
}

#[cfg(test)]
mod tests {
    use super::{svg_blip_embed, svg_is_safe};
    use roxmltree::Document;

    #[test]
    fn svg_blip_embed_prefers_the_vector_source() {
        let xml = r#"<a:blip xmlns:a="urn:a" xmlns:r="urn:r" xmlns:asvg="urn:s" r:embed="rIdRaster">
            <a:extLst><a:ext uri="{28A0092B}"><asvg:svgBlip r:embed="rIdVector"/></a:ext></a:extLst>
        </a:blip>"#;
        let doc = Document::parse(xml).unwrap();
        assert_eq!(svg_blip_embed(doc.root_element()).as_deref(), Some("rIdVector"));
    }

    #[test]
    fn svg_blip_embed_absent_without_extension() {
        let xml = r#"<a:blip xmlns:a="urn:a" xmlns:r="urn:r" r:embed="rIdRaster"/>"#;
        let doc = Document::parse(xml).unwrap();
        assert!(svg_blip_embed(doc.root_element()).is_none());
    }

    #[test]
    fn svg_safety_gate() {
        // The SVG namespace URL alone must NOT trip the gate.
        assert!(svg_is_safe(
            br#"<svg xmlns="http://www.w3.org/2000/svg"><rect width="1" height="1"/></svg>"#
        ));
        assert!(!svg_is_safe(br#"<svg><script>alert(1)</script></svg>"#));
        assert!(!svg_is_safe(
            br#"<svg><image href="http://evil/x.png"/></svg>"#
        ));
        assert!(!svg_is_safe(br#"<svg><foreignObject/></svg>"#));
        assert!(!svg_is_safe(b"not svg at all"));
    }
}
