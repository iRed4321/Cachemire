use super::*;
use crate::secret_vault::{MemoryVault, Secrets};
use std::fs;
use std::sync::atomic::Ordering;

#[test]
fn builds_redis_urls_from_parts() {
    assert_eq!(build_connection_string("127.0.0.1", 6379, 0, "", ""), "redis://127.0.0.1:6379");
    assert_eq!(build_connection_string("cache.local", 6380, 2, "", ""), "redis://cache.local:6380/2");
    assert_eq!(build_connection_string("h", 6379, 0, "", "s3cr@t"), "redis://:s3cr%40t@h:6379");
    assert_eq!(build_connection_string("h", 6379, 1, "app", "p w"), "redis://app:p%20w@h:6379/1");
    assert_eq!(build_connection_string("::1", 6379, 0, "", ""), "redis://[::1]:6379");
}

fn payload(name: &str, url: &str) -> ConnectionPayloadInput {
    ConnectionPayloadInput { name: Some(name.into()), connection_url: Some(url.into()), ..Default::default() }
}

#[test]
fn host_port_and_database_come_from_the_connection_url() {
    let c = connection_from_payload(payload(" prod ", "redis://app:s3cr%40t@cache.local:6380/3"), true).unwrap();
    let e = to_item(&c).endpoint().unwrap();
    assert_eq!((c.name.as_str(), e.host.as_str(), e.port, e.db), ("prod", "cache.local", 6380, 3));
    assert!(!c.ssh_enabled);

    let bare = connection_from_payload(payload("x", "cache.local"), true).unwrap();
    let e = to_item(&bare).endpoint().unwrap();
    assert_eq!((e.host.as_str(), e.port, e.db), ("cache.local", 6379, 0));

    assert_eq!(connection_from_payload(payload("x", ""), true).unwrap_err().to_string(), "Connection URL is required.");
    assert_eq!(connection_from_payload(payload("", "cache.local"), true).unwrap_err().to_string(), "Connection name is required.");
    assert!(connection_from_payload(payload("x", "redis://"), true).is_err());
}

#[test]
fn an_ssh_tunnel_needs_a_host_a_username_and_a_way_to_log_in() {
    let with_ssh = |edit: &dyn Fn(&mut ConnectionPayloadInput)| {
        let mut p = payload("t", "redis://10.0.0.5:6379");
        p.ssh_enabled = Some(true);
        p.ssh_host = Some("bastion.example.com".into());
        p.ssh_username = Some("deploy".into());
        p.ssh_password = Some("pw".into());
        edit(&mut p);
        connection_from_payload(p, true)
    };
    let c = with_ssh(&|_| {}).unwrap();
    let item = to_item(&c);
    let ssh = item.ssh.expect("tunnel settings");
    assert_eq!((ssh.host.as_str(), ssh.port, ssh.username.as_str(), ssh.password.as_str()), ("bastion.example.com", 22, "deploy", "pw"));

    assert_eq!(with_ssh(&|p| p.ssh_host = Some("  ".into())).unwrap_err().to_string(), "SSH host is required.");
    // without a username the SSH profile from the settings is used at connect time
    assert!(
        with_ssh(&|p| {
            p.ssh_username = Some("".into());
            p.ssh_password = Some("".into());
        })
        .is_ok()
    );
    assert_eq!(with_ssh(&|p| p.ssh_port = Some(0)).unwrap_err().to_string(), "SSH port must be a number between 1 and 65535.");
    assert_eq!(with_ssh(&|p| p.ssh_password = Some("".into())).unwrap_err().to_string(), "Provide an SSH password or a private key.");
    assert!(
        with_ssh(&|p| {
            p.ssh_password = Some("".into());
            p.ssh_private_key = Some("~/.ssh/id_ed25519".into());
        })
        .is_ok()
    );

    // switched off, the SSH fields aren't checked and there are no settings
    let off = with_ssh(&|p| {
        p.ssh_enabled = Some(false);
        p.ssh_host = Some("".into());
    })
    .unwrap();
    assert!(to_item(&off).ssh.is_none());
}

