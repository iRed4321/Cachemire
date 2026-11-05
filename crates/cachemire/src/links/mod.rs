//! Shareable links (`cachemire://`, see `cachemire_core::link`): the key panel's share button
//! copies one, and a link opened (at start, or handed over by another launch) is followed as
//! far as it goes: its server, then its key, then the field and the filter in it.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use rust_i18n::t;
use slint::ComponentHandle;

use crate::app::App;
use crate::error::Localize;
use crate::tabs::LinkTarget;
use crate::{AwsAccount, ConnectionEditor, KeyPanel, MainWindow};
use cachemire_core::aws;
use cachemire_core::key_name::KeyName;
use cachemire_core::link::{Link, LinkError, Server};

/// How often the running app looks for the links other launches hand it.
const INBOX_INTERVAL: Duration = Duration::from_millis(400);
/// A link handed over longer ago than this was for an app that has quit since: dropped.
const INBOX_EXPIRY: Duration = Duration::from_secs(60);
const INBOX_EXTENSION: &str = "link";

thread_local! {
    // a link waiting for its server: a connection to be saved, or an AWS account to be listed
    static PENDING: RefCell<Option<Link>> = const { RefCell::new(None) };
}

/// The link the app was launched with, if any: the first argument that is one.
pub fn from_args(args: impl IntoIterator<Item = String>) -> Option<String> {
    let prefix = format!("{}:", cachemire_core::link::SCHEME);
    args.into_iter().find(|arg| arg.get(..prefix.len()).is_some_and(|start| start.eq_ignore_ascii_case(&prefix)))
}

/// Why `error` keeps a link from opening, for the user.
fn describe(error: &LinkError) -> String {
    match error {
        LinkError::Newer(_) | LinkError::UnknownAction(_) | LinkError::NoServer => {
            t!("It was made by a newer version of Cachemire.").into_owned()
        }
        LinkError::NotALink | LinkError::TwoServers | LinkError::NoKey | LinkError::Unreadable(_) => {
            t!("It is incomplete or damaged.").into_owned()
        }
    }
}

/// Opens the link `text`: any link still waiting for its server is dropped.
pub fn open(app: &App, window: &MainWindow, text: &str) {
    PENDING.take();
    match Link::parse(text) {
        Ok(link) => {
            tracing::info!(server = %link.server_label(), "opening a link");
            follow(app, window, link, true);
        }
        Err(e) => {
            tracing::warn!("a link can't be opened ({e}): {text}");
            crate::toast::show(window, &t!("This link can't be opened. %{why}", why = describe(&e)));
        }
    }
}

/// Follows again the link waiting for its server, now that a connection was saved or an
/// AWS account listed; it goes on waiting if that wasn't its server either.
pub fn retry_pending(app: &App, window: &MainWindow) {
    if let Some(link) = PENDING.take() {
        follow(app, window, link, false);
    }
}

/// Where a link's server is, among what the app knows.
enum Found {
    /// a saved connection, or a cloud endpoint listed now
    Connection(String),
    /// a cloud endpoint, once `profile` (last seen listing its account) lists it as `id`
    CloudProfile { profile: String, id: String },
    /// a cloud account listed now that doesn't have the endpoint
    CloudGone,
    /// a cloud account no profile is known to list: the user signs in to it
    CloudUnknown,
    /// a Redis server no saved connection reaches
    NewServer,
}

fn find(app: &App, window: &MainWindow, link: &Link) -> Found {
    let store = app.state.connections.lock();
    let Server::Aws { account, region, name } = &link.server else {
        let saved = store.list_state().connections;
        return link.find_saved(&saved, &window.connection_id()).map_or(Found::NewServer, |c| Found::Connection(c.id.clone()));
    };
    if store.cloud_account() == (account.as_str(), region.as_str()) {
        return store.cloud_connections().iter().find(|c| c.name == *name).map_or(Found::CloudGone, |c| Found::Connection(c.id.clone()));
    }
    let profile = app.state.settings.lock().cloud_profiles_of(account, region).into_iter().next();
    match profile {
        Some(profile) if window.global::<AwsAccount>().get_available() => {
            Found::CloudProfile { id: aws::connection_id(&profile, name), profile }
        }
        _ => Found::CloudUnknown,
    }
}

