//! Search and highlight over already-colored paragraphs: finding matches,
//! layering highlight colors on top of syntax colors, and the collapsed
//! one-line preview.

use std::borrow::Cow;
use std::ops::Range;

use crate::styled_text::StyledText;
use slint::Color;

use crate::theme::paint;

use super::field::FieldValue;
use super::fold::{find_folded, fold, fold_with_origins};
use super::formats::{EditKind, colored_single_line};
use super::pretty_text::{
    COMPACT_PREVIEW_CHAR_BUDGET, ColorSpans, ColoredParagraph, MAX_SHOWN_STRING_BYTES, colored_paragraphs_for,
    compact_paragraphs_with_whole_strings, escaped_paragraphs,
};

// Matches are marked with a background, like a marker pen, in the theme's `paint()` colors:
// `mark_filter` the header value filter's, `mark_current` the current find match's,
// `mark_find` the other ones', `mark_text` the text on them.

/// One match: paragraph index plus byte range within that paragraph.
pub(super) type Match = (usize, Range<usize>);

/// Non-overlapping occurrences of `needle` per paragraph, case-sensitively or not, both
/// read as the search does (`fold`): a match is found in the folded text and mapped back to
/// the text, one that would cover part of a character left out.
pub(super) fn find_matches(paragraphs: &[ColoredParagraph], needle: &str, case_sensitive: bool) -> Vec<Match> {
    let mut out = Vec::new();
    let needle = fold(needle, case_sensitive);
    if needle.is_empty() {
        return out;
    }
    for (i, (text, _)) in paragraphs.iter().enumerate() {
        // (folding ASCII keeps every offset where it was)
        let mapped = (!text.is_ascii()).then(|| fold_with_origins(text, case_sensitive));
        let haystack = match &mapped {
            Some((folded, _)) => Cow::Borrowed(folded.as_str()),
            None => fold(text, case_sensitive),
        };
        let mut from = 0;
        while let Some(start) = find_folded(&haystack, &needle, from) {
            let end = start + needle.len();
            from = end;
            let range = match &mapped {
                Some((_, origins)) => origins[start].zip(origins[end]).map(|(start, end)| start..end),
                None => Some(start..end),
            };
            if let Some(range) = range {
                out.push((i, range));
            }
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
pub(super) fn overlay_highlights(
    paragraphs: &[ColoredParagraph],
    highlights: &[(usize, Range<usize>, Color)],
) -> Vec<HighlightedParagraph> {
    let text_color = paint().mark_text;
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

/// The marks of `paragraphs[range]`, indexed from its start: the search bar's `needles`,
/// and the find matches `finds` (of every paragraph, in order) over them, the
/// `current`-th apart.
pub(super) fn window_highlights(
    paragraphs: &[ColoredParagraph],
    range: Range<usize>,
    needles: &[String],
    case_sensitive: bool,
    finds: &[Match],
    current: i32,
) -> Vec<(usize, Range<usize>, Color)> {
    let shown = &paragraphs[range.clone()];
    let paint = paint();
    let filter_color = paint.mark_filter;
    let mut highlights: Vec<(usize, Range<usize>, Color)> = merge_overlapping(
        needles
            .iter()
            .flat_map(|needle| find_matches(shown, needle, case_sensitive).into_iter().map(|(p, r)| (p, r, filter_color)))
            .collect(),
    );
    if finds.is_empty() {
        return highlights;
    }
    let current = (current.rem_euclid(finds.len() as i32)) as usize;
    let first = finds.partition_point(|(p, _)| *p < range.start);
    let mine: Vec<Match> = finds[first..].iter().take_while(|(p, _)| *p < range.end).map(|(p, r)| (p - range.start, r.clone())).collect();
    // find matches win over filter matches where they overlap; `mine` is in (paragraph,
    // start) order and non-overlapping, so a binary search lands on the only one that could
    highlights.retain(|(p, r, _)| {
        let i = mine.partition_point(|(fp, fr)| (*fp, fr.end) <= (*p, r.start));
        !mine.get(i).is_some_and(|(fp, fr)| fp == p && fr.start < r.end)
    });
    for (i, (p, r)) in mine.into_iter().enumerate() {
        highlights.push((p, r, if first + i == current { paint.mark_current } else { paint.mark_find }));
    }
    highlights.sort_by_key(|(p, r, _)| (*p, r.start));
    highlights
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
    let color = paint().mark_filter;
    let highlights = merge_overlapping(
        needles
            .iter()
            .flat_map(|needle| find_matches(&paragraphs, needle, case_sensitive).into_iter().map(|(p, r)| (p, r, color)))
            .collect(),
    );
    StyledText::from_highlighted_paragraphs(overlay_highlights(&paragraphs, &highlights))
}

/// Collapsed one-line preview, with the search bar's active badges' matches
/// highlighted, as plain and as escaped into a JSON string.
pub(super) fn compact_styled_texts_for(value: FieldValue<'_>, needles: &[String], case_sensitive: bool) -> (StyledText, StyledText) {
    // a long text only ever shows its start, so it's cut before being serialized
    // (escapes only add characters); one too long to show is cut just past the limit
    let value = match value {
        FieldValue::Text(text) if text.len() > MAX_SHOWN_STRING_BYTES => FieldValue::Text(head_chars(text, MAX_SHOWN_STRING_BYTES + 1)),
        FieldValue::Text(text) => FieldValue::Text(head_chars(text, COMPACT_PREVIEW_CHAR_BUDGET)),
        json => json,
    };
    let paragraphs = colored_paragraphs_for(&value, false, COMPACT_PREVIEW_CHAR_BUDGET);
    let escaped = escaped_paragraphs(paragraphs.clone());
    (highlight_needle_matches(paragraphs, needles, case_sensitive), highlight_needle_matches(escaped, needles, case_sensitive))
}

/// Collapsed one-line preview of a value's own text (what YAML, XML and raw text rows show):
/// the start of it colored as `kind`, its lines end to end, the badges' matches highlighted.
pub(super) fn compact_raw_text_for(raw: &str, kind: EditKind, needles: &[String], case_sensitive: bool) -> StyledText {
    let line = colored_single_line(head_chars(raw, COMPACT_PREVIEW_CHAR_BUDGET), kind);
    highlight_needle_matches(vec![line], needles, case_sensitive)
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

/// A hit's text cut down to a window that shows the first place `needle` (any case) occurs.
fn hit_window(paragraph: &ColoredParagraph, needle: &str) -> ColoredParagraph {
    let first = find_matches(std::slice::from_ref(paragraph), needle, false).first().map(|(_, r)| r.start);
    let from = first.map_or(0, |start| {
        let back = paragraph.0[..start].char_indices().rev().nth(HIT_LEAD_CHARS).map_or(0, |(n, _)| n);
        // a window only when the match would otherwise be far from the start
        if back > 0 { back } else { 0 }
    });
    slice_paragraph(paragraph, from, HIT_KEPT_CHARS)
}

/// Marks every place `needle` (any case) occurs in a hit's text, and cuts the
/// text down to a window that shows the first one.
fn mark_hit(paragraph: ColoredParagraph, needle: &str) -> StyledText {
    let needle = needle.to_lowercase();
    let paragraphs = [hit_window(&paragraph, &needle)];
    let color = paint().mark_find;
    let highlights: Vec<(usize, Range<usize>, Color)> =
        find_matches(&paragraphs, &needle, false).into_iter().map(|(p, r)| (p, r, color)).collect();
    StyledText::from_highlighted_paragraphs(overlay_highlights(&paragraphs, &highlights))
}

/// The Display settings' sample: a field holding text and one holding JSON, colored as the
/// rows show them, `billing` marked as a search match; and the JSON's line count.
pub(crate) fn display_sample() -> (StyledText, StyledText, i32) {
    let text = colored_paragraphs_for(&FieldValue::Text("camille@example.com"), false, COMPACT_PREVIEW_CHAR_BUDGET);
    let json = serde_json::json!({
        "user_id": 48213,
        "active": true,
        "plan": null,
        "team": "billing",
        "total": 129.9,
        "last_seen": "2026-11-17T09:42:00Z"
    });
    let paragraphs = colored_paragraphs_for(&FieldValue::Json(&json), true, COMPACT_PREVIEW_CHAR_BUDGET);
    let lines = paragraphs.len() as i32;
    (StyledText::from_colored_paragraphs(text), highlight_needle_matches(paragraphs, &["billing".to_string()], false), lines)
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
    let paragraphs = compact_paragraphs_with_whole_strings(&value, 20_000);
    let paragraph = paragraphs.into_iter().next().unwrap_or_default();
    mark_hit(paragraph, needle)
}

#[cfg(test)]
mod tests;
