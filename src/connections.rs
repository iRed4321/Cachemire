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
        eprintln!("disconnect failed: {e}");
        return;
    }
    if !id.is_empty() {
        state.redis_connections.invalidate(&id);
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
    let (state, rt) = (app.state.clone(), app.rt.clone());
    refresh(app, window);

    // title bar pill's copy button: the connected instance's connection
    // string (credentials included, see ConnectionStore::connection_string)
    window.global::<ConnectionState>().on_copy_connection_string({
        let weak = window.as_weak();
        let state = state.clone();
        move || {
            let Some(window) = weak.upgrade() else { return };
            let id = window.connection_id().to_string();
            if id.is_empty() {
                return;
            }
            let Some(text) = state.connections.lock_recover().connection_string(&id) else { return };
            match crate::value_actions::copy_to_clipboard(&text) {
                Ok(()) => crate::toast::show_copied(&window),
                Err(e) => eprintln!("copy failed: {e}"),
            }
        }
    });

    window.global::<ConnectionState>().on_selected({
        let weak = window.as_weak();
        let state = state.clone();
        let rt = rt.clone();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            crate::connect_to(&window, state.clone(), &rt, id.to_string(), true);
        }
    });

    window.global::<ConnectionState>().on_established({
        let app = app.clone();
        move || {
            let Some(window) = app.window.upgrade() else { return };
            refresh(&app, &window);
            crate::keyspace::reload(&app);
        }
    });

    // the × in the title bar's pill: drops the connection in use and returns to
    // "No connection"; its tabs are dropped
    window.global::<ConnectionState>().on_disconnect_requested({
        let app = app.clone();
        move || {
            if let Some(window) = app.window.upgrade() {
                disconnect(&app, &window);
            }
        }
    });

    // "Delete" in the edit form: removes the connection, and disconnects
    // first if it is the one in use
    window.global::<ConnectionEditor>().on_deleted({
        let app = app.clone();
        let (state, rt) = (state.clone(), rt.clone());
        move |id| {
            let Some(window) = app.window.upgrade() else { return };
            let id = id.to_string();
            let removed = state.connections.lock_recover().remove(&id);
            match removed {
                Ok(cleanup) => {
                    // drops its pooled connections and its SSH tunnel
                    state.redis_connections.invalidate(&id);
                    if let Some(cleanup) = cleanup {
                        rt.spawn_blocking(move || cleanup.run());
                    }
                    if let Err(e) = state.settings.lock_recover().remove_saved_queries_of(&id) {
                        eprintln!("removing the connection's saved queries failed: {e}");
                    }
                    window.global::<ConnectionEditor>().set_open(false);
                    refresh(&app, &window);
                    crate::queries::refresh(&window, &state);
                    if window.connection_id() == id.as_str() {
                        // nothing is active any more, so this resets the title bar
                        crate::connect_active(&window, state.clone(), &rt);
                        crate::keyspace::reload(&app);
                    }
                }
                Err(e) => window.global::<ConnectionEditor>().set_error(e.into()),
            }
        }
    });

    // "Test connection" in the form: builds the connection as the form describes it, unsaved
    window.global::<ConnectionEditor>().on_test_requested({
        let (weak, state, rt) = (window.as_weak(), state.clone(), rt.clone());
        move |form| {
            let Some(window) = weak.upgrade() else { return };
            window.global::<ConnectionEditor>().set_test_status("".into());
            let payload = match payload_from_form(&form) {
                Ok(payload) => payload,
                Err(e) => {
                    window.global::<ConnectionEditor>().set_test_ok(false);
                    window.global::<ConnectionEditor>().set_test_status(e.into());
                    return;
                }
            };
            window.global::<ConnectionEditor>().set_testing(true);
            let (weak, state, id) = (weak.clone(), state.clone(), form.id.to_string());
            rt.spawn(async move {
                let building = state.clone();
                let built = tokio::task::spawn_blocking(move || {
                    staged(&building.connections, |store| {
                        store.unlock_secrets(&id)?;
                        store.preview(&id, payload.clone())
                    })
                })
                .await
                .map_err(AppError::from)
                .and_then(|built| built);
                let outcome = match built {
                    Ok(connection) => crate::backend::connection_service::test_connection(&state, connection).await,
                    Err(e) => Err(e),
                };
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(window) = weak.upgrade() else { return };
                    window.global::<ConnectionEditor>().set_testing(false);
                    window.global::<ConnectionEditor>().set_test_ok(outcome.is_ok());
                    window.global::<ConnectionEditor>().set_test_status(outcome.map_or_else(|e| e.to_string(), |()| t!("Connection successful.").into_owned()).into());
                });
            });
        }
    });

    window.global::<ConnectionEditor>().on_saved({
        let weak = window.as_weak();
        let state = state.clone();
        let rt = rt.clone();
        move |form| {
            let Some(window) = weak.upgrade() else { return };
            let payload = match payload_from_form(&form) {
                Ok(payload) => payload,
                Err(e) => {
                    window.global::<ConnectionEditor>().set_error(e.into());
                    return;
                }
            };
            // off the UI thread: storing the secrets can wait on the OS credential
            // store (a first-time setup, an unlock prompt)
            let state = state.clone();
            let id = form.id.to_string();
            rt.spawn_blocking(move || {
                let new_id = uuid::Uuid::new_v4().to_string();
                let result = staged(&state.connections, |store| {
                    if id.is_empty() { store.create_with_id(&new_id, payload.clone()) } else { store.update(&id, payload.clone()) }
                });
                // an open pool still reaches the server as it was before the edit
                if result.is_ok() && !id.is_empty() {
                    state.redis_connections.invalidate(&id);
                }
                let _ = slint::invoke_from_event_loop(move || crate::app::with(|app, window| match result {
                    Ok(_) => {
                        window.global::<ConnectionEditor>().set_error("".into());
                        window.global::<ConnectionEditor>().set_open(false);
                        refresh(app, window);
                        // (the connection in use may be the one that was edited)
                        crate::connection_profiles::apply_tint(window, &app.state);
                    }
                    Err(e) => window.global::<ConnectionEditor>().set_error(e.into()),
                }));
            });
        }
    });

    // "Browse…" beside the SSH private key
    window.global::<ConnectionEditor>().on_browse_key({
        let (weak, rt) = (window.as_weak(), rt.clone());
        move || browse_ssh_key(&rt, weak.clone(), |window, path| window.global::<ConnectionEditor>().set_ssh_key(path.into()))
    });
}

