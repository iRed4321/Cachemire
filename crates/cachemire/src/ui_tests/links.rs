//! Shareable links: where one leads on the connection shown, on another saved one, on a server
//! not saved yet and on an AWS endpoint, how far it goes when the key lacks its field, and
//! the link the share button copies.

use super::*;
use crate::{ConnectionEditor, ConnectionState, ConnectionView, KeyScroll, KeyTree, KeyView};
use cachemire_core::connection::{ConnectionItem, ConnectionPayloadInput};
use cachemire_core::state::AppState;
use serde_json::json;
use std::sync::Arc;

type Ready = (Harness, Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn, Arc<AppState>);

/// The app over a state holding `connections` (id, URL), with no tab open.
fn links_ready(connections: &[(&str, &str)]) -> Ready {
    let state = test_state();
    {
        let mut store = state.connections.lock();
        for (id, url) in connections {
            let payload = ConnectionPayloadInput {
                name: Some(id.to_string()),
                connection_url: Some(url.to_string()),
                password: Some("secret".into()),
                ..Default::default()
            };
            store.create_with_id(id, payload, cachemire_core::secret_vault::SecretsPolicy::PlainText).unwrap();
        }
    }
    let h = Harness::new();
    h.window.global::<TabStrip>().set_tabs(ModelRc::new(VecModel::from(Vec::<crate::TabData>::new())));
    let (app, runtime, turn) = install_app(&h, &state);
    crate::tabs::init(&app, &h.window);
    crate::connections::init(&app, &h.window);
    crate::links::init(&app, &h.window);
    (h, app, runtime, turn, state)
}

/// Makes `id` the connection shown, as a connect does.
fn show_connection(h: &Harness, id: &str) {
    h.window.global::<ConnectionState>().set_view(ConnectionView { connected: true, connection_id: id.into(), ..Default::default() });
    crate::app::with(|app, window| crate::tabs::follow_connection(app, window));
}

fn open(link: &str) {
    crate::app::with(|app, window| crate::links::open(app, window, link));
}

/// The tab bar's tabs, by id, and which one is active.
fn strip(h: &Harness) -> (Vec<String>, Option<String>) {
    let tabs = h.window.global::<TabStrip>().get_tabs();
    (tabs.iter().map(|t| t.key.to_string()).collect(), tabs.iter().find(|t| t.active).map(|t| t.key.to_string()))
}

/// Runs the background work and the window until the key panel has finished loading.
fn loaded(h: &mut Harness, rt: &tokio::runtime::Runtime) -> bool {
    for _ in 0..500 {
        rt.block_on(async { tokio::time::sleep(std::time::Duration::from_millis(2)).await });
        h.frame();
        if h.window.global::<KeyTree>().get_loading_key().is_empty() {
            return true;
        }
    }
    false
}

/// The rows of the key panel's list: their fields, and which start open.
fn rows(h: &Harness) -> Vec<(String, bool)> {
    h.window.global::<KeyPanel>().get_all_values_rows().iter().map(|r| (r.field.to_string(), r.initial_expanded)).collect()
}

fn picked(app: &crate::app::App) -> String {
    app.picked.borrow().clone()
}

#[test]
fn a_link_on_the_connection_shown_opens_its_field_and_without_it_just_the_key() {
    let (mut h, app, rt, _turn, _) = links_ready(&[("a", "redis://cache.test:6379")]);
    show_connection(&h, "a");
    app.key_detail.seed_hash("a", "user:1", &[("bio", json!({"age": 1})), ("name", json!({"age": 2}))]);
    crate::tabs::select_key(&app, "user:1".into());
    crate::tabs::select_key(&app, "other".into());

    open("cachemire://open?v=1&redis=cache.test:6379&key=user:1&field=name");
    assert_eq!(strip(&h), (vec!["user:1".into(), "other".into()], Some("user:1".into())), "its tab comes forward");
    assert!(loaded(&mut h, &rt));
    assert_eq!(rows(&h), [("bio".into(), false), ("name".into(), true)], "the field is open");
    let scroll = h.window.global::<KeyScroll>();
    assert_eq!((scroll.get_reveal_field().as_str(), scroll.get_reveal_index()), ("name", 1), "and scrolled to");
    assert_eq!(h.window.global::<KeyPanel>().get_view(), KeyView::Fields);

    // the tab shown: a field it no longer has leaves the key as it is, and a filter applies
    open("cachemire://open?v=1&redis=cache.test:6379&key=user:1&field=gone&path=$.age");
    assert!(loaded(&mut h, &rt));
    assert_eq!(strip(&h).1.as_deref(), Some("user:1"));
    assert_eq!(rows(&h).len(), 2, "the key is shown");
    let scroll = h.window.global::<KeyScroll>();
    assert!(!scroll.get_reveal_pending() && scroll.get_reveal_index() == -1, "nothing to scroll to");
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter(), "$.age");
    assert_eq!(picked(&app), "", "no connection was made");
}

#[test]
fn a_link_to_another_saved_connection_waits_in_its_tabs_while_it_connects() {
    let (h, app, _rt, _turn, _) = links_ready(&[("a", "redis://one.test:6379"), ("b", "rediss://two.test:6380/2")]);
    show_connection(&h, "a");
    crate::tabs::open_query(&app, "FROM KEY 'x'".into());
    let on_a = strip(&h);

    open("cachemire://open?v=1&redis=Two.Test:6380&tls=1&db=2&key=orders:7&field=line%201&path=$.qty");
    assert_eq!(picked(&app), "b", "the connection to the link's server is connected to");
    assert!(app.tabs.borrow().parked_connections().contains("b"));
    assert_eq!(strip(&h), on_a, "the tabs shown stay until it is connected");

    show_connection(&h, "b");
    assert_eq!(strip(&h), (vec!["orders:7".into()], Some("orders:7".into())));
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter(), "$.qty");
    assert_eq!(h.window.global::<KeyPanel>().get_view(), KeyView::Fields);
    let scroll = h.window.global::<KeyScroll>();
    assert!(scroll.get_reveal_pending());
    assert_eq!(scroll.get_reveal_field(), "line 1");

    show_connection(&h, "a");
    assert_eq!(strip(&h), on_a, "the first connection's tabs come back");
}

