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

    fn pic_data_uri(&mut self, node: Node) -> Option<String> {
        let blip = ch(node, "blipFill").and_then(|b| ch(b, "blip"))?;
        self.blip_data_uri(blip)
    }

    /// Resolve an `a:blip` (from a `p:pic` or a shape `blipFill`) to a data URI.
    fn blip_data_uri(&mut self, blip: Node) -> Option<String> {
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
    fn data_uri_for_target(&mut self, target: &str) -> Option<String> {
        // Hoist the Copy `&'a PresentationFile` so the byte slice carries
        // lifetime 'a and is provably independent of the `&mut self` borrow —
        // that's what lets us call `record_drop` below without aliasing `bytes`.
        let pf = self.pf;
        let bytes = pf.package.part(target)?;
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
        // EMF can't be rendered by browsers. Photo EMFs are metafile wrappers
        // around one big embedded bitmap — extract and embed that. A vector-only
        // EMF (paths, no bitmap — e.g. a logo) is interpreted into a
        // self-contained SVG. Only when both fail is the image unsupported.
        if ct == "image/x-emf" || ct == "image/emf" {
            if let Some(uri) = self.emf_data_uri(bytes) {
                return Some(uri);
            }
            if let Some(svg) = emf_vector_svg(bytes) {
                return Some(format!("data:image/svg+xml;base64,{}", B64.encode(svg.as_bytes())));
            }
            self.record_drop("unsupported-image");
            return None;
        }
        // TIFF: browsers can't display it, but it's a plain raster — decode and
        // re-encode as PNG (applying the thumbnail cap). The image crate (tiff
        // feature) is the primary decoder; palette-color TIFFs, which tiff
        // 0.11 rejects, fall back to the in-house uncompressed-subset decoder.
        // Only when both fail does the image count as unsupported.
        if ct == "image/tiff" {
            return match raster_to_png(bytes).or_else(|| decode_uncompressed_tiff(bytes)) {
                Some(png) => Some(self.png_data_uri(&png)),
                None => {
                    self.record_drop("unsupported-image");
                    None
                }
            };
        }
        // WMF, like EMF, is a metafile: photo WMFs wrap one embedded DIB drawn
        // via META_DIBSTRETCHBLT / META_STRETCHDIB / META_DIBBITBLT. Extract and
        // embed that; a vector-only WMF yields nothing → unsupported image.
        if ct == "image/x-wmf" || ct == "image/wmf" {
            return self.wmf_data_uri(bytes).or_else(|| {
                self.record_drop("unsupported-image");
                None
            });
        }
        if !(ct == "image/png" || ct == "image/jpeg" || ct == "image/gif") {
            self.record_drop("unsupported-image");
            return None; // unsupported raster (WDP/JPEG-XR, …): skip gracefully.
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
        self.embedded_bitmap_data_uri(emf_embedded_bitmap(bytes)?)
    }

    /// Extract the embedded bitmap from a WMF and encode it as a data URI. Same
    /// policy as [`Ctx::emf_data_uri`]: uncompressed DIBs transcode to PNG,
    /// the thumbnail cap applies, and a vector-only WMF yields `None`.
    fn wmf_data_uri(&self, bytes: &[u8]) -> Option<String> {
        self.embedded_bitmap_data_uri(wmf_embedded_bitmap(bytes)?)
    }

    /// Encode a `(mime, bytes)` bitmap pulled out of a metafile as a data URI:
    /// uncompressed DIBs (`image/bmp`) transcode to PNG (they are huge raw), and
    /// every result honors the thumbnail cap.
    fn embedded_bitmap_data_uri(&self, found: (&'static str, Vec<u8>)) -> Option<String> {
        let (mime, data) = found;
        if mime == "image/bmp" {
            // Browsers would render BMP, but uncompressed DIBs are huge.
            let png = raster_to_png(&data)?;
            return Some(self.png_data_uri(&png));
        }
        // BI_JPEG / BI_PNG payloads pass through as-is; still cap oversized ones.
        if let Some(cap) = self.options.max_image_px {
            if let Some((small_ct, small)) = downscale_raster(&data, cap) {
                return Some(format!("data:{small_ct};base64,{}", B64.encode(&small)));
            }
        }
        Some(format!("data:{mime};base64,{}", B64.encode(&data)))
    }

    /// Format already-PNG bytes as a data URI, applying the thumbnail cap (which
    /// may re-encode to JPEG for opaque images) when configured.
    fn png_data_uri(&self, png: &[u8]) -> String {
        if let Some(cap) = self.options.max_image_px {
            if let Some((small_ct, small)) = downscale_raster(png, cap) {
                return format!("data:{small_ct};base64,{}", B64.encode(&small));
            }
        }
        format!("data:image/png;base64,{}", B64.encode(png))
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
        "wmf" => Some("image/x-wmf".into()),
        "tif" | "tiff" => Some("image/tiff".into()),
        // NB: `.wdp` (JPEG XR / HD Photo) is deliberately NOT mapped — there is
        // no pure-Rust decoder, so it must land on the unsupported-image
        // placeholder rather than be claimed as a decodable raster here.
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

/// Find the largest bitmap embedded in a WMF's records. Photo WMFs — the older
/// sibling of EMF — wrap one big DIB drawn via `META_DIBSTRETCHBLT`,
/// `META_STRETCHDIB`, or `META_DIBBITBLT`. Unlike EMF, a WMF carries the DIB as
/// a single contiguous *packed* DIB (`BITMAPINFOHEADER` + masks + palette +
/// pixels) at the tail of the record, so [`split_packed_dib`] splits it into the
/// `(bmi, bits)` pair [`dib_to_image`] expects. An optional 22-byte Aldus
/// placeable header precedes the standard 18-byte `META_HEADER`. Vector-only
/// WMFs yield `None` (the caller falls back to the gray placeholder). Every
/// offset/length is bounds-checked — the input is untrusted.
fn wmf_embedded_bitmap(b: &[u8]) -> Option<(&'static str, Vec<u8>)> {
    let u16_at = |o: usize| -> Option<u16> {
        b.get(o..o + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap()))
    };
    let u32_at = |o: usize| -> Option<u32> {
        b.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    };
    // Skip the optional Aldus placeable header (magic 0x9AC6CDD7, 22 bytes).
    let mut off = if u32_at(0) == Some(0x9AC6_CDD7) { 22 } else { 0 };
    // Standard META_HEADER: Type (1=memory / 2=disk), HeaderSize == 9 words.
    // Require both to reject input that isn't actually a WMF.
    let hdr_type = u16_at(off)?;
    if (hdr_type != 1 && hdr_type != 2) || u16_at(off + 2)? != 9 {
        return None;
    }
    off += 18;
    let mut best: Option<(&'static str, Vec<u8>)> = None;
    let mut best_len = 0usize;
    while off + 6 <= b.len() {
        // RecordSize is in 16-bit words and includes the size+function fields.
        let size_words = u32_at(off)? as usize;
        let func = u16_at(off + 4)?;
        let Some(rec_bytes) = size_words.checked_mul(2) else { break };
        if rec_bytes < 6 || off + rec_bytes > b.len() {
            break;
        }
        // Byte offset (from the record start) of the packed DIB for records that
        // carry one; the DIB runs to the record end.
        let dib_off = match func {
            0x0940 => Some(22), // META_DIBBITBLT   (with-bitmap variant)
            0x0B41 => Some(26), // META_DIBSTRETCHBLT (with-bitmap variant)
            0x0F43 => Some(28), // META_STRETCHDIB
            0x0000 => break,    // META_EOF
            _ => None,
        };
        if let Some(d) = dib_off {
            // `get` yields None for a start-past-end range (no-bitmap variants,
            // where the DIB offset overshoots the short record) — safe.
            if let Some(dib) = b.get(off + d..off + rec_bytes) {
                if let Some((bmi, bits)) = split_packed_dib(dib) {
                    if bits.len() > best_len {
                        if let Some(found) = dib_to_image(bmi, bits) {
                            best_len = bits.len();
                            best = Some(found);
                        }
                    }
                }
            }
        }
        off += rec_bytes;
    }
    best
}

/// Split a *packed* DIB (as WMF records carry it inline) into the `(bmi, bits)`
/// pair [`dib_to_image`] consumes: `bmi` is the `BITMAPINFOHEADER` plus any
/// `BI_BITFIELDS` color masks and color table, `bits` the trailing pixel/stream
/// data. Only `BITMAPINFOHEADER`-family DIBs (`biSize >= 40`) are handled;
/// smaller (OS/2 core) headers and any out-of-bounds split yield `None`.
fn split_packed_dib(dib: &[u8]) -> Option<(&[u8], &[u8])> {
    let u16_at = |o: usize| dib.get(o..o + 2).map(|s| u16::from_le_bytes(s.try_into().unwrap()));
    let u32_at = |o: usize| dib.get(o..o + 4).map(|s| u32::from_le_bytes(s.try_into().unwrap()));
    let hdr = u32_at(0)? as usize;
    if hdr < 40 || hdr > dib.len() {
        return None;
    }
    let bit_count = u16_at(14)?;
    let compression = u32_at(16)?;
    let clr_used = u32_at(32)? as usize;
    // Number of RGBQUAD color-table entries following the header (+masks).
    let palette_entries = if clr_used != 0 {
        clr_used
    } else if bit_count <= 8 {
        1usize << bit_count
    } else {
        0
    };
    let (masks, palette) = match compression {
        // BI_JPEG / BI_PNG: a compressed stream follows the header, no palette.
        4 | 5 => (0usize, 0usize),
        // BI_BITFIELDS with a 40-byte header stores three DWORD masks inline
        // before the pixels (V4/V5 headers embed them, so only for biSize==40).
        3 if hdr == 40 => (12usize, palette_entries.checked_mul(4)?),
        _ => (0usize, palette_entries.checked_mul(4)?),
    };
    let bmi_end = hdr.checked_add(masks)?.checked_add(palette)?;
    if bmi_end > dib.len() {
        return None;
    }
    let (bmi, bits) = dib.split_at(bmi_end);
    if bits.is_empty() {
        return None;
    }
    Some((bmi, bits))
}

/// Decode a raster the browser can't display natively (TIFF, or a metafile's
/// uncompressed embedded DIB) and re-encode it as PNG bytes. Returns `None` only
/// on a genuine decode/encode failure.
fn raster_to_png(bytes: &[u8]) -> Option<Vec<u8>> {
    let img = image::load_from_memory(bytes).ok()?;
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
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

/// Minimal, defensive baseline-TIFF decoder — the fallback behind
/// [`raster_to_png`] for what the tiff crate (0.11) rejects, most notably
/// palette-color TIFFs as PowerPoint embeds them. Covers the uncompressed
/// subset seen in real decks: `Compression = 1` (none), chunky
/// (`PlanarConfiguration = 1`), strip-based, both byte orders (`II`/`MM`), and
/// one of
/// - palette-color (`PhotometricInterpretation = 3`), 4- or 8-bit indices
///   expanded through the `ColorMap` (16-bit entries scaled by `/257`),
/// - RGB / RGBA (`= 2`), 8 bits per sample, 3 or 4 samples,
/// - grayscale (`= 0` white-is-zero / `= 1` black-is-zero), 8-bit.
///
/// Everything outside the subset — any other compression, bit depth, planar
/// layout, or photometric — returns `None` (the caller then drops the image
/// honestly rather than rendering something wrong). All reads are bounds-checked
/// against untrusted input; a malformed file yields `None`, never a panic. A
/// palette index beyond the supplied colormap is clamped to its last entry.
fn decode_uncompressed_tiff(b: &[u8]) -> Option<Vec<u8>> {
    let le = match b.get(0..2)? {
        b"II" => true,
        b"MM" => false,
        _ => return None,
    };
    let r16 = |o: usize| -> Option<u16> {
        let s = b.get(o..o + 2)?;
        let a = [s[0], s[1]];
        Some(if le { u16::from_le_bytes(a) } else { u16::from_be_bytes(a) })
    };
    let r32 = |o: usize| -> Option<u32> {
        let s = b.get(o..o + 4)?;
        let a: [u8; 4] = s.try_into().unwrap();
        Some(if le { u32::from_le_bytes(a) } else { u32::from_be_bytes(a) })
    };
    if r16(2)? != 42 {
        return None;
    }
    let ifd = r32(4)? as usize;
    let n = r16(ifd)? as usize;
    if n == 0 || n > 512 {
        return None;
    }
    // Collect (tag, field-type, count, value-field-offset) for every IFD entry.
    let mut entries: Vec<(u16, u16, u32, usize)> = Vec::with_capacity(n);
    for i in 0..n {
        let e = ifd.checked_add(2)?.checked_add(i.checked_mul(12)?)?;
        entries.push((r16(e)?, r16(e + 2)?, r32(e + 4)?, e + 8));
    }
    let type_size = |t: u16| -> usize {
        match t {
            1 | 2 | 6 | 7 => 1, // BYTE / ASCII / SBYTE / UNDEFINED
            3 | 8 => 2,         // SHORT / SSHORT
            4 | 9 => 4,         // LONG / SLONG
            _ => 0,
        }
    };
    // All values of a tag, widened to u32 (only BYTE/SHORT/LONG are needed).
    let values = |tag: u16| -> Option<Vec<u32>> {
        let &(_, typ, count, valoff) = entries.iter().find(|e| e.0 == tag)?;
        let ts = type_size(typ);
        if ts == 0 {
            return None;
        }
        let count = count as usize;
        let total = ts.checked_mul(count)?;
        // A tag's values physically live in the file, so `total` can never exceed
        // its length. Reject an absurd count (a hostile IFD can claim ~4 billion,
        // e.g. count 0xFFFFFFFF of LONG → ~17 GB) HERE, before the
        // `Vec::with_capacity(count)` below reserves gigabytes and aborts.
        if total > b.len() {
            return None;
        }
        let base = if total <= 4 { valoff } else { r32(valoff)? as usize };
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let o = base.checked_add(i.checked_mul(ts)?)?;
            out.push(match ts {
                1 => *b.get(o)? as u32,
                2 => r16(o)? as u32,
                _ => r32(o)?,
            });
        }
        Some(out)
    };
    let first = |tag: u16| -> Option<u32> { values(tag)?.into_iter().next() };

    let width = first(256)? as usize;
    let height = first(257)? as usize;
    if width == 0 || height == 0 || width.checked_mul(height)? > 40_000_000 {
        return None;
    }
    if first(259).unwrap_or(1) != 1 {
        return None; // only uncompressed
    }
    if first(284).unwrap_or(1) != 1 {
        return None; // only chunky (interleaved) samples
    }
    if first(317).unwrap_or(1) != 1 {
        return None; // predictors (horizontal differencing) unsupported
    }
    let photometric = first(262)?;
    let spp = first(277).unwrap_or(1) as usize;
    let bits = values(258).unwrap_or_else(|| vec![1]);
    let rows_per_strip = match first(278) {
        Some(0) | None => height,
        Some(r) => r as usize,
    };
    let strip_offsets = values(273)?;
    let strip_counts = values(279)?;
    if strip_offsets.len() != strip_counts.len() || strip_offsets.is_empty() {
        return None;
    }
    // A strip spans whole rows, so a valid file has at most one strip per row.
    // Reject an absurd strip table (a hostile file can list millions of strips,
    // each claiming the whole file — naively summing them would `extend` terabytes)
    // before touching any strip data.
    if strip_offsets.len() > height {
        return None;
    }
    let _ = rows_per_strip; // strips are whole rows; sequential concat suffices.

    // Resolve output channels and the packed source row width. Computed BEFORE the
    // strip concatenation so it can bound how much we copy.
    let (out_ch, row_bytes) = match photometric {
        3 => {
            let bpp = *bits.first()?;
            if bpp != 4 && bpp != 8 {
                return None;
            }
            let rb = if bpp == 8 { width } else { width.div_ceil(2) };
            (3usize, rb)
        }
        2 => {
            if (spp != 3 && spp != 4) || bits.iter().take(spp).any(|&x| x != 8) {
                return None;
            }
            (spp, width.checked_mul(spp)?)
        }
        0 | 1 => {
            if spp != 1 || *bits.first()? != 8 {
                return None;
            }
            (3usize, width)
        }
        _ => return None,
    };

    // The image needs exactly `height` full rows. Bound the concatenation by this
    // (`width*height ≤ 40M` was checked above, so `expected ≤ ~160 MB`) and stop
    // once we have it — a strip table whose byte counts sum to gigabytes fills only
    // what the image uses and is truncated to exactly `expected`, never ballooning
    // `raw` past the image. A legit file's strips sum to precisely this.
    let expected = height.checked_mul(row_bytes)?;
    let mut raw: Vec<u8> = Vec::with_capacity(expected);
    for (o, c) in strip_offsets.iter().zip(&strip_counts) {
        if raw.len() >= expected {
            break;
        }
        let start = *o as usize;
        let end = start.checked_add(*c as usize)?;
        raw.extend_from_slice(b.get(start..end)?);
    }
    raw.truncate(expected);
    if raw.len() < expected {
        return None;
    }

    let colormap = if photometric == 3 { Some(values(320)?) } else { None };
    if let Some(cm) = &colormap {
        if cm.len() < 3 {
            return None;
        }
    }
    let bpp = *bits.first().unwrap_or(&8);

    let mut out: Vec<u8> = Vec::with_capacity(width.checked_mul(height)?.checked_mul(out_ch)?);
    for y in 0..height {
        let row = &raw[y * row_bytes..y * row_bytes + row_bytes];
        for x in 0..width {
            match photometric {
                3 => {
                    let cm = colormap.as_ref().unwrap();
                    let nent = cm.len() / 3;
                    let idx = if bpp == 8 {
                        row[x] as usize
                    } else {
                        let byte = row[x / 2] as usize;
                        if x % 2 == 0 {
                            byte >> 4
                        } else {
                            byte & 0x0f
                        }
                    }
                    .min(nent.saturating_sub(1));
                    out.push((cm[idx] / 257) as u8);
                    out.push((cm[nent + idx] / 257) as u8);
                    out.push((cm[2 * nent + idx] / 257) as u8);
                }
                2 => {
                    let base = x * spp;
                    out.extend_from_slice(&row[base..base + spp]);
                }
                _ => {
                    let v = row[x];
                    let g = if photometric == 0 { 255 - v } else { v };
                    out.extend_from_slice(&[g, g, g]);
                }
            }
        }
    }

    let (w, h) = (width as u32, height as u32);
    let img = if out_ch == 4 {
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_raw(w, h, out)?)
    } else {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_raw(w, h, out)?)
    };
    let mut png = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).ok()?;
    Some(png)
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

// ---------------------------------------------------------------------------
// EMF vector-path interpreter
// ---------------------------------------------------------------------------

/// A GDI object stored in the EMF handle table: a fill brush or a stroke pen.
/// `None` colour means the object paints nothing (NULL brush / NULL pen).
#[derive(Clone, Copy)]
enum EmfObj {
    Brush(Option<(u8, u8, u8)>),
    Pen(Option<(u8, u8, u8)>, f64),
}

#[derive(Clone, Copy, PartialEq)]
enum PolyKind {
    Line,
    LineTo,
    Bezier,
    BezierTo,
    Polygon,
}

fn ru32(r: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(r.get(o..o + 4)?.try_into().unwrap()))
}
fn ri32(r: &[u8], o: usize) -> Option<i32> {
    Some(i32::from_le_bytes(r.get(o..o + 4)?.try_into().unwrap()))
}
fn ri16(r: &[u8], o: usize) -> Option<i16> {
    Some(i16::from_le_bytes(r.get(o..o + 2)?.try_into().unwrap()))
}
/// A GDI `COLORREF` (`0x00bbggrr`) → `(r, g, b)`.
fn colorref(v: u32) -> (u8, u8, u8) {
    ((v & 0xff) as u8, ((v >> 8) & 0xff) as u8, ((v >> 16) & 0xff) as u8)
}

/// Interpret a vector-only EMF (path drawing, solid fills/strokes) into a
/// self-contained SVG document. Photo EMFs go through [`emf_embedded_bitmap`]
/// instead; this is the fallback for metafiles that draw with paths.
///
/// **Fidelity honesty:** harmless state records (comments, text alignment, bk
/// mode, clip-region ops, …) are skipped, but any record that produces drawing
/// output we do not model — text (`EXT/SMALLTEXTOUT`), blits, gradients, region
/// fills, or any unrecognised opcode — makes the whole conversion return `None`,
/// so the slide keeps its honest "approximate preview" placeholder rather than
/// showing a partial image. Clipping (`SELECTCLIPPATH`) is intentionally ignored
/// (an accepted v1 approximation: the clip paths in practice bound the drawing,
/// so dropping them does not add visible content). All reads are bounds-checked;
/// malformed input returns `None`, never panics.
fn emf_vector_svg(b: &[u8]) -> Option<String> {
    // EMR_HEADER (iType 1) with the " EMF" signature at offset 40.
    if ru32(b, 0)? != 1 || b.get(40..44)? != b" EMF" {
        return None;
    }
    // rclBounds (device units) → SVG viewBox.
    let (bl, bt) = (ri32(b, 8)? as f64, ri32(b, 12)? as f64);
    let (br, bb) = (ri32(b, 16)? as f64, ri32(b, 20)? as f64);
    let (vw, vh) = (br - bl, bb - bt);
    if vw <= 0.0 || vh <= 0.0 || vw > 100_000.0 || vh > 100_000.0 {
        return None;
    }

    let mut st = EmfCtx::new();
    let mut off = 0usize;
    let mut records = 0u32;
    while off + 8 <= b.len() {
        let typ = ru32(b, off)?;
        let size = ru32(b, off + 4)? as usize;
        if size < 8 || !size.is_multiple_of(4) || off.checked_add(size)? > b.len() {
            return None; // malformed → honest drop
        }
        records += 1;
        if records > 500_000 {
            return None;
        }
        if !st.handle(typ, &b[off..off + size]) {
            return None; // unmodelled drawing output → honest drop
        }
        if typ == 14 {
            break; // EMR_EOF
        }
        off += size;
    }
    if st.body.is_empty() {
        return None; // nothing drawable — let the caller drop it
    }
    Some(format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="{} {} {} {}" width="{}" height="{}">{}</svg>"#,
        fnum(bl),
        fnum(bt),
        fnum(vw),
        fnum(vh),
        fnum(vw),
        fnum(vh),
        st.body
    ))
}

