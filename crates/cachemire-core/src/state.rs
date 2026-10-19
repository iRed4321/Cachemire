use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::AtomicU64;

use crate::connection::ConnectionItem;
use crate::connection::pool::ConnectionCache;
use crate::connection::store::ConnectionStore;
use crate::error::{Error, NotFound};
use crate::redisql::history::HistoryStore;
use crate::settings::SettingsStore;

pub struct AppState {
    pub connections: Mutex<ConnectionStore>,
    pub settings: Mutex<SettingsStore>,
    /// the queries run from the query tabs
    pub history: Mutex<HistoryStore>,
    pub redis_connections: ConnectionCache,
    /// Numbers the attempts to connect (bumped on the UI thread as each starts): one
    /// that isn't the latest when it ends was superseded and must not touch the window
    /// or the active connection.
    pub connect_attempt: AtomicU64,
}

const DATA_FILE: &str = "databases.json";
const KNOWN_HOSTS_FILE: &str = "ssh_known_hosts";
const SETTINGS_FILE: &str = "settings.json";
const HISTORY_FILE: &str = "query_history.json";

/// The directory the executable lives in: the fallback if the platform has no
/// per-user data directory.
fn exe_dir() -> PathBuf {
    std::env::current_exe().ok().and_then(|p| p.parent().map(|p| p.to_path_buf())).unwrap_or_default()
}

/// Copies the `files` an earlier version kept in `legacy` into `dir`, but only while
/// `dir` has no saved connections of its own yet. Whether it copied every one there was.
fn migrate_legacy_data(legacy: &Path, dir: &Path, files: &[&str]) -> bool {
    if dir.join(DATA_FILE).exists() || !legacy.join(DATA_FILE).exists() || std::fs::create_dir_all(dir).is_err() {
        return false;
    }
    let mut all_copied = true;
    for name in files.iter().filter(|name| legacy.join(name).exists()) {
        all_copied &= std::fs::copy(legacy.join(name), dir.join(name)).is_ok();
    }
    all_copied
}

/// `databases.json` in the platform's per-user data directory (`%APPDATA%\Cachemire` or
/// `~/.local/share/Cachemire`), resolved once. The former name's folder (`Redust`) is
/// moved over once; data next to the executable (an older layout) is copied once.
pub fn resolve_data_file() -> PathBuf {
    static DATA_FILE_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
        let Some(data) = dirs::data_dir() else {
            return exe_dir().join(DATA_FILE);
        };
        let dir = data.join("Cachemire");
        let former = data.join("Redust");
        if migrate_legacy_data(&former, &dir, &[DATA_FILE, KNOWN_HOSTS_FILE, SETTINGS_FILE, HISTORY_FILE]) {
            let _ = std::fs::remove_dir_all(&former);
        }
        migrate_legacy_data(&exe_dir(), &dir, &[DATA_FILE, KNOWN_HOSTS_FILE]);
        dir.join(DATA_FILE)
    });
    DATA_FILE_PATH.clone()
}

impl AppState {
    /// The state saved in the data directory: the connections, settings and history.
    pub fn load() -> Self {
        let data_file = resolve_data_file();
        // next to the saved connections: the SSH servers whose host keys have been seen
        let known_hosts = data_file.with_file_name(KNOWN_HOSTS_FILE);
        Self {
            settings: Mutex::new(SettingsStore::new(data_file.with_file_name(SETTINGS_FILE))),
            history: Mutex::new(HistoryStore::new(data_file.with_file_name(HISTORY_FILE))),
            connections: Mutex::new(ConnectionStore::new(data_file)),
            redis_connections: ConnectionCache::new(known_hosts),
            connect_attempt: AtomicU64::new(0),
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AppState {
    /// A state whose files all live in `dir` and whose secrets stay in memory.
    pub fn for_tests(dir: &Path) -> Self {
        Self {
            settings: Mutex::new(SettingsStore::in_memory(dir.join(SETTINGS_FILE))),
            history: Mutex::new(HistoryStore::new(dir.join(HISTORY_FILE))),
            connections: Mutex::new(ConnectionStore::in_memory(dir.join(DATA_FILE))),
            redis_connections: ConnectionCache::new(dir.join(KNOWN_HOSTS_FILE)),
            connect_attempt: AtomicU64::new(0),
        }
    }
}

/// Resolves an explicit connection id if provided, otherwise falls back to
/// whichever connection is marked active. An SSH tunnel with no username of
/// its own comes back completed with the active SSH profile.
pub fn resolve_connection(state: &AppState, connection_id: Option<&str>) -> Result<ConnectionItem, Error> {
    let (mut connection, cloud_key) = {
        let store = state.connections.lock();
        let connection = match connection_id {
            Some(id) if !id.trim().is_empty() => store.get(id).ok_or_else(|| NotFound::Connection.into()),
            _ => store.get_active(),
        }?;
        let cloud_key = store.cloud_key(&connection.id);
        (connection, cloud_key)
    };
    if let Some(ssh) = connection.ssh.as_mut() {
        let settings = state.settings.lock();
        if let Some(key) = cloud_key {
            settings.fill_cloud_secrets(&key, ssh);
        }
        settings.fill_ssh(ssh)?;
    }
    Ok(connection)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_left_next_to_the_executable_is_copied_once_and_never_over_newer_data() {
        let root = std::env::temp_dir().join(format!("cachemire-migrate-{}", std::process::id()));
        let (legacy, dir) = (root.join("exe"), root.join("data").join("Cachemire"));
        std::fs::create_dir_all(&legacy).unwrap();
        std::fs::write(legacy.join(DATA_FILE), "old").unwrap();

        migrate_legacy_data(&legacy, &dir, &[DATA_FILE, KNOWN_HOSTS_FILE]);
        assert_eq!(std::fs::read_to_string(dir.join(DATA_FILE)).unwrap(), "old");
        assert!(!dir.join(KNOWN_HOSTS_FILE).exists(), "only what exists is copied");

        std::fs::write(dir.join(DATA_FILE), "new").unwrap();
        migrate_legacy_data(&legacy, &dir, &[DATA_FILE, KNOWN_HOSTS_FILE]);
        assert_eq!(std::fs::read_to_string(dir.join(DATA_FILE)).unwrap(), "new");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