/// Follows `link` if its server can be found, else keeps it waiting; `first` says why.
fn follow(app: &App, window: &MainWindow, link: Link, first: bool) {
    let target = LinkTarget { key: link.key.id(), field: link.field.clone(), path: link.path.clone() };
    match find(app, window, &link) {
        Found::Connection(id) if window.connection_id() == id => crate::tabs::open_link(app, window, target),
        Found::Connection(id) => {
            crate::tabs::park_link(app, window, &id, &target);
            crate::connections::refresh(app, window);
            crate::connections::connect_to(window, app.state.clone(), &app.rt, id, true);
        }
        Found::CloudProfile { profile, id } => {
            crate::tabs::park_link(app, window, &id, &target);
            crate::cloud::resume(app, window, profile, id);
        }
        Found::CloudGone => {
            crate::toast::show(window, &t!("%{server} is no longer among the account's endpoints.", server = link.server_label()));
        }
        Found::CloudUnknown => {
            if first {
                crate::toast::show(
                    window,
                    &t!("Sign in to AWS with a profile that lists %{server} to open this link.", server = link.server_label()),
                );
            }
            PENDING.set(Some(link));
        }
        Found::NewServer => {
            if first {
                fill_new_connection(window, &link);
                crate::toast::show(
                    window,
                    &t!("No saved connection reaches %{server}: save it to open the link.", server = link.server_label()),
                );
            }
            PENDING.set(Some(link));
        }
    }
}

/// Opens the connection form on a new connection to `link`'s server, its login left to fill in.
fn fill_new_connection(window: &MainWindow, link: &Link) {
    let (Some(url), Server::Redis { address, via, .. }) = (link.connection_url(), &link.server) else { return };
    let editor = window.global::<ConnectionEditor>();
    editor.invoke_start_new();
    editor.set_name(link.name.clone().unwrap_or_else(|| address.host.clone()).into());
    editor.set_connection_url(url.into());
    if let Some(via) = via {
        editor.set_ssh_enabled(true);
        editor.set_ssh_host(via.host.as_str().into());
        editor.set_ssh_port(via.port.to_string().into());
    }
}

/// The link to the key of the active tab, on the connection shown, with the panel's filter,
/// opened on `field` if there is one.
pub fn active_link(app: &App, window: &MainWindow, field: Option<String>) -> Option<String> {
    let key = app.tabs.borrow().active_key().map(KeyName::from_id)?;
    let id = window.connection_id();
    let store = app.state.connections.lock();
    let mut link = Link::to_key(&store.get(&id)?, store.cloud_key(&id).as_ref(), key)?;
    link.path = Some(window.global::<KeyPanel>().get_json_filter().trim().to_string()).filter(|path| !path.is_empty());
    link.field = field;
    Some(link.to_uri())
}

/// The share buttons, the key panel's and an open row's: copies the link to the key shown,
/// or to one of its fields.
fn share(app: &App, window: &MainWindow, field: Option<String>) {
    let Some(link) = active_link(app, window, field) else {
        crate::toast::show(window, &t!("This connection's address can't be put in a link."));
        return;
    };
    match crate::value_actions::copy_to_clipboard(&link) {
        Ok(()) => crate::toast::show(window, &t!("Link copied")),
        Err(e) => crate::toast::show(window, &t!("Copying failed: %{error}", error = e.localized())),
    }
}

pub fn init(app: &std::rc::Rc<App>, window: &MainWindow) {
    window.global::<KeyPanel>().on_share_requested({
        let app = app.clone();
        move || {
            if let Some(window) = app.window.upgrade() {
                share(&app, &window, None);
            }
        }
    });
    let app = app.clone();
    window.global::<KeyPanel>().on_share_field_requested(move |field| {
        if let Some(window) = app.window.upgrade() {
            share(&app, &window, Some(field.to_string()));
        }
    });
}

/// The folder another launch leaves its link in, for the app already running.
pub fn inbox() -> PathBuf {
    cachemire_core::state::resolve_data_file().with_file_name("links")
}

/// Hands `link` to the app running, through `dir`: written whole before it shows there.
pub fn send(dir: &Path, link: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let stamp = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap_or_default().as_nanos();
    let name = format!("{stamp:032}-{}", uuid::Uuid::new_v4().simple());
    let partial = dir.join(format!("{name}.partial"));
    std::fs::write(&partial, link)?;
    std::fs::rename(&partial, dir.join(format!("{name}.{INBOX_EXTENSION}")))
}

/// Takes the links handed over in `dir`, oldest first; those left too long are dropped.
pub fn receive(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut files: Vec<PathBuf> =
        entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|ext| ext == INBOX_EXTENSION)).collect();
    files.sort();
    files
        .into_iter()
        .filter_map(|file| {
            let fresh = std::fs::metadata(&file).and_then(|m| m.modified()).is_ok_and(|at| at.elapsed().unwrap_or_default() < INBOX_EXPIRY);
            let text = std::fs::read_to_string(&file).ok();
            if let Err(e) = std::fs::remove_file(&file) {
                tracing::warn!("can't remove {}: {e}", file.display());
            }
            text.filter(|_| fresh)
        })
        .collect()
}

/// Opens the links other launches hand over through `dir`, the window brought forward for each.
pub fn watch(dir: PathBuf) {
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, INBOX_INTERVAL, move || {
        for link in receive(&dir) {
            crate::app::with(|app, window| {
                crate::cloud::bring_to_front(window);
                open(app, window, &link);
            });
        }
    });
    std::mem::forget(timer);
}

#[cfg(test)]
mod tests;
