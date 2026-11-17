//! The windows over the app: Settings (with its About page: version and updates), and the
//! connections' import and export.

use super::*;
use crate::{About, ConnectionTransfer, Preferences, SettingsCategory, SshProfileForm};
use cachemire_core::connection::ConnectionPayloadInput;
use cachemire_core::updates::{Channel, Release};

/// The Settings window, open, over a state the test can read.
fn settings_open(state: &std::sync::Arc<cachemire_core::state::AppState>) -> (Harness, tokio::runtime::Runtime) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut h = Harness::new();
    crate::settings::init(&h.window, state.clone(), runtime.handle().clone());
    // the language is the whole process's: a click on its buttons mustn't reach the other tests
    h.window.global::<Preferences>().on_language_selected(|_| {});
    h.window.global::<Preferences>().set_open(true);
    h.frame();
    (h, runtime)
}

/// Puts the Settings window back open on its first page, after a click that left it.
fn back_to_settings(h: &Harness) {
    let preferences = h.window.global::<Preferences>();
    preferences.set_ssh_profile_editing(false);
    preferences.set_conn_profile_editing(false);
    preferences.set_category(crate::SettingsCategory::General);
    preferences.set_open(true);
}

#[test]
fn the_startup_update_check_box_saves_the_setting_both_ways() {
    let state = test_state();
    let (mut h, _rt) = settings_open(&state);
    assert!(!h.window.global::<Preferences>().get_check_updates_at_startup(), "off by default");

    let to_about = |h: &Harness| {
        back_to_settings(h);
        h.window.global::<Preferences>().set_category(crate::SettingsCategory::About);
    };
    to_about(&h);
    h.frame();
    let on = |h: &Harness| h.window.global::<Preferences>().get_check_updates_at_startup();
    let at = sweep(&mut h, 300..1000, 100..780, 6, to_about, on).expect("the box was found and ticked");
    assert!(state.settings.lock().check_updates_at_startup(), "ticking it saves it");

    h.click(at.0, at.1);
    assert!(!on(&h), "a second click unticks it");
    assert!(!state.settings.lock().check_updates_at_startup());
}

#[test]
fn the_settings_window_starts_on_what_is_saved() {
    let state = test_state();
    {
        let mut settings = state.settings.lock();
        settings.set_check_updates_at_startup(true).unwrap();
        settings.set_search_depth(200).unwrap();
        settings.set_server_search(true).unwrap();
        settings.set_export_escaped(true).unwrap();
    }
    let (h, _rt) = settings_open(&state);
    let preferences = h.window.global::<Preferences>();
    assert!(preferences.get_check_updates_at_startup());
    assert_eq!(preferences.get_search_depth(), 200);
    assert!(preferences.get_server_search());
    assert!(preferences.get_export_escaped());
    assert_eq!(h.window.global::<crate::ExportPrefs>().get_format(), 1, "new tabs start on escaped JSON");
}

#[test]
fn the_search_depth_buttons_save_the_depth_picked() {
    let state = test_state();
    let (mut h, _rt) = settings_open(&state);
    let picked = |h: &Harness| h.window.global::<Preferences>().get_search_depth() == 60;
    sweep(&mut h, 300..1000, 100..780, 6, back_to_settings, picked).expect("the 60 button");
    assert_eq!(state.settings.lock().search_depth(), 60);
}

/// The Settings window in a running app, for what it saves off the UI thread.
fn settings_in_app() -> (Harness, Rc<crate::app::App>, tokio::runtime::Runtime, EventLoopTurn) {
    let state = test_state();
    let h = Harness::new();
    let (app, runtime, turn) = install_app(&h, &state);
    crate::settings::init(&h.window, state, runtime.handle().clone());
    h.window.global::<Preferences>().on_language_selected(|_| {});
    (h, app, runtime, turn)
}

/// The SSH profiles listed: (name, username, active).
fn ssh_profiles(h: &Harness) -> Vec<(String, String, bool)> {
    h.window.global::<Preferences>().get_ssh_profiles().iter().map(|p| (p.name.to_string(), p.username.to_string(), p.active)).collect()
}

