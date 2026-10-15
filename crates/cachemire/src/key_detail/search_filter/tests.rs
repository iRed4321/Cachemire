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
