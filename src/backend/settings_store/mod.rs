//! The app's settings (`settings.json`): SSH profiles, the interface language, the
//! connection profiles, saved queries and what is set for cloud accounts and endpoints.
//! Passwords live in the OS credential store.

#[cfg(test)]
use super::LockExt;
use rust_i18n::t;
use rustc_hash::FxHashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::models::ProfileColor;
use crate::i18n::Language;
use super::error::AppError;
use super::persist::{self, Loaded};
use super::secret_vault::{OsVault, Secrets, Vault, VaultBacked};

mod cloud;
mod connection_profiles;
mod saved_queries;
mod ssh_profiles;

pub use cloud::{CloudScope, CloudSsh};
use cloud::{StoredCloud, cloud_entries};
use connection_profiles::{DEFAULT_PROFILE_ID, StoredConnectionProfile};
use saved_queries::StoredSavedQuery;
use ssh_profiles::StoredProfile;

/// The numbers of entries of a key the global search can be asked to read: the
/// fewer, the faster, and the less of a big key is searched.
pub const SEARCH_DEPTHS: [u32; 3] = [60, 200, 1000];

pub const DEFAULT_SEARCH_DEPTH: u32 = 1000;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSettings {
    #[serde(default)]
    ssh_profiles: Vec<StoredProfile>,
    // "" when no profile is active
    #[serde(default)]
    active_ssh_profile: String,
    // the interface language: "" follows the system, else a language code
    #[serde(default)]
    language: String,
    // whether the export buttons start on escaped JSON rather than plain JSON
    #[serde(default)]
    export_escaped: bool,
    // experimental: the global search filters inside Redis, with a script
    #[serde(default)]
    server_search: bool,
    // how many entries of each key the global search reads (0: not chosen yet)
    #[serde(default)]
    search_depth: u32,
    #[serde(default)]
    connection_profiles: Vec<StoredConnectionProfile>,
    // Redisql queries saved from a query tab, oldest first
    #[serde(default)]
    saved_queries: Vec<StoredSavedQuery>,
    // the default "Production" profile has been added: it is only ever added
    // once, so deleting it sticks
    #[serde(default)]
    connection_profiles_seeded: bool,
    // what is set for AWS accounts and their endpoints (none of the endpoints themselves)
    #[serde(default)]
    cloud: StoredCloud,
}

#[derive(Clone)]
pub struct SettingsStore {
    file_path: PathBuf,
    state: StoredSettings,
    vault: Arc<dyn Vault>,
    /// profiles whose password is in the vault and hasn't been read from it yet
    unloaded: FxHashSet<String>,
    /// cleared by the first failure to store a password: no more attempts for the run
    vault_usable: bool,
    /// set on a copy `staged` rehearses on: nothing is saved to the file
    dry_run: bool,
}

impl VaultBacked for SettingsStore {
    fn vault_mut(&mut self) -> &mut Arc<dyn Vault> {
        &mut self.vault
    }

    fn set_dry_run(&mut self, dry_run: bool) {
        self.dry_run = dry_run;
    }
}

impl SettingsStore {
    pub fn new(file_path: PathBuf) -> Self {
        Self::with_vault(file_path, Arc::new(OsVault))
    }

    fn with_vault(file_path: PathBuf, vault: Arc<dyn Vault>) -> Self {
        // a file that can't be read is kept aside, not overwritten by the defaults
        let state: StoredSettings = match persist::load(&file_path) {
            Loaded::Read(state) => state,
            Loaded::Missing => StoredSettings::default(),
            Loaded::Unusable(why) => {
                match persist::set_aside(&file_path) {
                    Some(kept) => eprintln!("{} can't be read ({why}); kept as {}", file_path.display(), kept.display()),
                    None => eprintln!("{} can't be read ({why}) and couldn't be moved aside", file_path.display()),
                }
                StoredSettings::default()
            }
        };
        let mut state = state;
        if !state.connection_profiles_seeded {
            state.connection_profiles_seeded = true;
            state.connection_profiles.push(StoredConnectionProfile {
                id: DEFAULT_PROFILE_ID.to_string(),
                name: "Production".to_string(),
                color: ProfileColor::Red.code().to_string(),
            });
        }
        let mut unloaded: FxHashSet<String> = state.ssh_profiles.iter().filter(|p| p.in_vault).map(|p| p.id.clone()).collect();
        unloaded.extend(cloud_entries(&mut state.cloud).filter(|(_, e)| e.ssh.as_ref().is_some_and(|s| s.in_vault)).map(|(key, _)| key));
        Self { file_path, state, vault, unloaded, vault_usable: true, dry_run: false }
    }

