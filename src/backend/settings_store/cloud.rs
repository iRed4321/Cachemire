//! What is set for cloud accounts and their endpoints: a connection profile and an
//! SSH tunnel, an endpoint's own settings winning over its account's.

use rustc_hash::{FxHashMap, FxHashSet};
use std::collections::BTreeMap;

use rust_i18n::t;
use serde::{Deserialize, Serialize};

use super::{SettingsStore, StoredSettings};
use crate::backend::error::AppError;
use crate::backend::models::{CloudKey, ConnectionItem, SshSettings};
use crate::backend::secret_vault::{Secrets, SecretsCleanup};

/// An SSH tunnel set for cloud endpoints, as `settings.json` keeps it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudSsh {
    /// false: no tunnel at all (what an endpoint says to ignore its account's)
    pub enabled: bool,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub private_key: String,
    // only while they aren't in the credential store
    #[serde(default)]
    pub password: String,
    #[serde(default)]
    pub passphrase: String,
    #[serde(default)]
    pub in_vault: bool,
}

impl CloudSsh {
    /// The tunnel that turns one off.
    pub fn none() -> Self {
        Self { enabled: false, host: String::new(), port: 22, username: String::new(), private_key: String::new(), password: String::new(), passphrase: String::new(), in_vault: false }
    }
}

/// What is set for one cloud account or endpoint: each setting is left out to inherit
/// (the account's, for an endpoint), or says what to use, "" or a disabled tunnel meaning none.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudEntry {
    /// what the account or endpoint is called, for display only
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connection_profile: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ssh: Option<CloudSsh>,
    /// listings this endpoint was missing from, in a row
    #[serde(default, skip_serializing_if = "is_zero")]
    pub missed: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

impl CloudEntry {
    fn is_empty(&self) -> bool {
        self.connection_profile.is_none() && self.ssh.is_none()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredCloud {
    // by account id
    #[serde(default)]
    pub(super) accounts: BTreeMap<String, CloudEntry>,
    // by `account/region/name`
    #[serde(default)]
    pub(super) endpoints: BTreeMap<String, CloudEntry>,
}

/// Which cloud settings are meant: an account's, or one endpoint's.
#[derive(Debug, Clone, PartialEq)]
pub enum CloudScope {
    Account(String),
    Endpoint(CloudKey),
}

/// An endpoint that is missing from this many listings in a row loses its settings.
pub(super) const MISSES_BEFORE_PRUNING: u32 = 3;

/// The vault entry of a cloud account or endpoint's tunnel secrets.
pub(super) fn cloud_vault_key(scope: &CloudScope) -> String {
    match scope {
        CloudScope::Account(account) => format!("cloud-account:{account}"),
        CloudScope::Endpoint(key) => format!("cloud-endpoint:{}", key.endpoint_id()),
    }
}

/// Every entry with its vault key.
pub(super) fn cloud_entries(cloud: &mut StoredCloud) -> impl Iterator<Item = (String, &mut CloudEntry)> {
    let accounts = cloud.accounts.iter_mut().map(|(id, entry)| (format!("cloud-account:{id}"), entry));
    let endpoints = cloud.endpoints.iter_mut().map(|(id, entry)| (format!("cloud-endpoint:{id}"), entry));
    accounts.chain(endpoints)
}

impl SettingsStore {
    pub(super) fn cloud_entry(&self, scope: &CloudScope) -> Option<&CloudEntry> {
        match scope {
            CloudScope::Account(account) => self.state.cloud.accounts.get(account),
            CloudScope::Endpoint(key) => self.state.cloud.endpoints.get(&key.endpoint_id()),
        }
    }

    pub(super) fn cloud_entry_mut<'a>(cloud: &'a mut StoredCloud, scope: &CloudScope) -> Option<&'a mut CloudEntry> {
        match scope {
            CloudScope::Account(account) => cloud.accounts.get_mut(account),
            CloudScope::Endpoint(key) => cloud.endpoints.get_mut(&key.endpoint_id()),
        }
    }

    /// What is set for `scope`, without its secrets.
    pub fn cloud_settings(&self, scope: &CloudScope) -> CloudEntry {
        let mut entry = self.cloud_entry(scope).cloned().unwrap_or_default();
        if let Some(ssh) = entry.ssh.as_mut() {
            ssh.password.clear();
            ssh.passphrase.clear();
        }
        entry
    }

