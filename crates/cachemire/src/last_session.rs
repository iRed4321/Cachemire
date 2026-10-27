//! The connection the app was closed on, with its tabs and the folders open in its
//! tree (`last_session.json`, next to the settings): written at exit and reopened at
//! the next start when the settings ask for it.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::MainWindow;
use crate::app::App;
use cachemire_core::aws;
use cachemire_core::persist::{self, Loaded};
use cachemire_core::state::AppState;

const FILE: &str = "last_session.json";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum SavedTab {
    Key { name: String },
    Query { text: String },
    Search { query: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LastSession {
    connection: String,
    // the AWS profile a cloud connection is listed under ("" for a saved connection)
    #[serde(default)]
    cloud_profile: String,
    #[serde(default)]
    tabs: Vec<SavedTab>,
    // the index in `tabs` of the active one
    #[serde(default)]
    active: Option<usize>,
    #[serde(default)]
    open_folders: Vec<String>,
}

fn path(state: &AppState) -> PathBuf {
    state.settings.lock().file_path().with_file_name(FILE)
}

/// Drops what was kept of the last session.
pub fn forget(state: &AppState) {
    if let Err(e) = std::fs::remove_file(path(state))
        && e.kind() != std::io::ErrorKind::NotFound
    {
        tracing::warn!("can't remove {FILE}: {e}");
    }
}

/// At exit: keeps the connection in use, its tabs and its open folders, when the
/// settings ask for it. With none in use (a restore still under way, a failed
/// attempt), what was kept stays; a disconnect drops it ([`forget`]).
pub fn save(app: &App, window: &MainWindow) {
    if !app.state.settings.lock().restore_session() {
        forget(&app.state);
        return;
    }
    let connection = window.connection_id().to_string();
    if connection.is_empty() {
        return;
    }
    let (open, active) = crate::tabs::open_tabs(app, window);
    let tabs: Vec<SavedTab> = open
        .iter()
        .map(|id| {
            if crate::search::is_search_id(id) {
                SavedTab::Search { query: app.searches.borrow().query(id).unwrap_or_default().to_string() }
            } else if crate::redisql::is_query_id(id) {
                SavedTab::Query { text: app.queries.borrow().text(id).unwrap_or_default().to_string() }
            } else {
                SavedTab::Key { name: id.clone() }
            }
        })
        .collect();
    let cloud_profile = if connection.starts_with(aws::ID_PREFIX) { app.cloud.borrow().profile().to_string() } else { String::new() };
    let session = LastSession {
        active: active.and_then(|active| open.iter().position(|id| *id == active)),
        open_folders: crate::keyspace::open_folders(app),
        connection,
        cloud_profile,
        tabs,
    };
    if let Err(e) = persist::save(&path(&app.state), &session) {
        tracing::warn!("can't save the last session: {e}");
    }
}

/// At start, when the settings ask for it: the last session's tabs and folders wait
/// for its connection, which is connected to (a cloud one once signed in and listed).
pub fn restore(app: &App, window: &MainWindow) {
    if !app.state.settings.lock().restore_session() {
        return;
    }
    let session = match persist::load::<LastSession>(&path(&app.state)) {
        Loaded::Read(session) => session,
        Loaded::Missing => return,
        Loaded::Unusable(why) => {
            tracing::warn!("{FILE} can't be read ({why})");
            return;
        }
    };
    let is_cloud = session.connection.starts_with(aws::ID_PREFIX);
    if !is_cloud && app.state.connections.lock().get(&session.connection).is_none() {
        return;
    }
    let open: Vec<String> = session
        .tabs
        .into_iter()
        .map(|tab| match tab {
            SavedTab::Key { name } => name,
            SavedTab::Query { text } => app.queries.borrow_mut().create(text),
            SavedTab::Search { query } => app.searches.borrow_mut().create(query),
        })
        .collect();
    let active = session.active.and_then(|i| open.get(i).cloned()).or_else(|| open.first().cloned());
    app.tabs.borrow_mut().park(&session.connection, open, active);
    crate::keyspace::park_folders(app, &session.connection, session.open_folders);
    tracing::info!(connection = %session.connection, "restoring the last session");
    if is_cloud {
        crate::cloud::resume(app, window, session.cloud_profile, session.connection);
    } else {
        crate::connect_to(window, app.state.clone(), &app.rt, session.connection, true);
    }
}
