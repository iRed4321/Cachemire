//! Wires the title bar's connection picker (dropdown + add/edit modal) to
//! `cachemire_core::connection::store`, which handles persistence and CRUD. This
//! module is UI glue: keep the list in sync, connect on click, save the form.

use crate::app::App;
use crate::host_key_dialog::Answered;
use rust_i18n::t;
use rustc_hash::FxHashMap;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::runtime::Handle;

use crate::connections::profiles::to_tint;
use crate::error::Localize;
use crate::{ConnectionEditor, ConnectionForm, ConnectionRowData, ConnectionState, ConnectionView, MainWindow, ProfileTint};
use cachemire_core::connection::ConnectionPayloadInput;
use cachemire_core::error::{Error, Invalid};
use cachemire_core::secret_vault::{LeftOut, SecretsPolicy, staged};
use cachemire_core::settings::ConnectionProfile;

mod connect;
pub mod profiles;
pub mod transfer;

pub(crate) use connect::{connect_active, connect_active_then, connect_to, keep_active_in_step};

pub(crate) fn refresh(app: &App, window: &MainWindow) {
    let state = &app.state;
    let (list, cloud) = {
        let store = state.connections.lock();
        (store.list_state(), store.cloud_connections().to_vec())
    };
    let active_id = list.active_connection_id;
    let background = app.tabs.borrow().parked_connections();
    // a profile that no longer exists counts as none
    let profiles: FxHashMap<String, ConnectionProfile> =
        state.settings.lock().connection_profiles().into_iter().map(|p| (p.id.clone(), p)).collect();
    let badge = |id: &str| {
        let profile = profiles.get(id);
        (profile.map_or_else(String::new, |p| p.name.clone()), profile.map_or(ProfileTint::Red, |p| to_tint(p.color)))
    };
    let rows: Vec<ConnectionRowData> = list
        .connections
        .into_iter()
        .map(|c| {
            let ssh = c.ssh.as_ref();
            let (profile_name, profile_tint) = badge(&c.profile_id);
            ConnectionRowData {
                address: c.connection_url.as_str().into(),
                active: c.id == active_id,
                in_background: background.contains(&c.id),
                id: c.id.into(),
                name: c.name.into(),
                connection_url: c.connection_url.as_str().into(),
                username: c.username.into(),
                ssh_enabled: ssh.is_some(),
                ssh_host: ssh.map(|s| s.host.as_str()).unwrap_or_default().into(),
                ssh_port: ssh.map_or(22, |s| s.port).to_string().into(),
                ssh_username: ssh.map(|s| s.username.as_str()).unwrap_or_default().into(),
                ssh_key: ssh.map(|s| s.private_key.as_str()).unwrap_or_default().into(),
                profile_id: if profiles.contains_key(&c.profile_id) { c.profile_id.clone() } else { String::new() }.into(),
                profile_name: profile_name.into(),
                profile_tint,
                read_only: c.read_only,
            }
        })
        .collect();
    window.global::<ConnectionState>().set_connections(ModelRc::new(VecModel::from(rows)));

    let cloud_rows: Vec<ConnectionRowData> = cloud
        .into_iter()
        .map(|c| {
            let (profile_name, profile_tint) = badge(&c.profile_id);
            ConnectionRowData {
                address: c.address().into(),
                active: c.id == active_id,
                in_background: background.contains(&c.id),
                id: c.id.into(),
                name: c.name.into(),
                connection_url: "".into(),
                username: "".into(),
                ssh_enabled: c.ssh.is_some(),
                ssh_host: "".into(),
                ssh_port: "22".into(),
                ssh_username: "".into(),
                ssh_key: "".into(),
                profile_id: "".into(),
                profile_name: profile_name.into(),
                profile_tint,
                read_only: false,
            }
        })
        .collect();
    window.global::<ConnectionState>().set_cloud_connections(ModelRc::new(VecModel::from(cloud_rows)));
}

