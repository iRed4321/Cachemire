use std::sync::{OnceLock, Weak};

use super::*;
use crate::connection::store::ConnectionStore;
use crate::connection::ConnectionPayloadInput;
use crate::test_support::temp_dir;

/// A vault that notes, on each call, whether the store it serves was locked then.
#[derive(Clone, Default)]
struct LockWatcher {
    inner: MemoryVault,
    store: Arc<OnceLock<Weak<Mutex<ConnectionStore>>>>,
    calls_while_locked: Arc<std::sync::atomic::AtomicUsize>,
}

impl LockWatcher {
    fn note(&self) {
        let store = self.store.get().and_then(Weak::upgrade).expect("the store");
        if store.try_lock().is_err() {
            self.calls_while_locked.fetch_add(1, Ordering::SeqCst);
        }
    }
}

impl Vault for LockWatcher {
    fn get(&self, id: &str) -> Result<Option<Secrets>, AppError> {
        self.note();
        self.inner.get(id)
    }

    fn set(&self, id: &str, secrets: &Secrets) -> Result<(), AppError> {
        self.note();
        self.inner.set(id, secrets)
    }

    fn delete(&self, id: &str) -> Result<(), AppError> {
        self.note();
        self.inner.delete(id)
    }
}

fn store_with(vault: Arc<dyn Vault>) -> (Arc<Mutex<ConnectionStore>>, std::path::PathBuf) {
    let file = temp_dir("staged").join("databases.json");
    (Arc::new(Mutex::new(ConnectionStore::with_vault(file.clone(), vault))), file)
}

fn with_password(name: &str, password: &str) -> ConnectionPayloadInput {
    ConnectionPayloadInput { name: Some(name.into()), connection_url: Some("redis://cache:6379".into()), password: Some(password.into()), ..Default::default() }
}

#[test]
fn the_vault_is_never_called_while_the_store_is_locked() {
    let watcher = LockWatcher::default();
    let (store, _) = store_with(Arc::new(watcher.clone()));
    watcher.store.set(Arc::downgrade(&store)).unwrap();

    staged(&store, |s| s.create_with_id("c1", with_password("prod", "s3cret"))).unwrap();
    staged(&store, |s| s.unlock_secrets("c1")).unwrap();
    assert_eq!(watcher.calls_while_locked.load(Ordering::SeqCst), 0);
    assert_eq!(watcher.inner.entries.lock_recover()["c1"].password, "s3cret");
}

#[test]
fn a_staged_save_lands_once_in_the_store_the_vault_and_the_file() {
    let vault = MemoryVault::default();
    let (store, file) = store_with(Arc::new(vault.clone()));
    staged(&store, |s| s.create_with_id("c1", with_password("prod", "s3cret"))).unwrap();

    let saved = store.lock_recover().get("c1").unwrap();
    assert_eq!((saved.name.as_str(), saved.password.as_str()), ("prod", "s3cret"));
    assert_eq!(vault.entries.lock_recover().len(), 1);
    let on_disk = std::fs::read_to_string(&file).unwrap();
    assert!(on_disk.contains("prod") && !on_disk.contains("s3cret"), "{on_disk}");
}

#[test]
fn a_secret_read_while_rehearsing_isnt_read_again_for_real() {
    let vault = MemoryVault::default();
    let (store, file) = store_with(Arc::new(vault.clone()));
    staged(&store, |s| s.create_with_id("c1", with_password("prod", "s3cret"))).unwrap();

    // a new run: the secret is only in the vault
    let reopened = Mutex::new(ConnectionStore::with_vault(file, Arc::new(vault.clone())));
    let before = vault.reads.load(Ordering::SeqCst);
    staged(&reopened, |s| s.unlock_secrets("c1")).unwrap();
    assert_eq!(vault.reads.load(Ordering::SeqCst) - before, 1);
    assert_eq!(reopened.lock_recover().get("c1").unwrap().password, "s3cret");
}

#[test]
fn the_store_keeps_its_own_vault_after_a_staged_run() {
    let vault = MemoryVault::default();
    let (store, _) = store_with(Arc::new(vault.clone()));
    staged(&store, |s| s.create_with_id("c1", with_password("prod", "first"))).unwrap();
    // an edit outside `staged` reaches the real vault directly
    store.lock_recover().create_with_id("c2", with_password("staging", "second")).unwrap();
    assert_eq!(vault.entries.lock_recover()["c2"].password, "second");
}

#[test]
fn a_vault_that_fails_while_writing_out_fails_the_real_run_the_same_way() {
    let vault = MemoryVault::default();
    let (store, _) = store_with(Arc::new(vault.clone()));
    vault.broken.store(true, Ordering::SeqCst);
    // no credential store: the password stays usable rather than lost
    staged(&store, |s| s.create_with_id("c1", with_password("prod", "s3cret"))).unwrap();
    assert_eq!(store.lock_recover().get("c1").unwrap().password, "s3cret");
    assert!(vault.entries.lock_recover().is_empty());
}

#[test]
fn a_clean_up_removes_the_entry_and_a_missing_one_is_fine() {
    let vault = MemoryVault::default();
    vault.set("gone", &Secrets { password: "x".into(), ..Default::default() }).unwrap();
    SecretsCleanup::new(Arc::new(vault.clone()), "gone").run();
    SecretsCleanup::new(Arc::new(vault.clone()), "never-there").run();
    assert!(vault.entries.lock_recover().is_empty());
    vault.broken.store(true, Ordering::SeqCst);
    SecretsCleanup::new(Arc::new(vault), "anything").run();
}
