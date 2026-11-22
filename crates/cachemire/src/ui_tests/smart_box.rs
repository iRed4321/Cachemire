//! The key header's search box: words entered become search badges that keep only the fields
//! matching all of them, a `$` path filters the JSON values, and its list says where to search.

use super::formats::show_seeded;
use super::links::{links_ready, show_connection};
use super::*;
use crate::SearchScope;
use serde_json::json;

/// The app showing a hash of five fields, two of them JSON documents.
fn key_shown() -> (Harness, Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn) {
    let (mut h, app, rt, turn, _) = links_ready(&[("a", "redis://cache.test:6379")]);
    show_connection(&h, "a");
    let fields = [
        ("name", json!("Ada Lovelace")),
        ("city", json!("London")),
        ("note", json!("moved to london in 1833")),
        ("profile", json!({"age": 36, "langs": ["en", "fr"]})),
        ("settings", json!({"theme": "dark", "langs": ["de"]})),
    ];
    app.key_detail.seed_hash("a", "user:1", &fields);
    show_seeded(&mut h, &app, &rt, "user:1");
    (h, app, rt, turn)
}

/// The fields the panel lists, by name.
fn shown(h: &Harness) -> Vec<String> {
    let mut fields: Vec<String> = h.window.global::<KeyPanel>().get_all_values_rows().iter().map(|row| row.field.to_string()).collect();
    fields.sort();
    fields
}

/// Runs the window until the panel lists `fields`; panics with what it lists otherwise.
fn until_shown(h: &mut Harness, fields: &[&str]) {
    let reached = wait_for(h, |h| shown(h) == fields);
    assert!(reached, "expected {fields:?}, the panel lists {:?}", shown(h));
}

/// Types `text` in the box and presses Enter.
fn enter(h: &Harness, text: &str) {
    let panel = h.window.global::<KeyPanel>();
    panel.set_filter_query(text.into());
    panel.invoke_filter_query_submitted();
}

fn badges(h: &Harness) -> Vec<(String, SearchScope)> {
    h.window.global::<KeyPanel>().get_filter_badges().iter().map(|b| (b.text.to_string(), b.scope)).collect()
}

const ALL: [&str; 5] = ["city", "name", "note", "profile", "settings"];

#[test]
fn words_entered_become_a_badge_that_keeps_the_fields_holding_them() {
    let (mut h, _app, _rt, _turn) = key_shown();
    until_shown(&mut h, &ALL);
    enter(&h, "london");
    assert_eq!(badges(&h), [("london".to_string(), SearchScope::Everywhere)]);
    assert_eq!(h.window.global::<KeyPanel>().get_filter_query(), "", "the box empties");
    until_shown(&mut h, &["city", "note"]);

    h.window.global::<KeyPanel>().invoke_filter_badge_removed("london".into(), SearchScope::Everywhere);
    assert!(badges(&h).is_empty());
    until_shown(&mut h, &ALL);
}

#[test]
fn a_field_or_value_prefix_searches_there_alone() {
    let (mut h, _app, _rt, _turn) = key_shown();
    // "lang" is in two values but in no field's name
    enter(&h, "field: lang");
    assert_eq!(badges(&h), [("lang".to_string(), SearchScope::Fields)]);
    until_shown(&mut h, &[]);
    h.window.global::<KeyPanel>().invoke_filter_badge_removed("lang".into(), SearchScope::Fields);
    enter(&h, "V:lang");
    assert_eq!(badges(&h), [("lang".to_string(), SearchScope::Value)], "whatever the prefix's case");
    until_shown(&mut h, &["profile", "settings"]);
}

#[test]
fn every_badge_must_match_and_case_matters_only_when_asked() {
    let (mut h, _app, _rt, _turn) = key_shown();
    enter(&h, "LONDON");
    until_shown(&mut h, &["city", "note"]);
    enter(&h, "moved to");
    assert_eq!(badges(&h).len(), 2, "spaces stay inside one badge");
    until_shown(&mut h, &["note"]);

    let panel = h.window.global::<KeyPanel>();
    panel.set_filter_case_sensitive(true);
    panel.invoke_filter_case_sensitivity_changed();
    until_shown(&mut h, &[]);
}

