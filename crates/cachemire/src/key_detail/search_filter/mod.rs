//! The field/value search bar: parsing what's typed into terms, and matching/
//! highlighting fields against whatever badges are currently active.

use std::sync::OnceLock;

use crate::SearchScope;

use super::field::Field;

/// One active badge: text to look for, and where.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SearchTerm {
    pub(super) text: String,
    pub(super) scope: SearchScope,
}

/// The search bar's state: every active badge, ANDed together, matched
/// case-sensitively or not (one setting for all of them).
#[derive(Default)]
pub(super) struct SearchFilter {
    pub(super) terms: Vec<SearchTerm>,
    pub(super) case_sensitive: bool,
    // each term's text lowercased once (case-insensitive mode only), the
    // first time this instance matches any field, then reused for every
    // other field in the same pass instead of redone per field
    lower_terms: OnceLock<Vec<String>>,
}

impl Clone for SearchFilter {
    fn clone(&self) -> Self {
        SearchFilter { terms: self.terms.clone(), case_sensitive: self.case_sensitive, lower_terms: OnceLock::new() }
    }
}

/// Same badges and case setting; the lowercasing cache doesn't count.
impl PartialEq for SearchFilter {
    fn eq(&self, other: &Self) -> bool {
        self.terms == other.terms && self.case_sensitive == other.case_sensitive
    }
}

impl SearchFilter {
    pub(super) fn new(terms: Vec<SearchTerm>, case_sensitive: bool) -> Self {
        SearchFilter { terms, case_sensitive, lower_terms: OnceLock::new() }
    }

    fn lower_terms(&self) -> &[String] {
        self.lower_terms.get_or_init(|| self.terms.iter().map(|t| t.text.to_lowercase()).collect())
    }

    pub(super) fn matches(&self, field: &Field) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        // `field`'s own lowercasing is memoized on `Field` itself (persists
        // across every future keystroke too); only the terms' need caching
        // here, since a fresh `SearchFilter` is built on every keystroke
        let lower_terms = (!self.case_sensitive).then(|| self.lower_terms());
        self.terms.iter().enumerate().all(|(i, term)| self.term_matches(term, lower_terms.map(|t| t[i].as_str()), field))
    }

    fn term_matches(&self, term: &SearchTerm, lower_text: Option<&str>, field: &Field) -> bool {
        let hay = |s: &str, lower: fn(&Field) -> &str| {
            if self.case_sensitive { s.contains(&term.text) } else { lower(field).contains(lower_text.unwrap()) }
        };
        match term.scope {
            SearchScope::Fields => hay(&field.name, Field::lower_name),
            SearchScope::Value => hay(&field.raw, Field::lower_raw),
            SearchScope::Everywhere => hay(&field.name, Field::lower_name) || hay(&field.raw, Field::lower_raw),
        }
    }

    /// The badges whose match can show inside the value text (a field-name-only
    /// badge has nothing to highlight there), for the value's highlighting.
    pub(super) fn highlight_needles(&self) -> Vec<String> {
        self.terms.iter().filter(|t| t.scope != SearchScope::Fields).map(|t| t.text.clone()).collect()
    }
}

/// Splits typed text into terms: runs of non-space characters, except a run
/// starting with `'` or `"`, which is one term up to the matching quote (or the end
/// of the text).
pub(super) fn tokenize(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut terms = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }
        if chars[i] == '\'' || chars[i] == '"' {
            let quote = chars[i];
            i += 1;
            let start = i;
            while i < chars.len() && chars[i] != quote {
                i += 1;
            }
            let term: String = chars[start..i].iter().collect();
            if i < chars.len() {
                i += 1; // the closing quote
            }
            if !term.is_empty() {
                terms.push(term);
            }
        } else {
            let start = i;
            while i < chars.len() && !chars[i].is_whitespace() {
                i += 1;
            }
            terms.push(chars[start..i].iter().collect());
        }
    }
    terms
}

#[cfg(test)]
mod tests;
