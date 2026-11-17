//! Text as the key panel's search compares it: each character canonically decomposed, so
//! `à` typed or stored as one character or as `a` and a combining accent reads the same,
//! and lowercased unless the search matches case.

use std::borrow::Cow;

use icu_normalizer::properties::{CanonicalCombiningClassMapBorrowed, CanonicalDecompositionBorrowed, Decomposed};

/// Pushes `c` fully decomposed onto `out`.
fn push_decomposed(c: char, out: &mut String) {
    if c.is_ascii() {
        out.push(c);
        return;
    }
    match CanonicalDecompositionBorrowed::new().decompose(c) {
        Decomposed::Default => out.push(c),
        Decomposed::Singleton(one) => push_decomposed(one, out),
        Decomposed::Expansion(first, second) => {
            push_decomposed(first, out);
            push_decomposed(second, out);
        }
    }
}

/// Pushes `c` as the search reads it onto `out`.
fn push_folded(c: char, case_sensitive: bool, out: &mut String) {
    if case_sensitive {
        push_decomposed(c, out);
    } else {
        c.to_lowercase().for_each(|lower| push_decomposed(lower, out));
    }
}

/// `text` as the search reads it: borrowed when that is `text` itself.
pub(super) fn fold(text: &str, case_sensitive: bool) -> Cow<'_, str> {
    if text.is_ascii() {
        let unchanged = case_sensitive || !text.bytes().any(|b| b.is_ascii_uppercase());
        return if unchanged { Cow::Borrowed(text) } else { Cow::Owned(text.to_ascii_lowercase()) };
    }
    let mut out = String::with_capacity(text.len());
    text.chars().for_each(|c| push_folded(c, case_sensitive, &mut out));
    if out == text { Cow::Borrowed(text) } else { Cow::Owned(out) }
}

/// `text` folded, with the offset in `text` of each of its bytes that starts the fold of a
/// character of `text` (and of its end): `None` inside one.
pub(super) fn fold_with_origins(text: &str, case_sensitive: bool) -> (String, Vec<Option<usize>>) {
    let mut out = String::with_capacity(text.len());
    let mut origins = Vec::with_capacity(text.len() + 1);
    for (at, c) in text.char_indices() {
        origins.push(Some(at));
        push_folded(c, case_sensitive, &mut out);
        origins.resize(out.len(), None);
    }
    origins.push(Some(text.len()));
    (out, origins)
}

/// The first place at or after byte `from` where the folded `needle` occurs in the folded
/// `haystack`, not followed by a combining mark: `e` doesn't match the start of `é`.
pub(super) fn find_folded(haystack: &str, needle: &str, mut from: usize) -> Option<usize> {
    let combining = CanonicalCombiningClassMapBorrowed::new();
    while let Some(pos) = haystack.get(from..)?.find(needle) {
        let start = from + pos;
        match haystack[start + needle.len()..].chars().next() {
            Some(next) if !next.is_ascii() && combining.get32_u8(u32::from(next)) != 0 => {
                from = start + haystack[start..].chars().next().map_or(1, char::len_utf8);
            }
            _ => return Some(start),
        }
    }
    None
}

/// Whether the folded `needle` occurs in the folded `haystack` (see [`find_folded`]).
pub(super) fn contains_folded(haystack: &str, needle: &str) -> bool {
    find_folded(haystack, needle, 0).is_some()
}
