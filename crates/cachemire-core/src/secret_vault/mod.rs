//! Where saved connections' passwords/passphrases live: the OS credential
//! store (Windows Credential Manager, or Secret Service on Linux), so `databases.json` doesn't hold them in clear text.

use parking_lot::Mutex;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};

/// The credential store failed; cloneable, since `staged` replays its answers.
#[derive(Debug, Clone, thiserror::Error)]
pub enum VaultError {
    #[error(transparent)]
    Keyring(Arc<keyring_core::Error>),
    #[error("the saved secrets can't be read: {0}")]
    Json(Arc<serde_json::Error>),
    #[error("no credential store")]
    Unavailable,
}

/// A read from the credential store failed, ahead of what needed the secrets.
#[derive(Debug, thiserror::Error)]
pub enum KeyringError {
    #[error(transparent)]
    Vault(#[from] VaultError),
    #[error("Couldn't read the saved passwords from the system keyring: {0}")]
    ConnectionPasswords(#[source] VaultError),
    #[error("Couldn't read the SSH profile's password from the system keyring: {0}")]
    SshProfilePassword(#[source] VaultError),
    #[error("Couldn't read the SSH tunnel's secrets from the system keyring: {0}")]
    TunnelSecrets(#[source] VaultError),
}

/// The name the entries are filed under in the credential store.
const SERVICE: &str = "Cachemire";

/// The app's former name, which older entries may still be filed under: read when
/// [`SERVICE`] holds nothing for an id, then moved over to it.
const FORMER_SERVICE: &str = "Redust";

/// Everything secret about one saved connection. Stored as one entry (named
/// after the connection's id) so that a connection costs one lookup.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Secrets {
    pub password: String,
    pub ssh_password: String,
    pub ssh_passphrase: String,
}

impl Secrets {
    pub fn is_empty(&self) -> bool {
        self.password.is_empty() && self.ssh_password.is_empty() && self.ssh_passphrase.is_empty()
    }
}

pub trait Vault: Send + Sync {
    /// `Ok(None)` when nothing is stored for `id`; `Err` when the credential
    /// store itself can't be used (none running, locked and refused, …).
    fn get(&self, id: &str) -> Result<Option<Secrets>, VaultError>;
    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), VaultError>;
    /// Removes the entry for `id`; not having one is fine.
    fn delete(&self, id: &str) -> Result<(), VaultError>;
}

/// What is left in the credential store after something that had secrets in
/// it was removed. Waiting on the store can take a while (it may be locked), so
/// this is handed back to run off the UI thread.
pub struct SecretsCleanup {
    vault: std::sync::Arc<dyn Vault>,
    id: String,
}

impl SecretsCleanup {
    pub fn new(vault: std::sync::Arc<dyn Vault>, id: &str) -> Self {
        Self { vault, id: id.to_string() }
    }

    /// Best effort: a locked or missing store leaves the entry behind, which
    /// nothing refers to any more.
    pub fn run(self) {
        let _ = self.vault.delete(&self.id);
    }
}

/// A store holding secrets in a `Vault`, which `staged` can run operations on
/// without keeping its lock through the vault's calls.
pub trait VaultBacked: Clone {
    fn vault_mut(&mut self) -> &mut Arc<dyn Vault>;
    /// While set, saving changes nothing on disk.
    fn set_dry_run(&mut self, dry_run: bool);
}

/// A write noted while rehearsing: the id, its secrets, and once made, how it went.
type NotedWrite = (String, Secrets, Option<Result<(), VaultError>>);

/// The vault as `staged` sees it: while rehearsing, reads go through to `real`
/// and writes are only noted; once they're done, the same calls get the
/// answers they had, and any other call goes to `real`.
struct Recorded {
    real: Arc<dyn Vault>,
    rehearsing: AtomicBool,
    reads: Mutex<FxHashMap<String, Result<Option<Secrets>, VaultError>>>,
    writes: Mutex<Vec<NotedWrite>>,
}

impl Recorded {
    /// Makes the writes noted while rehearsing, up to the first that fails
    /// (the store makes no more attempts after one).
    fn write_out(&self) {
        let mut writes = self.writes.lock();
        for (id, secrets, result) in writes.iter_mut() {
            let done = self.real.set(id, secrets);
            let failed = done.is_err();
            *result = Some(done);
            if failed {
                break;
            }
        }
        self.rehearsing.store(false, Ordering::SeqCst);
    }
}

impl Vault for Recorded {
    fn get(&self, id: &str) -> Result<Option<Secrets>, VaultError> {
        if let Some(read) = self.reads.lock().get(id) {
            return read.clone();
        }
        let read = self.real.get(id);
        if self.rehearsing.load(Ordering::SeqCst) {
            self.reads.lock().insert(id.to_string(), read.clone());
        }
        read
    }

    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), VaultError> {
        let mut writes = self.writes.lock();
        if self.rehearsing.load(Ordering::SeqCst) {
            writes.push((id.to_string(), secrets.clone(), None));
            return Ok(());
        }
        let done = writes.iter().find(|(i, s, result)| i == id && s == secrets && result.is_some());
        match done {
            Some((_, _, result)) => result.clone().unwrap_or(Ok(())),
            None => self.real.set(id, secrets),
        }
    }

    fn delete(&self, id: &str) -> Result<(), VaultError> {
        self.real.delete(id)
    }
}

/// Runs `op` on the store in `store` with its vault calls made while the lock
/// is free: `op` is rehearsed on a copy first, then run for real, replaying
/// what the vault answered. `op` must do the same thing both times (no fresh ids).
pub fn staged<S: VaultBacked, R>(store: &Mutex<S>, op: impl Fn(&mut S) -> R) -> R {
    let mut rehearsal = store.lock().clone();
    let recorded = Arc::new(Recorded {
        real: rehearsal.vault_mut().clone(),
        rehearsing: AtomicBool::new(true),
        reads: Mutex::default(),
        writes: Mutex::default(),
    });
    *rehearsal.vault_mut() = recorded.clone();
    rehearsal.set_dry_run(true);
    let _ = op(&mut rehearsal);
    drop(rehearsal);
    recorded.write_out();

    let mut store = store.lock();
    let real = std::mem::replace(store.vault_mut(), recorded);
    let result = op(&mut store);
    *store.vault_mut() = real;
    result
}

fn keyring_error(error: keyring_core::Error) -> VaultError {
    VaultError::Keyring(Arc::new(error))
}

/// The platform's store, the one `keyring` 4 used, so what it saved is found again:
/// the Credential Manager on Windows, the Secret Service on Linux.
#[cfg(windows)]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(windows_native_keyring_store::Store::new()?)
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Ok(zbus_secret_service_keyring_store::Store::new()?)
}

