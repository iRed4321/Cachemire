use super::*;

#[test]
fn a_leading_tilde_is_the_home_directory() {
    let home = dirs::home_dir().unwrap();
    assert_eq!(expand_home("~"), home);
    assert_eq!(expand_home("~/.ssh/id_ed25519"), home.join(".ssh/id_ed25519"));
    assert_eq!(expand_home("/etc/key"), PathBuf::from("/etc/key"));
    assert_eq!(expand_home("~other/key"), PathBuf::from("~other/key"));
    assert_eq!(expand_home("relative/key"), PathBuf::from("relative/key"));
}

fn key(byte: u8) -> PublicKeyOrCertificate {
    use russh::keys::ssh_key::public::{Ed25519PublicKey, KeyData};
    PublicKeyOrCertificate::PublicKey {
        key: russh::keys::PublicKey::new(KeyData::Ed25519(Ed25519PublicKey([byte; 32])), ""),
        hash_alg: None,
    }
}

/// Whether `known_hosts` accepts `key` for `host:port`, and why not when it doesn't.
fn check(known_hosts: &Path, host: &str, port: u16, key: &PublicKeyOrCertificate) -> (bool, Option<String>) {
    use russh::client::Handler;
    let refusal = Arc::new(Mutex::new(None));
    let mut checker = HostKeyCheck { host: host.into(), port, known_hosts: known_hosts.to_path_buf(), refusal: refusal.clone() };
    let accepted = crate::test_support::block_on(checker.check_server_key(key)).unwrap();
    (accepted, refusal.lock().take().map(|why| why.to_string()))
}

#[test]
fn a_server_seen_first_is_remembered_and_trusted_again() {
    let known_hosts = crate::test_support::temp_dir("ssh").join("ssh_known_hosts");
    assert_eq!(check(&known_hosts, "bastion", 22, &key(1)), (true, None));
    assert!(std::fs::read_to_string(&known_hosts).unwrap().contains("bastion"));
    assert_eq!(check(&known_hosts, "bastion", 22, &key(1)), (true, None));
}

#[test]
fn a_changed_host_key_is_refused_and_says_where_it_was_recorded() {
    let known_hosts = crate::test_support::temp_dir("ssh").join("ssh_known_hosts");
    check(&known_hosts, "bastion", 22, &key(1));
    let (accepted, why) = check(&known_hosts, "bastion", 22, &key(2));
    assert!(!accepted, "an impersonated server must not be trusted");
    let why = why.unwrap();
    assert!(why.contains(&known_hosts.display().to_string()), "{why}");
    // the line it names is the one to delete
    let line: usize = why.split("line ").nth(1).and_then(|rest| rest.split(' ').next()).and_then(|n| n.parse().ok()).unwrap();
    let recorded = std::fs::read_to_string(&known_hosts).unwrap();
    assert!(recorded.lines().nth(line - 1).unwrap().starts_with("bastion "), "line {line} of {recorded:?}");
    assert_eq!(check(&known_hosts, "bastion", 22, &key(1)), (true, None), "the recorded key still works");
}

#[test]
fn each_host_and_port_has_its_own_key() {
    let known_hosts = crate::test_support::temp_dir("ssh").join("ssh_known_hosts");
    check(&known_hosts, "bastion", 22, &key(1));
    assert!(check(&known_hosts, "bastion", 2222, &key(2)).0, "another port is another server");
    assert!(check(&known_hosts, "other", 22, &key(3)).0);
    assert!(!check(&known_hosts, "bastion", 2222, &key(1)).0);
}

#[test]
fn a_known_hosts_file_that_cant_be_written_refuses_the_key() {
    let dir = crate::test_support::temp_dir("ssh");
    // a directory where the file should be
    let known_hosts = dir.join("ssh_known_hosts");
    std::fs::create_dir(&known_hosts).unwrap();
    let (accepted, why) = check(&known_hosts, "bastion", 22, &key(1));
    assert!(!accepted);
    assert!(why.is_some());
}
