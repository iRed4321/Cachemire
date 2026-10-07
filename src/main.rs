// Release builds on Windows shouldn't drag a console window along with the
// GUI; debug builds keep it so eprintln! diagnostics stay visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use crate::backend::LockExt;
use rust_i18n::t;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

rust_i18n::i18n!("locales", fallback = "en");

mod app;
mod backend;
mod cloud;
mod cloud_settings;
mod connection_transfer;
mod export_files;
mod i18n;
mod connection_profiles;
mod connections;
mod json_path;
mod key_detail;
mod keyspace;
mod queries;
mod query_complete;
mod redisql;
mod redisql_highlight;
mod search;
mod server_stats;
mod settings;
mod table_text;
mod tabs;
mod theme_colors;
mod toast;
mod value_actions;
mod window_state;

use backend::error::AppError;
use backend::state::AppState;

slint::include_modules!();

impl MainWindow {
    /// The saved-connection id of the connection in use ("" when none is).
    pub fn connection_id(&self) -> slint::SharedString {
        self.global::<ConnectionState>().get_view().connection_id
    }
}

/// An error as the message a window property shows.
impl From<AppError> for slint::SharedString {
    fn from(error: AppError) -> Self {
        error.message().into()
    }
}

/// How long an attempt to connect gets before it is reported as failed.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Connects to a saved connection by id off the UI thread and shows the outcome in
/// the title bar; `announce` also raises `connection-established` on success. Only
/// the latest attempt counts: a superseded one's outcome is dropped.
pub(crate) fn connect_to(
    window: &MainWindow,
    state: Arc<AppState>,
    rt: &tokio::runtime::Handle,
    connection_id: String,
    announce: bool,
) {
    let attempt = state.connect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
    let connection = window.global::<ConnectionState>();
    connection.set_view(ConnectionView { connecting: true, ..connection.get_view() });
    let weak = window.as_weak();
    rt.spawn(async move {
        let outcome = match tokio::time::timeout(CONNECT_TIMEOUT, backend::connection_service::connect_connection(&state, &connection_id)).await {
            Ok(outcome) => outcome,
            Err(_) => Err(AppError::Timeout(t!("Connection timed out after %{seconds} seconds.", seconds = CONNECT_TIMEOUT.as_secs()).into_owned())),
        };
        // resolved from the saved connection itself, not the attempt's outcome,
        // so the status glyph can still show its lock (see TitleBar) when this
        // connection uses SSH but the attempt just failed
        let ssh_via = state
            .connections
            .lock_recover()
            .get(&connection_id)
            .and_then(|c| c.ssh.as_ref().map(ssh_server_label))
            .unwrap_or_default();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(window) = weak.upgrade() else { return };
            // (checked here, on the UI thread, where the attempts are numbered: nothing
            // can start between this check and the connection being made the active one)
            if state.connect_attempt.load(Ordering::SeqCst) != attempt {
                return;
            }
            // status/detail drive the title bar's dot + tooltip; ssh_failed
            // marks the SSH tunnel itself as the failure (not the Redis endpoint
            // past it) — the SSH badge colors red for that, not the dot
            let outcome = outcome.and_then(|c| {
                state.connections.lock_recover().set_active(&connection_id)?;
                Ok(c)
            });
            let view = match outcome {
                Ok(c) => ConnectionView {
                    connected: true,
                    status: ConnectionStatus::Connected,
                    connection_id: connection_id.into(),
                    address: c.address().into(),
                    name: c.name.into(),
                    ssh_via: ssh_via.into(),
                    ..Default::default()
                },
                Err(e) => ConnectionView {
                    status: ConnectionStatus::Disconnected,
                    status_detail: e.to_string().into(),
                    ssh_failed: matches!(e, AppError::Tunnel(_)),
                    ssh_via: ssh_via.into(),
                    ..Default::default()
                },
            };
            let connected = view.connected;
            window.global::<ConnectionState>().set_view(view);
            connection_profiles::apply_tint(&window, &state);
            if connected && announce {
                window.global::<ConnectionState>().invoke_established();
            }
        });
    });
}

/// `user@host` (with `:port` when it isn't 22) of an SSH tunnel's server.
fn ssh_server_label(ssh: &backend::models::SshSettings) -> String {
    match ssh.port {
        22 => format!("{}@{}", ssh.username, ssh.host),
        port => format!("{}@{}:{port}", ssh.username, ssh.host),
    }
}