#[test]
fn a_link_to_a_server_not_saved_offers_it_and_follows_once_it_is_saved() {
    let (mut h, app, _rt, _turn, state) = links_ready(&[("a", "redis://one.test:6379")]);
    let editor = h.window.global::<ConnectionEditor>();

    open("cachemire://open?v=1&redis=new.test:6390&via=jump.test:2222&db=3&name=Staging&key=k");
    assert!(editor.get_open() && !editor.get_is_edit(), "a new connection's form");
    assert_eq!(editor.get_name(), "Staging");
    assert_eq!(editor.get_connection_url(), "redis://new.test:6390/3");
    assert!(editor.get_ssh_enabled());
    assert_eq!((editor.get_ssh_host().as_str(), editor.get_ssh_port().as_str()), ("jump.test", "2222"));
    assert!(editor.get_username().is_empty() && editor.get_ssh_username().is_empty(), "the login is left to fill in");
    assert!(crate::toast::take_last().contains("new.test:6390 (db 3)"));
    editor.set_open(false);

    open("cachemire://open?v=1&redis=new.test:6390&db=3&key=k&field=f");
    assert_eq!(editor.get_name(), "new.test", "no name: the host");
    assert!(!editor.get_ssh_enabled());
    editor.invoke_saved(editor.invoke_form());
    assert!(wait_for(&mut h, |h| !h.window.global::<ConnectionEditor>().get_open()), "the form closes once saved");

    let id = state.connections.lock().list_state().connections.into_iter().find(|c| c.name == "new.test").unwrap().id;
    assert_eq!(picked(&app), id, "the link goes on to it");
    assert!(app.tabs.borrow().parked_connections().contains(&id));
}

#[test]
fn a_damaged_or_newer_link_says_why_and_opens_nothing() {
    let (h, app, _rt, _turn, _) = links_ready(&[("a", "redis://one.test:6379")]);
    for (link, why) in [
        ("cachemire://open?v=7&redis=one.test&key=k", "newer version"),
        ("cachemire://open?v=1&redis=one.test:99999&key=k", "damaged"),
        ("cachemire://open?v=1&redis=one.test", "damaged"),
    ] {
        open(link);
        let said = crate::toast::take_last();
        assert!(said.contains("can't be opened") && said.contains(why), "{link}: {said}");
    }
    assert!(strip(&h).0.is_empty() && picked(&app).is_empty() && app.tabs.borrow().parked_connections().is_empty());
    assert!(!h.window.global::<ConnectionEditor>().get_open());
}

#[test]
fn the_share_button_links_the_key_shown_with_its_filter_and_no_secret() {
    let (h, app, _rt, _turn, _) = links_ready(&[("prod", "redis://admin:pw@cache.test:6379/1")]);
    show_connection(&h, "prod");
    crate::tabs::select_key(&app, "weird key+1".into());
    h.window.global::<KeyPanel>().set_json_filter(" $.a b ".into());

    let link = crate::links::active_link(&app, &h.window, None).unwrap();
    assert_eq!(link, "cachemire://open?v=1&redis=cache.test:6379&db=1&name=prod&key=weird%20key%2B1&path=$.a%20b");
    assert!(!link.contains("admin") && !link.contains("pw") && !link.contains("secret"));

    // it leads back to the same key and filter
    crate::tabs::open_query(&app, String::new());
    assert_eq!(crate::links::active_link(&app, &h.window, None), None, "a query tab has no key to link to");
    open(&link);
    assert_eq!(strip(&h).1.as_deref(), Some("weird key+1"));
    assert_eq!(h.window.global::<KeyPanel>().get_json_filter(), "$.a b");
}

#[test]
fn an_aws_link_follows_once_its_account_is_listed() {
    let (_h, app, _rt, _turn, state) = links_ready(&[]);
    let endpoint = |profile: &str, name: &str| ConnectionItem {
        id: cachemire_core::aws::connection_id(profile, name),
        name: name.into(),
        connection_url: format!("rediss://{name}.cache.test:6379"),
        username: String::new(),
        password: String::new(),
        ssh: None,
        profile_id: String::new(),
        read_only: false,
    };
    state.connections.lock().set_cloud_connections("111", "eu-west-1", vec![endpoint("dev", "sessions")]);

    open("cachemire://open?v=1&aws=111/eu-west-1/sessions&key=cart:9");
    assert_eq!(picked(&app), "aws:dev:sessions", "listed now: connected to");
    assert!(app.tabs.borrow().parked_connections().contains("aws:dev:sessions"));

    open("cachemire://open?v=1&aws=111/eu-west-1/gone&key=k");
    assert!(crate::toast::take_last().contains("gone (111, eu-west-1) is no longer"));

    // an account no profile is known to list waits for one that lists it
    open("cachemire://open?v=1&aws=222/us-east-1/carts&key=cart:1");
    assert!(crate::toast::take_last().contains("Sign in to AWS"));
    assert_eq!(picked(&app), "aws:dev:sessions");
    state.connections.lock().set_cloud_connections("222", "us-east-1", vec![endpoint("ops", "carts")]);
    crate::app::with(|app, window| crate::links::retry_pending(app, window));
    assert_eq!(picked(&app), "aws:ops:carts");
    assert!(app.tabs.borrow().parked_connections().contains("aws:ops:carts"));
}
