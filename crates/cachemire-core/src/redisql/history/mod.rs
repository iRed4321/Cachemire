//! The queries run from the query tabs, kept per connection (`query_history.json`): the text that
//! ran, when, how long it took and whether it worked. A text run again moves back to the top.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::persist;

/// Entries kept for each connection.
const PER_CONNECTION: usize = 50;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoryEntry {
    pub connection_id: String,
    pub text: String,
    pub executed_at: i64,
    pub duration_ms: u64,
    pub ok: bool,
}

pub struct HistoryStore {
    file_path: PathBuf,
    /// newest first
    entries: Vec<HistoryEntry>,
    /// false when the file couldn't be read nor moved aside: it is then never saved over
    writable: bool,
}

impl HistoryStore {
    pub fn new(file_path: PathBuf) -> Self {
        // a file that can't be read is kept, never replaced by the next save
        let (entries, writable) = persist::load_store::<Vec<HistoryEntry>>(&file_path);
        Self { file_path, entries: entries.unwrap_or_default(), writable }
    }

    /// Adds a run of `text` on `connection_id` at the top, replacing an earlier run of the same
    /// text, and drops what is left over the limit for that connection.
    pub fn record(&mut self, entry: HistoryEntry) {
        self.entries.retain(|e| !(e.connection_id == entry.connection_id && e.text == entry.text));
        let connection = entry.connection_id.clone();
        self.entries.insert(0, entry);
        let mut kept = 0;
        self.entries.retain(|e| {
            if e.connection_id != connection {
                return true;
            }
            kept += 1;
            kept <= PER_CONNECTION
        });
        self.save();
    }

    /// The runs on `connection_id`, newest first.
    pub fn for_connection(&self, connection_id: &str) -> Vec<HistoryEntry> {
        self.entries.iter().filter(|e| e.connection_id == connection_id).cloned().collect()
    }

    pub fn clear(&mut self, connection_id: &str) {
        self.entries.retain(|e| e.connection_id != connection_id);
        self.save();
    }

    fn save(&self) {
        if !self.writable {
            return;
        }
        if let Err(e) = persist::save(&self.file_path, &self.entries) {
            tracing::error!("saving the query history failed: {e}");
        }
    }
}

#[cfg(test)]
mod tests;
