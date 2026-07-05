//! Advanced search-query parser: turns a user's search box string into a safe
//! FTS5 `MATCH` expression plus any date-range bounds, degrading gracefully on
//! anything it can't represent.
//!
//! # Grammar (v1)
//!
//! - **bare terms** — implicit AND: `alpha beta` matches slides with both.
//! - **quoted phrases** — `"multi word"` matches the words adjacent, in order;
//!   the last token gets a prefix match (`"multi word"*`).
//! - **fielded terms** — `title:`, `deck:`, `notes:`, `body:` restrict a term to
//!   one FTS column (`deck:` targets the `deck_title` column). Works with
//!   phrases too: `title:"annual report"`.
//! - **booleans** — uppercase `OR` and `NOT` keywords; `-term` is sugar for
//!   `NOT term`. Precedence is `NOT` > implicit-AND > `OR`. There are **no
//!   parentheses** in v1 — `OR` always splits at the top level.
//! - **date bounds** — `before:YYYY-MM-DD` / `after:YYYY-MM-DD` are lifted out of
//!   the text match into modified-date filters (`after` → `modified_from`,
//!   `before` → `modified_to`). Repeated bounds combine restrictively (max of
//!   the `after`s, min of the `before`s).
//!
//! # Safety
//!
//! Every user token is emitted as a double-quoted FTS5 string with internal `"`
//! doubled, then optionally suffixed with the prefix operator `*` — never
//! interpolated raw. Tokens with no alphanumeric content (the FTS `unicode61`
//! tokenizer would produce nothing from them) are dropped, so hostile input
//! (`%`, `;`, lone operators, unbalanced quotes, emoji, huge strings) can never
//! produce an FTS5 syntax error. A query the parser can't turn into any positive
//! term yields `match_expr == None`, and the caller falls back to plain
//! tokenization (see `sanitize_query`) of [`ParsedQuery::residual`] — only the
//! tokens the parser could NOT classify, never ones it already consumed (dates,
//! negatives, operators). Consequences, all deliberate v1 behavior:
//!
//! - a **date-only** query (`after:2020-01-01`) browses with the date filter
//!   applied instead of text-searching the date's digits;
//! - a **purely-negative** query (`-churn`, `NOT churn`) browses — a standalone
//!   negation is inexpressible in FTS5, and v1 does not text-search the word the
//!   user asked to exclude;
//! - pure junk still degrades to browse, exactly as before.

/// Known field prefixes → FTS column name. `deck` is the odd one out: the column
/// is `deck_title`.
fn field_column(field: &str) -> Option<&'static str> {
    match field {
        "title" => Some("title"),
        "body" => Some("body"),
        "notes" => Some("notes"),
        "deck" => Some("deck_title"),
        _ => None,
    }
}

/// A parsed search query: an FTS5 `MATCH` expression (absent when the query has
/// no usable text terms) plus optional date bounds in unix seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ParsedQuery {
    /// FTS5 `MATCH` string, or `None` when the query reduced to no positive
    /// terms (empty, date-only, or purely negative) — the caller then browses or
    /// falls back to plain tokens.
    pub match_expr: Option<String>,
    /// Lower bound on `modified_unix` (from `after:`), unix seconds.
    pub after: Option<i64>,
    /// Upper bound on `modified_unix` (from `before:`), unix seconds.
    pub before: Option<i64>,
    /// Space-joined raw tokens the parser could NOT classify (`Token::Nothing`
    /// junk only). When `match_expr` is `None`, the caller falls back to plain
    /// tokenization of THIS — not the raw input — so consumed tokens (dates,
    /// negatives) are never accidentally text-searched.
    pub residual: String,
}

/// Combine two optional bounds keeping the larger (more restrictive lower bound).
pub(crate) fn max_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.max(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// Combine two optional bounds keeping the smaller (more restrictive upper bound).
pub(crate) fn min_opt(a: Option<i64>, b: Option<i64>) -> Option<i64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    }
}

/// A single rendered FTS term (bare or column-scoped), already prefix-matched.
struct FtsTerm {
    col: Option<&'static str>,
    /// The phrase text (unescaped); guaranteed to contain ≥1 alphanumeric char.
    content: String,
}

