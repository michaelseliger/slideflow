//! Text bodies: per-run spans, the style-inheritance chain, span-aware
//! word-wrap, bullets, spacing/insets/autofit, and SVG `<text>` output.

use roxmltree::Node;

use super::color::Rgba;
use super::geometry::Rect;
use super::placeholder::{match_placeholder, Placeholder};
use super::style::{parse_ppr, parse_rpr, Bullet, LstStyle, ParaProps, RunProps, Spacing};
use super::{a, ch, esc, fnum, Ctx, EMU_PER_PT, LVL_INDENT};

/// One styled run fragment on a line.
#[derive(Clone)]
struct Span {
    text: String,
    size_pt: f64,
    bold: bool,
    italic: bool,
    underline: bool,
    color: Rgba,
    typeface: Option<String>,
    /// `a:highlight` marker color — a box drawn behind the line.
    highlight: Option<Rgba>,
}

impl Span {
    fn from_props(text: String, p: &RunProps, default_size: f64, default_color: Rgba, scale: f64) -> Span {
        Span {
            text,
            size_pt: p.size_pt.unwrap_or(default_size) * scale,
            bold: p.bold.unwrap_or(false),
            italic: p.italic.unwrap_or(false),
            underline: p.underline.unwrap_or(false),
            color: p.color.unwrap_or(default_color),
            typeface: p.typeface.clone(),
            highlight: p.highlight,
        }
    }
}

/// One visual line, ready to place: its spans, alignment, left inset (points from
/// the shape's left edge), vertical advance, and any gap preceding it.
struct Line {
    spans: Vec<Span>,
    algn: String,
    left: f64,
    advance: f64,
    space_before: f64,
}

/// Body-text autofit knobs from `a:bodyPr` (`normAutofit`).
struct Autofit {
    font_scale: f64,
    ln_spc_reduction: f64,
}

/// The result of the (pure) layout stage: placed lines plus the body-level
/// knobs the emit stage and the autofit measurer both need.
struct TextLayout {
    lines: Vec<Line>,
    anchor: String,
    t_ins: f64,
    b_ins: f64,
    r_ins: f64,
    wrap_none: bool,
}