/// Mutable graphics state for [`emf_vector_svg`].
struct EmfCtx {
    win_org: (f64, f64),
    win_ext: (f64, f64),
    vp_org: (f64, f64),
    vp_ext: (f64, f64),
    map_mode: u32,
    objects: std::collections::HashMap<u32, EmfObj>,
    pen: Option<(u8, u8, u8)>,
    pen_w: f64,
    brush: Option<(u8, u8, u8)>,
    evenodd: bool,
    in_path: bool,
    d: String,
    cur: (f64, f64),
    body: String,
}

impl EmfCtx {
    fn new() -> Self {
        // GDI defaults: MM_TEXT, black pen, white brush, ALTERNATE fill mode.
        EmfCtx {
            win_org: (0.0, 0.0),
            win_ext: (1.0, 1.0),
            vp_org: (0.0, 0.0),
            vp_ext: (1.0, 1.0),
            map_mode: 1,
            objects: std::collections::HashMap::new(),
            pen: Some((0, 0, 0)),
            pen_w: 0.0,
            brush: Some((255, 255, 255)),
            evenodd: true,
            in_path: false,
            d: String::new(),
            cur: (0.0, 0.0),
        body: String::new(),
        }
    }

    /// The window→viewport scale (logical → device). Honored for the explicit
    /// map modes MM_ISOTROPIC (7) / MM_ANISOTROPIC (8); every other mode
    /// (MM_TEXT and the fixed metric modes) maps 1 logical unit → 1 device unit.
    fn scale(&self) -> (f64, f64) {
        match self.map_mode {
            7 | 8 => (
                if self.win_ext.0 != 0.0 { self.vp_ext.0 / self.win_ext.0 } else { 1.0 },
                if self.win_ext.1 != 0.0 { self.vp_ext.1 / self.win_ext.1 } else { 1.0 },
            ),
            _ => (1.0, 1.0),
        }
    }