    /// The connection profile and tunnel an endpoint ends up with: its own setting,
    /// else its account's. The tunnel comes with the scope that holds it.
    pub(super) fn cloud_effective(&self, key: &CloudKey) -> (String, Option<(CloudScope, &CloudSsh)>) {
        let endpoint = self.state.cloud.endpoints.get(&key.endpoint_id());
        let account = self.state.cloud.accounts.get(&key.account);
        let profile = endpoint.and_then(|e| e.connection_profile.clone()).or_else(|| account.and_then(|a| a.connection_profile.clone())).unwrap_or_default();
        // a profile that no longer exists counts as none
        let profile = if self.state.connection_profiles.iter().any(|p| p.id == profile) { profile } else { String::new() };
        let ssh = match (endpoint.and_then(|e| e.ssh.as_ref()), account.and_then(|a| a.ssh.as_ref())) {
            (Some(ssh), _) => Some((CloudScope::Endpoint(key.clone()), ssh)),
            (None, Some(ssh)) => Some((CloudScope::Account(key.account.clone()), ssh)),
            (None, None) => None,
        };
        (profile, ssh.filter(|(_, ssh)| ssh.enabled))
    }

    /// Gives each of `items`, listed from `account` and `region`, the connection profile and
    /// tunnel set for it (without the secrets: see `fill_cloud_secrets`).
    pub fn apply_cloud(&self, account: &str, region: &str, items: &mut [ConnectionItem]) {
        for item in items {
            let key = CloudKey { account: account.to_string(), region: region.to_string(), name: item.name.clone() };
            let (profile, ssh) = self.cloud_effective(&key);
            item.profile_id = profile;
            item.ssh = ssh.map(|(_, ssh)| SshSettings {
                host: ssh.host.clone(),
                port: ssh.port,
                username: ssh.username.clone(),
                password: String::new(),
                private_key: ssh.private_key.clone(),
                passphrase: String::new(),
            });
        }
    }

    /// Adds the tunnel secrets that `key`'s tunnel has (see `unlock_cloud`).
    pub fn fill_cloud_secrets(&self, key: &CloudKey, ssh: &mut SshSettings) {
        if let Some((_, stored)) = self.cloud_effective(key).1 {
            ssh.password.clone_from(&stored.password);
            ssh.passphrase.clone_from(&stored.passphrase);
        }
    }

    /// Reads the tunnel secrets of `scope` from the credential store, if they are there
    /// and haven't been read yet.
    pub(super) fn unlock_cloud_scope(&mut self, scope: &CloudScope) -> Result<(), AppError> {
        let vault_key = cloud_vault_key(scope);
        if !self.unloaded.contains(&vault_key) {
            return Ok(());
        }
        let stored = self
            .vault
            .get(&vault_key)
            .map_err(|e| AppError::Keyring(t!("Couldn't read the SSH tunnel's secrets from the system keyring: %{error}", error = e).into_owned()))?;
        if let Some(ssh) = Self::cloud_entry_mut(&mut self.state.cloud, scope).and_then(|entry| entry.ssh.as_mut()) {
            match stored {
                Some(secrets) => {
                    ssh.password = secrets.ssh_password;
                    ssh.passphrase = secrets.ssh_passphrase;
                }
                // the entry was removed behind our back
                None => ssh.in_vault = false,
            }
        }
        self.unloaded.remove(&vault_key);
        Ok(())
    }

    /// Makes the tunnel secrets that apply to `key` available, ahead of a connection.
    pub fn unlock_cloud(&mut self, key: &CloudKey) -> Result<(), AppError> {
        self.unlock_cloud_scope(&CloudScope::Endpoint(key.clone()))?;
        self.unlock_cloud_scope(&CloudScope::Account(key.account.clone()))
    }