impl Ctx<'_> {
    pub(crate) fn render_text(&mut self, sp: Option<Node>, tx_body: Node, rect: &Rect, ph: Option<&Placeholder>) {
        let tl = self.layout_text(sp, tx_body, rect, ph);
        if tl.lines.is_empty() {
            return;
        }

        let total_h: f64 = tl.lines.iter().map(|l| l.space_before + l.advance).sum();
        let block_top = match tl.anchor.as_str() {
            "ctr" => rect.y + (rect.h - total_h) / 2.0,
            "b" => rect.y + rect.h - total_h - tl.b_ins,
            _ => rect.y + tl.t_ins,
        };

        // Deliberately NO clipPath: PowerPoint's default is overflow-visible
        // (text simply spills out of its box unless autofit shrinks it), and our
        // width estimates run slightly wide of the real fonts — clipping to the
        // shape rect cut words mid-glyph where PowerPoint shows them whole.
        let mut cursor = block_top;
        for line in &tl.lines {
            cursor += line.space_before;
            if !line.spans.is_empty() {
                // Record the typefaces actually rendered, so the SVG assembler
                // knows which embedded (or bundled-substitute) fonts to emit as
                // @font-face — the family set for embedded matching, the
                // (family, weight, style) variants for substitute embedding,
                // and the characters drawn per family for subsetting. This is
                // the single funnel all text passes through (shapes, tables,
                // bullet spans included), so the char sets are complete.
                for s in &line.spans {
                    if let Some(tf) = s.typeface.as_deref() {
                        if !tf.is_empty() {
                            self.used_fonts.insert(tf.to_string());
                            self.used_font_variants.insert((tf.to_string(), s.bold, s.italic));
                            self.used_font_chars
                                .entry(tf.to_ascii_lowercase())
                                .or_default()
                                .extend(s.text.chars());
                        }
                    }
                }
                let max_size = line.spans.iter().map(|s| s.size_pt).fold(0.0_f64, f64::max);
                self.emit_line_highlight(line, rect, tl.r_ins, cursor, max_size);
                let baseline = cursor + max_size * 0.8;
                self.emit_line(line, rect, tl.r_ins, baseline);
            }
            cursor += line.advance;
        }
    }

    /// For `a:spAutoFit` shapes: the stored extent, grown to hold the laid-out
    /// text. PowerPoint re-runs this fit on open, so the stored extent reflects
    /// the *author's* fonts; substituted (wider) fonts overflow it. Height grows
    /// for wrapped text; width too when the box hugs a single unwrapped line
    /// (`wrap="none"`). Never shrinks — when the text fits, the stored extent
    /// is authoritative.
    pub(crate) fn autofit_grow(&self, sp: Option<Node>, tx_body: Node, rect: &Rect, ph: Option<&Placeholder>) -> Rect {
        let tl = self.layout_text(sp, tx_body, rect, ph);
        if tl.lines.is_empty() {
            return *rect;
        }
        let total_h: f64 = tl.lines.iter().map(|l| l.space_before + l.advance).sum();
        let mut r = *rect;
        r.h = r.h.max(total_h + tl.t_ins + tl.b_ins);
        if tl.wrap_none {
            let needed = tl
                .lines
                .iter()
                .map(|l| l.left + l.spans.iter().map(|s| span_text_width(&s.text, s)).sum::<f64>())
                .fold(0.0_f64, f64::max)
                + tl.r_ins;
            r.w = r.w.max(needed);
        }
        r
    }

    fn layout_text(&self, sp: Option<Node>, tx_body: Node, rect: &Rect, ph: Option<&Placeholder>) -> TextLayout {
        // A placeholder with no explicit `type` defaults to "body" (OOXML), so it
        // uses the body style bucket and the default bullet; a non-placeholder
        // shape stays `None` (no bucket bullet).
        let ph_type: Option<&str> = ph.map(|p| p.typ.as_deref().unwrap_or("body"));
        let is_title = matches!(ph_type, Some("title") | Some("ctrTitle"));

        let body_pr = ch(tx_body, "bodyPr");
        let anchor = body_pr
            .and_then(|b| a(b, "anchor"))
            .unwrap_or(if ph_type == Some("ctrTitle") { "ctr" } else { "t" })
            .to_string();
        // Insets: bodyPr lIns/tIns/rIns/bIns (EMU → pt), else PowerPoint defaults.
        let ins = |name: &str, def: f64| {
            body_pr
                .and_then(|b| a(b, name))
                .and_then(|v| v.parse::<f64>().ok())
                .map(|v| v / EMU_PER_PT)
                .unwrap_or(def)
        };
        let (l_ins, t_ins, r_ins, b_ins) = (ins("lIns", 7.2), ins("tIns", 3.6), ins("rIns", 7.2), ins("bIns", 3.6));
        let wrap_none = body_pr.and_then(|b| a(b, "wrap")) == Some("none");
        let fit = autofit(body_pr);

        // Ultimate run defaults (weakest layer). A shape `p:style/a:fontRef`
        // supplies the default text color/font when the style chain sets none.
        let (fr_color, fr_font) = self.font_ref(sp);
        let default_size = if is_title { 44.0 } else { 18.0 };
        let default_font = if is_title { self.theme.major_font.clone() } else { self.theme.minor_font.clone() };
        let base_run = RunProps {
            size_pt: Some(default_size),
            bold: Some(false),
            italic: Some(false),
            underline: Some(false),
            color: Some(fr_color.unwrap_or_else(|| self.theme.text_default())),
            typeface: Some(fr_font.unwrap_or(default_font)),
            highlight: None,
        };
        let default_color = self.theme.text_default();
        let shape_lst = ch(tx_body, "lstStyle").map(|l| LstStyle::parse(l, &self.theme)).unwrap_or_default();

        let mut lines: Vec<Line> = Vec::new();
        let mut counters = [0u32; 9];
        let mut counters_on = [false; 9];
        let mut pending_gap = 0.0f64; // spcAft carried from the previous paragraph
        for para in tx_body.children().filter(|n| n.is_element() && n.tag_name().name() == "p") {
            self.collect_paragraph(
                para,
                rect,
                ph,
                ph_type,
                &base_run,
                default_size,
                default_color,
                &shape_lst,
                (l_ins, r_ins),
                &fit,
                wrap_none,
                &mut counters,
                &mut counters_on,
                &mut pending_gap,
                &mut lines,
            );
        }
        TextLayout { lines, anchor, t_ins, b_ins, r_ins, wrap_none }
    }

    /// Draw an `a:highlight` marker box behind a line. SVG lays the tspans out
    /// itself, so exact per-span x positions aren't known — one box is drawn for
    /// the whole line (decks overwhelmingly highlight full lines), sized from
    /// the same width estimate the wrapper used, in the first highlighted span's
    /// color.
    fn emit_line_highlight(&mut self, line: &Line, rect: &Rect, r_ins: f64, top: f64, max_size: f64) {
        let Some(color) = line.spans.iter().find_map(|s| s.highlight) else {
            return;
        };
        let est_w: f64 = line.spans.iter().map(|s| span_text_width(&s.text, s)).sum();
        let x = match line.algn.as_str() {
            "ctr" => rect.x + (rect.w - est_w) / 2.0,
            "r" => rect.x + rect.w - r_ins - est_w,
            _ => rect.x + line.left,
        };
        self.body.push_str(&format!(
            r#"<rect x="{x}" y="{y}" width="{w}" height="{h}" fill="{c}"/>"#,
            x = fnum(x - max_size * 0.08),
            y = fnum(top),
            w = fnum(est_w + max_size * 0.16),
            h = fnum(max_size * 1.1),
            c = color.hex()
        ));
    }

    /// Resolve a shape's `p:style/a:fontRef`: its `idx` selects major/minor font
    /// and its color child becomes the shape's default text color. Table cells
    /// pass `None` (no shape node), so the reference is simply absent.
    fn font_ref(&self, sp: Option<Node>) -> (Option<Rgba>, Option<String>) {
        let Some(fr) = sp.and_then(|s| ch(s, "style")).and_then(|s| ch(s, "fontRef")) else {
            return (None, None);
        };
        let color = fr
            .children()
            .find(|n| n.is_element())
            .and_then(|c| self.theme.parse_color(c));
        let font = match a(fr, "idx") {
            Some("major") => Some(self.theme.major_font.clone()),
            Some("minor") => Some(self.theme.minor_font.clone()),
            _ => None,
        };
        (color, font)
    }

    /// Merge the per-level style chain (weakest → strongest) for one paragraph.
    fn resolve_para(
        &self,
        ph: Option<&Placeholder>,
        ph_type: Option<&str>,
        lvl: usize,
        base_run: &RunProps,
        shape_lst: &LstStyle,
        ppr: Option<Node>,
    ) -> ParaProps {
        let bucket = match ph_type {
            Some("title") | Some("ctrTitle") => &self.title_style,
            Some("body") | Some("subTitle") => &self.body_style,
            _ => &self.other_style,
        };
        let mut pp = ParaProps { algn: Some("l".into()), def_rpr: base_run.clone(), ..Default::default() };
        pp.overlay(&self.pres_style.levels[lvl]);
        pp.overlay(&bucket.levels[lvl]);
        if let Some(m) = ph.and_then(|p| match_placeholder(&self.master_phs, p)) {
            pp.overlay(&m.text_styles.levels[lvl]);
        }
        if let Some(m) = ph.and_then(|p| match_placeholder(&self.layout_phs, p)) {
            pp.overlay(&m.text_styles.levels[lvl]);
        }
        pp.overlay(&shape_lst.levels[lvl]);
        if let Some(p) = ppr {
            pp.overlay(&parse_ppr(p, &self.theme));
        }
        pp
    }

    #[allow(clippy::too_many_arguments)]
    fn collect_paragraph(
        &self,
        para: Node,
        rect: &Rect,
        ph: Option<&Placeholder>,
        ph_type: Option<&str>,
        base_run: &RunProps,
        default_size: f64,
        default_color: Rgba,
        shape_lst: &LstStyle,
        (l_ins, r_ins): (f64, f64),
        fit: &Autofit,
        wrap_none: bool,
        counters: &mut [u32; 9],
        counters_on: &mut [bool; 9],
        pending_gap: &mut f64,
        out: &mut Vec<Line>,
    ) {
        let ppr = ch(para, "pPr");
        let lvl = ppr
            .and_then(|p| a(p, "lvl"))
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(0)
            .min(8);
        let pp = self.resolve_para(ph, ph_type, lvl, base_run, shape_lst, ppr);
        let algn = pp.algn.clone().unwrap_or_else(|| "l".into());
        let run_def = &pp.def_rpr;
        let para_size = run_def.size_pt.unwrap_or(default_size) * fit.font_scale;

        // Left inset for this paragraph's text, and the (hanging) bullet position.
        let text_left = l_ins + pp.mar_l.unwrap_or(lvl as f64 * LVL_INDENT);
        let bullet_left = text_left + pp.indent.unwrap_or(0.0);
        let avail = (rect.w - text_left - r_ins).max(para_size);

        // Inter-paragraph gaps: this paragraph's spcBef plus the previous one's
        // carried spcAft. spcAft is deferred to the next paragraph's first line.
        let spc_bef = spacing_pts(pp.spc_bef, para_size);
        let space_before = spc_bef + std::mem::take(pending_gap);
        *pending_gap = spacing_pts(pp.spc_aft, para_size);

        // Split runs into segments (an `a:br` starts a new segment/line).
        let segments = build_segments(
            para,
            &self.theme,
            run_def,
            default_size,
            default_color,
            fit.font_scale,
            self.slide_no,
        );
        let has_text = segments.iter().any(|s| s.iter().any(|sp| !sp.text.is_empty()));

        if !has_text {
            // Empty paragraph: size the blank line via endParaRPr, keep the gap.
            let mut end = run_def.clone();
            if let Some(e) = ch(para, "endParaRPr") {
                end.overlay(&parse_rpr(e, &self.theme));
            }
            let size = end.size_pt.unwrap_or(default_size) * fit.font_scale;
            out.push(Line {
                spans: Vec::new(),
                algn,
                left: text_left,
                advance: line_advance(size, pp.ln_spc, fit),
                space_before,
            });
            return;
        }

        let bullet = self.resolve_bullet(&pp, ph_type, run_def, default_size, fit.font_scale, lvl, counters, counters_on);

        let mut first_line = true;
        for seg in &segments {
            for visual in wrap_segment(seg, avail, wrap_none) {
                if visual.is_empty() {
                    continue;
                }
                let mut spans = visual;
                let mut left = text_left;
                let mut sb = 0.0;
                if first_line {
                    sb = space_before;
                    if let Some(b) = &bullet {
                        spans.insert(0, b.clone());
                        left = bullet_left;
                    }
                    first_line = false;
                }
                let max_size = spans.iter().map(|s| s.size_pt).fold(0.0_f64, f64::max);
                out.push(Line {
                    spans,
                    algn: algn.clone(),
                    left,
                    advance: line_advance(max_size, pp.ln_spc, fit),
                    space_before: sb,
                });
            }
        }
    }

    /// The bullet span for a paragraph, or `None` (buNone, or a non-body shape
    /// with nothing in the chain). Body/subTitle placeholders default to "•".
    #[allow(clippy::too_many_arguments)]
    fn resolve_bullet(
        &self,
        pp: &ParaProps,
        ph_type: Option<&str>,
        run_def: &RunProps,
        default_size: f64,
        scale: f64,
        lvl: usize,
        counters: &mut [u32; 9],
        counters_on: &mut [bool; 9],
    ) -> Option<Span> {
        let base_size = run_def.size_pt.unwrap_or(default_size) * scale;
        let color = run_def.color.unwrap_or_else(|| self.theme.text_default());
        let mk = |text: String, size_pct: Option<f64>, font: Option<String>| Span {
            text,
            size_pt: base_size * size_pct.unwrap_or(1.0),
            bold: run_def.bold.unwrap_or(false),
            italic: false,
            underline: false,
            color,
            typeface: font.or_else(|| run_def.typeface.clone()),
            highlight: None,
        };
        match &pp.bullet {
            Some(Bullet::None) => None,
            Some(Bullet::Char { chr, font, size_pct }) => {
                Some(mk(format!("{chr} "), *size_pct, font.clone()))
            }
            Some(Bullet::AutoNum { typ, start, size_pct }) => {
                let n = if counters_on[lvl] { counters[lvl] + 1 } else { *start };
                counters[lvl] = n;
                counters_on[lvl] = true;
                for d in counters.iter_mut().skip(lvl + 1) {
                    *d = 0;
                }
                for d in counters_on.iter_mut().skip(lvl + 1) {
                    *d = false;
                }
                Some(mk(format!("{} ", format_autonum(typ, n)), *size_pct, None))
            }
            None => {
                if matches!(ph_type, Some("body") | Some("subTitle")) {
                    Some(mk("\u{2022} ".to_string(), None, None))
                } else {
                    None
                }
            }
        }
    }

    fn emit_line(&mut self, line: &Line, rect: &Rect, r_ins: f64, baseline: f64) {
        let base = &line.spans[0];
        let (anchor_attr, tx) = match line.algn.as_str() {
            "ctr" => ("middle", rect.x + rect.w / 2.0),
            "r" => ("end", rect.x + rect.w - r_ins),
            _ => ("start", rect.x + line.left),
        };
        let mut text_style = String::new();
        if base.bold {
            text_style.push_str(r#" font-weight="bold""#);
        }
        if base.italic {
            text_style.push_str(r#" font-style="italic""#);
        }
        if base.underline {
            text_style.push_str(r#" text-decoration="underline""#);
        }
        self.body.push_str(&format!(
            r#"<text x="{x}" y="{y}" font-family="{ff}" font-size="{sz}" fill="{fill}" text-anchor="{anchor}"{style}>"#,
            x = fnum(tx),
            y = fnum(baseline),
            ff = font_family(base.typeface.as_deref()),
            sz = fnum(base.size_pt),
            fill = base.color.hex(),
            anchor = anchor_attr,
            style = text_style
        ));
        for span in &line.spans {
            let mut attrs = String::new();
            if (span.size_pt - base.size_pt).abs() > 1e-6 {
                attrs.push_str(&format!(r#" font-size="{}""#, fnum(span.size_pt)));
            }
            if span.color.hex() != base.color.hex() {
                attrs.push_str(&format!(r#" fill="{}""#, span.color.hex()));
            }
            if span.typeface != base.typeface {
                attrs.push_str(&format!(r#" font-family="{}""#, font_family(span.typeface.as_deref())));
            }
            if span.bold != base.bold {
                attrs.push_str(if span.bold { r#" font-weight="bold""# } else { r#" font-weight="normal""# });
            }
            if span.italic != base.italic {
                attrs.push_str(if span.italic { r#" font-style="italic""# } else { r#" font-style="normal""# });
            }
            if span.underline != base.underline {
                attrs.push_str(if span.underline {
                    r#" text-decoration="underline""#
                } else {
                    r#" text-decoration="none""#
                });
            }
            self.body.push_str(&format!("<tspan{attrs}>{t}</tspan>", t = esc(&span.text)));
        }
        self.body.push_str("</text>");
    }
}

/// Build a paragraph's runs into segments; each `a:br` starts a new segment
/// (a hard line break). `a:r` and `a:fld` contribute styled text.
#[allow(clippy::too_many_arguments)]
fn build_segments(
    para: Node,
    theme: &super::color::Theme,
    run_def: &RunProps,
    default_size: f64,
    default_color: Rgba,
    scale: f64,
    slide_no: usize,
) -> Vec<Vec<Span>> {
    let mut segments: Vec<Vec<Span>> = vec![Vec::new()];
    for child in para.children().filter(|n| n.is_element()) {
        match child.tag_name().name() {
            "r" | "fld" => {
                // A slide-number field renders the ACTUAL slide number; its
                // cached <a:t> often still holds the layout's "‹Nr.›" prompt.
                let slidenum = child.tag_name().name() == "fld"
                    && a(child, "type").is_some_and(|t| t.starts_with("slidenum"));
                let cached = ch(child, "t").and_then(|n| n.text());
                let t: String = if slidenum {
                    slide_no.to_string()
                } else {
                    match cached {
                        Some(t) if !t.is_empty() => t.to_string(),
                        _ => continue,
                    }
                };
                let mut props = run_def.clone();
                if let Some(rpr) = ch(child, "rPr") {
                    props.overlay(&parse_rpr(rpr, theme));
                }
                segments
                    .last_mut()
                    .unwrap()
                    .push(Span::from_props(t, &props, default_size, default_color, scale));
            }
            "br" => segments.push(Vec::new()),
            _ => {}
        }
    }
    segments
}

/// Greedy, span-aware word-wrap of one segment into visual lines. A word longer
/// than `avail` is placed alone (and clipped) rather than hard-broken.
fn wrap_segment(spans: &[Span], avail: f64, wrap_none: bool) -> Vec<Vec<Span>> {
    if wrap_none {
        return vec![spans.to_vec()];
    }
    // Tokenize into words (collapsing runs of whitespace); a word is a list of
    // (span index, text) pieces so a word may cross a run boundary.
    let mut words: Vec<Vec<(usize, String)>> = Vec::new();
    let mut cur_word: Vec<(usize, String)> = Vec::new();
    for (si, span) in spans.iter().enumerate() {
        for c in span.text.chars() {
            if c.is_whitespace() {
                if !cur_word.is_empty() {
                    words.push(std::mem::take(&mut cur_word));
                }
            } else if let Some(last) = cur_word.last_mut().filter(|l| l.0 == si) {
                last.1.push(c);
            } else {
                cur_word.push((si, c.to_string()));
            }
        }
    }
    if !cur_word.is_empty() {
        words.push(cur_word);
    }

    let word_w = |w: &[(usize, String)]| -> f64 {
        w.iter().map(|(si, t)| span_text_width(t, &spans[*si])).sum()
    };
    let push_piece = |line: &mut Vec<(usize, String)>, si: usize, t: &str| {
        if let Some(last) = line.last_mut().filter(|l| l.0 == si) {
            last.1.push_str(t);
        } else {
            line.push((si, t.to_string()));
        }
    };

    let mut lines: Vec<Vec<(usize, String)>> = Vec::new();
    let mut cur: Vec<(usize, String)> = Vec::new();
    let mut cur_w = 0.0;
    for word in &words {
        let ww = word_w(word);
        if cur.is_empty() {
            for (si, t) in word {
                push_piece(&mut cur, *si, t);
            }
            cur_w = ww;
            continue;
        }
        let sp_w = span_text_width(" ", &spans[word[0].0]);
        if cur_w + sp_w + ww > avail {
            lines.push(std::mem::take(&mut cur));
            for (si, t) in word {
                push_piece(&mut cur, *si, t);
            }
            cur_w = ww;
        } else {
            push_piece(&mut cur, word[0].0, " ");
            for (si, t) in word {
                push_piece(&mut cur, *si, t);
            }
            cur_w += sp_w + ww;
        }
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    if lines.is_empty() {
        return vec![Vec::new()];
    }
    lines
        .into_iter()
        .map(|line| {
            line.into_iter()
                .map(|(si, t)| Span { text: t, ..spans[si].clone() })
                .collect()
        })
        .collect()
}

/// Vertical advance for a line of `max_size`, honoring `lnSpc` and the autofit
/// `lnSpcReduction`. Default single spacing is 1.2× the font size.
fn line_advance(max_size: f64, ln_spc: Option<Spacing>, fit: &Autofit) -> f64 {
    let base = 1.2 * max_size;
    let adv = match ln_spc {
        Some(Spacing::Pct(f)) => f * base,
        Some(Spacing::Pts(p)) => p,
        None => base,
    };
    adv * (1.0 - fit.ln_spc_reduction)
}

/// A `spcBef`/`spcAft` value in points. Percentages are relative to the
/// paragraph's font size.
fn spacing_pts(sp: Option<Spacing>, size: f64) -> f64 {
    match sp {
        Some(Spacing::Pct(f)) => f * size,
        Some(Spacing::Pts(p)) => p,
        None => 0.0,
    }
}

/// Read `a:normAutofit` (fontScale/lnSpcReduction) and `a:spAutoFit` from a
/// `bodyPr`.
fn autofit(body_pr: Option<Node>) -> Autofit {
    let mut fit = Autofit { font_scale: 1.0, ln_spc_reduction: 0.0 };
    let Some(bp) = body_pr else { return fit };
    if let Some(na) = ch(bp, "normAutofit") {
        if let Some(fs) = a(na, "fontScale").and_then(|v| v.parse::<f64>().ok()) {
            fit.font_scale = fs / 100_000.0;
        }
        if let Some(lr) = a(na, "lnSpcReduction").and_then(|v| v.parse::<f64>().ok()) {
            fit.ln_spc_reduction = (lr / 100_000.0).clamp(0.0, 0.9);
        }
    }
    // spAutoFit ("shape grows to fit") needs no handling here: the stored xfrm
    // already reflects the grown shape, and text is never clipped anyway.
    fit
}

/// Format an `a:buAutoNum` counter `n` per its `type` (unknown → arabicPeriod).
fn format_autonum(typ: &str, n: u32) -> String {
    match typ {
        "arabicParenR" => format!("{n})"),
        "arabicParenBoth" => format!("({n})"),
        "alphaLcPeriod" => format!("{}.", to_alpha_lower(n)),
        "alphaLcParenR" => format!("{})", to_alpha_lower(n)),
        "alphaUcPeriod" => format!("{}.", to_alpha_lower(n).to_uppercase()),
        "alphaUcParenR" => format!("{})", to_alpha_lower(n).to_uppercase()),
        "romanLcPeriod" => format!("{}.", to_roman_lower(n)),
        "romanUcPeriod" => format!("{}.", to_roman_lower(n).to_uppercase()),
        _ => format!("{n}."),
    }
}

/// 1 → "a", 26 → "z", 27 → "aa", …
fn to_alpha_lower(mut n: u32) -> String {
    if n == 0 {
        return "a".to_string();
    }
    let mut s = String::new();
    while n > 0 {
        n -= 1;
        s.insert(0, (b'a' + (n % 26) as u8) as char);
        n /= 26;
    }
    s
}

/// 1 → "i", 4 → "iv", 9 → "ix", …
fn to_roman_lower(n: u32) -> String {
    const R: &[(u32, &str)] = &[
        (1000, "m"), (900, "cm"), (500, "d"), (400, "cd"), (100, "c"), (90, "xc"),
        (50, "l"), (40, "xl"), (10, "x"), (9, "ix"), (5, "v"), (4, "iv"), (1, "i"),
    ];
    let mut n = n;
    let mut s = String::new();
    for (v, sym) in R {
        while n >= *v {
            s.push_str(sym);
            n -= v;
        }
    }
    if s.is_empty() {
        s.push('i');
    }
    s
}

/// Greedy single-style word wrap using the glyph-width table (~average em),
/// with character-level hard-breaking of over-long words. The live text path
/// uses the span-aware [`wrap_segment`]; this single-style form backs its tests.
#[cfg(test)]
pub(crate) fn wrap(text: &str, font_size: f64, avail_width: f64) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut current_w = 0.0;
    let space_w = text_width(" ", font_size);
    for word in text.split_whitespace() {
        let ww = text_width(word, font_size);
        if ww > avail_width {
            if !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                current_w = 0.0;
            }
            // Hard-break the over-long word by characters.
            let mut chunk = String::new();
            let mut chunk_w = 0.0;
            for c in word.chars() {
                let cw = text_width(&c.to_string(), font_size);
                if chunk_w + cw > avail_width && !chunk.is_empty() {
                    lines.push(std::mem::take(&mut chunk));
                    chunk_w = 0.0;
                }
                chunk.push(c);
                chunk_w += cw;
            }
            if !chunk.is_empty() {
                current = chunk;
                current_w = chunk_w;
            }
            continue;
        }
        let extra = if current.is_empty() { ww } else { space_w + ww };
        if current_w + extra > avail_width && !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            current_w = 0.0;
        }
        if current.is_empty() {
            current.push_str(word);
            current_w = ww;
        } else {
            current.push(' ');
            current.push_str(word);
            current_w += space_w + ww;
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

/// Estimated text width in points, summing per-glyph advances.
fn text_width(s: &str, size: f64) -> f64 {
    s.chars().map(|c| glyph_em(c) * size).sum()
}

/// Span-aware width estimate: bold glyphs run ~5% wider than the regular
/// Helvetica advances in the table, which was enough to make bold-heavy lines
/// wrap later than PowerPoint does. The family factor corrects for typefaces
/// that run tighter than the Helvetica table — without it, decks authored in
/// e.g. Aptos wrap lines the author fit on one, and text bursts out of
/// author-hugged (`spAutoFit`) boxes.
fn span_text_width(s: &str, span: &Span) -> f64 {
    let mut w = text_width(s, span.size_pt);
    if span.bold {
        w *= 1.05;
    }
    w * span.typeface.as_deref().map_or(1.0, family_width_factor)
}

/// Advance-width correction vs the Helvetica table for families whose average
/// advances differ notably. Narrow/condensed cuts run ~18% tighter; the modern
/// Microsoft UI grotesques (Aptos, Segoe) ~5%; Calibri/Candara ~9%.
fn family_width_factor(family: &str) -> f64 {
    let f = family.to_ascii_lowercase();
    if is_condensed(&f) {
        0.82
    } else if f.contains("calibri") || f.contains("carlito") || f.contains("candara") {
        // Carlito is the bundled Calibri clone (identical metrics), so a run
        // that literally names Carlito must wrap like Calibri. Cambria/Caladea
        // keep the neutral 1.0 tail below — no calibrated serif factor exists,
        // and matching prior Cambria behavior avoids a wrap regression.
        0.91
    } else if f.contains("aptos") || f.contains("segoe") {
        0.95
    } else {
        1.0
    }
}

/// Narrow/condensed typeface family (Aptos Narrow, Arial Narrow, …).
fn is_condensed(family: &str) -> bool {
    let f = family.to_ascii_lowercase();
    f.contains("narrow") || f.contains("condensed")
}

/// Approximate glyph advance in em units (Helvetica metrics / 1000). Non-ASCII
/// (umlauts, CJK, …) falls back to a mid-width average.
fn glyph_em(c: char) -> f64 {
    // Advance widths for ASCII 0x20..=0x7E, in per-mille of an em.
    const W: [u16; 95] = [
        278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278, // 0x20-0x2F
        556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556, // 0x30-0x3F
        1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778, // 0x40-0x4F
        667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556, // 0x50-0x5F
        333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556, // 0x60-0x6F
        556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, // 0x70-0x7E
    ];
    let u = c as u32;
    if (0x20..=0x7E).contains(&u) {
        W[(u - 0x20) as usize] as f64 / 1000.0
    } else {
        0.55
    }
}

/// Emit an SVG `font-family` list: the run's typeface first, then fallbacks.
///
/// Narrow faces (Aptos Narrow ships only with Office 2024+) fall back through
/// the widely available narrow families before any regular-width font — a
/// regular fallback is ~20% wider and overflows author-fitted boxes.
///
/// For the common unembedded Office fonts (Calibri, Cambria, Segoe UI, …) we
/// splice in a richer, named fallback chain from [`crate::fonts`] right after
/// the authored name: Calibri leads to the bundled Carlito clone, Cambria to
/// Caladea, and the rest to the closest macOS/cross-platform staple. This is
/// the single place that string is built, so the webview SVG and the
/// resvg/fontdb exporter resolve the same way (the exporter also carries the
/// Carlito/Caladea bytes in its fontdb). Chains from `fonts` already end in a
/// CSS generic, so they replace — not extend — the default tail.
fn font_family(font: Option<&str>) -> String {
    match font.filter(|f| !f.is_empty()) {
        Some(f) if is_condensed(f) => format!(
            "{}, Arial Narrow, Liberation Sans Narrow, Helvetica Neue Condensed, Helvetica, Arial, sans-serif",
            esc(f)
        ),
        Some(f) => match crate::fonts::fallback_families(f) {
            // Named chain (already terminated by a CSS generic): authored name
            // first, then the chain. The chain families are our own ASCII
            // literals, so only the authored name needs XML-escaping.
            Some(chain) => format!("{}, {}", esc(f), chain.join(", ")),
            None => format!("{}, Helvetica, Arial, sans-serif", esc(f)),
        },
        None => "Helvetica, Arial, sans-serif".to_string(),
    }
}
