//! Wires the settings window (SSH profiles) to `cachemire_core::settings`,
//! which handles persistence and the credential store. This is UI glue:
//! keep the list in sync, switch/save/remove a profile.

use crate::error::Localize;
use cachemire_core::error::Error;
use cachemire_core::secret_vault::{SecretsPolicy, staged};
use std::rc::Rc;
use std::sync::Arc;

use slint::{ComponentHandle, ModelRc, VecModel};
use tokio::runtime::Handle;

use crate::app::App;
use crate::{ExportPrefs, KeyView, LanguageSetting, MainWindow, OpeningView, Preferences, SshProfileForm, SshProfileRowData};
use cachemire_core::i18n::Language;
use cachemire_core::settings::OpeningView as StoredView;
use cachemire_core::state::AppState;

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
        .ssh_profiles()
        .into_iter()
        .map(|p| SshProfileRowData { id: p.id.into(), name: p.name.into(), username: p.username.into(), active: p.active })
        .collect();
    window.global::<Preferences>().set_ssh_profiles(ModelRc::new(VecModel::from(rows)));
}

/// Adds the profile the form holds, or changes the one it names: a blank
/// password box on an existing profile keeps its password.
fn save_profile(state: &AppState, form: &SshProfileForm, policy: SecretsPolicy) -> Result<(), Error> {
    let new_id = uuid::Uuid::new_v4().to_string();
    staged(&state.settings, |settings| {
        if form.id.is_empty() {
            settings.create_ssh_profile_with_id(&new_id, &form.name, &form.username, &form.password, policy).map(|_| ())
        } else {
            let password = (!form.password.is_empty()).then_some(form.password.as_str());
            settings.update_ssh_profile(&form.id, &form.name, &form.username, password, policy)
        }
    })
}

/// [`save_profile`] off the UI thread: storing the password can wait on the OS credential
/// store (a first-time setup, an unlock prompt). If the keyring is unavailable, the user is
/// asked whether to keep the password in the settings file.
fn save_profile_off_thread(app: &Rc<App>, form: SshProfileForm, policy: SecretsPolicy) {
    let state = app.state.clone();
    app.rt.spawn_blocking(move || {
        let result = save_profile(&state, &form, policy);
        if result.is_ok() {
            drop_profile_pools(&state);
        }
        crate::app::later(move |app, window| match result {
            Ok(()) => {
                window.global::<Preferences>().set_ssh_profile_editing(false);
                refresh(window, &app.state);
            }
            Err(e) if crate::keyring_dialogs::is_unavailable(&e) => {
                let retry = Box::new(move |app: &Rc<App>, _: &MainWindow, policy| save_profile_off_thread(app, form, policy));
                crate::keyring_dialogs::ask_keyring(app, window, false, retry);
            }
            Err(e) => window.global::<Preferences>().set_error(e.localized().into()),
        });
    });
}

/// Drops the open pools of the connections whose SSH tunnel logs in with the
/// active profile: they'd go on using the profile as it was.
fn drop_profile_pools(state: &AppState) {
    let connections = state.connections.lock().list_state().connections;
    for connection in connections.iter().filter(|c| c.ssh.as_ref().is_some_and(|ssh| ssh.username.is_empty())) {
        state.redis_connections.invalidate(&connection.id);
    }
}

/// Shows the outcome of saving a setting: the error, or none and `apply` (the window's own
/// copy of the new value).
fn saved(window: &MainWindow, result: Result<(), Error>, apply: impl FnOnce(&MainWindow)) {
    match result {
        Ok(()) => {
            window.global::<Preferences>().set_error("".into());
            apply(window);
        }
        Err(e) => window.global::<Preferences>().set_error(e.localized().into()),
    }
}

/// Whether the settings' search finds `text`: every word of `query` is in it, whatever
/// their case and accents.
pub fn found(query: &str, text: &str) -> bool {
    let text = fold(text);
    fold(query).split_whitespace().all(|word| text.contains(word))
}

fn fold(text: &str) -> String {
    text.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'ç' => 'c',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            'ÿ' => 'y',
            c => c,
        })
        .collect()
}

