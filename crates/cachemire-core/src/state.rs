use parking_lot::Mutex;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::sync::atomic::AtomicU64;

use crate::connection::pool::ConnectionCache;
use crate::connection::ssh_tunnel::HostKeys;
use crate::connection::store::ConnectionStore;
use crate::connection::{CloudKey, ConnectionItem};
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
const LOCK_FILE: &str = "cachemire.lock";

/// Whether this process is the one instance saving to the data directory.
pub enum InstanceLock {
    /// it is, for as long as this file stays open
    Held(std::fs::File),
    /// another instance is running: this one must not save over its files
    Taken,
    /// the lock file can't be made (a read-only folder): nothing to keep apart
    Unavailable,
}

/// Takes the data directory's instance lock, which the system releases when the
/// process ends, however it ends.
pub fn lock_instance() -> InstanceLock {
    let path = resolve_data_file().with_file_name(LOCK_FILE);
    let file = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path));
    match file {
        Ok(file) => match file.try_lock() {
            Ok(()) => InstanceLock::Held(file),
            Err(std::fs::TryLockError::WouldBlock) => InstanceLock::Taken,
            Err(std::fs::TryLockError::Error(e)) => {
                tracing::warn!("couldn't lock {}: {e}", path.display());
                InstanceLock::Unavailable
            }
        },
        Err(e) => {
            tracing::warn!("couldn't open {}: {e}", path.display());
            InstanceLock::Unavailable
        }
    }
}

/// The file whose presence next to the executable keeps all the data beside it.
const PORTABLE_MARKER: &str = "portable";

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
/// `~/.local/share/Cachemire`), resolved once; with a `portable` file next to the
/// executable, in a `data` folder there instead. Older layouts are copied over once.
pub fn resolve_data_file() -> PathBuf {
    static DATA_FILE_PATH: LazyLock<PathBuf> = LazyLock::new(|| {
        if exe_dir().join(PORTABLE_MARKER).is_file() {
            let dir = exe_dir().join("data");
            let _ = std::fs::create_dir_all(&dir);
            return dir.join(DATA_FILE);
        }
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
            redis_connections: ConnectionCache::new(HostKeys::new(known_hosts).with_user_known_hosts()),
            connect_attempt: AtomicU64::new(0),
        }
        .following_host_key_setting()
    }

    /// Makes the SSH tunnels follow the saved setting for accepting new host keys.
    fn following_host_key_setting(self) -> Self {
        let accept = self.settings.lock().accept_new_ssh_host_keys();
        self.redis_connections.host_keys().set_accepts_new(accept);
        self
    }

    /// Saves whether new SSH host keys are trusted without asking, and applies it at once.
    pub fn set_accept_new_ssh_host_keys(&self, accept: bool) -> Result<(), Error> {
        self.settings.lock().set_accept_new_ssh_host_keys(accept)?;
        self.redis_connections.host_keys().set_accepts_new(accept);
        Ok(())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl AppState {
    /// A state whose files all live in `dir` and whose secrets stay in memory.
    pub fn for_tests(dir: &Path) -> Self {
        let host_keys = HostKeys::new(dir.join(KNOWN_HOSTS_FILE));
        // the tests' SSH servers are new ones, with nobody to ask
        host_keys.set_accepts_new(true);
        Self {
            settings: Mutex::new(SettingsStore::in_memory(dir.join(SETTINGS_FILE))),
            history: Mutex::new(HistoryStore::new(dir.join(HISTORY_FILE))),
            connections: Mutex::new(ConnectionStore::in_memory(dir.join(DATA_FILE))),
            redis_connections: ConnectionCache::new(host_keys),
            connect_attempt: AtomicU64::new(0),
        }
    }
}

/// The connection `connection_id` names (`None`: the active one) and, for a cloud one, its key.
fn lookup(state: &AppState, connection_id: Option<&str>) -> Result<(ConnectionItem, Option<CloudKey>), Error> {
    let store = state.connections.lock();
    // `Some("")`: a window on no connection, which never means the active one
    let connection = match connection_id {
        Some("") => Err(NotFound::ActiveConnection.into()),
        Some(id) => store.get(id).ok_or_else(|| NotFound::Connection.into()),
        None => store.get_active(),
    }?;
    let cloud_key = store.cloud_key(&connection.id);
    Ok((connection, cloud_key))
}

/// Whether no write may be made through the connection: its own setting, or its
/// profile's (a cloud endpoint has the profile set for it).
pub fn is_read_only(state: &AppState, connection_id: Option<&str>) -> Result<bool, Error> {
    let (connection, cloud_key) = lookup(state, connection_id)?;
    let settings = state.settings.lock();
    let profile = cloud_key.map_or_else(|| connection.profile_id.clone(), |key| settings.cloud_profile_id(&key));
    Ok(connection.read_only || settings.connection_profile(&profile).is_some_and(|p| p.read_only))
}

/// Refuses with [`Error::ReadOnly`] when the connection is read-only: every write to Redis goes through this.
pub fn ensure_writable(state: &AppState, connection_id: Option<&str>) -> Result<(), Error> {
    if is_read_only(state, connection_id)? { Err(Error::ReadOnly) } else { Ok(()) }
}

/// Resolves an explicit connection id if provided (`""` being none), otherwise the
/// connection marked active. An SSH tunnel with no username of its own comes back
/// completed with the active SSH profile.
pub fn resolve_connection(state: &AppState, connection_id: Option<&str>) -> Result<ConnectionItem, Error> {
    let (mut connection, cloud_key) = lookup(state, connection_id)?;
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

    #[test]
    fn a_connection_is_the_one_named_or_else_the_active_one_its_tunnel_completed_by_the_ssh_profile() {
        let dir = crate::test_support::temp_dir("resolve");
        let state = AppState::for_tests(&dir);
        let payload = |name: &str, ssh: bool| crate::connection::ConnectionPayloadInput {
            name: Some(name.into()),
            connection_url: Some("redis://localhost:6379".into()),
            ssh_enabled: Some(ssh),
            ssh_host: ssh.then(|| "bastion".into()),
            ..Default::default()
        };
        state.connections.lock().create_with_id("plain", payload("Plain", false), crate::secret_vault::SecretsPolicy::PlainText).unwrap();
        state
            .connections
            .lock()
            .create_with_id("tunnelled", payload("Tunnelled", true), crate::secret_vault::SecretsPolicy::PlainText)
            .unwrap();
        state.connections.lock().set_active("plain").unwrap();

        assert!(resolve_connection(&state, Some("tunnelled")).is_err(), "no SSH profile to sign in with");
        let profile = state.settings.lock().create_ssh_profile("Work", "deploy", "s3cret").unwrap();
        state.settings.lock().set_active_ssh_profile(&profile).unwrap();
        let ssh = resolve_connection(&state, Some("tunnelled")).unwrap().ssh.unwrap();
        assert_eq!((ssh.username.as_str(), ssh.password.expose()), ("deploy", "s3cret"));

        assert_eq!(resolve_connection(&state, None).unwrap().id, "plain");
        let blank = resolve_connection(&state, Some(""));
        assert!(matches!(blank, Err(Error::NotFound(NotFound::ActiveConnection))), "a window on no connection never gets the active one");
        assert!(matches!(resolve_connection(&state, Some("gone")), Err(Error::NotFound(NotFound::Connection))));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
