use super::*;
use cachemire_core::test_support::temp_dir;

fn connection(name: &str, url: &str) -> ConnectionPayloadInput {
    ConnectionPayloadInput { name: Some(name.into()), connection_url: Some(url.into()), ..Default::default() }
}

fn with_tunnel(mut payload: ConnectionPayloadInput) -> ConnectionPayloadInput {
    payload.ssh_enabled = Some(true);
    payload.ssh_host = Some("bastion.example".into());
    payload.ssh_port = Some(2222);
    payload.ssh_username = Some("deploy".into());
    payload.ssh_password = Some("ssh-secret".into());
    payload.ssh_private_key = Some("~/.ssh/id_ed25519".into());
    payload.ssh_passphrase = Some("key-secret".into());
    payload
}

/// A state holding `payloads` as saved connections, with their ids.
fn state_with(payloads: Vec<ConnectionPayloadInput>) -> (AppState, Vec<String>) {
    let state = AppState::for_tests(&temp_dir("transfer"));
    let ids: Vec<String> = (0..payloads.len()).map(|i| format!("c{i}")).collect();
    {
        let mut store = state.connections.lock_recover();
        for (id, payload) in ids.iter().zip(payloads) {
            store.create_with_id(id, payload).unwrap();
        }
    }
    (state, ids)
}

fn export_to_text(state: &AppState, ids: &[String], passwords: bool) -> String {
    let path = temp_dir("export").join("out.json");
    assert_eq!(write_export(state, ids, passwords, 0, &path).unwrap(), ids.len());
    std::fs::read_to_string(path).unwrap()
}

#[test]
fn an_export_reads_back_as_the_same_connections() {
    let mut prod = with_tunnel(connection("Prod", "redis://app@cache.example:6380/3"));
    prod.password = Some("redis-secret".into());
    let (state, ids) = state_with(vec![prod, connection("Local", "redis://127.0.0.1:6379")]);

    let drafts = parse_drafts(&export_to_text(&state, &ids, true)).unwrap();
    assert_eq!(drafts.len(), 2);
    let (prod, local) = (drafts[0].payload(), drafts[1].payload());
    assert_eq!(prod.name.as_deref(), Some("Prod"));
    assert_eq!(prod.connection_url.as_deref(), Some("redis://app@cache.example:6380/3"));
    assert_eq!(prod.password.as_deref(), Some("redis-secret"));
    assert_eq!(prod.ssh_enabled, Some(true));
    assert_eq!((prod.ssh_host.as_deref(), prod.ssh_port, prod.ssh_username.as_deref()), (Some("bastion.example"), Some(2222), Some("deploy")));
    assert_eq!(prod.ssh_private_key.as_deref(), Some("~/.ssh/id_ed25519"));
    assert_eq!((prod.ssh_password.as_deref(), prod.ssh_passphrase.as_deref()), (Some("ssh-secret"), Some("key-secret")));
    assert_eq!(local.ssh_enabled, Some(false));
    assert_eq!(local.password, None);
}

#[test]
fn an_export_without_passwords_holds_no_secret_anywhere() {
    // the password typed into the URL is moved out of it when the connection is saved
    let mut payload = with_tunnel(connection("Prod", "redis://app:url-secret@cache.example:6379"));
    payload.password = None;
    let (state, ids) = state_with(vec![payload]);

    let text = export_to_text(&state, &ids, false);
    for secret in ["url-secret", "ssh-secret", "key-secret"] {
        assert!(!text.contains(secret), "{secret} leaked into {text}");
    }
    let file: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(file["includesPasswords"], false);
    assert_eq!(file["count"], 1);
    let draft = &parse_drafts(&text).unwrap()[0];
    assert_eq!(draft.payload().password, None);
    assert_eq!(draft.payload().ssh_password, None);
}

#[test]
fn an_exported_file_imports_into_a_store_as_new_connections() {
    let (state, ids) = state_with(vec![with_tunnel(connection("Prod", "redis://cache.example:6379"))]);
    let text = export_to_text(&state, &ids, true);

    let target = AppState::for_tests(&temp_dir("import"));
    let mut store = target.connections.lock_recover();
    let active_before = store.active_connection_id().to_string();
    for draft in parse_drafts(&text).unwrap() {
        store.import_with_id("imported", draft.payload()).unwrap();
    }
    let imported = store.get("imported").unwrap();
    assert_eq!(imported.name, "Prod");
    assert_eq!(imported.ssh.unwrap().host, "bastion.example");
    assert_eq!(store.active_connection_id(), active_before, "an import never switches connection");
}

