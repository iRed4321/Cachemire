//! A saved connection to a Redis server and how one is reached: its settings, the
//! store that keeps them, the client and pool, and the SSH tunnel in between.

use crate::error::{Error, Invalid, NotFound, Timeout};
use crate::secret_vault::staged;
use crate::state::{AppState, resolve_connection};
use std::sync::Arc;
use std::time::Duration;

pub mod client;
pub mod pool;
pub mod ssh_tunnel;
pub mod store;
pub mod url;

/// How to reach the Redis server through an SSH server, when a connection
/// uses a tunnel: log in with a password, a private key, or both (the key is
/// tried first).
#[derive(Debug, Clone)]
pub struct SshSettings {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    /// path of a private key file (`~` stands for the home directory)
    pub private_key: String,
    pub passphrase: String,
}

/// Identifies a cloud endpoint across runs: the account and region it lives in
/// and its name (the replication group or cluster id). None of it is a secret.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloudKey {
    pub account: String,
    pub region: String,
    pub name: String,
}

impl CloudKey {
    /// How the endpoint is filed in the settings.
    pub fn endpoint_id(&self) -> String {
        format!("{}/{}/{}", self.account, self.region, self.name)
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionItem {
    pub id: String,
    pub name: String,
    /// where the server is, which database, and TLS (`rediss://`): see `endpoint`
    pub connection_url: String,
    /// used for whatever the connection URL doesn't say itself
    pub username: String,
    pub password: String,
    /// `Some` when the connection goes through an SSH tunnel
    pub ssh: Option<SshSettings>,
    /// the connection profile (see Settings) it belongs to; "" for none
    pub profile_id: String,
}

/// Everything needed to reach a Redis server, as a connection's URL and its own
/// username/password fields say together.
#[derive(Debug, Clone, PartialEq)]
pub struct Endpoint {
    pub host: String,
    pub port: u16,
    pub db: i64,
    pub tls: bool,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ConnectionItem {
    /// The server the URL names; the URL's own user/password win over the
    /// connection's fields, which fill in what it leaves out.
    pub fn endpoint(&self) -> Result<Endpoint, Error> {
        let non_empty = |value: String| if value.is_empty() { None } else { Some(value) };
        let parts = url::parse(&self.connection_url).filter(|parts| !parts.host.is_empty()).ok_or(Invalid::UnreadableUrl)?;
        Ok(Endpoint {
            host: parts.host,
            port: parts.port.filter(|p| *p > 0).unwrap_or(6379),
            db: parts.db.unwrap_or(0),
            tls: parts.tls,
            username: non_empty(parts.username).or_else(|| non_empty(self.username.trim().to_string())),
            password: non_empty(parts.password).or_else(|| non_empty(self.password.clone())),
        })
    }

    /// `host:port` of the server, or the URL as typed when it can't be read.
    pub fn address(&self) -> String {
        let bracketed = |host: String| if host.contains(':') { format!("[{host}]") } else { host };
        self.endpoint().map_or_else(|_| self.connection_url.clone(), |e| format!("{}:{}", bracketed(e.host), e.port))
    }
}

#[derive(Debug, Clone)]
pub struct ConnectionsState {
    pub active_connection_id: String,
    pub connections: Vec<ConnectionItem>,
}

/// Input payload for add/update. All fields optional so the same type can
/// represent a partial update payload; a field left `None` keeps its current
/// value (the way a blank password box means "keep the stored one").
#[derive(Debug, Clone, Default)]
pub struct ConnectionPayloadInput {
    pub name: Option<String>,
    pub connection_url: Option<String>,
    pub username: Option<String>,
    pub password: Option<String>,
    pub ssh_enabled: Option<bool>,
    pub ssh_host: Option<String>,
    pub ssh_port: Option<u16>,
    pub ssh_username: Option<String>,
    pub ssh_password: Option<String>,
    pub ssh_private_key: Option<String>,
    pub ssh_passphrase: Option<String>,
    /// the connection profile to use; `Some("")` for none
    pub profile_id: Option<String>,
}

/// Confirms `connection_id` is reachable, rebuilding its pool if stale, without
/// making it active (see `AppState::connect_attempt`). The error's kind says
/// what failed (the SSH tunnel, the login, the network…).
pub async fn connect_connection(state: &Arc<AppState>, connection_id: &str) -> Result<ConnectionItem, Error> {
    // the saved passwords are only fetched from the system keyring now, off
    // the async workers and outside the store's lock: it can wait on the user
    let unlocking = (state.clone(), connection_id.to_string());
    tokio::task::spawn_blocking(move || staged(&unlocking.0.connections, |store| store.unlock_secrets(&unlocking.1))).await??;

    // what else comes from the keyring: a cloud connection's tunnel secrets, and the active
    // SSH profile's password for a tunnel without a username of its own
    let (cloud_key, uses_profile) = {
        let store = state.connections.lock();
        let connection = store.get(connection_id).ok_or(NotFound::Connection)?;
        (store.cloud_key(connection_id), connection.ssh.is_some_and(|ssh| ssh.username.is_empty()))
    };

    if let Some(key) = cloud_key {
        let state = state.clone();
        tokio::task::spawn_blocking(move || staged(&state.settings, |settings| settings.unlock_cloud(&key))).await??;
    }

    if uses_profile {
        let state = state.clone();
        tokio::task::spawn_blocking(move || staged(&state.settings, |settings| settings.unlock_active_ssh_profile())).await??;
    }

    let connection = resolve_connection(state, Some(connection_id))?;

    // A pool already open may have gone silently stale (sleep, a dropped
    // NAT mapping): sockets that look open but never reply. A short PING of
    // each of its connections checks, and the pool is rebuilt if one is stale.
    let reused = state.redis_connections.is_connected(connection_id);
    if reused && !state.redis_connections.responds(connection_id, Duration::from_secs(3)).await {
        tracing::info!(name = %connection.name, "the open connections stopped answering; reopening them");
        state.redis_connections.invalidate(connection_id);
    }
    state.redis_connections.pool(&connection).await?;

    Ok(connection)
}

/// Checks that `connection` (as a form describes it, not saved) can be reached: through its
/// SSH tunnel if it has one, then a PING. A tunnel with no username logs in with the active SSH profile.
pub async fn test_connection(state: &Arc<AppState>, mut connection: ConnectionItem) -> Result<(), Error> {
    if connection.ssh.as_ref().is_some_and(|ssh| ssh.username.is_empty()) {
        let profile_state = state.clone();
        tokio::task::spawn_blocking(move || staged(&profile_state.settings, |settings| settings.unlock_active_ssh_profile())).await??;
    }
    if let Some(ssh) = connection.ssh.as_mut() {
        state.settings.lock().fill_ssh(ssh)?;
    }
    tokio::time::timeout(Duration::from_secs(15), state.redis_connections.test(&connection))
        .await
        .map_err(|_| Timeout::Connect { seconds: 15 })?
}

#[cfg(test)]
mod tests;