/// Drops the connection in use and returns to "No connection": it stops being
/// the active one, its pool closes and its tabs are dropped, with the last session.
pub(crate) fn disconnect(app: &App, window: &MainWindow) {
    let (state, rt) = (&app.state, &app.rt);
    let id = window.connection_id().to_string();
    app.picked.borrow_mut().clear();
    if !id.is_empty() {
        app.tabs.borrow_mut().discard_on_leave();
        crate::last_session::forget(&app.state);
    }
    if let Err(e) = state.connections.lock().clear_active() {
        crate::toast::show(window, &t!("Saving failed: %{error}", error = e.localized()));
        return;
    }
    if !id.is_empty() {
        state.redis_connections.invalidate(&id);
        tracing::info!(connection = %id, "disconnected");
    }
    refresh(app, window);
    connect_active(window, state.clone(), rt);
    let connection = window.global::<ConnectionState>();
    connection.set_view(ConnectionView { status_detail: "".into(), ..connection.get_view() });
    crate::keyspace::reload(app);
    crate::tabs::follow_connection(app, window);
}

/// Wires the connection list, the title bar's pill and the edit form. The keyspace
/// is reloaded once a picked connection is established (active switches only then).
pub fn init(app: &Rc<App>, window: &MainWindow) {
    refresh(app, window);

    let connection = window.global::<ConnectionState>();
    connection.on_copy_connection_string(|| crate::app::with(copy_connection_string));
    connection.on_selected(|id| crate::app::with(|app, window| connect_to(window, app.state.clone(), &app.rt, id.to_string(), true)));
    connection.on_established(|| {
        crate::app::with(|app, window| {
            refresh(app, window);
            crate::keyspace::reload(app);
        });
    });
    // the × in the title bar's pill: drops the connection in use and returns to
    // "No connection"; its tabs are dropped
    connection.on_disconnect_requested(|| crate::app::with(|app, window| disconnect(app, window)));

    let editor = window.global::<ConnectionEditor>();
    editor.on_deleted(|id| crate::app::with(|app, window| delete(app, window, &id)));
    editor.on_test_requested(|form| crate::app::with(|app, window| test_form(app, window, &form)));
    editor.on_saved(|form| crate::app::with(|app, window| save_form(app, window, &form)));
    // "Browse…" beside the SSH private key
    editor.on_browse_key(|| {
        crate::app::with(|app, window| {
            browse_ssh_key(&app.rt, window.as_weak(), |window, path| window.global::<ConnectionEditor>().set_ssh_key(path.into()))
        });
    });
}

/// The title bar pill's copy button: the connected instance's connection string
/// (credentials included, see ConnectionStore::connection_string).
fn copy_connection_string(app: &Rc<App>, window: &MainWindow) {
    let id = window.connection_id().to_string();
    if id.is_empty() {
        return;
    }
    let Some(text) = app.state.connections.lock().connection_string(&id) else { return };
    match crate::value_actions::copy_to_clipboard(&text) {
        Ok(()) => crate::toast::show_copied(window),
        Err(e) => crate::toast::show(window, &t!("Copying failed: %{error}", error = e.localized())),
    }
}

/// "Delete" in the edit form: removes the connection, and disconnects first if it
/// is the one in use.
fn delete(app: &Rc<App>, window: &MainWindow, id: &str) {
    let (state, rt) = (&app.state, &app.rt);
    let removed = state.connections.lock().remove(id);
    match removed {
        Ok(cleanup) => {
            // drops its pooled connections and its SSH tunnel
            state.redis_connections.invalidate(id);
            crate::keyspace::forget(app, id);
            if let Some(cleanup) = cleanup {
                rt.spawn_blocking(move || cleanup.run());
            }
            if let Err(e) = state.settings.lock().remove_saved_queries_of(id) {
                crate::toast::show(window, &t!("Saving failed: %{error}", error = e.localized()));
            }
            window.global::<ConnectionEditor>().set_open(false);
            refresh(app, window);
            crate::redisql::saved::refresh(window, state);
            if window.connection_id() == id {
                // the connection picked is gone, so this resets the title bar
                connect_active(window, state.clone(), rt);
                crate::keyspace::reload(app);
                crate::tabs::follow_connection(app, window);
            }
        }
        Err(e) => window.global::<ConnectionEditor>().set_error(e.localized().into()),
    }
}