/// Reconnects/re-verifies the store's active connection — used by the
/// keyspace refresh button. Unlike `connect_to`, never auto-called at
/// startup: the app starts with nothing connected.
pub(crate) fn connect_active(window: &MainWindow, state: Arc<AppState>, rt: &tokio::runtime::Handle) {
    let connection_id = state.connections.lock_recover().get_active().ok().map(|c| c.id);
    match connection_id {
        Some(id) => connect_to(window, state, rt, id, false),
        None => {
            // nothing to connect to: an attempt still under way is no longer wanted
            state.connect_attempt.fetch_add(1, Ordering::SeqCst);
            let view = ConnectionView { status: ConnectionStatus::None, status_detail: t!("No saved connection").as_ref().into(), ..Default::default() };
            window.global::<ConnectionState>().set_view(view);
            connection_profiles::apply_tint(window, &state);
        }
    }
}

/// Selects the Slint backend before any window exists. Windows defaults to
/// the software renderer (the OpenGL path presents unreliably there — stale
/// buffers, stalls during DWM animations) and `with_transparent(false)`.
fn select_backend() -> Result<(), slint::PlatformError> {
    let selector = slint::BackendSelector::new();
    #[cfg(target_os = "windows")]
    let selector = {
        let selector = selector.with_winit_window_attributes_hook(|attrs| attrs.with_transparent(false));
        if std::env::var_os("SLINT_BACKEND").is_none() {
            selector.backend_name("winit".into()).renderer_name("software".into())
        } else {
            selector
        }
    };
    selector.select()
}

fn main() -> Result<(), slint::PlatformError> {
    // Wayland has no per-window icon: the shell finds it through the app id,
    // as the icon of the cachemire.desktop entry (assets/cachemire.desktop)
    select_backend()?;
    slint::set_xdg_app_id("cachemire")?;
    let window = MainWindow::new()?;
    let save_window_state = window_state::install(&window);

    toast::init(&window);
    window.global::<About>().set_version(env!("CARGO_PKG_VERSION").into());

    window.on_request_drag_window({
        let weak = window.as_weak();
        move || {
            let Some(window) = weak.upgrade() else { return };
            window.window().with_winit_window(|winit_window| {
                let _ = winit_window.drag_window();
            });
        }
    });

    // right-click "Paste" in a text box: shown only when this says there's
    // something to paste (arboard's read is cheap/synchronous, no need to
    // leave the UI thread for it)
    window.on_clipboard_has_text(value_actions::clipboard_has_text);

    window.on_request_toggle_maximize({
        let weak = window.as_weak();
        move || {
            let Some(window) = weak.upgrade() else { return };
            window.window().with_winit_window(|winit_window| {
                winit_window.set_maximized(!winit_window.is_maximized());
            });
        }
    });

    let state = Arc::new(AppState::new());
    // the interface language: needs the window (Slint) and the settings
    i18n::apply(state.settings.lock_recover().language());
    let rt = backend::runtime::spawn();
    // no auto-connect here — the title bar and keyspace panel both start
    // empty until the user picks a connection from the title bar's picker
    let app = app::install(app::App {
        window: window.as_weak(),
        key_detail: key_detail::init(&window, state.clone(), rt.clone()),
        keyspace: keyspace::Keyspace::new(&window, state.clone(), rt.clone()),
        state: state.clone(),
        rt: rt.clone(),
        tabs: Default::default(),
        searches: Default::default(),
        queries: Default::default(),
        transfer: Default::default(),
        cloud: Default::default(),
    });

    // what needs the app's shared state (tabs, the tree, search and query tabs...)
    tabs::init(&app, &window);
    keyspace::init(&app, &window);
    search::init(&app, &window);
    redisql::init(&app, &window);
    connections::init(&app, &window);
    cloud::init(&app, &window);
    connection_transfer::init(&app, &window);
    cloud_settings::init(&app, &window);
    connection_profiles::init(&app, &window);

    // what only needs the stores and the runtime
    server_stats::init(&window, state.clone(), rt.clone());
    queries::init(&window, state.clone());
    settings::init(&window, state, rt);

    let outcome = window.run();
    save_window_state();
    outcome
}
