//! The queries run from the query tabs, kept per connection (`query_history.json`): the text that
//! ran, when, how long it took and whether it worked. A text run again moves back to the top.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::persist::{self, Loaded};

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
}

impl HistoryStore {
    pub fn new(file_path: PathBuf) -> Self {
        // a file that can't be read is kept aside, not overwritten by the next run
        let entries = match persist::load::<Vec<HistoryEntry>>(&file_path) {
            Loaded::Read(entries) => entries,
            Loaded::Missing => Vec::new(),
            Loaded::Unusable(why) => {
                eprintln!("{} can't be read ({why})", file_path.display());
                persist::set_aside(&file_path);
                Vec::new()
            }
        };
        Self { file_path, entries }
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
        if let Err(e) = persist::save(&self.file_path, &self.entries) {
            eprintln!("saving the query history failed: {e}");
        }
    }
}
