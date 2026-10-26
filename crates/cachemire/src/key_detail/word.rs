//! Where a double click selects in a value's pretty text: the word under the click.
//! Offsets are bytes into one of its paragraphs.

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

#[cfg(test)]
mod tests {
    use super::*;

    fn word_at(text: &str, offset: usize) -> Option<&str> {
        word_range(text, offset).map(|(start, end)| &text[start..end])
    }

    #[test]
    fn a_double_click_selects_the_word_it_touches() {
        let text = r#"{"user_id": 42, "name": "Zoë"}"#;
        assert_eq!(word_at(text, text.find("user").unwrap() + 2), Some("user_id"));
        assert_eq!(word_at(text, text.find("42").unwrap()), Some("42"), "the start of a word");
        assert_eq!(word_at(text, text.find("42").unwrap() + 2), Some("42"), "right after its end");
        assert_eq!(word_at(text, text.find("Zoë").unwrap() + 2), Some("Zoë"));
        assert_eq!(word_at(text, text.find(": ").unwrap() + 1), None, "between punctuation and a blank");
    }

    #[test]
    fn an_offset_outside_the_text_or_inside_a_character_selects_nothing() {
        assert_eq!(word_at("é", 1), None);
        assert_eq!(word_at("ab", 3), None);
        assert_eq!(word_at("", 0), None);
    }
}