    pub fn language(&self) -> Language {
        Language::from_code(&self.state.language)
    }

    /// How many entries of each key the global search reads: one of
    /// [`SEARCH_DEPTHS`], [`DEFAULT_SEARCH_DEPTH`] until another is chosen.
    pub fn search_depth(&self) -> u32 {
        Some(self.state.search_depth).filter(|depth| SEARCH_DEPTHS.contains(depth)).unwrap_or(DEFAULT_SEARCH_DEPTH)
    }

    pub fn set_search_depth(&mut self, depth: u32) -> Result<(), AppError> {
        if !SEARCH_DEPTHS.contains(&depth) {
            return Err(AppError::invalid(t!("That search depth isn't one of the choices.")));
        }
        let mut next = self.state.clone();
        next.search_depth = depth;
        self.commit(next, false)
    }

    /// Whether the global search lets Redis filter with a script (experimental).
    pub fn server_search(&self) -> bool {
        self.state.server_search
    }

    pub fn set_server_search(&mut self, enabled: bool) -> Result<(), AppError> {
        let mut next = self.state.clone();
        next.server_search = enabled;
        self.commit(next, false)
    }

    pub fn export_escaped(&self) -> bool {
        self.state.export_escaped
    }

    pub fn set_export_escaped(&mut self, escaped: bool) -> Result<(), AppError> {
        let mut next = self.state.clone();
        next.export_escaped = escaped;
        self.commit(next, false)
    }

    pub fn set_language(&mut self, language: Language) -> Result<(), AppError> {
        let mut next = self.state.clone();
        next.language = language.code().to_string();
        self.commit(next, false)
    }

    /// Puts the passwords that changed (or aren't in the vault yet) into it and
    /// marks them as living there. One that can't be stored stays in the file; one
    /// that isn't loaded is left alone.
    fn vault_secrets(&mut self, next: &mut StoredSettings) {
        self.vault_cloud_secrets(next);
        self.vault_profile_secrets(next);
    }

    /// Stores `secrets` under `key` while the vault is usable: the first failure makes
    /// it unusable for the rest of the run. Whether they are stored.
    fn store_secret(&mut self, key: &str, secrets: &Secrets) -> bool {
        self.vault_usable = self.vault_usable && self.vault.set(key, secrets).is_ok();
        self.vault_usable
    }

    fn commit(&mut self, mut next: StoredSettings, store_secrets: bool) -> Result<(), AppError> {
        if store_secrets {
            self.vault_secrets(&mut next);
        }
        let mut on_disk = next.clone();
        for profile in on_disk.ssh_profiles.iter_mut().filter(|p| p.in_vault) {
            profile.password.clear();
        }
        for (_, entry) in cloud_entries(&mut on_disk.cloud) {
            if let Some(ssh) = entry.ssh.as_mut().filter(|s| s.in_vault) {
                ssh.password.clear();
                ssh.passphrase.clear();
            }
        }
        if !self.dry_run {
            persist::save(&self.file_path, &on_disk)?;
        }
        self.state = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::models::{ConnectionProfile, SavedQuery, SshSettings};
    use super::super::secret_vault::MemoryVault;
    use super::ssh_profiles::{SshProfileItem, vault_key};
    use super::*;
    use std::fs;
    use std::sync::atomic::Ordering;

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir()
            .join(format!("cachemire-settings-{name}-{}-{}", std::process::id(), super::super::now_ms()))
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

        assert_eq!(store.create_ssh_profile("", "u", "p").unwrap_err(), AppError::Invalid("The profile needs a name.".into()));
        assert_eq!(store.create_ssh_profile("n", " ", "p").unwrap_err(), AppError::Invalid("The profile needs an SSH username.".into()));
        assert_eq!(store.create_ssh_profile("n", "u", "").unwrap_err(), AppError::Invalid("The profile needs an SSH password.".into()));
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
        assert_eq!(store.create_connection_profile("  ", ProfileColor::Blue).unwrap_err(), AppError::Invalid("The profile needs a name.".into()));

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

        assert_eq!(store.create_saved_query("  ", "SELECT *", "").unwrap_err(), AppError::Invalid("The query needs a name.".into()));
        assert_eq!(store.create_saved_query("Untitled", "  ", "").unwrap_err(), AppError::Invalid("The query needs some text to save.".into()));

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
}

