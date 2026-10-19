use crate::connection::SshSettings;
use crate::settings::{ConnectionProfile, SavedQuery};
use crate::secret_vault::MemoryVault;
use super::ssh_profiles::{SshProfileItem, vault_key};
use super::*;
use std::fs;
use std::sync::atomic::Ordering;

fn temp_file(name: &str) -> PathBuf {
    std::env::temp_dir()
        .join(format!("cachemire-settings-{name}-{}-{}", std::process::id(), crate::now_ms()))
        .join("settings.json")
}

fn ssh(username: &str, password: &str) -> SshSettings {
    SshSettings {
        host: "bastion".into(),
        port: 22,
        username: username.into(),
        password: password.into(),
        private_key: String::new(),
        passphrase: String::new(),
    }
}

#[test]
fn profiles_are_listed_without_their_passwords_and_the_first_is_active() {
    let mut store = SettingsStore::with_vault(temp_file("list"), Arc::new(MemoryVault::default()));
    assert!(store.ssh_profiles().is_empty());
    let work = store.create_ssh_profile(" Work ", " deploy ", "pw1").unwrap();
    let home = store.create_ssh_profile("Home", "me", "pw2").unwrap();
    let listed = store.ssh_profiles();
    assert_eq!(
        listed,
        vec![
            SshProfileItem { id: work.clone(), name: "Work".into(), username: "deploy".into(), active: true },
            SshProfileItem { id: home.clone(), name: "Home".into(), username: "me".into(), active: false },
        ]
    );

    store.set_active_ssh_profile(&home).unwrap();
    assert_eq!(store.active_ssh_profile(), home);
    store.set_active_ssh_profile("").unwrap();
    assert!(store.ssh_profiles().iter().all(|p| !p.active));
    assert!(store.set_active_ssh_profile("nope").is_err());

    assert_eq!(store.create_ssh_profile("", "u", "p").unwrap_err(), AppError::invalid("The profile needs a name."));
    assert_eq!(store.create_ssh_profile("n", " ", "p").unwrap_err(), AppError::invalid("The profile needs an SSH username."));
    assert_eq!(store.create_ssh_profile("n", "u", "").unwrap_err(), AppError::invalid("The profile needs an SSH password."));
}

#[test]
fn passwords_go_to_the_vault_and_are_only_read_when_needed() {
    let file = temp_file("vault");
    let vault = MemoryVault::default();
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(vault.clone()));
    let id = store.create_ssh_profile("Work", "deploy", "s3cret").unwrap();
    assert!(!fs::read_to_string(&file).unwrap().contains("s3cret"), "the file holds no password");
    assert_eq!(vault.entries.lock_recover()[&vault_key(&id)].ssh_password, "s3cret");

    // a new run reads nothing until a connection needs the profile
    let mut reopened = SettingsStore::with_vault(file.clone(), Arc::new(vault.clone()));
    assert_eq!(vault.reads.load(Ordering::SeqCst), 0);
    reopened.unlock_active_ssh_profile().unwrap();
    assert_eq!(vault.reads.load(Ordering::SeqCst), 1);
    let mut settings = ssh("", "");
    reopened.fill_ssh(&mut settings).unwrap();
    assert_eq!((settings.username.as_str(), settings.password.as_str()), ("deploy", "s3cret"));

    // changing the name or the username keeps the password; a new one replaces it
    reopened.update_ssh_profile(&id, "Work 2", "deploy2", None).unwrap();
    assert_eq!(vault.entries.lock_recover()[&vault_key(&id)].ssh_password, "s3cret");
    reopened.update_ssh_profile(&id, "Work 2", "deploy2", Some("")).unwrap();
    assert_eq!(vault.entries.lock_recover()[&vault_key(&id)].ssh_password, "s3cret", "a blank box keeps it");
    reopened.update_ssh_profile(&id, "Work 2", "deploy2", Some("n3w")).unwrap();
    assert_eq!(vault.entries.lock_recover()[&vault_key(&id)].ssh_password, "n3w");
    assert!(!fs::read_to_string(&file).unwrap().contains("n3w"));
}

#[test]
fn editing_an_unloaded_profile_does_not_touch_the_vault() {
    let file = temp_file("lazy");
    let vault = MemoryVault::default();
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(vault.clone()));
    let id = store.create_ssh_profile("Work", "deploy", "s3cret").unwrap();

    let mut reopened = SettingsStore::with_vault(file, Arc::new(vault.clone()));
    vault.broken.store(true, Ordering::SeqCst);
    reopened.update_ssh_profile(&id, "Renamed", "deploy", None).unwrap();
    reopened.set_active_ssh_profile(&id).unwrap();
    assert!(reopened.unlock_active_ssh_profile().unwrap_err().message().contains("system keyring"));
    vault.broken.store(false, Ordering::SeqCst);
    assert_eq!(vault.entries.lock_recover()[&vault_key(&id)].ssh_password, "s3cret", "nothing was lost");
}

