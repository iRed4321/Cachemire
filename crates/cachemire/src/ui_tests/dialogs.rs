//! The windows over the app: Settings, the info window (version and updates), and the
//! connections' import and export.

use super::*;
use cachemire_core::LockExt;
use cachemire_core::connection::ConnectionPayloadInput;
use cachemire_core::updates::{Channel, Release};
use crate::{About, ConnectionTransfer, Preferences};

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

/// Puts the Settings window back on its lists, open, after a click that left them.
fn back_to_settings(h: &Harness) {
    let preferences = h.window.global::<Preferences>();
    preferences.set_ssh_profile_editing(false);
    preferences.set_conn_profile_editing(false);
    preferences.set_open(true);
}

#[test]
fn the_startup_update_check_box_saves_the_setting_both_ways() {
    let state = test_state();
    let (mut h, _rt) = settings_open(&state);
    assert!(!h.window.global::<Preferences>().get_check_updates_at_startup(), "off by default");

    let on = |h: &Harness| h.window.global::<Preferences>().get_check_updates_at_startup();
    let at = sweep(&mut h, 300..1000, 100..780, 6, back_to_settings, on).expect("the box was found and ticked");
    assert!(state.settings.lock_recover().check_updates_at_startup(), "ticking it saves it");

    h.click(at.0, at.1);
    assert!(!on(&h), "a second click unticks it");
    assert!(!state.settings.lock_recover().check_updates_at_startup());
}

#[test]
fn the_settings_window_starts_on_what_is_saved() {
    let state = test_state();
    {
        let mut settings = state.settings.lock_recover();
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
    assert!(h.window.global::<crate::ExportPrefs>().get_escaped(), "new tabs start on escaped JSON");
}

#[test]
fn the_search_depth_buttons_save_the_depth_picked() {
    let state = test_state();
    let (mut h, _rt) = settings_open(&state);
    let picked = |h: &Harness| h.window.global::<Preferences>().get_search_depth() == 60;
    sweep(&mut h, 300..1000, 100..780, 6, back_to_settings, picked).expect("the 60 button");
    assert_eq!(state.settings.lock_recover().search_depth(), 60);
}

/// The info window, its callbacks wired, and its link to the repository kept from opening a browser.
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
    about.set_open(true);
    h.frame();
    (h, runtime, downloads)
}

fn reopen_info(h: &Harness) {
    h.window.global::<About>().set_open(true);
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
    sweep(&mut h, 500..800, 200..600, 4, reopen_info, beta).expect("the Beta segment");
    assert_eq!(state.settings.lock_recover().update_channel(), Some(Channel::Beta));
}

#[test]
fn an_update_found_at_startup_opens_the_info_window_with_its_button() {
    let (mut h, _rt, downloads) = info_window(&test_state());
    let about = h.window.global::<About>();
    about.set_open(false);
    let release = Release { version: semver::Version::new(9, 9, 9), page: "https://example.invalid/r".into(), installer: Some("https://example.invalid/i.msi".into()) };
    crate::updates::show_found(&about, release.clone(), true);
    h.frame();
    let about = h.window.global::<About>();
    assert!(about.get_open(), "found at startup: the window opens by itself");
    assert!(about.get_status().contains("9.9.9"), "{}", about.get_status());
    let label = if cfg!(windows) { "Download and install" } else { "Open the release page" };
    assert_eq!(about.get_download_label(), label);

    // a click on Check or on the other channel takes the release back, as it should: it is
    // offered again after each miss
    let offer_again = |h: &Harness| crate::updates::show_found(&h.window.global::<About>(), release.clone(), true);
    let clicked = |_: &Harness| downloads.get() > 0;
    sweep(&mut h, 500..800, 200..600, 4, offer_again, clicked).expect("the window shows the download button");
}

#[test]
fn an_update_found_by_the_button_doesnt_open_a_closed_window() {
    let (h, _rt, _) = info_window(&test_state());
    let about = h.window.global::<About>();
    about.set_open(false);
    let release = Release { version: semver::Version::new(9, 9, 9), page: String::new(), installer: None };
    crate::updates::show_found(&about, release, false);
    assert!(!about.get_open());
    assert!(about.get_update_available(), "it is offered the next time the window opens");
}

/// The import and export window, open, over a state holding `names` as connections.
fn transfer_open(names: &[&str]) -> (Harness, Rc<crate::app::App>, (tokio::runtime::Runtime, EventLoopTurn)) {
    let state = test_state();
    {
        let mut store = state.connections.lock_recover();
        for (i, name) in names.iter().enumerate() {
            let payload = ConnectionPayloadInput { name: Some(name.to_string()), connection_url: Some(format!("redis://h{i}:6379")), ..Default::default() };
            store.create_with_id(&format!("c{i}"), payload).unwrap();
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
    let names: Vec<String> = app.state.connections.lock_recover().list_state().connections.into_iter().map(|c| c.name).collect();
    assert!(names.contains(&"Prod (2)".to_string()) && names.contains(&"Cache".to_string()), "{names:?}");
    assert!(!names.contains(&"Skipped".to_string()), "an unticked row isn't imported");
    let listed = h.window.global::<crate::ConnectionState>().get_connections().row_count();
    assert_eq!(listed, names.len(), "the connection list shows them");
}
