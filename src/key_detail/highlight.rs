//! Search and highlight over already-colored paragraphs: finding matches,
//! layering highlight colors on top of syntax colors, and the collapsed
//! one-line preview.

use std::ops::Range;

use slint::Color;
use slint::private_unstable_api::re_exports::StyledText;

use crate::theme_colors::{ACCENT_ORANGE, ACCENT_YELLOW, BG_APP_BASE, HIGHLIGHT_FIND};

use super::field::FieldValue;
use super::pretty_text::{COMPACT_PREVIEW_CHAR_BUDGET, ColorSpans, ColoredParagraph, colored_paragraphs_for, escaped_paragraphs};

// Matches are marked with a background, like a marker pen; the text on it is dark
const HIGHLIGHT_FILTER: Color = ACCENT_YELLOW; // the header value filter's matches
const HIGHLIGHT_CURRENT: Color = ACCENT_ORANGE; // the current find match
const HIGHLIGHT_TEXT: Color = BG_APP_BASE;

/// One match: paragraph index plus byte range within that paragraph.
type Match = (usize, Range<usize>);

/// Non-overlapping occurrences of `needle` per paragraph, case-sensitively or
/// not. Case-insensitively, offsets come from the lowercased text, trusted
/// only when lowercasing kept the byte length (always true for ASCII).
pub(super) fn find_matches(paragraphs: &[ColoredParagraph], needle: &str, case_sensitive: bool) -> Vec<Match> {
    let mut out = Vec::new();
    if needle.is_empty() {
        return out;
    }
    let needle_lower;
    let needle: &str = if case_sensitive {
        needle
    } else {
        needle_lower = needle.to_lowercase();
        &needle_lower
    };
    for (i, (text, _)) in paragraphs.iter().enumerate() {
        let lower;
        let haystack: &str = if case_sensitive {
            text
        } else {
            lower = text.to_lowercase();
            if lower.len() == text.len() { &lower } else { text }
        };
        let mut from = 0;
        while let Some(pos) = haystack[from..].find(needle) {
            let start = from + pos;
            let end = start + needle.len();
            if text.is_char_boundary(start) && text.is_char_boundary(end) {
                out.push((i, start..end));
            }
            from = end;
        }
    }
    out
}

/// Merges the ranges of `highlights` that overlap or touch, per paragraph, since
/// `overlay_highlights` expects non-overlapping spans. Safe because they all share
/// one color.
fn merge_overlapping(mut highlights: Vec<(usize, Range<usize>, Color)>) -> Vec<(usize, Range<usize>, Color)> {
    highlights.sort_by_key(|(p, r, _)| (*p, r.start));
    let mut merged: Vec<(usize, Range<usize>, Color)> = Vec::with_capacity(highlights.len());
    for (p, r, c) in highlights {
        if let Some(last) = merged.last_mut()
            && last.0 == p
            && r.start <= last.1.end
        {
            last.1.end = last.1.end.max(r.end);
            continue;
        }
        merged.push((p, r, c));
    }
    merged
}

/// A paragraph with the background spans (a range and its highlight color)
/// that mark its matches.
type HighlightedParagraph = (String, ColorSpans, Vec<(Range<usize>, Color)>);

/// Marks `highlights` (sorted, non-overlapping per paragraph) in `paragraphs`: each
/// gets a background color and dark text, splitting the syntax-colored base spans
/// around it while keeping the list flat and ordered.
pub(super) fn overlay_highlights(paragraphs: &[ColoredParagraph], highlights: &[(usize, Range<usize>, Color)]) -> Vec<HighlightedParagraph> {
    let text_color = HIGHLIGHT_TEXT;
    // highlights arrive in paragraph order, so one cursor walks them in step
    // with the paragraphs instead of rescanning the whole list for each
    let mut next = 0;
    paragraphs
        .iter()
        .enumerate()
        .map(|(i, (text, spans))| {
            let first = next;
            while next < highlights.len() && highlights[next].0 == i {
                next += 1;
            }
            let mine = &highlights[first..next];
            if mine.is_empty() {
                return (text.clone(), spans.clone(), Vec::new());
            }

            let mut out: Vec<(Range<usize>, Color)> = Vec::with_capacity(spans.len() + mine.len() * 2);
            for (range, color) in spans {
                let mut pos = range.start;
                for (_, h, _) in mine {
                    if h.end <= pos || h.start >= range.end {
                        continue;
                    }
                    if h.start > pos {
                        out.push((pos..h.start, *color));
                    }
                    pos = pos.max(h.end);
                }
                if pos < range.end {
                    out.push((pos..range.end, *color));
                }
            }
            out.extend(mine.iter().map(|(_, r, _)| (r.clone(), text_color)));
            out.sort_by_key(|(r, _)| r.start);
            (text.clone(), out, mine.iter().map(|(_, r, c)| (r.clone(), *c)).collect())
        })
        .collect()
}

