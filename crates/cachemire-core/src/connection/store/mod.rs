use rustc_hash::FxHashSet;
use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::connection::{CloudKey, ConnectionItem, ConnectionPayloadInput, ConnectionsState, SshSettings};
use crate::error::{Error, Invalid, NotFound};
use crate::now_ms;
use crate::persist;
use crate::secret_vault::{KeyringError, LeftOut, OsVault, Secrets, SecretsCleanup, SecretsPolicy, Vault, VaultBacked, Vaulting, place};

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
    // no write to Redis is made through this connection
    #[serde(default)]
    read_only: bool,
    // the passwords and passphrase are in the credential store, not in this file
    #[serde(default)]
    in_vault: bool,
    // the secrets saved nowhere: asked for when connecting, held in memory for the run
    #[serde(default, skip_serializing_if = "LeftOut::is_empty")]
    left_out: LeftOut,
    // what a newer version wrote that this one doesn't know: saved back as it was
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
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
            read_only: false,
            in_vault: false,
            left_out: LeftOut::default(),
            extra: Default::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredState {
    active_connection_id: String,
    connections: Vec<StoredConnection>,
    // what a newer version wrote that this one doesn't know: saved back as it was
    #[serde(flatten)]
    extra: serde_json::Map<String, serde_json::Value>,
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
    let Some(mut parsed) = (if url.contains("://") { url::Url::parse(url) } else { url::Url::parse(&format!("redis://{url}")) }).ok()
    else {
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
pub fn build_connection_string(host: &str, port: u16, db: u32, username: &str, password: &str) -> String {
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
        conn.left_out.password = false;
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
        conn.left_out.ssh_password = false;
    }
    if let Some(key) = payload.ssh_private_key {
        conn.ssh_private_key = trimmed(key);
    }
    if let Some(passphrase) = payload.ssh_passphrase {
        conn.ssh_passphrase = passphrase;
        conn.left_out.ssh_passphrase = false;
    }
    if let Some(profile_id) = payload.profile_id {
        conn.profile_id = profile_id;
    }
    if let Some(read_only) = payload.read_only {
        conn.read_only = read_only;
    }
}

/// Checks the record can be connected with, moving a password typed into the
/// URL to its own field. `secrets_loaded` false means its secrets are still in
/// the credential store, so an SSH password can't be checked here.
fn finish(conn: &mut StoredConnection, secrets_loaded: bool) -> Result<(), Error> {
    if conn.name.is_empty() {
        return Err(Invalid::ConnectionName.into());
    }
    if conn.connection_url.is_empty() {
        return Err(Invalid::ConnectionUrl.into());
    }
    let parts = super::url::parse(&conn.connection_url)?;
    if parts.host.is_empty() {
        return Err(Invalid::UrlWithoutHost.into());
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
            return Err(Invalid::SshHost.into());
        }
        if conn.ssh_port == 0 {
            return Err(Invalid::SshPort.into());
        }
        // no username: the active SSH profile (see Settings) logs in, which
        // can't be known here, so there is nothing more to require
        if !conn.ssh_username.is_empty()
            && secrets_loaded
            && conn.ssh_password.is_empty()
            && !conn.left_out.ssh_password
            && conn.ssh_private_key.is_empty()
        {
            return Err(Invalid::SshLogin.into());
        }
    }
    Ok(())
}

/// Builds a full connection record from a create-connection payload.
fn connection_from_payload(payload: ConnectionPayloadInput, secrets_loaded: bool) -> Result<StoredConnection, Error> {
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
        extra: Default::default(),
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
        read_only: stored.read_only,
    }
}

impl ConnectionStore {
    pub fn new(file_path: PathBuf) -> Self {
        Self::with_vault(file_path, Arc::new(OsVault))
    }

    /// A store whose secrets stay in memory, for the tests.
    #[cfg(any(test, feature = "test-support"))]
    pub fn in_memory(file_path: PathBuf) -> Self {
        Self::with_vault(file_path, Arc::new(crate::secret_vault::MemoryVault::default()))
    }

    pub(crate) fn with_vault(file_path: PathBuf, vault: Arc<dyn Vault>) -> Self {
        // a file that can't be read is kept, never replaced by the defaults
        let (stored, writable) = persist::load_store::<StoredState>(&file_path);
        let first_write = stored.is_none() && writable;
        let mut state = stored.unwrap_or_else(initial_state);
        give_urls(&mut state);
        let unloaded = state.connections.iter().filter(|c| c.in_vault).map(|c| c.id.clone()).collect();

        let mut store = Self {
            file_path,
            state,
            vault,
            unloaded,
            vault_usable: true,
            dry_run: !writable,
            cloud: Vec::new(),
            cloud_active: None,
            cloud_where: Default::default(),
        };
        if first_write {
            let _ = store.commit(store.state.clone(), true);
        }
        store
    }