#[test]
fn updating_keeps_what_the_form_leaves_out_and_reads_the_new_url() {
    let dir = std::env::temp_dir().join(format!("cachemire-store-test-{}", now_ms()));
    let mut store = ConnectionStore::with_vault(dir.join("databases.json"), Arc::new(MemoryVault::default()));
    let mut p = payload("t", "redis://old.host:6379");
    p.password = Some("keep-me".into());
    p.ssh_enabled = Some(true);
    p.ssh_host = Some("bastion".into());
    p.ssh_username = Some("u".into());
    p.ssh_password = Some("ssh-secret".into());
    store.create(p).unwrap();
    let id = store.active_connection_id().to_string();

    // a blank password box arrives as None: both stored secrets stay
    let mut edit = payload("t2", "redis://new.host:6400/2");
    edit.ssh_host = Some("bastion2".into());
    store.update(&id, edit).unwrap();
    let item = store.get(&id).unwrap();
    let e = item.endpoint().unwrap();
    assert_eq!((item.name.as_str(), e.host.as_str(), e.port, e.db, item.password.as_str()), ("t2", "new.host", 6400, 2, "keep-me"));
    let ssh = item.ssh.unwrap();
    assert_eq!((ssh.host.as_str(), ssh.password.as_str()), ("bastion2", "ssh-secret"));

    // an invalid edit changes nothing
    let mut bad = payload("t3", "redis://x:6379");
    bad.ssh_host = Some("".into());
    assert!(store.update(&id, bad).is_err());
    assert_eq!(store.get(&id).unwrap().name, "t2");

    // a file from before SSH tunnels existed still loads
    let old = r#"{"activeConnectionId":"a","connections":[{"id":"a","name":"Old","connectionUrl":"","host":"h","port":6379,"db":0,"username":"","password":"","lastConnectedAt":null}]}"#;
    std::fs::write(dir.join("old.json"), old).unwrap();
    let store = ConnectionStore::with_vault(dir.join("old.json"), Arc::new(MemoryVault::default()));
    assert!(store.get("a").unwrap().ssh.is_none());
    let _ = std::fs::remove_dir_all(dir);
}

fn temp_file(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!("cachemire-vault-{name}-{}-{}", std::process::id(), now_ms())).join("databases.json")
}

fn with_secrets() -> ConnectionPayloadInput {
    let mut p = payload("t", "redis://cache:6379");
    p.password = Some("redis-secret".into());
    p.ssh_enabled = Some(true);
    p.ssh_host = Some("bastion".into());
    p.ssh_username = Some("u".into());
    p.ssh_password = Some("ssh-secret".into());
    p.ssh_passphrase = Some("phrase-secret".into());
    p
}

fn reads(vault: &MemoryVault) -> usize {
    vault.reads.load(Ordering::SeqCst)
}

#[test]
fn secrets_go_to_the_vault_and_stay_out_of_the_file() {
    let file = temp_file("out");
    let vault = MemoryVault::default();
    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create(with_secrets()).unwrap();
    let id = store.active_connection_id().to_string();

    let saved = fs::read_to_string(&file).unwrap();
    for secret in ["redis-secret", "ssh-secret", "phrase-secret"] {
        assert!(!saved.contains(secret), "{secret} leaked into the file");
    }
    assert_eq!(vault.entries.lock()[&id].ssh_passphrase, "phrase-secret");
    assert_eq!(store.get(&id).unwrap().password, "redis-secret");

    // unrelated changes keep the file clean
    store.set_active(&id).unwrap();
    assert!(!fs::read_to_string(&file).unwrap().contains("redis-secret"));

    // a new run doesn't ask the vault for anything until the connection is used
    let mut reopened = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
    assert_eq!(reads(&vault), 0);
    assert_eq!(reopened.get(&id).unwrap().password, "");
    reopened.unlock_secrets(&id).unwrap();
    let item = reopened.get(&id).unwrap();
    assert_eq!((item.password.as_str(), item.ssh.unwrap().passphrase.as_str()), ("redis-secret", "phrase-secret"));
    assert_eq!(reads(&vault), 1);
    reopened.unlock_secrets(&id).unwrap();
    assert_eq!(reads(&vault), 1, "read once");
}

#[test]
fn a_connection_without_secrets_never_touches_the_vault() {
    let file = temp_file("plain");
    let vault = MemoryVault::default();
    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create(payload("plain", "redis://cache:6379")).unwrap();
    let id = store.active_connection_id().to_string();
    vault.broken.store(true, Ordering::SeqCst);

    let mut reopened = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
    reopened.unlock_secrets(&id).unwrap();
    reopened.set_active(&id).unwrap();
    reopened.update(&id, payload("renamed", "redis://cache:6379")).unwrap();
    assert_eq!(reads(&vault), 0);
}

#[test]
fn without_a_credential_store_the_secrets_stay_in_the_file_until_a_store_appears() {
    let file = temp_file("none");
    let vault = MemoryVault::default();
    vault.broken.store(true, Ordering::SeqCst);
    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create(with_secrets()).unwrap();
    let id = store.active_connection_id().to_string();
    assert!(fs::read_to_string(&file).unwrap().contains("redis-secret"));

    let mut again = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    assert_eq!(again.get(&id).unwrap().password, "redis-secret");
    again.unlock_secrets(&id).unwrap();

    // with a store, using the connection moves them there
    vault.broken.store(false, Ordering::SeqCst);
    let mut reopened = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    assert!(fs::read_to_string(&file).unwrap().contains("redis-secret"), "not before the connection is used");
    reopened.unlock_secrets(&id).unwrap();
    assert_eq!(reopened.get(&id).unwrap().password, "redis-secret");
    assert!(!fs::read_to_string(&file).unwrap().contains("redis-secret"));
    assert_eq!(vault.entries.lock()[&id].password, "redis-secret");
}

