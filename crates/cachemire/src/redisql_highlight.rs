//! Basic syntax highlighting for the Redisql query box, for the grammar of
//! `backend::redisql`: keywords, quoted strings, numbers, booleans, `--` comments
//! and punctuation. Line-oriented: quotes and comments don't span lines.

use std::ops::Range;

use crate::backend::redisql::variable_ranges;
use crate::theme_colors;
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

/// `text` as one `StyledText`, one paragraph per line, each line's spans in its own
/// byte offsets, with `error` (byte offsets into `text`) marked. A trailing `\r` is
/// dropped from each line.
pub fn highlight_query(text: &str, error: Option<Range<usize>>) -> StyledText {
    // translucent, behind the text where the last run's error is
    let error_background = theme_colors::ACCENT_RED.with_alpha(0.4);
    let mut line_start = 0;
    StyledText::from_highlighted_paragraphs(text.split('\n').map(|raw| {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let marked = error
            .as_ref()
            .map(|e| e.start.max(line_start).saturating_sub(line_start)..e.end.min(line_start + line.len()).saturating_sub(line_start))
            .filter(|r| r.start < r.end);
        line_start += raw.len() + 1;
        (line.to_string(), highlight_line(line), marked.map(|r| (r, error_background)).into_iter().collect())
    }))
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
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(&str, &str)> {
        highlight_line(line).into_iter().map(|(range, color)| (&line[range], color_name(color))).collect()
    }

    fn color_name(color: Color) -> &'static str {
        match color {
            c if c == KEYWORD_COLOR => "keyword",
            c if c == STRING_COLOR => "string",
            c if c == NUMBER_COLOR => "number",
            c if c == BOOL_COLOR => "bool",
            c if c == PUNCT_COLOR => "punct",
            c if c == COMMENT_COLOR => "comment",
            _ => "?",
        }
    }

    #[test]
    fn clause_keywords_are_recognized_whatever_the_case() {
        assert_eq!(kinds("FROM KEY 'k' AS x"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'k'", "string"), ("AS", "keyword")]);
        assert_eq!(kinds("from key 'k' as x"), vec![("from", "keyword"), ("key", "keyword"), ("'k'", "string"), ("as", "keyword")]);
    }

    #[test]
    fn a_word_that_only_starts_with_a_keyword_is_not_one() {
        assert_eq!(kinds("SELECT selected"), vec![("SELECT", "keyword")]);
    }

    #[test]
    fn strings_numbers_and_booleans_are_colored() {
        assert_eq!(kinds("WHERE x.qty = 3 AND x.ok = true"), vec![
            ("WHERE", "keyword"),
            (".", "punct"),
            ("=", "punct"),
            ("3", "number"),
            ("AND", "keyword"),
            (".", "punct"),
            ("=", "punct"),
            ("true", "bool"),
        ]);
        assert_eq!(kinds("x.label = 'a,b'"), vec![(".", "punct"), ("=", "punct"), ("'a,b'", "string")]);
    }

    #[test]
    fn a_double_dash_comment_runs_to_end_of_line_but_not_inside_a_string() {
        assert_eq!(kinds("SELECT * -- everything"), vec![("SELECT", "keyword"), ("*", "punct"), ("-- everything", "comment")]);
        assert_eq!(kinds("FROM KEY 'a--b' AS x"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'a--b'", "string"), ("AS", "keyword")]);
    }

    #[test]
    fn an_in_list_colors_its_brackets_and_values() {
        assert_eq!(kinds("WHERE x.id IN [1, 2, 'c']"), vec![
            ("WHERE", "keyword"),
            (".", "punct"),
            ("IN", "keyword"),
            ("[", "punct"),
            ("1", "number"),
            (",", "punct"),
            ("2", "number"),
            (",", "punct"),
            ("'c'", "string"),
            ("]", "punct"),
        ]);
    }

    #[test]
    fn an_unterminated_string_still_colors_to_the_end_of_the_line() {
        assert_eq!(kinds("FROM KEY 'unterminated"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'unterminated", "string")]);
    }

    #[test]
    fn plain_identifiers_and_whitespace_get_no_span_but_the_dot_between_them_does() {
        assert_eq!(kinds("i.productId p.label"), vec![(".", "punct"), (".", "punct")]);
    }

    #[test]
    fn a_trailing_carriage_return_is_not_colored_as_part_of_the_line() {
        // a pasted Windows query still has `\r\n` for one frame, before
        // `normalize_line_endings` catches up (see redisql::init)
        assert_eq!(kinds("FROM KEY 'k'\r"), vec![("FROM", "keyword"), ("KEY", "keyword"), ("'k'", "string")]);
    }

    #[test]
    fn crlf_and_lone_cr_both_collapse_to_lf() {
        assert_eq!(normalize_line_endings("FROM KEY 'k'\r\nSELECT *\r\nLIMIT 1"), "FROM KEY 'k'\nSELECT *\nLIMIT 1");
        assert_eq!(normalize_line_endings("a\rb"), "a\nb");
        assert_eq!(normalize_line_endings("no newlines here"), "no newlines here");
    }
}
