use super::*;

#[test]
fn a_query_tab_id_is_never_a_key_or_a_search_tab() {
    let mut tabs = QueryTabs::default();
    let id = tabs.create(String::new());
    assert!(is_query_id(&id));
    assert!(!is_query_id("search:1"));
    assert!(!is_query_id("\u{1}search:1"));
    assert_ne!(id, tabs.create(String::new()));
    tabs.forget(&id);
}
