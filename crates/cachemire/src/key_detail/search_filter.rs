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
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tokenize_splits_on_spaces_and_keeps_a_quoted_phrase_together() {
        assert_eq!(tokenize("foo bar"), vec!["foo", "bar"]);
        assert_eq!(tokenize("  foo   bar  "), vec!["foo", "bar"]);
        assert_eq!(tokenize(r#""hello world" foo"#), vec!["hello world", "foo"]);
        assert_eq!(tokenize("'it is' done"), vec!["it is", "done"]);
        // an unterminated quote reads to the end of the text
        assert_eq!(tokenize(r#"foo "bar baz"#), vec!["foo", "bar baz"]);
        // an empty quoted phrase contributes nothing
        assert_eq!(tokenize(r#"foo "" bar"#), vec!["foo", "bar"]);
        assert!(tokenize("   ").is_empty());
        assert!(tokenize("").is_empty());
    }

    #[test]
    fn matches_checks_every_term_against_its_own_scope_anded_together() {
        // the `field()` test helper leaves `raw` empty, so a real value needs
        // `Field::from_value` instead, like a loaded key's fields get built
        let f = Field::from_value("hello".into(), json!("a needle in a haystack"));
        let name_term = |text: &str, scope: SearchScope| SearchFilter { terms: vec![SearchTerm { text: text.into(), scope }], case_sensitive: false, ..Default::default() };

        assert!(name_term("hel", SearchScope::Fields).matches(&f));
        assert!(!name_term("needle", SearchScope::Fields).matches(&f));
        assert!(name_term("needle", SearchScope::Value).matches(&f));
        assert!(!name_term("hel", SearchScope::Value).matches(&f));
        assert!(name_term("needle", SearchScope::Everywhere).matches(&f));
        assert!(name_term("hel", SearchScope::Everywhere).matches(&f));
        assert!(!name_term("zzz", SearchScope::Everywhere).matches(&f));

        // AND across terms
        let both = SearchFilter {
            terms: vec![SearchTerm { text: "needle".into(), scope: SearchScope::Value }, SearchTerm { text: "hel".into(), scope: SearchScope::Fields }],
            case_sensitive: false,
            ..Default::default()
        };
        assert!(both.matches(&f));
        let mismatch = SearchFilter {
            terms: vec![SearchTerm { text: "needle".into(), scope: SearchScope::Value }, SearchTerm { text: "zzz".into(), scope: SearchScope::Fields }],
            case_sensitive: false,
            ..Default::default()
        };
        assert!(!mismatch.matches(&f));

        // no terms: matches everything
        assert!(SearchFilter::default().matches(&f));
    }

    #[test]
    fn case_sensitivity_is_one_setting_for_every_term() {
        let f = Field::from_value("Hello".into(), json!("A Needle"));
        let insensitive = SearchFilter { terms: vec![SearchTerm { text: "needle".into(), scope: SearchScope::Value }], case_sensitive: false, ..Default::default() };
        let sensitive = SearchFilter { terms: vec![SearchTerm { text: "needle".into(), scope: SearchScope::Value }], case_sensitive: true, ..Default::default() };
        assert!(insensitive.matches(&f));
        assert!(!sensitive.matches(&f));
        let sensitive_match = SearchFilter { terms: vec![SearchTerm { text: "Needle".into(), scope: SearchScope::Value }], case_sensitive: true, ..Default::default() };
        assert!(sensitive_match.matches(&f));
    }

    #[test]
    fn highlight_needles_drops_field_only_badges() {
        let filter = SearchFilter {
            terms: vec![
                SearchTerm { text: "a".into(), scope: SearchScope::Fields },
                SearchTerm { text: "b".into(), scope: SearchScope::Value },
                SearchTerm { text: "c".into(), scope: SearchScope::Everywhere },
            ],
            case_sensitive: false,
            ..Default::default()
        };
        assert_eq!(filter.highlight_needles(), vec!["b".to_string(), "c".to_string()]);
    }
}