#[test]
fn a_badge_is_never_added_twice_and_five_at_most() {
    let (h, _app, _rt, _turn) = key_shown();
    for word in ["a", "b", "c", "d", "e"] {
        enter(&h, word);
    }
    enter(&h, "a");
    assert_eq!(badges(&h).len(), 5, "the same badge again adds nothing");
    assert_eq!(h.window.global::<KeyPanel>().get_filter_query(), "");

    enter(&h, "f");
    let panel = h.window.global::<KeyPanel>();
    assert_eq!(badges(&h).len(), 5);
    assert_eq!(panel.get_filter_limit_message(), "Remove a badge first (5 max)");
    assert_eq!(panel.get_filter_query(), "f", "the words stay in the box");

    panel.invoke_filter_badge_removed("c".into(), SearchScope::Everywhere);
    assert_eq!(panel.get_filter_limit_message(), "", "removing one clears the warning");
}

#[test]
fn clicking_a_badge_puts_it_back_in_the_box_and_what_was_typed_becomes_a_badge() {
    let (h, _app, _rt, _turn) = key_shown();
    enter(&h, "v: london");
    let panel = h.window.global::<KeyPanel>();
    panel.set_filter_query("f:name".into());
    let end = panel.invoke_filter_badge_edited("london".into(), SearchScope::Value);
    assert_eq!(panel.get_filter_query(), "v:london", "with its scope's prefix");
    assert_eq!(end, "v:london".len() as i32, "the caret goes to its end");
    assert_eq!(badges(&h), [("name".to_string(), SearchScope::Fields)]);
}

#[test]
fn a_json_path_entered_filters_the_values_and_says_how_much_matched() {
    let (mut h, _app, _rt, _turn) = key_shown();
    enter(&h, "$.langs");
    until_shown(&mut h, &["profile", "settings"]);
    let panel = h.window.global::<KeyPanel>();
    assert_eq!(panel.get_json_filter(), "$.langs");
    assert_eq!(panel.get_filter_query(), "", "the box empties once the path is taken");
    assert!(badges(&h).is_empty(), "a path is no search badge");
    assert!(wait_for(&mut h, |h| h.window.global::<KeyPanel>().get_json_filter_status() == "2 matches in 2 fields"));

    enter(&h, "$.nothing.here");
    until_shown(&mut h, &[]);
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter_status(), "No match");
}

#[test]
fn a_path_that_doesnt_parse_stays_in_the_box_with_the_reason() {
    let (mut h, _app, _rt, _turn) = key_shown();
    enter(&h, "$.langs[");
    let panel = h.window.global::<KeyPanel>();
    assert!(!panel.get_json_filter_error().is_empty(), "the reason is shown");
    assert_eq!(panel.get_filter_query(), "$.langs[", "left to be fixed");
    until_shown(&mut h, &ALL);
}

/// The list's rows for `text`: (pill, words, hint).
fn list(h: &Harness, text: &str) -> Vec<(String, String, String)> {
    let panel = h.window.global::<KeyPanel>();
    panel.invoke_suggest(text.into(), text.len() as i32);
    panel.get_suggestions().iter().map(|row| (row.tag.to_string(), row.matched.to_string(), row.hint.to_string())).collect()
}

fn row(tag: &str, words: &str, hint: &str) -> (String, String, String) {
    (tag.to_string(), words.to_string(), hint.to_string())
}

#[test]
fn the_list_offers_where_to_search_the_words_and_enter_takes_the_first() {
    let (h, _app, _rt, _turn) = key_shown();
    assert_eq!(
        list(&h, "ada"),
        [row("All", "ada", "Enter"), row("Fields", "ada", "f: / field:"), row("Value", "ada", "v: / value:"), row("Filter", "$.ada", "$.")]
    );
    assert_eq!(list(&h, "v: ada"), [row("Value", "ada", "Enter")], "a prefix leaves only its scope");
    assert_eq!(list(&h, "ada lovelace").len(), 3, "several words make no path");
    // with nothing typed yet, each row says what to type first
    assert_eq!(list(&h, ""), [row("Fields", "", "f: / field:"), row("Value", "", "v: / value:"), row("Filter", "$.", "$.")]);
}

#[test]
fn taking_an_item_of_the_list_starts_its_prefix_or_adds_its_badge() {
    let (h, _app, _rt, _turn) = key_shown();
    let panel = h.window.global::<KeyPanel>();
    let edit = panel.invoke_complete_suggestion("".into(), 0, 1);
    assert_eq!((edit.text.as_str(), edit.cursor), ("v:", 2), "with no word yet, its prefix goes in the box");
    let edit = panel.invoke_complete_suggestion("".into(), 0, 2);
    assert_eq!((edit.text.as_str(), edit.cursor), ("$.", 2));

    panel.set_filter_query("lond".into());
    let edit = panel.invoke_complete_suggestion("lond".into(), 4, 2);
    assert_eq!(edit.cursor, -1, "the box was acted on");
    assert_eq!(badges(&h), [("lond".to_string(), SearchScope::Value)]);
}