#[test]
fn without_a_credential_store_the_password_stays_in_the_file() {
    let file = temp_file("plain");
    let vault = MemoryVault::default();
    vault.broken.store(true, Ordering::SeqCst);
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(vault.clone()));
    store.create_ssh_profile("Work", "deploy", "s3cret").unwrap();
    assert!(fs::read_to_string(&file).unwrap().contains("s3cret"));

    // it moves to the vault once there is one and the profile is used
    vault.broken.store(false, Ordering::SeqCst);
    let mut reopened = SettingsStore::with_vault(file.clone(), Arc::new(vault.clone()));
    reopened.unlock_active_ssh_profile().unwrap();
    assert!(!fs::read_to_string(&file).unwrap().contains("s3cret"));
    assert_eq!(vault.entries.lock_recover().values().next().unwrap().ssh_password, "s3cret");
}

#[test]
fn a_connection_with_its_own_username_ignores_the_profile() {
    let mut store = SettingsStore::with_vault(temp_file("fill"), Arc::new(MemoryVault::default()));

    let mut none_active = ssh("", "");
    assert!(store.fill_ssh(&mut none_active).unwrap_err().message().contains("SSH profile"));

    store.create_ssh_profile("Work", "deploy", "profile-pw").unwrap();
    let mut own = ssh("me", "my-pw");
    store.fill_ssh(&mut own).unwrap();
    assert_eq!((own.username.as_str(), own.password.as_str()), ("me", "my-pw"));
    let mut key_only = ssh("me", "");
    store.fill_ssh(&mut key_only).unwrap();
    assert_eq!((key_only.username.as_str(), key_only.password.as_str()), ("me", ""), "the profile's password never goes with another username");

    let mut empty = ssh("", "");
    store.fill_ssh(&mut empty).unwrap();
    assert_eq!((empty.username.as_str(), empty.password.as_str()), ("deploy", "profile-pw"));
}

#[test]
fn removing_a_profile_clears_it_and_its_vault_entry() {
    let file = temp_file("remove");
    let vault = MemoryVault::default();
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(vault.clone()));
    let id = store.create_ssh_profile("Work", "deploy", "s3cret").unwrap();
    let cleanup = store.remove_ssh_profile(&id).unwrap().expect("its password was in the vault");
    assert!(store.ssh_profiles().is_empty());
    assert_eq!(store.active_ssh_profile(), "", "the active profile was removed");
    assert!(vault.entries.lock_recover().contains_key(&vault_key(&id)));
    cleanup.run();
    assert!(vault.entries.lock_recover().is_empty());
    assert!(store.remove_ssh_profile(&id).is_err());
    assert!(SettingsStore::with_vault(file, Arc::new(vault)).ssh_profiles().is_empty());
}

#[test]
fn a_settings_file_that_cannot_be_read_is_kept() {
    let file = temp_file("unusable");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, "{ broken").unwrap();
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    assert!(store.ssh_profiles().is_empty());
    store.create_ssh_profile("Work", "deploy", "pw").unwrap();
    let kept = fs::read_dir(file.parent().unwrap()).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().contains(".unusable-")).count();
    assert_eq!(kept, 1, "the file that couldn't be read was moved aside, not overwritten");
}

#[test]
fn a_first_run_has_a_red_production_profile_that_stays_deleted_once_removed() {
    let file = temp_file("connection-profiles");
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    let listed = store.connection_profiles();
    assert_eq!(listed, vec![ConnectionProfile { id: DEFAULT_PROFILE_ID.into(), name: "Production".into(), color: ProfileColor::Red }]);

    store.remove_connection_profile(DEFAULT_PROFILE_ID).unwrap();
    assert!(store.connection_profiles().is_empty());
    assert!(store.remove_connection_profile(DEFAULT_PROFILE_ID).is_err());
    let reopened = SettingsStore::with_vault(file, Arc::new(MemoryVault::default()));
    assert!(reopened.connection_profiles().is_empty(), "it is not added again");
}

