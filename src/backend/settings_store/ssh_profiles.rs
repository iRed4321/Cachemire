//! SSH profiles: a username and password a tunnel with no username of its own
//! logs in with. Their passwords live in the credential store.

use rust_i18n::t;
use serde::{Deserialize, Serialize};

use super::{SettingsStore, StoredSettings};
use crate::backend::error::AppError;
use crate::backend::models::SshSettings;
use crate::backend::secret_vault::{Secrets, SecretsCleanup};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredProfile {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) username: String,
    // only while the password isn't in the credential store
    #[serde(default)]
    pub(super) password: String,
    #[serde(default)]
    pub(super) in_vault: bool,
}

/// What the settings window shows of a profile: never the password.
#[derive(Debug, Clone, PartialEq)]
pub struct SshProfileItem {
    pub id: String,
    pub name: String,
    pub username: String,
    pub active: bool,
}

/// The vault entry of a profile (connections use their own id, which is a
/// bare uuid, so the two can't meet).
pub(super) fn vault_key(id: &str) -> String {
    format!("ssh-profile:{id}")
}

impl SettingsStore {
    pub fn ssh_profiles(&self) -> Vec<SshProfileItem> {
        self.state
            .ssh_profiles
            .iter()
            .map(|p| SshProfileItem {
                id: p.id.clone(),
                name: p.name.clone(),
                username: p.username.clone(),
                active: p.id == self.state.active_ssh_profile,
            })
            .collect()
    }

    pub fn active_ssh_profile(&self) -> &str {
        &self.state.active_ssh_profile
    }

    /// Adds a profile; the first one added becomes the active one.
    #[cfg(test)]
    pub fn create_ssh_profile(&mut self, name: &str, username: &str, password: &str) -> Result<String, AppError> {
        self.create_ssh_profile_with_id(&uuid::Uuid::new_v4().to_string(), name, username, password)
    }

    /// `create_ssh_profile`, under an id chosen by the caller.
    pub fn create_ssh_profile_with_id(&mut self, id: &str, name: &str, username: &str, password: &str) -> Result<String, AppError> {
        let (name, username) = (name.trim(), username.trim());
        if name.is_empty() {
            return Err(AppError::invalid(t!("The profile needs a name.")));
        }
        if username.is_empty() {
            return Err(AppError::invalid(t!("The profile needs an SSH username.")));
        }
        if password.is_empty() {
            return Err(AppError::invalid(t!("The profile needs an SSH password.")));
        }
        let mut next = self.state.clone();
        let id = id.to_string();
        next.ssh_profiles.push(StoredProfile {
            id: id.clone(),
            name: name.to_string(),
            username: username.to_string(),
            password: password.to_string(),
            in_vault: false,
        });
        if next.active_ssh_profile.is_empty() {
            next.active_ssh_profile.clone_from(&id);
        }
        self.commit(next, true)?;
        Ok(id)
    }

    /// Changes a profile; `password` is `None` to keep the one it has.
    pub fn update_ssh_profile(&mut self, id: &str, name: &str, username: &str, password: Option<&str>) -> Result<(), AppError> {
        let (name, username) = (name.trim(), username.trim());
        if name.is_empty() {
            return Err(AppError::invalid(t!("The profile needs a name.")));
        }
        if username.is_empty() {
            return Err(AppError::invalid(t!("The profile needs an SSH username.")));
        }
        let mut next = self.state.clone();
        let profile = next.ssh_profiles.iter_mut().find(|p| p.id == id).ok_or_else(|| AppError::not_found(t!("SSH profile not found.")))?;
        profile.name = name.to_string();
        profile.username = username.to_string();
        let new_password = password.filter(|p| !p.is_empty());
        if let Some(password) = new_password {
            profile.password = password.to_string();
        }
        self.commit(next, true)?;
        if new_password.is_some() {
            // the new password is what's in memory now
            self.unloaded.remove(id);
        }
        Ok(())
    }

