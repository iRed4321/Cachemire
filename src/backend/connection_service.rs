use super::LockExt;
use rust_i18n::t;
use std::sync::Arc;
use std::time::Duration;

use super::error::AppError;
use super::secret_vault::staged;
use super::models::ConnectionItem;
use super::state::{resolve_connection, AppState};

/// Confirms `connection_id` is reachable, rebuilding its pool if stale, without
/// making it active (see `AppState::connect_attempt`). The error's kind says
/// what failed (the SSH tunnel, the login, the network…).
pub async fn connect_connection(state: &Arc<AppState>, connection_id: &str) -> Result<ConnectionItem, AppError> {
    // the saved passwords are only fetched from the system keyring now, off
    // the async workers and outside the store's lock: it can wait on the user
    let unlocking = (state.clone(), connection_id.to_string());
    tokio::task::spawn_blocking(move || staged(&unlocking.0.connections, |store| store.unlock_secrets(&unlocking.1)))
        .await??;

    // a cloud connection's tunnel secrets come from the keyring the same way
    let cloud_key = state.connections.lock_recover().cloud_key(connection_id);
    if let Some(key) = cloud_key {
        let state = state.clone();
        tokio::task::spawn_blocking(move || staged(&state.settings, |settings| settings.unlock_cloud(&key)))
            .await??;
    }

    // a tunnel without a username of its own logs in with the active SSH
    // profile, whose password is fetched the same way
    let uses_profile = state
        .connections
        .lock_recover()
        .get(connection_id)
        .ok_or_else(|| AppError::not_found(t!("Connection not found.")))?
        .ssh
        .is_some_and(|ssh| ssh.username.is_empty());
    if uses_profile {
        let state = state.clone();
        tokio::task::spawn_blocking(move || staged(&state.settings, |settings| settings.unlock_active_ssh_profile()))
            .await??;
    }

    let connection = resolve_connection(state, Some(connection_id))?;

    // A pool already open may have gone silently stale (sleep, a dropped
    // NAT mapping): sockets that look open but never reply. A short PING of
    // each of its connections checks, and the pool is rebuilt if one is stale.
    let reused = state.redis_connections.is_connected(connection_id);
    if reused && !state.redis_connections.responds(connection_id, Duration::from_secs(3)).await {
        state.redis_connections.invalidate(connection_id);
    }
    state.redis_connections.pool(&connection).await?;

    Ok(connection)
}

/// Checks that `connection` (as a form describes it, not saved) can be reached: through its
/// SSH tunnel if it has one, then a PING. A tunnel with no username logs in with the active SSH profile.
pub async fn test_connection(state: &Arc<AppState>, mut connection: ConnectionItem) -> Result<(), AppError> {
    if connection.ssh.as_ref().is_some_and(|ssh| ssh.username.is_empty()) {
        let profile_state = state.clone();
        tokio::task::spawn_blocking(move || staged(&profile_state.settings, |settings| settings.unlock_active_ssh_profile()))
            .await??;
    }
    if let Some(ssh) = connection.ssh.as_mut() {
        state.settings.lock_recover().fill_ssh(ssh)?;
    }
    tokio::time::timeout(Duration::from_secs(15), state.redis_connections.test(&connection))
        .await
        .map_err(|_| AppError::Timeout(t!("Connection timed out after %{seconds} seconds.", seconds = 15).into_owned()))?
}
