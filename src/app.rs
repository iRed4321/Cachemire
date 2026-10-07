//! The UI thread's state, in one place: built once in `main`, then handed to each
//! module's `init`. Work coming back from Tokio (whose closures must be `Send`, so
//! can't hold it) reaches it again with [`with`].

use std::cell::{OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use tokio::runtime::Handle;

use crate::backend::state::AppState;
use crate::cloud::Cloud;
use crate::connection_transfer::TransferLists;
use crate::key_detail::KeyDetail;
use crate::keyspace::Keyspace;
use crate::redisql::QueryTabs;
use crate::search::SearchTabs;
use crate::tabs::Tabs;
use crate::MainWindow;

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
