//! The tab bar: switching, closing, reordering, and each connection's own tabs, on query
//! tabs (which, unlike a key's, read nothing from a server until run).

use super::*;
use crate::{ConnectionState, ConnectionView, KeyView};

/// The tab bar wired to an app over an empty state, on connection `a`.
fn tabs_ready() -> (Harness, Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn) {
    let h = Harness::new();
    h.window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(Vec::<crate::TabData>::new())));
    let (app, runtime, turn) = install_app(&h, &test_state());
    crate::tabs::init(&app, &h.window);
    connect_to(&h, "a");
    (h, app, runtime, turn)
}

/// Makes `id` the connection shown, as a connect does.
fn connect_to(h: &Harness, id: &str) {
    h.window.global::<ConnectionState>().set_view(ConnectionView { connected: true, connection_id: id.into(), ..Default::default() });
    h.window.global::<ConnectionState>().invoke_switched();
}

/// The tab bar's tabs, by id, and which one is active.
fn strip(h: &Harness) -> (Vec<String>, Option<String>) {
    let tabs = h.window.global::<TabStrip>().get_tabs();
    let ids = tabs.iter().map(|t| t.key.to_string()).collect();
    let active = tabs.iter().find(|t| t.active).map(|t| t.key.to_string());
    (ids, active)
}

fn query_text(h: &Harness) -> String {
    h.window.global::<QueryTab>().get_text().to_string()
}

#[test]
fn each_query_tab_comes_back_with_what_was_typed_and_how_it_was_shown() {
    let (h, app, _rt, _turn) = tabs_ready();
    crate::tabs::open_query(&app, "FROM KEY 'a'".into());
    let first = strip(&h).1.unwrap();
    assert_eq!(h.window.global::<KeyPanel>().get_view(), KeyView::Table, "a query's result starts as a table");
    h.window.global::<QueryTab>().set_text("FROM KEY 'a' SELECT id".into());
    h.window.global::<KeyPanel>().set_view(KeyView::Fields);
    h.window.global::<KeyPanel>().set_json_filter("$.id".into());

    crate::tabs::open_query(&app, "FROM KEY 'b'".into());
    assert_eq!(query_text(&h), "FROM KEY 'b'");
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter(), "", "a new tab starts from nothing of the previous one");

    h.window.global::<TabStrip>().invoke_selected(first.as_str().into());
    assert_eq!(strip(&h).1.as_deref(), Some(first.as_str()));
    assert_eq!(query_text(&h), "FROM KEY 'a' SELECT id");
    assert_eq!(h.window.global::<KeyPanel>().get_view(), KeyView::Fields);
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter(), "$.id");
}

#[test]
fn closing_the_active_tab_shows_its_neighbour_and_closing_all_empties_the_panel() {
    let (h, app, _rt, _turn) = tabs_ready();
    for text in ["one", "two", "three"] {
        crate::tabs::open_query(&app, text.into());
    }
    let (ids, _) = strip(&h);
    h.window.global::<TabStrip>().invoke_selected(ids[1].as_str().into());

    // Ctrl+W
    h.window.global::<TabStrip>().invoke_close_active();
    assert_eq!(strip(&h), (vec![ids[0].clone(), ids[2].clone()], Some(ids[0].clone())));
    assert_eq!(query_text(&h), "one", "the tab now active is shown");

    h.window.global::<TabStrip>().invoke_close_all();
    assert_eq!(strip(&h), (Vec::new(), None));
    assert_eq!(h.window.global::<TabStrip>().get_active_index(), -1);
    assert!(!h.window.global::<KeyPanel>().get_loaded() && !h.window.global::<TabStrip>().get_active_is_query());
}

#[test]
fn close_others_keeps_one_tab_and_shows_it() {
    let (h, app, _rt, _turn) = tabs_ready();
    for text in ["one", "two", "three"] {
        crate::tabs::open_query(&app, text.into());
    }
    let (ids, _) = strip(&h);
    h.window.global::<TabStrip>().invoke_close_others(ids[0].as_str().into());
    assert_eq!(strip(&h), (vec![ids[0].clone()], Some(ids[0].clone())));
    assert_eq!(query_text(&h), "one");
}

#[test]
fn a_dragged_tab_lands_where_it_is_dropped_and_the_active_one_stays_active() {
    let (h, app, _rt, _turn) = tabs_ready();
    for text in ["one", "two", "three"] {
        crate::tabs::open_query(&app, text.into());
    }
    let (ids, active) = strip(&h);
    h.window.global::<TabStrip>().invoke_reordered(ids[2].as_str().into(), 0);
    assert_eq!(strip(&h), (vec![ids[2].clone(), ids[0].clone(), ids[1].clone()], active.clone()));
    assert_eq!(h.window.global::<TabStrip>().get_active_index(), 0);

    // dropped past the end: it goes last
    h.window.global::<TabStrip>().invoke_reordered(ids[2].as_str().into(), 99);
    assert_eq!(strip(&h).0, ids);
}

#[test]
fn a_saved_query_opens_in_a_fresh_unrun_tab_each_time() {
    let (h, app, _rt, _turn) = tabs_ready();
    let id = app.state.settings.lock().create_saved_query("Orders", "FROM KEY 'orders'", "").unwrap();
    h.window.global::<SavedQueries>().invoke_clicked(id.as_str().into());
    h.window.global::<QueryTab>().set_text("FROM KEY 'orders' LIMIT 1".into());
    h.window.global::<SavedQueries>().invoke_clicked(id.as_str().into());

    assert_eq!(strip(&h).0.len(), 2, "the tab opened first isn't reused");
    assert_eq!(query_text(&h), "FROM KEY 'orders'", "the saved text, not the first tab's edit");
    assert!(!h.window.global::<QueryTab>().get_ran(), "it is reviewed before it is run");
}

#[test]
fn each_connection_gets_its_own_tabs_back_and_a_new_one_starts_with_none() {
    let (h, app, _rt, _turn) = tabs_ready();
    let payload = |name: &str| cachemire_core::connection::ConnectionPayloadInput {
        name: Some(name.into()),
        connection_url: Some("redis://localhost:6379".into()),
        ..Default::default()
    };
    app.state.connections.lock().create_with_id("a", payload("A")).unwrap();
    app.state.connections.lock().create_with_id("b", payload("B")).unwrap();
    crate::tabs::open_query(&app, "on a".into());
    h.window.global::<QueryTab>().set_text("on a, edited".into());
    let on_a = strip(&h);

    connect_to(&h, "b");
    assert_eq!(strip(&h), (Vec::new(), None));
    assert!(app.tabs.borrow().parked_connections().contains("a"));
    crate::tabs::open_query(&app, "on b".into());

    connect_to(&h, "a");
    assert_eq!(strip(&h), on_a);
    assert_eq!(query_text(&h), "on a, edited", "what was typed went with the tab");
}

#[test]
fn a_deleted_connection_leaves_no_tabs_behind() {
    let (h, app, _rt, _turn) = tabs_ready();
    crate::tabs::open_query(&app, "on a".into());
    // "a" was never saved: it is as gone as a deleted one
    connect_to(&h, "b");
    connect_to(&h, "a");
    assert_eq!(strip(&h), (Vec::new(), None));
}
