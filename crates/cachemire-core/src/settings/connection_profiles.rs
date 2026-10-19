//! Connection profiles: a name and a tint the app's background takes while a
//! connection using one is the one in use.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::SettingsStore;
use crate::error::{Error, Invalid, NotFound};

/// The tints a connection profile can take: a fixed set, so every one has been
/// checked against the app's background.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ProfileColor {
    #[default]
    Red,
    Orange,
    Yellow,
    Green,
    Cyan,
    Blue,
    Purple,
    Pink,
}

impl ProfileColor {
    /// How a color is saved.
    pub fn code(self) -> &'static str {
        match self {
            ProfileColor::Red => "red",
            ProfileColor::Orange => "orange",
            ProfileColor::Yellow => "yellow",
            ProfileColor::Green => "green",
            ProfileColor::Cyan => "cyan",
            ProfileColor::Blue => "blue",
            ProfileColor::Purple => "purple",
            ProfileColor::Pink => "pink",
        }
    }

    /// The color a saved code stands for; red for one we don't know (a file
    /// from a newer version, say).
    pub fn from_code(code: &str) -> ProfileColor {
        [
            ProfileColor::Red,
            ProfileColor::Orange,
            ProfileColor::Yellow,
            ProfileColor::Green,
            ProfileColor::Cyan,
            ProfileColor::Blue,
            ProfileColor::Purple,
            ProfileColor::Pink,
        ]
        .into_iter()
        .find(|color| color.code() == code)
        .unwrap_or_default()
    }
}

/// A connection profile: a name ("dev", "Production"...) and the tint the app's
/// background takes while a connection using it is the one in use.
#[derive(Debug, Clone, PartialEq)]
pub struct ConnectionProfile {
    pub id: String,
    pub name: String,
    pub color: ProfileColor,
}

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

    pub fn create_connection_profile(&mut self, name: &str, color: ProfileColor) -> Result<String, Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Invalid::ProfileName.into());
        }
        let mut next = self.state.clone();
        let id = Uuid::new_v4().to_string();
        next.connection_profiles.push(StoredConnectionProfile { id: id.clone(), name: name.to_string(), color: color.code().to_string() });
        self.commit(next, false)?;
        Ok(id)
    }

    pub fn update_connection_profile(&mut self, id: &str, name: &str, color: ProfileColor) -> Result<(), Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Invalid::ProfileName.into());
        }
        let mut next = self.state.clone();
        let profile = next.connection_profiles.iter_mut().find(|p| p.id == id).ok_or(NotFound::ConnectionProfile)?;
        profile.name = name.to_string();
        profile.color = color.code().to_string();
        self.commit(next, false)
    }

    /// Removes a profile. Whether a connection still uses it is for the caller
    /// to check first (the connections are not kept here).
    pub fn remove_connection_profile(&mut self, id: &str) -> Result<(), Error> {
        if !self.state.connection_profiles.iter().any(|p| p.id == id) {
            return Err(NotFound::ConnectionProfile.into());
        }
        let mut next = self.state.clone();
        next.connection_profiles.retain(|p| p.id != id);
        self.commit(next, false)
    }
}
