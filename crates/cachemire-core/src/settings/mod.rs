//! The app's settings (`settings.json`): SSH profiles, the interface language, the
//! connection profiles, saved queries and what is set for cloud accounts and endpoints.
//! Passwords live in the OS credential store.

#[cfg(test)]
use crate::LockExt;
use rustc_hash::FxHashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::i18n::Language;
use crate::error::{Error, Invalid};
use crate::persist::{self, Loaded};
use crate::secret_vault::{OsVault, Secrets, Vault, VaultBacked};
use crate::updates::Channel;

mod cloud;
mod connection_profiles;
mod saved_queries;
mod ssh_profiles;

pub use cloud::{CloudScope, CloudSsh};
pub use connection_profiles::{ConnectionProfile, ProfileColor};
pub use saved_queries::SavedQuery;
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
    // the release channel the update check follows ("" until one is picked: the running build's)
    #[serde(default)]
    update_channel: String,
    // whether the app looks for an update on that channel each time it starts
    #[serde(default)]
    check_updates_at_startup: bool,
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

    /// A store whose secrets stay in memory, for the tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory(file_path: PathBuf) -> Self {
        Self::with_vault(file_path, Arc::new(crate::secret_vault::MemoryVault::default()))
    }

    fn with_vault(file_path: PathBuf, vault: Arc<dyn Vault>) -> Self {
        // a file that can't be read is kept aside, not overwritten by the defaults
        let state: StoredSettings = match persist::load(&file_path) {
            Loaded::Read(state) => state,
            Loaded::Missing => StoredSettings::default(),
            Loaded::Unusable(why) => {
                match persist::set_aside(&file_path) {
                    Some(kept) => tracing::error!("{} can't be read ({why}); kept as {}", file_path.display(), kept.display()),
                    None => tracing::error!("{} can't be read ({why}) and couldn't be moved aside", file_path.display()),
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

    pub fn set_search_depth(&mut self, depth: u32) -> Result<(), Error> {
        if !SEARCH_DEPTHS.contains(&depth) {
            return Err(Invalid::SearchDepth.into());
        }
        let mut next = self.state.clone();
        next.search_depth = depth;
        self.commit(next, false)
    }

    /// Whether the global search lets Redis filter with a script (experimental).
    pub fn server_search(&self) -> bool {
        self.state.server_search
    }

    pub fn set_server_search(&mut self, enabled: bool) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.server_search = enabled;
        self.commit(next, false)
    }

    /// The release channel picked for the update check, if one was.
    pub fn update_channel(&self) -> Option<Channel> {
        Channel::from_code(&self.state.update_channel)
    }

    pub fn set_update_channel(&mut self, channel: Channel) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.update_channel = channel.code().to_string();
        self.commit(next, false)
    }

    /// Whether the app looks for an update each time it starts.
    pub fn check_updates_at_startup(&self) -> bool {
        self.state.check_updates_at_startup
    }

    pub fn set_check_updates_at_startup(&mut self, enabled: bool) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.check_updates_at_startup = enabled;
        self.commit(next, false)
    }

    pub fn export_escaped(&self) -> bool {
        self.state.export_escaped
    }

    pub fn set_export_escaped(&mut self, escaped: bool) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.export_escaped = escaped;
        self.commit(next, false)
    }

    pub fn set_language(&mut self, language: Language) -> Result<(), Error> {
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

    fn commit(&mut self, mut next: StoredSettings, store_secrets: bool) -> Result<(), Error> {
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
mod tests;
