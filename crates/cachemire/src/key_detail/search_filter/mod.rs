//! The field/value search bar: matching and highlighting fields against
//! whatever badges are currently active.

use std::sync::OnceLock;

use crate::SearchScope;

use super::field::Field;
use super::fold::{contains_folded, fold};

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
    // each term's text folded (`fold`) once, the first time this instance matches
    // a field, then reused for every other field in the same pass
    folded_terms: OnceLock<Vec<String>>,
}

impl Clone for SearchFilter {
    fn clone(&self) -> Self {
        SearchFilter { terms: self.terms.clone(), case_sensitive: self.case_sensitive, folded_terms: OnceLock::new() }
    }
}

/// Same badges and case setting; the folding cache doesn't count.
impl PartialEq for SearchFilter {
    fn eq(&self, other: &Self) -> bool {
        self.terms == other.terms && self.case_sensitive == other.case_sensitive
    }
}

impl SearchFilter {
    pub(super) fn new(terms: Vec<SearchTerm>, case_sensitive: bool) -> Self {
        SearchFilter { terms, case_sensitive, folded_terms: OnceLock::new() }
    }

    fn folded_terms(&self) -> &[String] {
        self.folded_terms.get_or_init(|| self.terms.iter().map(|t| fold(&t.text, self.case_sensitive).into_owned()).collect())
    }

    pub(super) fn matches(&self, field: &Field) -> bool {
        if self.terms.is_empty() {
            return true;
        }
        // the field's own folding is kept on `Field` (for every later badge too)
        let folded = self.folded_terms();
        self.terms.iter().zip(folded).all(|(term, needle)| self.term_matches(term.scope, needle, field))
    }

    fn term_matches(&self, scope: SearchScope, needle: &str, field: &Field) -> bool {
        let name = || contains_folded(field.searched_name(self.case_sensitive), needle);
        let value = || {
            contains_folded(field.searched_raw(self.case_sensitive), needle)
                || field.searched_unescaped(self.case_sensitive).is_some_and(|text| contains_folded(text, needle))
        };
        match scope {
            SearchScope::Fields => name(),
            SearchScope::Value => value(),
            SearchScope::Everywhere => name() || value(),
        }
    }

    /// The badges whose match can show inside the value text (a field-name-only
    /// badge has nothing to highlight there), for the value's highlighting.
    pub(super) fn highlight_needles(&self) -> Vec<String> {
        self.terms.iter().filter(|t| t.scope != SearchScope::Fields).map(|t| t.text.clone()).collect()
    }
}

#[cfg(test)]
mod tests;