    /// Sets what is set for `scope`: `profile` is `None` to inherit, "" for none; `ssh` is `None`
    /// to inherit. A blank password or passphrase keeps the stored one while the tunnel stays on.
    /// What it leaves in the credential store is for the caller to clear (the returned cleanup).
    pub fn set_cloud_scope(&mut self, scope: &CloudScope, label: &str, profile: Option<String>, ssh: Option<CloudSsh>) -> Result<Option<SecretsCleanup>, AppError> {
        if profile.as_ref().is_some_and(|id| !id.is_empty() && !self.state.connection_profiles.iter().any(|p| &p.id == id)) {
            return Err(AppError::not_found(t!("Connection profile not found.")));
        }
        if let Some(ssh) = ssh.as_ref().filter(|ssh| ssh.enabled) {
            if ssh.host.trim().is_empty() {
                return Err(AppError::invalid(t!("SSH host is required.")));
            }
            if ssh.port == 0 {
                return Err(AppError::invalid(t!("SSH port must be a number between 1 and 65535.")));
            }
        }
        // the secrets are kept in one entry: changing one needs the other
        self.unlock_cloud_scope(scope)?;
        let vault_key = cloud_vault_key(scope);
        let old = self.cloud_entry(scope).cloned().unwrap_or_default();
        let was_in_vault = old.ssh.as_ref().is_some_and(|s| s.in_vault);

        let mut ssh = ssh;
        if let (Some(new), Some(old)) = (ssh.as_mut(), old.ssh.as_ref().filter(|s| s.enabled)) {
            if new.enabled {
                if new.password.is_empty() {
                    new.password.clone_from(&old.password);
                }
                if new.passphrase.is_empty() {
                    new.passphrase.clone_from(&old.passphrase);
                }
            }
            new.in_vault = old.in_vault;
        }

        let mut next = self.state.clone();
        let map = match scope {
            CloudScope::Account(_) => &mut next.cloud.accounts,
            CloudScope::Endpoint(_) => &mut next.cloud.endpoints,
        };
        let id = match scope {
            CloudScope::Account(account) => account.clone(),
            CloudScope::Endpoint(key) => key.endpoint_id(),
        };
        let entry = CloudEntry { label: if label.is_empty() { old.label } else { label.to_string() }, connection_profile: profile, ssh, missed: 0 };
        if entry.is_empty() {
            map.remove(&id);
        } else {
            map.insert(id, entry);
        }
        self.commit(next, true)?;
        self.unloaded.remove(&vault_key);
        let now_in_vault = self.cloud_entry(scope).and_then(|e| e.ssh.as_ref()).is_some_and(|s| s.in_vault);
        Ok((was_in_vault && !now_in_vault).then(|| SecretsCleanup::new(self.vault.clone(), &vault_key)))
    }

    /// Counts the endpoints of `account` and `region` that this complete listing didn't
    /// have: one missing from the third in a row loses its settings. The clean-ups of what
    /// it had in the credential store are returned.
    pub fn observe_cloud_listing(&mut self, account: &str, region: &str, present: &FxHashSet<String>) -> Result<Vec<SecretsCleanup>, AppError> {
        let prefix = format!("{account}/{region}/");
        let mut next = self.state.clone();
        let (mut changed, mut cleanups, mut pruned) = (false, Vec::new(), Vec::new());
        next.cloud.endpoints.retain(|id, entry| {
            let Some(name) = id.strip_prefix(&prefix) else { return true };
            if present.contains(name) {
                changed |= entry.missed != 0;
                entry.missed = 0;
                return true;
            }
            entry.missed += 1;
            changed = true;
            if entry.missed < MISSES_BEFORE_PRUNING {
                return true;
            }
            let vault_key = format!("cloud-endpoint:{id}");
            if entry.ssh.as_ref().is_some_and(|s| s.in_vault) {
                cleanups.push(SecretsCleanup::new(self.vault.clone(), &vault_key));
            }
            pruned.push(vault_key);
            false
        });
        if changed {
            self.commit(next, false)?;
            self.unloaded.retain(|key| !pruned.contains(key));
        }
        Ok(cleanups)
    }

    /// Vaults the cloud tunnels' secrets that changed (see `vault_secrets`).
    pub(super) fn vault_cloud_secrets(&mut self, next: &mut StoredSettings) {
        let mut before: FxHashMap<String, (String, String, bool)> = FxHashMap::default();
        for (key, entry) in cloud_entries(&mut self.state.cloud.clone()) {
            if let Some(ssh) = entry.ssh.as_ref() {
                before.insert(key, (ssh.password.clone(), ssh.passphrase.clone(), ssh.in_vault));
            }
        }
        for (key, entry) in cloud_entries(&mut next.cloud) {
            let Some(ssh) = entry.ssh.as_mut() else { continue };
            if self.unloaded.contains(&key) {
                continue;
            }
            if !ssh.enabled || (ssh.password.is_empty() && ssh.passphrase.is_empty()) {
                ssh.in_vault = false;
                continue;
            }
            let unchanged = ssh.in_vault && before.get(&key).is_some_and(|(password, passphrase, in_vault)| *in_vault && *password == ssh.password && *passphrase == ssh.passphrase);
            if unchanged {
                continue;
            }
            let secrets = Secrets { ssh_password: ssh.password.clone(), ssh_passphrase: ssh.passphrase.clone(), ..Default::default() };
            ssh.in_vault = self.store_secret(&key, &secrets);
        }
    }
}
