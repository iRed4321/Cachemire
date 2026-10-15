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
