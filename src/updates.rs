//! The About window's update check: looks for a newer release on the picked channel (stable,
//! or betas too), then downloads and runs its installer (Windows) or opens its page (elsewhere).

use std::cell::RefCell;
use std::sync::Arc;

use rust_i18n::t;
use semver::Version;
use slint::ComponentHandle;

use crate::backend::LockExt;
use crate::backend::state::AppState;
use crate::backend::updates::{self, Channel, Release};
use crate::{About, MainWindow};

thread_local! {
    // the newer release the last check found; UI thread only
    static FOUND: RefCell<Option<Release>> = const { RefCell::new(None) };
}

/// Runs `f` on the UI thread with the window's About global.
fn on_ui(weak: slint::Weak<MainWindow>, f: impl FnOnce(&About<'_>) + Send + 'static) {
    let _ = weak.upgrade_in_event_loop(move |window| {
        f(&window.global::<About>());
    });
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
    state.settings.lock_recover().update_channel().unwrap_or(match Channel::of(&current_version()) {
        Channel::Beta => Channel::Beta,
        Channel::Stable | Channel::Dev => Channel::Stable,
    })
}

/// The toggle's segment of `channel`.
fn segment(channel: Channel) -> i32 {
    i32::from(channel == Channel::Beta)
}

pub fn init(window: &MainWindow, state: Arc<AppState>, rt: tokio::runtime::Handle) {
    let about = window.global::<About>();
    about.set_version(env!("CARGO_PKG_VERSION").into());
    about.set_channel(segment(channel(&state)));
    about.set_download_label(if cfg!(windows) { t!("Download and install") } else { t!("Open the release page") }.as_ref().into());

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
            if let Err(e) = state.settings.lock_recover().set_update_channel(picked) {
                return say(&about, e.to_string(), false);
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
            let (weak, channel) = (weak.clone(), channel(&state));
            rt.spawn(async move {
                let found = tokio::task::spawn_blocking(move || updates::newer_release(&current, channel)).await.map_err(|e| e.to_string()).and_then(|r| r);
                match &found {
                    Ok(Some(release)) => tracing::info!(version = %release.version, "an update is available"),
                    Ok(None) => tracing::info!("no update available"),
                    Err(error) => tracing::warn!("the update check failed: {error}"),
                }
                on_ui(weak, move |about| match found {
                    Ok(Some(release)) => {
                        say(about, t!("Version %{version} is available.", version = release.version.to_string()).into_owned(), false);
                        about.set_update_available(true);
                        FOUND.set(Some(release));
                    }
                    Ok(None) => say(about, t!("Cachemire is up to date.").into_owned(), false),
                    Err(error) => say(about, t!("Couldn't check for updates: %{error}", error = error).into_owned(), false),
                });
            });
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
                let installed = tokio::task::spawn_blocking(move || launch(&installer)).await.map_err(|e| e.to_string()).and_then(|r| r);
                on_ui(weak, move |about| match installed {
                    // the installer replaces the app's files: it must not be running
                    Ok(()) => {
                        let _ = slint::quit_event_loop();
                    }
                    Err(error) => say(about, t!("Couldn't download the update: %{error}", error = error).into_owned(), false),
                });
            });
        }
    });
}

#[cfg(windows)]
fn launch(url: &str) -> Result<(), String> {
    updates::run_installer(&updates::download_installer(url)?)
}

#[cfg(not(windows))]
fn launch(_url: &str) -> Result<(), String> {
    Err(String::new())
}
