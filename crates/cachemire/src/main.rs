// Release builds on Windows shouldn't drag a console window along with the
// GUI; debug builds keep it so the log on stderr stays visible.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use rust_i18n::t;
use std::sync::Arc;

use slint::ComponentHandle;
use slint::winit_030::WinitWindowAccessor;

rust_i18n::i18n!("../../resources/lang", fallback = "en");

mod app;
mod cloud;
mod connections;
#[cfg(windows)]
mod drag_latch;
mod error;
mod i18n;
mod key_detail;
mod keyspace;
mod last_session;
mod links;
mod logging;
mod redisql;
mod search;
mod server_stats;
mod settings;
mod shortcuts;
mod table_text;
mod tabs;
mod theme_colors;
mod toast;
#[cfg(test)]
mod ui_tests;
mod updates;
mod value_actions;
mod window_state;

use cachemire_core::state::{AppState, InstanceLock};

slint::include_modules!();

impl MainWindow {
    /// The saved-connection id of the connection in use ("" when none is).
    pub fn connection_id(&self) -> slint::SharedString {
        self.global::<ConnectionState>().get_view().connection_id
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
    #[cfg(windows)]
    let mut cursor = slint::winit_030::winit::dpi::PhysicalPosition::new(-1.0, -1.0);
    window.window().on_winit_window_event(move |window, event| {
        if let Some(main) = weak.upgrade() {
            toast::on_window_event(&main, window, event);
        }
        #[cfg(windows)]
        use_exe_icon(window, event, &mut icon_applied);
        #[cfg(windows)]
        drag_latch::on_window_event(window, event, &mut cursor);
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
    let link = links::from_args(std::env::args().skip(1));
    // held until the process ends
    let instance = cachemire_core::state::lock_instance();
    if matches!(instance, InstanceLock::Taken) {
        // a link goes to the instance running, which may take the foreground to show it
        if let Some(link) = &link {
            match links::send(&links::inbox(), link) {
                Ok(()) => {
                    tracing::info!("a link was handed to the instance running");
                    #[cfg(windows)]
                    // SAFETY: a plain Win32 call, with no pointer
                    unsafe {
                        use windows_sys::Win32::UI::WindowsAndMessaging::{ASFW_ANY, AllowSetForegroundWindow};
                        AllowSetForegroundWindow(ASFW_ANY);
                    }
                    return Ok(());
                }
                Err(e) => tracing::warn!("can't hand the link to the instance running: {e}"),
            }
        }
        tracing::warn!("another instance is running: this one stops");
        rust_i18n::set_locale(cachemire_core::i18n::Language::Automatic.resolve());
        rfd::MessageDialog::new()
            .set_title("Cachemire")
            .set_description(t!("Cachemire is already running."))
            .set_level(rfd::MessageLevel::Info)
            .show();
        return Ok(());
    }
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
                #[cfg(windows)]
                drag_latch::clear(winit_window);
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
        picked: Default::default(),
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
    links::init(&app, &window);

    // what only needs the stores and the runtime
    server_stats::init(&window, state.clone(), rt.clone());
    redisql::saved::init(&window, state.clone());
    shortcuts::init(&window, state.clone());
    settings::init(&window, state, rt);
    // nothing is connected yet: no connection stays active from the last run
    connections::keep_active_in_step(&app, &window);
    // a link launched with goes where it leads instead of the last session
    match link {
        Some(link) => links::open(&app, &window, &link),
        None => last_session::restore(&app, &window),
    }
    last_session::keep_saved();
    links::watch(links::inbox());

    let outcome = window.run();
    last_session::save(&app, &window);
    save_window_state();
    outcome
}