    fn dev(&self, x: f64, y: f64) -> (f64, f64) {
        let (sx, sy) = self.scale();
        ((x - self.win_org.0) * sx + self.vp_org.0, (y - self.win_org.1) * sy + self.vp_org.1)
    }

    /// Emit `<path>` for the accumulated `frag`, honoring the current fill/stroke.
    fn draw_frag(&mut self, frag: &str, fill: bool, stroke: bool) {
        let d = frag.trim();
        if d.is_empty() {
            return;
        }
        let fill_attr = match (fill, self.brush) {
            (true, Some((r, g, b))) => format!(r##"fill="#{r:02x}{g:02x}{b:02x}""##),
            _ => "fill=\"none\"".to_string(),
        };
        let rule = if fill && self.evenodd { " fill-rule=\"evenodd\"" } else { "" };
        let stroke_attr = match (stroke, self.pen) {
            (true, Some((r, g, b))) => {
                let (sx, sy) = self.scale();
                let avg = ((sx.abs() + sy.abs()) / 2.0).max(1e-6);
                let w = if self.pen_w <= 0.0 { 0.75 } else { (self.pen_w * avg).max(0.3) };
                format!(r##"stroke="#{r:02x}{g:02x}{b:02x}" stroke-width="{}""##, fnum(w))
            }
            _ => "stroke=\"none\"".to_string(),
        };
        if fill_attr.ends_with("none\"") && stroke_attr.ends_with("none\"") {
            return; // fully invisible
        }
        self.body.push_str(&format!(r#"<path d="{d}" {fill_attr}{rule} {stroke_attr}/>"#));
    }

    /// Read a `count`-prefixed point array (`POINTL`/`POINTS`) and route it into
    /// the current path or a direct draw, per `kind`.
    fn poly(&mut self, r: &[u8], kind: PolyKind, sixteen: bool) -> bool {
        let count = match ru32(r, 24) {
            Some(c) => c as usize,
            None => return false,
        };
        if count > 200_000 {
            return false;
        }
        let stride = if sixteen { 4 } else { 8 };
        let mut pts = Vec::with_capacity(count);
        for i in 0..count {
            let o = 28 + i * stride;
            let p = if sixteen {
                match (ri16(r, o), ri16(r, o + 2)) {
                    (Some(x), Some(y)) => (x as i32, y as i32),
                    _ => return false,
                }
            } else {
                match (ri32(r, o), ri32(r, o + 4)) {
                    (Some(x), Some(y)) => (x, y),
                    _ => return false,
                }
            };
            pts.push(p);
        }
        self.emit_poly(&pts, kind);
        true
    }

    fn emit_poly(&mut self, pts: &[(i32, i32)], kind: PolyKind) {
        if pts.is_empty() {
            return;
        }
        let dev: Vec<(f64, f64)> = pts.iter().map(|p| self.dev(p.0 as f64, p.1 as f64)).collect();
        let starts_new =
            matches!(kind, PolyKind::Line | PolyKind::Polygon | PolyKind::Bezier);
        let mut frag = String::new();
        if starts_new {
            frag.push_str(&format!("M{} {} ", fnum(dev[0].0), fnum(dev[0].1)));
        } else if !self.in_path {
            // A "…To" variant drawn directly starts from the current point.
            frag.push_str(&format!("M{} {} ", fnum(self.cur.0), fnum(self.cur.1)));
        }
        let rest = if starts_new { &dev[1..] } else { &dev[..] };
        match kind {
            PolyKind::Line | PolyKind::LineTo | PolyKind::Polygon => {
                for &(x, y) in rest {
                    frag.push_str(&format!("L{} {} ", fnum(x), fnum(y)));
                }
            }
            PolyKind::Bezier | PolyKind::BezierTo => {
                for g in rest.chunks_exact(3) {
                    frag.push_str(&format!(
                        "C{} {} {} {} {} {} ",
                        fnum(g[0].0),
                        fnum(g[0].1),
                        fnum(g[1].0),
                        fnum(g[1].1),
                        fnum(g[2].0),
                        fnum(g[2].1),
                    ));
                }
            }
        }
        if kind == PolyKind::Polygon {
            frag.push_str("Z ");
        }
        if let Some(&last) = dev.last() {
            self.cur = last;
        }
        if self.in_path {
            self.d.push_str(&frag);
        } else if kind == PolyKind::Polygon {
            self.draw_frag(&frag, true, true);
        } else {
            self.draw_frag(&frag, false, true);
        }
    }

    /// `EMR_POLYPOLYLINE[16]` / `EMR_POLYPOLYGON[16]`: several sub-paths.
    fn polypoly(&mut self, r: &[u8], sixteen: bool, polygon: bool) -> bool {
        let npolys = match ru32(r, 24) {
            Some(n) => n as usize,
            None => return false,
        };
        if npolys == 0 || npolys > 100_000 {
            return false;
        }
        let mut counts = Vec::with_capacity(npolys);
        for i in 0..npolys {
            match ru32(r, 32 + i * 4) {
                Some(c) => counts.push(c as usize),
                None => return false,
            }
        }
        let stride = if sixteen { 4 } else { 8 };
        let mut pts_off = 32 + npolys * 4;
        let mut frag = String::new();
        for c in counts {
            if c == 0 || c > 100_000 {
                return false;
            }
            for i in 0..c {
                let o = pts_off + i * stride;
                let (x, y) = if sixteen {
                    match (ri16(r, o), ri16(r, o + 2)) {
                        (Some(x), Some(y)) => (x as i32, y as i32),
                        _ => return false,
                    }
                } else {
                    match (ri32(r, o), ri32(r, o + 4)) {
                        (Some(x), Some(y)) => (x, y),
                        _ => return false,
                    }
                };
                let (dx, dy) = self.dev(x as f64, y as f64);
                frag.push_str(&format!("{}{} {} ", if i == 0 { 'M' } else { 'L' }, fnum(dx), fnum(dy)));
                self.cur = (dx, dy);
            }
            if polygon {
                frag.push_str("Z ");
            }
            pts_off += c * stride;
        }
        if self.in_path {
            self.d.push_str(&frag);
        } else {
            self.draw_frag(&frag, polygon, true);
        }
        true
    }

    fn select_object(&mut self, r: &[u8]) -> bool {
        let Some(h) = ru32(r, 8) else { return false };
        if h & 0x8000_0000 != 0 {
            // Stock object.
            match h {
                0x8000_0000 => self.brush = Some((255, 255, 255)), // WHITE_BRUSH
                0x8000_0001 => self.brush = Some((192, 192, 192)), // LTGRAY_BRUSH
                0x8000_0002 => self.brush = Some((128, 128, 128)), // GRAY_BRUSH
                0x8000_0003 => self.brush = Some((64, 64, 64)),    // DKGRAY_BRUSH
                0x8000_0004 => self.brush = Some((0, 0, 0)),       // BLACK_BRUSH
                0x8000_0005 => self.brush = None,                  // NULL_BRUSH
                0x8000_0006 => self.pen = Some((255, 255, 255)),   // WHITE_PEN
                0x8000_0007 => self.pen = Some((0, 0, 0)),         // BLACK_PEN
                0x8000_0008 => self.pen = None,                    // NULL_PEN
                _ => {}                                            // fonts/palette: no effect
            }
            return true;
        }
        match self.objects.get(&h) {
            Some(EmfObj::Brush(c)) => self.brush = *c,
            Some(EmfObj::Pen(c, w)) => {
                self.pen = *c;
                self.pen_w = *w;
            }
            None => {} // selecting an undefined handle: ignore
        }
        true
    }

    fn handle(&mut self, typ: u32, r: &[u8]) -> bool {
        match typ {
            1 | 14 => true, // HEADER (already validated) / EOF
            // window / viewport mapping
            9 => match (ri32(r, 8), ri32(r, 12)) {
                (Some(x), Some(y)) => {
                    self.win_ext = (x as f64, y as f64);
                    true
                }
                _ => false,
            },
            10 => match (ri32(r, 8), ri32(r, 12)) {
                (Some(x), Some(y)) => {
                    self.win_org = (x as f64, y as f64);
                    true
                }
                _ => false,
            },
            11 => match (ri32(r, 8), ri32(r, 12)) {
                (Some(x), Some(y)) => {
                    self.vp_ext = (x as f64, y as f64);
                    true
                }
                _ => false,
            },
            12 => match (ri32(r, 8), ri32(r, 12)) {
                (Some(x), Some(y)) => {
                    self.vp_org = (x as f64, y as f64);
                    true
                }
                _ => false,
            },
            17 => match ru32(r, 8) {
                Some(m) => {
                    self.map_mode = m;
                    true
                }
                None => false,
            },
            19 => match ru32(r, 8) {
                // SETPOLYFILLMODE: 1 = ALTERNATE (evenodd), 2 = WINDING (nonzero)
                Some(m) => {
                    self.evenodd = m == 1;
                    true
                }
                None => false,
            },
            // path construction
            59 => {
                self.d.clear();
                self.in_path = true;
                true
            }
            60 => {
                self.in_path = false;
                true
            }
            61 => {
                self.d.push_str("Z ");
                true
            }
            27 => match (ri32(r, 8), ri32(r, 12)) {
                (Some(x), Some(y)) => {
                    let d = self.dev(x as f64, y as f64);
                    self.cur = d;
                    if self.in_path {
                        self.d.push_str(&format!("M{} {} ", fnum(d.0), fnum(d.1)));
                    }
                    true
                }
                _ => false,
            },
            54 => match (ri32(r, 8), ri32(r, 12)) {
                (Some(x), Some(y)) => {
                    let d = self.dev(x as f64, y as f64);
                    if self.in_path {
                        self.d.push_str(&format!("L{} {} ", fnum(d.0), fnum(d.1)));
                    } else {
                        let frag = format!(
                            "M{} {} L{} {} ",
                            fnum(self.cur.0),
                            fnum(self.cur.1),
                            fnum(d.0),
                            fnum(d.1)
                        );
                        self.draw_frag(&frag, false, true);
                    }
                    self.cur = d;
                    true
                }
                _ => false,
            },
            2 => self.poly(r, PolyKind::Bezier, false),
            3 => self.poly(r, PolyKind::Polygon, false),
            4 => self.poly(r, PolyKind::Line, false),
            5 => self.poly(r, PolyKind::BezierTo, false),
            6 => self.poly(r, PolyKind::LineTo, false),
            85 => self.poly(r, PolyKind::Bezier, true),
            86 => self.poly(r, PolyKind::Polygon, true),
            87 => self.poly(r, PolyKind::Line, true),
            88 => self.poly(r, PolyKind::BezierTo, true),
            89 => self.poly(r, PolyKind::LineTo, true),
            7 => self.polypoly(r, false, false),
            8 => self.polypoly(r, false, true),
            90 => self.polypoly(r, true, false),
            91 => self.polypoly(r, true, true),
            // painting
            62 => {
                let d = std::mem::take(&mut self.d);
                self.draw_frag(&d, true, false);
                true
            }
            63 => {
                let d = std::mem::take(&mut self.d);
                self.draw_frag(&d, true, true);
                true
            }
            64 => {
                let d = std::mem::take(&mut self.d);
                self.draw_frag(&d, false, true);
                true
            }
            67 | 68 => {
                // SELECTCLIPPATH (clip ignored) / ABORTPATH: consume the path.
                self.d.clear();
                self.in_path = false;
                true
            }
            // object table
            37 => self.select_object(r),
            38 => match (ru32(r, 8), ru32(r, 12), ri32(r, 16), ru32(r, 24)) {
                // CREATEPEN: PS_NULL (5) → no stroke; else solid.
                (Some(h), Some(style), Some(wx), Some(color)) => {
                    let c = if style & 0xf == 5 { None } else { Some(colorref(color)) };
                    self.objects.insert(h, EmfObj::Pen(c, (wx as f64).max(0.0)));
                    true
                }
                _ => false,
            },
            94 => match (ru32(r, 8), ru32(r, 28), ru32(r, 32), ru32(r, 36), ru32(r, 40)) {
                // EXTCREATEPEN: PS_NULL or a NULL brush → no stroke; else solid.
                (Some(h), Some(style), Some(wx), Some(bstyle), Some(color)) => {
                    let c = if style & 0xf == 5 || bstyle == 1 {
                        None
                    } else {
                        Some(colorref(color))
                    };
                    self.objects.insert(h, EmfObj::Pen(c, wx as f64));
                    true
                }
                _ => false,
            },
            39 => match (ru32(r, 8), ru32(r, 12), ru32(r, 16)) {
                // CREATEBRUSHINDIRECT: BS_NULL (1) → no fill; else solid.
                (Some(h), Some(style), Some(color)) => {
                    let c = if style == 1 { None } else { Some(colorref(color)) };
                    self.objects.insert(h, EmfObj::Brush(c));
                    true
                }
                _ => false,
            },
            40 => match ru32(r, 8) {
                Some(h) => {
                    if h & 0x8000_0000 == 0 {
                        self.objects.remove(&h);
                    }
                    true
                }
                None => false,
            },
            // Harmless state records — safe to ignore for a vector rendering:
            // SETBRUSHORGEX, SETMAPPERFLAGS, SETBKMODE, SETROP2,
            // SETSTRETCHBLTMODE, SETTEXTALIGN, SETTEXTCOLOR, SETBKCOLOR,
            // SAVEDC, RESTOREDC, SETMITERLIMIT, FLATTENPATH, WIDENPATH,
            // COMMENT, EXTSELECTCLIPRGN (clip ignored), EXTCREATEFONTINDIRECTW.
            13 | 16 | 18 | 20 | 21 | 22 | 24 | 25 | 33 | 34 | 58 | 65 | 66 | 70 | 75 | 82 => true,
            // Anything else may be drawing output we do not model → drop honestly.
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        colorref, decode_uncompressed_tiff, emf_embedded_bitmap, emf_vector_svg, mime_from_ext,
        parse_src_rect, raster_to_png, svg_blip_embed, svg_is_safe, wmf_embedded_bitmap,
    };
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

    /// Assemble a WMF from complete records: optional 22-byte Aldus placeable
    /// header, the 18-byte standard META_HEADER, the given records, then META_EOF.
    fn wmf(records: &[Vec<u8>], placeable: bool) -> Vec<u8> {
        let mut out = Vec::new();
        if placeable {
            let mut ph = vec![0u8; 22];
            ph[0..4].copy_from_slice(&0x9AC6_CDD7u32.to_le_bytes());
            out.extend_from_slice(&ph);
        }
        let mut hdr = vec![0u8; 18];
        hdr[0..2].copy_from_slice(&1u16.to_le_bytes()); // Type = MEMORYMETAFILE
        hdr[2..4].copy_from_slice(&9u16.to_le_bytes()); // HeaderSize (words)
        hdr[4..6].copy_from_slice(&0x0300u16.to_le_bytes()); // Version
        out.extend_from_slice(&hdr);
        for r in records {
            out.extend_from_slice(r);
        }
        let mut eof = Vec::new();
        eof.extend_from_slice(&3u32.to_le_bytes()); // RecordSize (words)
        eof.extend_from_slice(&0x0000u16.to_le_bytes()); // META_EOF
        out.extend_from_slice(&eof);
        out
    }

    /// A record with `func` and a fixed `params` prefix, followed by the packed
    /// `dib` running to the record end. `dib.len()` must keep the record
    /// word-aligned (the caller supplies even-length DIBs).
    fn wmf_bitmap_record(func: u16, params_fixed: usize, dib: &[u8]) -> Vec<u8> {
        let total = 6 + params_fixed + dib.len();
        assert_eq!(total % 2, 0, "record must be word-aligned");
        let mut r = Vec::new();
        r.extend_from_slice(&((total / 2) as u32).to_le_bytes());
        r.extend_from_slice(&func.to_le_bytes());
        r.extend_from_slice(&vec![0u8; params_fixed]);
        r.extend_from_slice(dib);
        r
    }

    // META_STRETCHDIB fixed params = RasterOp(4)+ColorUsage(2)+8×INT16(16) = 22.
    fn stretchdib(dib: &[u8]) -> Vec<u8> {
        wmf_bitmap_record(0x0F43, 22, dib)
    }
    // META_DIBSTRETCHBLT fixed params = RasterOp(4)+8×INT16(16) = 20.
    fn dibstretchblt(dib: &[u8]) -> Vec<u8> {
        wmf_bitmap_record(0x0B41, 20, dib)
    }

    #[test]
    fn wmf_extracts_embedded_jpeg_after_placeable_header() {
        // Packed DIB = 40-byte BITMAPINFOHEADER (BI_JPEG) + JPEG stream (even len).
        let jpeg = b"\xFF\xD8\xFFhello\xFF\xD9".to_vec();
        assert_eq!(jpeg.len() % 2, 0);
        let dib = [bmi(64, 64, 24, 4 /* BI_JPEG */), jpeg.clone()].concat();
        let wmf = wmf(&[stretchdib(&dib)], true /* placeable header present */);
        let (mime, data) = wmf_embedded_bitmap(&wmf).expect("bitmap found");
        assert_eq!(mime, "image/jpeg");
        assert_eq!(data, jpeg, "placeable header skipped, exact JPEG recovered");
    }

    #[test]
    fn wmf_wraps_rgb_dib_as_decodable_bmp() {
        // 1×1 24-bit BI_RGB in a DIBSTRETCHBLT, no placeable header.
        let dib = [bmi(1, 1, 24, 0 /* BI_RGB */), vec![0, 0, 255, 0]].concat();
        let wmf = wmf(&[dibstretchblt(&dib)], false);
        let (mime, data) = wmf_embedded_bitmap(&wmf).expect("bitmap found");
        assert_eq!(mime, "image/bmp");
        let img = image::load_from_memory(&data).expect("BMP decodes");
        assert_eq!((img.width(), img.height()), (1, 1));
    }

    #[test]
    fn wmf_picks_the_largest_of_several_dibs() {
        let small = [bmi(1, 1, 24, 5 /* BI_PNG */), b"\x89PNGsmall!".to_vec()].concat();
        let big_png = b"\x89PNGa much larger payload here!!".to_vec();
        assert_eq!(big_png.len() % 2, 0);
        let big = [bmi(1, 1, 24, 5 /* BI_PNG */), big_png.clone()].concat();
        let wmf = wmf(&[stretchdib(&small), dibstretchblt(&big)], false);
        let (mime, data) = wmf_embedded_bitmap(&wmf).expect("bitmap found");
        assert_eq!(mime, "image/png");
        assert_eq!(data, big_png, "larger DIB wins");
    }

    #[test]
    fn wmf_without_bitmap_yields_none() {
        // Header + EOF only (a vector-only metafile).
        assert!(wmf_embedded_bitmap(&wmf(&[], false)).is_none());
        assert!(wmf_embedded_bitmap(&wmf(&[], true)).is_none());
        // A non-bitmap record (META_LINETO, 0x0213) is ignored.
        let mut lineto = Vec::new();
        lineto.extend_from_slice(&5u32.to_le_bytes()); // 5 words
        lineto.extend_from_slice(&0x0213u16.to_le_bytes());
        lineto.extend_from_slice(&[0u8; 4]); // x, y
        assert!(wmf_embedded_bitmap(&wmf(&[lineto], false)).is_none());
        // Not a WMF at all.
        assert!(wmf_embedded_bitmap(b"not a wmf at all").is_none());
        assert!(wmf_embedded_bitmap(&[]).is_none());
    }

    #[test]
    fn wmf_malformed_records_do_not_panic() {
        let dib = [bmi(8, 8, 24, 0), vec![0u8; 8]].concat();
        let good = wmf(&[stretchdib(&dib)], true);
        // Every truncation prefix must return cleanly (None or Some), never panic.
        for n in 0..good.len() {
            let _ = wmf_embedded_bitmap(&good[..n]);
        }
        // A record claiming a preposterous size is rejected by the bounds check.
        let mut huge = Vec::new();
        huge.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes()); // size = 4G words
        huge.extend_from_slice(&0x0F43u16.to_le_bytes());
        assert!(wmf_embedded_bitmap(&wmf(&[huge], false)).is_none());
        // A DIB header claiming a size beyond the record is rejected by the split.
        let mut bad_dib = bmi(1, 1, 24, 0);
        bad_dib[0..4].copy_from_slice(&9999u32.to_le_bytes()); // biSize ≫ payload
        let rec = stretchdib(&[bad_dib.as_slice(), &[0u8, 0]].concat());
        assert!(wmf_embedded_bitmap(&wmf(&[rec], false)).is_none());
    }

    #[test]
    fn tiff_decodes_to_png() {
        // Encode a tiny 2×2 TIFF in memory (exercises the "tiff" image feature),
        // then run it through the TIFF → PNG transcode.
        let src = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(2, 2, |x, y| {
            image::Rgb([(x * 100) as u8, (y * 100) as u8, 30])
        }));
        let mut tiff = Vec::new();
        src.write_to(&mut std::io::Cursor::new(&mut tiff), image::ImageFormat::Tiff)
            .expect("encode TIFF");
        assert_eq!(image::guess_format(&tiff).unwrap(), image::ImageFormat::Tiff);
        let png = raster_to_png(&tiff).expect("TIFF transcodes to PNG");
        let out = image::load_from_memory(&png).expect("PNG decodes");
        assert_eq!(image::guess_format(&png).unwrap(), image::ImageFormat::Png);
        assert_eq!((out.width(), out.height()), (2, 2));
        assert!(raster_to_png(b"not an image").is_none());
    }

    #[test]
    fn mime_from_ext_claims_tiff_wmf_but_not_wdp() {
        assert_eq!(mime_from_ext("a/b/x.tiff").as_deref(), Some("image/tiff"));
        assert_eq!(mime_from_ext("x.TIF").as_deref(), Some("image/tiff"));
        assert_eq!(mime_from_ext("x.wmf").as_deref(), Some("image/x-wmf"));
        // WDP (JPEG XR) must stay unclaimed → lands on the placeholder.
        assert!(mime_from_ext("x.wdp").is_none());
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

    // --- TIFF decoder -------------------------------------------------------

    /// Assemble a baseline TIFF from strips and IFD tags. Width/length (256/257)
    /// and StripOffsets/StripByteCounts (273/279) are injected automatically;
    /// `le` picks the byte order. Out-of-line arrays are appended after the IFD.
    fn build_tiff(
        le: bool,
        width: u32,
        height: u32,
        strips: &[Vec<u8>],
        mut tags: Vec<(u16, u16, Vec<u32>)>,
    ) -> Vec<u8> {
        let u16b = move |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let u32b = move |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
        let tsize = |t: u16| -> usize {
            match t {
                1 | 2 => 1,
                3 => 2,
                4 => 4,
                _ => panic!("unsupported field type"),
            }
        };
        let mut out = Vec::new();
        out.extend_from_slice(if le { b"II" } else { b"MM" });
        out.extend_from_slice(&u16b(42));
        out.extend_from_slice(&[0u8; 4]); // IFD offset — patched below.
        let mut soff = Vec::new();
        let mut scnt = Vec::new();
        for s in strips {
            soff.push(out.len() as u32);
            scnt.push(s.len() as u32);
            out.extend_from_slice(s);
        }
        tags.push((256, 3, vec![width]));
        tags.push((257, 3, vec![height]));
        tags.push((273, 4, soff));
        tags.push((279, 4, scnt));
        tags.sort_by_key(|t| t.0);
        let ifd_off = out.len() as u32;
        out[4..8].copy_from_slice(&u32b(ifd_off));
        let ifd_size = 2 + 12 * tags.len() + 4;
        let extras_start = ifd_off as usize + ifd_size;
        let mut ifd = Vec::new();
        let mut extras = Vec::new();
        ifd.extend_from_slice(&u16b(tags.len() as u16));
        for (tag, typ, vals) in &tags {
            ifd.extend_from_slice(&u16b(*tag));
            ifd.extend_from_slice(&u16b(*typ));
            ifd.extend_from_slice(&u32b(vals.len() as u32));
            let total = tsize(*typ) * vals.len();
            let encode = |dst: &mut Vec<u8>, v: u32| match tsize(*typ) {
                1 => dst.push(v as u8),
                2 => dst.extend_from_slice(&u16b(v as u16)),
                _ => dst.extend_from_slice(&u32b(v)),
            };
            if total <= 4 {
                let mut field = Vec::new();
                for v in vals {
                    encode(&mut field, *v);
                }
                field.resize(4, 0);
                ifd.extend_from_slice(&field);
            } else {
                ifd.extend_from_slice(&u32b((extras_start + extras.len()) as u32));
                for v in vals {
                    encode(&mut extras, *v);
                }
            }
        }
        ifd.extend_from_slice(&u32b(0)); // next IFD
        out.extend_from_slice(&ifd);
        out.extend_from_slice(&extras);
        out
    }

    /// A 3-entry (red, green, blue) colormap laid out TIFF-style
    /// (all reds, then all greens, then all blues), 16-bit per channel.
    fn rgb_colormap() -> Vec<u32> {
        vec![65535, 0, 0, /* R */ 0, 65535, 0, /* G */ 0, 0, 65535 /* B */]
    }

    fn decode_png(png: &[u8]) -> image::RgbaImage {
        image::load_from_memory(png).expect("png decodes").to_rgba8()
    }

    #[test]
    fn palette_tiff_round_trips_both_endian() {
        // 2×2 palette image, indices [0,1 / 2,0] → red, green / blue, red.
        for le in [true, false] {
            let tiff = build_tiff(
                le,
                2,
                2,
                &[vec![0, 1, 2, 0]],
                vec![
                    (258, 3, vec![8]),          // BitsPerSample
                    (259, 3, vec![1]),          // Compression = none
                    (262, 3, vec![3]),          // Photometric = palette
                    (277, 3, vec![1]),          // SamplesPerPixel
                    (278, 3, vec![2]),          // RowsPerStrip
                    (320, 3, rgb_colormap()),   // ColorMap
                ],
            );
            let png = decode_uncompressed_tiff(&tiff).expect("palette decodes");
            let img = decode_png(&png);
            assert_eq!(img.dimensions(), (2, 2), "le={le}");
            assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255], "le={le}");
            assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0, 255], "le={le}");
            assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255, 255], "le={le}");
            assert_eq!(img.get_pixel(1, 1).0, [255, 0, 0, 255], "le={le}");
        }
    }

    #[test]
    fn palette_tiff_4bit() {
        // 3×1 4-bit palette: two indices per byte, row padded to a whole byte.
        // Pixels 0,1,2 → 0x01, 0x2_ → red, green, blue.
        let tiff = build_tiff(
            true,
            3,
            1,
            &[vec![0x01, 0x20]],
            vec![
                (258, 3, vec![4]),
                (259, 3, vec![1]),
                (262, 3, vec![3]),
                (277, 3, vec![1]),
                (320, 3, rgb_colormap()),
            ],
        );
        let img = decode_png(&decode_uncompressed_tiff(&tiff).expect("4-bit palette decodes"));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0, 255]);
        assert_eq!(img.get_pixel(2, 0).0, [0, 0, 255, 255]);
    }

    #[test]
    fn rgba_tiff_preserves_alpha() {
        // 2×1 RGBA, second sample-set half transparent.
        let strip = vec![10, 20, 30, 255, 40, 50, 60, 128];
        let tiff = build_tiff(
            false,
            2,
            1,
            &[strip],
            vec![
                (258, 3, vec![8, 8, 8, 8]),
                (259, 3, vec![1]),
                (262, 3, vec![2]),   // RGB
                (277, 3, vec![4]),   // 4 samples
                (338, 3, vec![2]),   // ExtraSamples = unassociated alpha
            ],
        );
        let img = decode_png(&decode_uncompressed_tiff(&tiff).expect("rgba decodes"));
        assert_eq!(img.get_pixel(0, 0).0, [10, 20, 30, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [40, 50, 60, 128]);
    }

    #[test]
    fn multi_strip_rgb_tiff() {
        // 1×2 RGB split across two single-row strips.
        let tiff = build_tiff(
            true,
            1,
            2,
            &[vec![1, 2, 3], vec![4, 5, 6]],
            vec![
                (258, 3, vec![8, 8, 8]),
                (259, 3, vec![1]),
                (262, 3, vec![2]),
                (277, 3, vec![3]),
                (278, 3, vec![1]), // RowsPerStrip = 1 → two strips
            ],
        );
        let img = decode_png(&decode_uncompressed_tiff(&tiff).expect("multi-strip decodes"));
        assert_eq!(img.get_pixel(0, 0).0, [1, 2, 3, 255]);
        assert_eq!(img.get_pixel(0, 1).0, [4, 5, 6, 255]);
    }

    #[test]
    fn palette_index_out_of_range_is_clamped() {
        // 8-bit index 5 against a 2-entry colormap (black, red): clamps to the
        // last entry (red) rather than reading out of bounds.
        let tiff = build_tiff(
            true,
            1,
            1,
            &[vec![5]],
            vec![
                (258, 3, vec![8]),
                (259, 3, vec![1]),
                (262, 3, vec![3]),
                (277, 3, vec![1]),
                (320, 3, vec![0, 65535, 0, 0, 0, 0]), // R=[0,65535] G=[0,0] B=[0,0]
            ],
        );
        let img = decode_png(&decode_uncompressed_tiff(&tiff).expect("clamped decode"));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn compressed_tiff_is_rejected() {
        // The FALLBACK decoder only handles Compression = 1; anything else
        // (here: a claimed LZW, 5) must yield None. Compressed TIFFs that are
        // actually well-formed are the primary decoder's (`raster_to_png`) job —
        // this guards the fallback against decoding data it can't interpret.
        let tiff = build_tiff(
            true,
            1,
            1,
            &[vec![0]],
            vec![
                (258, 3, vec![8]),
                (259, 3, vec![5]), // LZW
                (262, 3, vec![3]),
                (277, 3, vec![1]),
                (320, 3, rgb_colormap()),
            ],
        );
        assert!(decode_uncompressed_tiff(&tiff).is_none());
    }

    #[test]
    fn truncated_tiff_never_panics() {
        let tiff = build_tiff(
            true,
            2,
            2,
            &[vec![0, 1, 2, 0]],
            vec![
                (258, 3, vec![8]),
                (259, 3, vec![1]),
                (262, 3, vec![3]),
                (277, 3, vec![1]),
                (320, 3, rgb_colormap()),
            ],
        );
        for n in 0..tiff.len() {
            let _ = decode_uncompressed_tiff(&tiff[..n]);
            let _ = raster_to_png(&tiff[..n]); // primary decoder must not panic either
        }
        // Hostile inputs.
        assert!(decode_uncompressed_tiff(b"II*\0\xff\xff\xff\xff").is_none());
        assert!(decode_uncompressed_tiff(b"").is_none());
    }

    /// A little-endian TIFF whose IFD entries are written verbatim — each is
    /// `(tag, field-type, count, 4-byte value/offset)` — with no strip payload.
    /// Lets a test set a hostile `count` the honest `build_tiff` (which derives
    /// counts from real arrays) can't express.
    fn tiff_with_raw_ifd(entries: &[(u16, u16, u32, u32)]) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"II");
        out.extend_from_slice(&42u16.to_le_bytes());
        out.extend_from_slice(&8u32.to_le_bytes()); // IFD at offset 8
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        for (tag, typ, count, val) in entries {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&typ.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            out.extend_from_slice(&val.to_le_bytes());
        }
        out.extend_from_slice(&0u32.to_le_bytes()); // next IFD
        out
    }

    #[test]
    fn hostile_tiff_giant_ifd_count_rejected_without_oom() {
        // Regression: the `values` closure did `Vec::with_capacity(count)` with a
        // count read straight from the file. A StripOffsets entry claiming ~4
        // billion LONGs (0xFFFFFFFF × 4 bytes ≈ 17 GB) must now bail — the count
        // exceeds the file length — returning None cheaply instead of aborting.
        let tiff = tiff_with_raw_ifd(&[
            (256, 3, 1, 2),                    // ImageWidth = 2
            (257, 3, 1, 2),                    // ImageLength = 2
            (258, 3, 1, 8),                    // BitsPerSample = 8
            (259, 3, 1, 1),                    // Compression = none
            (262, 3, 1, 2),                    // Photometric = RGB
            (277, 3, 1, 3),                    // SamplesPerPixel = 3
            (273, 4, 0xFFFF_FFFF, 8),          // StripOffsets: absurd count
            (279, 4, 1, 12),                   // StripByteCounts = 12
        ]);
        assert!(decode_uncompressed_tiff(&tiff).is_none());

        // The same guard on a bloated ColorMap (palette photometric).
        let tiff = tiff_with_raw_ifd(&[
            (256, 3, 1, 2),
            (257, 3, 1, 2),
            (258, 3, 1, 8),
            (259, 3, 1, 1),
            (262, 3, 1, 3),                    // Photometric = palette
            (277, 3, 1, 1),
            (273, 4, 1, 8),
            (279, 4, 1, 4),
            (320, 3, 0xFFFF_FFFF, 8),          // ColorMap: absurd count
        ]);
        assert!(decode_uncompressed_tiff(&tiff).is_none());
    }

    #[test]
    fn hostile_tiff_strip_flood_rejected_without_oom() {
        // Regression: strips were bounds-checked individually but their SUM was
        // unbounded — a million strips each claiming the whole file would `extend`
        // terabytes into `raw`. A 2-row image listing 300k strips has more strips
        // than rows, so it must be rejected up front (no allocation, no panic).
        let n = 300_000u32;
        let mut entries = vec![
            (256u16, 3u16, 1u32, 2u32),        // ImageWidth = 2
            (257, 3, 1, 2),                    // ImageLength = 2 (only 2 rows!)
            (258, 3, 1, 8),
            (259, 3, 1, 1),
            (262, 3, 1, 2),                    // RGB
            (277, 3, 1, 3),
        ];
        // Out-of-line StripOffsets/StripByteCounts arrays of `n` entries each,
        // appended after the IFD; every strip points at (and claims) the header.
        let ifd_entry_count = (entries.len() + 2) as u32; // + the two strip tags
        let ifd_start = 8u32;
        let ifd_size = 2 + 12 * ifd_entry_count + 4;
        let arr1_off = ifd_start + ifd_size;
        let arr2_off = arr1_off + 4 * n;
        entries.push((273, 4, n, arr1_off));
        entries.push((279, 4, n, arr2_off));
        entries.sort_by_key(|e| e.0);

        let mut tiff = tiff_with_raw_ifd(&entries);
        // The raw builder wrote the IFD but not the out-of-line arrays; append them.
        for _ in 0..n {
            tiff.extend_from_slice(&0u32.to_le_bytes()); // StripOffsets → 0
        }
        for _ in 0..n {
            tiff.extend_from_slice(&8u32.to_le_bytes()); // StripByteCounts → 8
        }
        assert!(decode_uncompressed_tiff(&tiff).is_none());
    }

    // --- EMF vector interpreter --------------------------------------------

    fn emf_header(w: i32, h: i32) -> Vec<u8> {
        let mut hdr = vec![0u8; 88];
        hdr[0..4].copy_from_slice(&1u32.to_le_bytes()); // EMR_HEADER
        hdr[4..8].copy_from_slice(&88u32.to_le_bytes());
        hdr[16..20].copy_from_slice(&w.to_le_bytes()); // rclBounds.right
        hdr[20..24].copy_from_slice(&h.to_le_bytes()); // rclBounds.bottom
        hdr[40..44].copy_from_slice(b" EMF");
        hdr
    }

    fn emf_eof() -> Vec<u8> {
        let mut e = vec![0u8; 20];
        e[0..4].copy_from_slice(&14u32.to_le_bytes());
        e[4..8].copy_from_slice(&20u32.to_le_bytes());
        e
    }

    /// A single EMR record: iType + size + payload (size padded to 4 bytes).
    fn emr(typ: u32, payload: &[u8]) -> Vec<u8> {
        let size = 8 + payload.len();
        assert_eq!(size % 4, 0, "record size must be 4-aligned");
        let mut r = Vec::with_capacity(size);
        r.extend_from_slice(&typ.to_le_bytes());
        r.extend_from_slice(&(size as u32).to_le_bytes());
        r.extend_from_slice(payload);
        r
    }

    fn u32s(vals: &[u32]) -> Vec<u8> {
        vals.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    /// A poly16 payload: 16-byte rclBounds, u32 point count, then i16 pairs.
    fn poly16(points: &[(i16, i16)]) -> Vec<u8> {
        let mut p = vec![0u8; 16];
        p.extend_from_slice(&(points.len() as u32).to_le_bytes());
        for (x, y) in points {
            p.extend_from_slice(&x.to_le_bytes());
            p.extend_from_slice(&y.to_le_bytes());
        }
        while !p.len().is_multiple_of(4) {
            p.push(0);
        }
        p
    }

    fn brush(ih: u32, color: u32) -> Vec<u8> {
        emr(39, &u32s(&[ih, 0 /* BS_SOLID */, color, 0]))
    }

    #[test]
    fn emf_filled_polygon16_direct() {
        // Green triangle drawn directly (outside a path) via POLYGON16.
        let mut emf = emf_header(100, 100);
        emf.extend(brush(1, 0x0000_ff00)); // COLORREF 0x00bbggrr → (0,255,0)
        emf.extend(emr(37, &u32s(&[1]))); // SELECTOBJECT ih=1
        emf.extend(emr(86, &poly16(&[(10, 10), (90, 10), (50, 90)])));
        emf.extend(emf_eof());
        let svg = emf_vector_svg(&emf).expect("vector svg");
        assert!(svg.contains(r#"viewBox="0 0 100 100""#), "{svg}");
        assert!(svg.contains("M10 10 L90 10 L50 90 Z"), "{svg}");
        assert!(svg.contains(r##"fill="#00ff00""##), "{svg}");
    }

    #[test]
    fn emf_fillpath_with_winding_rule() {
        // Path + FILLPATH, WINDING fill mode → no evenodd fill-rule.
        let mut emf = emf_header(100, 100);
        emf.extend(brush(1, 0x0000_00ff)); // (255,0,0)
        emf.extend(emr(37, &u32s(&[1])));
        emf.extend(emr(19, &u32s(&[2]))); // SETPOLYFILLMODE WINDING
        emf.extend(emr(59, &[])); // BEGINPATH
        emf.extend(emr(27, &u32s(&[10, 10]))); // MOVETOEX
        emf.extend(emr(89, &poly16(&[(90, 10), (50, 90)]))); // POLYLINETO16
        emf.extend(emr(61, &[])); // CLOSEFIGURE
        emf.extend(emr(60, &[])); // ENDPATH
        emf.extend(emr(62, &[0u8; 16])); // FILLPATH (rclBounds)
        emf.extend(emf_eof());
        let svg = emf_vector_svg(&emf).expect("vector svg");
        assert!(svg.contains("M10 10 L90 10 L50 90 Z"), "{svg}");
        assert!(svg.contains(r##"fill="#ff0000""##), "{svg}");
        assert!(!svg.contains("evenodd"), "winding must not emit evenodd: {svg}");
    }

    #[test]
    fn emf_polybezier_path() {
        let mut emf = emf_header(100, 100);
        emf.extend(brush(1, 0x0000_0000));
        emf.extend(emr(37, &u32s(&[1])));
        emf.extend(emr(59, &[]));
        emf.extend(emr(27, &u32s(&[0, 0]))); // MOVETOEX 0,0
        emf.extend(emr(88, &poly16(&[(10, 0), (20, 10), (30, 30)]))); // POLYBEZIERTO16
        emf.extend(emr(60, &[]));
        emf.extend(emr(62, &[0u8; 16]));
        emf.extend(emf_eof());
        let svg = emf_vector_svg(&emf).expect("vector svg");
        assert!(svg.contains("C10 0 20 10 30 30"), "{svg}");
    }

    #[test]
    fn emf_null_brush_strokes_only() {
        // Solid blue pen + stock NULL_BRUSH → stroke only, no fill.
        let mut emf = emf_header(100, 100);
        // CREATEPEN ih=1, PS_SOLID, width 2, color (0,0,255).
        emf.extend(emr(38, &u32s(&[1, 0, 2, 0, 0x00ff_0000])));
        emf.extend(emr(37, &u32s(&[1]))); // select pen
        emf.extend(emr(37, &u32s(&[0x8000_0005]))); // select NULL_BRUSH
        emf.extend(emr(86, &poly16(&[(10, 10), (90, 10), (50, 90)])));
        emf.extend(emf_eof());
        let svg = emf_vector_svg(&emf).expect("vector svg");
        assert!(svg.contains(r#"fill="none""#), "{svg}");
        assert!(svg.contains(r##"stroke="#0000ff""##), "{svg}");
        assert!(svg.contains(r#"stroke-width="2""#), "{svg}");
    }

    #[test]
    fn emf_unknown_drawing_record_drops() {
        // A fill followed by an EXTTEXTOUTW (text output we do not model): the
        // whole conversion must return None so the slide keeps its placeholder,
        // rather than silently omitting the text.
        let mut emf = emf_header(100, 100);
        emf.extend(brush(1, 0x0000_ff00));
        emf.extend(emr(37, &u32s(&[1])));
        emf.extend(emr(86, &poly16(&[(0, 0), (10, 0), (5, 10)])));
        emf.extend(emr(84, &[0u8; 32])); // EMR_EXTTEXTOUTW
        emf.extend(emf_eof());
        assert!(emf_vector_svg(&emf).is_none());
    }

    #[test]
    fn emf_vector_svg_truncation_never_panics() {
        let mut emf = emf_header(100, 100);
        emf.extend(brush(1, 0x0000_ff00));
        emf.extend(emr(37, &u32s(&[1])));
        emf.extend(emr(86, &poly16(&[(10, 10), (90, 10), (50, 90)])));
        emf.extend(emf_eof());
        for n in 0..emf.len() {
            let _ = emf_vector_svg(&emf[..n]);
        }
        assert!(emf_vector_svg(b"not an emf").is_none());
    }

    #[test]
    fn emf_without_drawing_yields_none() {
        // Header + a harmless comment + EOF: nothing drawable → None.
        let mut emf = emf_header(50, 50);
        emf.extend(emr(70, &[0u8; 8])); // EMR_COMMENT
        emf.extend(emf_eof());
        assert!(emf_vector_svg(&emf).is_none());
    }

    #[test]
    fn colorref_channel_order() {
        // 0x00bbggrr → (r, g, b).
        assert_eq!(colorref(0x0011_2233), (0x33, 0x22, 0x11));
    }
}
