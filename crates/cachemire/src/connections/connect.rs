//! Connecting: the one place the connection the app is on changes. The title bar's view
//! is set, the store's active connection follows it, then the tabs, in that order.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use rust_i18n::t;
use slint::ComponentHandle;
use tokio::runtime::Handle;

use crate::app::App;
use crate::error::Localize;
use crate::{ConnectionState, ConnectionStatus, ConnectionView, MainWindow};
use cachemire_core::connection::SshSettings;
use cachemire_core::error::ErrorKind;
use cachemire_core::state::AppState;

/// Connects to a saved connection by id off the UI thread and shows the outcome in
/// the title bar; `announce` also raises `connection-established` on success. Only
/// the latest attempt counts: a superseded one's outcome is dropped.
pub(crate) fn connect_to(window: &MainWindow, state: Arc<AppState>, rt: &Handle, connection_id: String, announce: bool) {
    connect_then(window, state, rt, connection_id, announce, || {});
}

/// [`connect_to`], with `then` run on the UI thread once connected. A connection saved
/// without some of its passwords asks for them first.
fn connect_then(
    window: &MainWindow,
    state: Arc<AppState>,
    rt: &Handle,
    connection_id: String,
    announce: bool,
    then: impl FnOnce() + Send + 'static,
) {
    let (missing, name) = {
        let store = state.connections.lock();
        (store.missing_secrets(&connection_id), store.get(&connection_id).map(|c| c.name).unwrap_or_default())
    };
    if !missing.is_empty() {
        let asked_id = connection_id.clone();
        let resume = Box::new(move |app: &Rc<App>, window: &MainWindow| {
            connect_then(window, state, &app.rt, connection_id.clone(), announce, then);
        });
        crate::app::with(|app, window| crate::keyring_dialogs::ask_secrets(app, window, &asked_id, &name, missing, resume));
        return;
    }
    let attempt = state.connect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
    crate::app::with(|app, _| connection_id.clone_into(&mut app.picked.borrow_mut()));
    let connection = window.global::<ConnectionState>();
    connection.set_view(ConnectionView { connecting: true, ..connection.get_view() });
    let (task_state, id) = (state.clone(), connection_id.clone());
    let connect = async move {
        // (the keyring may ask for its password first: only opening the connection is timed)
        let outcome = cachemire_core::connection::connect_connection(&task_state, &id).await;
        // resolved from the saved connection itself, not the attempt's outcome, so the
        // status glyph can still show its lock when this connection uses SSH but failed
        let ssh_via = task_state.connections.lock().get(&id).and_then(|c| c.ssh.as_ref().map(ssh_server_label)).unwrap_or_default();
        (outcome, ssh_via)
    };
    crate::app::spawn(rt, connect, move |app, window, done| {
        // (checked here, on the UI thread, where the attempts are numbered: nothing
        // can start between this check and the connection being made the active one)
        if state.connect_attempt.load(Ordering::SeqCst) != attempt {
            return;
        }
        let (outcome, ssh_via) = done.unwrap_or_else(|e| (Err(e), String::new()));
        let outcome = outcome.and_then(|c| {
            state.connections.lock().set_active(&connection_id)?;
            Ok(c)
        });
        // ssh_failed marks the SSH tunnel itself as the failure (not the Redis endpoint
        // past it): the SSH badge colors red for that, not the dot
        let view = match outcome {
            Ok(c) => {
                tracing::info!(name = %c.name, address = %c.address(), "connected");
                ConnectionView {
                    connected: true,
                    status: ConnectionStatus::Connected,
                    connection_id: connection_id.into(),
                    address: c.address().into(),
                    name: c.name.into(),
                    ssh_via: ssh_via.into(),
                    ..Default::default()
                }
            }
            Err(e) => {
                tracing::warn!(connection = %connection_id, kind = ?e.kind(), "couldn't connect: {e}");
                ConnectionView {
                    status: ConnectionStatus::Disconnected,
                    status_detail: e.localized().into(),
                    ssh_failed: e.kind() == ErrorKind::Tunnel,
                    ssh_via: ssh_via.into(),
                    ..Default::default()
                }
            }
        };
        let connected = view.connected;
        show(app, window, view);
        if connected && announce {
            window.global::<ConnectionState>().invoke_established();
        }
        if connected {
            then();
        }
        crate::tabs::follow_connection(app, window);
    });
}

/// Puts `view` in the title bar. With no connection in it, none stays active in the
/// store either, so whatever falls back to the active connection finds none.
fn show(app: &Rc<App>, window: &MainWindow, view: ConnectionView) {
    if view.connection_id.is_empty()
        && let Err(e) = app.state.connections.lock().clear_active()
    {
        tracing::warn!("can't save that no connection is active: {e}");
    }
    window.global::<ConnectionState>().set_view(view);
    crate::connections::profiles::apply_tint(window, &app.state);
}

/// Makes the store's active connection the one in use again, after something else
/// (a connection created from the form) made another one active.
pub(crate) fn keep_active_in_step(app: &App, window: &MainWindow) {
    let id = window.connection_id();
    let mut store = app.state.connections.lock();
    if store.active_connection_id() == id.as_str() {
        return;
    }
    let synced = if id.is_empty() { store.clear_active() } else { store.set_active(&id) };
    if let Err(e) = synced {
        tracing::warn!("can't save which connection is active: {e}");
    }
}

/// `user@host` (with `:port` when it isn't 22) of an SSH tunnel's server.
fn ssh_server_label(ssh: &SshSettings) -> String {
    match ssh.port {
        22 => format!("{}@{}", ssh.username, ssh.host),
        port => format!("{}@{}:{port}", ssh.username, ssh.host),
    }
}

/// Connects again to the connection last picked (re-verifying the one in use). Never
/// called at startup, where only a restored last session connects (see `last_session`).
pub(crate) fn connect_active(window: &MainWindow, state: Arc<AppState>, rt: &Handle) {
    connect_active_then(window, state, rt, || {});
}

/// [`connect_active`], with `then` run once connected: the keyspace refresh button's
/// new walk waits for a stale connection to be found and opened again.
pub(crate) fn connect_active_then(window: &MainWindow, state: Arc<AppState>, rt: &Handle, then: impl FnOnce() + Send + 'static) {
    let mut picked = None;
    crate::app::with(|app, _| picked = Some(app.picked.borrow().clone()));
    let picked = picked.filter(|id| state.connections.lock().get(id).is_some());
    match picked {
        Some(id) => connect_then(window, state, rt, id, false, then),
        None => {
            // nothing to connect to: an attempt still under way is no longer wanted
            state.connect_attempt.fetch_add(1, Ordering::SeqCst);
            let view = ConnectionView {
                status: ConnectionStatus::None,
                status_detail: t!("No saved connection").as_ref().into(),
                ..Default::default()
            };
            crate::app::with(|app, window| show(app, window, view));
        }
    }
}