    /// Removes a profile (and stops it being the active one). What it had in the
    /// credential store is left for the caller to clear, off the UI thread.
    pub fn remove_ssh_profile(&mut self, id: &str) -> Result<Option<SecretsCleanup>, AppError> {
        let stored = self.state.ssh_profiles.iter().find(|p| p.id == id).ok_or_else(|| AppError::not_found(t!("SSH profile not found.")))?;
        let cleanup = stored.in_vault.then(|| SecretsCleanup::new(self.vault.clone(), &vault_key(id)));
        let mut next = self.state.clone();
        next.ssh_profiles.retain(|p| p.id != id);
        if next.active_ssh_profile == id {
            next.active_ssh_profile.clear();
        }
        self.commit(next, false)?;
        self.unloaded.remove(id);
        Ok(cleanup)
    }

    /// Makes `id` the active profile; "" leaves none active.
    pub fn set_active_ssh_profile(&mut self, id: &str) -> Result<(), AppError> {
        if !id.is_empty() && !self.state.ssh_profiles.iter().any(|p| p.id == id) {
            return Err(AppError::not_found(t!("SSH profile not found.")));
        }
        let mut next = self.state.clone();
        next.active_ssh_profile = id.to_string();
        self.commit(next, false)
    }

    /// Makes the active profile's password available, ahead of a connection
    /// using it: reads it from the credential store, or, when the file still
    /// holds it, moves it there. Costs nothing when no profile is active.
    pub fn unlock_active_ssh_profile(&mut self) -> Result<(), AppError> {
        let id = self.state.active_ssh_profile.clone();
        let Some(position) = self.state.ssh_profiles.iter().position(|p| p.id == id) else { return Ok(()) };
        if self.unloaded.contains(&id) {
            let stored = self
                .vault
                .get(&vault_key(&id))
                .map_err(|e| AppError::Keyring(t!("Couldn't read the SSH profile's password from the system keyring: %{error}", error = e).into_owned()))?;
            let profile = &mut self.state.ssh_profiles[position];
            match stored {
                Some(secrets) => profile.password = secrets.ssh_password,
                // the entry was removed behind our back
                None => profile.in_vault = false,
            }
            self.unloaded.remove(&id);
        } else {
            let profile = &self.state.ssh_profiles[position];
            if !profile.in_vault && !profile.password.is_empty() && self.vault_usable {
                return self.commit(self.state.clone(), true);
            }
        }
        Ok(())
    }

    /// Completes an SSH tunnel's settings with the active profile when the
    /// connection has no username of its own. (A connection that has one uses
    /// its own username and password, whatever the profile is.)
    pub fn fill_ssh(&self, ssh: &mut SshSettings) -> Result<(), AppError> {
        if !ssh.username.is_empty() {
            return Ok(());
        }
        let profile = self
            .state
            .ssh_profiles
            .iter()
            .find(|p| p.id == self.state.active_ssh_profile)
            .ok_or_else(|| AppError::invalid(t!("This SSH connection has no username or password: fill them in, or make an SSH profile active in Settings.")))?;
        ssh.username.clone_from(&profile.username);
        if ssh.password.is_empty() {
            ssh.password.clone_from(&profile.password);
        }
        Ok(())
    }

    /// Vaults the SSH profiles' passwords that changed (see `vault_secrets`).
    pub(super) fn vault_profile_secrets(&mut self, next: &mut StoredSettings) {
        for profile in &mut next.ssh_profiles {
            if self.unloaded.contains(&profile.id) {
                continue;
            }
            if profile.password.is_empty() {
                profile.in_vault = false;
                continue;
            }
            let unchanged = profile.in_vault
                && self.state.ssh_profiles.iter().any(|p| p.id == profile.id && p.in_vault && p.password == profile.password);
            if unchanged {
                continue;
            }
            profile.in_vault = self.store_secret(&vault_key(&profile.id), &Secrets { ssh_password: profile.password.clone(), ..Default::default() });
        }
    }
}
