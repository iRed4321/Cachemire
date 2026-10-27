// Release builds on Windows shouldn't drag a console window along with the
// GUI; debug builds keep it so the log on stderr stays visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rust_i18n::t;
use std::sync::Arc;
use std::sync::atomic::Ordering;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

rust_i18n::i18n!("../../resources/lang", fallback = "en");

mod app;
mod cloud;
mod connections;
mod error;
mod i18n;
mod key_detail;
mod keyspace;
mod last_session;
mod logging;
mod redisql;
mod search;
mod server_stats;
mod settings;
mod table_text;
mod tabs;
mod theme_colors;
mod toast;
#[cfg(test)]
mod ui_tests;
mod updates;
mod value_actions;
mod window_state;

use cachemire_core::error::{ErrorKind, Timeout};
use cachemire_core::state::AppState;
use error::Localize;

slint::include_modules!();

impl MainWindow {
    /// The saved-connection id of the connection in use ("" when none is).
    pub fn connection_id(&self) -> slint::SharedString {
        self.global::<ConnectionState>().get_view().connection_id
    }
}

/// How long an attempt to connect gets before it is reported as failed.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// Connects to a saved connection by id off the UI thread and shows the outcome in
/// the title bar; `announce` also raises `connection-established` on success. Only
/// the latest attempt counts: a superseded one's outcome is dropped.
pub(crate) fn connect_to(window: &MainWindow, state: Arc<AppState>, rt: &tokio::runtime::Handle, connection_id: String, announce: bool) {
    let attempt = state.connect_attempt.fetch_add(1, Ordering::SeqCst) + 1;
    let connection = window.global::<ConnectionState>();
    connection.set_view(ConnectionView { connecting: true, ..connection.get_view() });
    let weak = window.as_weak();
    rt.spawn(async move {
        let outcome =
            match tokio::time::timeout(CONNECT_TIMEOUT, cachemire_core::connection::connect_connection(&state, &connection_id)).await {
                Ok(outcome) => outcome,
                Err(_) => Err(Timeout::Connect { seconds: CONNECT_TIMEOUT.as_secs() }.into()),
            };
        // resolved from the saved connection itself, not the attempt's outcome,
        // so the status glyph can still show its lock (see TitleBar) when this
        // connection uses SSH but the attempt just failed
        let ssh_via = state.connections.lock().get(&connection_id).and_then(|c| c.ssh.as_ref().map(ssh_server_label)).unwrap_or_default();

        let _ = weak.upgrade_in_event_loop(move |window| {
            // (checked here, on the UI thread, where the attempts are numbered: nothing
            // can start between this check and the connection being made the active one)
            if state.connect_attempt.load(Ordering::SeqCst) != attempt {
                return;
            }
            // status/detail drive the title bar's dot + tooltip; ssh_failed
            // marks the SSH tunnel itself as the failure (not the Redis endpoint
            // past it) — the SSH badge colors red for that, not the dot
            let outcome = outcome.and_then(|c| {
                state.connections.lock().set_active(&connection_id)?;
                Ok(c)
            });
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
            window.global::<ConnectionState>().set_view(view);
            connections::profiles::apply_tint(&window, &state);
            if connected && announce {
                window.global::<ConnectionState>().invoke_established();
            }
        });
    });
}

/// `user@host` (with `:port` when it isn't 22) of an SSH tunnel's server.
fn ssh_server_label(ssh: &cachemire_core::connection::SshSettings) -> String {
    match ssh.port {
        22 => format!("{}@{}", ssh.username, ssh.host),
        port => format!("{}@{}:{port}", ssh.username, ssh.host),
    }
}

/// Reconnects/re-verifies the store's active connection — used by the
/// keyspace refresh button. Never called at startup, where only a
/// restored last session connects (see `last_session`).
pub(crate) fn connect_active(window: &MainWindow, state: Arc<AppState>, rt: &tokio::runtime::Handle) {
    let connection_id = state.connections.lock().get_active().ok().map(|c| c.id);
    match connection_id {
        Some(id) => connect_to(window, state, rt, id, false),
        None => {
            // nothing to connect to: an attempt still under way is no longer wanted
            state.connect_attempt.fetch_add(1, Ordering::SeqCst);
            let view = ConnectionView {
                status: ConnectionStatus::None,
                status_detail: t!("No saved connection").as_ref().into(),
                ..Default::default()
            };
            window.global::<ConnectionState>().set_view(view);
            connections::profiles::apply_tint(window, &state);
        }
    }
}

/// The id of the exe's icon resource (see build.rs).
#[cfg(windows)]
const ICON_RESOURCE: u16 = 1;

/// Gives the window the exe's own icon at the sizes Windows draws (16 and 32 times the scale),
/// on its first event and again when its scale changes: Windows then picks the .ico's
/// matching images instead of shrinking one bitmap itself.
#[cfg(windows)]
fn use_exe_icon(window: &slint::Window, event: &slint::winit_030::winit::event::WindowEvent, applied: &mut bool) {
    use slint::winit_030::winit::{
        dpi::PhysicalSize,
        event::WindowEvent,
        platform::windows::{IconExtWindows, WindowExtWindows},
        window::Icon,
    };

    if !*applied || matches!(event, WindowEvent::ScaleFactorChanged { .. }) {
        *applied = true;
        window.with_winit_window(|winit_window| {
            let scale = winit_window.scale_factor();
            let icon = |logical: f64| {
                let side = (logical * scale).round() as u32;
                Icon::from_resource(ICON_RESOURCE, Some(PhysicalSize::new(side, side))).ok()
            };
            winit_window.set_window_icon(icon(16.0));
            winit_window.set_taskbar_icon(icon(32.0));
        });
    }
}

/// The window's winit event hook. Slint keeps a single one per window, so everything
/// that needs these events is called from here.
fn install_window_events(window: &MainWindow) {
    let weak = window.as_weak();
    #[cfg(windows)]
    let mut icon_applied = false;
    window.window().on_winit_window_event(move |window, event| {
        if let Some(main) = weak.upgrade() {
            toast::on_window_event(&main, window, event);
        }
        #[cfg(windows)]
        use_exe_icon(window, event, &mut icon_applied);
        slint::winit_030::EventResult::Propagate
    });
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
    let _log = logging::init(&cachemire_core::state::resolve_data_file().with_file_name("logs"));
    // Wayland has no per-window icon: the shell finds it through the app id,
    // as the icon of the cachemire.desktop entry (resources/cachemire.desktop)
    select_backend()?;
    slint::set_xdg_app_id("cachemire")?;
    let window = MainWindow::new()?;
    let save_window_state = window_state::install(&window);
    toast::init(&window);
    install_window_events(&window);

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

    let state = Arc::new(AppState::load());
    // the interface language: needs the window (Slint) and the settings
    i18n::apply(state.settings.lock().language());
    let rt = cachemire_core::runtime::spawn();
    updates::init(&window, state.clone(), rt.clone());
    // no auto-connect here unless the settings ask for the last session
    // back (see last_session): otherwise the user picks a connection
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
    connections::transfer::init(&window);
    cloud::settings::init(&window);
    connections::profiles::init(&app, &window);

    // what only needs the stores and the runtime
    server_stats::init(&window, state.clone(), rt.clone());
    redisql::saved::init(&window, state.clone());
    settings::init(&window, state, rt);
    last_session::restore(&app, &window);

    let outcome = window.run();
    last_session::save(&app, &window);
    save_window_state();
    outcome
}
