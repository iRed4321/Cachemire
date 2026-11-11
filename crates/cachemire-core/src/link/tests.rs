use super::*;

fn saved(id: &str, name: &str, url: &str, ssh: Option<(&str, u16)>) -> ConnectionItem {
    ConnectionItem {
        id: id.to_string(),
        name: name.to_string(),
        connection_url: url.to_string(),
        username: "admin".to_string(),
        password: "hunter2".to_string(),
        ssh: ssh.map(|(host, port)| SshSettings {
            host: host.to_string(),
            port,
            username: "deploy".to_string(),
            password: "ssh-secret".to_string(),
            private_key: "~/.ssh/id_ed25519".to_string(),
            passphrase: String::new(),
        }),
        profile_id: String::new(),
        read_only: false,
    }
}

fn redis(host: &str, port: u16) -> Server {
    Server::Redis { address: Address { host: host.to_string(), port }, tls: false, via: None }
}

fn link(server: Server, key: &str) -> Link {
    Link { server, db: 0, name: None, key: KeyName::from(key), field: None, path: None }
}

#[test]
fn a_link_reads_back_as_it_was_written() {
    let links = [
        Link {
            server: Server::Redis {
                address: Address { host: "cache.prod".into(), port: 6380 },
                tls: true,
                via: Some(Address { host: "bastion".into(), port: 2222 }),
            },
            db: 3,
            name: Some("Prod & co = 100%".into()),
            key: KeyName::from("user:42/profile #1 +x"),
            field: Some("é?&=".into()),
            path: Some("$.items[0][\"a b\"]".into()),
        },
        link(Server::Aws { account: "123456789012".into(), region: "eu-west-1".into(), name: "sessions".into() }, "cart:9"),
        link(redis("::1", 6379), "k"),
        Link { key: KeyName::from(vec![0xff, b'\\', 0x00, b'a']), ..link(redis("h", 1), "") },
    ];
    for link in links {
        let uri = link.to_uri();
        assert_eq!(Link::parse(&uri), Ok(link), "{uri}");
    }
}

#[test]
fn a_written_link_stays_readable_and_carries_nothing_secret() {
    let connection = saved("c1", "Prod", "redis://admin:pw@cache.prod:6380/2", Some(("bastion", 22)));
    let mut link = Link::to_key(&connection, None, KeyName::from("user:42")).unwrap();
    link.field = Some("profile".into());
    let uri = link.to_uri();
    assert_eq!(uri, "cachemire://open?v=1&redis=cache.prod:6380&via=bastion:22&db=2&name=Prod&key=user:42&field=profile");
    for secret in ["admin", "pw", "hunter2", "deploy", "ssh-secret", "id_ed25519"] {
        assert!(!uri.contains(secret), "{secret} leaked into {uri}");
    }
    // the characters that end or split a link elsewhere are encoded
    let uri = Link { path: Some("$.a b(c)+'d'".into()), ..link }.to_uri();
    assert!(uri.ends_with("&path=$.a%20b%28c%29%2B%27d%27"), "{uri}");
}

#[test]
fn a_cloud_endpoint_is_named_by_its_account_region_and_id() {
    let connection = saved("aws:dev:sessions", "sessions", "rediss://master.sessions.cache.amazonaws.com:6379", None);
    let cloud = CloudKey { account: "123456789012".into(), region: "eu-west-1".into(), name: "sessions".into() };
    let uri = Link::to_key(&connection, Some(&cloud), KeyName::from("cart:9")).unwrap().to_uri();
    assert_eq!(uri, "cachemire://open?v=1&aws=123456789012/eu-west-1/sessions&key=cart:9", "no profile, no name repeating the id");
}

