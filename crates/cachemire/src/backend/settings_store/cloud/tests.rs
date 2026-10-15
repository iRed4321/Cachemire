use std::sync::Arc;

use super::*;
use crate::backend::LockExt;
use crate::backend::models::ProfileColor;
use crate::backend::secret_vault::MemoryVault;
use crate::test_support::temp_dir;

fn key(name: &str) -> CloudKey {
    CloudKey { account: "111".into(), region: "eu-west-1".into(), name: name.into() }
}

fn tunnel(host: &str, password: &str) -> CloudSsh {
    CloudSsh { enabled: true, host: host.into(), port: 22, username: "ec2-user".into(), private_key: String::new(), password: password.into(), passphrase: String::new(), in_vault: false }
}

fn store_in(file: &std::path::Path, vault: &MemoryVault) -> SettingsStore {
    SettingsStore::with_vault(file.to_path_buf(), Arc::new(vault.clone()))
}

fn new_store() -> (SettingsStore, MemoryVault, std::path::PathBuf) {
    let file = temp_dir("cloud").join("settings.json");
    let vault = MemoryVault::default();
    (store_in(&file, &vault), vault, file)
}

fn account() -> CloudScope {
    CloudScope::Account("111".into())
}

fn endpoint(name: &str) -> CloudScope {
    CloudScope::Endpoint(key(name))
}

/// The profile and tunnel host `name` ends up with.
fn effective(store: &SettingsStore, name: &str) -> (String, Option<String>) {
    let (profile, ssh) = store.cloud_effective(&key(name));
    (profile, ssh.map(|(_, ssh)| ssh.host.clone()))
}

#[test]
fn an_endpoint_inherits_its_accounts_settings_until_it_sets_its_own() {
    let (mut store, _, _) = new_store();
    let staging = store.create_connection_profile("Staging", ProfileColor::Green).unwrap();
    let prod = store.connection_profiles()[0].id.clone();
    store.set_cloud_scope(&account(), "Main", Some(staging.clone()), Some(tunnel("account-bastion", ""))).unwrap();
    store.set_cloud_scope(&endpoint("orders"), "orders", Some(prod.clone()), None).unwrap();
    store.set_cloud_scope(&endpoint("cache"), "cache", None, Some(tunnel("own-bastion", ""))).unwrap();

    assert_eq!(effective(&store, "orders"), (prod, Some("account-bastion".into())));
    assert_eq!(effective(&store, "cache"), (staging.clone(), Some("own-bastion".into())));
    assert_eq!(effective(&store, "never-set"), (staging, Some("account-bastion".into())));
}

#[test]
fn an_endpoint_can_turn_off_what_its_account_sets() {
    let (mut store, _, _) = new_store();
    let prod = store.connection_profiles()[0].id.clone();
    store.set_cloud_scope(&account(), "", Some(prod), Some(tunnel("bastion", ""))).unwrap();
    store.set_cloud_scope(&endpoint("direct"), "", Some(String::new()), Some(CloudSsh::none())).unwrap();
    assert_eq!(effective(&store, "direct"), (String::new(), None));
}

#[test]
fn a_deleted_connection_profile_counts_as_none() {
    let (mut store, _, _) = new_store();
    let staging = store.create_connection_profile("Staging", ProfileColor::Green).unwrap();
    store.set_cloud_scope(&account(), "", Some(staging.clone()), None).unwrap();
    store.remove_connection_profile(&staging).unwrap();
    assert_eq!(effective(&store, "any").0, "");
}

#[test]
fn settings_that_cant_be_used_are_refused() {
    let (mut store, _, _) = new_store();
    assert!(store.set_cloud_scope(&account(), "", Some("no-such-profile".into()), None).is_err());
    assert!(store.set_cloud_scope(&account(), "", None, Some(tunnel("  ", ""))).is_err());
    let mut no_port = tunnel("bastion", "");
    no_port.port = 0;
    assert!(store.set_cloud_scope(&account(), "", None, Some(no_port)).is_err());
    let mut off = tunnel("", "");
    off.enabled = false;
    assert!(store.set_cloud_scope(&account(), "", None, Some(off)).is_ok(), "a tunnel turned off needs no host");
}

#[test]
fn listed_endpoints_get_their_settings_without_secrets_until_asked() {
    let (mut store, _, _) = new_store();
    store.set_cloud_scope(&account(), "", None, Some(tunnel("bastion", "pw"))).unwrap();
    let mut items = vec![ConnectionItem { id: "x".into(), name: "orders".into(), connection_url: "redis://h:6379".into(), username: String::new(), password: String::new(), ssh: None, profile_id: String::new() }];
    store.apply_cloud("111", "eu-west-1", &mut items);
    let mut ssh = items[0].ssh.clone().unwrap();
    assert_eq!((ssh.host.as_str(), ssh.password.as_str()), ("bastion", ""));
    store.fill_cloud_secrets(&key("orders"), &mut ssh);
    assert_eq!(ssh.password, "pw");
    assert_eq!(store.cloud_settings(&account()).ssh.unwrap().password, "", "what the settings show holds no secret");

    store.apply_cloud("222", "eu-west-1", &mut items);
    assert!(items[0].ssh.is_none(), "another account's endpoint has nothing set");
}

