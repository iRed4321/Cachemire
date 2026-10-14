use std::time::{SystemTime, UNIX_EPOCH};

pub mod aws;
pub mod connection_cache;
pub mod connection_service;
pub mod connection_store;
pub mod error;
pub mod explorer;
pub mod history_store;
pub mod key_name;
pub mod models;
pub mod persist;
pub mod redis_client;
pub mod redis_url;
pub mod redis_value_type;
pub mod redisql;
pub mod runtime;
pub mod secret_vault;
pub mod server_info;
pub mod session;
pub mod settings_store;
pub mod ssh_tunnel;
pub mod state;
pub mod updates;

pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `lock()` for a mutex whose holder may have panicked: a poisoned lock
/// would otherwise panic every later use, including the UI thread's, over
/// one failed background task. Locked data is only ever replaced as a whole.
pub trait LockExt<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T>;
}

impl<T> LockExt<T> for std::sync::Mutex<T> {
    fn lock_recover(&self) -> std::sync::MutexGuard<'_, T> {
        self.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::LockExt;
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_lock_whose_holder_panicked_can_still_be_taken() {
        let shared = Arc::new(Mutex::new(vec![1, 2, 3]));
        let holder = shared.clone();
        let _ = std::thread::spawn(move || {
            let _guard = holder.lock().unwrap();
            panic!("a background task failing while it holds the lock");
        })
        .join();
        assert!(shared.lock().is_err(), "the lock is poisoned");
        assert_eq!(*shared.lock_recover(), vec![1, 2, 3], "and taken anyway, with its data");
    }
}

