use super::*;

fn key(name: &str, entries: &[(&str, &str)]) -> KeyEntries {
    KeyEntries {
        key: name.into(),
        key_type: RedisValueType::Hash,
        entries: entries.iter().map(|(f, v)| (f.to_string(), v.to_string())).collect(),
    }
}

#[test]
fn a_search_tab_id_is_never_a_key() {
    let mut tabs = SearchTabs::default();
    let id = tabs.create(String::new());
    assert!(is_search_id(&id));
    assert!(!is_search_id("search:1"));
    assert_ne!(id, tabs.create(String::new()));
    tabs.forget(&id);
}

#[test]
fn hits_are_the_entries_that_hold_the_word_or_the_key_alone() {
    let mut keys = 0;
    let rows = hits_in(
        vec![
            key("user:1", &[("name", "Ada Lovelace"), ("city", "London")]),
            key("cart:7", &[("total", "12")]),
            key("lovely:key", &[("a", "b")]),
            key("misc", &[("x", "y")]),
        ],
        "love",
        100,
        &mut keys,
        &mut Longest::default(),
    );
    // Ada's name, and the key "lovely:key" (matched on its own name, without an entry)
    assert_eq!(rows.len(), 2);
    assert_eq!(keys, 2);
}

#[test]
fn a_page_never_exceeds_the_room_left() {
    let mut keys = 0;
    let entries: Vec<(String, String)> = (0..50).map(|i| (format!("f{i}"), "hit".to_string())).collect();
    let many = KeyEntries { key: "k".into(), key_type: RedisValueType::Hash, entries };
    assert_eq!(hits_in(vec![many], "hit", 10, &mut keys, &mut Longest::default()).len(), 10);
}

#[test]
fn the_key_and_field_columns_fit_their_longest_cell_within_bounds() {
    let mut longest = Longest::default();
    let rows = hits_in(vec![key("session:long-key-name", &[("f", "hit"), ("a-longer-field", "hit")])], "hit", 10, &mut 0, &mut longest);
    assert_eq!(rows.len(), 2);
    assert_eq!((longest.key, longest.field), (21, 14));
    let (key_w, field_w) = longest.widths();
    assert_eq!((key_w, field_w), (21.0 * CHAR_PX + CELL_PAD_PX, 14.0 * CHAR_PX + CELL_PAD_PX));
    // a huge key is capped, a tiny one gets a minimum
    assert_eq!(Longest { key: 500, field: 1 }.widths(), (48.0 * CHAR_PX + CELL_PAD_PX, 5.0 * CHAR_PX + CELL_PAD_PX));
}

#[test]
fn the_status_says_where_the_search_is() {
    assert!(status_text("x", 500, 0, 3, 2, false, false).contains("500"));
    assert!(status_text("zz", 900, 900, 0, 0, true, false).contains("zz"));
    assert!(status_text("x", 900, 900, 1000, 40, true, true).contains("1000"));
    // with the size of the instance known, it says how many keys there are
    let running = status_text("x", 500, 2000, 3, 2, false, false);
    assert!(running.contains("500") && running.contains("2000"), "{running}");
    // (keys that arrived since the count was taken don't make it read past the end)
    assert!(!status_text("x", 2100, 2000, 3, 2, false, false).contains("2100"));
}

#[test]
fn the_bar_only_advances_and_stops_short_of_the_end_until_the_walk_is_over() {
    assert_eq!(progress_of(0, 1000), 0.0);
    assert_eq!(progress_of(500, 1000), 0.5);
    assert_eq!(progress_of(1000, 1000), 0.99);
    assert_eq!(progress_of(1500, 1000), 0.99, "keys added meanwhile can't push it past the end");
    assert_eq!(progress_of(10, 0), -1.0, "no size: unknown");

    assert_eq!(next_progress(0.4, 0.3), 0.4, "never back");
    assert_eq!(next_progress(0.4, 0.6), 0.6);
    assert_eq!(next_progress(0.99, 1.0), 1.0);
    assert_eq!(next_progress(0.5, 3.0), 1.0);
    assert_eq!(next_progress(0.0, -1.0), -1.0);
}