/// What an SSH tunnel with no login of its own logs in with: the active profile's username and password.
fn tunnel_login(state: &cachemire_core::state::AppState) -> Option<(String, String)> {
    let mut ssh = cachemire_core::connection::SshSettings {
        host: "bastion".into(),
        port: 22,
        username: String::new(),
        password: Default::default(),
        private_key: String::new(),
        passphrase: Default::default(),
    };
    state.settings.lock().fill_ssh(&mut ssh).ok()?;
    Some((ssh.username, ssh.password.expose().to_string()))
}

#[test]
fn the_first_ssh_profile_added_is_the_one_tunnels_log_in_with_until_a_click_turns_it_off() {
    let (mut h, app, _rt, _turn) = settings_in_app();
    let preferences = h.window.global::<Preferences>();
    preferences.set_ssh_profile_editing(true);
    let form = SshProfileForm { id: "".into(), name: "Bastion".into(), username: "deploy".into(), password: "pw".into() };
    preferences.invoke_ssh_profile_saved(form);
    assert!(wait_for(&mut h, |h| !ssh_profiles(h).is_empty()), "the profile is saved");
    assert_eq!(ssh_profiles(&h), [("Bastion".to_string(), "deploy".to_string(), true)]);
    assert!(!h.window.global::<Preferences>().get_ssh_profile_editing(), "the form closes");
    assert_eq!(tunnel_login(&app.state), Some(("deploy".to_string(), "pw".to_string())));

    let id = h.window.global::<Preferences>().get_ssh_profiles().row_data(0).unwrap().id;
    h.window.global::<Preferences>().invoke_ssh_profile_activated(id.clone());
    assert!(!ssh_profiles(&h)[0].2, "a click on the active one leaves none active");
    assert_eq!(tunnel_login(&app.state), None);
    h.window.global::<Preferences>().invoke_ssh_profile_activated(id);
    assert!(ssh_profiles(&h)[0].2, "and another makes it active again");

    // a form that can't be saved says why and stays open
    h.window.global::<Preferences>().set_ssh_profile_editing(true);
    let unnamed = SshProfileForm { id: "".into(), name: " ".into(), username: "x".into(), password: "".into() };
    h.window.global::<Preferences>().invoke_ssh_profile_saved(unnamed);
    assert!(wait_for(&mut h, |h| !h.window.global::<Preferences>().get_error().is_empty()));
    assert!(h.window.global::<Preferences>().get_ssh_profile_editing());
    assert_eq!(ssh_profiles(&h).len(), 1);
}

#[test]
fn an_ssh_profile_edited_with_a_blank_password_keeps_its_own_and_can_be_deleted() {
    let (mut h, app, _rt, _turn) = settings_in_app();
    let policy = cachemire_core::secret_vault::SecretsPolicy::PlainText;
    let id = app.state.settings.lock().create_ssh_profile_with_id("p1", "Bastion", "deploy", "pw", policy).unwrap();
    let preferences = h.window.global::<Preferences>();
    let form = SshProfileForm { id: id.as_str().into(), name: "Jump host".into(), username: "ops".into(), password: "".into() };
    preferences.invoke_ssh_profile_saved(form);
    assert!(wait_for(&mut h, |h| ssh_profiles(h).first().is_some_and(|p| p.0 == "Jump host")));
    assert_eq!(tunnel_login(&app.state), Some(("ops".to_string(), "pw".to_string())), "the password stays");

    h.window.global::<Preferences>().set_ssh_profile_editing(true);
    h.window.global::<Preferences>().invoke_ssh_profile_deleted(id.as_str().into());
    assert!(ssh_profiles(&h).is_empty());
    assert!(!h.window.global::<Preferences>().get_ssh_profile_editing(), "its form closes");
    assert_eq!(tunnel_login(&app.state), None);
}

#[test]
fn the_fonts_and_zoom_picked_are_saved_and_used_at_once() {
    let (h, app, _rt, _turn) = settings_in_app();
    let preferences = h.window.global::<Preferences>();
    preferences.invoke_ui_font_selected("Noto Sans".into());
    preferences.invoke_mono_font_selected("Fira Code".into());
    let theme = h.window.global::<crate::Theme>();
    assert_eq!((theme.get_font_ui().as_str(), theme.get_font_mono().as_str()), ("Noto Sans", "Fira Code"));
    let settings = app.state.settings.lock();
    assert_eq!((settings.ui_font(), settings.mono_font()), ("Noto Sans", "Fira Code"));
    drop(settings);

    preferences.invoke_ui_font_selected("".into());
    assert_eq!(theme.get_font_ui(), "Roboto", "none picked: the app's own");

    preferences.invoke_zoom_selected(150);
    assert_eq!((preferences.get_zoom(), theme.get_zoom()), (150, 1.5));
    preferences.invoke_zoom_selected(500);
    assert_eq!(preferences.get_zoom(), 200, "kept within the slider's marks");
    assert_eq!(app.state.settings.lock().zoom(), 200);
}