/// "Test connection" in the form: builds the connection as the form describes it, unsaved.
fn test_form(app: &Rc<App>, window: &MainWindow, form: &ConnectionForm) {
    let editor = window.global::<ConnectionEditor>();
    editor.set_test_status("".into());
    let payload = match payload_from_form(form) {
        Ok(payload) => payload,
        Err(e) => {
            editor.set_test_ok(false);
            editor.set_test_status(e.localized().into());
            return;
        }
    };
    // a connection saved without a password the form leaves blank asks for it first
    if !form.id.is_empty() {
        let (missing, name) = {
            let store = app.state.connections.lock();
            (store.missing_secrets(&form.id), form.name.to_string())
        };
        let needed = LeftOut {
            password: missing.password && payload.password.is_none(),
            ssh_password: missing.ssh_password && payload.ssh_password.is_none(),
            ssh_passphrase: missing.ssh_passphrase && payload.ssh_passphrase.is_none(),
        };
        if !needed.is_empty() {
            let (form, id) = (form.clone(), form.id.to_string());
            let resume = Box::new(move |app: &Rc<App>, window: &MainWindow| test_form(app, window, &form));
            crate::keyring_dialogs::ask_secrets(app, window, &id, &name, needed, resume);
            return;
        }
    }
    editor.set_testing(true);
    let (state, id) = (app.state.clone(), form.id.to_string());
    let test = async move {
        let building = state.clone();
        let built = tokio::task::spawn_blocking(move || {
            staged(&building.connections, |store| {
                store.unlock_secrets(&id)?;
                store.preview(&id, payload.clone())
            })
        })
        .await
        .map_err(Error::from)
        .flatten();
        match built {
            Ok(connection) => cachemire_core::connection::test_connection(&state, connection).await,
            Err(e) => Err(e),
        }
    };
    let form = form.clone();
    crate::app::spawn(&app.rt, test, move |app, window, outcome| {
        let outcome = outcome.flatten();
        let editor = window.global::<ConnectionEditor>();
        // a server not seen before: the user decides, and a trusted key runs the test again
        if let Err(e) = &outcome
            && let Some(server) = crate::host_key_dialog::unknown(e)
        {
            let answer = Box::new(move |app: &Rc<App>, window: &MainWindow, answered: Answered| match answered {
                Answered::Trusted => test_form(app, window, &form),
                Answered::Cancelled => window.global::<ConnectionEditor>().set_testing(false),
                Answered::NotSaved(e) => {
                    let editor = window.global::<ConnectionEditor>();
                    editor.set_testing(false);
                    editor.set_test_ok(false);
                    editor.set_test_status(e.localized().into());
                }
            });
            crate::host_key_dialog::ask(app, window, server.clone(), answer);
            return;
        }
        editor.set_testing(false);
        editor.set_test_ok(outcome.is_ok());
        editor.set_test_status(outcome.map_or_else(|e| e.localized(), |()| t!("Connection successful.").into_owned()).into());
    });
}

/// "Save" in the form: creates the connection, or updates the one it edits.
fn save_form(app: &Rc<App>, window: &MainWindow, form: &ConnectionForm) {
    let payload = match payload_from_form(form) {
        Ok(payload) => payload,
        Err(e) => {
            window.global::<ConnectionEditor>().set_error(e.localized().into());
            return;
        }
    };
    save_payload(app, form.id.to_string(), payload, SecretsPolicy::VaultOrFail);
}

