//! The About page's update check, also made at startup when the setting asks: looks for a newer
//! release on the picked channel (stable, or betas too), then downloads and runs its installer
//! (Windows) or opens its page (elsewhere).

use crate::error::Localize;
use std::cell::RefCell;
use std::sync::Arc;

use rust_i18n::t;
use semver::Version;
use slint::ComponentHandle;

use crate::{About, MainWindow, Preferences, SettingsCategory};
use cachemire_core::state::AppState;
use cachemire_core::updates::{self, Channel, Release};

thread_local! {
    // the newer release the last check found; UI thread only
    static FOUND: RefCell<Option<Release>> = const { RefCell::new(None) };
}

/// Runs `f` on the UI thread with the window.
fn on_ui(weak: slint::Weak<MainWindow>, f: impl FnOnce(&MainWindow) + Send + 'static) {
    let _ = weak.upgrade_in_event_loop(move |window| f(&window));
}

fn say(about: &About, status: String, busy: bool) {
    about.set_status(status.into());
    about.set_busy(busy);
}

/// The version running, `0.0.0` if it can't be read.
fn current_version() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| Version::new(0, 0, 0))
}

/// The channel the check follows: the one picked, else the running build's (stable for a dev build).
fn channel(state: &AppState) -> Channel {
    state.settings.lock().update_channel().unwrap_or(match Channel::of(&current_version()) {
        Channel::Beta => Channel::Beta,
        Channel::Stable | Channel::Dev => Channel::Stable,
    })
}

/// The toggle's segment of `channel`.
fn segment(channel: Channel) -> i32 {
    i32::from(channel == Channel::Beta)
}

/// Offers `release` on the About page, which the settings window opens on `at_startup`.
pub(crate) fn show_found(window: &MainWindow, release: Release, at_startup: bool) {
    let about = window.global::<About>();
    say(&about, t!("Version %{version} is available.", version = release.version.to_string()).into_owned(), false);
    about.set_update_available(true);
    FOUND.set(Some(release));
    if at_startup {
        window.global::<Preferences>().invoke_show(SettingsCategory::About);
    }
}

/// Looks for a newer release on `channel` off the UI thread, and shows what it found on the
/// About page. `at_startup`, only a newer release is shown, and it opens the page.
fn look_for_update(weak: slint::Weak<MainWindow>, rt: &tokio::runtime::Handle, channel: Channel, at_startup: bool) {
    let current = current_version();
    rt.spawn(async move {
        let found = tokio::task::spawn_blocking(move || updates::newer_release(&current, channel))
            .await
            .map_err(|e| e.to_string())
            .and_then(|r| r.map_err(|e| e.to_string()));
        match &found {
            Ok(Some(release)) => tracing::info!(version = %release.version, at_startup, "an update is available"),
            Ok(None) => tracing::info!(at_startup, "no update available"),
            Err(error) => tracing::warn!(at_startup, "the update check failed: {error}"),
        }
        on_ui(weak, move |window| match found {
            Ok(Some(release)) => show_found(window, release, at_startup),
            // nothing to say unasked
            _ if at_startup => {}
            Ok(None) => say(&window.global::<About>(), t!("Cachemire is up to date.").into_owned(), false),
            Err(error) => say(&window.global::<About>(), t!("Couldn't check for updates: %{error}", error = error).into_owned(), false),
        });
    });
}

pub fn init(window: &MainWindow, state: Arc<AppState>, rt: tokio::runtime::Handle) {
    let about = window.global::<About>();
    about.set_version(env!("CARGO_PKG_VERSION").into());
    about.set_channel(segment(channel(&state)));
    about.set_download_label(if cfg!(windows) { t!("Download and install") } else { t!("Open the release page") }.as_ref().into());

    if state.settings.lock().check_updates_at_startup() && Channel::of(&current_version()) != Channel::Dev {
        look_for_update(window.as_weak(), &rt, channel(&state), true);
    }

    about.on_open_repo(|| {
        let _ = open::that_detached(updates::repo_url());
    });

    // another channel: what the last check found was for the previous one
    about.on_channel_picked({
        let (weak, state) = (window.as_weak(), state.clone());
        move |index| {
            let Some(window) = weak.upgrade() else { return };
            let about = window.global::<About>();
            let picked = if index == 1 { Channel::Beta } else { Channel::Stable };
            if let Err(e) = state.settings.lock().set_update_channel(picked) {
                return say(&about, e.localized(), false);
            }
            tracing::info!(channel = picked.code(), "update channel picked");
            about.set_channel(segment(picked));
            FOUND.take();
            about.set_update_available(false);
            say(&about, String::new(), false);
        }
    });

    about.on_check({
        let (weak, rt) = (window.as_weak(), rt.clone());
        move || {
            let Some(window) = weak.upgrade() else { return };
            let about = window.global::<About>();
            FOUND.take();
            about.set_update_available(false);
            let current = current_version();
            if Channel::of(&current) == Channel::Dev {
                return say(&about, t!("This is the development version: it doesn't look for updates.").into_owned(), false);
            }
            say(&about, t!("Checking for updates...").into_owned(), true);
            look_for_update(weak.clone(), &rt, channel(&state), false);
        }
    });

    about.on_download({
        let (weak, rt) = (window.as_weak(), rt);
        move || {
            let Some(window) = weak.upgrade() else { return };
            let about = window.global::<About>();
            let Some(release) = FOUND.with_borrow(Clone::clone) else { return };
            let Some(installer) = release.installer.clone().filter(|_| cfg!(windows)) else {
                if let Err(error) = open::that_detached(&release.page) {
                    say(&about, t!("Couldn't open the browser: %{error}", error = error.to_string()).into_owned(), false);
                }
                return;
            };
            say(&about, t!("Downloading version %{version}...", version = release.version.to_string()).into_owned(), true);
            let weak = weak.clone();
            rt.spawn(async move {
                let sha256 = release.installer_sha256;
                let installed = tokio::task::spawn_blocking(move || launch(&installer, sha256.as_deref()))
                    .await
                    .map_err(|e| e.to_string())
                    .and_then(|r| r);
                on_ui(weak, move |window| match installed {
                    // the installer replaces the app's files: it must not be running
                    Ok(()) => {
                        let _ = slint::quit_event_loop();
                    }
                    Err(error) => {
                        say(&window.global::<About>(), t!("Couldn't download the update: %{error}", error = error).into_owned(), false)
                    }
                });
            });
        }
    });
}

#[cfg(windows)]
fn launch(url: &str, sha256: Option<&str>) -> Result<(), String> {
    updates::download_installer(url, sha256).and_then(|installer| updates::run_installer(&installer)).map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn launch(_url: &str, _sha256: Option<&str>) -> Result<(), String> {
    Err(String::new())
}
