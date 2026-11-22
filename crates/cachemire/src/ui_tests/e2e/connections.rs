//! Connections on a real server: the form's Test button, a server that can't be reached,
//! and leaving the connection in use by disconnecting or deleting it.

use super::*;
use crate::ConnectionEditor;
use cachemire_core::connection::ConnectionPayloadInput;
use cachemire_core::secret_vault::SecretsPolicy;

/// Fills the connection form for a new connection to `url`, and presses Test.
fn test_form(r: &mut Running, url: &str) -> (bool, String) {
    let editor = r.h.window.global::<ConnectionEditor>();
    editor.set_ssh_enabled(false);
    editor.set_profile_enabled(false);
    editor.set_id("".into());
    editor.set_name("Tried".into());
    editor.set_connection_url(url.into());
    editor.set_open(true);
    editor.invoke_test_requested(editor.invoke_form());
    r.until("the test", |h| !h.window.global::<ConnectionEditor>().get_testing());
    let editor = r.h.window.global::<ConnectionEditor>();
    (editor.get_test_ok(), editor.get_test_status().to_string())
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_form_tests_a_connection_before_it_is_saved() {
    let mut r = Running::start(|_| {});
    let url = r.redis.url.clone();
    assert_eq!(test_form(&mut r, &url), (true, "Connection successful.".to_string()));
    let (ok, status) = test_form(&mut r, "redis://127.0.0.1:1/0");
    assert!(!ok && !status.is_empty(), "the reason it failed: {status}");
    let (ok, status) = test_form(&mut r, "http://example.test");
    assert!(!ok && status.contains("valkey://"), "a URL it can't use is refused at once: {status}");
    let saved: Vec<String> = r.app.state.connections.lock().list_state().connections.into_iter().map(|c| c.name).collect();
    assert!(!saved.contains(&"Tried".to_string()), "testing saves nothing");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_server_that_cant_be_reached_says_why_and_no_connection_stays_in_use() {
    let mut r = Running::start(|redis| redis.run(&["SET", "k", "v"]));
    let payload =
        ConnectionPayloadInput { name: Some("Down".into()), connection_url: Some("redis://127.0.0.1:1".into()), ..Default::default() };
    r.app.state.connections.lock().create_with_id("down", payload, SecretsPolicy::PlainText).unwrap();
    r.h.window.global::<ConnectionState>().invoke_selected("down".into());
    r.until("the failure", |h| !h.window.global::<ConnectionState>().get_view().status_detail.is_empty());
    let view = r.h.window.global::<ConnectionState>().get_view();
    assert!(!view.connected && !view.connecting);
    assert_eq!(view.connection_id, "");
    assert_eq!(r.app.state.connections.lock().active_connection_id(), "", "none is active in the store either");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn disconnecting_drops_the_tabs_and_returns_to_no_connection() {
    let mut r = Running::start(|redis| redis.run(&["SET", "k", "v"]));
    r.open_key("k");
    r.h.window.global::<ConnectionState>().invoke_disconnect_requested();
    r.until("no connection", |h| !h.window.global::<ConnectionState>().get_view().connected);
    let view = r.h.window.global::<ConnectionState>().get_view();
    assert_eq!(view.connection_id, "");
    assert_eq!(r.h.window.global::<TabStrip>().get_tabs().row_count(), 0, "its tabs are dropped");

    // picked again, it opens with no tab left from before
    r.h.window.global::<ConnectionState>().invoke_selected("e2e".into());
    r.until("the connection", |h| h.window.global::<ConnectionState>().get_view().connected);
    assert_eq!(r.h.window.global::<TabStrip>().get_tabs().row_count(), 0);
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn deleting_the_connection_in_use_leaves_it() {
    let mut r = Running::start(|redis| redis.run(&["SET", "k", "v"]));
    r.open_key("k");
    let editor = r.h.window.global::<ConnectionEditor>();
    editor.set_open(true);
    editor.invoke_deleted("e2e".into());
    assert!(!editor.get_open(), "the form closes");
    assert!(r.app.state.connections.lock().get("e2e").is_none());
    r.until("no connection", |h| !h.window.global::<ConnectionState>().get_view().connected);
    assert_eq!(r.h.window.global::<TabStrip>().get_tabs().row_count(), 0);
    let listed: Vec<String> = r.h.window.global::<ConnectionState>().get_connections().iter().map(|c| c.name.to_string()).collect();
    assert!(!listed.contains(&"End-to-end".to_string()), "{listed:?}");
}
