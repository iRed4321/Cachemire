//! Constructors shared by the tests of more than one module.

#![cfg(test)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

/// A new empty directory under the system's temp directory, unique to this call.
pub fn temp_dir(name: &str) -> PathBuf {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("cachemire-test-{name}-{}-{}-{n}", std::process::id(), crate::backend::now_ms()));
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

/// Runs `future` to its end on a runtime of its own.
pub fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread().enable_all().build().expect("test runtime").block_on(future)
}