#[test]
fn reading_is_lenient_on_the_form_and_strict_on_the_meaning() {
    let read = |text: &str| Link::parse(text);
    let plain = read("cachemire://open?v=1&redis=h:6379&key=k").unwrap();
    assert_eq!(plain, link(redis("h", 6379), "k"));
    // what browsers and shells do to a link, and what a newer version may add
    assert_eq!(read("  CacheMire://OPEN/?redis=h&key=k&shiny=new#frag "), Ok(plain.clone()), "default port, no version, extra parameter");
    assert_eq!(read("cachemire:open?redis=h&key=k"), Ok(plain.clone()));
    assert_eq!(read("cachemire://open?redis=h&key=k&key=other").unwrap().key, KeyName::from("k"), "the first one counts");
    assert_eq!(read("cachemire://open?redis=h&key=a+b").unwrap().key, KeyName::from("a+b"), "a plus is a plus");
    assert_eq!(read("cachemire://open?redis=[fe80::1]:7000&key=k").unwrap().server, redis("fe80::1", 7000));

    assert_eq!(read("https://example.com"), Err(LinkError::NotALink));
    assert_eq!(read("cachemire://open?v=2&redis=h&key=k"), Err(LinkError::Newer(2)));
    assert_eq!(read("cachemire://query?redis=h&key=k"), Err(LinkError::UnknownAction("query".into())));
    assert_eq!(read("cachemire://open?azure=x&key=k"), Err(LinkError::NoServer));
    assert_eq!(read("cachemire://open?redis=h&aws=a/r/n&key=k"), Err(LinkError::TwoServers));
    assert_eq!(read("cachemire://open?redis=h&key="), Err(LinkError::NoKey));
    for (text, bad) in [
        ("redis=h:0", "redis"),
        ("redis=h:x", "redis"),
        ("redis=h&tls=yes", "tls"),
        ("redis=h&db=-1", "db"),
        ("redis=h&via=b:99999", "via"),
        ("aws=123/eu-west-1", "aws"),
        ("redis=h&field=%FF", "field"),
    ] {
        assert_eq!(read(&format!("cachemire://open?{text}&key=k")), Err(LinkError::Unreadable(bad)), "{text}");
    }
}

#[test]
fn a_link_finds_the_saved_connection_to_the_same_server_and_database() {
    let connections = [
        saved("other-db", "", "redis://cache.prod:6379/1", None),
        saved("direct", "Direct", "redis://Cache.Prod:6379", None),
        saved("tunnel", "Tunnel", "redis://cache.prod:6379", Some(("bastion", 22))),
        saved("local", "Local", "redis://127.0.0.1:6379", Some(("bastion", 22))),
    ];
    let found = |link: &Link, current: &str| link.find_saved(&connections, current).map(|c| c.id.as_str());
    let to_prod = link(redis("cache.prod", 6379), "k");

    assert_eq!(found(&to_prod, ""), Some("direct"), "the host's case doesn't count, a direct one wins");
    let via = |link: &Link, host: &str| Link {
        server: Server::Redis {
            address: Address { host: host.into(), port: 6379 },
            tls: false,
            via: Some(Address { host: "bastion".into(), port: 22 }),
        },
        ..link.clone()
    };
    assert_eq!(found(&via(&to_prod, "cache.prod"), ""), Some("tunnel"), "through the same SSH server first");
    assert_eq!(found(&Link { name: Some("Tunnel".into()), ..to_prod.clone() }, ""), Some("direct"), "the tunnel before the name");
    assert_eq!(found(&Link { db: 1, ..to_prod.clone() }, ""), Some("other-db"));
    assert_eq!(found(&Link { db: 2, ..to_prod.clone() }, ""), None, "another database is another place");
    assert_eq!(found(&link(redis("cache.prod", 6380), "k"), ""), None);

    // a loopback address is only the same server through the same tunnel
    assert_eq!(found(&via(&to_prod, "localhost"), ""), Some("local"));
    assert_eq!(found(&link(redis("localhost", 6379), "k"), ""), Some("local"), "no tunnel named: any one will do");
    let elsewhere = Link {
        server: Server::Redis {
            address: Address { host: "localhost".into(), port: 6379 },
            tls: false,
            via: Some(Address { host: "jump".into(), port: 22 }),
        },
        ..to_prod.clone()
    };
    assert_eq!(found(&elsewhere, ""), None);

    let twins = [saved("a", "A", "redis://h:6379", None), saved("b", "B", "redis://h:6379", None)];
    let to_h = link(redis("h", 6379), "k");
    assert_eq!(to_h.find_saved(&twins, "").map(|c| c.id.as_str()), Some("a"), "the first of equals");
    assert_eq!(to_h.find_saved(&twins, "b").map(|c| c.id.as_str()), Some("b"), "the one in use");
    let named_a = Link { name: Some("A".into()), ..to_h };
    assert_eq!(named_a.find_saved(&twins, "b").map(|c| c.id.as_str()), Some("a"), "the link's name before the one in use");
}

#[test]
fn a_new_connection_to_the_server_has_no_login() {
    let tls = Link {
        server: Server::Redis { address: Address { host: "::1".into(), port: 6380 }, tls: true, via: None },
        db: 4,
        ..link(redis("h", 1), "k")
    };
    assert_eq!(tls.connection_url().as_deref(), Some("rediss://[::1]:6380/4"));
    assert_eq!(tls.server_label(), "[::1]:6380 (db 4)");
    let cloud = link(Server::Aws { account: "1".into(), region: "r".into(), name: "n".into() }, "k");
    assert_eq!(cloud.connection_url(), None);
    assert_eq!(cloud.server_label(), "n (1, r)");
}