impl FtsTerm {
    /// Render as a safe, prefix-matched FTS5 sub-expression, e.g. `title:"foo"*`.
    fn render(&self) -> String {
        let mut out = String::new();
        if let Some(col) = self.col {
            out.push_str(col);
            out.push(':');
        }
        out.push('"');
        // Escape the FTS string delimiter by doubling it; everything else inside
        // a double-quoted FTS string is literal, tokenized by unicode61.
        out.push_str(&self.content.replace('"', "\"\""));
        out.push('"');
        out.push('*');
        out
    }
}

/// Build a term, dropping it if it has no tokenizable (alphanumeric) content.
fn make_term(col: Option<&'static str>, content: &str) -> Option<FtsTerm> {
    if content.chars().any(char::is_alphanumeric) {
        Some(FtsTerm { col, content: content.to_string() })
    } else {
        None
    }
}

/// What one whitespace-delimited (quote-aware) token classifies to.
enum Token {
    Term { negate: bool, term: FtsTerm },
    After(i64),
    Before(i64),
    Nothing,
}

/// Split the input on whitespace, keeping double-quoted spans (which may contain
/// spaces) intact. An unterminated quote runs to end of input.
fn lex(input: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    for ch in input.chars() {
        if ch == '"' {
            in_quote = !in_quote;
            cur.push(ch);
        } else if ch.is_whitespace() && !in_quote {
            if !cur.is_empty() {
                tokens.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        tokens.push(cur);
    }
    tokens
}

/// Strip one pair of surrounding double quotes (leading and/or trailing).
fn unquote(v: &str) -> &str {
    let v = v.strip_prefix('"').unwrap_or(v);
    v.strip_suffix('"').unwrap_or(v)
}

/// Parse `YYYY-MM-DD` (UTC midnight) to unix seconds; `None` if it doesn't parse.
fn parse_date(value: &str) -> Option<i64> {
    let v = unquote(value);
    let mut parts = v.split('-');
    let y: i64 = parts.next()?.parse().ok()?;
    let mo: i64 = parts.next()?.parse().ok()?;
    let d: i64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None; // more than three components
    }
    ymd_to_unix(y, mo, d)
}

/// Days-from-civil (Howard Hinnant's algorithm) → unix seconds at UTC midnight.
/// Range-checks the components so absurd input yields `None` rather than a bogus
/// timestamp.
fn ymd_to_unix(y: i64, m: i64, d: i64) -> Option<i64> {
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400; // [0, 399]
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy; // [0, 146096]
    let days = era * 146097 + doe - 719_468;
    Some(days * 86_400)
}

/// Classify one lexed token (operators `OR`/`NOT` are handled by the caller).
fn classify(tok: &str) -> Token {
    let mut s = tok;
    let mut negate = false;
    // `-term` is NOT sugar. Only one leading dash is peeled.
    if let Some(rest) = s.strip_prefix('-') {
        negate = true;
        s = rest;
    }
    if s.is_empty() {
        return Token::Nothing;
    }

    if let Some(colon) = s.find(':') {
        let field = &s[..colon];
        let value = &s[colon + 1..];
        if let Some(col) = field_column(field) {
            return term_token(negate, Some(col), value);
        }
        match field {
            "before" => {
                if let Some(u) = parse_date(value) {
                    return Token::Before(u);
                }
            }
            "after" => {
                if let Some(u) = parse_date(value) {
                    return Token::After(u);
                }
            }
            _ => {}
        }
        // Unknown field or unparseable date: treat the whole token as a bare
        // term (the colon becomes a tokenizer separator inside the quotes).
    }
    term_token(negate, None, s)
}

fn term_token(negate: bool, col: Option<&'static str>, value: &str) -> Token {
    match make_term(col, unquote(value)) {
        Some(term) => Token::Term { negate, term },
        None => Token::Nothing,
    }
}

/// Render one OR-group (a conjunction of positives, minus its negatives) to FTS5.
/// `None` when the group has no positive term (a purely-negative group can't be
/// expressed as a standalone FTS match).
fn render_group(pos: &[String], neg: &[String]) -> Option<String> {
    if pos.is_empty() {
        return None;
    }
    let mut expr = pos.join(" AND ");
    if !neg.is_empty() {
        // Parenthesize a multi-term conjunction so `NOT` applies to the whole
        // group, not just the last positive.
        if pos.len() > 1 {
            expr = format!("({expr})");
        }
        for n in neg {
            expr.push_str(" NOT ");
            expr.push_str(n);
        }
    }
    Some(expr)
}

/// Join rendered OR-groups. Each group is parenthesized when there is more than
/// one, so the boolean structure is explicit and independent of FTS5 precedence.
fn render_query(groups: &[(Vec<String>, Vec<String>)]) -> Option<String> {
    let rendered: Vec<String> = groups
        .iter()
        .filter_map(|(pos, neg)| render_group(pos, neg))
        .collect();
    match rendered.len() {
        0 => None,
        1 => rendered.into_iter().next(),
        _ => Some(
            rendered
                .iter()
                .map(|g| format!("({g})"))
                .collect::<Vec<_>>()
                .join(" OR "),
        ),
    }
}

/// Parse a raw search string into an FTS5 match expression plus date bounds.
///
/// Never fails and never panics: unrepresentable input yields
/// `match_expr == None` for the caller to handle.
pub(crate) fn parse_query(input: &str) -> ParsedQuery {
    let mut groups: Vec<(Vec<String>, Vec<String>)> = Vec::new();
    let mut pos: Vec<String> = Vec::new();
    let mut neg: Vec<String> = Vec::new();
    let mut pending_not = false;
    let mut after: Option<i64> = None;
    let mut before: Option<i64> = None;
    let mut residual: Vec<String> = Vec::new();

    for tok in lex(input) {
        if tok == "OR" {
            if !pos.is_empty() || !neg.is_empty() {
                groups.push((std::mem::take(&mut pos), std::mem::take(&mut neg)));
            }
            pending_not = false;
            continue;
        }
        if tok == "NOT" {
            pending_not = true;
            continue;
        }
        match classify(&tok) {
            Token::After(u) => {
                after = max_opt(after, Some(u));
                pending_not = false;
            }
            Token::Before(u) => {
                before = min_opt(before, Some(u));
                pending_not = false;
            }
            Token::Term { negate, term } => {
                let rendered = term.render();
                if negate || pending_not {
                    neg.push(rendered);
                } else {
                    pos.push(rendered);
                }
                pending_not = false;
            }
            Token::Nothing => {
                residual.push(tok);
                pending_not = false;
            }
        }
    }
    if !pos.is_empty() || !neg.is_empty() {
        groups.push((pos, neg));
    }

    ParsedQuery { match_expr: render_query(&groups), after, before, residual: residual.join(" ") }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(input: &str) -> Option<String> {
        parse_query(input).match_expr
    }

    #[test]
    fn bare_term_prefix() {
        assert_eq!(m("revenue").as_deref(), Some("\"revenue\"*"));
    }

    #[test]
    fn implicit_and() {
        assert_eq!(m("alpha beta").as_deref(), Some("\"alpha\"* AND \"beta\"*"));
    }

    #[test]
    fn fielded_terms() {
        assert_eq!(m("title:foo").as_deref(), Some("title:\"foo\"*"));
        assert_eq!(m("body:foo").as_deref(), Some("body:\"foo\"*"));
        assert_eq!(m("notes:foo").as_deref(), Some("notes:\"foo\"*"));
        // `deck:` maps to the deck_title column.
        assert_eq!(m("deck:foo").as_deref(), Some("deck_title:\"foo\"*"));
    }

    #[test]
    fn quoted_phrase() {
        assert_eq!(m("\"multi word\"").as_deref(), Some("\"multi word\"*"));
    }

    #[test]
    fn fielded_phrase() {
        assert_eq!(m("title:\"annual report\"").as_deref(), Some("title:\"annual report\"*"));
    }

    #[test]
    fn or_widens() {
        assert_eq!(m("revenue OR churn").as_deref(), Some("(\"revenue\"*) OR (\"churn\"*)"));
    }

    #[test]
    fn not_keyword_and_dash_sugar_are_equivalent() {
        let expected = Some("\"revenue\"* NOT \"churn\"*".to_string());
        assert_eq!(m("revenue NOT churn"), expected);
        assert_eq!(m("revenue -churn"), expected);
    }

    #[test]
    fn not_binds_over_multiple_positives() {
        assert_eq!(
            m("alpha beta NOT gamma").as_deref(),
            Some("(\"alpha\"* AND \"beta\"*) NOT \"gamma\"*"),
        );
    }

    #[test]
    fn purely_negative_has_no_match() {
        assert_eq!(m("-churn"), None);
        assert_eq!(m("NOT churn"), None);
        // The consumed negative must NOT leak into the fallback residual —
        // otherwise the excluded word would get text-searched.
        assert_eq!(parse_query("-churn").residual, "");
        assert_eq!(parse_query("NOT churn").residual, "");
    }

    #[test]
    fn date_bounds_are_lifted_out() {
        let p = parse_query("after:2020-01-01 before:2021-12-31 revenue");
        assert_eq!(p.match_expr.as_deref(), Some("\"revenue\"*"));
        assert_eq!(p.after, Some(1_577_836_800)); // 2020-01-01T00:00:00Z
        assert_eq!(p.before, Some(1_640_908_800)); // 2021-12-31T00:00:00Z
    }

    #[test]
    fn date_only_query_has_no_match() {
        let p = parse_query("after:2020-01-01");
        assert_eq!(p.match_expr, None);
        assert_eq!(p.after, Some(1_577_836_800));
        // The consumed date must NOT leak into the fallback residual — otherwise
        // its digits would get text-searched and return zero hits.
        assert_eq!(p.residual, "");
    }

    #[test]
    fn residual_keeps_only_unclassified_junk() {
        // Junk tokens survive into the residual verbatim; classified tokens
        // (dates, terms, operators) never do.
        let p = parse_query("%%% after:2020-01-01 ;;;");
        assert_eq!(p.match_expr, None);
        assert_eq!(p.residual, "%%% ;;;");
        assert_eq!(parse_query("revenue %%%").residual, "%%%");
        assert_eq!(parse_query("OR NOT").residual, "");
    }

    #[test]
    fn repeated_dates_combine_restrictively() {
        let p = parse_query("after:2020-01-01 after:2021-01-01 before:2025-01-01 before:2023-01-01");
        assert_eq!(p.after, Some(ymd_to_unix(2021, 1, 1).unwrap())); // max of afters
        assert_eq!(p.before, Some(ymd_to_unix(2023, 1, 1).unwrap())); // min of befores
    }

    #[test]
    fn malformed_date_falls_back_to_bare_term() {
        // Not a valid date → the whole token is searched as text (colon becomes a
        // tokenizer separator), and no date bound is set.
        let p = parse_query("before:nope");
        assert_eq!(p.match_expr.as_deref(), Some("\"before:nope\"*"));
        assert_eq!(p.before, None);
    }

    #[test]
    fn unknown_field_is_a_bare_term() {
        assert_eq!(m("foo:bar").as_deref(), Some("\"foo:bar\"*"));
    }

    #[test]
    fn lowercase_operators_are_literal_terms() {
        // Only uppercase OR/NOT are operators.
        assert_eq!(m("revenue or churn").as_deref(), Some("\"revenue\"* AND \"or\"* AND \"churn\"*"));
    }

    #[test]
    fn internal_quote_is_doubled() {
        // The escape path produces valid FTS on any residual quote in content.
        let term = FtsTerm { col: None, content: "a\"b".into() };
        assert_eq!(term.render(), "\"a\"\"b\"*");
    }

    #[test]
    fn empty_and_junk_have_no_match() {
        assert_eq!(m(""), None);
        assert_eq!(m("   "), None);
        assert_eq!(m("%%% ;;; ()"), None);
        assert_eq!(m("\"\""), None);
        assert_eq!(m("title:"), None);
    }

    #[test]
    fn hostile_input_never_panics() {
        for q in [
            "\"unterminated",
            "title:\"unterminated",
            "NEAR(a b)",
            "(revenue AND",
            "* * *",
            "revenue) NOT churn",
            "a:b:c:d",
            "-",
            "--",
            "- -",
            "OR OR OR",
            "NOT NOT NOT",
            ":::",
            "😀🎉",
            "before:2020-13-45",
            &"z".repeat(10_000),
            "a OR b OR c NOT d -e title:f deck:\"g h\"",
        ] {
            // Must not panic; result is either None or a rendered string.
            let _ = parse_query(q);
        }
    }
}
