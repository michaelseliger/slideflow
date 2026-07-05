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
        // EMF can't be rendered by browsers, but photo EMFs are metafile
        // wrappers around one big embedded bitmap — extract and embed that. A
        // vector-only EMF yields nothing, so count it as an unsupported image.
        if ct == "image/x-emf" || ct == "image/emf" {
            return self.emf_data_uri(bytes).or_else(|| {
                self.record_drop("unsupported-image");
                None
            });
        }
        // TIFF: browsers can't display it, but it's a plain raster — decode and
        // re-encode as PNG (applying the thumbnail cap). Only a genuine decode
        // failure counts as an unsupported image.
        if ct == "image/tiff" {
            return match raster_to_png(bytes) {
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
    use super::{
        emf_embedded_bitmap, mime_from_ext, parse_src_rect, raster_to_png, svg_blip_embed,
        svg_is_safe, wmf_embedded_bitmap,
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
}