    /// Makes connection `id`'s secrets available before use: reads them from
    /// the credential store, or moves them there if the file still holds
    /// them (no store was available when they were saved). No-op if it has none.
    pub fn unlock_secrets(&mut self, id: &str) -> Result<(), Error> {
        let Some(position) = self.state.connections.iter().position(|c| c.id == id) else { return Ok(()) };
        if self.unloaded.contains(id) {
            let stored = self.vault.get(id).map_err(KeyringError::ConnectionPasswords)?;
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
            if !conn.in_vault && !conn.secrets().without(conn.left_out).is_empty() && self.vault_usable {
                return self.commit(self.state.clone(), true);
            }
        }
        Ok(())
    }

    /// Puts `next`'s connections' secrets into the vault where needed, never those left out
    /// or not loaded. The ones `vaulting` names follow `policy`; the others are moved quietly
    /// and stay in the file if they can't be stored.
    fn vault_secrets(&mut self, next: &mut StoredState, vaulting: Vaulting<'_>, policy: SecretsPolicy) -> Result<(), Error> {
        for conn in &mut next.connections {
            if self.unloaded.contains(&conn.id) {
                continue;
            }
            let saving = matches!(vaulting, Vaulting::Saving(ids) if ids.contains(&conn.id.as_str()));
            if saving && policy == SecretsPolicy::LeaveOut {
                let set = conn.secrets().set_ones();
                conn.left_out = LeftOut {
                    password: conn.left_out.password || set.password,
                    ssh_password: conn.left_out.ssh_password || set.ssh_password,
                    ssh_passphrase: conn.left_out.ssh_passphrase || set.ssh_passphrase,
                };
            }
            let secrets = conn.secrets().without(conn.left_out);
            if secrets.is_empty() {
                conn.in_vault = false;
                continue;
            }
            let before = self.state.connections.iter().find(|c| c.id == conn.id);
            let unchanged = conn.in_vault && before.is_some_and(|c| c.in_vault && c.secrets().without(c.left_out) == secrets);
            if unchanged {
                continue;
            }
            // what the file holds already is no new exposure: it isn't asked about again
            let in_file_already = before.is_some_and(|c| !c.in_vault && c.secrets().without(c.left_out) == secrets);
            let policy = if saving && !in_file_already { policy } else { SecretsPolicy::PlainText };
            conn.in_vault = place(&*self.vault, &conn.id, &secrets, policy, &mut self.vault_usable)?;
        }
        Ok(())
    }

    /// Persists `next` and only then makes it current, so a failed write
    /// leaves memory and disk agreeing on the old state. With
    /// `store_secrets`, changed secrets are vaulted first; without it, untouched.
    fn commit(&mut self, next: StoredState, store_secrets: bool) -> Result<(), Error> {
        self.commit_with(next, if store_secrets { Vaulting::Quietly } else { Vaulting::Untouched }, SecretsPolicy::PlainText)
    }