/// Builds the on-screen text for a cached pretty rendering: the search bar's
/// active badges' matches in yellow, the find query's in orange, and the
/// `current`-th find match in cyan.
pub(super) fn pretty_styled_text_with_highlights(paragraphs: &[ColoredParagraph], filter_needles: &[String], filter_case_sensitive: bool, query: &str, current: i32) -> StyledText {
    let filter_color = HIGHLIGHT_FILTER;
    let mut highlights: Vec<(usize, Range<usize>, Color)> = merge_overlapping(
        filter_needles
            .iter()
            .flat_map(|needle| find_matches(paragraphs, needle, filter_case_sensitive).into_iter().map(|(p, r)| (p, r, filter_color)))
            .collect(),
    );
    let finds = find_matches(paragraphs, query, false);
    if !finds.is_empty() {
        let current = (current.rem_euclid(finds.len() as i32)) as usize;
        let find_color = HIGHLIGHT_FIND;
        let current_color = HIGHLIGHT_CURRENT;
        // find matches win over filter matches where they overlap; `finds` is
        // in (paragraph, start) order and non-overlapping, so a binary search
        // lands on the only find that could overlap a given filter match
        highlights.retain(|(p, r, _)| {
            let i = finds.partition_point(|(fp, fr)| (*fp, fr.end) <= (*p, r.start));
            !finds.get(i).is_some_and(|(fp, fr)| fp == p && fr.start < r.end)
        });
        for (i, (p, r)) in finds.into_iter().enumerate() {
            highlights.push((p, r, if i == current { current_color } else { find_color }));
        }
        highlights.sort_by_key(|(p, r, _)| (*p, r.start));
    }
    let rendered = if highlights.is_empty() {
        paragraphs.iter().map(|(text, spans)| (text.clone(), spans.clone(), Vec::new())).collect()
    } else {
        overlay_highlights(paragraphs, &highlights)
    };
    StyledText::from_highlighted_paragraphs(rendered)
}

/// The first `n` characters of `s`.
pub(super) fn head_chars(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((end, _)) => &s[..end],
        None => s,
    }
}

/// Marks every place any of `needles` occurs in `paragraphs`, with the search
/// bar's filter color — shared by the field/value list's collapsed previews
/// and the table view's cells. `paragraphs` back unmarked when `needles` is empty.
pub(super) fn highlight_needle_matches(paragraphs: Vec<ColoredParagraph>, needles: &[String], case_sensitive: bool) -> StyledText {
    if needles.is_empty() {
        return StyledText::from_colored_paragraphs(paragraphs);
    }
    let color = HIGHLIGHT_FILTER;
    let highlights = merge_overlapping(
        needles.iter().flat_map(|needle| find_matches(&paragraphs, needle, case_sensitive).into_iter().map(|(p, r)| (p, r, color))).collect(),
    );
    StyledText::from_highlighted_paragraphs(overlay_highlights(&paragraphs, &highlights))
}

/// Collapsed one-line preview, with the search bar's active badges' matches
/// highlighted, as plain and as escaped into a JSON string.
pub(super) fn compact_styled_texts_for(value: FieldValue<'_>, needles: &[String], case_sensitive: bool) -> (StyledText, StyledText) {
    // a long text only ever shows its start, so it's cut before being
    // serialized (escapes only add characters, never remove any)
    let value = match value {
        FieldValue::Text(text) => FieldValue::Text(head_chars(text, COMPACT_PREVIEW_CHAR_BUDGET)),
        json => json,
    };
    let paragraphs = colored_paragraphs_for(&value, false, COMPACT_PREVIEW_CHAR_BUDGET);
    let escaped = escaped_paragraphs(paragraphs.clone());
    (highlight_needle_matches(paragraphs, needles, case_sensitive), highlight_needle_matches(escaped, needles, case_sensitive))
}

/// How much of a hit's value is kept for its row, and how far before the first
/// match the kept part starts (the row shows one clipped line, so the match has
/// to be near its start).
const HIT_KEPT_CHARS: usize = 400;
const HIT_LEAD_CHARS: usize = 24;