/// Saves `payload` as the connection `id` (a new one when empty), as `policy` says for the
/// secrets the keyring can't take. If it is unavailable, the user is asked what to do.
fn save_payload(app: &Rc<App>, id: String, payload: ConnectionPayloadInput, policy: SecretsPolicy) {
    // off the UI thread: storing the secrets can wait on the OS credential
    // store (a first-time setup, an unlock prompt)
    let state = app.state.clone();
    app.rt.spawn_blocking(move || {
        let new_id = uuid::Uuid::new_v4().to_string();
        let result = staged(&state.connections, |store| {
            if id.is_empty() { store.create_with_id(&new_id, payload.clone(), policy) } else { store.update(&id, payload.clone(), policy) }
        });
        // an open pool still reaches the server as it was before the edit
        if result.is_ok() && !id.is_empty() {
            state.redis_connections.invalidate(&id);
        }
        crate::app::later(move |app, window| match result {
            Ok(_) => {
                // a connection created is made the active one in the store: the one in use stays
                keep_active_in_step(app, window);
                window.global::<ConnectionEditor>().set_error("".into());
                window.global::<ConnectionEditor>().set_open(false);
                refresh(app, window);
                // (the connection in use may be the one that was edited)
                crate::connections::profiles::apply_tint(window, &app.state);
                // a link that waited for a connection to its server
                crate::links::retry_pending(app, window);
            }
            Err(e) if crate::keyring_dialogs::is_unavailable(&e) => {
                let retry = Box::new(move |app: &Rc<App>, _: &MainWindow, policy| save_payload(app, id, payload, policy));
                crate::keyring_dialogs::ask_keyring(app, window, true, retry);
            }
            Err(e) => window.global::<ConnectionEditor>().set_error(e.localized().into()),
        });
    });
}

/// Opens a native file dialog for an SSH private key, on ~/.ssh when there is one, off the UI
/// thread; `set` puts the chosen path into the window.
pub(crate) fn browse_ssh_key(rt: &Handle, weak: slint::Weak<MainWindow>, set: fn(&MainWindow, &str)) {
    rt.spawn(async move {
        let mut dialog = rfd::AsyncFileDialog::new().set_title(t!("Select the SSH private key"));
        if let Some(ssh_dir) = dirs::home_dir().map(|home| home.join(".ssh")).filter(|dir| dir.is_dir()) {
            dialog = dialog.set_directory(ssh_dir);
        }
        let Some(file) = dialog.pick_file().await else { return };
        let _ = weak.upgrade_in_event_loop(move |window| {
            set(&window, file.path().to_string_lossy().as_ref());
        });
    });
}

/// What the form's fields say, as a payload. A blank password box leaves the
/// stored password alone (`None`); everything else is taken as typed.
fn payload_from_form(form: &ConnectionForm) -> Result<ConnectionPayloadInput, Error> {
    let keep_if_blank = |text: &slint::SharedString| if text.is_empty() { None } else { Some(text.to_string()) };
    let ssh_port = match form.ssh_port.trim() {
        "" => 22,
        text => text.parse::<u16>().ok().filter(|port| *port > 0).ok_or(Invalid::SshPort)?,
    };
    Ok(ConnectionPayloadInput {
        name: Some(form.name.to_string()),
        connection_url: Some(form.connection_url.to_string()),
        username: Some(form.username.to_string()),
        password: keep_if_blank(&form.password),
        ssh_enabled: Some(form.ssh_enabled),
        ssh_host: Some(form.ssh_host.to_string()),
        ssh_port: Some(ssh_port),
        ssh_username: Some(form.ssh_username.to_string()),
        ssh_password: keep_if_blank(&form.ssh_password),
        ssh_private_key: Some(form.ssh_key.to_string()),
        ssh_passphrase: keep_if_blank(&form.ssh_passphrase),
        profile_id: Some(form.profile_id.to_string()),
        read_only: Some(form.read_only),
    })
}

#[cfg(test)]
mod tests;
