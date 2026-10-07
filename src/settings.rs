//! Wires the settings window (SSH profiles) to `backend::settings_store`,
//! which handles persistence and the credential store. This is UI glue:
//! keep the list in sync, switch/save/remove a profile.

use crate::backend::LockExt;
use crate::backend::error::AppError;
use crate::backend::secret_vault::staged;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::runtime::Handle;

use crate::backend::state::AppState;
use crate::i18n::Language;
use crate::{ExportPrefs, LanguageSetting, MainWindow, Preferences, SshProfileForm, SshProfileRowData};

/// The UI's enum for a language setting, and back: matches, so a language added
/// to one and not the other doesn't compile.
fn to_setting(language: Language) -> LanguageSetting {
    match language {
        Language::Automatic => LanguageSetting::Automatic,
        Language::English => LanguageSetting::English,
        Language::French => LanguageSetting::French,
    }
}

fn from_setting(setting: LanguageSetting) -> Language {
    match setting {
        LanguageSetting::Automatic => Language::Automatic,
        LanguageSetting::English => Language::English,
        LanguageSetting::French => Language::French,
    }
}

fn refresh(window: &MainWindow, state: &Arc<AppState>) {
    let rows: Vec<SshProfileRowData> = state
        .settings
        .lock()
        .unwrap()
        .ssh_profiles()
        .into_iter()
        .map(|p| SshProfileRowData { id: p.id.into(), name: p.name.into(), username: p.username.into(), active: p.active })
        .collect();
    window.global::<Preferences>().set_ssh_profiles(ModelRc::new(VecModel::from(rows)));
}

/// Adds the profile the form holds, or changes the one it names: a blank
/// password box on an existing profile keeps its password.
fn save_profile(state: &AppState, form: &SshProfileForm) -> Result<(), AppError> {
    let new_id = uuid::Uuid::new_v4().to_string();
    staged(&state.settings, |settings| {
        if form.id.is_empty() {
            settings.create_ssh_profile_with_id(&new_id, &form.name, &form.username, &form.password).map(|_| ())
        } else {
            let password = (!form.password.is_empty()).then_some(form.password.as_str());
            settings.update_ssh_profile(&form.id, &form.name, &form.username, password)
        }
    })
}

/// Drops the open pools of the connections whose SSH tunnel logs in with the
/// active profile: they'd go on using the profile as it was.
fn drop_profile_pools(state: &AppState) {
    let connections = state.connections.lock_recover().list_state().connections;
    for connection in connections.iter().filter(|c| c.ssh.as_ref().is_some_and(|ssh| ssh.username.is_empty())) {
        state.redis_connections.invalidate(&connection.id);
    }
}

/// Shows the outcome of saving a setting: the error, or none and `apply` (the window's own
/// copy of the new value).
fn saved(window: &MainWindow, result: Result<(), AppError>, apply: impl FnOnce(&MainWindow)) {
    match result {
        Ok(()) => {
            window.global::<Preferences>().set_error("".into());
            apply(window);
        }
        Err(e) => window.global::<Preferences>().set_error(e.into()),
    }
}

pub fn init(window: &MainWindow, state: Arc<AppState>, rt: Handle) {
    refresh(window, &state);
    window.global::<Preferences>().set_language(to_setting(state.settings.lock_recover().language()));
    window.global::<Preferences>().set_server_search(state.settings.lock_recover().server_search());
    let export_escaped = state.settings.lock_recover().export_escaped();
    window.global::<Preferences>().set_export_escaped(export_escaped);
    window.global::<ExportPrefs>().set_escaped(export_escaped);
    window.global::<ExportPrefs>().set_header_escaped(export_escaped);

    // the export format buttons: saved as what new tabs start with; open tabs keep their own
    window.global::<Preferences>().on_export_escaped_selected({
        let weak = window.as_weak();
        let state = state.clone();
        move |escaped| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock_recover().set_export_escaped(escaped);
            saved(&window, result, |window| window.global::<Preferences>().set_export_escaped(escaped));
        }
    });
    window.global::<Preferences>().set_search_depth(state.settings.lock_recover().search_depth() as i32);

    // how deep into a key the global search reads: saved, and used by the next search
    window.global::<Preferences>().on_search_depth_selected({
        let weak = window.as_weak();
        let state = state.clone();
        move |depth| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock_recover().set_search_depth(depth as u32);
            saved(&window, result, |window| window.global::<Preferences>().set_search_depth(depth));
        }
    });

    // the experimental search on the server: saved, and used by the next search
    window.global::<Preferences>().on_server_search_toggled({
        let weak = window.as_weak();
        let state = state.clone();
        move |enabled| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock_recover().set_server_search(enabled);
            saved(&window, result, |window| window.global::<Preferences>().set_server_search(enabled));
        }
    });

    // the language buttons: saved, and switched to at once
    window.global::<Preferences>().on_language_selected({
        let weak = window.as_weak();
        let state = state.clone();
        move |setting| {
            let Some(window) = weak.upgrade() else { return };
            let language = from_setting(setting);
            let result = state.settings.lock_recover().set_language(language);
            saved(&window, result, |window| {
                window.global::<Preferences>().set_language(setting);
                crate::i18n::apply(language);
            });
        }
    });

    // a click on a profile makes it the active one; on the active one, none
    window.global::<Preferences>().on_ssh_profile_activated({
        let weak = window.as_weak();
        let state = state.clone();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            let result = {
                let mut settings = state.settings.lock_recover();
                let next = if settings.active_ssh_profile() == id.as_str() { "" } else { id.as_str() };
                settings.set_active_ssh_profile(next)
            };
            if result.is_ok() {
                drop_profile_pools(&state);
            }
            window.global::<Preferences>().set_error(result.err().map(Into::into).unwrap_or_default());
            refresh(&window, &state);
        }
    });

    window.global::<Preferences>().on_ssh_profile_saved({
        let weak = window.as_weak();
        let state = state.clone();
        let rt = rt.clone();
        move |form| {
            let Some(window) = weak.upgrade() else { return };
            window.global::<Preferences>().set_error("".into());
            // off the UI thread: storing the password can wait on the OS
            // credential store (a first-time setup, an unlock prompt)
            let weak = weak.clone();
            let state = state.clone();
            rt.spawn_blocking(move || {
                let result = save_profile(&state, &form);
                if result.is_ok() {
                    drop_profile_pools(&state);
                }
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(window) = weak.upgrade() else { return };
                    match result {
                        Ok(()) => {
                            window.global::<Preferences>().set_ssh_profile_editing(false);
                            refresh(&window, &state);
                        }
                        Err(e) => window.global::<Preferences>().set_error(e.into()),
                    }
                });
            });
        }
    });

    window.global::<Preferences>().on_ssh_profile_deleted({
        let weak = window.as_weak();
        let state = state.clone();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            let removed = state.settings.lock_recover().remove_ssh_profile(&id);
            match removed {
                Ok(cleanup) => {
                    drop_profile_pools(&state);
                    if let Some(cleanup) = cleanup {
                        rt.spawn_blocking(move || cleanup.run());
                    }
                    window.global::<Preferences>().set_error("".into());
                    window.global::<Preferences>().set_ssh_profile_editing(false);
                    refresh(&window, &state);
                }
                Err(e) => window.global::<Preferences>().set_error(e.into()),
            }
        }
    });
}
