//! Basic syntax highlighting for the Redisql query box, for the grammar of
//! `cachemire_core::redisql`: keywords, quoted strings, numbers, booleans, `--` comments
//! and punctuation. Line-oriented: quotes and comments don't span lines.

use std::ops::Range;

use crate::theme_colors;
use cachemire_core::redisql::variable_ranges;
use slint::Color;
use slint::private_unstable_api::re_exports::StyledText;

const KEYWORDS: &[&str] = &["from", "join", "where", "select", "limit", "as", "on", "and", "in", "key", "var"];
const BOOLEANS: &[&str] = &["true", "false"];

const KEYWORD_COLOR: Color = theme_colors::ACCENT_BLUE;
const STRING_COLOR: Color = theme_colors::JSON_STRING;
const NUMBER_COLOR: Color = theme_colors::JSON_NUMBER;
const BOOL_COLOR: Color = theme_colors::JSON_BOOL;
const PUNCT_COLOR: Color = theme_colors::JSON_PUNCT;
const VAR_COLOR: Color = theme_colors::QUERY_VARIABLE;
const COMMENT_COLOR: Color = theme_colors::TEXT_TERTIARY;

fn is_ident_start(c: char) -> bool {
    c.is_alphabetic() || c == '_'
}

fn is_ident_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// One line's color spans: byte ranges into that line's own text.
fn highlight_line(line: &str) -> Vec<(Range<usize>, Color)> {
    let mut spans = Vec::new();
    let mut chars = line.char_indices().peekable();
    while let Some(&(i, c)) = chars.peek() {
        // a `--` comment runs to the end of the line
        if c == '-' && line[i + 1..].starts_with('-') {
            spans.push((i..line.len(), COMMENT_COLOR));
            break;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            chars.next();
            let mut end = line.len();
            for (j, cc) in chars.by_ref() {
                if cc == quote {
                    end = j + cc.len_utf8();
                    break;
                }
            }
            // a `&name` variable inside the string gets its own color
            let inner_end = if line[i + 1..end].ends_with(quote) { end - 1 } else { end };
            let mut pos = i;
            for range in variable_ranges(&line[i + 1..inner_end]) {
                let (from, to) = (i + 1 + range.start, i + 1 + range.end);
                spans.push((pos..from, STRING_COLOR));
                spans.push((from..to, VAR_COLOR));
                pos = to;
            }
            spans.push((pos..end, STRING_COLOR));
            continue;
        }
        if c.is_ascii_digit() {
            let mut end = i + c.len_utf8();
            chars.next();
            while let Some(&(j, cc)) = chars.peek() {
                if !cc.is_ascii_digit() && cc != '.' {
                    break;
                }
                end = j + cc.len_utf8();
                chars.next();
            }
            spans.push((i..end, NUMBER_COLOR));
            continue;
        }
        if is_ident_start(c) {
            let mut end = i + c.len_utf8();
            chars.next();
            while let Some(&(j, cc)) = chars.peek() {
                if !is_ident_char(cc) {
                    break;
                }
                end = j + cc.len_utf8();
                chars.next();
            }
            let word = &line[i..end];
            if KEYWORDS.iter().any(|k| k.eq_ignore_ascii_case(word)) {
                spans.push((i..end, KEYWORD_COLOR));
            } else if BOOLEANS.iter().any(|k| k.eq_ignore_ascii_case(word)) {
                spans.push((i..end, BOOL_COLOR));
            }
            continue;
        }
        if matches!(c, '.' | ',' | ';' | '=' | '[' | ']' | '(' | ')' | '*') {
            spans.push((i..i + c.len_utf8(), PUNCT_COLOR));
        }
        chars.next();
    }
    spans
}

/// A line, its color spans, and the part of an error on it.
type HighlightedLine = (String, Vec<(Range<usize>, Color)>, Option<Range<usize>>);

/// Each line of `text` with its color spans and the part of `error` (byte offsets into
/// `text`) on it, all in the line's own byte offsets. A trailing `\r` is dropped from each line.
fn highlighted_lines(text: &str, error: Option<Range<usize>>) -> Vec<HighlightedLine> {
    let mut line_start = 0;
    text.split('\n')
        .map(|raw| {
            let line = raw.strip_suffix('\r').unwrap_or(raw);
            let marked = error
                .as_ref()
                .map(|e| e.start.max(line_start).saturating_sub(line_start)..e.end.min(line_start + line.len()).saturating_sub(line_start))
                .filter(|r| r.start < r.end);
            line_start += raw.len() + 1;
            (line.to_string(), highlight_line(line), marked)
        })
        .collect()
}

/// `text` as one `StyledText`, one paragraph per line (see `highlighted_lines`), with
/// `error` marked.
pub fn highlight_query(text: &str, error: Option<Range<usize>>) -> StyledText {
    // translucent, behind the text where the last run's error is
    let error_background = theme_colors::ACCENT_RED.with_alpha(0.4);
    StyledText::from_highlighted_paragraphs(
        highlighted_lines(text, error)
            .into_iter()
            .map(|(line, spans, marked)| (line, spans, marked.map(|r| (r, error_background)).into_iter().collect())),
    )
}

/// Collapses `\r\n` and lone `\r` to `\n`, so the query box's buffer never holds a
/// `\r` from a pasted Windows query (see `redisql::init`).
pub fn normalize_line_endings(text: &str) -> String {
    if !text.contains('\r') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' {
            out.push('\n');
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests;
