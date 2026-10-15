use serde_json::json;

use super::super::test_support::field;
use super::*;

fn fields() -> Vec<Arc<Field>> {
    vec![
        field("one", Some(json!({"records": [{"label": "a", "id": 1}, {"label": "b", "extra": true}], "name": "n", "a.b": 1}))),
        field("two", Some(json!({"records": [{"label": "c"}], "Nested": {"deep": {"label": 1}}}))),
        field("text", None),
    ]
}

fn keys(text: &str) -> Vec<String> {
    let at = text.len();
    Suggester::default().complete(&fields(), text, at).map(|c| c.items.into_iter().map(|i| i.key).collect()).unwrap_or_default()
}

#[test]
fn a_dot_lists_what_the_values_have_at_that_path() {
    assert_eq!(keys("records."), ["*", "0", "1"]);
    assert_eq!(keys("records.*."), ["*", "label", "id", "extra"]);
    assert_eq!(keys("records[0]."), ["*", "label", "id"]);
    assert_eq!(keys("$."), ["*", "records", "name", "a.b", "Nested"]);
    assert_eq!(keys("records.0.label."), Vec::<String>::new(), "a string has nothing under it");
    assert_eq!(keys("nothing."), Vec::<String>::new());
    assert_eq!(keys("records[."), Vec::<String>::new(), "no completion inside a bracket");
}

#[test]
fn the_first_segment_completes_once_something_is_typed() {
    assert_eq!(keys(""), Vec::<String>::new());
    assert_eq!(keys("re"), ["records"]);
    assert_eq!(keys("$re"), Vec::<String>::new(), "`$` needs a separator");
}

#[test]
fn what_is_typed_after_the_dot_narrows_the_list_and_ranks_prefixes_first() {
    assert_eq!(keys("records.*.l"), ["label"]);
    // contains, not just starts with; those that start with it first
    assert_eq!(keys("$.a"), ["a.b", "name"]);
    assert_eq!(keys("$.NAM"), ["name"]);
    let completion = Suggester::default().complete(&fields(), "$.ecor", 6).unwrap();
    assert_eq!(completion.items, vec![Item { key: "records".into(), matched: (1, 5) }]);
    assert_eq!(keys("records.*.label"), Vec::<String>::new(), "already what is typed");
    assert_eq!(keys("records.*.zzz"), Vec::<String>::new());
}

#[test]
fn two_dots_list_the_keys_at_any_depth() {
    assert_eq!(keys(".."), ["records", "label", "id", "extra", "name", "a.b", "Nested", "deep"]);
    assert_eq!(keys("Nested.."), ["deep", "label"]);
    assert_eq!(keys("..la"), ["label"]);
}

#[test]
fn accepting_replaces_the_word_and_quotes_what_cannot_be_typed() {
    let suggester = Suggester::default();
    let accept = |text: &str, cursor: usize, key: &str| {
        let completion = suggester.complete(&fields(), text, cursor).unwrap();
        let index = completion.items.iter().position(|i| i.key == key).unwrap();
        completion.accept(text, index).unwrap()
    };
    assert_eq!(accept("re", 2, "records"), Edit { text: "records".into(), cursor: 7 });
    assert_eq!(accept("records.*.l", 11, "label"), Edit { text: "records.*.label".into(), cursor: 15 });
    // the rest of the word being edited goes too, the rest of the path stays
    assert_eq!(accept("rec.x.label", 2, "records"), Edit { text: "records.x.label".into(), cursor: 7 });
    assert_eq!(accept("records.", 8, "*"), Edit { text: "records.*".into(), cursor: 9 });
    // a key holding a dot takes the bracket form, in place of the dot
    assert_eq!(accept("$.", 2, "a.b"), Edit { text: "$[\"a.b\"]".into(), cursor: 8 });
    assert_eq!(accept("..", 2, "a.b").text, "..[\"a.b\"]");
    // and the result is a path that parses
    assert!(json_path::parse("$[\"a.b\"]").is_ok() && json_path::parse("..[\"a.b\"]").is_ok());
}

#[test]
fn what_was_found_is_kept_until_the_fields_change() {
    let suggester = Suggester::default();
    let one = fields();
    assert!(suggester.complete(&one, "records.", 8).is_some());
    assert!(suggester.complete(&[], "records.", 8).is_some(), "kept, so not read again");
    suggester.forget();
    assert!(suggester.complete(&[], "records.", 8).is_none());
}

#[test]
fn a_match_is_found_whatever_the_case_and_reported_in_bytes() {
    assert_eq!(find_ignoring_case("Élan", "ÉL"), Some((0, 3)));
    assert_eq!(find_ignoring_case("xélan", "LA"), Some((3, 5)));
    assert_eq!(find_ignoring_case("abc", ""), Some((0, 0)));
    assert_eq!(find_ignoring_case("abc", "d"), None);
}
