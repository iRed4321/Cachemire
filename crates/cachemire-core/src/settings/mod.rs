//! The app's settings (`settings.json`): SSH profiles, the interface language, the
//! connection profiles, saved queries and what is set for cloud accounts and endpoints.
//! Passwords live in the OS credential store.

use rustc_hash::FxHashSet;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Invalid};
use crate::i18n::Language;
use crate::persist;
use crate::secret_vault::{OsVault, SecretsPolicy, Vault, VaultBacked, Vaulting};
use crate::updates::Channel;

mod cloud;
mod connection_profiles;
mod saved_queries;
mod ssh_profiles;

pub use cloud::{CloudScope, CloudSsh};
use cloud::{StoredCloud, cloud_entries};
pub use connection_profiles::{ConnectionProfile, ProfileColor};
use connection_profiles::{DEFAULT_PROFILE_ID, StoredConnectionProfile};
pub use saved_queries::SavedQuery;
use saved_queries::StoredSavedQuery;
use ssh_profiles::StoredProfile;

/// The numbers of entries of a key the global search can be asked to read: the
/// fewer, the faster, and the less of a big key is searched.
pub const SEARCH_DEPTHS: [u32; 3] = [60, 200, 1000];

pub const DEFAULT_SEARCH_DEPTH: u32 = 1000;

/// The view a key's new tab starts on: the fields, the table, or the one last picked.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpeningView {
    Table,
    Last,
    // also what a value this version doesn't know reads as
    #[default]
    #[serde(other)]
    Fields,
}

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
    // the view a key's new tab starts on
    #[serde(default)]
    opening_view: OpeningView,
    // whether the view last picked in a key's tab is the table
    #[serde(default)]
    last_view_table: bool,
    // the release channel the update check follows ("" until one is picked: the running build's)
    #[serde(default)]
    update_channel: String,
    // whether the app looks for an update on that channel each time it starts
    #[serde(default)]
    check_updates_at_startup: bool,
    // whether the app reopens the last connection with its tabs and open folders
    #[serde(default)]
    restore_session: bool,
    // experimental: the global search filters inside Redis, with a script
    #[serde(default)]
    server_search: bool,
    // how many entries of each key the global search reads (0: not chosen yet)
    #[serde(default)]
    search_depth: u32,
    // the keyboard shortcuts changed from their defaults: an action's name, and its keys
    #[serde(default)]
    shortcuts: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    connection_profiles: Vec<StoredConnectionProfile>,
    // Redisql queries saved from a query tab, oldest first
    #[serde(default)]
    saved_queries: Vec<StoredSavedQuery>,
    // the default "Production" profile has been added: it is only ever added
    // once, so deleting it sticks
    #[serde(default)]
    connection_profiles_seeded: bool,
    // the default "Production" profile has been made read-only: done once, so
    // unticking it sticks
    #[serde(default)]
    default_profile_read_only_set: bool,
    // what is set for AWS accounts and their endpoints (none of the endpoints themselves)
    #[serde(default)]
    cloud: StoredCloud,
    // what a newer version wrote that this one doesn't know: saved back as it was
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
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
        // a file that can't be read is kept, never replaced by the defaults
        let (stored, writable) = persist::load_store::<StoredSettings>(&file_path);
        let mut state = stored.unwrap_or_default();
        if !state.connection_profiles_seeded {
            state.connection_profiles_seeded = true;
            state.connection_profiles.push(StoredConnectionProfile {
                id: DEFAULT_PROFILE_ID.to_string(),
                name: "Production".to_string(),
                color: ProfileColor::Red.code().to_string(),
                read_only: true,
            });
        }
        if !state.default_profile_read_only_set {
            state.default_profile_read_only_set = true;
            if let Some(profile) = state.connection_profiles.iter_mut().find(|p| p.id == DEFAULT_PROFILE_ID) {
                profile.read_only = true;
            }
        }
        let mut unloaded: FxHashSet<String> = state.ssh_profiles.iter().filter(|p| p.in_vault).map(|p| p.id.clone()).collect();
        unloaded.extend(cloud_entries(&mut state.cloud).filter(|(_, e)| e.ssh.as_ref().is_some_and(|s| s.in_vault)).map(|(key, _)| key));
        Self { file_path, state, vault, unloaded, vault_usable: true, dry_run: !writable }
    }

    /// Where the settings are saved: the app's other files sit next to it.
    pub fn file_path(&self) -> &Path {
        &self.file_path
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

    /// Whether the app reopens, at start, the connection it was closed on, with its tabs and open folders.
    pub fn restore_session(&self) -> bool {
        self.state.restore_session
    }

    pub fn set_restore_session(&mut self, enabled: bool) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.restore_session = enabled;
        self.commit(next, false)
    }

    /// The keys picked for `action` in place of its default ones, if any: the
    /// parts of a key combination (modifiers, then the key), as the UI wrote them.
    pub fn shortcut(&self, action: &str) -> Option<&[String]> {
        self.state.shortcuts.get(action).map(Vec::as_slice)
    }

    /// Binds `action` to `keys`, or back to its default keys with `None`.
    pub fn set_shortcut(&mut self, action: &str, keys: Option<Vec<String>>) -> Result<(), Error> {
        let mut next = self.state.clone();
        match keys {
            Some(keys) => next.shortcuts.insert(action.to_string(), keys),
            None => next.shortcuts.remove(action),
        };
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

    pub fn opening_view(&self) -> OpeningView {
        self.state.opening_view
    }

    pub fn set_opening_view(&mut self, view: OpeningView) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.opening_view = view;
        self.commit(next, false)
    }

    pub fn last_view_table(&self) -> bool {
        self.state.last_view_table
    }

    /// Keeps whether the view last picked is the table; nothing is saved when it is unchanged.
    pub fn set_last_view_table(&mut self, table: bool) -> Result<(), Error> {
        if self.state.last_view_table == table {
            return Ok(());
        }
        let mut next = self.state.clone();
        next.last_view_table = table;
        self.commit(next, false)
    }

    pub fn set_language(&mut self, language: Language) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.language = language.code().to_string();
        self.commit(next, false)
    }

    /// Puts the passwords that changed (or aren't in the vault yet) into it and
    /// marks them as living there. The one `vaulting` names is stored as `policy` says;
    /// another that can't be stored stays in the file. One that isn't loaded is left alone.
    fn vault_secrets(&mut self, next: &mut StoredSettings, vaulting: Vaulting<'_>, policy: SecretsPolicy) -> Result<(), Error> {
        self.vault_cloud_secrets(next, vaulting, policy)?;
        self.vault_profile_secrets(next, vaulting, policy)
    }

    fn commit(&mut self, next: StoredSettings, store_secrets: bool) -> Result<(), Error> {
        self.commit_with(next, if store_secrets { Vaulting::Quietly } else { Vaulting::Untouched }, SecretsPolicy::PlainText)
    }

    fn commit_with(&mut self, mut next: StoredSettings, vaulting: Vaulting<'_>, policy: SecretsPolicy) -> Result<(), Error> {
        if vaulting != Vaulting::Untouched {
            self.vault_secrets(&mut next, vaulting, policy)?;
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
