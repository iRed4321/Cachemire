//! Connection profiles: a name and a tint the app's background takes while a
//! connection using one is the one in use.

use rust_i18n::t;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::SettingsStore;
use crate::backend::error::AppError;
use crate::backend::models::{ConnectionProfile, ProfileColor};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredConnectionProfile {
    pub(super) id: String,
    pub(super) name: String,
    // a `ProfileColor` code
    pub(super) color: String,
}

/// The id of the profile a first run starts with (fixed, so a connection can
/// refer to it before anything has been saved).
pub(super) const DEFAULT_PROFILE_ID: &str = "production";

pub(super) fn to_profile(stored: &StoredConnectionProfile) -> ConnectionProfile {
    ConnectionProfile { id: stored.id.clone(), name: stored.name.clone(), color: ProfileColor::from_code(&stored.color) }
}

impl SettingsStore {
    pub fn connection_profiles(&self) -> Vec<ConnectionProfile> {
        self.state.connection_profiles.iter().map(to_profile).collect()
    }

    pub fn connection_profile(&self, id: &str) -> Option<ConnectionProfile> {
        self.state.connection_profiles.iter().find(|p| p.id == id).map(to_profile)
    }

    pub fn create_connection_profile(&mut self, name: &str, color: ProfileColor) -> Result<String, AppError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::invalid(t!("The profile needs a name.")));
        }
        let mut next = self.state.clone();
        let id = Uuid::new_v4().to_string();
        next.connection_profiles.push(StoredConnectionProfile { id: id.clone(), name: name.to_string(), color: color.code().to_string() });
        self.commit(next, false)?;
        Ok(id)
    }

    pub fn update_connection_profile(&mut self, id: &str, name: &str, color: ProfileColor) -> Result<(), AppError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::invalid(t!("The profile needs a name.")));
        }
        let mut next = self.state.clone();
        let profile = next.connection_profiles.iter_mut().find(|p| p.id == id).ok_or_else(|| AppError::not_found(t!("Connection profile not found.")))?;
        profile.name = name.to_string();
        profile.color = color.code().to_string();
        self.commit(next, false)
    }

    /// Removes a profile. Whether a connection still uses it is for the caller
    /// to check first (the connections are not kept here).
    pub fn remove_connection_profile(&mut self, id: &str) -> Result<(), AppError> {
        if !self.state.connection_profiles.iter().any(|p| p.id == id) {
            return Err(AppError::not_found(t!("Connection profile not found.")));
        }
        let mut next = self.state.clone();
        next.connection_profiles.retain(|p| p.id != id);
        self.commit(next, false)
    }
}
