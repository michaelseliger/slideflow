//! Picture (`p:pic`) rendering: image embedding as data URIs, raster
//! downscaling, and gray photo-glyph placeholders.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use roxmltree::{Document, Node};

use crate::opc::resolve_target;

use super::fill::A_NS;
use super::geometry::{parse_xfrm, Rect, Transform};
use super::{a, ch, fnum, Ctx};

impl Ctx<'_> {
    pub(crate) fn render_pic(&mut self, node: Node, tf: Transform) {
        let sp_pr = ch(node, "spPr");
        let ph = super::placeholder::shape_placeholder(node);
        // Placeholder pictures (p:ph type="pic") often carry no xfrm of their
        // own — geometry comes from the matching layout/master placeholder,
        // exactly like placeholder text shapes.
        let xfrm = sp_pr
            .and_then(|s| ch(s, "xfrm"))
            .map(parse_xfrm)
            .or_else(|| ph.as_ref().and_then(|p| self.inherited_xfrm(p)));
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

        // Non-rectangular geometry — the picture's own, or inherited from the
        // layout placeholder (templates cut photo placeholders into diagonals
        // via custGeom) — clips the image.
        let own_geom = sp_pr.and_then(|s| ch(s, "custGeom").or_else(|| ch(s, "prstGeom")));
        let inh_xml: Option<String> = if own_geom.is_none() {
            ph.as_ref()
                .and_then(|p| self.inherited_geom_xml(p))
                .map(|g| format!(r#"<sf xmlns:a="{A_NS}">{g}</sf>"#))
        } else {
            None
        };
        let inh_doc = inh_xml.as_deref().and_then(|xml| Document::parse(xml).ok());
        let geom_node = own_geom
            .or_else(|| inh_doc.as_ref().and_then(|d| d.root_element().first_element_child()));
        let geom_clip = self.geometry_clip(geom_node, &rect);
        if let Some(id) = &geom_clip {
            self.body.push_str(&format!(r#"<g clip-path="url(#{id})">"#));
        }

        let data_uri = if self.options.embed_images {
            self.pic_data_uri(node)
        } else {
            None
        };
        match data_uri {
            Some(uri) => self.emit_image(node, &rect, &uri),
            None => self.draw_image_placeholder(&rect),
        }

        if geom_clip.is_some() {
            self.body.push_str("</g>");
        }
        if open_g {
            self.body.push_str("</g>");
        }
    }

    /// Emit an `<image>` for `rect`, honoring a `blipFill/srcRect` crop: the crop
    /// selects a sub-rectangle of the source that must fill `rect`, so we draw
    /// the *full* image enlarged so its visible sub-rect lands on `rect`, then
    /// clip to `rect`.
    fn emit_image(&mut self, node: Node, rect: &Rect, uri: &str) {
        if let Some((l, t, r, b)) = ch(node, "blipFill").and_then(parse_src_rect) {
            let vis_w = 1.0 - l - r;
            let vis_h = 1.0 - t - b;
            let cropped = l != 0.0 || t != 0.0 || r != 0.0 || b != 0.0;
            if cropped && vis_w > 0.0 && vis_h > 0.0 {
                let full_w = rect.w / vis_w;
                let full_h = rect.h / vis_h;
                let img_x = rect.x - l * full_w;
                let img_y = rect.y - t * full_h;
                let cid = self.clip_id;
                self.clip_id += 1;
                self.defs.push_str(&format!(
                    r#"<clipPath id="imgclip{cid}"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath>"#,
                    x = fnum(rect.x),
                    y = fnum(rect.y),
                    w = fnum(rect.w),
                    h = fnum(rect.h),
                ));
                self.body.push_str(&format!(
                    r#"<image x="{x}" y="{y}" width="{w}" height="{h}" preserveAspectRatio="none" clip-path="url(#imgclip{cid})" href="{uri}"/>"#,
                    x = fnum(img_x),
                    y = fnum(img_y),
                    w = fnum(full_w),
                    h = fnum(full_h),
                ));
                return;
            }
        }
        self.body.push_str(&format!(
            r#"<image x="{x}" y="{y}" width="{w}" height="{h}" preserveAspectRatio="none" href="{uri}"/>"#,
            x = fnum(rect.x),
            y = fnum(rect.y),
            w = fnum(rect.w),
            h = fnum(rect.h),
        ));
    }

    fn pic_data_uri(&self, node: Node) -> Option<String> {
        let blip = ch(node, "blipFill").and_then(|b| ch(b, "blip"))?;
        self.blip_data_uri(blip)
    }

    /// Resolve an `a:blip` (from a `p:pic` or a shape `blipFill`) to a data URI.
    fn blip_data_uri(&self, blip: Node) -> Option<String> {
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
        let (target, _) = self.embed_target(embed)?;
        self.data_uri_for_target(&target)
    }

    /// Encode a resolved image part (by part name) as a data URI, applying the
    /// same content-type gate and thumbnail downscaling as blip embedding.
    fn data_uri_for_target(&self, target: &str) -> Option<String> {
        let bytes = self.pf.package.part(target)?;
        let ct = self
            .content_types
            .as_ref()
            .and_then(|c| c.content_type_of(target))
            .map(|s| s.to_string())
            .or_else(|| mime_from_ext(target))?;
        // A directly-content-typed SVG passes through untouched (compact,
        // resolution-independent) after the same safety check.
        if ct == "image/svg+xml" {
            return svg_is_safe(bytes)
                .then(|| format!("data:image/svg+xml;base64,{}", B64.encode(bytes)));
        }
        // EMF can't be rendered by browsers, but photo EMFs are metafile
        // wrappers around one big embedded bitmap — extract and embed that.
        if ct == "image/x-emf" || ct == "image/emf" {
            return self.emf_data_uri(bytes);
        }
        if !(ct == "image/png" || ct == "image/jpeg" || ct == "image/gif") {
            return None; // unsupported raster (WMF/TIFF/…): skip gracefully.
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

    /// Extract the embedded bitmap from an EMF and encode it as a data URI,
    /// transcoding uncompressed DIBs to PNG and applying the thumbnail cap.
    fn emf_data_uri(&self, bytes: &[u8]) -> Option<String> {
        let (mime, mut data) = emf_embedded_bitmap(bytes)?;
        let mut mime = mime.to_string();
        if mime == "image/bmp" {
            // Browsers would render BMP, but uncompressed DIBs are huge.
            let img = image::load_from_memory(&data).ok()?;
            let mut png = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
            data = png;
            mime = "image/png".to_string();
        }
        if let Some(cap) = self.options.max_image_px {
            if let Some((small_ct, small)) = downscale_raster(&data, cap) {
                return Some(format!("data:{small_ct};base64,{}", B64.encode(&small)));
            }
        }
        Some(format!("data:{mime};base64,{}", B64.encode(&data)))
    }

    /// Paint a shape's `blipFill` as a clipped image filling `rect` (PowerPoint
    /// stretches/crops a picture fill to the shape bounds — approximated as a
    /// clipped `slice` image). `geom_clip` (from [`Ctx::geometry_clip`]) clips
    /// to non-rectangular shape geometry; otherwise a plain rect clip is used.
    /// Falls back to the gray photo placeholder when not embedding or when the
    /// image can't be resolved.
    pub(crate) fn emit_blip_fill(&mut self, blip_fill: Node, rect: &Rect, geom_clip: Option<String>) {
        let uri = if self.options.embed_images {
            ch(blip_fill, "blip").and_then(|b| self.blip_data_uri(b))
        } else {
            None
        };
        let Some(uri) = uri else {
            self.draw_image_placeholder(rect);
            return;
        };
        let clip = geom_clip.unwrap_or_else(|| {
            let cid = self.clip_id;
            self.clip_id += 1;
            self.defs.push_str(&format!(
                r#"<clipPath id="blipclip{cid}"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath>"#,
                x = fnum(rect.x),
                y = fnum(rect.y),
                w = fnum(rect.w),
                h = fnum(rect.h),
            ));
            format!("blipclip{cid}")
        });
        self.body.push_str(&format!(
            r#"<image x="{x}" y="{y}" width="{w}" height="{h}" preserveAspectRatio="xMidYMid slice" clip-path="url(#{clip})" href="{uri}"/>"#,
            x = fnum(rect.x),
            y = fnum(rect.y),
            w = fnum(rect.w),
            h = fnum(rect.h),
        ));
    }

    /// Paint a full-slide picture background: resolve `embed` against the part
    /// that declared the `<p:bg>` (slide/layout/master) and emit an `<image>`
    /// covering the whole `w_pt`×`h_pt` slide (`slice` = fill, cropping overflow).
    pub(crate) fn emit_bg_image(&mut self, embed: &str, part: &str, w_pt: f64, h_pt: f64) {
        if !self.options.embed_images {
            return;
        }
        let rels = self.pf.package.rels_for(part).unwrap_or_default();
        let Some(rel) = rels.iter().find(|r| r.id == embed && !r.external) else {
            return;
        };
        let target = resolve_target(part, &rel.target);
        let Some(uri) = self.data_uri_for_target(&target) else {
            return;
        };
        self.body.push_str(&format!(
            r#"<image x="0" y="0" width="{w}" height="{h}" preserveAspectRatio="xMidYMid slice" href="{uri}"/>"#,
            w = fnum(w_pt),
            h = fnum(h_pt),
        ));
    }

    /// Resolve a (non-external) relationship id against the current part to the
    /// referenced part's `(target, bytes)`.
    fn embed_target(&self, embed_id: &str) -> Option<(String, &[u8])> {
        let rel = self.cur_rels.iter().find(|r| r.id == embed_id && !r.external)?;
        let target = resolve_target(&self.cur_part, &rel.target);
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

/// The `a:srcRect` crop fractions `(l, t, r, b)` of a `blipFill`, each a
/// fraction of the source dimension (0 = no crop; negative = zoom out — legal).
fn parse_src_rect(blip_fill: Node) -> Option<(f64, f64, f64, f64)> {
    let sr = ch(blip_fill, "srcRect")?;
    let pct = |name: &str| {
        a(sr, name)
            .and_then(|v| v.parse::<f64>().ok())
            .map(|v| v / 100_000.0)
            .unwrap_or(0.0)
    };
    Some((pct("l"), pct("t"), pct("r"), pct("b")))
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
        "emf" => Some("image/x-emf".into()),
        _ => None,
    }
}

/// Find the largest bitmap embedded in an EMF's records. Photo EMFs are
/// metafile wrappers around one big DIB drawn via `EMR_STRETCHDIBITS` (or
/// siblings); the DIB's `biCompression` decides the payload: `BI_JPEG`/`BI_PNG`
/// carry a ready-to-embed stream, `BI_RGB`/`BI_BITFIELDS` get a 14-byte
/// `BITMAPFILEHEADER` prepended so the bits parse as a regular BMP. Vector-only
/// EMFs yield `None` (the caller falls back to the gray placeholder).
fn emf_embedded_bitmap(b: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    let u32_at = |o: usize| -> Option<u32> {
        b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    // EMR_HEADER: iType 1, " EMF" signature at offset 40.
    if u32_at(0)? != 1 || b.get(40..44)? != b" EMF" {
        return None;
    }
    let mut best: Option<(&'static str, Vec<u8>)> = None;
    let mut best_len = 0usize;
    let mut off = 0usize;
    while off + 8 <= b.len() {
        let (Some(typ), Some(size)) = (u32_at(off), u32_at(off + 4).map(|v| v as usize)) else {
            break;
        };
        if size < 8 || size % 4 != 0 || off + size > b.len() {
            break;
        }
        // (offBmiSrc, cbBmiSrc, offBitsSrc, cbBitsSrc) positions per record type.
        let dib_fields = match typ {
            80 | 81 => Some(48), // EMR_SETDIBITSTODEVICE / EMR_STRETCHDIBITS
            76 | 77 => Some(84), // EMR_BITBLT / EMR_STRETCHBLT
            14 => break,         // EMR_EOF
            _ => None,
        };
        if let Some(f) = dib_fields {
            let get = |i: usize| u32_at(off + f + i * 4).map(|v| v as usize);
            if let (Some(off_bmi), Some(cb_bmi), Some(off_bits), Some(cb_bits)) =
                (get(0), get(1), get(2), get(3))
            {
                let bmi = b.get(off + off_bmi..off + off_bmi + cb_bmi);
                let bits = b.get(off + off_bits..off + off_bits + cb_bits);
                if let (Some(bmi), Some(bits)) = (bmi, bits) {
                    if bits.len() > best_len {
                        if let Some(found) = dib_to_image(bmi, bits) {
                            best_len = bits.len();
                            best = Some(found);
                        }
                    }
                }
            }
        }
        off += size;
    }
    best
}

/// Convert a DIB (`BITMAPINFO` + bits) into an embeddable image, keyed by
/// `biCompression`.
fn dib_to_image(bmi: &[u8], bits: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    if bmi.len() < 20 || bits.is_empty() {
        return None;
    }
    let compression = u32::from_le_bytes(bmi[16..20].try_into().unwrap());
    match compression {
        4 => Some(("image/jpeg", bits.to_vec())), // BI_JPEG
        5 => Some(("image/png", bits.to_vec())),  // BI_PNG
        0 | 3 => {
            // BI_RGB / BI_BITFIELDS: prepend a BITMAPFILEHEADER → valid BMP.
            let off_bits = 14 + bmi.len() as u32;
            let file_size = off_bits + bits.len() as u32;
            let mut bmp = Vec::with_capacity(file_size as usize);
            bmp.extend_from_slice(b"BM");
            bmp.extend_from_slice(&file_size.to_le_bytes());
            bmp.extend_from_slice(&0u32.to_le_bytes());
            bmp.extend_from_slice(&off_bits.to_le_bytes());
            bmp.extend_from_slice(bmi);
            bmp.extend_from_slice(bits);
            Some(("image/bmp", bmp))
        }
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
    use super::{emf_embedded_bitmap, parse_src_rect, svg_blip_embed, svg_is_safe};
    use roxmltree::Document;

    /// Minimal EMF: EMR_HEADER + one EMR_STRETCHDIBITS carrying `bmi`+`bits`
    /// + EMR_EOF.
    fn emf_with_stretchdibits(bmi: &[u8], bits: &[u8]) -> Vec<u8> {
        let mut hdr = vec![0u8; 88];
        hdr[0..4].copy_from_slice(&1u32.to_le_bytes()); // EMR_HEADER
        hdr[4..8].copy_from_slice(&88u32.to_le_bytes());
        hdr[40..44].copy_from_slice(b" EMF");
        let off_bmi = 80usize;
        let off_bits = off_bmi + bmi.len();
        let size = (80 + bmi.len() + bits.len() + 3) & !3;
        let mut rec = vec![0u8; size];
        rec[0..4].copy_from_slice(&81u32.to_le_bytes()); // EMR_STRETCHDIBITS
        rec[4..8].copy_from_slice(&(size as u32).to_le_bytes());
        rec[48..52].copy_from_slice(&(off_bmi as u32).to_le_bytes());
        rec[52..56].copy_from_slice(&(bmi.len() as u32).to_le_bytes());
        rec[56..60].copy_from_slice(&(off_bits as u32).to_le_bytes());
        rec[60..64].copy_from_slice(&(bits.len() as u32).to_le_bytes());
        rec[off_bmi..off_bmi + bmi.len()].copy_from_slice(bmi);
        rec[off_bits..off_bits + bits.len()].copy_from_slice(bits);
        let mut eof = vec![0u8; 20];
        eof[0..4].copy_from_slice(&14u32.to_le_bytes()); // EMR_EOF
        eof[4..8].copy_from_slice(&20u32.to_le_bytes());
        [hdr, rec, eof].concat()
    }

    /// A 40-byte BITMAPINFOHEADER with the given `biCompression`.
    fn bmi(width: i32, height: i32, bit_count: u16, compression: u32) -> Vec<u8> {
        let mut b = vec![0u8; 40];
        b[0..4].copy_from_slice(&40u32.to_le_bytes());
        b[4..8].copy_from_slice(&width.to_le_bytes());
        b[8..12].copy_from_slice(&height.to_le_bytes());
        b[12..14].copy_from_slice(&1u16.to_le_bytes());
        b[14..16].copy_from_slice(&bit_count.to_le_bytes());
        b[16..20].copy_from_slice(&compression.to_le_bytes());
        b
    }

    #[test]
    fn emf_extracts_embedded_jpeg() {
        let jpeg = b"\xFF\xD8\xFFfake jpeg payload\xFF\xD9".to_vec();
        let emf = emf_with_stretchdibits(&bmi(100, 100, 24, 4 /* BI_JPEG */), &jpeg);
        let (mime, data) = emf_embedded_bitmap(&emf).expect("bitmap found");
        assert_eq!(mime, "image/jpeg");
        assert_eq!(data, jpeg);
    }

    #[test]
    fn emf_wraps_rgb_dib_as_decodable_bmp() {
        // 1×1 24-bit BI_RGB: one BGR pixel padded to a 4-byte row.
        let emf = emf_with_stretchdibits(&bmi(1, 1, 24, 0 /* BI_RGB */), &[0, 0, 255, 0]);
        let (mime, data) = emf_embedded_bitmap(&emf).expect("bitmap found");
        assert_eq!(mime, "image/bmp");
        let img = image::load_from_memory(&data).expect("BMP decodes");
        assert_eq!((img.width(), img.height()), (1, 1));
    }

    #[test]
    fn emf_without_bitmap_yields_none() {
        // Header + EOF only (a vector-only metafile).
        let mut hdr = vec![0u8; 88];
        hdr[0..4].copy_from_slice(&1u32.to_le_bytes());
        hdr[4..8].copy_from_slice(&88u32.to_le_bytes());
        hdr[40..44].copy_from_slice(b" EMF");
        let mut eof = vec![0u8; 20];
        eof[0..4].copy_from_slice(&14u32.to_le_bytes());
        eof[4..8].copy_from_slice(&20u32.to_le_bytes());
        let emf = [hdr, eof].concat();
        assert!(emf_embedded_bitmap(&emf).is_none());
        assert!(emf_embedded_bitmap(b"not an emf at all").is_none());
    }

    #[test]
    fn parse_src_rect_reads_fractions() {
        let xml = r#"<a:blipFill xmlns:a="urn:a"><a:srcRect l="25000" t="0" r="10000" b="50000"/></a:blipFill>"#;
        let doc = Document::parse(xml).unwrap();
        let (l, t, r, b) = parse_src_rect(doc.root_element()).unwrap();
        assert!((l - 0.25).abs() < 1e-9);
        assert_eq!(t, 0.0);
        assert!((r - 0.10).abs() < 1e-9);
        assert!((b - 0.50).abs() < 1e-9);
    }

    #[test]
    fn parse_src_rect_absent_without_element() {
        let xml = r#"<a:blipFill xmlns:a="urn:a"><a:blip/></a:blipFill>"#;
        let doc = Document::parse(xml).unwrap();
        assert!(parse_src_rect(doc.root_element()).is_none());
    }

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
