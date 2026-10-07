//! Where saved connections' passwords/passphrases live: the OS credential
//! store (Windows Credential Manager, or Secret Service on Linux), so `databases.json` doesn't hold them in clear text.

use super::LockExt;
use serde::{Deserialize, Serialize};
use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::error::AppError;

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
    fn get(&self, id: &str) -> Result<Option<Secrets>, AppError>;
    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), AppError>;
    /// Removes the entry for `id`; not having one is fine.
    fn delete(&self, id: &str) -> Result<(), AppError>;
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
type NotedWrite = (String, Secrets, Option<Result<(), AppError>>);

/// The vault as `staged` sees it: while rehearsing, reads go through to `real`
/// and writes are only noted; once they're done, the same calls get the
/// answers they had, and any other call goes to `real`.
struct Recorded {
    real: Arc<dyn Vault>,
    rehearsing: AtomicBool,
    reads: Mutex<FxHashMap<String, Result<Option<Secrets>, AppError>>>,
    writes: Mutex<Vec<NotedWrite>>,
}

impl Recorded {
    /// Makes the writes noted while rehearsing, up to the first that fails
    /// (the store makes no more attempts after one).
    fn write_out(&self) {
        let mut writes = self.writes.lock_recover();
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
    fn get(&self, id: &str) -> Result<Option<Secrets>, AppError> {
        if let Some(read) = self.reads.lock_recover().get(id) {
            return read.clone();
        }
        let read = self.real.get(id);
        if self.rehearsing.load(Ordering::SeqCst) {
            self.reads.lock_recover().insert(id.to_string(), read.clone());
        }
        read
    }

    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), AppError> {
        let mut writes = self.writes.lock_recover();
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

    fn delete(&self, id: &str) -> Result<(), AppError> {
        self.real.delete(id)
    }
}

/// Runs `op` on the store in `store` with its vault calls made while the lock
/// is free: `op` is rehearsed on a copy first, then run for real, replaying
/// what the vault answered. `op` must do the same thing both times (no fresh ids).
pub fn staged<S: VaultBacked, R>(store: &Mutex<S>, op: impl Fn(&mut S) -> R) -> R {
    let mut rehearsal = store.lock_recover().clone();
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

    let mut store = store.lock_recover();
    let real = std::mem::replace(store.vault_mut(), recorded);
    let result = op(&mut store);
    *store.vault_mut() = real;
    result
}

fn keyring_error(error: keyring::Error) -> AppError {
    AppError::Keyring(error.to_string())
}

/// The operating system's credential store.
pub struct OsVault;

impl OsVault {
    fn read(service: &str, id: &str) -> Result<Option<Secrets>, AppError> {
        let entry = keyring::Entry::new(service, id).map_err(keyring_error)?;
        match entry.get_password() {
            Ok(json) => serde_json::from_str(&json).map(Some).map_err(|e| AppError::Keyring(e.to_string())),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(e) => Err(keyring_error(e)),
        }
    }

    fn remove(service: &str, id: &str) -> Result<(), AppError> {
        match keyring::Entry::new(service, id).and_then(|entry| entry.delete_credential()) {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(e) => Err(keyring_error(e)),
        }
    }
}

impl Vault for OsVault {
    fn get(&self, id: &str) -> Result<Option<Secrets>, AppError> {
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

    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), AppError> {
        let json = serde_json::to_string(secrets).map_err(|e| AppError::Keyring(e.to_string()))?;
        keyring::Entry::new(SERVICE, id).and_then(|entry| entry.set_password(&json)).map_err(keyring_error)
    }

    fn delete(&self, id: &str) -> Result<(), AppError> {
        Self::remove(SERVICE, id)?;
        Self::remove(FORMER_SERVICE, id)
    }
}

/// An in-memory stand-in for tests, which must never touch the real store;
/// it can be made to fail like a machine with no credential store.
#[cfg(test)]
#[derive(Clone, Default)]
pub struct MemoryVault {
    pub entries: std::sync::Arc<std::sync::Mutex<rustc_hash::FxHashMap<String, Secrets>>>,
    pub broken: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// how many times the vault was asked for something
    pub reads: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[cfg(test)]
impl MemoryVault {
    fn check(&self) -> Result<(), AppError> {
        if self.broken.load(std::sync::atomic::Ordering::SeqCst) { Err(AppError::Keyring("no credential store".to_string())) } else { Ok(()) }
    }
}

#[cfg(test)]
impl Vault for MemoryVault {
    fn get(&self, id: &str) -> Result<Option<Secrets>, AppError> {
        self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        self.check()?;
        Ok(self.entries.lock_recover().get(id).cloned())
    }

    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), AppError> {
        self.check()?;
        self.entries.lock_recover().insert(id.to_string(), secrets.clone());
        Ok(())
    }

    fn delete(&self, id: &str) -> Result<(), AppError> {
        self.check()?;
        self.entries.lock_recover().remove(id);
        Ok(())
    }
}