#[test]
fn new_tabs_can_start_on_the_view_last_picked() {
    let (h, app, _rt, _turn) = settings_in_app();
    let preferences = h.window.global::<Preferences>();
    preferences.invoke_opening_view_selected(crate::OpeningView::Last);
    assert_eq!(preferences.get_opening_view(), crate::OpeningView::Last);
    assert_eq!(app.state.settings.lock().opening_view(), cachemire_core::settings::OpeningView::Last);

    preferences.invoke_view_picked(crate::KeyView::Table);
    assert_eq!(preferences.get_last_view(), crate::KeyView::Table);
    assert!(app.state.settings.lock().last_view_table(), "remembered for the next start");
}

#[test]
fn turning_off_the_session_restore_forgets_the_session_kept() {
    let (h, app, _rt, _turn) = settings_in_app();
    let file = app.state.settings.lock().file_path().with_file_name("last_session.json");
    let preferences = h.window.global::<Preferences>();
    preferences.invoke_restore_session_toggled(true);
    std::fs::write(&file, r#"{"connection": "e2e"}"#).unwrap();
    assert!(preferences.get_restore_session());

    preferences.invoke_restore_session_toggled(false);
    assert!(!preferences.get_restore_session());
    assert!(!app.state.settings.lock().restore_session());
    assert!(!file.exists(), "nothing is left to reopen");
}

#[test]
fn new_ssh_host_keys_can_be_accepted_without_asking() {
    let (h, app, _rt, _turn) = settings_in_app();
    let preferences = h.window.global::<Preferences>();
    let before = app.state.settings.lock().accept_new_ssh_host_keys();
    preferences.invoke_accept_new_ssh_host_keys_toggled(!before);
    assert_eq!(preferences.get_accept_new_ssh_host_keys(), !before);
    assert_eq!(app.state.settings.lock().accept_new_ssh_host_keys(), !before);
}

#[test]
fn the_settings_search_ignores_case_and_accents_and_needs_every_word() {
    let (h, _rt) = settings_open(&test_state());
    let matches = |query: &str, text: &str| h.window.global::<Preferences>().invoke_search_matches(query.into(), text.into());
    assert!(matches("DEMARRAGE", "Restaurer au démarrage"));
    assert!(matches("dé", "Delete"), "an accent typed matches the plain letter too");
    assert!(matches("zoom molette", "Molette de la souris : zoom"), "the words in any order");
    assert!(!matches("zoom clavier", "Molette de la souris : zoom"), "every word must be there");
    assert!(matches("", "anything"));
}

/// The About page, its callbacks wired, and its link to the repository kept from opening a browser.
fn info_window(state: &std::sync::Arc<cachemire_core::state::AppState>) -> (Harness, tokio::runtime::Runtime, Rc<std::cell::Cell<usize>>) {
    let runtime = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
    let mut h = Harness::new();
    crate::updates::init(&h.window, state.clone(), runtime.handle().clone());
    let downloads = Rc::new(std::cell::Cell::new(0));
    let about = h.window.global::<About>();
    about.on_open_repo(|| {});
    about.on_download({
        let downloads = downloads.clone();
        move || downloads.set(downloads.get() + 1)
    });
    h.window.global::<Preferences>().invoke_show(SettingsCategory::About);
    h.frame();
    (h, runtime, downloads)
}

fn reopen_info(h: &Harness) {
    h.window.global::<Preferences>().invoke_show(SettingsCategory::About);
}

/// Whether the settings window is open on its About page.
fn on_about(h: &Harness) -> bool {
    let preferences = h.window.global::<Preferences>();
    preferences.get_open() && preferences.get_category() == SettingsCategory::About
}

#[test]
fn the_info_window_shows_the_version_and_a_dev_build_never_looks_for_updates() {
    let (mut h, _rt, _) = info_window(&test_state());
    let about = h.window.global::<About>();
    assert_eq!(about.get_version(), env!("CARGO_PKG_VERSION"));
    assert_eq!(Channel::of(&semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap()), Channel::Dev, "the tests run a dev build");

    let said = |h: &Harness| !h.window.global::<About>().get_status().is_empty();
    sweep(&mut h, 500..800, 200..600, 4, reopen_info, said).expect("the Check for updates button");
    let about = h.window.global::<About>();
    assert!(about.get_status().contains("development version"), "{}", about.get_status());
    assert!(!about.get_busy() && !about.get_update_available());
}

#[test]
fn the_channel_toggle_saves_the_channel_picked() {
    let state = test_state();
    let (mut h, _rt, _) = info_window(&state);
    assert_eq!(h.window.global::<About>().get_channel(), 0, "a dev build follows the stable releases");
    let beta = |h: &Harness| h.window.global::<About>().get_channel() == 1;
    sweep(&mut h, 500..800, 30..600, 4, reopen_info, beta).expect("the Beta segment");
    assert_eq!(state.settings.lock().update_channel(), Some(Channel::Beta));
}

#[test]
fn an_update_found_at_startup_opens_the_about_page_with_its_button() {
    let (mut h, _rt, downloads) = info_window(&test_state());
    let preferences = h.window.global::<Preferences>();
    preferences.set_open(false);
    preferences.set_category(SettingsCategory::General);
    let release = Release {
        version: semver::Version::new(9, 9, 9),
        page: "https://example.invalid/r".into(),
        installer: Some("https://example.invalid/i.msi".into()),
        installer_sha256: None,
    };
    crate::updates::show_found(&h.window, release.clone(), true);
    h.frame();
    let about = h.window.global::<About>();
    assert!(on_about(&h), "found at startup: the page opens by itself");
    assert!(about.get_status().contains("9.9.9"), "{}", about.get_status());
    let label = if cfg!(windows) { "Download and install" } else { "Open the release page" };
    assert_eq!(about.get_download_label(), label);

    // a click on Check or on the other channel takes the release back, as it should: it is
    // offered again after each miss
    let offer_again = |h: &Harness| crate::updates::show_found(&h.window, release.clone(), true);
    let clicked = |_: &Harness| downloads.get() > 0;
    sweep(&mut h, 500..800, 200..600, 4, offer_again, clicked).expect("the window shows the download button");
}

#[test]
fn an_update_found_by_the_button_doesnt_open_a_closed_window() {
    let (h, _rt, _) = info_window(&test_state());
    h.window.global::<Preferences>().set_open(false);
    let release = Release { version: semver::Version::new(9, 9, 9), page: String::new(), installer: None, installer_sha256: None };
    crate::updates::show_found(&h.window, release, false);
    assert!(!h.window.global::<Preferences>().get_open());
    assert!(h.window.global::<About>().get_update_available(), "it is offered the next time the window opens");
}

/// The import and export window, open, over a state holding `names` as connections.
fn transfer_open(names: &[&str]) -> (Harness, Rc<crate::app::App>, (tokio::runtime::Runtime, EventLoopTurn)) {
    let state = test_state();
    {
        let mut store = state.connections.lock();
        for (i, name) in names.iter().enumerate() {
            let payload = ConnectionPayloadInput {
                name: Some(name.to_string()),
                connection_url: Some(format!("redis://h{i}:6379")),
                ..Default::default()
            };
            store.create_with_id(&format!("c{i}"), payload, cachemire_core::secret_vault::SecretsPolicy::PlainText).unwrap();
        }
    }
    let h = Harness::new();
    let (app, runtime, turn) = install_app(&h, &state);
    crate::connections::transfer::init(&h.window);
    h.window.global::<ConnectionTransfer>().invoke_open_requested();
    (h, app, (runtime, turn))
}

#[test]
fn the_export_list_starts_with_every_connection_ticked_and_a_click_unticks_one() {
    let (mut h, _app, _rt) = transfer_open(&["Prod", "Staging", "Local"]);
    let transfer = h.window.global::<ConnectionTransfer>();
    // the store starts with its own "Default" connection
    let all = transfer.get_export_rows().row_count() as i32;
    assert_eq!(all, 4);
    assert_eq!(transfer.get_export_count(), all, "every connection is ticked");
    transfer.set_tab(1);
    h.frame();

    let reopen = |h: &Harness| {
        let transfer = h.window.global::<ConnectionTransfer>();
        transfer.set_open(true);
        transfer.set_tab(1);
    };
    let fewer = |h: &Harness| h.window.global::<ConnectionTransfer>().get_export_count() < all;
    sweep(&mut h, 450..850, 150..650, 6, reopen, fewer).expect("a row's box unticks it");
    let transfer = h.window.global::<ConnectionTransfer>();
    assert_eq!(transfer.get_export_count(), all - 1);
    let unticked = (0..all as usize).filter(|&i| !transfer.get_export_rows().row_data(i).unwrap().ticked).count();
    assert_eq!(unticked, 1, "the count follows the boxes");
}

#[test]
fn ticking_a_range_and_all_or_none_update_the_count() {
    let (h, _app, _rt) = transfer_open(&["A", "B", "C", "D"]);
    let transfer = h.window.global::<ConnectionTransfer>();
    transfer.invoke_set_all(1, false);
    assert_eq!(transfer.get_export_count(), 0);
    // a Shift-click from the second row to the fourth gives them the second's new state
    transfer.invoke_toggle(1, 1);
    transfer.invoke_toggle_range(1, 1, 3);
    assert_eq!(transfer.get_export_count(), 3);
    let ticked: Vec<bool> = (0..5).map(|i| transfer.get_export_rows().row_data(i).unwrap().ticked).collect();
    assert_eq!(ticked, [false, true, true, true, false]);
    transfer.invoke_set_all(1, true);
    assert_eq!(transfer.get_export_count(), 5);
}

#[test]
fn an_import_file_lists_its_connections_and_marks_names_already_taken() {
    let (h, app, _rt) = transfer_open(&["Prod"]);
    let file = r#"{"connections": [{"name": "prod", "connectionUrl": "redis://p:6379"}, {"name": "New", "host": "n", "ssh": {"host": "bastion"}}]}"#;
    crate::connections::transfer::show_file(&app, &h.window, file, "export.json");
    let transfer = h.window.global::<ConnectionTransfer>();
    assert_eq!(transfer.get_import_file(), "export.json");
    assert_eq!(transfer.get_import_count(), 2);
    let rows: Vec<crate::TransferRow> = (0..2).map(|i| transfer.get_import_rows().row_data(i).unwrap()).collect();
    assert_eq!(rows[0].note, "added as a copy", "a saved connection already has that name, whatever its case");
    assert_eq!(rows[1].note, "");
    assert!(rows[1].detail.contains("bastion"), "a tunnel shows where it goes through: {}", rows[1].detail);

    crate::connections::transfer::show_file(&app, &h.window, "not json", "bad.json");
    assert_eq!(transfer.get_import_rows().row_count(), 0);
    assert!(!transfer.get_message().is_empty(), "the file is refused with a reason");
    assert!(!transfer.get_message_ok());
}

#[test]
fn importing_saves_the_ticked_connections_under_free_names_and_says_so() {
    let (mut h, app, _rt) = transfer_open(&["Prod"]);
    let file = r#"[{"name": "Prod", "connectionUrl": "redis://p2:6379"}, {"name": "Skipped", "host": "s"}, {"name": "Cache", "host": "c", "port": 6390}]"#;
    crate::connections::transfer::show_file(&app, &h.window, file, "export.json");
    let transfer = h.window.global::<ConnectionTransfer>();
    transfer.invoke_toggle(0, 1);
    assert_eq!(transfer.get_import_count(), 2);
    transfer.invoke_run_import();
    assert!(wait_for(&mut h, |h| h.window.global::<ConnectionTransfer>().get_message_ok()), "the import reports back");

    let transfer = h.window.global::<ConnectionTransfer>();
    assert!(transfer.get_message().contains('2'), "{}", transfer.get_message());
    assert_eq!(transfer.get_import_rows().row_count(), 0, "the list is cleared once imported");
    let names: Vec<String> = app.state.connections.lock().list_state().connections.into_iter().map(|c| c.name).collect();
    assert!(names.contains(&"Prod (2)".to_string()) && names.contains(&"Cache".to_string()), "{names:?}");
    assert!(!names.contains(&"Skipped".to_string()), "an unticked row isn't imported");
    let listed = h.window.global::<crate::ConnectionState>().get_connections().row_count();
    assert_eq!(listed, names.len(), "the connection list shows them");
}
