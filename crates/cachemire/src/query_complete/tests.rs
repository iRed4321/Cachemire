use super::*;
use crate::test_support::{block_on, temp_dir};

/// A state with an active connection, and caches already holding `levels` (by prefix)
/// and `fields` (by key), so nothing is asked of a server.
fn setup(levels: &[(&str, &[&str])], fields: &[(&str, &[&str])]) -> (Arc<AppState>, Caches) {
    let state = Arc::new(AppState::for_tests(&temp_dir("complete")));
    let connection = state.connections.lock_recover().active_connection_id().to_string();
    let caches = Caches::default();
    let fill = |map: &Listed, entries: &[(&str, &[&str])]| {
        for (at, names) in entries {
            map.lock_recover().insert((connection.clone(), at.to_string()), (Instant::now(), names.iter().map(|n| n.to_string()).collect()));
        }
    };
    fill(&caches.levels, levels);
    fill(&caches.fields, fields);
    (state, caches)
}

fn offered(state: &Arc<AppState>, caches: &Caches, text: &str, force: bool) -> Option<Completion> {
    block_on(complete(state, caches, text, text.len(), force))
}

fn shown(completion: &Option<Completion>) -> Vec<&str> {
    completion.iter().flat_map(|c| c.items.iter().map(|(name, _)| name.as_str())).collect()
}

#[test]
fn keywords_and_root_keys_complete_what_is_typed() {
    let (state, caches) = setup(&[("", &["fresh", "from:cache:", "frozen"])], &[]);
    let completion = offered(&state, &caches, "SELECT * FR", false);
    assert_eq!(shown(&completion), ["FROM", "from:cache:", "fresh", "frozen"], "keywords, then folders, then keys");
    assert!(offered(&state, &caches, "SELECT * F", false).is_none(), "one character waits for Ctrl+Space");
    assert!(offered(&state, &caches, "SELECT * F", true).is_some());
}

#[test]
fn a_quoted_key_lists_the_level_it_is_in() {
    let (state, caches) = setup(&[("orders:", &["orders:2024:", "orders:42", "orders:7"])], &[]);
    let text = "FROM KEY 'orders:4";
    let completion = offered(&state, &caches, text, false).unwrap();
    assert_eq!(shown(&Some(completion.clone())), ["orders:42"]);
    assert_eq!(completion.quote, Some('\''));
    let (next, caret) = completion.accept(text, 0).unwrap();
    assert_eq!(next, "FROM KEY 'orders:42'");
    assert_eq!(caret, next.len());
}

#[test]
fn choosing_a_folder_leaves_the_key_open_to_go_on() {
    let (state, caches) = setup(&[("", &["orders:", "order"])], &[]);
    let completion = offered(&state, &caches, "FROM KEY or", false).unwrap();
    let folder = completion.items.iter().position(|(name, _)| name == "orders:").unwrap();
    assert_eq!(completion.accept("FROM KEY or", folder).unwrap().0, "FROM KEY 'orders:");
    let key = completion.items.iter().position(|(name, _)| name == "order").unwrap();
    assert_eq!(completion.accept("FROM KEY or", key).unwrap().0, "FROM KEY order", "a name without ':' is written as is");
}

#[test]
fn a_key_already_closed_by_its_quote_is_not_closed_twice() {
    let (state, caches) = setup(&[("a:", &["a:bc"])], &[]);
    let text = "FROM KEY 'a:b' AS x";
    let caret = text.find("b'").unwrap() + 1;
    let completion = block_on(complete(&state, &caches, text, caret, false)).unwrap();
    assert_eq!(completion.accept(text, 0).unwrap().0, "FROM KEY 'a:bc' AS x");
}

#[test]
fn an_alias_completes_with_its_keys_fields() {
    let (state, caches) = setup(&[], &[("orders:42", &["total", "customerId", "status"])]);
    let text = "FROM KEY 'orders:42' AS o WHERE o.st";
    assert_eq!(shown(&offered(&state, &caches, text, false)), ["o.status", "o.customerId"], "a match at the start comes first");
    let text = "FROM KEY 'orders:42' o SELECT O.";
    assert_eq!(shown(&offered(&state, &caches, text, false)).len(), 3, "AS is optional and aliases ignore case");
    assert!(offered(&state, &caches, "FROM KEY 'orders:42' AS o SELECT z.", false).is_none(), "an unknown alias offers nothing");
}

#[test]
fn the_token_under_the_caret_spans_the_whole_quoted_key() {
    assert_eq!(token_bounds("KEY 'ab:cd' AS x", 7), (5, 10, Some('\'')));
    assert_eq!(token_bounds("WHERE o.status = 1", 9), (6, 14, None));
    assert_eq!(token_bounds("a 'b' 'c", 8), (7, 8, Some('\'')), "only an unclosed quote holds the caret");
    assert_eq!(open_quote("'a'\n'b", 6), Some(('\'', 4)), "quotes don't carry over a line break");
}

#[test]
fn aliases_are_read_whatever_the_text_around_them() {
    let aliases = alias_keys("FROM KEY 'İstanbul:1' AS a JOIN KEY 'b' AS b ON a.x = b.y");
    assert_eq!(aliases.get("a").map(String::as_str), Some("İstanbul:1"));
    assert_eq!(aliases.get("b").map(String::as_str), Some("b"));
    let aliases = alias_keys("FROM KEY 'ẞ' AS s JOIN KEY 'ǅ' AS d JOIN KEY 'k' AS k");
    assert_eq!(aliases.len(), 3);
    assert!(alias_keys("FROM KEY 'k' WHERE").is_empty(), "a keyword is not an alias");
}

#[test]
fn a_caret_inside_a_character_offers_nothing() {
    let (state, caches) = setup(&[], &[]);
    assert!(block_on(complete(&state, &caches, "é", 1, true)).is_none());
    assert!(block_on(complete(&state, &caches, "ab", 3, true)).is_none());
}
