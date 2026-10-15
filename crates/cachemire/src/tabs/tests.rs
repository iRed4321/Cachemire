use super::*;

fn open(tabs: &mut Tabs, keys: &[&str]) {
    tabs.open = keys.iter().map(|k| k.to_string()).collect();
    tabs.active = keys.last().map(|k| k.to_string());
}

#[test]
fn each_connection_gets_its_tabs_back() {
    let mut tabs = Tabs::default();
    tabs.switch_connection("a", true);
    open(&mut tabs, &["users", "orders"]);
    tabs.view_state.insert("users".into(), TabViewState { json_filter: "x".into(), ..Default::default() });

    tabs.switch_connection("b", true);
    assert!(tabs.open.is_empty() && tabs.active.is_none(), "b starts with no tabs");
    open(&mut tabs, &["cache"]);

    tabs.switch_connection("a", true);
    assert_eq!((tabs.open.clone(), tabs.active.clone()), (vec!["users".to_string(), "orders".to_string()], Some("orders".to_string())));
    assert_eq!(tabs.view_state["users"].json_filter, "x");

    tabs.switch_connection("b", true);
    assert_eq!(tabs.open, vec!["cache".to_string()]);
    // switching to the connection already shown changes nothing
    tabs.switch_connection("b", true);
    assert_eq!(tabs.open, vec!["cache".to_string()]);
}

#[test]
fn no_connection_hides_the_tabs_and_a_deleted_one_forgets_them() {
    let mut tabs = Tabs::default();
    tabs.switch_connection("a", true);
    open(&mut tabs, &["k"]);
    tabs.switch_connection("", true);
    assert!(tabs.open.is_empty());
    tabs.switch_connection("a", true);
    assert_eq!(tabs.open, vec!["k".to_string()]);

    tabs.switch_connection("", false);
    tabs.switch_connection("a", true);
    assert!(tabs.open.is_empty(), "the tabs of a connection that no longer exists aren't kept");
}

#[test]
fn the_tree_highlight_is_remembered_per_connection() {
    let mut tabs = Tabs::default();
    tabs.switch_connection("a", true);
    open(&mut tabs, &["users"]);
    tabs.highlighted = true;

    tabs.switch_connection("b", true);
    assert!(!tabs.highlighted, "b starts without one");
    open(&mut tabs, &["cache"]);

    tabs.switch_connection("a", true);
    assert!(tabs.highlighted, "a's key is highlighted again");
    tabs.highlighted = false;
    tabs.switch_connection("b", true);
    assert!(!tabs.highlighted, "b's key never was");
    tabs.switch_connection("a", true);
    assert!(!tabs.highlighted, "and a's no longer is");

    // a deleted connection leaves nothing behind
    tabs.highlighted = true;
    tabs.switch_connection("", false);
    tabs.switch_connection("a", true);
    assert!(!tabs.highlighted);
}

fn active(tabs: &Tabs) -> Option<&str> {
    tabs.active.as_deref()
}

#[test]
fn closing_the_active_tab_activates_its_left_neighbour() {
    let mut tabs = Tabs::default();
    open(&mut tabs, &["a", "b", "c"]);
    tabs.active = Some("b".into());
    tabs.highlighted = true;
    assert_eq!(tabs.close("b"), Some((true, Some("a".into()))));
    assert_eq!(tabs.open, ["a", "c"]);
    assert!(!tabs.highlighted, "the tree highlight goes with the closed tab");
}

#[test]
fn closing_the_first_tab_while_active_activates_the_next_one() {
    let mut tabs = Tabs::default();
    open(&mut tabs, &["a", "b"]);
    tabs.active = Some("a".into());
    assert_eq!(tabs.close("a"), Some((true, Some("b".into()))));
    assert_eq!(tabs.close("b"), Some((true, None)), "the last tab closed: none is active");
    assert!(tabs.open.is_empty());
}

#[test]
fn closing_another_tab_keeps_the_active_one_and_its_highlight() {
    let mut tabs = Tabs::default();
    open(&mut tabs, &["a", "b", "c"]);
    tabs.highlighted = true;
    tabs.view_state.insert("a".into(), TabViewState::default());
    assert_eq!(tabs.close("a"), Some((false, None)));
    assert_eq!((active(&tabs), tabs.highlighted), (Some("c"), true));
    assert!(!tabs.view_state.contains_key("a"), "a closed tab forgets its view");
    assert_eq!(tabs.close("nope"), None);
}

#[test]
fn closing_the_other_tabs_keeps_one_and_makes_it_active() {
    let mut tabs = Tabs::default();
    open(&mut tabs, &["a", "b", "c"]);
    tabs.highlighted = true;
    for key in ["a", "b", "c"] {
        tabs.view_state.insert(key.into(), TabViewState::default());
    }
    let (closed, was_active) = tabs.close_others("b").unwrap();
    assert_eq!((closed, was_active), (vec!["a".to_string(), "c".to_string()], false));
    assert_eq!((tabs.open.as_slice(), active(&tabs)), (["b".to_string()].as_slice(), Some("b")));
    assert_eq!(tabs.view_state.keys().collect::<Vec<_>>(), ["b"]);
    assert!(!tabs.highlighted, "another tab became the active one");
}

#[test]
fn closing_the_others_of_the_active_tab_keeps_its_highlight() {
    let mut tabs = Tabs::default();
    open(&mut tabs, &["a", "b"]);
    tabs.highlighted = true;
    assert_eq!(tabs.close_others("b"), Some((vec!["a".to_string()], true)));
    assert!(tabs.highlighted);
    assert_eq!(tabs.close_others("gone"), None);
}
