//! Wires the title bar's connection picker (dropdown + add/edit modal) to
//! `backend::connection_store`, which handles persistence and CRUD. This
//! module is UI glue: keep the list in sync, connect on click, save the form.

use crate::app::App;
use crate::backend::LockExt;
use rust_i18n::t;
use rustc_hash::FxHashMap;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::runtime::Handle;

use crate::backend::error::AppError;
use crate::backend::models::{ConnectionPayloadInput, ConnectionProfile};
use crate::backend::secret_vault::staged;
use crate::connection_profiles::to_tint;
use crate::{ConnectionEditor, ConnectionForm, ConnectionRowData, ConnectionState, ConnectionView, MainWindow, ProfileTint};

pub(crate) fn refresh(app: &App, window: &MainWindow) {
    let state = &app.state;
    let (list, cloud) = {
        let store = state.connections.lock_recover();
        (store.list_state(), store.cloud_connections().to_vec())
    };
    let active_id = list.active_connection_id;
    let background = app.tabs.borrow().parked_connections();
    // a profile that no longer exists counts as none
    let profiles: FxHashMap<String, ConnectionProfile> = state.settings.lock_recover().connection_profiles().into_iter().map(|p| (p.id.clone(), p)).collect();
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
            }
        })
        .collect();
    window.global::<ConnectionState>().set_cloud_connections(ModelRc::new(VecModel::from(cloud_rows)));
}

/// Drops the connection in use and returns to "No connection": it stops being
/// the active one, its pool closes and its tabs are dropped.
pub(crate) fn disconnect(app: &App, window: &MainWindow) {
    let (state, rt) = (&app.state, &app.rt);
    let id = window.connection_id().to_string();
    if !id.is_empty() {
        app.tabs.borrow_mut().discard_on_leave();
    }
    if let Err(e) = state.connections.lock_recover().clear_active() {
        tracing::error!("disconnect failed: {e}");
        return;
    }
    if !id.is_empty() {
        state.redis_connections.invalidate(&id);
        tracing::info!(connection = %id, "disconnected");
    }
    refresh(app, window);
    crate::connect_active(window, state.clone(), rt);
    let connection = window.global::<ConnectionState>();
    connection.set_view(ConnectionView { status_detail: "".into(), ..connection.get_view() });
    crate::keyspace::reload(app);
}

/// Wires the connection list, the title bar's pill and the edit form. The keyspace
/// is reloaded once a picked connection is established (active switches only then).
pub fn init(app: &Rc<App>, window: &MainWindow) {
    refresh(app, window);

    let connection = window.global::<ConnectionState>();
    connection.on_copy_connection_string(|| crate::app::with(copy_connection_string));
    connection.on_selected(|id| crate::app::with(|app, window| crate::connect_to(window, app.state.clone(), &app.rt, id.to_string(), true)));
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
        crate::app::with(|app, window| browse_ssh_key(&app.rt, window.as_weak(), |window, path| window.global::<ConnectionEditor>().set_ssh_key(path.into())));
    });
}

/// The title bar pill's copy button: the connected instance's connection string
/// (credentials included, see ConnectionStore::connection_string).
fn copy_connection_string(app: &Rc<App>, window: &MainWindow) {
    let id = window.connection_id().to_string();
    if id.is_empty() {
        return;
    }
    let Some(text) = app.state.connections.lock_recover().connection_string(&id) else { return };
    match crate::value_actions::copy_to_clipboard(&text) {
        Ok(()) => crate::toast::show_copied(window),
        Err(e) => tracing::warn!("copy failed: {e}"),
    }
}

/// "Delete" in the edit form: removes the connection, and disconnects first if it
/// is the one in use.
fn delete(app: &Rc<App>, window: &MainWindow, id: &str) {
    let (state, rt) = (&app.state, &app.rt);
    let removed = state.connections.lock_recover().remove(id);
    match removed {
        Ok(cleanup) => {
            // drops its pooled connections and its SSH tunnel
            state.redis_connections.invalidate(id);
            if let Some(cleanup) = cleanup {
                rt.spawn_blocking(move || cleanup.run());
            }
            if let Err(e) = state.settings.lock_recover().remove_saved_queries_of(id) {
                tracing::error!("removing the connection's saved queries failed: {e}");
            }
            window.global::<ConnectionEditor>().set_open(false);
            refresh(app, window);
            crate::queries::refresh(window, state);
            if window.connection_id() == id {
                // nothing is active any more, so this resets the title bar
                crate::connect_active(window, state.clone(), rt);
                crate::keyspace::reload(app);
            }
        }
        Err(e) => window.global::<ConnectionEditor>().set_error(e.into()),
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
            editor.set_test_status(e.into());
            return;
        }
    };
    editor.set_testing(true);
    let (state, id) = (app.state.clone(), form.id.to_string());
    app.rt.spawn(async move {
        let building = state.clone();
        let built = tokio::task::spawn_blocking(move || {
            staged(&building.connections, |store| {
                store.unlock_secrets(&id)?;
                store.preview(&id, payload.clone())
            })
        })
        .await
        .map_err(AppError::from)
        .flatten();
        let outcome = match built {
            Ok(connection) => crate::backend::connection_service::test_connection(&state, connection).await,
            Err(e) => Err(e),
        };
        crate::app::later(move |_, window| {
            let editor = window.global::<ConnectionEditor>();
            editor.set_testing(false);
            editor.set_test_ok(outcome.is_ok());
            editor.set_test_status(outcome.map_or_else(|e| e.to_string(), |()| t!("Connection successful.").into_owned()).into());
        });
    });
}

/// "Save" in the form: creates the connection, or updates the one it edits.
fn save_form(app: &Rc<App>, window: &MainWindow, form: &ConnectionForm) {
    let payload = match payload_from_form(form) {
        Ok(payload) => payload,
        Err(e) => {
            window.global::<ConnectionEditor>().set_error(e.into());
            return;
        }
    };
    // off the UI thread: storing the secrets can wait on the OS credential
    // store (a first-time setup, an unlock prompt)
    let (state, id) = (app.state.clone(), form.id.to_string());
    app.rt.spawn_blocking(move || {
        let new_id = uuid::Uuid::new_v4().to_string();
        let result = staged(&state.connections, |store| {
            if id.is_empty() { store.create_with_id(&new_id, payload.clone()) } else { store.update(&id, payload.clone()) }
        });
        // an open pool still reaches the server as it was before the edit
        if result.is_ok() && !id.is_empty() {
            state.redis_connections.invalidate(&id);
        }
        crate::app::later(move |app, window| match result {
            Ok(_) => {
                window.global::<ConnectionEditor>().set_error("".into());
                window.global::<ConnectionEditor>().set_open(false);
                refresh(app, window);
                // (the connection in use may be the one that was edited)
                crate::connection_profiles::apply_tint(window, &app.state);
            }
            Err(e) => window.global::<ConnectionEditor>().set_error(e.into()),
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
fn payload_from_form(form: &ConnectionForm) -> Result<ConnectionPayloadInput, AppError> {
    let keep_if_blank = |text: &slint::SharedString| if text.is_empty() { None } else { Some(text.to_string()) };
    let ssh_port = match form.ssh_port.trim() {
        "" => 22,
        text => text
            .parse::<u16>()
            .ok()
            .filter(|port| *port > 0)
            .ok_or_else(|| AppError::invalid(t!("SSH port must be a number between 1 and 65535.")))?,
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
    })
}

#[cfg(test)]
mod tests;