#[test]
fn a_vault_that_cannot_be_read_never_costs_the_stored_secrets() {
    let file = temp_file("locked");
    let vault = MemoryVault::default();
    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create(with_secrets()).unwrap();
    let id = store.active_connection_id().to_string();

    vault.broken.store(true, Ordering::SeqCst);
    let mut locked = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
    assert!(locked.unlock_secrets(&id).unwrap_err().to_string().contains("system keyring"));
    // everything that doesn't need the secrets still works, and they stay put
    locked.set_active(&id).unwrap();
    locked.update(&id, payload("renamed", "redis://cache:6379")).unwrap();
    assert!(
        locked.update(&id, ConnectionPayloadInput { password: Some("new".into()), ..payload("renamed", "redis://cache:6379") }).is_err()
    );
    vault.broken.store(false, Ordering::SeqCst);
    assert_eq!(
        vault.entries.lock()[&id],
        Secrets { password: "redis-secret".into(), ssh_password: "ssh-secret".into(), ssh_passphrase: "phrase-secret".into() }
    );
    locked.unlock_secrets(&id).unwrap();
    assert_eq!(locked.get(&id).unwrap().password, "redis-secret");
}

#[test]
fn changing_one_secret_keeps_the_others() {
    let file = temp_file("one");
    let vault = MemoryVault::default();
    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create(with_secrets()).unwrap();
    let id = store.active_connection_id().to_string();

    let mut reopened = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
    let edit = ConnectionPayloadInput { password: Some("new".into()), ..payload("t", "redis://cache:6379") };
    reopened.update(&id, edit).unwrap();
    let stored = vault.entries.lock()[&id].clone();
    assert_eq!(
        (stored.password.as_str(), stored.ssh_password.as_str(), stored.ssh_passphrase.as_str()),
        ("new", "ssh-secret", "phrase-secret")
    );
}

#[test]
fn a_password_typed_into_the_url_is_moved_out_of_it() {
    let file = temp_file("url");
    let vault = MemoryVault::default();
    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create(payload("t", "redis://app:s3cr%40t@cache:6380/2")).unwrap();
    let id = store.active_connection_id().to_string();

    let item = store.get(&id).unwrap();
    assert_eq!((item.connection_url.as_str(), item.username.as_str(), item.password.as_str()), ("redis://app@cache:6380/2", "", "s3cr@t"));
    assert!(!fs::read_to_string(&file).unwrap().contains("s3cr"));
    // copying still gives the complete string
    assert_eq!(store.connection_string(&id).unwrap(), "redis://app:s3cr%40t@cache:6380/2");

    // a URL without credentials picks up the saved ones
    assert_eq!(with_credentials("cache:6379", "", "pw"), "redis://:pw@cache:6379");
    assert_eq!(with_credentials("redis://cache:6379", "", ""), "redis://cache:6379");
    assert_eq!(without_password("redis://:pw@cache:6379").unwrap(), "redis://cache:6379");
}

#[test]
fn removing_a_connection_forgets_it_and_its_secrets() {
    let file = temp_file("remove");
    let vault = MemoryVault::default();
    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create(payload("keep", "redis://a:6379")).unwrap();
    let keep = store.active_connection_id().to_string();
    store.create(with_secrets()).unwrap();
    let gone = store.active_connection_id().to_string();
    assert!(vault.entries.lock().contains_key(&gone));

    // even with the vault unreachable the connection goes; clearing its entry comes after
    let cleanup = store.remove(&gone).unwrap().expect("it had secrets in the vault");
    assert!(store.get(&gone).is_none());
    assert_eq!(store.active_connection_id(), "", "the active connection was removed");
    assert!(store.get(&keep).is_some());
    assert!(vault.entries.lock().contains_key(&gone));
    cleanup.run();
    assert!(!vault.entries.lock().contains_key(&gone));

    // gone from the file too, and removing a plain one has nothing to clean
    assert!(store.remove(&keep).unwrap().is_none());
    assert!(store.remove(&keep).is_err());
    let reopened = ConnectionStore::with_vault(file, Arc::new(vault));
    assert!(reopened.get(&keep).is_none() && reopened.get(&gone).is_none());
}

#[test]
fn a_file_that_cannot_be_read_is_kept_and_never_overwritten() {
    let file = temp_file("unusable");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    let broken = r#"{"activeConnectionId": "a", "connections": [{"id": "a", "port": "not a number"}]}"#;
    fs::write(&file, broken).unwrap();

    let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    store.create(payload("fresh", "redis://cache:6379")).unwrap();
    // what was there is still there, next to the new file
    let kept: Vec<_> = fs::read_dir(file.parent().unwrap())
        .unwrap()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().contains(".unusable-"))
        .collect();
    assert_eq!(kept.len(), 1);
    assert_eq!(fs::read_to_string(kept[0].path()).unwrap(), broken);
    assert!(fs::read_to_string(&file).unwrap().contains("fresh"));
}