#[test]
fn tunnel_secrets_live_in_the_vault_and_come_back_after_a_restart() {
    let (mut store, vault, file) = new_store();
    store.set_cloud_scope(&endpoint("orders"), "", None, Some(tunnel("bastion", "s3cret"))).unwrap();
    assert!(!std::fs::read_to_string(&file).unwrap().contains("s3cret"));
    assert_eq!(vault.entries.lock_recover()[&cloud_vault_key(&endpoint("orders"))].ssh_password, "s3cret");

    let mut reopened = store_in(&file, &vault);
    let mut ssh = SshSettings { host: String::new(), port: 22, username: String::new(), password: String::new(), private_key: String::new(), passphrase: String::new() };
    reopened.fill_cloud_secrets(&key("orders"), &mut ssh);
    assert_eq!(ssh.password, "", "nothing is read from the vault before it's needed");
    reopened.unlock_cloud(&key("orders")).unwrap();
    reopened.fill_cloud_secrets(&key("orders"), &mut ssh);
    assert_eq!(ssh.password, "s3cret");
}

#[test]
fn a_blank_password_keeps_the_stored_one() {
    let (mut store, vault, file) = new_store();
    store.set_cloud_scope(&account(), "", None, Some(tunnel("bastion", "s3cret"))).unwrap();
    // a new run edits the tunnel's host without retyping the password
    let mut reopened = store_in(&file, &vault);
    reopened.set_cloud_scope(&account(), "", None, Some(tunnel("new-bastion", ""))).unwrap();
    reopened.unlock_cloud(&key("any")).unwrap();
    let mut ssh = SshSettings { host: String::new(), port: 22, username: String::new(), password: String::new(), private_key: String::new(), passphrase: String::new() };
    reopened.fill_cloud_secrets(&key("any"), &mut ssh);
    assert_eq!(ssh.password, "s3cret");
    assert_eq!(effective(&reopened, "any").1.as_deref(), Some("new-bastion"));
}

#[test]
fn turning_a_tunnel_off_hands_back_the_clean_up_of_its_secrets() {
    let (mut store, vault, _) = new_store();
    store.set_cloud_scope(&account(), "", None, Some(tunnel("bastion", "s3cret"))).unwrap();
    let cleanup = store.set_cloud_scope(&account(), "", None, Some(CloudSsh::none())).unwrap().expect("a clean-up");
    cleanup.run();
    assert!(vault.entries.lock_recover().is_empty());

    store.set_cloud_scope(&account(), "", None, Some(tunnel("bastion", "again"))).unwrap();
    assert!(store.set_cloud_scope(&account(), "", None, None).unwrap().is_some(), "back to inheriting clears it too");
}

#[test]
fn inheriting_everything_again_forgets_the_entry_but_a_label_is_kept_while_it_lives() {
    let (mut store, _, _) = new_store();
    let prod = store.connection_profiles()[0].id.clone();
    store.set_cloud_scope(&endpoint("orders"), "Orders cache", Some(prod.clone()), None).unwrap();
    store.set_cloud_scope(&endpoint("orders"), "", Some(prod), None).unwrap();
    assert_eq!(store.cloud_settings(&endpoint("orders")).label, "Orders cache");
    store.set_cloud_scope(&endpoint("orders"), "", None, None).unwrap();
    assert!(store.cloud_entry(&endpoint("orders")).is_none());
}

#[test]
fn an_endpoint_missing_from_three_listings_in_a_row_loses_its_settings() {
    let (mut store, vault, file) = new_store();
    store.set_cloud_scope(&endpoint("gone"), "", None, Some(tunnel("bastion", "s3cret"))).unwrap();
    store.set_cloud_scope(&endpoint("flaky"), "", None, Some(tunnel("bastion", ""))).unwrap();
    let other_region = CloudScope::Endpoint(CloudKey { account: "111".into(), region: "us-east-1".into(), name: "gone".into() });
    store.set_cloud_scope(&other_region, "", None, Some(tunnel("bastion", ""))).unwrap();

    let listed = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<FxHashSet<String>>();
    assert!(store.observe_cloud_listing("111", "eu-west-1", &listed(&[])).unwrap().is_empty());
    assert!(store.observe_cloud_listing("111", "eu-west-1", &listed(&["flaky"])).unwrap().is_empty());
    let cleanups = store.observe_cloud_listing("111", "eu-west-1", &listed(&[])).unwrap();
    assert!(store.cloud_entry(&endpoint("gone")).is_none(), "missed three times in a row");
    assert!(store.cloud_entry(&endpoint("flaky")).is_some(), "seen in between: its count started over");
    assert!(store.cloud_entry(&other_region).is_some(), "another region's listing doesn't count");
    assert_eq!(cleanups.len(), 1);
    cleanups.into_iter().for_each(SecretsCleanup::run);
    assert!(!vault.entries.lock_recover().contains_key(&cloud_vault_key(&endpoint("gone"))));
    assert!(store_in(&file, &vault).cloud_entry(&endpoint("gone")).is_none(), "the pruning is saved");
}