#[test]
fn a_settings_file_from_before_connection_profiles_gets_the_default_one_once() {
    let file = temp_file("connection-profiles-old");
    fs::create_dir_all(file.parent().unwrap()).unwrap();
    fs::write(&file, r#"{"sshProfiles":[],"activeSshProfile":"","language":"fr"}"#).unwrap();
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    assert_eq!(store.connection_profiles().len(), 1);
    assert_eq!(store.language(), Language::French);
    // saved with the next change, and gone for good if it's deleted
    store.remove_connection_profile(DEFAULT_PROFILE_ID).unwrap();
    assert!(SettingsStore::with_vault(file, Arc::new(MemoryVault::default())).connection_profiles().is_empty());
}

#[test]
fn connection_profiles_are_created_renamed_recolored_and_read_back() {
    let file = temp_file("connection-profiles-crud");
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    let dev = store.create_connection_profile(" dev ", ProfileColor::Green).unwrap();
    assert_eq!(store.connection_profile(&dev), Some(ConnectionProfile { id: dev.clone(), name: "dev".into(), color: ProfileColor::Green }));
    assert_eq!(store.create_connection_profile("  ", ProfileColor::Blue).unwrap_err(), AppError::invalid("The profile needs a name."));

    store.update_connection_profile(&dev, "staging", ProfileColor::Purple).unwrap();
    assert_eq!(store.connection_profile(&dev).map(|p| (p.name, p.color)), Some(("staging".to_string(), ProfileColor::Purple)));
    assert!(store.update_connection_profile("nope", "x", ProfileColor::Red).is_err());
    assert!(store.update_connection_profile(&dev, "", ProfileColor::Red).is_err());

    let reopened = SettingsStore::with_vault(file, Arc::new(MemoryVault::default()));
    assert_eq!(reopened.connection_profile(&dev).map(|p| (p.name, p.color)), Some(("staging".to_string(), ProfileColor::Purple)));
}

#[test]
fn the_experimental_server_search_is_off_until_turned_on_and_is_kept() {
    let file = temp_file("server-search");
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    assert!(!store.server_search());
    store.set_server_search(true).unwrap();
    assert!(SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default())).server_search());
    store.set_server_search(false).unwrap();
    assert!(!SettingsStore::with_vault(file, Arc::new(MemoryVault::default())).server_search());
}

#[test]
fn the_search_depth_is_one_of_the_choices_and_deep_until_another_is_picked() {
    let file = temp_file("search-depth");
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    assert_eq!(store.search_depth(), DEFAULT_SEARCH_DEPTH);
    store.set_search_depth(60).unwrap();
    assert_eq!(SettingsStore::with_vault(file, Arc::new(MemoryVault::default())).search_depth(), 60);
    assert!(store.set_search_depth(7).is_err());
    assert_eq!(store.search_depth(), 60, "a refused choice changes nothing");
    assert!(SEARCH_DEPTHS.contains(&DEFAULT_SEARCH_DEPTH));
}

#[test]
fn saved_queries_are_created_listed_and_removed() {
    let file = temp_file("saved-queries");
    let mut store = SettingsStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
    assert!(store.saved_queries().is_empty());

    let id = store.create_saved_query(" Orders by product ", "FROM KEY 'orders' AS o\nSELECT *", "").unwrap();
    assert_eq!(
        store.saved_query(&id),
        Some(SavedQuery { id: id.clone(), name: "Orders by product".into(), text: "FROM KEY 'orders' AS o\nSELECT *".into(), connection_id: String::new() })
    );
    assert_eq!(store.saved_queries().len(), 1);

    assert_eq!(store.create_saved_query("  ", "SELECT *", "").unwrap_err(), AppError::invalid("The query needs a name."));
    assert_eq!(store.create_saved_query("Untitled", "  ", "").unwrap_err(), AppError::invalid("The query needs some text to save."));

    store.remove_saved_query(&id).unwrap();
    assert!(store.saved_queries().is_empty());
    assert!(store.remove_saved_query(&id).is_err());

    // saved across a reopen
    let id2 = store.create_saved_query("Kept", "SELECT *", "").unwrap();
    let reopened = SettingsStore::with_vault(file, Arc::new(MemoryVault::default()));
    assert_eq!(reopened.saved_query(&id2).map(|q| q.name), Some("Kept".to_string()));
}

#[test]
fn every_color_has_its_own_code_and_an_unknown_one_reads_as_red() {
    let all = [
        ProfileColor::Red,
        ProfileColor::Orange,
        ProfileColor::Yellow,
        ProfileColor::Green,
        ProfileColor::Cyan,
        ProfileColor::Blue,
        ProfileColor::Purple,
        ProfileColor::Pink,
    ];
    for color in all {
        assert_eq!(ProfileColor::from_code(color.code()), color);
    }
    assert_eq!(all.iter().map(|c| c.code()).collect::<FxHashSet<_>>().len(), 8);
    assert_eq!(ProfileColor::from_code("chartreuse"), ProfileColor::Red);
}
