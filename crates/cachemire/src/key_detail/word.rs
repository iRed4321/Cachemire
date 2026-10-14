//! Where a double or triple click selects in a value's pretty text: the word under the click, or
//! all of it. Offsets are bytes into the text as a `StyledText` reports them (its lines joined by `\n`).

use super::pretty_text::ColoredParagraph;

/// The text a `StyledText` built from `paragraphs` selects in.
pub(super) fn plain_text(paragraphs: &[ColoredParagraph]) -> String {
    let mut text = String::new();
    for (line, _) in paragraphs {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(line);
    }
    text
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The byte range of the word at `offset` (a caret place: the word it touches, the one after it
/// first), or `None` on punctuation and blanks.
pub(super) fn word_range(text: &str, offset: usize) -> Option<(usize, usize)> {
    if offset > text.len() || !text.is_char_boundary(offset) {
        return None;
    }
    let next = text[offset..].chars().next().filter(|c| is_word_char(*c));
    let before = text[..offset].chars().next_back().filter(|c| is_word_char(*c));
    if next.is_none() && before.is_none() {
        return None;
    }
    let start = text[..offset].char_indices().rev().take_while(|(_, c)| is_word_char(*c)).last().map_or(offset, |(i, _)| i);
    let end = text[offset..].char_indices().find(|(_, c)| !is_word_char(*c)).map_or(text.len(), |(i, _)| offset + i);
    Some((start, end))
}
