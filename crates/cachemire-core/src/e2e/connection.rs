//! Reaching the server: connecting, logging in, a pool that went stale, a form's test.

use super::*;
use crate::connection::{ConnectionItem, connect_connection, test_connection};
use crate::error::ErrorKind;
use crate::session::Session;
use crate::test_support::e2e_state;
use std::time::Duration;

fn item(url: &str) -> ConnectionItem {
    ConnectionItem {
        id: "form".into(),
        name: "Form".into(),
        connection_url: url.into(),
        username: String::new(),
        password: String::new(),
        ssh: None,
        profile_id: String::new(),
    }
}

/// A local address nothing listens on.
fn closed_port_url() -> String {
    let port = std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    format!("redis://127.0.0.1:{port}")
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn connecting_opens_the_pool_once_and_reuses_it() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "k", "v"]);
    let state = std::sync::Arc::new(redis.state());

    block_on(async {
        assert!(!state.redis_connections.is_connected("e2e"));
        let connected = connect_connection(&state, "e2e").await.unwrap();
        assert_eq!(connected.address(), redis.url.trim_start_matches("redis://").rsplit_once('/').unwrap().0);
        assert!(state.redis_connections.is_connected("e2e"));
        assert!(state.redis_connections.responds("e2e", Duration::from_secs(3)).await);

        let mut session = Session::open(&state, None).await.unwrap();
        assert_eq!(session.get(&key("k")).await.unwrap().as_deref(), Some("v"), "on the database the URL names");
        assert_eq!(session.dbsize().await.unwrap(), 1);
        assert!(session.info("server").await.unwrap().contains("redis_version"));

        connect_connection(&state, "e2e").await.unwrap();
        state.redis_connections.invalidate("e2e");
        assert!(!state.redis_connections.is_connected("e2e"));
        assert!(!state.redis_connections.responds("e2e", Duration::from_secs(1)).await, "no pool, no answer");
    });
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_wrong_password_and_a_server_that_isnt_there_fail_differently() {
    let mut redis = TestRedis::claim();
    let good = acl_user(&mut redis, "login", &["~*", "+@all"]);
    let wrong = good.replacen(":e2e-secret@", ":not-it@", 1);

    let state = std::sync::Arc::new(e2e_state(&wrong));
    let refused = block_on(connect_connection(&state, "e2e")).unwrap_err();
    assert_eq!(refused.kind(), ErrorKind::Auth, "{refused}");
    assert!(!state.redis_connections.is_connected("e2e"), "a failed open keeps no pool");

    let state = std::sync::Arc::new(e2e_state(&closed_port_url()));
    let unreachable = block_on(connect_connection(&state, "e2e")).unwrap_err();
    assert_eq!(unreachable.kind(), ErrorKind::Network, "{unreachable}");

    let state = std::sync::Arc::new(e2e_state(&good));
    block_on(connect_connection(&state, "e2e")).expect("the right password logs in");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn the_connection_on_its_own_fields_logs_in_when_the_url_names_no_user() {
    let mut redis = TestRedis::claim();
    let with_user = acl_user(&mut redis, "fields", &["~*", "+@all"]);
    let username = with_user.trim_start_matches("redis://").split(':').next().unwrap().to_string();
    let state = std::sync::Arc::new(redis.state());

    let mut form = item(&redis.url);
    form.username = username;
    form.password = "e2e-secret".into();
    block_on(test_connection(&state, form.clone())).expect("logs in with the form's username and password");
    form.password = "wrong".into();
    assert_eq!(block_on(test_connection(&state, form)).unwrap_err().kind(), ErrorKind::Auth);
    assert_eq!(block_on(test_connection(&state, item(&closed_port_url()))).unwrap_err().kind(), ErrorKind::Network);
    assert!(!state.redis_connections.is_connected("form"), "a test keeps nothing open");
}

#[test]
#[ignore = "needs a Redis server: cargo xtask e2e"]
fn a_pool_whose_connections_the_server_closed_is_opened_again_on_connecting() {
    let mut redis = TestRedis::claim();
    redis.run(&["SET", "k", "v"]);
    let url = acl_user(&mut redis, "dropped", &["~*", "+@all"]);
    let username = url.trim_start_matches("redis://").split(':').next().unwrap().to_string();
    let state = std::sync::Arc::new(e2e_state(&url));

    block_on(async {
        connect_connection(&state, "e2e").await.unwrap();
        let killed: i64 = redis.query(&["CLIENT", "KILL", "USER", &username]);
        assert!(killed >= 3, "the pool's connections were closed ({killed})");

        connect_connection(&state, "e2e").await.expect("connecting again finds the pool stale and reopens it");
        let mut session = Session::open(&state, None).await.unwrap();
        assert_eq!(session.get(&key("k")).await.unwrap().as_deref(), Some("v"));
    });
}