pub fn init(window: &MainWindow, state: Arc<AppState>, rt: Handle) {
    refresh(window, &state);
    window.global::<Preferences>().on_search_matches(|query, text| found(&query, &text));
    window.global::<Preferences>().set_language(to_setting(state.settings.lock().language()));
    window.global::<Preferences>().set_server_search(state.settings.lock().server_search());
    window.global::<Preferences>().set_check_updates_at_startup(state.settings.lock().check_updates_at_startup());
    window.global::<Preferences>().set_restore_session(state.settings.lock().restore_session());
    let export_escaped = state.settings.lock().export_escaped();
    window.global::<Preferences>().set_export_escaped(export_escaped);
    window.global::<ExportPrefs>().set_escaped(export_escaped);
    window.global::<ExportPrefs>().set_header_escaped(export_escaped);

    // the export format buttons: saved as what new tabs start with; open tabs keep their own
    window.global::<Preferences>().on_export_escaped_selected({
        let weak = window.as_weak();
        let state = state.clone();
        move |escaped| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock().set_export_escaped(escaped);
            saved(&window, result, |window| window.global::<Preferences>().set_export_escaped(escaped));
        }
    });
    let (opening, last_table) = {
        let settings = state.settings.lock();
        (settings.opening_view(), settings.last_view_table())
    };
    window.global::<Preferences>().set_opening_view(match opening {
        StoredView::Fields => OpeningView::Fields,
        StoredView::Table => OpeningView::Table,
        StoredView::Last => OpeningView::Last,
    });
    window.global::<Preferences>().set_last_view(if last_table { KeyView::Table } else { KeyView::Fields });

    // the view new key tabs start on: saved, and used by the next tab opened
    window.global::<Preferences>().on_opening_view_selected({
        let weak = window.as_weak();
        let state = state.clone();
        move |view| {
            let Some(window) = weak.upgrade() else { return };
            let stored = match view {
                OpeningView::Fields => StoredView::Fields,
                OpeningView::Table => StoredView::Table,
                OpeningView::Last => StoredView::Last,
            };
            let result = state.settings.lock().set_opening_view(stored);
            saved(&window, result, |window| window.global::<Preferences>().set_opening_view(view));
        }
    });

    // a view picked in a key's tab: kept, for new tabs to start on when the setting says so
    window.global::<Preferences>().on_view_picked({
        let weak = window.as_weak();
        let state = state.clone();
        move |view| {
            let Some(window) = weak.upgrade() else { return };
            window.global::<Preferences>().set_last_view(view);
            if let Err(e) = state.settings.lock().set_last_view_table(view == KeyView::Table) {
                tracing::warn!("the last view picked can't be saved: {e}");
            }
        }
    });
    window.global::<Preferences>().set_search_depth(state.settings.lock().search_depth() as i32);

    // how deep into a key the global search reads: saved, and used by the next search
    window.global::<Preferences>().on_search_depth_selected({
        let weak = window.as_weak();
        let state = state.clone();
        move |depth| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock().set_search_depth(depth as u32);
            saved(&window, result, |window| window.global::<Preferences>().set_search_depth(depth));
        }
    });

    // the experimental search on the server: saved, and used by the next search
    window.global::<Preferences>().on_server_search_toggled({
        let weak = window.as_weak();
        let state = state.clone();
        move |enabled| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock().set_server_search(enabled);
            saved(&window, result, |window| window.global::<Preferences>().set_server_search(enabled));
        }
    });

    // the update check at startup: saved, and done from the next start on
    window.global::<Preferences>().on_check_updates_at_startup_toggled({
        let weak = window.as_weak();
        let state = state.clone();
        move |enabled| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock().set_check_updates_at_startup(enabled);
            saved(&window, result, |window| window.global::<Preferences>().set_check_updates_at_startup(enabled));
        }
    });

    // reopening the last connection at startup: saved; turned off, what was kept of it goes
    window.global::<Preferences>().on_restore_session_toggled({
        let weak = window.as_weak();
        let state = state.clone();
        move |enabled| {
            let Some(window) = weak.upgrade() else { return };
            let result = state.settings.lock().set_restore_session(enabled);
            if result.is_ok() && !enabled {
                crate::last_session::forget(&state);
            }
            saved(&window, result, |window| window.global::<Preferences>().set_restore_session(enabled));
        }
    });

    // the language buttons: saved, and switched to at once
    window.global::<Preferences>().on_language_selected({
        let weak = window.as_weak();
        let state = state.clone();
        move |setting| {
            let Some(window) = weak.upgrade() else { return };
            let language = from_setting(setting);
            let result = state.settings.lock().set_language(language);
            saved(&window, result, |window| {
                window.global::<Preferences>().set_language(setting);
                crate::i18n::apply(language);
                crate::shortcuts::refresh(window, &state);
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
                let mut settings = state.settings.lock();
                let next = if settings.active_ssh_profile() == id.as_str() { "" } else { id.as_str() };
                settings.set_active_ssh_profile(next)
            };
            if result.is_ok() {
                drop_profile_pools(&state);
            }
            window.global::<Preferences>().set_error(result.err().map(|e| e.localized().into()).unwrap_or_default());
            refresh(&window, &state);
        }
    });

    window.global::<Preferences>().on_ssh_profile_saved(|form| {
        crate::app::with(|app, window| {
            window.global::<Preferences>().set_error("".into());
            save_profile_off_thread(app, form, SecretsPolicy::VaultOrFail);
        });
    });

    window.global::<Preferences>().on_ssh_profile_deleted({
        let weak = window.as_weak();
        let state = state.clone();
        move |id| {
            let Some(window) = weak.upgrade() else { return };
            let removed = state.settings.lock().remove_ssh_profile(&id);
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
                Err(e) => window.global::<Preferences>().set_error(e.localized().into()),
            }
        }
    });
}
