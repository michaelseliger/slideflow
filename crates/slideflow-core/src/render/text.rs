//! Text bodies: paragraph collection, greedy word-wrap, and SVG `<text>` output.

use roxmltree::Node;

use super::color::Rgba;
use super::geometry::Rect;
use super::placeholder::Placeholder;
use super::{a, ch, esc, fnum, Ctx, L_INS, LVL_INDENT, R_INS, T_INS};

struct Line {
    text: String,
    size: f64,
    bold: bool,
    italic: bool,
    color: Rgba,
    algn: String,
    indent: f64,
    line_height: f64,
}

impl Ctx<'_> {
    pub(crate) fn render_text(&mut self, sp: Node, tx_body: Node, rect: &Rect, ph: Option<&Placeholder>) {
        let is_title = ph
            .and_then(|p| p.typ.as_deref())
            .map(|t| t == "title" || t == "ctrTitle")
            .unwrap_or(false);
        let is_body_ph = ph.is_some() && !is_title;
        let default_sz = if is_title { 44.0 } else { 18.0 };
        let font = if is_title {
            &self.theme.major_font
        } else {
            &self.theme.minor_font
        };
        let font_family = font_family(font);

        let body_pr = ch(tx_body, "bodyPr");
        let anchor = body_pr
            .and_then(|b| a(b, "anchor"))
            .unwrap_or(if ph.and_then(|p| p.typ.as_deref()) == Some("ctrTitle") {
                "ctr"
            } else {
                "t"
            })
            .to_string();

        // Collect wrapped lines with per-line style.
        let mut lines: Vec<Line> = Vec::new();
        for para in tx_body.children().filter(|n| n.is_element() && n.tag_name().name() == "p") {
            self.collect_paragraph(para, rect, is_title, is_body_ph, default_sz, &mut lines);
        }
        if lines.is_empty() {
            return;
        }

        let total_h: f64 = lines.iter().map(|l| l.line_height).sum();
        let block_top = match anchor.as_str() {
            "ctr" => rect.y + (rect.h - total_h) / 2.0,
            "b" => rect.y + rect.h - total_h - T_INS,
            _ => rect.y + T_INS,
        };

        // Clip text to the shape.
        self.clip_id += 1;
        let clip = format!("clip{}", self.clip_id);
        self.defs.push_str(&format!(
            r#"<clipPath id="{clip}"><rect x="{x}" y="{y}" width="{w}" height="{h}"/></clipPath>"#,
            clip = clip,
            x = fnum(rect.x),
            y = fnum(rect.y),
            w = fnum(rect.w),
            h = fnum(rect.h)
        ));
        self.body.push_str(&format!(r#"<g clip-path="url(#{clip})">"#));

        let mut cursor = block_top;
        for line in &lines {
            let baseline = cursor + line.size * 0.8;
            let (anchor_attr, tx) = match line.algn.as_str() {
                "ctr" => ("middle", rect.x + rect.w / 2.0),
                "r" => ("end", rect.x + rect.w - R_INS),
                _ => ("start", rect.x + L_INS + line.indent),
            };
            let mut style = String::new();
            if line.bold {
                style.push_str(r#" font-weight="bold""#);
            }
            if line.italic {
                style.push_str(r#" font-style="italic""#);
            }
            self.body.push_str(&format!(
                r#"<text x="{x}" y="{y}" font-family="{ff}" font-size="{sz}" fill="{fill}" text-anchor="{anchor}"{style}>{text}</text>"#,
                x = fnum(tx),
                y = fnum(baseline),
                ff = font_family,
                sz = fnum(line.size),
                fill = line.color.hex(),
                anchor = anchor_attr,
                style = style,
                text = esc(&line.text)
            ));
            cursor += line.line_height;
        }
        self.body.push_str("</g>");
        let _ = sp;
    }

    fn collect_paragraph(
        &self,
        para: Node,
        rect: &Rect,
        is_title: bool,
        is_body_ph: bool,
        default_sz: f64,
        out: &mut Vec<Line>,
    ) {
        let p_pr = ch(para, "pPr");
        let lvl = p_pr
            .and_then(|p| a(p, "lvl"))
            .and_then(|v| v.parse::<f64>().ok())
            .unwrap_or(0.0);
        let algn = p_pr
            .and_then(|p| a(p, "algn"))
            .unwrap_or("l")
            .to_string();
        let bu_none = p_pr
            .map(|p| p.children().any(|n| n.is_element() && n.tag_name().name() == "buNone"))
            .unwrap_or(false);

        // Gather run texts and take style from the first run.
        let mut text = String::new();
        let mut size = default_sz;
        let mut bold = false;
        let mut italic = false;
        let mut color = self.theme.text_default();
        let mut first_run = true;
        for child in para.children().filter(|n| n.is_element()) {
            match child.tag_name().name() {
                "r" => {
                    let r_pr = ch(child, "rPr");
                    if first_run {
                        if let Some(rp) = r_pr {
                            if let Some(sz) = a(rp, "sz").and_then(|v| v.parse::<f64>().ok()) {
                                size = sz / 100.0;
                            }
                            bold = a(rp, "b") == Some("1");
                            italic = a(rp, "i") == Some("1");
                            if let Some(c) = ch(rp, "solidFill")
                                .and_then(|f| f.children().find(|n| n.is_element()))
                                .and_then(|cn| self.theme.parse_color(cn))
                            {
                                color = c;
                            }
                        }
                        first_run = false;
                    }
                    if let Some(t) = ch(child, "t") {
                        text.push_str(t.text().unwrap_or(""));
                    }
                }
                "br" => text.push('\n'),
                "fld" => {
                    if let Some(t) = ch(child, "t") {
                        text.push_str(t.text().unwrap_or(""));
                    }
                }
                _ => {}
            }
        }

        let indent = lvl * LVL_INDENT;
        let bullet = is_body_ph && !is_title && !bu_none;
        let avail = (rect.w - L_INS - R_INS - indent).max(size); // never below one glyph
        let line_height = size * 1.2;

        if text.trim().is_empty() {
            // Preserve empty paragraphs as vertical space.
            out.push(Line {
                text: String::new(),
                size,
                bold,
                italic,
                color,
                algn: algn.clone(),
                indent,
                line_height,
            });
            return;
        }

        // Honor explicit line breaks, then wrap each segment.
        for (seg_idx, segment) in text.split('\n').enumerate() {
            let wrapped = wrap(segment, size, avail);
            for (wi, wl) in wrapped.iter().enumerate() {
                let prefix = if bullet && seg_idx == 0 && wi == 0 { "• " } else { "" };
                out.push(Line {
                    text: format!("{prefix}{wl}"),
                    size,
                    bold,
                    italic,
                    color,
                    algn: algn.clone(),
                    indent,
                    line_height,
                });
            }
        }
    }
}

/// Greedy word wrap using an average-glyph-width heuristic (~0.52em).
pub(crate) fn wrap(text: &str, font_size: f64, avail_width: f64) -> Vec<String> {
    let char_w = 0.52 * font_size;
    let max_chars = ((avail_width / char_w).floor() as usize).max(1);
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_len = 0usize;

    for word in text.split_whitespace() {
        let wlen = word.chars().count();
        if wlen > max_chars {
            // Flush the current line, then hard-break the long word.
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_len = 0;
            }
            let mut chunk = String::new();
            for c in word.chars() {
                chunk.push(c);
                if chunk.chars().count() >= max_chars {
                    lines.push(std::mem::take(&mut chunk));
                }
            }
            if !chunk.is_empty() {
                current = chunk;
                current_len = current.chars().count();
            }
            continue;
        }
        let extra = if current.is_empty() { wlen } else { wlen + 1 };
        if current_len + extra > max_chars && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
        if current.is_empty() {
            current.push_str(word);
            current_len = wlen;
        } else {
            current.push(' ');
            current.push_str(word);
            current_len += wlen + 1;
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn font_family(font: &str) -> String {
    format!("{}, Helvetica, Arial, sans-serif", esc(font))
}
