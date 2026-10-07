#[cfg(test)]
use super::LockExt;
use rust_i18n::t;
use rustc_hash::FxHashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::models::{CloudKey, ConnectionItem, ConnectionPayloadInput, ConnectionsState, SshSettings};
use super::secret_vault::{OsVault, Secrets, SecretsCleanup, Vault, VaultBacked};
use super::error::AppError;
use super::persist::{self, Loaded};
use super::{now_ms, redis_url};

fn default_ssh_port() -> u16 {
    22
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredConnection {
    id: String,
    name: String,
    connection_url: String,
    // what a connection saved before the URL was required has instead of one:
    // only read, to build its URL when the file is loaded
    #[serde(default, skip_serializing)]
    host: String,
    #[serde(default, skip_serializing)]
    port: Option<u16>,
    #[serde(default, skip_serializing)]
    db: Option<u32>,
    username: String,
    password: String,
    last_connected_at: Option<i64>,
    // (`default`s: connections saved before SSH tunnels existed have none of these)
    #[serde(default)]
    ssh_enabled: bool,
    #[serde(default)]
    ssh_host: String,
    #[serde(default = "default_ssh_port")]
    ssh_port: u16,
    #[serde(default)]
    ssh_username: String,
    #[serde(default)]
    ssh_password: String,
    #[serde(default)]
    ssh_private_key: String,
    #[serde(default)]
    ssh_passphrase: String,
    // the connection profile it uses ("" for none)
    #[serde(default)]
    profile_id: String,
    // the passwords and passphrase are in the credential store, not in this file
    #[serde(default)]
    in_vault: bool,
}

impl StoredConnection {
    fn secrets(&self) -> Secrets {
        Secrets { password: self.password.clone(), ssh_password: self.ssh_password.clone(), ssh_passphrase: self.ssh_passphrase.clone() }
    }

    fn set_secrets(&mut self, secrets: Secrets) {
        self.password = secrets.password;
        self.ssh_password = secrets.ssh_password;
        self.ssh_passphrase = secrets.ssh_passphrase;
    }
}

impl Default for StoredConnection {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            connection_url: String::new(),
            host: String::new(),
            port: None,
            db: None,
            username: String::new(),
            password: String::new(),
            last_connected_at: None,
            ssh_enabled: false,
            ssh_host: String::new(),
            ssh_port: default_ssh_port(),
            ssh_username: String::new(),
            ssh_password: String::new(),
            ssh_private_key: String::new(),
            ssh_passphrase: String::new(),
            profile_id: String::new(),
            in_vault: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredState {
    active_connection_id: String,
    connections: Vec<StoredConnection>,
}

/// The saved connections, kept in memory and written through to a JSON
/// file. Passwords/passphrases live in the OS credential store (`Vault`)
/// instead when one exists; secrets are unlocked from it on demand (`unlock_secrets`).
#[derive(Clone)]
pub struct ConnectionStore {
    file_path: PathBuf,
    state: StoredState,
    vault: Arc<dyn Vault>,
    /// connections whose secrets are in the vault and haven't been read from
    /// it yet: their secrets are empty in `state` until they are
    unloaded: FxHashSet<String>,
    /// cleared by the first failure to store secrets: no more attempts (and,
    /// on Linux, unlock prompts) for the rest of the run
    vault_usable: bool,
    /// set on a copy `staged` rehearses on: nothing is saved to the file
    dry_run: bool,
    /// the cloud account's connections: known for this run only, never saved
    cloud: Vec<ConnectionItem>,
    /// the cloud connection in use, if the active one is such
    cloud_active: Option<String>,
    /// the account and region `cloud` was listed from
    cloud_where: (String, String),
}

impl VaultBacked for ConnectionStore {
    fn vault_mut(&mut self) -> &mut Arc<dyn Vault> {
        &mut self.vault
    }

    fn set_dry_run(&mut self, dry_run: bool) {
        self.dry_run = dry_run;
    }
}

/// Takes the password out of a connection URL, leaving the rest as it was.
fn without_password(url: &str) -> Option<String> {
    let mut parsed = if url.contains("://") { url::Url::parse(url) } else { url::Url::parse(&format!("redis://{url}")) }.ok()?;
    parsed.set_password(None).ok()?;
    Some(parsed.to_string())
}

/// `url` with the saved username and password added where the URL has none.
fn with_credentials(url: &str, username: &str, password: &str) -> String {
    let Some(mut parsed) = (if url.contains("://") { url::Url::parse(url) } else { url::Url::parse(&format!("redis://{url}")) }).ok() else {
        return url.to_string();
    };
    let mut changed = false;
    if parsed.username().is_empty() && !username.is_empty() && parsed.set_username(username).is_ok() {
        changed = true;
    }
    if parsed.password().is_none() && !password.is_empty() && parsed.set_password(Some(password)).is_ok() {
        changed = true;
    }
    if changed { parsed.to_string() } else { url.to_string() }
}

/// `redis://[user[:password]@]host:port[/db]`, with user and password
/// percent-encoded (via `url`) and IPv6 hosts bracketed; db 0 is left out.
pub(crate) fn build_connection_string(host: &str, port: u16, db: u32, username: &str, password: &str) -> String {
    let host = if host.contains(':') && !host.starts_with('[') { format!("[{host}]") } else { host.to_string() };
    let mut base = format!("redis://{host}:{port}");
    if db != 0 {
        base.push_str(&format!("/{db}"));
    }
    let Ok(mut url) = url::Url::parse(&base) else { return base };
    if !username.is_empty() {
        let _ = url.set_username(username);
    }
    if !password.is_empty() {
        let _ = url.set_password(Some(password));
    }
    url.to_string()
}

/// Applies the fields `payload` carries onto `conn` (the others keep their
/// values), trimming what's typed.
fn apply_payload(conn: &mut StoredConnection, payload: ConnectionPayloadInput) {
    let trimmed = |text: String| text.trim().to_string();
    if let Some(name) = payload.name {
        conn.name = trimmed(name);
    }
    if let Some(url) = payload.connection_url {
        conn.connection_url = trimmed(url);
    }
    if let Some(username) = payload.username {
        conn.username = trimmed(username);
    }
    if let Some(password) = payload.password {
        conn.password = password;
    }
    if let Some(enabled) = payload.ssh_enabled {
        conn.ssh_enabled = enabled;
    }
    if let Some(host) = payload.ssh_host {
        conn.ssh_host = trimmed(host);
    }
    if let Some(port) = payload.ssh_port {
        conn.ssh_port = port;
    }
    if let Some(username) = payload.ssh_username {
        conn.ssh_username = trimmed(username);
    }
    if let Some(password) = payload.ssh_password {
        conn.ssh_password = password;
    }
    if let Some(key) = payload.ssh_private_key {
        conn.ssh_private_key = trimmed(key);
    }
    if let Some(passphrase) = payload.ssh_passphrase {
        conn.ssh_passphrase = passphrase;
    }
    if let Some(profile_id) = payload.profile_id {
        conn.profile_id = profile_id;
    }
}

/// Checks the record can be connected with, moving a password typed into the
/// URL to its own field. `secrets_loaded` false means its secrets are still in
/// the credential store, so an SSH password can't be checked here.
fn finish(conn: &mut StoredConnection, secrets_loaded: bool) -> Result<(), AppError> {
    if conn.name.is_empty() {
        return Err(AppError::invalid(t!("Connection name is required.")));
    }
    if conn.connection_url.is_empty() {
        return Err(AppError::invalid(t!("Connection URL is required.")));
    }
    let parts = redis_url::parse(&conn.connection_url).ok_or_else(|| AppError::invalid(t!("Couldn't understand the connection URL.")))?;
    if parts.host.is_empty() {
        return Err(AppError::invalid(t!("The connection URL has no host.")));
    }
    // a password typed into the URL is a secret like any other: it moves to
    // the password field (which is what the URL's own password wins over
    // anyway) so it doesn't stay in the saved URL
    if !parts.password.is_empty()
        && let Some(stripped) = without_password(&conn.connection_url)
    {
        conn.password = parts.password;
        conn.connection_url = stripped;
    }
    if conn.ssh_enabled {
        if conn.ssh_host.is_empty() {
            return Err(AppError::invalid(t!("SSH host is required.")));
        }
        if conn.ssh_port == 0 {
            return Err(AppError::invalid(t!("SSH port must be a number between 1 and 65535.")));
        }
        // no username: the active SSH profile (see Settings) logs in, which
        // can't be known here, so there is nothing more to require
        if !conn.ssh_username.is_empty()
            && secrets_loaded
            && conn.ssh_password.is_empty()
            && conn.ssh_private_key.is_empty()
        {
            return Err(AppError::invalid(t!("Provide an SSH password or a private key.")));
        }
    }
    Ok(())
}

/// Builds a full connection record from a create-connection payload.
fn connection_from_payload(payload: ConnectionPayloadInput, secrets_loaded: bool) -> Result<StoredConnection, AppError> {
    let mut conn = StoredConnection { id: Uuid::new_v4().to_string(), ..Default::default() };
    apply_payload(&mut conn, payload);
    finish(&mut conn, secrets_loaded)?;
    Ok(conn)
}

/// The connection a fresh install starts with, seeded from `REDIS_*`
/// environment variables.
fn initial_state() -> StoredState {
    let mut state = StoredState {
        active_connection_id: "env-default".to_string(),
        connections: vec![StoredConnection {
            id: "env-default".to_string(),
            name: "Default".to_string(),
            connection_url: std::env::var("REDIS_URL").unwrap_or_default(),
            host: std::env::var("REDIS_HOST").unwrap_or_else(|_| "127.0.0.1".to_string()),
            port: std::env::var("REDIS_PORT").ok().and_then(|v| v.parse().ok()),
            db: std::env::var("REDIS_DB").ok().and_then(|v| v.parse().ok()),
            username: std::env::var("REDIS_USERNAME").unwrap_or_default(),
            password: std::env::var("REDIS_PASSWORD").unwrap_or_default(),
            ..Default::default()
        }],
    };
    give_urls(&mut state);
    state
}

/// Builds a URL for each connection that has only a host, port and database.
fn give_urls(state: &mut StoredState) {
    for conn in state.connections.iter_mut().filter(|c| c.connection_url.trim().is_empty() && !c.host.is_empty()) {
        conn.connection_url = build_connection_string(&conn.host, conn.port.unwrap_or(6379), conn.db.unwrap_or(0), "", "");
    }
}

fn to_item(stored: &StoredConnection) -> ConnectionItem {
    ConnectionItem {
        id: stored.id.clone(),
        name: stored.name.clone(),
        connection_url: stored.connection_url.clone(),
        username: stored.username.clone(),
        password: stored.password.clone(),
        ssh: stored.ssh_enabled.then(|| SshSettings {
            host: stored.ssh_host.clone(),
            port: stored.ssh_port,
            username: stored.ssh_username.clone(),
            password: stored.ssh_password.clone(),
            private_key: stored.ssh_private_key.clone(),
            passphrase: stored.ssh_passphrase.clone(),
        }),
        profile_id: stored.profile_id.clone(),
    }
}

impl ConnectionStore {
    pub fn new(file_path: PathBuf) -> Self {
        Self::with_vault(file_path, Arc::new(OsVault))
    }

    fn with_vault(file_path: PathBuf, vault: Arc<dyn Vault>) -> Self {
        // A file that can't be read is kept, not replaced: starting from defaults
        // and saving them must not destroy the connections that were in it.
        let (mut state, first_write) = match persist::load::<StoredState>(&file_path) {
            Loaded::Read(state) => (state, false),
            Loaded::Missing => (initial_state(), true),
            Loaded::Unusable(why) => {
                match persist::set_aside(&file_path) {
                    Some(kept) => eprintln!("{} can't be read ({why}); kept as {}", file_path.display(), kept.display()),
                    None => eprintln!("{} can't be read ({why}) and couldn't be moved aside", file_path.display()),
                }
                (initial_state(), true)
            }
        };
        give_urls(&mut state);
        let unloaded = state.connections.iter().filter(|c| c.in_vault).map(|c| c.id.clone()).collect();

        let mut store = Self { file_path, state, vault, unloaded, vault_usable: true, dry_run: false, cloud: Vec::new(), cloud_active: None, cloud_where: Default::default() };
        if first_write {
            let _ = store.commit(store.state.clone(), true);
        }
        store
    }

    /// Makes connection `id`'s secrets available before use: reads them from
    /// the credential store, or moves them there if the file still holds
    /// them (no store was available when they were saved). No-op if it has none.
    pub fn unlock_secrets(&mut self, id: &str) -> Result<(), AppError> {
        let Some(position) = self.state.connections.iter().position(|c| c.id == id) else { return Ok(()) };
        if self.unloaded.contains(id) {
            let stored = self
                .vault
                .get(id)
                .map_err(|e| AppError::Keyring(t!("Couldn't read the saved passwords from the system keyring: %{error}", error = e).into_owned()))?;
            let conn = &mut self.state.connections[position];
            match stored {
                Some(stored) => {
                    // (anything the file holds too is newer)
                    let mut secrets = conn.secrets();
                    for (mine, theirs) in [
                        (&mut secrets.password, stored.password),
                        (&mut secrets.ssh_password, stored.ssh_password),
                        (&mut secrets.ssh_passphrase, stored.ssh_passphrase),
                    ] {
                        if mine.is_empty() {
                            *mine = theirs;
                        }
                    }
                    conn.set_secrets(secrets);
                }
                // the entry was removed behind our back
                None => conn.in_vault = false,
            }
            self.unloaded.remove(id);
        } else {
            let conn = &self.state.connections[position];
            if !conn.in_vault && !conn.secrets().is_empty() && self.vault_usable {
                return self.commit(self.state.clone(), true);
            }
        }
        Ok(())
    }

    /// Puts `next`'s connections' secrets into the vault where needed. One
    /// whose secrets aren't loaded is left alone, as is one that can't be
    /// stored — its secrets stay in the file.
    fn vault_secrets(&mut self, next: &mut StoredState) {
        for conn in &mut next.connections {
            if self.unloaded.contains(&conn.id) {
                continue;
            }
            let secrets = conn.secrets();
            if secrets.is_empty() {
                conn.in_vault = false;
                continue;
            }
            let unchanged = conn.in_vault && self.state.connections.iter().any(|c| c.id == conn.id && c.in_vault && c.secrets() == secrets);
            if unchanged {
                continue;
            }
            conn.in_vault = self.vault_usable
                && match self.vault.set(&conn.id, &secrets) {
                    Ok(()) => true,
                    Err(_) => {
                        self.vault_usable = false;
                        false
                    }
                };
        }
    }

    /// Persists `next` and only then makes it current, so a failed write
    /// leaves memory and disk agreeing on the old state. With
    /// `store_secrets`, changed secrets are vaulted first; without it, untouched.
    fn commit(&mut self, mut next: StoredState, store_secrets: bool) -> Result<(), AppError> {
        if store_secrets {
            self.vault_secrets(&mut next);
        }
        let mut on_disk = next.clone();
        for conn in on_disk.connections.iter_mut().filter(|c| c.in_vault) {
            conn.set_secrets(Secrets::default());
        }
        if !self.dry_run {
            persist::save(&self.file_path, &on_disk)?;
        }
        self.state = next;
        Ok(())
    }

    pub fn list_state(&self) -> ConnectionsState {
        ConnectionsState {
            active_connection_id: self.active_connection_id().to_string(),
            connections: self.state.connections.iter().map(to_item).collect(),
        }
    }

    pub fn active_connection_id(&self) -> &str {
        self.cloud_active.as_deref().unwrap_or(&self.state.active_connection_id)
    }

    pub fn get(&self, id: &str) -> Option<ConnectionItem> {
        self.state.connections.iter().find(|c| c.id == id).map(to_item).or_else(|| self.cloud.iter().find(|c| c.id == id).cloned())
    }

    pub fn get_active(&self) -> Result<ConnectionItem, AppError> {
        self.get(self.active_connection_id()).ok_or_else(|| AppError::not_found(t!("No active connection.")))
    }

    pub fn cloud_connections(&self) -> &[ConnectionItem] {
        &self.cloud
    }

    /// The account and region the cloud connections were listed from ("" for none).
    pub fn cloud_account(&self) -> (&str, &str) {
        (&self.cloud_where.0, &self.cloud_where.1)
    }

    /// Where cloud connection `id` lives, for its settings.
    pub fn cloud_key(&self, id: &str) -> Option<CloudKey> {
        let item = self.cloud.iter().find(|c| c.id == id)?;
        Some(CloudKey { account: self.cloud_where.0.clone(), region: self.cloud_where.1.clone(), name: item.name.clone() })
    }

    /// Replaces the cloud account's connections, listed from `account` and `region`; the
    /// active one is dropped if it isn't among them.
    pub fn set_cloud_connections(&mut self, account: &str, region: &str, connections: Vec<ConnectionItem>) {
        self.cloud_where = (account.to_string(), region.to_string());
        if self.cloud_active.as_ref().is_some_and(|id| !connections.iter().any(|c| &c.id == id)) {
            self.cloud_active = None;
        }
        self.cloud = connections;
    }

    #[cfg(test)]
    pub fn create(&mut self, payload: ConnectionPayloadInput) -> Result<(), AppError> {
        self.create_with_id(&Uuid::new_v4().to_string(), payload)
    }

    /// `create`, under an id chosen by the caller.
    pub fn create_with_id(&mut self, id: &str, payload: ConnectionPayloadInput) -> Result<(), AppError> {
        self.add(id, payload, true)
    }

    /// Adds a connection without making it the active one (an import). Its tunnel may come
    /// with a username and no password or key, as a file made without the passwords has it.
    pub fn import_with_id(&mut self, id: &str, payload: ConnectionPayloadInput) -> Result<(), AppError> {
        self.add(id, payload, false)
    }

    fn add(&mut self, id: &str, payload: ConnectionPayloadInput, from_form: bool) -> Result<(), AppError> {
        let conn = StoredConnection { id: id.to_string(), ..connection_from_payload(payload, from_form)? };
        let mut next = self.state.clone();
        if from_form {
            next.active_connection_id = conn.id.clone();
        }
        next.connections.push(conn);
        self.commit(next, true)
    }

    /// The connection `payload` describes as it would be saved, without saving it: an edit
    /// of the saved connection `id` (its secrets unlocked first), or a new one when `id` is empty.
    pub fn preview(&self, id: &str, payload: ConnectionPayloadInput) -> Result<ConnectionItem, AppError> {
        if id.is_empty() {
            return Ok(to_item(&connection_from_payload(payload, true)?));
        }
        let mut conn = self.state.connections.iter().find(|c| c.id == id).cloned().ok_or_else(|| AppError::not_found(t!("Connection not found.")))?;
        apply_payload(&mut conn, payload);
        finish(&mut conn, !self.unloaded.contains(id))?;
        Ok(to_item(&conn))
    }

    pub fn update(&mut self, id: &str, payload: ConnectionPayloadInput) -> Result<(), AppError> {
        // the secrets are kept in one entry: changing one needs the others
        if payload.password.is_some() || payload.ssh_password.is_some() || payload.ssh_passphrase.is_some() {
            self.unlock_secrets(id)?;
        }
        let mut next = self.state.clone();
        let existing = next
            .connections
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| AppError::not_found(t!("Connection not found.")))?;
        apply_payload(existing, payload);
        finish(existing, !self.unloaded.contains(id))?;
        self.commit(next, true)
    }

    pub fn set_active(&mut self, id: &str) -> Result<(), AppError> {
        if self.cloud.iter().any(|c| c.id == id) {
            self.cloud_active = Some(id.to_string());
            return Ok(());
        }
        let mut next = self.state.clone();
        let conn = next
            .connections
            .iter_mut()
            .find(|c| c.id == id)
            .ok_or_else(|| AppError::not_found(t!("Connection not found.")))?;
        conn.last_connected_at = Some(now_ms());
        next.active_connection_id = id.to_string();
        self.commit(next, false)?;
        self.cloud_active = None;
        Ok(())
    }

    /// Leaves no connection active; every saved one is kept.
    pub fn clear_active(&mut self) -> Result<(), AppError> {
        self.cloud_active = None;
        let mut next = self.state.clone();
        next.active_connection_id.clear();
        self.commit(next, false)
    }

    /// Removes saved connection `id` (clearing active status if it was
    /// active). Its credential-store entries are left for the caller — run
    /// the returned `SecretsCleanup` off the UI thread.
    pub fn remove(&mut self, id: &str) -> Result<Option<SecretsCleanup>, AppError> {
        let stored = self.state.connections.iter().find(|c| c.id == id).ok_or_else(|| AppError::not_found(t!("Connection not found.")))?;
        let cleanup = stored.in_vault.then(|| SecretsCleanup::new(self.vault.clone(), id));
        let mut next = self.state.clone();
        next.connections.retain(|c| c.id != id);
        if next.active_connection_id == id {
            next.active_connection_id.clear();
        }
        self.commit(next, false)?;
        self.unloaded.remove(id);
        Ok(cleanup)
    }

    /// A connection string for connection `id`, credentials included: its URL
    /// with the saved username/password added where the URL has none.
    pub fn connection_string(&self, id: &str) -> Option<String> {
        let c = self.get(id)?;
        Some(with_credentials(c.connection_url.trim(), &c.username, &c.password))
    }
}

#[cfg(test)]
mod tests {
    use super::super::secret_vault::{MemoryVault, Secrets};
    use super::*;
    use std::fs;
    use std::sync::atomic::Ordering;

    #[test]
    fn builds_redis_urls_from_parts() {
        assert_eq!(build_connection_string("127.0.0.1", 6379, 0, "", ""), "redis://127.0.0.1:6379");
        assert_eq!(build_connection_string("cache.local", 6380, 2, "", ""), "redis://cache.local:6380/2");
        assert_eq!(build_connection_string("h", 6379, 0, "", "s3cr@t"), "redis://:s3cr%40t@h:6379");
        assert_eq!(build_connection_string("h", 6379, 1, "app", "p w"), "redis://app:p%20w@h:6379/1");
        assert_eq!(build_connection_string("::1", 6379, 0, "", ""), "redis://[::1]:6379");
    }

    fn payload(name: &str, url: &str) -> ConnectionPayloadInput {
        ConnectionPayloadInput { name: Some(name.into()), connection_url: Some(url.into()), ..Default::default() }
    }

    #[test]
    fn host_port_and_database_come_from_the_connection_url() {
        let c = connection_from_payload(payload(" prod ", "redis://app:s3cr%40t@cache.local:6380/3"), true).unwrap();
        let e = to_item(&c).endpoint().unwrap();
        assert_eq!((c.name.as_str(), e.host.as_str(), e.port, e.db), ("prod", "cache.local", 6380, 3));
        assert!(!c.ssh_enabled);

        let bare = connection_from_payload(payload("x", "cache.local"), true).unwrap();
        let e = to_item(&bare).endpoint().unwrap();
        assert_eq!((e.host.as_str(), e.port, e.db), ("cache.local", 6379, 0));

        assert_eq!(connection_from_payload(payload("x", ""), true).unwrap_err(), AppError::Invalid("Connection URL is required.".into()));
        assert_eq!(connection_from_payload(payload("", "cache.local"), true).unwrap_err(), AppError::Invalid("Connection name is required.".into()));
        assert!(connection_from_payload(payload("x", "redis://"), true).is_err());
    }

    #[test]
    fn an_ssh_tunnel_needs_a_host_a_username_and_a_way_to_log_in() {
        let with_ssh = |edit: &dyn Fn(&mut ConnectionPayloadInput)| {
            let mut p = payload("t", "redis://10.0.0.5:6379");
            p.ssh_enabled = Some(true);
            p.ssh_host = Some("bastion.example.com".into());
            p.ssh_username = Some("deploy".into());
            p.ssh_password = Some("pw".into());
            edit(&mut p);
            connection_from_payload(p, true)
        };
        let c = with_ssh(&|_| {}).unwrap();
        let item = to_item(&c);
        let ssh = item.ssh.expect("tunnel settings");
        assert_eq!((ssh.host.as_str(), ssh.port, ssh.username.as_str(), ssh.password.as_str()), ("bastion.example.com", 22, "deploy", "pw"));

        assert_eq!(with_ssh(&|p| p.ssh_host = Some("  ".into())).unwrap_err(), AppError::Invalid("SSH host is required.".into()));
        // without a username the SSH profile from the settings is used at connect time
        assert!(with_ssh(&|p| {
            p.ssh_username = Some("".into());
            p.ssh_password = Some("".into());
        })
        .is_ok());
        assert_eq!(with_ssh(&|p| p.ssh_port = Some(0)).unwrap_err(), AppError::Invalid("SSH port must be a number between 1 and 65535.".into()));
        assert_eq!(with_ssh(&|p| p.ssh_password = Some("".into())).unwrap_err(), AppError::Invalid("Provide an SSH password or a private key.".into()));
        assert!(with_ssh(&|p| {
            p.ssh_password = Some("".into());
            p.ssh_private_key = Some("~/.ssh/id_ed25519".into());
        })
        .is_ok());

        // switched off, the SSH fields aren't checked and there are no settings
        let off = with_ssh(&|p| {
            p.ssh_enabled = Some(false);
            p.ssh_host = Some("".into());
        })
        .unwrap();
        assert!(to_item(&off).ssh.is_none());
    }

    #[test]
    fn updating_keeps_what_the_form_leaves_out_and_reads_the_new_url() {
        let dir = std::env::temp_dir().join(format!("cachemire-store-test-{}", now_ms()));
        let mut store = ConnectionStore::with_vault(dir.join("databases.json"), Arc::new(MemoryVault::default()));
        let mut p = payload("t", "redis://old.host:6379");
        p.password = Some("keep-me".into());
        p.ssh_enabled = Some(true);
        p.ssh_host = Some("bastion".into());
        p.ssh_username = Some("u".into());
        p.ssh_password = Some("ssh-secret".into());
        store.create(p).unwrap();
        let id = store.active_connection_id().to_string();

        // a blank password box arrives as None: both stored secrets stay
        let mut edit = payload("t2", "redis://new.host:6400/2");
        edit.ssh_host = Some("bastion2".into());
        store.update(&id, edit).unwrap();
        let item = store.get(&id).unwrap();
        let e = item.endpoint().unwrap();
        assert_eq!((item.name.as_str(), e.host.as_str(), e.port, e.db, item.password.as_str()), ("t2", "new.host", 6400, 2, "keep-me"));
        let ssh = item.ssh.unwrap();
        assert_eq!((ssh.host.as_str(), ssh.password.as_str()), ("bastion2", "ssh-secret"));

        // an invalid edit changes nothing
        let mut bad = payload("t3", "redis://x:6379");
        bad.ssh_host = Some("".into());
        assert!(store.update(&id, bad).is_err());
        assert_eq!(store.get(&id).unwrap().name, "t2");

        // a file from before SSH tunnels existed still loads
        let old = r#"{"activeConnectionId":"a","connections":[{"id":"a","name":"Old","connectionUrl":"","host":"h","port":6379,"db":0,"username":"","password":"","lastConnectedAt":null}]}"#;
        std::fs::write(dir.join("old.json"), old).unwrap();
        let store = ConnectionStore::with_vault(dir.join("old.json"), Arc::new(MemoryVault::default()));
        assert!(store.get("a").unwrap().ssh.is_none());
        let _ = std::fs::remove_dir_all(dir);
    }

    fn temp_file(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("cachemire-vault-{name}-{}-{}", std::process::id(), now_ms())).join("databases.json")
    }

    fn with_secrets() -> ConnectionPayloadInput {
        let mut p = payload("t", "redis://cache:6379");
        p.password = Some("redis-secret".into());
        p.ssh_enabled = Some(true);
        p.ssh_host = Some("bastion".into());
        p.ssh_username = Some("u".into());
        p.ssh_password = Some("ssh-secret".into());
        p.ssh_passphrase = Some("phrase-secret".into());
        p
    }

    fn reads(vault: &MemoryVault) -> usize {
        vault.reads.load(Ordering::SeqCst)
    }

    #[test]
    fn secrets_go_to_the_vault_and_stay_out_of_the_file() {
        let file = temp_file("out");
        let vault = MemoryVault::default();
        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        store.create(with_secrets()).unwrap();
        let id = store.active_connection_id().to_string();

        let saved = fs::read_to_string(&file).unwrap();
        for secret in ["redis-secret", "ssh-secret", "phrase-secret"] {
            assert!(!saved.contains(secret), "{secret} leaked into the file");
        }
        assert_eq!(vault.entries.lock_recover()[&id].ssh_passphrase, "phrase-secret");
        assert_eq!(store.get(&id).unwrap().password, "redis-secret");

        // unrelated changes keep the file clean
        store.set_active(&id).unwrap();
        assert!(!fs::read_to_string(&file).unwrap().contains("redis-secret"));

        // a new run doesn't ask the vault for anything until the connection is used
        let mut reopened = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
        assert_eq!(reads(&vault), 0);
        assert_eq!(reopened.get(&id).unwrap().password, "");
        reopened.unlock_secrets(&id).unwrap();
        let item = reopened.get(&id).unwrap();
        assert_eq!((item.password.as_str(), item.ssh.unwrap().passphrase.as_str()), ("redis-secret", "phrase-secret"));
        assert_eq!(reads(&vault), 1);
        reopened.unlock_secrets(&id).unwrap();
        assert_eq!(reads(&vault), 1, "read once");
    }

    #[test]
    fn a_connection_without_secrets_never_touches_the_vault() {
        let file = temp_file("plain");
        let vault = MemoryVault::default();
        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        store.create(payload("plain", "redis://cache:6379")).unwrap();
        let id = store.active_connection_id().to_string();
        vault.broken.store(true, Ordering::SeqCst);

        let mut reopened = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
        reopened.unlock_secrets(&id).unwrap();
        reopened.set_active(&id).unwrap();
        reopened.update(&id, payload("renamed", "redis://cache:6379")).unwrap();
        assert_eq!(reads(&vault), 0);
    }

    #[test]
    fn without_a_credential_store_the_secrets_stay_in_the_file_until_a_store_appears() {
        let file = temp_file("none");
        let vault = MemoryVault::default();
        vault.broken.store(true, Ordering::SeqCst);
        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        store.create(with_secrets()).unwrap();
        let id = store.active_connection_id().to_string();
        assert!(fs::read_to_string(&file).unwrap().contains("redis-secret"));

        let mut again = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        assert_eq!(again.get(&id).unwrap().password, "redis-secret");
        again.unlock_secrets(&id).unwrap();

        // with a store, using the connection moves them there
        vault.broken.store(false, Ordering::SeqCst);
        let mut reopened = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        assert!(fs::read_to_string(&file).unwrap().contains("redis-secret"), "not before the connection is used");
        reopened.unlock_secrets(&id).unwrap();
        assert_eq!(reopened.get(&id).unwrap().password, "redis-secret");
        assert!(!fs::read_to_string(&file).unwrap().contains("redis-secret"));
        assert_eq!(vault.entries.lock_recover()[&id].password, "redis-secret");
    }

    #[test]
    fn a_vault_that_cannot_be_read_never_costs_the_stored_secrets() {
        let file = temp_file("locked");
        let vault = MemoryVault::default();
        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        store.create(with_secrets()).unwrap();
        let id = store.active_connection_id().to_string();

        vault.broken.store(true, Ordering::SeqCst);
        let mut locked = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
        assert!(locked.unlock_secrets(&id).unwrap_err().message().contains("system keyring"));
        // everything that doesn't need the secrets still works, and they stay put
        locked.set_active(&id).unwrap();
        locked.update(&id, payload("renamed", "redis://cache:6379")).unwrap();
        assert!(locked.update(&id, ConnectionPayloadInput { password: Some("new".into()), ..payload("renamed", "redis://cache:6379") }).is_err());
        vault.broken.store(false, Ordering::SeqCst);
        assert_eq!(vault.entries.lock_recover()[&id], Secrets {
            password: "redis-secret".into(),
            ssh_password: "ssh-secret".into(),
            ssh_passphrase: "phrase-secret".into(),
        });
        locked.unlock_secrets(&id).unwrap();
        assert_eq!(locked.get(&id).unwrap().password, "redis-secret");
    }

    #[test]
    fn changing_one_secret_keeps_the_others() {
        let file = temp_file("one");
        let vault = MemoryVault::default();
        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        store.create(with_secrets()).unwrap();
        let id = store.active_connection_id().to_string();

        let mut reopened = ConnectionStore::with_vault(file, Arc::new(vault.clone()));
        let edit = ConnectionPayloadInput { password: Some("new".into()), ..payload("t", "redis://cache:6379") };
        reopened.update(&id, edit).unwrap();
        let stored = vault.entries.lock_recover()[&id].clone();
        assert_eq!((stored.password.as_str(), stored.ssh_password.as_str(), stored.ssh_passphrase.as_str()), ("new", "ssh-secret", "phrase-secret"));
    }

    #[test]
    fn a_password_typed_into_the_url_is_moved_out_of_it() {
        let file = temp_file("url");
        let vault = MemoryVault::default();
        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        store.create(payload("t", "redis://app:s3cr%40t@cache:6380/2")).unwrap();
        let id = store.active_connection_id().to_string();

        let item = store.get(&id).unwrap();
        assert_eq!((item.connection_url.as_str(), item.username.as_str(), item.password.as_str()), ("redis://app@cache:6380/2", "", "s3cr@t"));
        assert!(!fs::read_to_string(&file).unwrap().contains("s3cr"));
        // copying still gives the complete string
        assert_eq!(store.connection_string(&id).unwrap(), "redis://app:s3cr%40t@cache:6380/2");

        // a URL without credentials picks up the saved ones
        assert_eq!(with_credentials("cache:6379", "", "pw"), "redis://:pw@cache:6379");
        assert_eq!(with_credentials("redis://cache:6379", "", ""), "redis://cache:6379");
        assert_eq!(without_password("redis://:pw@cache:6379").unwrap(), "redis://cache:6379");
    }

    #[test]
    fn removing_a_connection_forgets_it_and_its_secrets() {
        let file = temp_file("remove");
        let vault = MemoryVault::default();
        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(vault.clone()));
        store.create(payload("keep", "redis://a:6379")).unwrap();
        let keep = store.active_connection_id().to_string();
        store.create(with_secrets()).unwrap();
        let gone = store.active_connection_id().to_string();
        assert!(vault.entries.lock_recover().contains_key(&gone));

        // even with the vault unreachable the connection goes; clearing its entry comes after
        let cleanup = store.remove(&gone).unwrap().expect("it had secrets in the vault");
        assert!(store.get(&gone).is_none());
        assert_eq!(store.active_connection_id(), "", "the active connection was removed");
        assert!(store.get(&keep).is_some());
        assert!(vault.entries.lock_recover().contains_key(&gone));
        cleanup.run();
        assert!(!vault.entries.lock_recover().contains_key(&gone));

        // gone from the file too, and removing a plain one has nothing to clean
        assert!(store.remove(&keep).unwrap().is_none());
        assert!(store.remove(&keep).is_err());
        let reopened = ConnectionStore::with_vault(file, Arc::new(vault));
        assert!(reopened.get(&keep).is_none() && reopened.get(&gone).is_none());
    }

    #[test]
    fn a_file_that_cannot_be_read_is_kept_and_never_overwritten() {
        let file = temp_file("unusable");
        fs::create_dir_all(file.parent().unwrap()).unwrap();
        let broken = r#"{"activeConnectionId": "a", "connections": [{"id": "a", "port": "not a number"}]}"#;
        fs::write(&file, broken).unwrap();

        let mut store = ConnectionStore::with_vault(file.clone(), Arc::new(MemoryVault::default()));
        store.create(payload("fresh", "redis://cache:6379")).unwrap();
        // what was there is still there, next to the new file
        let kept: Vec<_> = fs::read_dir(file.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().contains(".unusable-"))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(fs::read_to_string(kept[0].path()).unwrap(), broken);
        assert!(fs::read_to_string(&file).unwrap().contains("fresh"));
    }
}

