//! The UI thread's state, in one place: built once in `main`, then handed to each
//! module's `init`. Work coming back from Tokio (whose closures must be `Send`, so
//! can't hold it) reaches it again with [`later`].

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use tokio::runtime::Handle;
use tokio::task::AbortHandle;

use crate::MainWindow;
use crate::cloud::Cloud;
use crate::connections::transfer::TransferLists;
use crate::key_detail::KeyDetail;
use crate::keyspace::Keyspace;
use crate::redisql::QueryTabs;
use crate::search::SearchTabs;
use crate::tabs::Tabs;
use cachemire_core::error::{Error, SystemError};
use cachemire_core::state::AppState;

pub struct App {
    pub window: slint::Weak<MainWindow>,
    pub state: Arc<AppState>,
    pub rt: Handle,
    /// the open tabs of each connection, and which is shown
    pub tabs: RefCell<Tabs>,
    /// what each search tab and query tab holds
    pub searches: RefCell<SearchTabs>,
    pub queries: RefCell<QueryTabs>,
    /// the rows of the import/export dialog
    pub transfer: RefCell<TransferLists>,
    /// the cloud account's sign-in
    pub cloud: RefCell<Cloud>,
    /// the key panel: shows a key or a query's rows
    pub key_detail: KeyDetail,
    /// the keyspace tree
    pub keyspace: Keyspace,
}

thread_local! {
    static APP: OnceCell<Rc<App>> = const { OnceCell::new() };
}

/// Makes `app` the one [`with`] gives; called once, before the window runs.
pub fn install(app: App) -> Rc<App> {
    let app = Rc::new(app);
    APP.with(|cell| assert!(cell.set(app.clone()).is_ok(), "the app is installed once"));
    app
}

/// Runs `f` on the app and its window, if the window is still open. UI thread only:
/// from a callback, or a closure `slint::invoke_from_event_loop` runs.
pub fn with(f: impl FnOnce(&Rc<App>, &MainWindow)) {
    let Some(app) = APP.with(|cell| cell.get().cloned()) else { return };
    if let Some(window) = app.window.upgrade() {
        f(&app, &window);
    }
}

/// [`with`] from any thread: `f` runs on the UI thread once its event loop gets to it
/// (dropped if the loop has ended), such as a Tokio task's result going back to the window.
pub fn later(f: impl FnOnce(&Rc<App>, &MainWindow) + Send + 'static) {
    let _ = slint::invoke_from_event_loop(move || with(f));
}

/// Runs `task` on the runtime, then `then` with what it gave as [`later`] would; a panic
/// of the task reaches `then` as an error. Aborting the task (the handle) skips `then`.
pub fn spawn<T: Send + 'static>(
    rt: &Handle,
    task: impl Future<Output = T> + Send + 'static,
    then: impl FnOnce(&Rc<App>, &MainWindow, Result<T, Error>) + Send + 'static,
) -> AbortHandle {
    let task = rt.spawn(task);
    let abort = task.abort_handle();
    rt.spawn(async move {
        let outcome = match task.await {
            Ok(value) => Ok(value),
            Err(e) if e.is_panic() => Err(Error::from(e)),
            Err(_) => return,
        };
        later(move |app, window| then(app, window, outcome));
    });
    abort
}

/// [`spawn`] for blocking work, run on the runtime's blocking threads.
pub fn spawn_blocking<T: Send + 'static>(
    rt: &Handle,
    work: impl FnOnce() -> T + Send + 'static,
    then: impl FnOnce(&Rc<App>, &MainWindow, Result<T, Error>) + Send + 'static,
) {
    rt.spawn_blocking(move || {
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work)).map_err(|panic| {
            let why = panic.downcast_ref::<&str>().map(|why| why.to_string()).or_else(|| panic.downcast_ref::<String>().cloned());
            Error::from(SystemError::Panicked(why.unwrap_or_default()))
        });
        later(move |app, window| then(app, window, outcome));
    });
}
