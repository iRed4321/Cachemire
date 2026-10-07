use std::sync::mpsc;

use tokio::runtime::{Builder, Handle};

/// Spawns a background thread hosting a multi-threaded Tokio runtime: Slint's
/// event loop owns the main thread and isn't async, so all Redis I/O runs
/// here, handed back via `slint::invoke_from_event_loop`. Kept alive forever.
pub fn spawn() -> Handle {
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("redis-io".into())
        .spawn(move || {
            // Redis I/O is network-bound and heavy formatting runs on rayon/
            // blocking tasks, so a couple of async workers is plenty; the
            // blocking pool is capped rather than left to grow to 512 threads
            let rt = Builder::new_multi_thread()
                .worker_threads(2)
                .max_blocking_threads(8)
                .enable_all()
                .build()
                .expect("failed to start the redis I/O runtime");
            tx.send(rt.handle().clone())
                .expect("main thread gone before the redis I/O runtime started");
            rt.block_on(std::future::pending::<()>());
        })
        .expect("failed to spawn the redis I/O thread");
    rx.recv().expect("redis I/O thread died before sending its runtime handle")
}
