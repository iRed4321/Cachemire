use super::*;

fn connection(url: &str) -> ConnectionItem {
    ConnectionItem {
        id: "c".into(),
        name: "c".into(),
        connection_url: url.into(),
        username: " app ".into(),
        password: "pw".into(),
        ssh: None,
        profile_id: String::new(),
    }
}

#[test]
fn the_url_fills_in_the_endpoint_and_the_fields_what_it_leaves_out() {
    let endpoint = connection("rediss://cache.example/3").endpoint().unwrap();
    assert_eq!(
        endpoint,
        Endpoint { host: "cache.example".into(), port: 6379, db: 3, tls: true, username: Some("app".into()), password: Some("pw".into()) }
    );
    let own = connection("redis://url-user:url-pw@h:0").endpoint().unwrap();
    assert_eq!(
        (own.port, own.username.as_deref(), own.password.as_deref()),
        (6379, Some("url-user"), Some("url-pw")),
        "port 0 means the default"
    );
    assert!(connection("redis://:6379").endpoint().is_err(), "a URL with no host can't be used");
}

#[test]
fn an_ipv6_server_is_reached_by_its_bare_address_and_shown_bracketed() {
    let ipv6 = connection("redis://[::1]:6380");
    assert_eq!(ipv6.endpoint().unwrap().host, "::1");
    assert_eq!(ipv6.address(), "[::1]:6380");
    assert_eq!(connection("cache.example").address(), "cache.example:6379");
    assert_eq!(connection("not a url").address(), "not a url");
}
