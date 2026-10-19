//! The Redisql queries saved from a query tab, for one connection or for all.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::SettingsStore;
use crate::error::{Error, Invalid, NotFound};

/// A Redisql query saved from a query tab, under a name the user gave it.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedQuery {
    pub id: String,
    pub name: String,
    pub text: String,
    /// the saved connection it belongs to; empty for a global query
    pub connection_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct StoredSavedQuery {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) text: String,
    #[serde(default)]
    pub(super) connection_id: String,
}

pub(super) fn to_saved_query(stored: &StoredSavedQuery) -> SavedQuery {
    SavedQuery { id: stored.id.clone(), name: stored.name.clone(), text: stored.text.clone(), connection_id: stored.connection_id.clone() }
}

impl SettingsStore {
    pub fn saved_queries(&self) -> Vec<SavedQuery> {
        self.state.saved_queries.iter().map(to_saved_query).collect()
    }

    pub fn saved_query(&self, id: &str) -> Option<SavedQuery> {
        self.state.saved_queries.iter().find(|q| q.id == id).map(to_saved_query)
    }

    /// Saves `text` as a new query named `name`, for `connection_id` (empty: global).
    pub fn create_saved_query(&mut self, name: &str, text: &str, connection_id: &str) -> Result<String, Error> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Invalid::QueryName.into());
        }
        if text.trim().is_empty() {
            return Err(Invalid::QueryText.into());
        }
        let mut next = self.state.clone();
        let id = Uuid::new_v4().to_string();
        next.saved_queries.push(StoredSavedQuery {
            id: id.clone(),
            name: name.to_string(),
            text: text.to_string(),
            connection_id: connection_id.to_string(),
        });
        self.commit(next, false)?;
        Ok(id)
    }

    pub fn remove_saved_query(&mut self, id: &str) -> Result<(), Error> {
        if !self.state.saved_queries.iter().any(|q| q.id == id) {
            return Err(NotFound::SavedQuery.into());
        }
        let mut next = self.state.clone();
        next.saved_queries.retain(|q| q.id != id);
        self.commit(next, false)
    }

    /// Removes every query saved for connection `connection_id`.
    pub fn remove_saved_queries_of(&mut self, connection_id: &str) -> Result<(), Error> {
        let mut next = self.state.clone();
        next.saved_queries.retain(|q| q.connection_id != connection_id);
        self.commit(next, false)
    }
}