    fn commit_with(&mut self, mut next: StoredState, vaulting: Vaulting<'_>, policy: SecretsPolicy) -> Result<(), Error> {
        if vaulting != Vaulting::Untouched {
            self.vault_secrets(&mut next, vaulting, policy)?;
        }
        let mut on_disk = next.clone();
        for conn in on_disk.connections.iter_mut() {
            let kept = if conn.in_vault { Secrets::default() } else { conn.secrets().without(conn.left_out) };
            conn.set_secrets(kept);
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

    pub fn get_active(&self) -> Result<ConnectionItem, Error> {
        self.get(self.active_connection_id()).ok_or_else(|| NotFound::ActiveConnection.into())
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
    pub fn create(&mut self, payload: ConnectionPayloadInput) -> Result<(), Error> {
        self.create_with_id(&Uuid::new_v4().to_string(), payload, SecretsPolicy::PlainText)
    }

    /// `create`, under an id chosen by the caller; `policy` says what to do with secrets
    /// the credential store won't take.
    pub fn create_with_id(&mut self, id: &str, payload: ConnectionPayloadInput, policy: SecretsPolicy) -> Result<(), Error> {
        self.add(id, payload, policy)
    }

    /// Adds a connection without making it the active one (an import). Its tunnel may come
    /// with a username and no password or key, as a file made without the passwords has it.
    pub fn import_with_id(&mut self, id: &str, payload: ConnectionPayloadInput, policy: SecretsPolicy) -> Result<(), Error> {
        self.import_all(&[(id.to_string(), payload)], policy)
    }

    /// `import_with_id` for several at once: all are saved, or none.
    pub fn import_all(&mut self, items: &[(String, ConnectionPayloadInput)], policy: SecretsPolicy) -> Result<(), Error> {
        let mut next = self.state.clone();
        for (id, payload) in items {
            next.connections.push(StoredConnection { id: id.clone(), ..connection_from_payload(payload.clone(), false)? });
        }
        let ids: Vec<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
        self.commit_with(next, Vaulting::Saving(&ids), policy)
    }

    fn add(&mut self, id: &str, payload: ConnectionPayloadInput, policy: SecretsPolicy) -> Result<(), Error> {
        let conn = StoredConnection { id: id.to_string(), ..connection_from_payload(payload, true)? };
        let mut next = self.state.clone();
        next.active_connection_id = conn.id.clone();
        next.connections.push(conn);
        self.commit_with(next, Vaulting::Saving(&[id]), policy)
    }

    /// The connection `payload` describes as it would be saved, without saving it: an edit
    /// of the saved connection `id` (its secrets unlocked first), or a new one when `id` is empty.
    pub fn preview(&self, id: &str, payload: ConnectionPayloadInput) -> Result<ConnectionItem, Error> {
        if id.is_empty() {
            return Ok(to_item(&connection_from_payload(payload, true)?));
        }
        let mut conn = self.state.connections.iter().find(|c| c.id == id).cloned().ok_or(NotFound::Connection)?;
        apply_payload(&mut conn, payload);
        finish(&mut conn, !self.unloaded.contains(id))?;
        Ok(to_item(&conn))
    }

    pub fn update(&mut self, id: &str, payload: ConnectionPayloadInput, policy: SecretsPolicy) -> Result<(), Error> {
        // the secrets are kept in one entry: changing one needs the others
        if payload.password.is_some() || payload.ssh_password.is_some() || payload.ssh_passphrase.is_some() {
            self.unlock_secrets(id)?;
        }
        let mut next = self.state.clone();
        let existing = next.connections.iter_mut().find(|c| c.id == id).ok_or(NotFound::Connection)?;
        apply_payload(existing, payload);
        finish(existing, !self.unloaded.contains(id))?;
        self.commit_with(next, Vaulting::Saving(&[id]), policy)
    }

    /// The secrets of connection `id` that were saved nowhere and aren't known yet this run.
    pub fn missing_secrets(&self, id: &str) -> LeftOut {
        self.state.connections.iter().find(|c| c.id == id).map_or_else(LeftOut::default, |c| c.secrets().missing(c.left_out))
    }

    /// Gives connection `id` the secrets asked of the user, for this run only: they are
    /// never written (a field of `secrets` left empty keeps what the connection has).
    pub fn provide_secrets(&mut self, id: &str, secrets: Secrets) {
        let Some(conn) = self.state.connections.iter_mut().find(|c| c.id == id) else { return };
        let mut known = conn.secrets();
        for (mine, given) in [
            (&mut known.password, secrets.password),
            (&mut known.ssh_password, secrets.ssh_password),
            (&mut known.ssh_passphrase, secrets.ssh_passphrase),
        ] {
            if !given.is_empty() {
                *mine = given;
            }
        }
        conn.set_secrets(known);
    }

    pub fn set_active(&mut self, id: &str) -> Result<(), Error> {
        if self.cloud.iter().any(|c| c.id == id) {
            self.cloud_active = Some(id.to_string());
            return Ok(());
        }
        let mut next = self.state.clone();
        let conn = next.connections.iter_mut().find(|c| c.id == id).ok_or(NotFound::Connection)?;
        conn.last_connected_at = Some(now_ms());
        next.active_connection_id = id.to_string();
        self.commit(next, false)?;
        self.cloud_active = None;
        Ok(())
    }

    /// Leaves no connection active; every saved one is kept.
    pub fn clear_active(&mut self) -> Result<(), Error> {
        self.cloud_active = None;
        let mut next = self.state.clone();
        next.active_connection_id.clear();
        self.commit(next, false)
    }

    /// Removes saved connection `id` (clearing active status if it was
    /// active). Its credential-store entries are left for the caller — run
    /// the returned `SecretsCleanup` off the UI thread.
    pub fn remove(&mut self, id: &str) -> Result<Option<SecretsCleanup>, Error> {
        let stored = self.state.connections.iter().find(|c| c.id == id).ok_or(NotFound::Connection)?;
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
mod tests;
