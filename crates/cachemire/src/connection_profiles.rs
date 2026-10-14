//! Connection profiles (a name and a tint each, edited in Settings, chosen in a
//! connection's form): keeps the window's list in step with `backend::settings_store`
//! and tints the app background for the connection in use.

use std::rc::Rc;

use rust_i18n::t;
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::app::App;
use crate::backend::LockExt;
use crate::backend::models::{ConnectionItem, ProfileColor};
use crate::backend::state::AppState;
use crate::{ConnProfileRowData, MainWindow, Preferences, ProfileTint, Theme};

/// The UI's enum for a profile color, and back: matches, so a color added to one
/// and not the other doesn't compile.
pub(crate) fn to_tint(color: ProfileColor) -> ProfileTint {
    match color {
        ProfileColor::Red => ProfileTint::Red,
        ProfileColor::Orange => ProfileTint::Orange,
        ProfileColor::Yellow => ProfileTint::Yellow,
        ProfileColor::Green => ProfileTint::Green,
        ProfileColor::Cyan => ProfileTint::Cyan,
        ProfileColor::Blue => ProfileTint::Blue,
        ProfileColor::Purple => ProfileTint::Purple,
        ProfileColor::Pink => ProfileTint::Pink,
    }
}

fn from_tint(tint: ProfileTint) -> ProfileColor {
    match tint {
        ProfileTint::Red => ProfileColor::Red,
        ProfileTint::Orange => ProfileColor::Orange,
        ProfileTint::Yellow => ProfileColor::Yellow,
        ProfileTint::Green => ProfileColor::Green,
        ProfileTint::Cyan => ProfileColor::Cyan,
        ProfileTint::Blue => ProfileColor::Blue,
        ProfileTint::Purple => ProfileColor::Purple,
        ProfileTint::Pink => ProfileColor::Pink,
    }
}

/// Puts the profiles into the window: the Settings list, and the choices of the
/// connection form.
pub(crate) fn refresh(window: &MainWindow, state: &AppState) {
    let rows: Vec<ConnProfileRowData> = state
        .settings
        .lock_recover()
        .connection_profiles()
        .into_iter()
        .map(|p| ConnProfileRowData { id: p.id.into(), name: p.name.into(), tint: to_tint(p.color) })
        .collect();
    window.global::<Preferences>().set_conn_profiles(ModelRc::new(VecModel::from(rows)));
}

/// Tints the background for the connection in use — the one connected to, if
/// its profile still exists — or clears the tint. Called whenever that can have
/// changed: a connection made or lost, a connection or a profile edited.
pub(crate) fn apply_tint(window: &MainWindow, state: &AppState) {
    let connection_id = window.connection_id();
    let profile_id = (!connection_id.is_empty()).then(|| state.connections.lock_recover().get(&connection_id).map(|c| c.profile_id)).flatten();
    let color = profile_id.filter(|id| !id.is_empty()).and_then(|id| state.settings.lock_recover().connection_profile(&id)).map(|p| p.color);
    let theme = window.global::<Theme>();
    match color {
        Some(color) => {
            theme.set_env_tint(to_tint(color));
            theme.set_env_tinted(true);
        }
        None => theme.set_env_tinted(false),
    }
}

/// The names of the connections using profile `id`.
fn names_using(connections: &[ConnectionItem], id: &str) -> Vec<String> {
    connections.iter().filter(|c| c.profile_id == id).map(|c| c.name.clone()).collect()
}

/// Why a profile can't be deleted while `names` (the connections using it) exist.
fn in_use_message(names: &[String]) -> String {
    const SHOWN: usize = 3;
    let mut list = names.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if names.len() > SHOWN {
        list.push_str(&format!(" +{}", names.len() - SHOWN));
    }
    t!("This profile is still used by: %{connections}. Remove it from those connections first.", connections = list).into_owned()
}

pub fn init(app: &Rc<App>, window: &MainWindow) {
    let state = app.state.clone();
    refresh(window, &state);

    window.global::<Preferences>().on_conn_profile_saved({
        let app = app.clone();
        move |form| {
            let Some(window) = app.window.upgrade() else { return };
            let state = &app.state;
            let color = from_tint(form.tint);
            let saved = {
                let mut settings = state.settings.lock_recover();
                if form.id.is_empty() {
                    settings.create_connection_profile(&form.name, color).map(|_| ())
                } else {
                    settings.update_connection_profile(&form.id, &form.name, color)
                }
            };
            match saved {
                Ok(()) => {
                    window.global::<Preferences>().set_error("".into());
                    window.global::<Preferences>().set_conn_profile_editing(false);
                    refresh(&window, state);
                    // (the profile in use may be the one that was recolored)
                    apply_tint(&window, state);
                    crate::connections::refresh(&app, &window);
                }
                Err(e) => window.global::<Preferences>().set_error(e.into()),
            }
        }
    });

    // a profile goes only once no connection uses it
    window.global::<Preferences>().on_conn_profile_deleted({
        let app = app.clone();
        move |id| {
            let Some(window) = app.window.upgrade() else { return };
            let names = names_using(&state.connections.lock_recover().list_state().connections, &id);
            if !names.is_empty() {
                window.global::<Preferences>().set_error(in_use_message(&names).into());
                return;
            }
            let removed = state.settings.lock_recover().remove_connection_profile(&id);
            match removed {
                Ok(()) => {
                    window.global::<Preferences>().set_error("".into());
                    window.global::<Preferences>().set_conn_profile_editing(false);
                    refresh(&window, &state);
                    crate::connections::refresh(&app, &window);
                }
                Err(e) => window.global::<Preferences>().set_error(e.into()),
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connection(name: &str, profile_id: &str) -> ConnectionItem {
        ConnectionItem {
            id: name.into(),
            name: name.into(),
            connection_url: String::new(),
            username: String::new(),
            password: String::new(),
            ssh: None,
            profile_id: profile_id.into(),
        }
    }

    #[test]
    fn every_color_maps_to_its_own_tint_and_back() {
        for color in [
            ProfileColor::Red,
            ProfileColor::Orange,
            ProfileColor::Yellow,
            ProfileColor::Green,
            ProfileColor::Cyan,
            ProfileColor::Blue,
            ProfileColor::Purple,
            ProfileColor::Pink,
        ] {
            assert_eq!(from_tint(to_tint(color)), color);
        }
    }

    #[test]
    fn a_profile_is_in_use_by_the_connections_that_have_it() {
        let connections = [connection("cache", "prod"), connection("dev", "dev"), connection("queue", "prod"), connection("plain", "")];
        assert_eq!(names_using(&connections, "prod"), vec!["cache".to_string(), "queue".to_string()]);
        assert!(names_using(&connections, "staging").is_empty());
    }

    #[test]
    fn the_message_names_a_few_of_the_connections_and_counts_the_rest() {
        let few = in_use_message(&["cache".to_string(), "queue".to_string()]);
        assert!(few.contains("cache, queue") && !few.contains('+'), "{few}");
        let many = in_use_message(&["a".to_string(), "b".to_string(), "c".to_string(), "d".to_string(), "e".to_string()]);
        assert!(many.contains("a, b, c +2"), "{many}");
    }
}