/// Opens a native file dialog for an SSH private key, on ~/.ssh when there is one, off the UI
/// thread; `set` puts the chosen path into the window.
pub(crate) fn browse_ssh_key(rt: &Handle, weak: slint::Weak<MainWindow>, set: fn(&MainWindow, &str)) {
    rt.spawn_blocking(move || {
        let mut dialog = rfd::FileDialog::new().set_title(t!("Select the SSH private key"));
        if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
            let ssh_dir = std::path::PathBuf::from(home).join(".ssh");
            if ssh_dir.is_dir() {
                dialog = dialog.set_directory(ssh_dir);
            }
        }
        let Some(path) = dialog.pick_file() else { return };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(window) = weak.upgrade() {
                set(&window, path.to_string_lossy().as_ref());
            }
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
mod tests {
    use super::*;

    fn form() -> ConnectionForm {
        ConnectionForm {
            id: "".into(),
            name: "prod".into(),
            connection_url: "redis://10.0.0.5:6379".into(),
            username: "".into(),
            password: "".into(),
            ssh_enabled: true,
            ssh_host: "bastion".into(),
            ssh_port: "2222".into(),
            ssh_username: "deploy".into(),
            ssh_password: "".into(),
            ssh_key: "~/.ssh/id_ed25519".into(),
            ssh_passphrase: "".into(),
            profile_id: "production".into(),
        }
    }

    #[test]
    fn a_blank_password_box_keeps_the_stored_password_and_everything_else_is_taken_as_typed() {
        let payload = payload_from_form(&form()).unwrap();
        assert_eq!((payload.password, payload.ssh_password, payload.ssh_passphrase), (None, None, None));
        assert_eq!((payload.ssh_enabled, payload.ssh_port), (Some(true), Some(2222)));
        assert_eq!(payload.ssh_private_key.as_deref(), Some("~/.ssh/id_ed25519"));
        assert_eq!(payload.profile_id.as_deref(), Some("production"));
        assert_eq!(payload_from_form(&ConnectionForm { profile_id: "".into(), ..form() }).unwrap().profile_id.as_deref(), Some(""), "no profile is an empty one, not a kept one");

        let typed = payload_from_form(&ConnectionForm { password: "a".into(), ssh_password: "b".into(), ssh_passphrase: "c".into(), ..form() }).unwrap();
        assert_eq!((typed.password.as_deref(), typed.ssh_password.as_deref(), typed.ssh_passphrase.as_deref()), (Some("a"), Some("b"), Some("c")));
    }

    #[test]
    fn the_ssh_port_defaults_to_22_and_must_be_a_number() {
        assert_eq!(payload_from_form(&ConnectionForm { ssh_port: "".into(), ..form() }).unwrap().ssh_port, Some(22));
        assert_eq!(payload_from_form(&ConnectionForm { ssh_port: " 2200 ".into(), ..form() }).unwrap().ssh_port, Some(2200));
        for bad in ["ssh", "0", "65536", "-1", "70000"] {
            assert!(payload_from_form(&ConnectionForm { ssh_port: bad.into(), ..form() }).is_err(), "{bad}");
        }
    }
}

