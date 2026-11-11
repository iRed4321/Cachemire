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

#[test]
fn through_a_tunnel_the_secured_handshake_checks_the_endpoints_real_host() {
    let secured = connection("rediss://cache.example:6380");
    let client = client::build_client(&secured, Some(4242)).unwrap();
    // redis-rs talks plain TCP to the tunnel's local port...
    assert_eq!(*client.get_connection_info().addr(), redis::ConnectionAddr::Tcp("127.0.0.1".into(), 4242));
    // ...and the tunnel verifies the certificate against the real host name
    assert_eq!(client::tls_name_of(&secured).unwrap(), Some(rustls::pki_types::ServerName::try_from("cache.example").unwrap()));
    assert_eq!(client::tls_name_of(&connection("redis://cache.example")).unwrap(), None);
}