#[test]
fn a_file_from_an_older_version_gives_its_host_port_and_database() {
    let drafts = parse_drafts(r#"[{"name": "Old", "host": "legacy.local", "port": 6390, "db": 4, "username": " app "}]"#).unwrap();
    let payload = drafts[0].payload();
    assert_eq!(payload.connection_url.as_deref(), Some("redis://legacy.local:6390/4"));
    assert_eq!(payload.username.as_deref(), Some("app"));
}

#[test]
fn the_entries_that_cant_be_a_connection_are_skipped() {
    let text = r#"{"connections": [
            {"name": "", "connectionUrl": "redis://a:6379"},
            {"name": "No address"},
            {"name": "Bad port", "host": "h", "port": 70000},
            {"name": "Tunnel without host", "connectionUrl": "redis://b:6379", "ssh": {"host": " ", "username": "u"}},
            "not an object"
        ]}"#;
    let drafts = parse_drafts(text).unwrap();
    let names: Vec<&str> = drafts.iter().map(|d| d.name.as_str()).collect();
    assert_eq!(names, ["Bad port", "Tunnel without host"]);
    assert_eq!(drafts[0].connection_url, "redis://h:6379", "an impossible port falls back to the default");
    assert!(drafts[1].ssh.is_none());
}

#[test]
fn a_file_with_no_connection_or_no_json_is_refused() {
    for text in ["", "{", "{}", r#"{"connections": []}"#, r#"[{"name": ""}]"#, "42"] {
        assert!(parse_drafts(text).is_err(), "{text:?} was accepted");
    }
}

#[test]
fn an_imported_name_already_taken_gets_the_next_free_number() {
    let mut taken: FxHashSet<String> = ["prod".to_string(), "prod (2)".to_string()].into_iter().collect();
    assert_eq!(free_name("Prod", &mut taken), "Prod (3)");
    assert_eq!(free_name("PROD", &mut taken), "PROD (4)", "names are compared whatever their case");
    assert_eq!(free_name("Staging", &mut taken), "Staging");
    assert_eq!(free_name("Staging", &mut taken), "Staging (2)", "two drafts of one file don't share a name");
}

#[test]
fn utc_dates_are_right_across_leap_years_and_before_1970() {
    assert_eq!(utc_parts(0), (1970, 1, 1, 0, 0, 0, 0));
    // 2024-02-29T23:59:59.999Z
    assert_eq!(utc_parts(1_709_251_199_999), (2024, 2, 29, 23, 59, 59, 999));
    // 2000-03-01T00:00:00Z, the day after a leap day of a century leap year
    assert_eq!(utc_parts(951_868_800_000), (2000, 3, 1, 0, 0, 0, 0));
    // 1969-12-31T23:59:59.500Z
    assert_eq!(utc_parts(-500), (1969, 12, 31, 23, 59, 59, 500));
}

#[test]
fn the_export_says_when_it_was_made() {
    let (state, ids) = state_with(vec![connection("Local", "redis://127.0.0.1:6379")]);
    let path = temp_dir("stamp").join("out.json");
    write_export(&state, &ids, false, 1_709_251_199_999, &path).unwrap();
    let file: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(file["exportedAtUtc"], "2024-02-29T23:59:59.999Z");
}

#[test]
fn an_ipv6_server_survives_an_export_and_an_import() {
    let (state, ids) = state_with(vec![connection("Local v6", "redis://[::1]:6380/2")]);
    let text = export_to_text(&state, &ids, false);
    let file: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(file["connections"][0]["host"], "::1", "the host field holds the bare address");
    let drafts = parse_drafts(&text).unwrap();
    assert_eq!(drafts[0].connection_url, "redis://[::1]:6380/2");
    // a file that only has the host, port and database
    let old = parse_drafts(r#"[{"name": "v6", "host": "::1", "port": 6380, "db": 2}]"#).unwrap();
    assert_eq!(old[0].connection_url, "redis://[::1]:6380/2");
}