#[cfg(not(any(windows, all(unix, not(any(target_os = "macos", target_os = "ios", target_os = "android"))))))]
fn platform_store() -> keyring_core::Result<Arc<keyring_core::CredentialStore>> {
    Err(keyring_core::Error::NoDefaultStore)
}

/// Made `keyring-core`'s default store the first time an entry is needed; why not, if it can't be.
static STORE: LazyLock<Result<(), Arc<keyring_core::Error>>> =
    LazyLock::new(|| platform_store().map(keyring_core::set_default_store).map_err(Arc::new));

fn entry(service: &str, id: &str) -> Result<keyring_core::Entry, VaultError> {
    STORE.as_ref().map_err(|e| VaultError::Keyring(e.clone()))?;
    keyring_core::Entry::new(service, id).map_err(keyring_error)
}

fn json_error(error: serde_json::Error) -> VaultError {
    VaultError::Json(Arc::new(error))
}

/// The operating system's credential store.
pub struct OsVault;

impl OsVault {
    fn read(service: &str, id: &str) -> Result<Option<Secrets>, VaultError> {
        match entry(service, id)?.get_password() {
            Ok(json) => serde_json::from_str(&json).map(Some).map_err(json_error),
            Err(keyring_core::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error(e)),
        }
    }

    fn remove(service: &str, id: &str) -> Result<(), VaultError> {
        match entry(service, id)?.delete_credential() {
            Ok(()) | Err(keyring_core::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_error(e)),
        }
    }
}

impl Vault for OsVault {
    fn get(&self, id: &str) -> Result<Option<Secrets>, VaultError> {
        if let Some(secrets) = Self::read(SERVICE, id)? {
            return Ok(Some(secrets));
        }
        let former = Self::read(FORMER_SERVICE, id)?;
        // the former entry goes once the copy is stored; otherwise it is read again next time
        if let Some(secrets) = &former
            && self.set(id, secrets).is_ok()
        {
            let _ = Self::remove(FORMER_SERVICE, id);
        }
        Ok(former)
    }

    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), VaultError> {
        let json = serde_json::to_string(secrets).map_err(json_error)?;
        entry(SERVICE, id)?.set_password(&json).map_err(keyring_error)
    }

    fn delete(&self, id: &str) -> Result<(), VaultError> {
        Self::remove(SERVICE, id)?;
        Self::remove(FORMER_SERVICE, id)
    }
}

/// An in-memory stand-in for tests, which must never touch the real store;
/// it can be made to fail like a machine with no credential store.
#[cfg(any(test, feature = "test-support"))]
#[derive(Clone, Default)]
pub struct MemoryVault {
    pub entries: std::sync::Arc<parking_lot::Mutex<rustc_hash::FxHashMap<String, Secrets>>>,
    pub broken: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// how many times the vault was asked for something
    pub reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(any(test, feature = "test-support"))]
impl MemoryVault {
    fn check(&self) -> Result<(), VaultError> {
        if self.broken.load(std::sync::atomic::Ordering::SeqCst) { Err(VaultError::Unavailable) } else { Ok(()) }
    }
}

#[cfg(any(test, feature = "test-support"))]
impl Vault for MemoryVault {
    fn get(&self, id: &str) -> Result<Option<Secrets>, VaultError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.check()?;
        Ok(self.entries.lock().get(id).cloned())
    }

    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), VaultError> {
        self.check()?;
        self.entries.lock().insert(id.to_string(), secrets.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), VaultError> {
        self.check()?;
        self.entries.lock().remove(id);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