/// `paragraph` from byte `from` on, at most `keep` chars of it, with a leading
/// `…` when it does not start at the beginning; the color spans follow.
fn slice_paragraph(paragraph: &ColoredParagraph, from: usize, keep: usize) -> ColoredParagraph {
    let (text, spans) = paragraph;
    let to = text[from..].char_indices().nth(keep).map_or(text.len(), |(n, _)| from + n);
    let lead = if from > 0 { "…" } else { "" };
    let spans = spans
        .iter()
        .filter(|(range, _)| range.end > from && range.start < to)
        .map(|(range, color)| (range.start.max(from) - from + lead.len()..range.end.min(to) - from + lead.len(), *color))
        .collect();
    (format!("{lead}{}", &text[from..to]), spans)
}

/// Marks every place `needle` (any case) occurs in a hit's text, and cuts the
/// text down to a window that shows the first one.
fn mark_hit(paragraph: ColoredParagraph, needle: &str) -> StyledText {
    let needle = needle.to_lowercase();
    let first = find_matches(std::slice::from_ref(&paragraph), &needle, false).first().map(|(_, r)| r.start);
    let from = first.map_or(0, |start| {
        let back = paragraph.0[..start].char_indices().rev().nth(HIT_LEAD_CHARS).map_or(0, |(n, _)| n);
        // a window only when the match would otherwise be far from the start
        if back > 0 { back } else { 0 }
    });
    let paragraph = slice_paragraph(&paragraph, from, HIT_KEPT_CHARS);
    let paragraphs = [paragraph];
    let color = HIGHLIGHT_FIND;
    let highlights: Vec<(usize, Range<usize>, Color)> =
        find_matches(&paragraphs, &needle, false).into_iter().map(|(p, r)| (p, r, color)).collect();
    StyledText::from_highlighted_paragraphs(overlay_highlights(&paragraphs, &highlights))
}

/// A hit's key or field name: one line, its matches marked.
pub(crate) fn search_plain_text(text: &str, needle: &str) -> StyledText {
    mark_hit((text.to_string(), Vec::new()), needle)
}

/// A hit's value as the collapsed rows show one (colored compact JSON, or a
/// JSON string for plain text), cut to a window around the first match, with
/// the matches marked.
pub(crate) fn search_value_text(raw: &str, needle: &str) -> StyledText {
    // a container only: a bare number or `true` in a string stays the text it is
    let json = serde_json::from_str::<serde_json::Value>(raw).ok().filter(|v| v.is_object() || v.is_array());
    let value = match &json {
        Some(json) => FieldValue::Json(json),
        None => FieldValue::Text(raw),
    };
    let paragraphs = colored_paragraphs_for(&value, false, 20_000);
    let paragraph = paragraphs.into_iter().next().unwrap_or_default();
    mark_hit(paragraph, needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(hex: &str) -> Color {
        let channel = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).unwrap();
        Color::from_rgb_u8(channel(1), channel(3), channel(5))
    }

    #[test]
    fn find_matches_is_case_insensitive_and_non_overlapping() {
        let paragraphs = vec![("Hello hello".to_string(), vec![]), ("aaa".to_string(), vec![])];
        assert_eq!(find_matches(&paragraphs, "hello", false), vec![(0, 0..5), (0, 6..11)]);
        assert_eq!(find_matches(&paragraphs, "aa", false), vec![(1, 0..2)]);
        assert!(find_matches(&paragraphs, "", false).is_empty());
    }

    #[test]
    fn overlay_splits_base_spans_around_highlights() {
        let paragraphs = vec![("abcdefgh".to_string(), vec![(0..4, c("#111111")), (4..8, c("#222222"))])];
        let out = overlay_highlights(&paragraphs, &[(0, 2..6, c("#333333"))]);
        let spans: Vec<Range<usize>> = out[0].1.iter().map(|(r, _)| r.clone()).collect();
        assert_eq!(spans, vec![0..2, 2..6, 6..8]);
        // the marked text is drawn in the dark color, on the highlight's background
        assert_eq!(out[0].1[1].1, HIGHLIGHT_TEXT);
        assert_eq!(out[0].2, vec![(2..6, c("#333333"))]);
    }

    #[test]
    fn overlay_walks_highlights_paragraph_by_paragraph() {
        let paragraphs = vec![
            ("aaaa".to_string(), vec![(0..4, c("#111111"))]),
            ("bbbb".to_string(), vec![(0..4, c("#111111"))]),
            ("cccc".to_string(), vec![(0..4, c("#111111"))]),
        ];
        let out = overlay_highlights(&paragraphs, &[(0, 0..1, c("#222222")), (2, 1..3, c("#333333")), (2, 3..4, c("#444444"))]);
        let spans = |i: usize| out[i].1.iter().map(|(r, _)| r.clone()).collect::<Vec<_>>();
        assert_eq!(spans(0), vec![0..1, 1..4]);
        assert_eq!(spans(1), vec![0..4]);
        assert_eq!(spans(2), vec![0..1, 1..3, 3..4]);
    }
}
